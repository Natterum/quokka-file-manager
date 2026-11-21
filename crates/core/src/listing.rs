//! Reading a folder in the background. A tab asks for a listing and keeps showing
//! what it has (or "Загрузка…") until the result arrives; a newer request makes the
//! older one obsolete, so a slow network folder never blocks the window or
//! overwrites a folder the user has already moved on to.

use quokka_domain::{tr, trf};
use crate::ports::Services;
use quokka_domain::entry::{self, Entry};
use quokka_domain::paths::{parent_of, root_label};
use quokka_domain::SortKey;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What to list.
#[derive(Clone, Debug)]
pub enum Source {
    /// The virtual "This PC" view.
    Pc,
    Dir(PathBuf),
    /// Search results fed to a tab: names relative to `root`, ".." leads back to it.
    Results { root: PathBuf, paths: Vec<PathBuf> },
}

#[derive(Clone, Debug)]
pub struct Request {
    pub source: Source,
    pub show_hidden: bool,
    pub sort: SortKey,
    pub asc: bool,
}

/// A finished listing, already sorted by the requested key.
#[derive(Default)]
pub struct Loaded {
    pub entries: Vec<Entry>,
    pub sort: Option<(SortKey, bool)>,
    pub dirs: usize,
    pub files: usize,
    pub bytes: u64,
    pub disk: Option<(u64, u64)>,
    pub error: Option<String>,
    /// How long the listing took — used to pace automatic refreshes.
    pub cost: Duration,
}

/// A listing in progress. Dropping it cancels the work.
pub struct Pending {
    rx: Receiver<Loaded>,
    cancel: Arc<AtomicBool>,
}

impl Pending {
    pub fn try_take(&mut self) -> Option<Loaded> {
        match self.rx.try_recv() {
            Ok(l) => Some(l),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Loaded { error: Some(tr!("Чтение папки прервано", "Folder reading was interrupted").into()), ..Default::default() }),
        }
    }

    /// Wait up to `timeout` for the result.
    pub fn wait(&mut self, timeout: Duration) -> Option<Loaded> {
        match self.rx.recv_timeout(timeout) {
            Ok(l) => Some(l),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Loaded { error: Some(tr!("Чтение папки прервано", "Folder reading was interrupted").into()), ..Default::default() }),
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn start(services: &Services, req: Request) -> Pending {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let (fs, waker, flag) = (services.fs.clone(), services.waker.clone(), cancel.clone());
    let spawned = std::thread::Builder::new().name("quokka-list".into()).spawn(move || {
        let t0 = Instant::now();
        let mut loaded = build(&*fs, &req, &flag);
        if flag.load(Ordering::Relaxed) {
            return;
        }
        loaded.cost = t0.elapsed();
        if tx.send(loaded).is_ok() {
            waker.wake();
        }
    });
    if let Err(e) = spawned {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(Loaded { error: Some(trf!("Не удалось прочитать папку: {e}", "Failed to read the folder: {e}")), ..Default::default() });
        return Pending { rx, cancel };
    }
    Pending { rx, cancel }
}

fn build(fs: &dyn crate::ports::FileSystem, req: &Request, cancel: &AtomicBool) -> Loaded {
    let mut out = Loaded::default();
    match &req.source {
        Source::Pc => {
            for d in fs.drives() {
                let total = fs.disk_space(&d).map(|(t, _)| t).unwrap_or(0);
                out.entries.push(Entry::drive(root_label(&d), d, total));
                out.dirs += 1;
            }
        }
        Source::Dir(dir) => {
            if let Some(parent) = parent_of(dir) {
                out.entries.push(Entry::parent(parent));
            }
            match fs.read_dir(dir, cancel) {
                Ok(items) => {
                    out.entries.reserve(items.len());
                    for e in items {
                        if e.hidden && !req.show_hidden {
                            continue;
                        }
                        out.count(&e);
                        out.entries.push(e);
                    }
                }
                Err(e) => out.error = Some(e.to_string()),
            }
            out.disk = fs.disk_space(dir);
        }
        Source::Results { root, paths } => {
            out.entries.push(Entry::parent(root.clone()));
            for path in paths {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let Some(meta) = fs.stat(path) else { continue };
                let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let name = match path.strip_prefix(root) {
                    Ok(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
                    _ => path.display().to_string(),
                };
                let e = Entry::item(name, &file_name, path.clone(), meta);
                out.count(&e);
                out.entries.push(e);
            }
        }
    }
    entry::sort(&mut out.entries, req.sort, req.asc);
    out.sort = Some((req.sort, req.asc));
    out
}

impl Loaded {
    fn count(&mut self, e: &Entry) {
        if e.is_dir {
            self.dirs += 1;
        } else {
            self.files += 1;
            self.bytes += e.size;
        }
    }
}

/// Check that `dir` opens, but give up waiting after `timeout` (a dead network share
/// can take tens of seconds to fail) — the listing will then report the error itself.
pub fn probe_dir(services: &Services, dir: &std::path::Path, timeout: Duration) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let (fs, dir) = (services.fs.clone(), dir.to_path_buf());
    std::thread::spawn(move || {
        let _ = tx.send(fs.check_dir(&dir));
    });
    rx.recv_timeout(timeout).unwrap_or(Ok(()))
}
