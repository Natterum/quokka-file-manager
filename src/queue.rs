//! Queue of background operations. Tasks run one at a time in the order they were
//! added; pending tasks can be reordered or removed, failed ones retried.
//!
//! To add a new kind of operation: add a variant to `OpSpec`, give it a title and
//! a `start` arm that returns an `fsops::Job` — the queue and its UI need no changes.

use crate::fsops::{self, Job};
use crate::util::items_text;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

#[derive(Clone, Debug)]
pub enum OpSpec {
    Copy { sources: Vec<PathBuf>, dest: PathBuf, overwrite: bool },
    Move { sources: Vec<PathBuf>, dest: PathBuf, overwrite: bool },
    Delete { sources: Vec<PathBuf> },
    Recycle { sources: Vec<PathBuf> },
    /// Restore items sent to the Recycle Bin (by their original paths).
    Restore { originals: Vec<PathBuf> },
    Zip { sources: Vec<PathBuf>, dest: PathBuf },
    Unzip { archive: PathBuf, dest: PathBuf, overwrite: bool },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Copy,
    Move,
    Delete,
    Archive,
}

fn what(sources: &[PathBuf]) -> String {
    match sources {
        [one] => format!("«{}»", file_name(one)),
        _ => items_text(sources.len() as u64),
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn folder(p: &Path) -> String {
    let s = file_name(p);
    if s.is_empty() {
        p.display().to_string()
    } else {
        s
    }
}

impl OpSpec {
    pub fn title(&self) -> String {
        match self {
            OpSpec::Copy { sources, dest, .. } => format!("Копирование {} в «{}»", what(sources), folder(dest)),
            OpSpec::Move { sources, dest, .. } => format!("Перемещение {} в «{}»", what(sources), folder(dest)),
            OpSpec::Delete { sources } => format!("Удаление {}", what(sources)),
            OpSpec::Recycle { sources } => format!("В корзину: {}", what(sources)),
            OpSpec::Restore { originals } => format!("Восстановление {} из корзины", what(originals)),
            OpSpec::Zip { sources, dest } => format!("Архивация {} в «{}»", what(sources), file_name(dest)),
            OpSpec::Unzip { archive, dest, .. } => format!("Распаковка «{}» в «{}»", file_name(archive), folder(dest)),
        }
    }

    pub fn kind(&self) -> OpKind {
        match self {
            OpSpec::Copy { .. } => OpKind::Copy,
            OpSpec::Move { .. } => OpKind::Move,
            OpSpec::Delete { .. } | OpSpec::Recycle { .. } => OpKind::Delete,
            OpSpec::Restore { .. } => OpKind::Move,
            OpSpec::Zip { .. } | OpSpec::Unzip { .. } => OpKind::Archive,
        }
    }

    fn start(&self, ctx: &egui::Context) -> Job {
        let ctx = ctx.clone();
        match self.clone() {
            OpSpec::Copy { sources, dest, overwrite } => fsops::start_copy(sources, dest, overwrite, ctx),
            OpSpec::Move { sources, dest, overwrite } => fsops::start_move(sources, dest, overwrite, ctx),
            OpSpec::Delete { sources } => fsops::start_delete(sources, ctx),
            OpSpec::Recycle { sources } => fsops::start_recycle(sources, ctx),
            OpSpec::Restore { originals } => fsops::start_unrecycle(originals, ctx),
            OpSpec::Zip { sources, dest } => fsops::start_zip(sources, dest, ctx),
            OpSpec::Unzip { archive, dest, overwrite } => fsops::start_unzip(archive, dest, overwrite, ctx),
        }
    }
}

pub enum State {
    Pending,
    Running(Job),
    Done(String),
    Failed(String),
    Cancelled,
}

pub struct Task {
    pub id: u64,
    pub spec: OpSpec,
    pub title: String,
    pub state: State,
}

impl Task {
    pub fn is_finished(&self) -> bool {
        matches!(self.state, State::Done(_) | State::Failed(_) | State::Cancelled)
    }
}

/// A task that finished during the last `poll`.
pub struct Finished {
    pub spec: OpSpec,
    pub result: Result<String, String>,
    pub cancelled: bool,
}

#[derive(Default)]
pub struct Queue {
    pub tasks: Vec<Task>,
    pub paused: bool,
    next_id: u64,
}

impl Queue {
    pub fn push(&mut self, spec: OpSpec) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let title = spec.title();
        self.tasks.push(Task { id, spec, title, state: State::Pending });
        id
    }

    pub fn running(&self) -> Option<&Task> {
        self.tasks.iter().find(|t| matches!(t.state, State::Running(_)))
    }

    pub fn pending_count(&self) -> usize {
        self.tasks.iter().filter(|t| matches!(t.state, State::Pending)).count()
    }

    /// Something is running or waiting to run.
    pub fn is_active(&self) -> bool {
        self.tasks.iter().any(|t| matches!(t.state, State::Pending | State::Running(_)))
    }

    pub fn has_finished(&self) -> bool {
        self.tasks.iter().any(Task::is_finished)
    }

    /// Collect progress, finish the running task and start the next pending one.
    pub fn poll(&mut self, ctx: &egui::Context) -> Vec<Finished> {
        let mut out = Vec::new();
        for t in &mut self.tasks {
            let State::Running(job) = &mut t.state else { continue };
            job.poll();
            let Some(res) = job.finished.take() else { continue };
            let cancelled = job.cancel.load(Ordering::Relaxed) && res.is_err();
            t.state = match (&res, cancelled) {
                (_, true) => State::Cancelled,
                (Ok(m), _) => State::Done(m.clone()),
                (Err(e), _) => State::Failed(e.clone()),
            };
            out.push(Finished { spec: t.spec.clone(), result: res, cancelled });
        }
        if !self.paused && self.running().is_none() {
            if let Some(t) = self.tasks.iter_mut().find(|t| matches!(t.state, State::Pending)) {
                t.state = State::Running(t.spec.start(ctx));
            }
        }
        out
    }

    /// Cancel a running task, or drop a pending one.
    pub fn cancel(&mut self, id: u64) {
        let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) else { return };
        match &t.state {
            State::Running(job) => job.cancel.store(true, Ordering::Relaxed),
            State::Pending => t.state = State::Cancelled,
            _ => {}
        }
    }

    pub fn cancel_all(&mut self) {
        let ids: Vec<u64> = self.tasks.iter().map(|t| t.id).collect();
        for id in ids {
            self.cancel(id);
        }
    }

    /// Put a failed or cancelled task back at the end of the queue.
    pub fn retry(&mut self, id: u64) {
        let Some(i) = self.tasks.iter().position(|t| t.id == id) else { return };
        if matches!(self.tasks[i].state, State::Failed(_) | State::Cancelled) {
            let mut t = self.tasks.remove(i);
            t.state = State::Pending;
            self.tasks.push(t);
        }
    }

    pub fn remove(&mut self, id: u64) {
        self.tasks.retain(|t| t.id != id || !t.is_finished());
    }

    /// Swap a pending task with the neighbouring pending task (`up` = run earlier).
    pub fn shift(&mut self, id: u64, up: bool) {
        let pending: Vec<usize> =
            self.tasks.iter().enumerate().filter(|(_, t)| matches!(t.state, State::Pending)).map(|(i, _)| i).collect();
        let Some(k) = pending.iter().position(|&i| self.tasks[i].id == id) else { return };
        let other = if up { k.checked_sub(1) } else { Some(k + 1) };
        if let Some(&j) = other.and_then(|o| pending.get(o)) {
            self.tasks.swap(pending[k], j);
        }
    }

    pub fn clear_finished(&mut self) {
        self.tasks.retain(|t| !t.is_finished());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(q: &mut Queue, ctx: &egui::Context) -> Vec<Finished> {
        let mut all = Vec::new();
        for _ in 0..500 {
            all.extend(q.poll(ctx));
            if !q.is_active() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        all
    }

    #[test]
    fn runs_tasks_in_order() {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join("quokka-queue-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.txt"), "данные".repeat(1000)).unwrap();

        let mut q = Queue::default();
        let zip = dir.join("a.zip");
        q.push(OpSpec::Zip { sources: vec![dir.join("src")], dest: zip.clone() });
        q.push(OpSpec::Unzip { archive: zip.clone(), dest: dir.join("out"), overwrite: false });
        q.push(OpSpec::Delete { sources: vec![zip.clone()] });
        let done = wait(&mut q, &ctx);

        assert_eq!(done.len(), 3, "all tasks finish");
        assert!(done.iter().all(|f| f.result.is_ok()), "{:?}", done.iter().map(|f| f.result.clone()).collect::<Vec<_>>());
        assert_eq!(std::fs::read_to_string(dir.join("out/src/a.txt")).unwrap(), "данные".repeat(1000));
        assert!(!zip.exists());
    }

    #[test]
    fn reorder_and_cancel_pending() {
        let mut q = Queue { paused: true, ..Default::default() };
        let a = q.push(OpSpec::Delete { sources: vec![] });
        let b = q.push(OpSpec::Delete { sources: vec![] });
        q.shift(b, true);
        assert_eq!(q.tasks[0].id, b);
        q.cancel(a);
        assert!(matches!(q.tasks[1].state, State::Cancelled));
        q.retry(a);
        assert!(matches!(q.tasks.last().unwrap().state, State::Pending));
    }
}
