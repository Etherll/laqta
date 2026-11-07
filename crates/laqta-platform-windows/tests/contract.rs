//! Platform contract tests (docs/plan/M0.md §7.1). They need an interactive desktop session.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use laqta_domain::{Action, Frame, Key, KeyChord, Modifiers};

fn check_frames(frames: &[Frame], backend: &str) -> bool {
    assert!(!frames.is_empty(), "{backend}: no frames");
    for f in frames {
        assert_eq!(
            (f.image.width(), f.image.height()),
            (f.monitor.rect.width, f.monitor.rect.height),
            "{backend}: frame size differs from its monitor {:?}",
            f.monitor
        );
    }
    let lit = frames.iter().any(|f| f.image.pixels().as_chunks::<4>().0.iter().any(|p| p[..3] != [0, 0, 0]));
    if !lit {
        eprintln!("{backend}: every frame is black (hosted runner without a lit desktop?); pixel check skipped");
    }
    lit
}

#[test]
fn capture_all_returns_one_frame_per_monitor() {
    laqta_platform_windows::set_dpi_awareness();
    let ports = laqta_platform_windows::ports();
    let frames = futures_lite::future::block_on(ports.screen.capture_all()).unwrap();
    check_frames(&frames, "capture_all");
}

#[test]
fn gdi_fallback_captures() {
    laqta_platform_windows::set_dpi_awareness();
    let frames = laqta_platform_windows::capture_with_gdi().unwrap();
    check_frames(&frames, "gdi");
}

#[test]
fn clipboard_round_trip_keeps_size() {
    let ports = laqta_platform_windows::ports();
    let image = laqta_domain::Bitmap::new(3, 2, laqta_domain::PixelFormat::Bgra8, vec![0x80; 24].into()).unwrap();
    ports.clipboard.set_image(&image).unwrap();
    let read = arboard::Clipboard::new().unwrap().get_image().unwrap();
    assert_eq!((read.width, read.height), (3, 2));
}

#[test]
fn hotkey_registration_succeeds() {
    let ports = laqta_platform_windows::ports();
    let chord = KeyChord { modifiers: Modifiers::empty(), key: Key::PrintScreen };
    let results = ports.hotkeys.register(&[(chord, Action::PrintScreen)], Arc::new(|_| {}));
    assert_eq!(results.len(), 1);
    assert!(results[0].1.is_ok(), "{:?}", results[0].1);
    laqta_platform_windows::shutdown();
}
