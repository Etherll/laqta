//! The annotation editor window under a stream of mouse moves (D7). Renders wait in the
//! `QueuedSpawner` until the test runs them, as a 4K render lags a 280 Hz mouse.
//!
//! D7c is implicit: before the fix, the view waited for each render with `block_on`, so with a
//! queued render these tests would hang until nextest's timeout instead of passing.

use gpui::{Modifiers, MouseButton, Pixels, Point, TestAppContext, VisualTestContext, point, px};
use laqta_domain::{Bitmap, PixelFormat};

use super::{QueuedSpawner, install, services};

fn grey(w: u32, h: u32) -> Bitmap {
    let mut px = vec![128u8; (w * h * 4) as usize];
    for a in px.iter_mut().skip(3).step_by(4) {
        *a = 255;
    }
    Bitmap::new(w, h, PixelFormat::Rgba8, px.into()).unwrap()
}

/// Opens the editor on a 2560×1440 capture and drains its first render.
fn open(cx: &mut TestAppContext) -> (QueuedSpawner, VisualTestContext, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let spawner = QueuedSpawner::default();
    install(cx, services(spawner.clone(), dir.path()));
    cx.update(|cx| crate::editor_window::open(Some(grey(2560, 1440)), None, None, cx));
    let window = *cx.windows().last().expect("the editor window");
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    drain(&spawner, &mut vcx);
    spawner.reset();
    (spawner, vcx, dir)
}

/// Runs queued compute work until none is left (a render, then its picture conversion, then any
/// coalesced render).
fn drain(spawner: &QueuedSpawner, vcx: &mut VisualTestContext) -> usize {
    let mut total = 0;
    loop {
        vcx.run_until_parked();
        let n = spawner.run_all();
        vcx.run_until_parked();
        if n == 0 {
            return total;
        }
        total += n;
    }
}

/// A point in the middle band of the window, inside the canvas.
fn at(i: usize) -> Point<Pixels> {
    point(px(420. + (i % 200) as f32), px(300. + (i / 200) as f32 * 10.))
}

#[gpui::test]
fn hover_starts_no_render(cx: &mut TestAppContext) {
    let (spawner, mut vcx, _dir) = open(cx);
    vcx.simulate_keystrokes("v"); // Select tool
    for i in 0..300 {
        vcx.simulate_mouse_move(at(i), None, Modifiers::default());
    }
    vcx.run_until_parked();
    assert_eq!(spawner.calls(), 0, "a hover must not render the canvas");
}

#[gpui::test]
fn drag_coalesces_renders_while_one_is_pending(cx: &mut TestAppContext) {
    let (spawner, mut vcx, _dir) = open(cx);
    // Rectangle is the default tool.
    vcx.simulate_mouse_down(at(0), MouseButton::Left, Modifiers::default());
    for i in 1..=300 {
        vcx.simulate_mouse_move(at(i), Some(MouseButton::Left), Modifiers::default());
    }
    vcx.run_until_parked();
    assert_eq!(spawner.calls(), 1, "one render starts; the other 299 moves coalesce behind it");

    // The pending render finishes: its picture is converted, then exactly one trailing render.
    let ran = drain(&spawner, &mut vcx);
    assert!(ran <= 4, "render + conversion, then one coalesced render + conversion; ran {ran}");

    vcx.simulate_mouse_up(at(300), MouseButton::Left, Modifiers::default());
    let after_up = drain(&spawner, &mut vcx);
    assert!(after_up <= 2, "committing the shape renders once; ran {after_up}");
}
