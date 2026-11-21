//! FTP and FTPS (explicit TLS, `AUTH TLS`) over suppaftp. One control connection
//! per server answers listings and small commands; every file transfer opens its own
//! connection, so a long upload never blocks browsing. TLS goes through Windows'
//! own SChannel (native-tls), with the usual certificate checks.

use crate::net::{child_path as child, not_connected, ConnectError, Login, RemoteWriter, Stat};
use quokka_domain::remote::{Protocol, Site};
use quokka_domain::{tr, trf};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use suppaftp::list::{File as ListItem, ListParser};
use suppaftp::types::FileType;
use suppaftp::{FtpError, ImplFtpStream, NativeTlsConnector, NativeTlsFtpStream, Status, TlsStream, TransferStream};

type Stream = NativeTlsFtpStream;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

struct Conn {
    site: Site,
    login: Login,
    /// The control connection for listings and commands; `None` after it broke.
    control: Mutex<Option<Stream>>,
    /// The server understands MLSD/MLST (exact, machine-readable listings).
    mlsd: bool,
}

fn registry() -> &'static Mutex<HashMap<Site, Arc<Conn>>> {
    static REG: OnceLock<Mutex<HashMap<Site, Arc<Conn>>>> = OnceLock::new();
    REG.get_or_init(Default::default)
}

fn failed(e: impl std::fmt::Display) -> ConnectError {
    ConnectError::Failed(e.to_string())
}

/// A fresh, logged-in connection in binary mode.
fn open_stream(site: &Site, login: &Login) -> Result<Stream, ConnectError> {
    let addr = (site.host.as_str(), site.port)
        .to_socket_addrs()
        .map_err(|e| ConnectError::Failed(format!("{}: {e}", site.host)))?
        .next()
        .ok_or_else(|| ConnectError::Failed(trf!("{}: адрес не найден", "{}: address not found", site.host)))?;
    let mut ftp = Stream::connect_timeout(addr, CONNECT_TIMEOUT).map_err(|e| match e {
        FtpError::ConnectionError(io) if io.kind() == io::ErrorKind::TimedOut => {
            ConnectError::Failed(trf!("{}: сервер не отвечает", "{}: the server does not respond", site.host))
        }
        e => ConnectError::Failed(format!("{}: {e}", site.host)),
    })?;
    if site.protocol == Protocol::Ftps {
        let tls = native_tls::TlsConnector::new().map_err(failed)?;
        ftp = ftp.into_secure(NativeTlsConnector::from(tls), &site.host).map_err(|e| {
            ConnectError::Failed(trf!("Не удалось включить TLS: {e}", "Could not start TLS: {e}"))
        })?;
    }
    let password = login.password.clone().unwrap_or_default();
    match ftp.login(site.user.as_str(), password.as_str()) {
        Ok(()) => {}
        Err(FtpError::UnexpectedResponse(r)) if matches!(r.status, Status::NotLoggedIn | Status::NeedPassword | Status::LoginNeedAccount) => {
            return Err(ConnectError::AuthFailed)
        }
        Err(e) => return Err(failed(e)),
    }
    ftp.transfer_type(FileType::Binary).map_err(failed)?;
    // names are UTF-8 on any modern server; older ones simply refuse the option
    let _ = ftp.opts("UTF8", Some("ON"));
    Ok(ftp)
}

pub fn connect(site: &Site, login: Login) -> Result<PathBuf, ConnectError> {
    let mut ftp = open_stream(site, &login)?;
    let home = ftp.pwd().unwrap_or_else(|_| "/".into());
    let mlsd = ftp.feat().map(|f| f.contains_key("MLST")).unwrap_or(false);
    let conn = Conn { site: site.clone(), login, control: Mutex::new(Some(ftp)), mlsd };
    registry().lock().unwrap().insert(site.clone(), Arc::new(conn));
    Ok(site.path(if home.starts_with('/') { &home } else { "/" }))
}

pub fn disconnect(site: &Site) {
    let conn = registry().lock().unwrap().remove(site);
    if let Some(c) = conn {
        if let Some(mut ftp) = c.control.lock().unwrap().take() {
            let _ = ftp.quit();
        }
    }
}

pub fn is_connected(site: &Site) -> bool {
    registry().lock().unwrap().contains_key(site)
}

fn to_io(e: FtpError) -> io::Error {
    match e {
        FtpError::ConnectionError(io) => io,
        FtpError::UnexpectedResponse(r) => {
            let text = String::from_utf8_lossy(&r.body).trim().to_string();
            let kind = match r.status {
                Status::FileUnavailable => io::ErrorKind::NotFound,
                Status::BadFilename | Status::NotLoggedIn => io::ErrorKind::PermissionDenied,
                _ => io::ErrorKind::Other,
            };
            io::Error::new(kind, if text.is_empty() { r.status.to_string() } else { text })
        }
        e => io::Error::other(e.to_string()),
    }
}

/// The connection is gone (not just a refused command): worth one reconnect.
fn broken(e: &FtpError) -> bool {
    matches!(e, FtpError::ConnectionError(_) | FtpError::BadResponse)
        || matches!(e, FtpError::UnexpectedResponse(r) if r.status.code() == 421)
}

impl Conn {
    /// Run `f` on the control connection, reconnecting once if it broke.
    fn with<R>(&self, mut f: impl FnMut(&mut Stream) -> Result<R, FtpError>) -> io::Result<R> {
        let mut guard = self.control.lock().unwrap();
        for attempt in 0..2 {
            if guard.is_none() {
                *guard = Some(open_stream(&self.site, &self.login).map_err(|e| io::Error::new(io::ErrorKind::NotConnected, e.to_string()))?);
            }
            match f(guard.as_mut().expect("just connected")) {
                Ok(v) => return Ok(v),
                Err(e) if attempt == 0 && broken(&e) => *guard = None,
                Err(e) => return Err(to_io(e)),
            }
        }
        unreachable!()
    }
}

fn session(site: &Site) -> io::Result<Arc<Conn>> {
    registry().lock().unwrap().get(site).cloned().ok_or_else(|| not_connected(site))
}

fn stat_of(item: &ListItem) -> Stat {
    Stat {
        is_dir: item.is_directory(),
        is_link: item.is_symlink(),
        len: item.size() as u64,
        modified: Some(item.modified()).filter(|t| *t > std::time::UNIX_EPOCH),
        accessed: None,
        mode: None,
    }
}

fn parent_and_name(path: &str) -> (String, String) {
    match path.rfind('/') {
        Some(0) => ("/".into(), path[1..].to_string()),
        Some(i) => (path[..i].to_string(), path[i + 1..].to_string()),
        None => ("/".into(), path.to_string()),
    }
}

/// An FTP location resolved to its connection.
pub struct Remote {
    conn: Arc<Conn>,
    pub site: Site,
    pub path: String,
}

impl Remote {
    pub fn open(site: Site, path: String) -> io::Result<Remote> {
        let conn = session(&site)?;
        Ok(Remote { conn, site, path })
    }

    pub fn child(&self, name: &str) -> Remote {
        Remote { conn: self.conn.clone(), site: self.site.clone(), path: child(&self.path, name) }
    }

    /// Raw listing of `dir`: MLSD when the server has it, LIST otherwise.
    fn raw_list(&self, dir: &str) -> io::Result<Vec<(String, Stat)>> {
        let mlsd = self.conn.mlsd;
        let lines = self.conn.with(|ftp| if mlsd { ftp.mlsd(Some(dir)) } else { ftp.list(Some(dir)) })?;
        let mut out = Vec::new();
        for line in lines {
            let parsed = if mlsd { ListParser::parse_mlsd(&line) } else { line.parse::<ListItem>() };
            let Ok(item) = parsed else { continue };
            // MLSD also reports the folder itself and its parent
            let name = item.name().rsplit('/').next().unwrap_or("").to_string();
            let lower = line.to_ascii_lowercase();
            let self_or_parent = mlsd && (lower.contains("type=cdir") || lower.contains("type=pdir"));
            if name.is_empty() || name == "." || name == ".." || self_or_parent {
                continue;
            }
            out.push((name, stat_of(&item)));
        }
        Ok(out)
    }

    /// Whether `path` opens as a folder (how links are followed over FTP).
    fn opens_as_dir(&self, path: &str) -> bool {
        self.conn.with(|ftp| ftp.cwd(path)).is_ok()
    }

    pub fn lstat(&self) -> io::Result<Option<Stat>> {
        if self.path == "/" {
            return Ok(Some(Stat { is_dir: true, ..Stat::default() }));
        }
        let (parent, name) = parent_and_name(&self.path);
        match self.raw_list(&parent) {
            Ok(items) => {
                let found = items.into_iter().find(|(n, _)| *n == name).map(|(_, s)| s);
                Ok(match found {
                    Some(mut s) if s.is_link => {
                        s.is_dir = self.opens_as_dir(&self.path);
                        Some(s)
                    }
                    other => other,
                })
            }
            // the parent cannot be listed (no permission): ask about the item itself
            Err(_) if self.opens_as_dir(&self.path) => Ok(Some(Stat { is_dir: true, ..Stat::default() })),
            Err(_) => match self.conn.with(|ftp| ftp.size(&self.path)) {
                Ok(n) => Ok(Some(Stat { len: n as u64, ..Stat::default() })),
                Err(_) => Ok(None),
            },
        }
    }

    pub fn stat(&self) -> io::Result<Option<Stat>> {
        Ok(self.lstat()?.map(|mut s| {
            s.is_link = false;
            s
        }))
    }

    pub fn list(&self, cancel: &AtomicBool) -> io::Result<Vec<(String, Stat)>> {
        let mut items = self.raw_list(&self.path)?;
        for (name, st) in &mut items {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if st.is_link {
                st.is_dir = self.opens_as_dir(&child(&self.path, name));
            }
        }
        Ok(items)
    }

    pub fn mkdir(&self) -> io::Result<()> {
        self.conn.with(|ftp| ftp.mkdir(&self.path))
    }

    pub fn remove_file(&self) -> io::Result<()> {
        self.conn.with(|ftp| ftp.rm(&self.path))
    }

    pub fn remove_dir(&self) -> io::Result<()> {
        self.conn.with(|ftp| ftp.rmdir(&self.path))
    }

    pub fn rename(&self, to: &Remote) -> io::Result<()> {
        self.conn.with(|ftp| ftp.rename(&self.path, &to.path))
    }

    pub fn create_new(&self) -> io::Result<()> {
        if self.lstat()?.is_some() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, tr!("Файл уже существует", "The file already exists")));
        }
        self.writer()?.finish()
    }

    fn transfer_conn(&self) -> io::Result<Stream> {
        open_stream(&self.site, &self.conn.login).map_err(|e| io::Error::new(io::ErrorKind::NotConnected, e.to_string()))
    }

    pub fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
        let mut ftp = self.transfer_conn()?;
        let data = ftp.retr_as_stream(&self.path).map_err(to_io)?;
        Ok(Box::new(FtpFile { data: Some(data), ftp: Some(ftp) }))
    }

    pub fn writer(&self) -> io::Result<Box<dyn RemoteWriter>> {
        let mut ftp = self.transfer_conn()?;
        let data = ftp.put_with_stream(&self.path).map_err(to_io)?;
        Ok(Box::new(FtpFile { data: Some(data), ftp: Some(ftp) }))
    }
}

/// One file transfer with its own connection.
struct FtpFile<T: TlsStream> {
    data: Option<TransferStream<T>>,
    ftp: Option<ImplFtpStream<T>>,
}

impl<T: TlsStream + Send + 'static> Read for FtpFile<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.data.as_mut().ok_or(io::ErrorKind::BrokenPipe)?.read(buf)
    }
}

impl<T: TlsStream + Send + 'static> Write for FtpFile<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.data.as_mut().ok_or(io::ErrorKind::BrokenPipe)?.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.data.as_mut().ok_or(io::ErrorKind::BrokenPipe)?.flush()
    }
}

impl<T: TlsStream + Send + 'static> RemoteWriter for FtpFile<T> {
    fn finish(mut self: Box<Self>) -> io::Result<()> {
        let result = match self.data.take() {
            Some(data) => data.finish().map_err(to_io),
            None => Ok(()),
        };
        if let Some(mut ftp) = self.ftp.take() {
            let _ = ftp.quit();
        }
        result
    }
}

impl<T: TlsStream> Drop for FtpFile<T> {
    fn drop(&mut self) {
        // a reader stopped early (cancelled, or only the start was needed): just hang up
        drop(self.data.take());
        if let Some(mut ftp) = self.ftp.take() {
            let _ = ftp.quit();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::net::{check_dir, read_dir, stat, tree_size};
    use std::fs;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn browse_and_stat() {
        let (root, site) = crate::ftp_test_server::served("browse");
        fs::create_dir_all(root.join("home/папка")).unwrap();
        fs::write(root.join("home/a.txt"), b"hello").unwrap();
        let home = site.path("/home");
        let mut names: Vec<(String, bool, u64)> = read_dir(&home, &AtomicBool::new(false)).unwrap()
            .into_iter().map(|e| (e.name, e.is_dir, e.size)).collect();
        names.sort();
        assert_eq!(names, [("a.txt".into(), false, 5), ("папка".into(), true, 0)]);
        assert!(check_dir(&home).is_ok());
        assert!(check_dir(&site.path("/")).is_ok());
        assert!(check_dir(&home.join("a.txt")).is_err());
        assert!(check_dir(&site.path("/nope")).is_err());
        assert_eq!(stat(&home.join("a.txt")).unwrap().size, 5);
        assert!(stat(&site.path("/home/missing")).is_none());
        assert_eq!(tree_size(&site.path("/"), &AtomicBool::new(false)).unwrap().bytes, 5);
        crate::net::disconnect(&site);
        assert!(read_dir(&home, &AtomicBool::new(false)).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
