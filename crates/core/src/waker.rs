//! A cheap, cloneable "something changed, redraw" signal that background work sends
//! to the UI. The UI supplies the implementation (for egui — `request_repaint`).

use std::sync::Arc;

#[derive(Clone)]
pub struct Waker(Arc<dyn Fn() + Send + Sync>);

impl Waker {
    pub fn new(f: impl Fn() + Send + Sync + 'static) -> Self {
        Waker(Arc::new(f))
    }

    /// A waker that does nothing — for tests and headless use.
    pub fn noop() -> Self {
        Waker::new(|| {})
    }

    pub fn wake(&self) {
        (self.0)()
    }
}

impl Default for Waker {
    fn default() -> Self {
        Waker::noop()
    }
}

impl std::fmt::Debug for Waker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Waker")
    }
}
