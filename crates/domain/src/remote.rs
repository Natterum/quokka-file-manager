//! Remote (SFTP / FTP / FTPS) locations. A remote item is addressed by an ordinary
//! `PathBuf` of the form `sftp://user@host:port/abs/path` (or `ftp://`, `ftps://`), so
//! tabs, history and favorites keep working with paths. Windows sees `sftp:` and
//! `user@host:port` as plain components (not a drive), and `Path::join` may add `\` —
//! both separators are accepted here.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Protocol {
    #[default]
    Sftp,
    Ftp,
    /// FTP with explicit TLS (`AUTH TLS`).
    Ftps,
    /// A phone, camera or player connected over MTP (Windows Portable Devices):
    /// `mtp://Device name/Storage/folder`. Not a server: no login, no port.
    Mtp,
}

impl Protocol {
    pub const ALL: [Protocol; 3] = [Protocol::Sftp, Protocol::Ftp, Protocol::Ftps];

    pub fn scheme(self) -> &'static str {
        match self {
            Protocol::Sftp => "sftp",
            Protocol::Ftp => "ftp",
            Protocol::Ftps => "ftps",
            Protocol::Mtp => "mtp",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Protocol::Sftp => "SFTP",
            Protocol::Ftp => "FTP",
            Protocol::Ftps => "FTPS",
            Protocol::Mtp => "MTP",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Sftp => 22,
            Protocol::Ftp | Protocol::Ftps => 21,
            Protocol::Mtp => 0,
        }
    }

    pub fn from_scheme(s: &str) -> Option<Protocol> {
        Protocol::ALL.into_iter().chain([Protocol::Mtp]).find(|p| p.scheme().eq_ignore_ascii_case(s))
    }
}

/// One server account: what a connection is keyed by.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Site {
    pub protocol: Protocol,
    pub user: String,
    pub host: String,
    pub port: u16,
}

impl Site {
    /// `user@host` or `user@host:port` (the port only when it is not the default).
    pub fn authority(&self) -> String {
        if self.protocol == Protocol::Mtp {
            return self.host.clone();
        }
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        if self.port == self.protocol.default_port() {
            format!("{}@{host}", self.user)
        } else {
            format!("{}@{host}:{}", self.user, self.port)
        }
    }

    /// `sftp://user@host[:port]` — how the server is shown and remembered.
    pub fn url(&self) -> String {
        format!("{}://{}", self.protocol.scheme(), self.authority())
    }

    /// The path of `remote` (an absolute POSIX path) on this site.
    pub fn path(&self, remote: &str) -> PathBuf {
        let rest = remote.trim_start_matches('/');
        PathBuf::from(format!("{}/{rest}", self.url()))
    }

    /// Parse `user@host[:port]` for `protocol`; `[::1]:2222` style hosts are accepted too.
    pub fn parse(protocol: Protocol, authority: &str) -> Option<Site> {
        if protocol == Protocol::Mtp {
            // the device name as Windows shows it
            let name = authority.trim();
            return (!name.is_empty()).then(|| Site { protocol, user: String::new(), host: name.to_string(), port: 0 });
        }
        let (user, hostport) = authority.rsplit_once('@')?;
        let default = protocol.default_port();
        let (host, port) = if let Some(rest) = hostport.strip_prefix('[') {
            let (h, tail) = rest.split_once(']')?;
            let port = match tail.strip_prefix(':') {
                Some(p) => p.parse().ok()?,
                None if tail.is_empty() => default,
                None => return None,
            };
            (h.to_string(), port)
        } else {
            match hostport.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), p.parse().ok()?),
                None => (hostport.to_string(), default),
            }
        };
        if user.is_empty() || host.is_empty() || host.contains(['/', '\\']) {
            return None;
        }
        Some(Site { protocol, user: user.to_string(), host, port })
    }

    /// Parse `sftp://user@host[:port]` (a trailing path is ignored).
    pub fn parse_url(url: &str) -> Option<Site> {
        let (scheme, rest) = url.split_once("://")?;
        let protocol = Protocol::from_scheme(scheme)?;
        let authority = rest.split(['/', '\\']).next()?;
        Site::parse(protocol, authority)
    }
}

/// Protocol and the text of `path` after `scheme://`, with `\` turned into `/` —
/// remote paths are only ever compared and shown in this form.
fn text(path: &Path) -> Option<(Protocol, String)> {
    let s = path.to_str()?.replace('\\', "/");
    let (scheme, rest) = s.split_once("://")?;
    Some((Protocol::from_scheme(scheme)?, rest.to_string()))
}

pub fn is_remote(path: &Path) -> bool {
    text(path).is_some()
}

/// Whether `path` is on a portable device (`mtp://…`).
pub fn is_device(path: &Path) -> bool {
    text(path).is_some_and(|(p, _)| p == Protocol::Mtp)
}

/// The device root (`mtp://Pixel 7`) of a device path.
pub fn device_root(path: &Path) -> Option<PathBuf> {
    split(path).filter(|(s, _)| s.protocol == Protocol::Mtp).map(|(s, _)| s.path("/"))
}

/// Site and absolute POSIX path (always starting with `/`, no trailing slash except
/// for the root, no empty or `.` segments) of a remote path.
pub fn split(path: &Path) -> Option<(Site, String)> {
    let (protocol, rest) = text(path)?;
    let (authority, tail) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest.as_str(), ""),
    };
    let site = Site::parse(protocol, authority)?;
    let mut parts: Vec<&str> = Vec::new();
    for seg in tail.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    Some((site, format!("/{}", parts.join("/"))))
}

/// Same location written the canonical way (`/` separators, no trailing slash).
pub fn normalize(path: &Path) -> Option<PathBuf> {
    split(path).map(|(site, p)| site.path(&p))
}

/// What the address bar shows for a remote path.
pub fn display(path: &Path) -> Option<String> {
    split(path).map(|(site, p)| format!("{}{p}", site.url()))
}

/// Parent folder; `None` for the server root.
pub fn parent(path: &Path) -> Option<PathBuf> {
    let (site, p) = split(path)?;
    if p == "/" {
        return None;
    }
    let cut = p.rfind('/').unwrap_or(0);
    Some(site.path(if cut == 0 { "/" } else { &p[..cut] }))
}

/// Child `name` of the remote folder `dir`.
pub fn join(dir: &Path, name: &str) -> Option<PathBuf> {
    let (site, p) = split(dir)?;
    Some(site.path(&if p == "/" { format!("/{name}") } else { format!("{p}/{name}") }))
}

/// Last segment of a remote path (empty for the root).
pub fn file_name(path: &Path) -> Option<String> {
    let (_, p) = split(path)?;
    Some(p.rsplit('/').next().unwrap_or("").to_string())
}

/// Whether `child` is `ancestor` or lies inside it (same site).
pub fn starts_with(child: &Path, ancestor: &Path) -> bool {
    match (split(child), split(ancestor)) {
        (Some((s1, c)), Some((s2, a))) => s1 == s2 && (a == "/" || c == a || c.starts_with(&format!("{a}/"))),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_paths() {
        let p = Path::new("mtp://Pixel 7/Внутренний общий накопитель/DCIM");
        assert!(is_remote(p) && is_device(p));
        let (site, rest) = split(p).unwrap();
        assert_eq!((site.protocol, site.host.as_str(), rest.as_str()), (Protocol::Mtp, "Pixel 7", "/Внутренний общий накопитель/DCIM"));
        assert_eq!(site.url(), "mtp://Pixel 7");
        assert_eq!(parent(Path::new("mtp://Pixel 7/A")), Some(PathBuf::from("mtp://Pixel 7/")));
        assert_eq!(device_root(p), Some(PathBuf::from("mtp://Pixel 7/")));
        assert!(!is_device(Path::new("sftp://a@b/c")));
        assert!(!Protocol::ALL.contains(&Protocol::Mtp), "not offered in the connect dialog");
    }

    #[test]
    fn parses_sites() {
        let sftp = Protocol::Sftp;
        assert_eq!(Site::parse(sftp, "bob@example.com"), Some(Site { protocol: sftp, user: "bob".into(), host: "example.com".into(), port: 22 }));
        assert_eq!(Site::parse(sftp, "me@h:2222").unwrap().port, 2222);
        assert_eq!(Site::parse(sftp, "a@b@[::1]:23"), Some(Site { protocol: sftp, user: "a@b".into(), host: "::1".into(), port: 23 }));
        assert!(Site::parse(sftp, "host").is_none());
        assert!(Site::parse(sftp, "u@h:abc").is_none());
        assert_eq!(Site::parse(Protocol::Ftp, "u@h").unwrap().port, 21);
        let ftp = Site::parse_url("FTP://anonymous@ftp.example.org/pub").unwrap();
        assert_eq!((ftp.protocol, ftp.port, ftp.url()), (Protocol::Ftp, 21, "ftp://anonymous@ftp.example.org".to_string()));
        assert_eq!(Site::parse_url("ftps://u@h:990").unwrap().authority(), "u@h:990");
        assert_eq!(Site::parse(sftp, "u@[::1]:22").unwrap().authority(), "u@[::1]");
    }

    #[test]
    fn splits_paths_with_either_separator() {
        let p = Path::new("sftp://bob@h:2222/home/bob").join("docs");
        let (site, remote) = split(&p).unwrap();
        assert_eq!((site.port, remote.as_str()), (2222, "/home/bob/docs"));
        assert_eq!(split(Path::new("sftp://bob@h")).unwrap().1, "/");
        assert_eq!(split(Path::new("SFTP://bob@h/a/./b/../c/")).unwrap().1, "/a/c");
        assert!(split(Path::new("C:\\sftp")).is_none());
        assert!(!is_remote(Path::new("C:\\")));
    }

    #[test]
    fn navigation_helpers() {
        let p = PathBuf::from("sftp://bob@h/home/bob");
        assert_eq!(parent(&p), Some(PathBuf::from("sftp://bob@h/home")));
        assert_eq!(parent(&PathBuf::from("sftp://bob@h/home")), Some(PathBuf::from("sftp://bob@h/")));
        assert_eq!(parent(&PathBuf::from("sftp://bob@h/")), None);
        assert_eq!(join(&PathBuf::from("sftp://bob@h/"), "x"), Some(PathBuf::from("sftp://bob@h/x")));
        assert_eq!(display(&p.join("d")).unwrap(), "sftp://bob@h/home/bob/d");
        assert_eq!(file_name(&p).unwrap(), "bob");
        assert!(starts_with(&p.join("x"), &p));
        assert!(!starts_with(Path::new("sftp://bob@h/home/bobby"), &p));
        assert!(!starts_with(Path::new("sftp://eve@h/home/bob/x"), &p));
    }
}
