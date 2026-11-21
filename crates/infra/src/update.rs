//! Self-update from GitHub Releases: ask the API for the latest release, download the
//! package for this install (MSI or portable ZIP), check it against the published
//! SHA-256, then hand over to the new version.
//!
//! * MSI install — a hidden PowerShell waits for Quokka to exit, runs
//!   `msiexec /i … /passive` into the same folder and starts the upgraded Quokka.
//! * Portable copy — the running executable is renamed to `quokka.exe.old` (Windows
//!   allows renaming a running image), the new one takes its place and is started with
//!   `--updated-from <pid>` so it waits for this process to exit before showing a
//!   window; the next start removes the `.old` file.

use quokka_domain::update::{self, InstallKind, Version};
use quokka_domain::{tr, trf};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// `owner/name` of the repository whose releases are installed.
pub const REPO: &str = "Natterum/quokka-file-manager";

/// File the MSI package installs next to `quokka.exe`; a portable copy has none.
pub const MSI_MARKER: &str = "quokka-msi.txt";

/// Upper bound for a downloaded package, as a guard against a runaway response.
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    /// The release page on GitHub.
    pub page: String,
    /// Release notes (Markdown).
    pub notes: String,
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// Whether this release carries a package (and its checksum) for `kind`.
    pub fn can_install(&self, kind: InstallKind) -> bool {
        let name = update::asset_for(kind, &self.version, arch());
        self.asset(&name).is_some() && self.asset(&update::checksum_asset(&name)).is_some()
    }
}

/// Architecture name used in the package names.
pub fn arch() -> &'static str {
    if cfg!(target_arch = "x86") {
        "x86"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    }
}

/// The running executable.
pub fn current_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| e.to_string())
}

/// How the running copy was installed.
pub fn install_kind() -> InstallKind {
    match current_exe() {
        Ok(exe) if exe.with_file_name(MSI_MARKER).is_file() => InstallKind::Msi,
        _ => InstallKind::Portable,
    }
}

fn agent() -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(tls))
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .user_agent(concat!("quokka-file-manager/", env!("CARGO_PKG_VERSION")))
        .build())
}

fn http_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(403, _) | ureq::Error::Status(429, _) => {
            tr!("GitHub временно ограничил запросы, попробуйте позже", "GitHub is rate-limiting requests, try again later").into()
        }
        ureq::Error::Status(404, _) => tr!("Выпуски не найдены", "No releases found").into(),
        ureq::Error::Status(code, _) => trf!("Ошибка сервера: HTTP {code}", "Server error: HTTP {code}"),
        ureq::Error::Transport(t) => trf!("Нет связи с GitHub: {t}", "Cannot reach GitHub: {t}"),
    }
}

/// Parse the JSON of `GET /repos/{repo}/releases/latest`.
pub fn parse_release(json: &str) -> Result<Release, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or("tag_name missing")?.to_string();
    let version = Version::parse(&tag).ok_or_else(|| format!("not a version tag: {tag}"))?;
    let assets = v["assets"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    Some(Asset {
                        name: a["name"].as_str()?.to_string(),
                        url: a["browser_download_url"].as_str()?.to_string(),
                        size: a["size"].as_u64().unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Release {
        version,
        tag,
        page: v["html_url"].as_str().unwrap_or_default().to_string(),
        notes: v["body"].as_str().unwrap_or_default().replace("\r\n", "\n"),
        assets,
    })
}

/// The latest published release (drafts and pre-releases are not offered).
pub fn latest() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = agent()?
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(http_error)?
        .into_string()
        .map_err(|e| e.to_string())?;
    parse_release(&body)
}

fn sha256_hex(path: &Path) -> std::io::Result<String> {
    use sha2::Digest;
    let mut f = std::fs::File::open(path)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Compare a downloaded file with the hash from its `.sha256` file.
pub fn verify(path: &Path, checksum_text: &str) -> Result<(), String> {
    let want = update::parse_checksum(checksum_text).ok_or_else(|| tr!("Неверный файл контрольной суммы", "Invalid checksum file").to_string())?;
    let got = sha256_hex(path).map_err(|e| e.to_string())?;
    if got != want {
        return Err(tr!("Контрольная сумма не совпадает — файл повреждён", "Checksum mismatch — the download is corrupt").into());
    }
    Ok(())
}

fn fetch_to(agent: &ureq::Agent, asset: &Asset, dest: &Path, cancel: &AtomicBool, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    let resp = agent.get(&asset.url).call().map_err(http_error)?;
    let total = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(asset.size);
    let mut reader = resp.into_reader().take(MAX_DOWNLOAD);
    let part = dest.with_extension("part");
    let mut out = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    let result = loop {
        if cancel.load(Ordering::Relaxed) {
            break Err(tr!("Отменено", "Cancelled").to_string());
        }
        match reader.read(&mut buf) {
            Ok(0) => break out.flush().map_err(|e| e.to_string()),
            Ok(n) => {
                if let Err(e) = out.write_all(&buf[..n]) {
                    break Err(e.to_string());
                }
                done += n as u64;
                progress(done, total);
            }
            Err(e) => break Err(e.to_string()),
        }
    };
    drop(out);
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
        return result;
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

/// Folder for downloaded packages.
fn download_dir() -> PathBuf {
    std::env::temp_dir().join("quokka-update")
}

/// Download the package of `release` for `kind` and check its SHA-256.
/// `progress(done, total)` is called from this thread as bytes arrive.
pub fn download(release: &Release, kind: InstallKind, cancel: &AtomicBool, progress: &dyn Fn(u64, u64)) -> Result<PathBuf, String> {
    let name = update::asset_for(kind, &release.version, arch());
    let asset = release.asset(&name).ok_or_else(|| trf!("В выпуске нет файла {name}", "The release has no {name}"))?;
    let sum = release
        .asset(&update::checksum_asset(&name))
        .ok_or_else(|| trf!("В выпуске нет контрольной суммы для {name}", "The release has no checksum for {name}"))?;
    let agent = agent()?;
    let checksum = agent.get(&sum.url).call().map_err(http_error)?.into_string().map_err(|e| e.to_string())?;
    let dir = download_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(&asset.name);
    fetch_to(&agent, asset, &dest, cancel, progress)?;
    if let Err(e) = verify(&dest, &checksum) {
        let _ = std::fs::remove_file(&dest);
        return Err(e);
    }
    Ok(dest)
}

fn old_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

/// Replace `exe` with the `quokka.exe` from the portable `zip`, keeping the running
/// image as `<exe>.old`. On failure everything is put back.
pub fn swap_portable(zip: &Path, exe: &Path) -> Result<(), String> {
    let cancel = AtomicBool::new(false);
    let (data, _) = crate::archive::read(&zip.join("quokka.exe"), MAX_DOWNLOAD as usize, false, &cancel)
        .map_err(|e| trf!("Не удалось распаковать обновление: {e}", "Could not unpack the update: {e}"))?;
    if data.len() < 2 || &data[..2] != b"MZ" {
        return Err(tr!("В архиве обновления нет программы", "The update archive has no program").into());
    }
    let new = exe.with_extension("exe.new");
    let old = old_path(exe);
    let denied = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            trf!(
                "Нет прав на запись в {} — скачайте новую версию вручную",
                "No permission to write to {} — download the new version manually",
                exe.parent().unwrap_or(exe).display()
            )
        } else {
            e.to_string()
        }
    };
    std::fs::write(&new, &data).map_err(denied)?;
    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(denied(e));
    }
    if let Err(e) = std::fs::rename(&new, exe) {
        let _ = std::fs::rename(&old, exe);
        let _ = std::fs::remove_file(&new);
        return Err(denied(e));
    }
    Ok(())
}

/// Remove what a previous update left behind (the replaced executable, packages).
pub fn cleanup() {
    if let Ok(exe) = current_exe() {
        let _ = std::fs::remove_file(old_path(&exe));
    }
    let _ = std::fs::remove_dir_all(download_dir());
}

/// PowerShell single-quoted string literal.
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Script that waits for `pid`, upgrades from `msi` into `dir` and restarts `exe`.
pub fn msi_script(pid: u32, msi: &Path, dir: &Path, exe: &Path) -> String {
    // msiexec takes one command line; the path and folder go in double quotes
    let args = format!("/i \"{}\" /passive /norestart INSTALLFOLDER=\"{}\"", msi.display(), dir.display());
    format!(
        "Wait-Process -Id {pid} -Timeout 60 -ErrorAction SilentlyContinue; \
         $p = Start-Process -FilePath msiexec.exe -ArgumentList {} -Wait -PassThru; \
         if ($p.ExitCode -eq 0 -or $p.ExitCode -eq 3010) {{ Start-Process -FilePath {} -ArgumentList '--updated' }}",
        ps_quote(&args),
        ps_quote(&exe.display().to_string()),
    )
}

/// Install the downloaded package and start the new version. The caller must quit
/// right after a successful return so the files are free.
pub fn apply(kind: InstallKind, package: &Path) -> Result<(), String> {
    let exe = current_exe()?;
    let pid = std::process::id();
    match kind {
        InstallKind::Portable => {
            swap_portable(package, &exe)?;
            if let Err(e) = std::process::Command::new(&exe).arg("--updated-from").arg(pid.to_string()).spawn() {
                // put the old version back so the user is not left without a program
                let _ = std::fs::remove_file(&exe);
                let _ = std::fs::rename(old_path(&exe), &exe);
                return Err(e.to_string());
            }
        }
        InstallKind::Msi => {
            let dir = exe.parent().ok_or("no install folder")?;
            let script = msi_script(pid, package, dir, &exe);
            let mut cmd = std::process::Command::new("powershell.exe");
            cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-Command", &script]);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
            cmd.spawn().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Block until process `pid` exits (or `timeout` passes) — the new version waits
/// for the old one so the single-window check does not find it.
pub fn wait_for_exit(pid: u32, timeout: Duration) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, timeout.as_millis().min(u32::MAX as u128) as u32);
            CloseHandle(h);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (pid, timeout);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "tag_name": "v0.2.0",
        "html_url": "https://github.com/Natterum/quokka-file-manager/releases/tag/v0.2.0",
        "body": "Line 1\r\nLine 2",
        "prerelease": false,
        "assets": [
            {"name": "quokka-v0.2.0-windows-x64.zip", "size": 10, "browser_download_url": "https://example.invalid/x64.zip"},
            {"name": "quokka-v0.2.0-windows-x64.zip.sha256", "size": 1, "browser_download_url": "https://example.invalid/x64.zip.sha256"},
            {"name": "quokka-v0.2.0-windows-x86.msi", "size": 20, "browser_download_url": "https://example.invalid/x86.msi"}
        ]
    }"#;

    #[test]
    fn parses_github_release_json() {
        let r = parse_release(SAMPLE).unwrap();
        assert_eq!(r.version.to_string(), "0.2.0");
        assert_eq!(r.notes, "Line 1\nLine 2");
        assert_eq!(r.assets.len(), 3);
        assert_eq!(r.asset("QUOKKA-v0.2.0-windows-x86.msi").unwrap().size, 20);
        assert!(parse_release(r#"{"tag_name": "nightly"}"#).is_err());
        assert!(parse_release("not json").is_err());
    }

    #[test]
    fn installable_only_with_package_and_checksum() {
        let r = parse_release(SAMPLE).unwrap();
        // the x86 MSI has no checksum, the x64 ZIP has one
        assert_eq!(r.can_install(InstallKind::Portable), arch() == "x64");
        assert!(!r.can_install(InstallKind::Msi));
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quokka-update-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn verifies_sha256() {
        let dir = temp("sha");
        let f = dir.join("a.bin");
        std::fs::write(&f, b"abc").unwrap();
        let good = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  a.bin\n";
        verify(&f, good).unwrap();
        assert!(verify(&f, &good.replace("ba78", "0078")).is_err());
        assert!(verify(&f, "nonsense").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn swaps_the_portable_executable() {
        let dir = temp("swap");
        let exe = dir.join("quokka.exe");
        std::fs::write(&exe, b"MZ old").unwrap();
        let zip = dir.join("pkg.zip");
        {
            let mut z = crate::zip::ZipWriter::create(&zip).unwrap();
            z.add_file("quokka.exe", &mut &b"MZ new"[..], None, |_| Ok(())).unwrap();
            z.add_file("README.md", &mut &b"readme"[..], None, |_| Ok(())).unwrap();
            z.finish().unwrap();
        }
        swap_portable(&zip, &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"MZ new");
        assert_eq!(std::fs::read(old_path(&exe)).unwrap(), b"MZ old");
        assert!(!exe.with_extension("exe.new").exists());

        // a package without the program leaves everything as it was
        let bad = dir.join("bad.zip");
        {
            let mut z = crate::zip::ZipWriter::create(&bad).unwrap();
            z.add_file("README.md", &mut &b"readme"[..], None, |_| Ok(())).unwrap();
            z.finish().unwrap();
        }
        assert!(swap_portable(&bad, &exe).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"MZ new");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn msi_script_quotes_paths() {
        let s = msi_script(42, Path::new(r"C:\Temp\it's.msi"), Path::new(r"C:\Program Files\Quokka"), Path::new(r"C:\Program Files\Quokka\quokka.exe"));
        assert!(s.starts_with("Wait-Process -Id 42 "));
        assert!(s.contains(r#"'/i "C:\Temp\it''s.msi" /passive /norestart INSTALLFOLDER="C:\Program Files\Quokka"'"#), "{s}");
        assert!(s.contains(r"-FilePath 'C:\Program Files\Quokka\quokka.exe' -ArgumentList '--updated'"));
    }

    /// Talks to GitHub; run by hand.
    #[test]
    #[ignore]
    fn fetches_the_latest_release() {
        let r = latest().unwrap();
        eprintln!("{} {} assets={}", r.tag, r.page, r.assets.len());
    }
}
