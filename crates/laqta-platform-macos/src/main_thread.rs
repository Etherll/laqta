//! AppKit objects (the status item, the pasteboard, windows) may only be used on the main thread.
//! Port methods are called from worker threads, so they send their AppKit work here.

use objc2::MainThreadMarker;

/// Runs `work` on the main thread and waits for its result. Called on the main thread, it runs
/// `work` directly.
pub(crate) fn on_main<R: Send>(work: impl FnOnce(MainThreadMarker) -> R + Send) -> R {
    dispatch2::run_on_main(work)
}

/// Queues `work` on the main thread and returns at once.
pub(crate) fn on_main_later(work: impl FnOnce(MainThreadMarker) + Send + 'static) {
    dispatch2::DispatchQueue::main().exec_async(move || {
        if let Some(mtm) = MainThreadMarker::new() {
            work(mtm);
        }
    });
}
