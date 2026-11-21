//! SFTP over russh. The SSH stack is async; one small tokio runtime runs it, and
//! every call here blocks the calling thread (see [`crate::net`]).

use crate::net::{child_path as child, not_connected, ConnectError, Login, RemoteWriter, Stat};
use quokka_domain::remote::Site;
use quokka_domain::{tr, trf};
use russh::keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::fs::{File, Metadata};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{OpenFlags, StatusCode};
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT_SECS: u64 = 30;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("quokka-ssh")
            .enable_io()
            .enable_time()
            .build()
            .expect("tokio runtime")
    })
}

fn block<F: Future>(f: F) -> F::Output {
    runtime().block_on(f)
}

// ───────────────────────────── host keys ─────────────────────────────

#[derive(Clone, Debug)]
enum HostCheck {
    Unknown(String),
    Changed(String, usize),
    Unsupported,
}

struct Client {
    site: Site,
    /// Fingerprint the user agreed to trust (recorded in known_hosts on success).
    trust: Option<String>,
    /// Where to read and record keys; `None` — the user's `~/.ssh/known_hosts`.
    known_hosts: Option<PathBuf>,
    verdict: Arc<Mutex<Option<HostCheck>>>,
}

impl russh::client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            *self.verdict.lock().unwrap() = Some(HostCheck::Unsupported);
            return Ok(false);
        };
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let (host, port) = (self.site.host.as_str(), self.site.port);
        let known = match &self.known_hosts {
            Some(p) => keys::check_known_hosts_path(host, port, key, p),
            None => keys::check_known_hosts(host, port, key),
        };
        match known {
            Ok(true) => Ok(true),
            Err(keys::Error::KeyChanged { line }) => {
                *self.verdict.lock().unwrap() = Some(HostCheck::Changed(fingerprint, line));
                Ok(false)
            }
            // not recorded, or no known_hosts file yet
            _ if self.trust.as_deref() == Some(fingerprint.as_str()) => {
                let _ = match &self.known_hosts {
                    Some(p) => keys::known_hosts::learn_known_hosts_path(host, port, key, p),
                    None => keys::known_hosts::learn_known_hosts(host, port, key),
                };
                Ok(true)
            }
            _ => {
                *self.verdict.lock().unwrap() = Some(HostCheck::Unknown(fingerprint));
                Ok(false)
            }
        }
    }
}

// ───────────────────────────── connections ─────────────────────────────

struct Conn {
    handle: russh::client::Handle<Client>,
    sftp: SftpSession,
    home: String,
}

#[derive(Default)]
struct Registry {
    live: HashMap<Site, Arc<Conn>>,
    logins: HashMap<Site, (Login, Option<PathBuf>)>,
}

fn registry() -> &'static Mutex<Registry> {
    static REG: OnceLock<Mutex<Registry>> = OnceLock::new();
    REG.get_or_init(Default::default)
}

fn default_keys() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) else { return Vec::new() };
    let ssh = PathBuf::from(home).join(".ssh");
    ["id_ed25519", "id_ecdsa", "id_rsa"].iter().map(|n| ssh.join(n)).filter(|p| p.is_file()).collect()
}

async fn open(site: &Site, login: &Login, trust: Option<String>, known_hosts: Option<PathBuf>) -> Result<Conn, ConnectError> {
    let verdict = Arc::new(Mutex::new(None));
    let client = Client { site: site.clone(), trust, known_hosts, verdict: verdict.clone() };
    let config = Arc::new(russh::client::Config {
        inactivity_timeout: None,
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        ..Default::default()
    });
    let connecting = russh::client::connect(config, (site.host.as_str(), site.port), client);
    let mut handle = match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
        Err(_) => return Err(ConnectError::Failed(trf!("{}: сервер не отвечает", "{}: the server does not respond", site.host))),
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            return Err(match verdict.lock().unwrap().take() {
                Some(HostCheck::Unknown(fingerprint)) => ConnectError::UnknownHost { fingerprint },
                Some(HostCheck::Changed(fingerprint, line)) => ConnectError::HostKeyChanged { fingerprint, line },
                Some(HostCheck::Unsupported) => ConnectError::Failed(tr!("Сертификаты хоста не поддерживаются", "Host certificates are not supported").into()),
                None => ConnectError::Failed(format!("{}: {e}", site.host)),
            })
        }
    };
    let failed = |e: russh::Error| ConnectError::Failed(e.to_string());

    let mut ok = false;
    let keys: Vec<PathBuf> = match &login.key_file {
        Some(k) => vec![k.clone()],
        None => default_keys(),
    };
    for path in keys {
        let key = match keys::load_secret_key(&path, login.passphrase.as_deref()) {
            Ok(k) => k,
            // an explicitly chosen key must load; the default ones are just a try
            Err(e) if login.key_file.is_some() => return Err(ConnectError::Failed(trf!("Ключ {}: {e}", "Key {}: {e}", path.display()))),
            Err(_) => continue,
        };
        let hash = handle.best_supported_rsa_hash().await.map_err(failed)?.flatten();
        let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
        if handle.authenticate_publickey(&site.user, key).await.map_err(failed)?.success() {
            ok = true;
            break;
        }
    }
    if !ok {
        if let Some(pw) = &login.password {
            ok = handle.authenticate_password(&site.user, pw).await.map_err(failed)?.success();
            if !ok {
                ok = keyboard_interactive(&mut handle, &site.user, pw).await.map_err(failed)?;
            }
        }
    }
    if !ok {
        return Err(ConnectError::AuthFailed);
    }

    let channel = handle.channel_open_session().await.map_err(failed)?;
    channel.request_subsystem(true, "sftp").await.map_err(failed)?;
    let sftp = SftpSession::new(channel.into_stream()).await.map_err(|e| ConnectError::Failed(e.to_string()))?;
    sftp.set_timeout(REQUEST_TIMEOUT_SECS);
    let home = sftp.canonicalize(".").await.unwrap_or_else(|_| "/".into());
    Ok(Conn { handle, sftp, home })
}

/// Answer every keyboard-interactive prompt with the password (what PAM-based
/// servers with plain password login disabled usually ask for).
async fn keyboard_interactive(handle: &mut russh::client::Handle<Client>, user: &str, password: &str) -> Result<bool, russh::Error> {
    use russh::client::KeyboardInteractiveAuthResponse as R;
    let mut reply = handle.authenticate_keyboard_interactive_start(user, None).await?;
    for _ in 0..5 {
        match reply {
            R::Success => return Ok(true),
            R::Failure { .. } => return Ok(false),
            R::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| password.to_string()).collect();
                reply = handle.authenticate_keyboard_interactive_respond(answers).await?;
            }
        }
    }
    Ok(false)
}

/// Log in to `site` and keep the connection; returns the remote home folder as a
/// path to open. `trust` — a fingerprint the user accepted after [`ConnectError::UnknownHost`].
pub fn connect(site: &Site, login: Login, trust: Option<String>) -> Result<PathBuf, ConnectError> {
    connect_with(site, login, trust, None)
}

/// [`connect`] with a custom known_hosts file (tests).
pub fn connect_with(site: &Site, login: Login, trust: Option<String>, known_hosts: Option<PathBuf>) -> Result<PathBuf, ConnectError> {
    let conn = block(open(site, &login, trust, known_hosts.clone()))?;
    let home = site.path(&conn.home);
    let mut reg = registry().lock().unwrap();
    reg.live.insert(site.clone(), Arc::new(conn));
    reg.logins.insert(site.clone(), (login, known_hosts));
    Ok(home)
}

/// Close the connection and forget the login.
pub fn disconnect(site: &Site) {
    let conn = {
        let mut reg = registry().lock().unwrap();
        reg.logins.remove(site);
        reg.live.remove(site)
    };
    if let Some(c) = conn {
        block(async {
            let _ = c.sftp.close().await;
            let _ = c.handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
        });
    }
}

pub fn is_connected(site: &Site) -> bool {
    registry().lock().unwrap().logins.contains_key(site)
}

/// The live connection to `site`, reconnecting with the remembered login if it dropped.
fn session(site: &Site) -> io::Result<Arc<Conn>> {
    let login = {
        let reg = registry().lock().unwrap();
        if let Some(c) = reg.live.get(site) {
            if !c.handle.is_closed() {
                return Ok(c.clone());
            }
        }
        reg.logins.get(site).cloned()
    };
    let Some((login, known_hosts)) = login else { return Err(not_connected(site)) };
    let conn = Arc::new(block(open(site, &login, None, known_hosts)).map_err(|e| io::Error::new(io::ErrorKind::NotConnected, e.to_string()))?);
    registry().lock().unwrap().live.insert(site.clone(), conn.clone());
    Ok(conn)
}

fn to_io(e: SftpError) -> io::Error {
    let kind = match &e {
        SftpError::Status(s) => match s.status_code {
            StatusCode::NoSuchFile => io::ErrorKind::NotFound,
            StatusCode::PermissionDenied => io::ErrorKind::PermissionDenied,
            StatusCode::Eof => io::ErrorKind::UnexpectedEof,
            StatusCode::OpUnsupported => io::ErrorKind::Unsupported,
            _ => io::ErrorKind::Other,
        },
        SftpError::Timeout => io::ErrorKind::TimedOut,
        _ => io::ErrorKind::Other,
    };
    let text = match &e {
        SftpError::Status(s) if !s.error_message.is_empty() => s.error_message.clone(),
        _ => e.to_string(),
    };
    io::Error::new(kind, text)
}

/// An SFTP location resolved to its connection.
pub struct Remote {
    conn: Arc<Conn>,
    pub site: Site,
    pub path: String,
}

/// `drwxr-xr-x` from POSIX mode bits.
fn mode_text(md: &Metadata) -> Option<String> {
    let mode = md.permissions?;
    let kind = if md.is_dir() { 'd' } else if md.is_symlink() { 'l' } else { '-' };
    let mut s = String::with_capacity(10);
    s.push(kind);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        s.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    Some(s)
}

fn stat_of(md: &Metadata, is_link: bool) -> Stat {
    Stat {
        is_dir: md.is_dir(),
        is_link: is_link || md.is_symlink(),
        len: md.len(),
        modified: md.modified().ok(),
        accessed: md.accessed().ok(),
        mode: mode_text(md),
    }
}

fn not_found_is_none<T>(r: Result<T, SftpError>) -> io::Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) => {
            let e = to_io(e);
            if e.kind() == io::ErrorKind::NotFound { Ok(None) } else { Err(e) }
        }
    }
}

impl Remote {
    pub fn open(site: Site, path: String) -> io::Result<Remote> {
        let conn = session(&site)?;
        Ok(Remote { conn, site, path })
    }

    fn sftp(&self) -> &SftpSession {
        &self.conn.sftp
    }

    pub fn child(&self, name: &str) -> Remote {
        Remote { conn: self.conn.clone(), site: self.site.clone(), path: child(&self.path, name) }
    }

    pub fn stat(&self) -> io::Result<Option<Stat>> {
        let md = not_found_is_none(block(self.sftp().metadata(self.path.clone())))?;
        Ok(md.map(|m| stat_of(&m, false)))
    }

    pub fn lstat(&self) -> io::Result<Option<Stat>> {
        let Some(md) = not_found_is_none(block(self.sftp().symlink_metadata(self.path.clone())))? else { return Ok(None) };
        if !md.is_symlink() {
            return Ok(Some(stat_of(&md, false)));
        }
        // say what the link points to, but keep it marked as a link
        let target = block(self.sftp().metadata(self.path.clone())).ok();
        Ok(Some(stat_of(target.as_ref().unwrap_or(&md), true)))
    }

    pub fn list(&self, cancel: &AtomicBool) -> io::Result<Vec<(String, Stat)>> {
        let items = block(self.sftp().read_dir(self.path.clone())).map_err(to_io)?;
        let mut out = Vec::new();
        for e in items {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let name = e.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let md = e.metadata();
            let st = if md.is_symlink() {
                match block(self.sftp().metadata(child(&self.path, &name))) {
                    Ok(target) => stat_of(&target, true),
                    Err(_) => stat_of(&md, true),
                }
            } else {
                stat_of(&md, false)
            };
            out.push((name, st));
        }
        Ok(out)
    }

    pub fn mkdir(&self) -> io::Result<()> {
        block(self.sftp().create_dir(self.path.clone())).map_err(to_io)
    }

    pub fn remove_file(&self) -> io::Result<()> {
        block(self.sftp().remove_file(self.path.clone())).map_err(to_io)
    }

    pub fn remove_dir(&self) -> io::Result<()> {
        block(self.sftp().remove_dir(self.path.clone())).map_err(to_io)
    }

    pub fn rename(&self, to: &Remote) -> io::Result<()> {
        block(self.sftp().rename(self.path.clone(), to.path.clone())).map_err(to_io)
    }

    pub fn create_new(&self) -> io::Result<()> {
        let flags = OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE;
        let f = block(self.sftp().open_with_flags(self.path.clone(), flags)).map_err(to_io)?;
        block(async move {
            let mut f = f;
            f.shutdown().await
        })
    }

    pub fn reader(&self) -> io::Result<Box<dyn io::Read + Send>> {
        let f = block(self.sftp().open(self.path.clone())).map_err(to_io)?;
        Ok(Box::new(RemoteFile(Some(f))))
    }

    pub fn writer(&self) -> io::Result<Box<dyn RemoteWriter>> {
        let flags = OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE;
        let f = block(self.sftp().open_with_flags(self.path.clone(), flags)).map_err(to_io)?;
        Ok(Box::new(RemoteFile(Some(f))))
    }

    /// (total, free) bytes, if the server supports `statvfs`.
    pub fn space(&self) -> Option<(u64, u64)> {
        let v = block(self.sftp().fs_info(self.path.clone())).ok()??;
        Some((v.blocks.saturating_mul(v.fragment_size), v.blocks_avail.saturating_mul(v.fragment_size)))
    }
}

/// An open remote file with blocking `Read` / `Write`.
pub struct RemoteFile(Option<File>);

impl io::Read for RemoteFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let f = self.0.as_mut().ok_or(io::ErrorKind::BrokenPipe)?;
        block(f.read(buf))
    }
}

impl io::Write for RemoteFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let f = self.0.as_mut().ok_or(io::ErrorKind::BrokenPipe)?;
        block(f.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        let f = self.0.as_mut().ok_or(io::ErrorKind::BrokenPipe)?;
        block(f.flush())
    }
}

impl RemoteWriter for RemoteFile {
    /// Flush and close, reporting errors (dropping closes too, but silently).
    fn finish(mut self: Box<Self>) -> io::Result<()> {
        match self.0.take() {
            Some(mut f) => block(async move { f.shutdown().await }),
            None => Ok(()),
        }
    }
}

impl Drop for RemoteFile {
    fn drop(&mut self) {
        if let Some(mut f) = self.0.take() {
            block(async move {
                let _ = f.shutdown().await;
            });
        }
    }
}

#[cfg(test)]
pub(crate) fn test_runtime() -> &'static tokio::runtime::Runtime {
    runtime()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::net::{check_dir, read_dir, stat, tree_size};
    use quokka_domain::remote::Protocol;
    use std::fs;

    /// A served temp folder (with `home/`) and a logged-in site for it.
    pub(crate) fn served(tag: &str) -> (PathBuf, Site) {
        let root = std::env::temp_dir().join(format!("quokka-sftp-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).unwrap();
        let port = crate::sftp_test_server::start(root.clone());
        let site = Site { protocol: Protocol::Sftp, user: "tester".into(), host: "127.0.0.1".into(), port };
        let kh = root.with_extension("known_hosts");
        let _ = fs::remove_file(&kh);
        let login = Login { password: Some("pw".into()), key_file: None, passphrase: None };
        let fp = match connect_with(&site, login.clone(), None, Some(kh.clone())) {
            Err(ConnectError::UnknownHost { fingerprint }) => fingerprint,
            other => panic!("expected unknown host, got {other:?}"),
        };
        let home = connect_with(&site, login, Some(fp), Some(kh)).unwrap();
        assert_eq!(home, site.path("/home"));
        (root, site)
    }

    #[test]
    fn browse_and_stat() {
        let (root, site) = served("browse");
        fs::create_dir_all(root.join("home/папка")).unwrap();
        fs::write(root.join("home/a.txt"), b"hello").unwrap();
        fs::write(root.join("home/.hidden"), b"").unwrap();
        let home = site.path("/home");
        let mut names: Vec<(String, bool, u64)> = read_dir(&home, &AtomicBool::new(false)).unwrap()
            .into_iter().map(|e| (e.name, e.is_dir, e.size)).collect();
        names.sort();
        assert_eq!(names, [(".hidden".into(), false, 0), ("a.txt".into(), false, 5), ("папка".into(), true, 0)]);
        assert!(check_dir(&home).is_ok());
        assert!(check_dir(&home.join("a.txt")).is_err());
        assert!(check_dir(&site.path("/nope")).is_err());
        assert_eq!(stat(&home.join("a.txt")).unwrap().size, 5);
        assert!(stat(&site.path("/home/missing")).is_none());
        assert_eq!(tree_size(&site.path("/"), &AtomicBool::new(false)).unwrap().bytes, 5);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn known_host_is_remembered_and_wrong_password_fails() {
        let (root, site) = served("auth");
        let kh = root.with_extension("known_hosts");
        // the key is recorded now: no question the second time
        let login = Login { password: Some("pw".into()), ..Login::default() };
        assert!(connect_with(&site, login, None, Some(kh.clone())).is_ok());
        let bad = Login { password: Some("nope".into()), ..Login::default() };
        assert_eq!(connect_with(&site, bad, None, Some(kh.clone())), Err(ConnectError::AuthFailed));
        // a different key for the same host:port is refused outright
        let other = site.clone();
        let text = fs::read_to_string(&kh).unwrap();
        let changed = text.replace(text.split_whitespace().nth(2).unwrap(), "AAAAC3NzaC1lZDI1NTE5AAAAIPp+sv1U8Bx6Yq9AdVz60h/z47A9c+a0MYaruN6Nh38w");
        fs::write(&kh, changed).unwrap();
        let login = Login { password: Some("pw".into()), ..Login::default() };
        assert!(matches!(connect_with(&other, login, None, Some(kh)), Err(ConnectError::HostKeyChanged { .. })));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn key_file_login() {
        let (root, site) = served("key");
        let kh = root.with_extension("known_hosts");
        let key = root.with_extension("key");
        fs::write(&key, crate::sftp_test_server::CLIENT_KEY).unwrap();
        let login = Login { password: None, key_file: Some(key.clone()), passphrase: Some("secret".into()) };
        assert!(connect_with(&site, login, None, Some(kh.clone())).is_ok());
        let wrong = Login { password: None, key_file: Some(key), passphrase: Some("bad".into()) };
        assert!(matches!(connect_with(&site, wrong, None, Some(kh)), Err(ConnectError::Failed(_))));
        let _ = fs::remove_dir_all(root);
    }
}

/// Not a real test: serves a folder over SFTP for trying Quokka by hand.
/// `set QUOKKA_SFTP_ROOT=C:\some\folder` then
/// `cargo test -p quokka-infra serve_sftp_by_hand -- --ignored --nocapture`;
/// log in as `tester` / `pw` at the printed address. Stops after an hour.
#[cfg(test)]
#[test]
#[ignore]
fn serve_sftp_by_hand() {
    let root = std::env::var_os("QUOKKA_SFTP_ROOT").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("quokka-sftp-manual"));
    std::fs::create_dir_all(root.join("home")).unwrap();
    let port = crate::sftp_test_server::start(root.clone());
    println!("SFTP server for {} at sftp://tester@127.0.0.1:{port}  (password: pw)", root.display());
    std::thread::sleep(Duration::from_secs(3600));
}
