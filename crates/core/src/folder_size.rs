//! Cancellable background folder size scans for one tab.

use crate::ports::{Services, Size};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{atomic::{AtomicBool, Ordering}, mpsc, Arc};

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

    pub fn start(&mut self, paths: Vec<PathBuf>, services: &Services) {
        self.stop();
        if paths.is_empty() {
            return;
        }
        for path in &paths {
            self.sizes.remove(path);
        }
        self.pending.extend(paths.iter().cloned());
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let (fs, waker) = (services.fs.clone(), services.waker.clone());
        std::thread::spawn(move || {
            for path in paths {
                let Some(size) = fs.tree_size(&path, &cancel) else { break };
                if tx.send((path, size)).is_err() {
                    break;
                }
                waker.wake();
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
        if self.pending.is_empty() {
            self.rx = None;
        }
        updates
    }
}

impl Drop for FolderSizes {
    fn drop(&mut self) {
        self.stop();
    }
}
