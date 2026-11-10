//! The adapters against the real OS. They run on macOS only, when `LAQTA_CONTRACT=1` (the CI
//! runner), never on a desktop session someone is using.
//!
//! No Mac is available where Laqta is developed, so each test prints what the OS answered: the CI
//! log is how the result is read. A test fails only when the answer is wrong. A runner without the
//! Screen Recording or Accessibility permission gets the adapter's "Laqta needs the … permission"
//! message, which counts as a correct answer and is printed.
//!
//! Ports that work on the main thread (pasteboard, menu bar item, hotkeys, sounds) are not here:
//! the test harness owns the main thread and never runs its queue. The app run in
//! `ci/mac-smoke.sh` covers them.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::time::{Duration, Instant};

use futures_lite::future::block_on;
use laqta_domain::geometry::PhysRect;
use laqta_domain::region::TargetKind;
use laqta_domain::{Bitmap, PixelFormat};
use laqta_ports::{AudioInput, CaptureRequest, PortError, VideoSource};

fn enabled() -> bool {
    let on = std::env::var_os("LAQTA_CONTRACT").is_some_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set LAQTA_CONTRACT=1");
    }
    on
}

/// True when the error is the adapter saying a macOS permission is missing.
fn needs_permission(e: &PortError) -> bool {
    matches!(e, PortError::PermissionDenied) || e.to_string().contains("permission")
}

#[test]
fn screenshots_give_one_frame_per_display() {
    if !enabled() {
        return;
    }
    let ports = laqta_platform_macos::ports();
    let started = Instant::now();
    match block_on(ports.screen.capture(CaptureRequest { show_cursor: true })) {
        Ok(frames) => {
            eprintln!("capture: {} frame(s) in {:?}", frames.len(), started.elapsed());
            assert!(!frames.is_empty());
            for frame in &frames {
                let (m, image) = (&frame.monitor, &frame.image);
                eprintln!(
                    "  {:?} rect {:?} scale {} primary {} image {}x{}",
                    m.id,
                    m.rect,
                    m.scale,
                    m.primary,
                    image.width(),
                    image.height()
                );
                assert_eq!((image.width(), image.height()), (m.rect.width, m.rect.height), "a frame is its monitor's pixel size");
                let opaque = image.pixels().as_chunks::<4>().0.iter().all(|px| px[3] == 255);
                let varied = image.pixels().as_chunks::<4>().0.iter().any(|px| px[..3] != image.pixels()[..3]);
                eprintln!("  opaque {opaque}, more than one colour {varied}");
                assert!(opaque);
            }
            assert_eq!(frames.iter().filter(|f| f.monitor.primary).count(), 1);
        }
        Err(e) => {
            eprintln!("capture refused: {e}");
            assert!(needs_permission(&e), "unexpected capture error: {e}");
        }
    }
}

#[test]
fn hover_lists_windows_without_a_permission() {
    if !enabled() {
        return;
    }
    let ports = laqta_platform_macos::ports();
    let targets = block_on(ports.hover.targets()).unwrap();
    eprintln!("hover: {} target(s)", targets.len());
    for t in targets.iter().take(12) {
        eprintln!("  {:?} {:?} handle {} {:?} / {:?}", t.kind, t.rect, t.handle, t.process, t.title);
    }
    assert!(targets.iter().all(|t| t.rect.width > 0 && t.rect.height > 0));
    let active = block_on(ports.hover.active_window()).unwrap();
    eprintln!("active window: {:?}", active.as_ref().map(|t| (&t.process, t.rect)));
    if let Some(window) = targets.iter().find(|t| t.kind == TargetKind::Window) {
        let details = ports.window_ops.inspect(window).unwrap();
        eprintln!("inspect: {details:?}");
        assert!(!details.is_empty());
    }
}

#[test]
fn recording_delivers_frames_at_the_asked_size() {
    if !enabled() {
        return;
    }
    let ports = laqta_platform_macos::ports();
    let devices = block_on(ports.stream.audio_devices());
    eprintln!("audio devices: {devices:?}");
    eprintln!("cameras: {:?}", block_on(ports.stream.camera_devices()));
    let system: Vec<_> = devices.unwrap_or_default().into_iter().filter(|d| d.loopback).collect();
    let area = PhysRect { x: 0, y: 0, width: 640, height: 480 };
    let mut input = match block_on(ports.stream.open(area, true, 30, &system)) {
        Ok(input) => input,
        Err(e) => {
            eprintln!("stream refused: {e}");
            assert!(needs_permission(&e), "unexpected stream error: {e}");
            return;
        }
    };
    let VideoSource::Frames(frames) = &mut input.video else { panic!("macOS records from frames") };
    assert_eq!(frames.size(), (640, 480));
    // An idle screen sends few frames, so count what arrives in two seconds and require one.
    let (started, mut count) = (Instant::now(), 0);
    while started.elapsed() < Duration::from_secs(2) {
        match frames.next_frame() {
            Ok(Some(frame)) => {
                assert_eq!((frame.width(), frame.height()), (640, 480));
                count += 1;
            }
            Ok(None) => break,
            Err(e) => panic!("next_frame: {e}"),
        }
    }
    eprintln!("stream: {count} frame(s) in {:?}", started.elapsed());
    for audio in &mut input.audio {
        if let AudioInput::Pcm(pcm) = audio {
            let mut buf = vec![0i16; 4800];
            let read = pcm.read(&mut buf);
            eprintln!("system audio: format {:?}, read {read:?}", pcm.format());
            pcm.stop();
        }
    }
    frames.stop();
    frames.stop();
    assert!(count >= 1, "no frame in two seconds");
}

#[test]
fn ocr_lists_languages_and_reads_an_empty_image_as_no_text() {
    if !enabled() {
        return;
    }
    let ports = laqta_platform_macos::ports();
    let languages = ports.ocr.languages();
    eprintln!("ocr languages: {languages:?}");
    assert!(!languages.is_empty());
    let white = Bitmap::new(200, 80, PixelFormat::Rgba8, vec![255u8; 200 * 80 * 4].into()).unwrap();
    let result = block_on(ports.ocr.recognize(white, String::new())).unwrap();
    eprintln!("ocr of a white image: {:?}", result.lines);
    assert!(result.lines.is_empty());
}

#[test]
fn secrets_round_trip_through_the_keychain() {
    if !enabled() {
        return;
    }
    let ports = laqta_platform_macos::ports();
    let key = format!("laqta-contract-{}", std::process::id());
    ports.secrets.set(&key, "one").unwrap();
    ports.secrets.set(&key, "two").unwrap();
    assert_eq!(ports.secrets.get(&key).unwrap().as_deref(), Some("two"));
    ports.secrets.delete(&key).unwrap();
    assert_eq!(ports.secrets.get(&key).unwrap(), None);
    ports.secrets.delete(&key).unwrap();
}

#[test]
fn what_needs_an_app_bundle_or_a_permission_says_so() {
    if !enabled() {
        return;
    }
    let ports = laqta_platform_macos::ports();
    eprintln!("autostart outside a bundle: {:?}", ports.autostart.is_enabled());
    eprintln!("integration installed: {:?}", ports.integration.is_installed());
    let scroll = ports.input.scroll(100, 100, laqta_domain::capture::ScrollMethod::MouseWheel, 1);
    eprintln!("synthetic scroll: {scroll:?}");
    let highlighter = ports.highlighter.set_enabled(true);
    eprintln!("pointer highlighter on: {highlighter:?}, enabled {}", ports.highlighter.is_enabled());
    let _ = ports.highlighter.set_enabled(false);
    assert!(!ports.highlighter.is_enabled());
}
