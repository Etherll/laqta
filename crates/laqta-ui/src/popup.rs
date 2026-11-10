//! Windows without a title bar: one way to open them, with the Windows quirks handled.
//!
//! [`open`] is how a popup opens. It corrects the placement drift described below and removes the
//! outline the OS draws around a borderless popup. A popup shorter than [`min_height`] is not
//! possible on Windows, so a caller that sizes a window from content clamps to it. The region
//! overlay is the one exception: it may open fullscreen, so it calls `placed` and `observe` itself.
//!
//! GPUI's Windows backend centres a window's frame around the bounds it is given, but a window
//! without a title bar keeps a frame only at its bottom. So the window lands above the place asked
//! for, by half that frame: 4 logical pixels with a standard frame. The region overlay then shows
//! the frozen screen shifted, and the recording border cuts into the recorded area. `placed` asks
//! for a position lower by that drift, and `observe` measures the drift a window really had, so
//! the correction ends once GPUI places such windows where asked.

use std::sync::atomic::{AtomicI32, Ordering};

use gpui::{App, Bounds, Entity, Pixels, Render, Window, WindowBounds, WindowHandle, WindowOptions, px};

use crate::runtime::{NativeWindow, UiState};

/// The drift in thousandths of a logical pixel.
static DRIFT: AtomicI32 = AtomicI32::new(if cfg!(windows) { 4000 } else { 0 });

/// The shortest window GPUI opens on Windows, in logical pixels. Zero elsewhere.
pub(crate) fn min_height() -> f32 {
    if cfg!(windows) { 31.0 } else { 0.0 }
}

/// Opens a window without a title bar exactly at the bounds in `options`, with no outline or
/// shadow from the OS. `options.window_bounds` holds the bounds wanted; do not pass them through
/// `placed` first.
pub(crate) fn open<V: 'static + Render>(
    mut options: WindowOptions,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> gpui::Result<WindowHandle<V>> {
    options.titlebar = None;
    options.window_bounds = options.window_bounds.map(|bounds| match bounds {
        WindowBounds::Windowed(b) => WindowBounds::Windowed(placed(b)),
        other => other,
    });
    cx.open_window(options, |window, cx| {
        (cx.global::<UiState>().services.native_window)(window, NativeWindow::PlainPopup);
        build(window, cx)
    })
}

/// `bounds`, moved so that a window without a title bar opens exactly at `bounds`.
pub(crate) fn placed(mut bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    bounds.origin.y += px(DRIFT.load(Ordering::Relaxed) as f32 / 1000.0);
    bounds
}

/// Corrects the drift from a window opened with `placed`, which was meant to open at `wanted_y`.
pub(crate) fn observe(window: &Window, wanted_y: Pixels) {
    let off = f32::from(wanted_y - window.bounds().origin.y);
    // A larger gap is the window manager moving the window, not the frame.
    if cfg!(windows) && off.abs() > 0.4 && off.abs() < 16.0 {
        DRIFT.fetch_add((off * 1000.0) as i32, Ordering::Relaxed);
    }
}
