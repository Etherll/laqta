//! Render scheduling for the editor presenter.
//!
//! `RenderGate` lets one render run at a time. A request made while a render runs is queued as
//! one more render. `compute` runs work on the compute pool and resolves to its result.

use std::sync::Arc;

use futures_channel::oneshot;
use futures_lite::future::zip;
use laqta_ports::BoxFuture;

/// Coalesces render requests: at most one render runs, and a request made meanwhile runs once more.
#[derive(Debug, Default)]
pub struct RenderGate {
    rendering: bool,
    again: bool,
}

impl RenderGate {
    /// True: start a render now. False: one is running; render once more when it ends.
    pub fn request(&mut self) -> bool {
        if self.rendering {
            self.again = true;
            false
        } else {
            self.rendering = true;
            true
        }
    }

    /// Call when a render ends. True: start one more now (`rendering` stays true, `again`
    /// clears). False: idle (`rendering` false).
    pub fn finished(&mut self) -> bool {
        self.rendering = std::mem::take(&mut self.again);
        self.rendering
    }
}

/// Runs work on the compute pool; resolves when done. Shaped like `Spawner::run_compute`.
pub type ComputeFn = Arc<dyn Fn(Box<dyn FnOnce() + Send>) -> BoxFuture<'static, ()> + Send + Sync>;

/// Runs `f` on the compute pool and resolves to its result. Resolves to `None` if `f` panicked,
/// because the sender is dropped while unwinding.
///
/// The spawner's future may do nothing until it is polled, so it is polled alongside the result.
pub fn compute<T: Send + 'static>(c: &ComputeFn, f: impl FnOnce() -> T + Send + 'static) -> BoxFuture<'static, Option<T>> {
    let (tx, rx) = oneshot::channel();
    let work: Box<dyn FnOnce() + Send> = Box::new(move || {
        let _ = tx.send(f());
    });
    let run = c(work);
    Box::pin(async move {
        let ((), result) = zip(run, rx).await;
        result.ok()
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_lite::future::block_on;
    use laqta_ports::BoxFuture;

    use super::{ComputeFn, RenderGate, compute};

    #[test]
    fn truth_table() {
        let mut gate = RenderGate::default();
        assert!(gate.request());
        assert!(!gate.request());
        assert!(gate.finished());
        assert!(!gate.request());
        assert!(gate.finished());
        assert!(!gate.finished());
        assert!(gate.request());
    }

    #[test]
    fn compute_polls_a_lazy_spawner() {
        let lazy: ComputeFn = Arc::new(|work: Box<dyn FnOnce() + Send>| -> BoxFuture<'static, ()> { Box::pin(async move { work() }) });
        assert_eq!(block_on(compute(&lazy, || 7)), Some(7));
    }
}
