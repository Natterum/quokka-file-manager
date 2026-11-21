//! Ports: what the application layer needs from the outside world. Adapters in
//! `quokka-infra` implement them; tests use in-memory fakes.

use crate::job::Job;
use crate::Waker;
use quokka_domain::entry::Meta;
use quokka_domain::{Entry, OpSpec};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Recursive size of a folder.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Size {
    pub bytes: u64,
    /// Items that could not be read or were links (not followed).
    pub skipped: u64,
}

/// Read-only access to the file system. Implementations must be cheap to call from
/// any thread; slow calls only ever happen on background threads.
pub trait FileSystem: Send + Sync + 'static {
    /// Items of `dir`, without "..", hidden ones included (flagged in `Entry::hidden`).
    /// Should give up with `ErrorKind::Interrupted` soon after `cancel` is set.
    fn read_dir(&self, dir: &Path, cancel: &AtomicBool) -> io::Result<Vec<Entry>>;
    /// Metadata of one item, following links; `None` if it does not exist.
    fn stat(&self, path: &Path) -> Option<Meta>;
    /// Whether `dir` can be opened as a folder (error text for the user if not).
    fn check_dir(&self, dir: &Path) -> Result<(), String>;
    /// Roots shown in the "This PC" view.
    fn drives(&self) -> Vec<PathBuf>;
    /// (total, free) bytes of the volume holding `path`.
    fn disk_space(&self, path: &Path) -> Option<(u64, u64)>;
    /// Recursive size; `None` if cancelled.
    fn tree_size(&self, root: &Path, cancel: &AtomicBool) -> Option<Size>;
}

/// Starts background file operations (copy, move, delete, archive …).
pub trait FileOps: Send + Sync + 'static {
    fn start(&self, spec: &OpSpec, waker: Waker) -> Job;
}

/// Keeps a directory watch alive; dropping it stops the watch.
pub struct WatchGuard(#[allow(dead_code)] Box<dyn Send>);

impl WatchGuard {
    pub fn new(inner: impl Send + 'static) -> Self {
        WatchGuard(Box::new(inner))
    }
}

/// Change notifications for a folder (its direct children).
pub trait Watcher: Send + Sync + 'static {
    /// Call `on_change` whenever something in `dir` changes; `None` if the folder
    /// cannot be watched (some network drives), then the caller falls back to
    /// refreshing after its own operations.
    fn watch(&self, dir: &Path, on_change: Box<dyn Fn() + Send + Sync>) -> Option<WatchGuard>;
}

/// The adapters a tab works with, bundled so they are passed around as one value.
#[derive(Clone)]
pub struct Services {
    pub fs: Arc<dyn FileSystem>,
    pub watcher: Option<Arc<dyn Watcher>>,
    pub waker: Waker,
}
