//! Platform contract tests (M0 §7.1). They need a display: Xvfb for X11 or headless sway for
//! Wayland. They touch the real screen and clipboard, so they run only with `LAQTA_CONTRACT=1`
//! (set by CI); otherwise each test logs why and returns. On CI (`CI` set) the root is #336699.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use laqta_domain::{Action, Bitmap, Key, KeyChord, Modifiers, PixelFormat};
use laqta_platform_linux::{Session, ports};
use laqta_ports::PortError;

fn session() -> Option<Session> {
    if std::env::var_os("LAQTA_CONTRACT").is_none() {
        eprintln!("skipped: set LAQTA_CONTRACT=1 to run platform contract tests");
        return None;
    }
    let s = Session::detect();
    if s.is_none() {
        eprintln!("skipped: no WAYLAND_DISPLAY or DISPLAY");
    }
    s
}

fn on_ci() -> bool {
    std::env::var_os("CI").is_some()
}

fn rgba_at(b: &Bitmap, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * b.width() + x) * 4) as usize;
    let p = &b.pixels()[i..i + 4];
    match b.format() {
        PixelFormat::Rgba8 => [p[0], p[1], p[2]],
        PixelFormat::Bgra8 => [p[2], p[1], p[0]],
    }
}

#[test]
fn capture_all_returns_monitor_sized_frames() {
    let Some(s) = session() else { return };
    let frames = futures_lite::future::block_on(ports(s).screen.capture_all()).unwrap();
    assert!(!frames.is_empty());
    for f in &frames {
        assert!(f.image.width() > 0 && f.image.height() > 0);
        assert_eq!((f.monitor.rect.width, f.monitor.rect.height), (f.image.width(), f.image.height()));
        if on_ci() {
            let centre = rgba_at(&f.image, f.image.width() / 2, f.image.height() / 2);
            assert_eq!(centre, [0x33, 0x66, 0x99], "centre pixel of {}", f.monitor.name);
        }
    }
}

#[test]
fn clipboard_round_trip_size() {
    let Some(s) = session() else { return };
    let px: Vec<u8> = std::iter::repeat_n([0x33u8, 0x66, 0x99, 255], 16 * 8).flatten().collect();
    let bmp = Bitmap::new(16, 8, PixelFormat::Rgba8, Arc::from(px)).unwrap();
    let p = ports(s);
    if let Err(e) = p.clipboard.set_image(&bmp) {
        eprintln!("skipped: clipboard unavailable here: {e}");
        return;
    }
    let mut reader = arboard::Clipboard::new().unwrap();
    let img = reader.get_image().unwrap();
    assert_eq!((img.width, img.height), (16, 8));
}

#[test]
fn hotkey_register_matches_session() {
    let Some(s) = session() else { return };
    let chord = KeyChord { modifiers: Modifiers::empty(), key: Key::PrintScreen };
    let results = ports(s).hotkeys.register(&[(chord, Action::PrintScreen)], Arc::new(|_| {}));
    assert_eq!(results.len(), 1);
    match (s, &results[0].1) {
        (Session::X11, r) => assert!(r.is_ok(), "{r:?}"),
        (Session::Wayland, Err(PortError::Unsupported(_))) => {}
        (Session::Wayland, r) if !on_ci() => eprintln!("wayland hotkeys on this desktop: {r:?}"),
        (Session::Wayland, r) => panic!("expected Unsupported on headless sway, got {r:?}"),
    }
}
