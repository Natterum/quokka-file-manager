//! A running background operation: progress, cancellation and the final result.
//! Adapters do the actual work inside [`Job::spawn`] and report through [`Reporter`].

use quokka_domain::{tr, trf};
use crate::Waker;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

pub enum Msg {
    Progress { done: u64, total: u64, current: String },
    Done(Result<String, String>, Vec<(PathBuf, PathBuf)>),
}

pub struct Job {
    #[allow(dead_code)] // the queue shows its own, more detailed titles
    pub title: String,
    pub bytes: bool,
    rx: Receiver<Msg>,
    pub cancel: Arc<AtomicBool>,
    pub done: u64,
    pub total: u64,
    pub current: String,
    pub finished: Option<Result<String, String>>,
    /// `(from, to)` of the items the job moved, for undo (see [`Reporter::moved`]).
    pub moved: Vec<(PathBuf, PathBuf)>,
}

impl Job {
    pub fn poll(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(Msg::Progress { done, total, current }) => {
                    self.done = done;
                    self.total = total;
                    self.current = current;
                }
                Ok(Msg::Done(r, moved)) => {
                    self.finished = Some(r);
                    self.moved = moved;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.finished.is_none() {
                        self.finished = Some(Err(tr!("Операция прервана", "Operation aborted").into()));
                    }
                    break;
                }
            }
        }
    }

    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.done as f64 / self.total as f64).clamp(0.0, 1.0) as f32
        }
    }

    /// Run `f` on its own thread. `bytes` — progress is counted in bytes, not items.
    pub fn spawn<F>(title: String, bytes: bool, waker: Waker, f: F) -> Job
    where
        F: FnOnce(&mut Reporter) -> Result<String, String> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut rep = Reporter {
            tx,
            waker,
            cancel: cancel.clone(),
            done: 0,
            total: 0,
            current: String::new(),
            last: Instant::now(),
            count_bytes: bytes,
            skipped: 0,
            moved: Vec::new(),
        };
        // shell operations (Recycle Bin, shell extensions such as OneDrive or antivirus
        // hooks) can need far more stack than Rust's 2 MB default for spawned threads
        let spawned = thread::Builder::new().name("quokka-job".into()).stack_size(32 * 1024 * 1024).spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut rep)))
                .unwrap_or_else(|e| Err(trf!("Внутренняя ошибка: {}", "Internal error: {}", panic_text(&e))));
            let moved = std::mem::take(&mut rep.moved);
            let _ = rep.tx.send(Msg::Done(r, moved));
            rep.waker.wake();
        });
        if let Err(e) = spawned {
            let (tx2, rx2) = mpsc::channel();
            let _ = tx2.send(Msg::Done(Err(trf!("Не удалось запустить операцию: {e}", "Failed to start the operation: {e}")), Vec::new()));
            return Job { title, bytes, rx: rx2, cancel, done: 0, total: 0, current: String::new(), finished: None, moved: Vec::new() };
        }
        Job { title, bytes, rx, cancel, done: 0, total: 0, current: String::new(), finished: None, moved: Vec::new() }
    }
}

/// The worker side of a [`Job`].
pub struct Reporter {
    tx: Sender<Msg>,
    waker: Waker,
    pub cancel: Arc<AtomicBool>,
    pub done: u64,
    pub total: u64,
    pub current: String,
    last: Instant,
    pub count_bytes: bool,
    pub skipped: usize,
    /// `(from, to)` of every item moved so far; sent back even if the job fails or is
    /// cancelled, so a partial move can still be undone.
    pub moved: Vec<(PathBuf, PathBuf)>,
}

impl Reporter {
    /// Send progress to the UI, at most every ~50 ms unless `force`.
    pub fn tick(&mut self, force: bool) {
        if force || self.last.elapsed().as_millis() > 50 {
            let _ = self.tx.send(Msg::Progress { done: self.done, total: self.total, current: self.current.clone() });
            self.waker.wake();
            self.last = Instant::now();
        }
    }

    /// `Err(Interrupted)` once the user cancelled the job.
    pub fn check(&self) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(io::Error::new(io::ErrorKind::Interrupted, tr!("Отменено", "Cancelled")))
        } else {
            Ok(())
        }
    }
}

pub fn panic_text(e: &Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| e.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| tr!("неизвестная ошибка", "unknown error").into())
}
