//! [`FileSystem`] adapter on top of `std::fs`.

use quokka_core::ports::{FileSystem, Size};
use quokka_domain::entry::Meta;
use quokka_domain::remote;
use quokka_domain::Entry;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct StdFs;

#[cfg(windows)]
fn is_hidden(name: &str, m: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    name.starts_with('.') || m.file_attributes() & 0x2 != 0
}

#[cfg(not(windows))]
fn is_hidden(name: &str, _m: &fs::Metadata) -> bool {
    name.starts_with('.')
}

/// Symbolic links and Windows reparse points (junctions etc.).
fn is_link(md: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        md.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        md.file_type().is_symlink()
    }
}

fn attributes(m: &fs::Metadata, hidden: bool) -> String {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let flags = m.file_attributes();
        [(0x1, 'R'), (0x2, 'H'), (0x4, 'S'), (0x20, 'A'), (0x10, 'D'),
            (0x400, 'L'), (0x800, 'C'), (0x4000, 'E')].into_iter()
            .filter_map(|(flag, letter)| ((flags & flag != 0) || (letter == 'H' && hidden)).then_some(letter)).collect()
    }
    #[cfg(not(windows))]
    {
        [(m.permissions().readonly(), 'R'), (hidden, 'H'), (m.is_dir(), 'D'), (m.file_type().is_symlink(), 'L')]
            .into_iter().filter_map(|(set, letter)| set.then_some(letter)).collect()
    }
}

fn meta(name: &str, path: &Path, link: &fs::Metadata) -> Meta {
    // hidden/system flags belong to the link itself; everything else to its target
    let target = if link.file_type().is_symlink() { fs::metadata(path).ok() } else { None };
    let m = target.as_ref().unwrap_or(link);
    let hidden = is_hidden(name, link) || is_hidden(name, m);
    Meta { is_dir: m.is_dir(), size: if m.is_dir() { 0 } else { m.len() }, modified: m.modified().ok(),
        created: m.created().ok(), accessed: m.accessed().ok(), attributes: Some(attributes(link, hidden)), hidden }
}

impl FileSystem for StdFs {
    fn read_dir(&self, dir: &Path, cancel: &AtomicBool) -> io::Result<Vec<Entry>> {
        if remote::is_remote(dir) { return crate::net::read_dir(dir, cancel); }
        if crate::archive::location(dir).is_some() { return crate::archive::read_dir(dir, cancel); }
        let mut out = Vec::new();
        for (i, de) in fs::read_dir(dir)?.flatten().enumerate() {
            if i % 256 == 0 && cancel.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let path = de.path();
            let name = de.file_name().to_string_lossy().into_owned();
            // on Windows this comes with the directory listing itself — no extra I/O
            let m = match de.metadata() {
                Ok(link) => meta(&name, &path, &link),
                Err(_) => Meta { is_dir: false, size: 0, modified: None, hidden: name.starts_with('.'), ..Meta::default() },
            };
            out.push(Entry::item(name.clone(), &name, path, m));
        }
        Ok(out)
    }

    fn stat(&self, path: &Path) -> Option<Meta> {
        if remote::is_remote(path) { return crate::net::stat(path); }
        if crate::archive::is_member(path) { return crate::archive::stat(path); }
        let link = fs::symlink_metadata(path).ok()?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Some(meta(&name, path, &link))
    }

    fn check_dir(&self, dir: &Path) -> Result<(), String> {
        if remote::is_remote(dir) { return crate::net::check_dir(dir); }
        if crate::archive::location(dir).is_some() {
            return crate::archive::read_dir(dir, &AtomicBool::new(false)).map(|_| ()).map_err(|e| e.to_string());
        }
        fs::read_dir(dir).map(|_| ()).map_err(|e| format!("{}: {}", dir.display(), e))
    }

    fn drives(&self) -> Vec<PathBuf> {
        #[cfg(windows)]
        {
            // a bitmask from the OS instead of probing C:–Z: one by one, which can
            // hang for seconds on disconnected network drives; then phones and
            // cameras (MTP), which have no letter, as last found by the drive watch
            let mut v = crate::drives::roots(crate::drives::mask());
            v.extend(crate::mtp::roots());
            v
        }
        #[cfg(not(windows))]
        {
            vec![PathBuf::from("/")]
        }
    }

    fn disk_space(&self, path: &Path) -> Option<(u64, u64)> {
        if remote::is_remote(path) { return crate::net::disk_space(path); }
        let archive = crate::archive::location(path);
        let path = archive.as_ref().map_or(path, |(a, _)| a.as_path());
        match (fs2::total_space(path), fs2::available_space(path)) {
            (Ok(t), Ok(f)) => Some((t, f)),
            _ => None,
        }
    }

    fn tree_size(&self, root: &Path, cancel: &AtomicBool) -> Option<Size> {
        if remote::is_remote(root) { return crate::net::tree_size(root, cancel); }
        if crate::archive::is_member(root) {
            return Some(match crate::archive::size(root) { Ok(bytes) => Size { bytes, skipped: 0 }, Err(_) => Size { bytes: 0, skipped: 1 } });
        }
        let mut result = Size::default();
        let mut stack = vec![root.to_path_buf()];
        while let Some(path) = stack.pop() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let md = match fs::symlink_metadata(&path) {
                Ok(md) => md,
                Err(_) => {
                    result.skipped += 1;
                    continue;
                }
            };
            if is_link(&md) {
                result.skipped += 1;
            } else if md.is_file() {
                result.bytes = result.bytes.saturating_add(md.len());
            } else if md.is_dir() {
                match fs::read_dir(path) {
                    Ok(entries) => {
                        for entry in entries {
                            if cancel.load(Ordering::Relaxed) {
                                return None;
                            }
                            match entry {
                                Ok(entry) => stack.push(entry.path()),
                                Err(_) => result.skipped += 1,
                            }
                        }
                    }
                    Err(_) => result.skipped += 1,
                }
            }
        }
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quokka_core::panel::Panel;
    use quokka_core::ports::Services;
    use quokka_core::Waker;
    use std::sync::Arc;

    fn services() -> Services {
        Services { fs: Arc::new(StdFs), watcher: None, waker: Waker::noop() }
    }

    #[test]
    fn listing_reads_dates_and_attributes_from_metadata() {
        let root = std::env::temp_dir().join(format!("quokka-column-meta-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("sample.txt");
        fs::write(&path, b"metadata").unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let listed = StdFs.read_dir(&root, &AtomicBool::new(false)).unwrap();
        let entry = listed.iter().find(|e| e.path == path).unwrap();
        assert_eq!(entry.created, metadata.created().ok());
        assert_eq!(entry.modified, metadata.modified().ok());
        assert_eq!(entry.accessed, metadata.accessed().ok());
        assert_eq!(entry.attributes.as_deref(), Some(attributes(&metadata, false).as_str()));
        assert_eq!(entry.ext, "txt");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recursive_sizes_include_hidden_and_empty_folders() {
        let root = std::env::temp_dir().join(format!("quokka-size-{}", std::process::id()));
        fs::create_dir_all(root.join("sub/empty")).unwrap();
        fs::write(root.join("a"), b"123").unwrap();
        fs::write(root.join("sub/.hidden"), b"4567").unwrap();
        let cancel = AtomicBool::new(false);
        let result = StdFs.tree_size(&root, &cancel).unwrap();
        assert_eq!(result.bytes, 7);
        assert_eq!(result.skipped, 0);
        assert_eq!(StdFs.tree_size(&root.join("sub/empty"), &cancel).unwrap().bytes, 0);
        assert_eq!(StdFs.tree_size(&root.join("missing"), &cancel).unwrap().skipped, 1);
        cancel.store(true, Ordering::Relaxed);
        assert!(StdFs.tree_size(&root, &cancel).is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn real_folder_in_a_tab() {
        let dir = std::env::temp_dir().join(format!("quokka-stdfs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), b"xy").unwrap();
        fs::write(dir.join(".hidden"), b"z").unwrap();
        let mut p = Panel::new(dir.clone(), false, services());
        p.wait_loaded();
        let names: Vec<&str> = p.visible.iter().map(|&i| p.all[i].name.as_str()).collect();
        assert_eq!(names, ["..", "sub", "a.txt"]);
        assert_eq!((p.dirs, p.files, p.bytes), (1, 1, 2));
        assert_eq!(&*p.all[2].type_name, "Текстовый документ");
        assert!(p.navigate(dir.join("missing")).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Folders the user may not read are counted as skipped; the rest is still summed.
    #[cfg(windows)]
    #[test]
    fn unreadable_folders_are_skipped_not_fatal() {
        let root = std::env::temp_dir().join(format!("quokka-size-denied-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("locked/inner")).unwrap();
        fs::write(root.join("locked/inner/f"), b"12345").unwrap();
        fs::write(root.join("ok"), b"12").unwrap();
        let locked = root.join("locked");
        let icacls = |args: &[&str]| std::process::Command::new("icacls").arg(&locked).args(args).output().unwrap();
        // Everyone: deny listing the folder
        assert!(icacls(&["/deny", "*S-1-1-0:(RD)"]).status.success());
        let result = StdFs.tree_size(&root, &AtomicBool::new(false));
        icacls(&["/remove:d", "*S-1-1-0"]);
        let result = result.unwrap();
        assert_eq!(result.bytes, 2);
        assert_eq!(result.skipped, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn drives_include_system_drive() {
        let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        assert!(StdFs.drives().iter().any(|d| d.to_string_lossy().eq_ignore_ascii_case(&format!("{sys}\\"))));
    }
}
