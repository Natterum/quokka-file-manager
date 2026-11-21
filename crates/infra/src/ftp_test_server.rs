//! A real FTP server for tests (libunftp serving a local folder; any user and
//! password are accepted).

use quokka_domain::remote::{Protocol, Site};
use std::path::PathBuf;

/// Start a server for `root` on a free local port; returns the port.
pub fn start(root: PathBuf) -> u16 {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let rt = crate::sftp::test_runtime();
    let server = libunftp::ServerBuilder::new(Box::new(move || unftp_sbe_fs::Filesystem::new(root.clone()).unwrap()))
        .greeting("quokka test")
        .build()
        .unwrap();
    rt.spawn(async move {
        let _ = server.listen(format!("127.0.0.1:{port}")).await;
    });
    // wait until it accepts connections
    for _ in 0..200 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    port
}

/// A served temp folder (with `home/`) and a logged-in FTP site whose root is it.
pub fn served(tag: &str) -> (PathBuf, Site) {
    let root = std::env::temp_dir().join(format!("quokka-ftp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("home")).unwrap();
    let port = start(root.clone());
    let site = Site { protocol: Protocol::Ftp, user: "tester".into(), host: "127.0.0.1".into(), port };
    let login = crate::net::Login { password: Some("pw".into()), ..Default::default() };
    let home = crate::net::connect(&site, login, None).unwrap();
    assert_eq!(home, site.path("/"));
    (root, site)
}

/// Not a real test: serves a folder over FTP for trying Quokka by hand (any user
/// and password). `set QUOKKA_FTP_ROOT=C:\some\folder` then
/// `cargo test -p quokka-infra serve_ftp_by_hand -- --ignored --nocapture`.
#[test]
#[ignore]
fn serve_ftp_by_hand() {
    let root = std::env::var_os("QUOKKA_FTP_ROOT").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("quokka-ftp-manual"));
    std::fs::create_dir_all(&root).unwrap();
    let port = start(root.clone());
    println!("FTP server for {} at ftp://tester@127.0.0.1:{port}  (any password)", root.display());
    std::thread::sleep(std::time::Duration::from_secs(3600));
}
