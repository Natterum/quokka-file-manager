//! Remote file systems (SFTP, FTP, FTPS) behind one interface. Remote items are
//! addressed by `sftp://user@host:port/path`, `ftp://…` or `ftps://…` (see
//! [`quokka_domain::remote`]); [`crate::stdfs::StdFs`] routes such paths here, and
//! [`crate::xfer`] moves data between them and local folders.
//!
//! Every call blocks the calling thread — callers are background threads (listing,
//! jobs, the connect dialog's worker), never the UI thread.
//!
//! Logins are kept in memory for the life of the process, so a dropped connection is
//! re-established transparently on the next call.

use crate::{ftp, sftp};
use quokka_core::ports::Size;
use quokka_domain::entry::Meta;
use quokka_domain::remote::{self, Protocol, Site};
use quokka_domain::{tr, trf, Entry};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

/// How to log in. SFTP tries the key file (or, without one, the usual `~/.ssh/id_*`
/// keys), then the password; FTP uses the password only.
#[derive(Clone, Default)]
pub struct Login {
    pub password: Option<String>,
    pub key_file: Option<PathBuf>,
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    /// The server's key is not in `~/.ssh/known_hosts`: ask the user, then call
    /// [`connect`] again with `trust` set to this fingerprint.
    UnknownHost { fingerprint: String },
    /// The key differs from the recorded one — possibly an attack; never accepted here.
    HostKeyChanged { fingerprint: String, line: usize },
    AuthFailed,
    Failed(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::UnknownHost { fingerprint } => write!(f, "{}", trf!("Неизвестный ключ сервера {fingerprint}", "Unknown server key {fingerprint}")),
            ConnectError::HostKeyChanged { fingerprint, line } => write!(f, "{}", trf!(
                "Ключ сервера изменился ({fingerprint}) и не совпадает с записью в known_hosts (строка {line}). Подключение отклонено.",
                "The server key changed ({fingerprint}) and does not match known_hosts (line {line}). Connection refused.")),
            ConnectError::AuthFailed => write!(f, "{}", tr!("Неверный логин, пароль или ключ", "Wrong user name, password or key")),
            ConnectError::Failed(e) => write!(f, "{e}"),
        }
    }
}

/// Log in to `site` and keep the connection; returns the folder to open (the remote
/// home). `trust` — an SFTP key fingerprint the user accepted after
/// [`ConnectError::UnknownHost`].
pub fn connect(site: &Site, login: Login, trust: Option<String>) -> Result<PathBuf, ConnectError> {
    match site.protocol {
        Protocol::Sftp => sftp::connect(site, login, trust),
        Protocol::Ftp | Protocol::Ftps => ftp::connect(site, login),
    }
}

pub fn disconnect(site: &Site) {
    match site.protocol {
        Protocol::Sftp => sftp::disconnect(site),
        Protocol::Ftp | Protocol::Ftps => ftp::disconnect(site),
    }
}

pub fn is_connected(site: &Site) -> bool {
    match site.protocol {
        Protocol::Sftp => sftp::is_connected(site),
        Protocol::Ftp | Protocol::Ftps => ftp::is_connected(site),
    }
}

pub(crate) fn not_connected(site: &Site) -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, trf!("Нет подключения к {}. Подключитесь заново (Ctrl+K).", "Not connected to {}. Connect again (Ctrl+K).", site.url()))
}

pub(crate) fn child_path(dir: &str, name: &str) -> String {
    if dir == "/" { format!("/{name}") } else { format!("{dir}/{name}") }
}

/// What a remote item is.
#[derive(Clone, Debug, Default)]
pub struct Stat {
    pub is_dir: bool,
    /// A symbolic link (`is_dir` then tells what it points to, when known).
    pub is_link: bool,
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    /// `drwxr-xr-x` style permissions, when the server tells them.
    pub mode: Option<String>,
}

impl Stat {
    fn meta(&self, name: &str) -> Meta {
        Meta {
            is_dir: self.is_dir,
            size: if self.is_dir { 0 } else { self.len },
            modified: self.modified,
            created: None,
            accessed: self.accessed,
            attributes: self.mode.clone(),
            hidden: name.starts_with('.'),
        }
    }
}

/// A file being written to a server; [`RemoteWriter::finish`] reports whether the
/// server stored it.
pub trait RemoteWriter: Write + Send {
    fn finish(self: Box<Self>) -> io::Result<()>;
}

/// A remote location resolved to its connection.
pub enum Node {
    Sftp(sftp::Remote),
    Ftp(ftp::Remote),
}

pub fn resolve(path: &Path) -> io::Result<Node> {
    let (site, p) = remote::split(path).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, path.display().to_string()))?;
    Ok(match site.protocol {
        Protocol::Sftp => Node::Sftp(sftp::Remote::open(site, p)?),
        Protocol::Ftp | Protocol::Ftps => Node::Ftp(ftp::Remote::open(site, p)?),
    })
}

macro_rules! each {
    ($self:expr, $r:ident => $e:expr) => {
        match $self {
            Node::Sftp($r) => $e,
            Node::Ftp($r) => $e,
        }
    };
}

impl Node {
    pub fn site(&self) -> &Site {
        each!(self, r => &r.site)
    }

    pub fn path(&self) -> &str {
        each!(self, r => &r.path)
    }

    pub fn local_path(&self) -> PathBuf {
        self.site().path(self.path())
    }

    pub fn child(&self, name: &str) -> Node {
        match self {
            Node::Sftp(r) => Node::Sftp(r.child(name)),
            Node::Ftp(r) => Node::Ftp(r.child(name)),
        }
    }

    pub fn same_server(&self, other: &Node) -> bool {
        self.site() == other.site()
    }

    /// Metadata following links; `None` if nothing is there.
    pub fn stat(&self) -> io::Result<Option<Stat>> {
        each!(self, r => r.stat())
    }

    /// Metadata of the item itself (`is_link` set for links).
    pub fn lstat(&self) -> io::Result<Option<Stat>> {
        each!(self, r => r.lstat())
    }

    /// Children with their metadata; links are followed so linked folders open.
    pub fn list(&self, cancel: &AtomicBool) -> io::Result<Vec<(String, Stat)>> {
        each!(self, r => r.list(cancel))
    }

    pub fn mkdir(&self) -> io::Result<()> {
        each!(self, r => r.mkdir())
    }

    pub fn mkdir_all(&self) -> io::Result<()> {
        let mut cur = Vec::new();
        let root = self.local_path();
        let (site, _) = remote::split(&root).expect("a remote path");
        for seg in self.path().split('/').filter(|s| !s.is_empty()) {
            cur.push(seg);
            let node = resolve(&site.path(&format!("/{}", cur.join("/"))))?;
            match node.stat()? {
                Some(s) if s.is_dir => {}
                Some(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, node.path().to_string())),
                None => node.mkdir()?,
            }
        }
        Ok(())
    }

    pub fn remove_file(&self) -> io::Result<()> {
        each!(self, r => r.remove_file())
    }

    pub fn remove_dir(&self) -> io::Result<()> {
        each!(self, r => r.remove_dir())
    }

    /// Rename within one server.
    pub fn rename(&self, to: &Node) -> io::Result<()> {
        match (self, to) {
            (Node::Sftp(a), Node::Sftp(b)) if a.site == b.site => a.rename(b),
            (Node::Ftp(a), Node::Ftp(b)) if a.site == b.site => a.rename(b),
            _ => Err(io::Error::new(io::ErrorKind::Unsupported, tr!("Переименование между серверами невозможно", "Cannot rename across servers"))),
        }
    }

    /// Create an empty file; fails if something is already there.
    pub fn create_new(&self) -> io::Result<()> {
        each!(self, r => r.create_new())
    }

    pub fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
        each!(self, r => r.reader())
    }

    pub fn writer(&self) -> io::Result<Box<dyn RemoteWriter>> {
        each!(self, r => r.writer())
    }

    /// (total, free) bytes, if the server tells.
    pub fn space(&self) -> Option<(u64, u64)> {
        match self {
            Node::Sftp(r) => r.space(),
            Node::Ftp(_) => None,
        }
    }
}

// ───────────────────────────── FileSystem routing ─────────────────────────────

pub fn read_dir(dir: &Path, cancel: &AtomicBool) -> io::Result<Vec<Entry>> {
    let node = resolve(dir)?;
    let items = node.list(cancel)?;
    let site = node.site().clone();
    Ok(items
        .into_iter()
        .map(|(name, st)| {
            let path = site.path(&child_path(node.path(), &name));
            Entry::item(name.clone(), &name, path, st.meta(&name))
        })
        .collect())
}

pub fn stat(path: &Path) -> Option<Meta> {
    let st = resolve(path).ok()?.stat().ok()??;
    Some(st.meta(&remote::file_name(path).unwrap_or_default()))
}

pub fn check_dir(dir: &Path) -> Result<(), String> {
    let shown = remote::display(dir).unwrap_or_else(|| dir.display().to_string());
    let node = resolve(dir).map_err(|e| e.to_string())?;
    match node.stat() {
        Ok(Some(s)) if s.is_dir => Ok(()),
        Ok(Some(_)) => Err(trf!("{shown}: это не папка", "{shown}: not a folder")),
        Ok(None) => Err(trf!("{shown}: папка не найдена", "{shown}: folder not found")),
        Err(e) => Err(format!("{shown}: {e}")),
    }
}

pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    resolve(path).ok()?.space()
}

pub fn tree_size(root: &Path, cancel: &AtomicBool) -> Option<Size> {
    let Ok(node) = resolve(root) else { return Some(Size { bytes: 0, skipped: 1 }) };
    let mut size = Size::default();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        match n.lstat() {
            Ok(Some(s)) if s.is_link => size.skipped += 1,
            Ok(Some(s)) if s.is_dir => match n.list(cancel) {
                Ok(items) => {
                    for (name, st) in items {
                        if st.is_link {
                            size.skipped += 1;
                        } else if st.is_dir {
                            stack.push(n.child(&name));
                        } else {
                            size.bytes = size.bytes.saturating_add(st.len);
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => return None,
                Err(_) => size.skipped += 1,
            },
            Ok(Some(s)) => size.bytes = size.bytes.saturating_add(s.len),
            _ => size.skipped += 1,
        }
    }
    Some(size)
}
