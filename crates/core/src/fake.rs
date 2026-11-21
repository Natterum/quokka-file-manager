//! In-memory adapters for tests of the application layer.

use crate::job::Job;
use crate::ports::{FileOps, FileSystem, Services, Size, WatchGuard, Watcher};
use crate::Waker;
use quokka_domain::entry::Meta;
use quokka_domain::{Entry, OpSpec};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Callback = Arc<dyn Fn() + Send + Sync>;

/// A tiny file system: files with sizes; folders exist implicitly as parents.
pub struct FakeFs {
    files: Mutex<BTreeMap<PathBuf, u64>>,
    delay: Mutex<Duration>,
    watches: Mutex<Vec<(PathBuf, std::sync::Weak<()>, Callback)>>,
}

impl FakeFs {
    pub fn new() -> Arc<FakeFs> {
        Arc::new(FakeFs { files: Default::default(), delay: Default::default(), watches: Default::default() })
    }

    pub fn services(self: &Arc<Self>) -> Services {
        Services { fs: self.clone(), watcher: Some(self.clone()), waker: Waker::noop() }
    }

    pub fn file(&self, path: &str, size: u64) {
        self.files.lock().unwrap().insert(PathBuf::from(path), size);
    }

    pub fn remove(&self, path: &str) {
        self.files.lock().unwrap().remove(Path::new(path));
    }

    /// Make every `read_dir` take this long.
    pub fn set_delay(&self, d: Duration) {
        *self.delay.lock().unwrap() = d;
    }

    /// Report a change in `dir` to its watchers.
    pub fn notify(&self, dir: &str) {
        let cbs: Vec<Callback> = self
            .watches
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, alive, _)| p == Path::new(dir) && alive.strong_count() > 0)
            .map(|(_, _, cb)| cb.clone())
            .collect();
        for cb in cbs {
            cb();
        }
    }

    fn is_dir(&self, p: &Path) -> bool {
        p == Path::new("/") || self.files.lock().unwrap().keys().any(|f| f.starts_with(p) && f != p)
    }
}

impl FileSystem for FakeFs {
    fn read_dir(&self, dir: &Path, cancel: &AtomicBool) -> io::Result<Vec<Entry>> {
        let delay = *self.delay.lock().unwrap();
        let t0 = std::time::Instant::now();
        while t0.elapsed() < delay {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if !self.is_dir(dir) {
            return Err(io::ErrorKind::NotFound.into());
        }
        let mut children: BTreeMap<PathBuf, Meta> = BTreeMap::new();
        for (f, &size) in self.files.lock().unwrap().iter() {
            let Ok(rest) = f.strip_prefix(dir) else { continue };
            let mut comps = rest.components();
            let Some(first) = comps.next() else { continue };
            let is_dir = comps.next().is_some();
            children.entry(dir.join(first)).or_insert(Meta { is_dir, size: if is_dir { 0 } else { size }, modified: None, hidden: false, ..Meta::default() });
        }
        Ok(children
            .into_iter()
            .map(|(p, m)| {
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                Entry::item(name.clone(), &name, p, m)
            })
            .collect())
    }

    fn stat(&self, path: &Path) -> Option<Meta> {
        if let Some(&size) = self.files.lock().unwrap().get(path) {
            return Some(Meta { is_dir: false, size, modified: None, hidden: false, ..Meta::default() });
        }
        self.is_dir(path).then_some(Meta { is_dir: true, size: 0, modified: None, hidden: false, ..Meta::default() })
    }

    fn check_dir(&self, dir: &Path) -> Result<(), String> {
        if self.is_dir(dir) {
            Ok(())
        } else {
            Err(format!("{}: not found", dir.display()))
        }
    }

    fn drives(&self) -> Vec<PathBuf> {
        vec![PathBuf::from("/")]
    }

    fn disk_space(&self, _: &Path) -> Option<(u64, u64)> {
        Some((100, 50))
    }

    fn tree_size(&self, root: &Path, cancel: &AtomicBool) -> Option<Size> {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let bytes = self.files.lock().unwrap().iter().filter(|(f, _)| f.starts_with(root)).map(|(_, s)| s).sum();
        Some(Size { bytes, skipped: 0 })
    }
}

impl Watcher for FakeFs {
    fn watch(&self, dir: &Path, on_change: Box<dyn Fn() + Send + Sync>) -> Option<WatchGuard> {
        let token = Arc::new(());
        self.watches.lock().unwrap().push((dir.to_path_buf(), Arc::downgrade(&token), Arc::from(on_change)));
        Some(WatchGuard::new(token))
    }
}

/// File operations that succeed (or fail, for `Delete` of nothing) right away.
pub struct FakeOps {
    pub started: Mutex<Vec<String>>,
}

impl FileOps for FakeOps {
    fn start(&self, spec: &OpSpec, waker: Waker) -> Job {
        let title = spec.title();
        self.started.lock().unwrap().push(title.clone());
        let fail = matches!(spec, OpSpec::Delete { sources } if sources.is_empty());
        Job::spawn(title.clone(), false, waker, move |rep| {
            rep.total = 1;
            rep.done = 1;
            rep.tick(true);
            if fail {
                Err("nothing to delete".into())
            } else {
                Ok(title)
            }
        })
    }
}
