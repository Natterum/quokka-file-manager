//! [`Watcher`] adapter on top of `notify` (ReadDirectoryChangesW on Windows,
//! inotify / FSEvents elsewhere).

use notify::{Event, EventKind, RecursiveMode, Watcher as _};
use quokka_core::ports::{WatchGuard, Watcher};
use std::path::Path;

pub struct NotifyWatcher;

impl Watcher for NotifyWatcher {
    fn watch(&self, dir: &Path, on_change: Box<dyn Fn() + Send + Sync>) -> Option<WatchGuard> {
        let archive = crate::archive::location(dir);
        let dir = archive.as_ref().and_then(|(a, _)| a.parent()).unwrap_or(dir);
        let mut w = notify::recommended_watcher(move |res: notify::Result<Event>| match res {
            // reading a file changes nothing the list shows
            Ok(ev) if matches!(ev.kind, EventKind::Access(_)) => {}
            // on errors (e.g. buffer overflow after a flood of changes) refresh anyway
            _ => on_change(),
        })
        .ok()?;
        w.watch(dir, RecursiveMode::NonRecursive).ok()?;
        Some(WatchGuard::new(w))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn reports_new_files_until_dropped() {
        let dir = std::env::temp_dir().join(format!("quokka-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let guard = NotifyWatcher.watch(&dir, Box::new(move || {
            h.fetch_add(1, Ordering::Relaxed);
        }));
        assert!(guard.is_some());
        std::fs::write(dir.join("new.txt"), b"x").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while hits.load(Ordering::Relaxed) == 0 {
            assert!(Instant::now() < deadline, "no change reported");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(guard);
        std::thread::sleep(Duration::from_millis(100));
        let before = hits.load(Ordering::Relaxed);
        std::fs::write(dir.join("later.txt"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(hits.load(Ordering::Relaxed), before, "no reports after the guard is dropped");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
