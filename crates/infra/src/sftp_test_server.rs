//! A tiny real SFTP server for tests: russh + russh-sftp serving a local folder.
//! User `tester`, password `pw`, or the client key in [`CLIENT_KEY`] (passphrase `secret`).

use russh::keys::{self, PublicKey};
use russh::server::{Auth, ChannelOpenHandle, Msg, PublicKeyOffer, Server as _, Session, VerifiedPublicKey};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{Attrs, Data, File as Item, FileAttributes, Handle, Name, OpenFlags, SftpHandle, Status, StatusCode};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

const HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACASMh4AFqXEABvaUi9aZFWJpuQMYPjB6VnSdv9Tk0pXcgAAAJB66MvLeujL
ywAAAAtzc2gtZWQyNTUxOQAAACASMh4AFqXEABvaUi9aZFWJpuQMYPjB6VnSdv9Tk0pXcg
AAAECBWYGos5SZCwkQLXCVFwUTi97aRWUymuv+sYzcDjW3gBIyHgAWpcQAG9pSL1pkVYmm
5Axg+MHpWdJ2/1OTSldyAAAAC3F1b2trYS10ZXN0AQI=
-----END OPENSSH PRIVATE KEY-----
";

pub const CLIENT_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABDqUPjvgG
nk5arD6UFJMzMYAAAAGAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIPp+sv1U8Bx6Yq9A
dVz60h/z47A9c+a0MYaruN6Nh38wAAAAkKb227q+enx5pXJE0G9cGJcpDLHROWyBgRrRjy
YsihwpF4/jecCbUaj8EXw9VN5OQBThozsOe0v+wktWYGkJaFh2ilNGIqSpd20VC69nyUIy
P/VeLhQ77i9KGCCaKv9nxKZ77wSM5oaTSSVfF+t+C+2Kl6fgM5vK5XuAWYJB1KFFMUDlPD
rRIFx8k+yBWVrXhw==
-----END OPENSSH PRIVATE KEY-----
";
const CLIENT_PUB: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPp+sv1U8Bx6Yq9AdVz60h/z47A9c+a0MYaruN6Nh38w";

/// Start a server for `root` on a free local port; returns the port. The server
/// runs on the shared SSH runtime until the process exits.
pub fn start(root: PathBuf) -> u16 {
    let rt = super::sftp::test_runtime();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Arc::new(russh::server::Config {
        keys: vec![keys::decode_secret_key(HOST_KEY, None).unwrap()],
        auth_rejection_time: std::time::Duration::from_millis(10),
        auth_rejection_time_initial: Some(std::time::Duration::ZERO),
        ..Default::default()
    });
    rt.spawn(async move {
        let mut server = Server { root };
        let _ = server.run_on_socket(config, &listener).await;
    });
    port
}

fn key_auth(user: &str, key: &PublicKey) -> Auth {
    let ok = user == "tester" && PublicKey::from_openssh(CLIENT_PUB).is_ok_and(|k| k.key_data() == key.key_data());
    if ok { Auth::Accept } else { Auth::reject() }
}

#[derive(Clone)]
struct Server {
    root: PathBuf,
}

impl russh::server::Server for Server {
    type Handler = SshSession;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> SshSession {
        SshSession { root: self.root.clone(), channels: Default::default() }
    }
}

struct SshSession {
    root: PathBuf,
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

impl russh::server::Handler for SshSession {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == "tester" && password == "pw" { Auth::Accept } else { Auth::reject() })
    }

    async fn auth_publickey_offered(&mut self, user: &str, offer: PublicKeyOffer<'_>) -> Result<Auth, Self::Error> {
        Ok(key_auth(user, offer.public_key))
    }

    async fn auth_publickey(&mut self, user: &str, key: VerifiedPublicKey<'_>) -> Result<Auth, Self::Error> {
        Ok(key_auth(user, key.public_key))
    }

    async fn channel_open_session(&mut self, channel: Channel<Msg>, reply: ChannelOpenHandle, _: &mut Session) -> Result<(), Self::Error> {
        self.channels.lock().await.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn channel_eof(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        session.close(channel)?;
        Ok(())
    }

    async fn subsystem_request(&mut self, id: ChannelId, name: &str, session: &mut Session) -> Result<(), Self::Error> {
        let channel = self.channels.lock().await.remove(&id);
        match channel {
            Some(channel) if name == "sftp" => {
                session.channel_success(id)?;
                let sftp = Sftp { root: self.root.clone(), handles: HashMap::new(), next: 0 };
                tokio::spawn(russh_sftp::server::run(channel.into_stream(), sftp));
            }
            _ => session.channel_failure(id)?,
        }
        Ok(())
    }
}

enum Open {
    File(fs::File),
    Dir(Option<Vec<Item>>),
}

struct Sftp {
    root: PathBuf,
    handles: HashMap<SftpHandle, Open>,
    next: u64,
}

fn ok(id: u32) -> Status {
    Status { id, status_code: StatusCode::Ok, error_message: "Ok".into(), language_tag: "en-US".into() }
}

fn code(e: std::io::Error) -> StatusCode {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

fn attrs(md: &fs::Metadata) -> FileAttributes {
    let mut a = FileAttributes::from(md);
    a.permissions = Some(if md.is_dir() { 0o40755 } else { 0o100644 });
    a
}

/// `/a/b` → `root/a/b`, with `..` kept inside the root.
fn normalize(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

impl Sftp {
    fn local(&self, p: &str) -> PathBuf {
        let p = if p.starts_with('/') { normalize(p) } else { normalize(&format!("/home/{p}")) };
        p.split('/').filter(|s| !s.is_empty()).fold(self.root.clone(), |acc, s| acc.join(s))
    }

    fn handle(&mut self, open: Open) -> SftpHandle {
        self.next += 1;
        let h = SftpHandle::from(self.next.to_string());
        self.handles.insert(h.clone(), open);
        h
    }
}

fn stat_of(p: &Path) -> Result<FileAttributes, StatusCode> {
    fs::metadata(p).map(|m| attrs(&m)).map_err(code)
}

impl russh_sftp::server::Handler for Sftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> StatusCode {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _: u32, _: HashMap<String, String>) -> Result<russh_sftp::protocol::Version, StatusCode> {
        Ok(russh_sftp::protocol::Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, StatusCode> {
        let abs = if path.starts_with('/') { normalize(&path) } else { normalize(&format!("/home/{path}")) };
        Ok(Name { id, files: vec![Item::dummy(abs)] })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        Ok(Attrs { id, attrs: stat_of(&self.local(&path))? })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        Ok(Attrs { id, attrs: stat_of(&self.local(&path))? })
    }

    async fn fstat(&mut self, id: u32, handle: SftpHandle) -> Result<Attrs, StatusCode> {
        match self.handles.get(&handle) {
            Some(Open::File(f)) => Ok(Attrs { id, attrs: f.metadata().map(|m| attrs(&m)).map_err(code)? }),
            _ => Err(StatusCode::Failure),
        }
    }

    async fn setstat(&mut self, id: u32, _: String, _: FileAttributes) -> Result<Status, StatusCode> {
        Ok(ok(id))
    }

    async fn fsetstat(&mut self, id: u32, _: SftpHandle, _: FileAttributes) -> Result<Status, StatusCode> {
        Ok(ok(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, StatusCode> {
        let dir = self.local(&path);
        let mut items = Vec::new();
        for e in fs::read_dir(&dir).map_err(code)? {
            let e = e.map_err(code)?;
            let md = e.metadata().map_err(code)?;
            items.push(Item::new(e.file_name().to_string_lossy().into_owned(), attrs(&md)));
        }
        let handle = self.handle(Open::Dir(Some(items)));
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: SftpHandle) -> Result<Name, StatusCode> {
        match self.handles.get_mut(&handle) {
            Some(Open::Dir(items)) => match items.take() {
                Some(files) => Ok(Name { id, files }),
                None => Err(StatusCode::Eof),
            },
            _ => Err(StatusCode::Failure),
        }
    }

    async fn open(&mut self, id: u32, filename: String, flags: OpenFlags, _: FileAttributes) -> Result<Handle, StatusCode> {
        let p = self.local(&filename);
        let f = fs::OpenOptions::new()
            .read(flags.contains(OpenFlags::READ))
            .write(flags.contains(OpenFlags::WRITE))
            .append(flags.contains(OpenFlags::APPEND))
            .create(flags.contains(OpenFlags::CREATE) && !flags.contains(OpenFlags::EXCLUDE))
            .create_new(flags.contains(OpenFlags::CREATE) && flags.contains(OpenFlags::EXCLUDE))
            .truncate(flags.contains(OpenFlags::TRUNCATE))
            .open(p)
            .map_err(code)?;
        let handle = self.handle(Open::File(f));
        Ok(Handle { id, handle })
    }

    async fn read(&mut self, id: u32, handle: SftpHandle, offset: u64, len: u32) -> Result<Data, StatusCode> {
        let Some(Open::File(f)) = self.handles.get_mut(&handle) else { return Err(StatusCode::Failure) };
        f.seek(SeekFrom::Start(offset)).map_err(code)?;
        let mut data = vec![0; len as usize];
        let n = f.read(&mut data).map_err(code)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        data.truncate(n);
        Ok(Data { id, data })
    }

    async fn write(&mut self, id: u32, handle: SftpHandle, offset: u64, data: Vec<u8>) -> Result<Status, StatusCode> {
        let Some(Open::File(f)) = self.handles.get_mut(&handle) else { return Err(StatusCode::Failure) };
        f.seek(SeekFrom::Start(offset)).map_err(code)?;
        f.write_all(&data).map_err(code)?;
        Ok(ok(id))
    }

    async fn close(&mut self, id: u32, handle: SftpHandle) -> Result<Status, StatusCode> {
        self.handles.remove(&handle);
        Ok(ok(id))
    }

    async fn mkdir(&mut self, id: u32, path: String, _: FileAttributes) -> Result<Status, StatusCode> {
        fs::create_dir(self.local(&path)).map_err(code)?;
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, StatusCode> {
        fs::remove_dir(self.local(&path)).map_err(code)?;
        Ok(ok(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, StatusCode> {
        fs::remove_file(self.local(&filename)).map_err(code)?;
        Ok(ok(id))
    }

    async fn rename(&mut self, id: u32, from: String, to: String) -> Result<Status, StatusCode> {
        let to = self.local(&to);
        if to.exists() {
            return Err(StatusCode::Failure);
        }
        fs::rename(self.local(&from), to).map_err(code)?;
        Ok(ok(id))
    }
}
