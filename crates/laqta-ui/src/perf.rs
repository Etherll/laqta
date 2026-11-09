//! Latency trace points (ARCHITECTURE §1.2 budgets). Each new window logs when it is built and
//! when its first frame is drawn, under the `laqta::perf` target at debug level, so
//! `RUST_LOG=info,laqta::perf=debug` shows dispatch → window built → first frame.

use std::time::Instant;

use gpui::Window;

/// Logs that a `kind` window was built and, on its next frame, how long that frame took to come.
pub(crate) fn first_frame(kind: &'static str, window: &Window) {
    let built = Instant::now();
    tracing::debug!(target: "laqta::perf", kind, "window built");
    // Next-frame callbacks run before that frame draws, so the nested one runs once the first frame
    // has been drawn and presented.
    window.on_next_frame(move |window, _| {
        window.on_next_frame(move |_, _| {
            tracing::debug!(target: "laqta::perf", kind, after_build_ms = built.elapsed().as_secs_f64() * 1000.0, "first frame");
        });
    });
}
