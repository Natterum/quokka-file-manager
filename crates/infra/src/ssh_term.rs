//! «SSH-терминал»: an interactive shell on an SFTP server in its own window.
//! PuTTY is used when it is installed, otherwise the OpenSSH client (`ssh.exe`) that
//! ships with Windows. Either one logs in by itself (asks for the password in its
//! window); Quokka never passes a password on a command line.

use quokka_domain::remote::{Protocol, Site};
use quokka_domain::tr;
use std::io;
use std::path::{Path, PathBuf};

/// Which program opened the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Client {
    Putty,
    OpenSsh,
}

impl Client {
    pub fn label(self) -> &'static str {
        match self {
            Client::Putty => "PuTTY",
            Client::OpenSsh => "ssh",
        }
    }
}

/// Open a shell on `site` (SFTP only) that starts in the remote folder `dir`
/// (`None` — the home folder).
/// `key_file` is the key of the saved connection, if any: an OpenSSH key goes to
/// `ssh.exe`, a `.ppk` one to PuTTY.
pub fn open(site: &Site, dir: Option<&str>, key_file: Option<&Path>) -> io::Result<Client> {
    if site.protocol != Protocol::Sftp {
        return Err(io::Error::new(io::ErrorKind::Unsupported, tr!("SSH-терминал доступен только для SFTP-подключений", "An SSH terminal is only available for SFTP connections")));
    }
    let key = key_file.filter(|k| !k.as_os_str().is_empty());
    if let Some(putty) = find_putty() {
        open_putty(&putty, site, dir, key.filter(|k| is_ppk(k)))?;
        return Ok(Client::Putty);
    }
    if let Some(ssh) = find_ssh() {
        let mut cmd = std::process::Command::new(ssh);
        cmd.args(ssh_args(site, dir, key.filter(|k| !is_ppk(k))));
        spawn_console(cmd)?;
        return Ok(Client::OpenSsh);
    }
    Err(io::Error::new(io::ErrorKind::NotFound, tr!(
        "Не найден ни PuTTY, ни ssh.exe. Установите PuTTY или компонент Windows «Клиент OpenSSH».",
        "Neither PuTTY nor ssh.exe was found. Install PuTTY or the Windows “OpenSSH Client” feature."
    )))
}

/// Remote command: go to `dir` (stay home if it is gone), then the user's login shell.
fn remote_command(dir: Option<&str>) -> String {
    let shell = "exec \"${SHELL:-/bin/sh}\" -l";
    match dir {
        Some(d) => format!("cd {} 2>/dev/null || true; {shell}", sh_quote(d)),
        None => shell.to_string(),
    }
}

/// POSIX single-quoted word.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn is_ppk(key: &Path) -> bool {
    key.extension().is_some_and(|e| e.eq_ignore_ascii_case("ppk"))
}

/// `ssh -t -p PORT [-i KEY] -l USER HOST "cd …; exec $SHELL -l"`.
fn ssh_args(site: &Site, dir: Option<&str>, key: Option<&Path>) -> Vec<std::ffi::OsString> {
    let mut a: Vec<std::ffi::OsString> = vec!["-t".into(), "-p".into(), site.port.to_string().into()];
    if let Some(k) = key {
        a.push("-i".into());
        a.push(k.into());
    }
    a.extend(["-l".into(), site.user.clone().into(), site.host.clone().into(), remote_command(dir).into()]);
    a
}

/// PuTTY takes the remote command from a file (`-m`); `-t` keeps a terminal for it.
fn open_putty(putty: &Path, site: &Site, dir: Option<&str>, key: Option<&Path>) -> io::Result<()> {
    let script = std::env::temp_dir().join("quokka-putty-shell.sh");
    std::fs::write(&script, format!("{}\n", remote_command(dir)))?;
    let mut cmd = std::process::Command::new(putty);
    cmd.args(["-ssh", "-t", "-P", &site.port.to_string(), "-l", &site.user]);
    if let Some(k) = key {
        cmd.arg("-i").arg(k);
    }
    cmd.arg("-m").arg(&script).arg(&site.host);
    cmd.spawn()?;
    Ok(())
}

/// A console program in a new visible window (Quokka itself has no console).
fn spawn_console(mut cmd: std::process::Command) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x00000010); // CREATE_NEW_CONSOLE
        cmd.spawn()?;
    }
    #[cfg(not(windows))]
    {
        let program = cmd.get_program().to_owned();
        let args: Vec<_> = cmd.get_args().map(|a| a.to_owned()).collect();
        std::process::Command::new("x-terminal-emulator").arg("-e").arg(program).args(args).spawn()?;
    }
    Ok(())
}

/// First `name` found in the `PATH` folders.
fn in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

fn find_putty() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    in_path("putty.exe").or_else(|| {
        ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|d| PathBuf::from(d).join("PuTTY").join("putty.exe"))
            .find(|p| p.is_file())
    })
}

fn find_ssh() -> Option<PathBuf> {
    if !cfg!(windows) {
        return in_path("ssh");
    }
    in_path("ssh.exe").or_else(|| {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        // a 32-bit process sees System32 as SysWOW64, where there is no OpenSSH
        ["System32", "Sysnative"]
            .iter()
            .map(|sys| PathBuf::from(&root).join(sys).join("OpenSSH").join("ssh.exe"))
            .find(|p| p.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(port: u16) -> Site {
        Site { protocol: Protocol::Sftp, user: "bob".into(), host: "example.com".into(), port }
    }

    #[test]
    fn quotes_folder_for_the_remote_shell() {
        assert_eq!(sh_quote("/home/bob"), "'/home/bob'");
        assert_eq!(sh_quote("/srv/it's here"), r"'/srv/it'\''s here'");
        assert_eq!(remote_command(Some("/a b")), "cd '/a b' 2>/dev/null || true; exec \"${SHELL:-/bin/sh}\" -l");
        assert_eq!(remote_command(None), "exec \"${SHELL:-/bin/sh}\" -l");
    }

    #[test]
    fn builds_ssh_arguments() {
        let a = ssh_args(&site(2222), Some("/var/www"), Some(Path::new("C:\\keys\\id_ed25519")));
        let a: Vec<String> = a.iter().map(|s| s.to_string_lossy().into_owned()).collect();
        assert_eq!(a[..6], ["-t", "-p", "2222", "-i", "C:\\keys\\id_ed25519", "-l"]);
        assert_eq!(a[6..8], ["bob", "example.com"]);
        assert!(a[8].starts_with("cd '/var/www'"));
    }

    #[test]
    fn ppk_keys_are_for_putty_only() {
        assert!(is_ppk(Path::new("C:\\k\\server.PPK")));
        assert!(!is_ppk(Path::new("C:\\k\\id_rsa")));
    }

    #[test]
    fn ftp_has_no_terminal() {
        let ftp = Site { protocol: Protocol::Ftp, ..site(21) };
        assert_eq!(open(&ftp, None, None).unwrap_err().kind(), io::ErrorKind::Unsupported);
    }
}
