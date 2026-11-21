//! Queue of background operations. Tasks run one at a time in the order they were
//! added; pending tasks can be reordered or removed, failed ones retried.
//!
//! To add a new kind of operation: add a variant to `OpSpec`, give it a title and
//! an arm in the `FileOps` adapter that runs it — the queue and its UI need no changes.

use crate::job::Job;
use crate::ports::FileOps;
use crate::Waker;
pub use quokka_domain::{OpKind, OpSpec};
use std::sync::atomic::Ordering;

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
    /// `(from, to)` of the items a move put elsewhere (empty for other operations).
    pub moved: Vec<(std::path::PathBuf, std::path::PathBuf)>,
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
    pub fn poll(&mut self, ops: &dyn FileOps, waker: &Waker) -> Vec<Finished> {
        let mut out = Vec::new();
        for t in &mut self.tasks {
            let State::Running(job) = &mut t.state else { continue };
            job.poll();
            let Some(res) = job.finished.take() else { continue };
            let cancelled = job.cancel.load(Ordering::Relaxed) && res.is_err();
            let moved = std::mem::take(&mut job.moved);
            t.state = match (&res, cancelled) {
                (_, true) => State::Cancelled,
                (Ok(m), _) => State::Done(m.clone()),
                (Err(e), _) => State::Failed(e.clone()),
            };
            out.push(Finished { spec: t.spec.clone(), result: res, cancelled, moved });
        }
        if !self.paused && self.running().is_none() {
            if let Some(t) = self.tasks.iter_mut().find(|t| matches!(t.state, State::Pending)) {
                t.state = State::Running(ops.start(&t.spec, waker.clone()));
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
    use crate::fake::FakeOps;
    use std::path::PathBuf;

    fn wait(q: &mut Queue, ops: &FakeOps) -> Vec<Finished> {
        let mut all = Vec::new();
        for _ in 0..500 {
            all.extend(q.poll(ops, &Waker::noop()));
            if !q.is_active() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        all
    }

    #[test]
    fn runs_tasks_in_order() {
        let ops = FakeOps { started: Default::default() };
        let mut q = Queue::default();
        let zip = PathBuf::from("/d/a.zip");
        q.push(OpSpec::Zip { sources: vec![PathBuf::from("/d/src")], dest: zip.clone() });
        q.push(OpSpec::Unzip { archive: zip.clone(), dest: PathBuf::from("/d/out"), overwrite: false });
        q.push(OpSpec::Delete { sources: vec![] });
        let done = wait(&mut q, &ops);

        assert_eq!(done.len(), 3, "all tasks finish");
        assert!(done[0].result.is_ok() && done[1].result.is_ok());
        assert!(done[2].result.is_err());
        assert!(matches!(q.tasks[2].state, State::Failed(_)));
        let started = ops.started.lock().unwrap();
        assert!(started[0].starts_with("Архивация") && started[1].starts_with("Распаковка"));
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
