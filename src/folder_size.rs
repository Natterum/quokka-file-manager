//! Cancellable folder size scans. Links and Windows reparse points are not followed.
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{atomic::{AtomicBool, Ordering}, mpsc, Arc};

#[derive(Clone, Copy, Default, Debug)]
pub struct Size {
    pub bytes: u64,
    pub skipped: u64,
}

fn is_link(md: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        md.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    { md.file_type().is_symlink() }
}

fn scan(root: &Path, cancel: &AtomicBool) -> Option<Size> {
    let mut result = Size::default();
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        if cancel.load(Ordering::Relaxed) { return None; }
        let md = match fs::symlink_metadata(&path) {
            Ok(md) => md,
            Err(_) => { result.skipped += 1; continue; }
        };
        if is_link(&md) {
            result.skipped += 1;
        } else if md.is_file() {
            result.bytes = result.bytes.saturating_add(md.len());
        } else if md.is_dir() {
            match fs::read_dir(path) {
                Ok(entries) => for entry in entries {
                    if cancel.load(Ordering::Relaxed) { return None; }
                    match entry {
                        Ok(entry) => stack.push(entry.path()),
                        Err(_) => result.skipped += 1,
                    }
                },
                Err(_) => result.skipped += 1,
            }
        }
    }
    Some(result)
}

#[derive(Default)]
pub struct FolderSizes {
    pub sizes: HashMap<PathBuf, Size>,
    pub pending: HashSet<PathBuf>,
    rx: Option<mpsc::Receiver<(PathBuf, Size)>>,
    cancel: Arc<AtomicBool>,
}

impl FolderSizes {
    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.rx = None;
        self.pending.clear();
    }

    pub fn clear(&mut self) {
        self.stop();
        self.sizes.clear();
    }

    pub fn start(&mut self, paths: Vec<PathBuf>, ctx: egui::Context) {
        self.stop();
        if paths.is_empty() { return; }
        for path in &paths { self.sizes.remove(path); }
        self.pending.extend(paths.iter().cloned());
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            for path in paths {
                let Some(size) = scan(&path, &cancel) else { break };
                if tx.send((path, size)).is_err() { break; }
                ctx.request_repaint();
            }
        });
    }

    pub fn poll(&mut self) -> Vec<(PathBuf, Size)> {
        let mut updates = Vec::new();
        if let Some(rx) = &self.rx {
            while let Ok((path, size)) = rx.try_recv() {
                self.pending.remove(&path);
                self.sizes.insert(path.clone(), size);
                updates.push((path, size));
            }
        }
        if self.pending.is_empty() { self.rx = None; }
        updates
    }
}

impl Drop for FolderSizes {
    fn drop(&mut self) { self.stop(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursive_sizes_include_hidden_and_empty_folders() {
        let root = std::env::temp_dir().join(format!("quokka-size-{}", std::process::id()));
        fs::create_dir_all(root.join("sub/empty")).unwrap();
        fs::write(root.join("a"), b"123").unwrap();
        fs::write(root.join("sub/.hidden"), b"4567").unwrap();
        let cancel = AtomicBool::new(false);
        let result = scan(&root, &cancel).unwrap();
        assert_eq!(result.bytes, 7);
        assert_eq!(result.skipped, 0);
        assert_eq!(scan(&root.join("sub/empty"), &cancel).unwrap().bytes, 0);
        assert_eq!(scan(&root.join("missing"), &cancel).unwrap().skipped, 1);
        cancel.store(true, Ordering::Relaxed);
        assert!(scan(&root, &cancel).is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
