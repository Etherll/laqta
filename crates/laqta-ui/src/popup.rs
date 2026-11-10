//! Exact placement for windows without a title bar on Windows.
//!
//! GPUI's Windows backend centres a window's frame around the bounds it is given, but a window
//! without a title bar keeps a frame only at its bottom. So the window lands above the place asked
//! for, by half that frame: 4 logical pixels with a standard frame. The region overlay then shows
//! the frozen screen shifted, and the recording border cuts into the recorded area. `placed` asks
//! for a position lower by that drift, and `observe` measures the drift a window really had, so
//! the correction ends once GPUI places such windows where asked.

use std::sync::atomic::{AtomicI32, Ordering};

use gpui::{Bounds, Pixels, Window, px};

/// The drift in thousandths of a logical pixel.
static DRIFT: AtomicI32 = AtomicI32::new(if cfg!(windows) { 4000 } else { 0 });

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
