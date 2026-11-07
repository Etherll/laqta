#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! M1 dispatcher routes: every capture mode, windows, hotkey toggle, and the routing table.

use std::path::Path;
use std::sync::Arc;

use futures_lite::future::{block_on, poll_once};
use laqta_app::{App, Dispatched, Ports};
use laqta_domain::region::{Button, HoverTarget, KeyMods, RegionInput, RegionKey, TargetKind};
use laqta_domain::{Action, Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat, Settings};
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeHoverSource, FakeNotifier, FakeOverlay, FakeScreen, FakeShell, FakeToolHost,
    InlineSpawner, ManualClock,
};
use laqta_ports::{AppError, HostInfo, ToolWindow};
use tempfile::TempDir;

struct H {
    app: Arc<App>,
    overlay: Arc<FakeOverlay>,
    tools: Arc<FakeToolHost>,
    spawner: Arc<InlineSpawner>,
    screen: Arc<FakeScreen>,
}

fn frame(x: i32, colour: u8) -> Frame {
    Frame {
        monitor: Monitor {
            id: MonitorId(format!("m{x}")),
            name: format!("M{x}"),
            rect: PhysRect { x, y: 0, width: 8, height: 8 },
            scale: 1.0,
            primary: x == 0,
        },
        image: Bitmap::new(8, 8, PixelFormat::Bgra8, vec![colour; 256].into()).unwrap(),
    }
}

fn window() -> HoverTarget {
    HoverTarget {
        handle: 0,
        rect: PhysRect { x: 9, y: 1, width: 4, height: 3 },
        title: "Notes - Editor".into(),
        process: "editor".into(),
        kind: TargetKind::Window,
    }
}

fn harness(dir: &Path, edit: impl FnOnce(&mut Settings)) -> H {
    let overlay = Arc::new(FakeOverlay::default());
    let tools = Arc::new(FakeToolHost::default());
    let spawner = Arc::new(InlineSpawner::default());
    let hover = FakeHoverSource::new(vec![window()]);
    *hover.active.lock().unwrap() = Some(window());
    let screen = Arc::new(FakeScreen::new(vec![frame(0, 10), frame(8, 200)]));
    let ports = Ports {
        screen: screen.clone(),
        clipboard: Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed)),
        shell: Arc::new(FakeShell::default()),
        clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0))),
        spawner: spawner.clone(),
        overlay: overlay.clone(),
        tools: tools.clone(),
        prompt: Arc::new(laqta_ports::fakes::FakePrompt::default()),
        editor: Arc::new(laqta_ports::fakes::FakeEditorHost::default()),
        media: Arc::new(laqta_ports::fakes::FakeMedia::default()),
        stream: Arc::new(laqta_ports::fakes::FakeScreenStream::default()),
        ocr: Arc::new(laqta_ports::fakes::FakeOcr::default()),
        window_ops: Arc::new(laqta_ports::fakes::FakeWindowOps::default()),
        input: Arc::new(laqta_ports::fakes::FakeInputSynth::default()),
        watcher: Arc::new(laqta_ports::fakes::FakeFileWatcher::default()),
        uploaders: Arc::new(laqta_ports::fakes::FakeUploaders::default()),
        hover: Arc::new(hover),
        notifier: Arc::new(FakeNotifier::default()),
        rng: Arc::new(CountingRng::default()),
        host: HostInfo::default(),
        codecs: laqta_imaging::Codecs::builtin(),
    };
    let mut settings = Settings::default_for(dir.to_path_buf());
    settings.screenshots_dir = dir.to_path_buf();
    settings.defaults.capture.region.min_size = 2;
    edit(&mut settings);
    H { app: App::new(ports, settings), overlay, tools, spawner, screen }
}

/// A window bitmap that is not the 4x3 crop, so a test can tell which one was saved.
fn window_bitmap(width: u32, height: u32) -> Bitmap {
    Bitmap::new(width, height, PixelFormat::Rgba8, vec![0u8; (width * height * 4) as usize].into()).unwrap()
}

/// Runs `f` inside a compute pool (imaging asserts it) and drives spawned jobs to the end.
fn run<T: Send>(h: &H, f: impl std::future::Future<Output = T> + Send) -> T {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    pool.install(|| {
        block_on(async {
            let out = f.await;
            loop {
                let jobs = h.spawner.take_spawned();
                if jobs.is_empty() {
                    break;
                }
                for j in jobs {
                    j.await;
                }
            }
            out
        })
    })
}

fn saved_size(dir: &Path) -> (u32, u32) {
    let month = dir.join("2026-11");
    let file = std::fs::read_dir(&month).unwrap().next().unwrap().unwrap().path();
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(file).unwrap()));
    let r = dec.read_info().unwrap();
    (r.info().width, r.info().height)
}

#[test]
fn active_window_crops_to_the_window() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), |_| {});
    let r = run(&h, h.app.dispatch(Action::ActiveWindow, None));
    assert!(matches!(r, Ok(Dispatched::Task(_))));
    assert_eq!(saved_size(tmp.path()), (4, 3));
    // window name pattern uses the process name
    let name = std::fs::read_dir(tmp.path().join("2026-11")).unwrap().next().unwrap().unwrap().file_name();
    assert!(name.to_string_lossy().starts_with("editor_"), "{name:?}");
}

#[test]
fn transparent_active_window_saves_the_window_bitmap() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), |s| s.defaults.capture.transparent_window = true);
    *h.screen.window_image.lock().unwrap() = Some(window_bitmap(6, 5));
    run(&h, h.app.dispatch(Action::ActiveWindow, None)).unwrap();
    assert_eq!(saved_size(tmp.path()), (6, 5));
    // The target's process name still names the file, so the target info reached the task.
    let name = std::fs::read_dir(tmp.path().join("2026-11")).unwrap().next().unwrap().unwrap().file_name();
    assert!(name.to_string_lossy().starts_with("editor_"), "{name:?}");
}

#[test]
fn transparent_active_window_falls_back_to_the_crop_when_unsupported() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), |s| s.defaults.capture.transparent_window = true);
    run(&h, h.app.dispatch(Action::ActiveWindow, None)).unwrap();
    assert_eq!(saved_size(tmp.path()), (4, 3));
}

#[test]
fn window_bitmap_is_ignored_when_transparent_is_off() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), |_| {});
    *h.screen.window_image.lock().unwrap() = Some(window_bitmap(6, 5));
    run(&h, h.app.dispatch(Action::ActiveWindow, None)).unwrap();
    assert_eq!(saved_size(tmp.path()), (4, 3));
}

#[test]
fn transparent_custom_window_saves_the_window_bitmap() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), |s| {
        s.defaults.capture.custom_window_title = "notes".into();
        s.defaults.capture.transparent_window = true;
    });
    *h.screen.window_image.lock().unwrap() = Some(window_bitmap(6, 5));
    run(&h, h.app.dispatch(Action::CustomWindow, None)).unwrap();
    assert_eq!(saved_size(tmp.path()), (6, 5));
}

#[test]
fn active_monitor_takes_the_monitor_under_the_window() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), |_| {});
    run(&h, h.app.dispatch(Action::ActiveMonitor, None)).unwrap();
    assert_eq!(saved_size(tmp.path()), (8, 8));
}

#[test]
fn custom_region_and_missing_custom_region() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), |s| s.defaults.capture.custom_region = Some(PhysRect { x: 6, y: 2, width: 5, height: 4 }));
    run(&h, h.app.dispatch(Action::CustomRegion, None)).unwrap();
    assert_eq!(saved_size(tmp.path()), (5, 4));
    let tmp2 = TempDir::new().unwrap();
    let h2 = harness(tmp2.path(), |_| {});
    assert!(matches!(run(&h2, h2.app.dispatch(Action::CustomRegion, None)), Err(AppError::Unsupported(_))));
}

#[test]
fn custom_window_matches_title() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), |s| s.defaults.capture.custom_window_title = "notes".into());
    run(&h, h.app.dispatch(Action::CustomWindow, None)).unwrap();
    assert_eq!(saved_size(tmp.path()), (4, 3));
}

#[test]
fn region_then_last_region() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), |_| {});
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    pool.install(|| {
        block_on(async {
            let mut fut = h.app.dispatch(Action::RectangleRegion, None);
            while !h.overlay.is_open() {
                assert!(poll_once(&mut fut).await.is_none());
            }
            for i in [
                RegionInput::PointerMove { x: 1, y: 1 },
                RegionInput::PointerDown { x: 1, y: 1, button: Button::Left },
                RegionInput::PointerMove { x: 6, y: 5 },
                RegionInput::PointerUp { x: 6, y: 5, button: Button::Left },
            ] {
                h.overlay.send(i);
            }
            assert!(matches!(fut.await, Ok(Dispatched::Task(_))));
        })
    });
    run(&h, async {});
    let first = saved_size(tmp.path());
    assert_eq!(first.0, first.1 + 1);
    // LastRegion reuses the rect without opening the overlay.
    let shown_before = h.overlay.views.lock().unwrap().len();
    run(&h, h.app.dispatch(Action::LastRegion, None)).unwrap();
    assert_eq!(h.overlay.views.lock().unwrap().len(), shown_before);
    assert_eq!(std::fs::read_dir(tmp.path().join("2026-11")).unwrap().count(), 2);
}

#[test]
fn region_cancel_is_cancelled() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), |_| {});
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let r = pool.install(|| {
        block_on(async {
            let mut fut = h.app.dispatch(Action::RectangleRegion, None);
            while !h.overlay.is_open() {
                assert!(poll_once(&mut fut).await.is_none());
            }
            h.overlay.send(RegionInput::Key { key: RegionKey::Escape, mods: KeyMods::default() });
            fut.await
        })
    });
    assert!(matches!(r, Err(AppError::Cancelled)));
}

#[test]
fn windows_open_through_the_tool_host() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), |_| {});
    for a in [Action::OpenMainWindow, Action::OpenSettings, Action::OpenHistory, Action::OpenImageHistory] {
        assert_eq!(run(&h, h.app.dispatch(a, None)).unwrap(), Dispatched::Done);
    }
    assert_eq!(
        *h.tools.opened.lock().unwrap(),
        vec![ToolWindow::Main, ToolWindow::Settings, ToolWindow::History, ToolWindow::ImageHistory]
    );
}

#[test]
fn disable_hotkeys_toggles() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), |_| {});
    assert_eq!(run(&h, h.app.dispatch(Action::DisableHotkeys, None)).unwrap(), Dispatched::HotkeysToggled(true));
    assert!(h.app.settings().disable_hotkeys);
    assert_eq!(run(&h, h.app.dispatch(Action::DisableHotkeys, None)).unwrap(), Dispatched::HotkeysToggled(false));
}

#[test]
fn workflow_settings_apply() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), |s| {
        s.workflows[0].patch.image =
            Some(laqta_domain::workflow::ImageSettings { format: laqta_domain::ImageFormat::Bmp, ..Default::default() });
    });
    let wf = h.app.settings().workflows[0].id;
    run(&h, h.app.dispatch(Action::PrintScreen, Some(wf))).unwrap();
    let name = std::fs::read_dir(tmp.path().join("2026-11")).unwrap().next().unwrap().unwrap().file_name();
    assert!(name.to_string_lossy().ends_with(".bmp"), "{name:?}");
}

/// The routing table: every action returns without hanging or panicking. Region-selecting actions
/// (which wait for overlay input) are covered above.
#[test]
fn every_action_routes() {
    // Region selections wait on the overlay; recordings start a driver that runs until stopped.
    let waits = [
        Action::RectangleRegion,
        Action::RectangleLight,
        Action::RectangleTransparent,
        Action::LastRegion,
        Action::ScreenRecorder,
        Action::ScreenRecorderActiveWindow,
        Action::StartScreenRecorder,
        Action::ScreenRecorderGif,
        Action::ScreenRecorderGifActiveWindow,
        Action::StartScreenRecorderGif,
        // Scrolling capture waits on the overlay; auto capture keeps a timer loop running.
        Action::ScrollingCapture,
        Action::AutoCapture,
        Action::StartAutoCapture,
        // Tools that select a region first.
        Action::ScreenColorPicker,
        Action::Ruler,
        Action::PinToScreen,
        Action::PinToScreenFromScreen,
        Action::Ocr,
        Action::QrCodeScanRegion,
    ];
    for &a in Action::ALL.iter().filter(|a| !waits.contains(a)) {
        let tmp = TempDir::new().unwrap();
        let h = harness(tmp.path(), |_| {});
        let r = run(&h, h.app.dispatch(a, None));
        match a {
            Action::ExitLaqta => assert_eq!(r.unwrap(), Dispatched::Exit),
            Action::CustomRegion | Action::CustomWindow => assert!(r.is_err()),
            _ => {
                if let Err(e) = &r {
                    assert!(matches!(e, AppError::Unsupported(_)), "{a:?}: {e:?}");
                }
            }
        }
    }
}

#[test]
fn tool_actions_open_their_windows() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), |_| {});
    for (action, kind) in [
        (Action::ColorPicker, laqta_ports::ToolKind::ColorPicker),
        (Action::HashCheck, laqta_ports::ToolKind::HashCheck),
        (Action::IndexFolder, laqta_ports::ToolKind::IndexFolder),
        (Action::NetworkMonitor, laqta_ports::ToolKind::NetworkMonitor),
    ] {
        assert_eq!(run(&h, h.app.dispatch(action, None)).unwrap(), Dispatched::Done);
        assert_eq!(h.tools.opened.lock().unwrap().last(), Some(&ToolWindow::Tool(kind)));
    }
}

#[test]
fn pins_and_highlighter() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), |_| {});
    h.tools.pins.lock().unwrap().push((4, 4));
    run(&h, h.app.dispatch(Action::PinToScreenCloseAll, None)).unwrap();
    assert!(h.tools.pins.lock().unwrap().is_empty());
    run(&h, h.app.dispatch(Action::MouseHighlighter, None)).unwrap();
    assert!(*h.tools.highlighter.lock().unwrap());
    run(&h, h.app.dispatch(Action::MouseHighlighter, None)).unwrap();
    assert!(!*h.tools.highlighter.lock().unwrap());
}
