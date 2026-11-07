#![allow(clippy::unwrap_used)]

//! Scripted-input tests for the Region session. A poll runs the session until it waits for input.
//! The test sends input and polls again, and the session finishes. A script that never finishes
//! fails its assertion instead of hanging.

use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex};

use futures_lite::future::{block_on, poll_once};
use laqta_domain::capture::{RegionCaptureOptions, RegionMouseAction};
use laqta_domain::region::{Button, HoverTarget, KeyMods, RegionConfig, RegionInput, RegionKey, RegionState, TargetKind};
use laqta_domain::{Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat};
use laqta_imaging::Codecs;
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeHoverSource, FakeNotifier, FakeOverlay, FakeScreen, FakeShell, InlineSpawner,
    ManualClock,
};
use laqta_ports::{AppError, BoxFuture, HostInfo, HoverSource, PortError, RegionView};

use super::{Selection, crop_frames, select, view};
use crate::dispatcher::Ports;

/// The left monitor's pixel as the frame holds it (BGRA), and as the composed image holds it (RGBA).
const LEFT_BGRA: [u8; 4] = [10, 20, 30, 255];
const LEFT: [u8; 4] = [30, 20, 10, 255];
const RIGHT_BGRA: [u8; 4] = [40, 50, 60, 255];
const RIGHT: [u8; 4] = [60, 50, 40, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

/// Runs `body` in a 2-thread compute pool, because compose checks that it runs inside one (ADR 024).
fn in_pool<R: Send>(body: impl FnOnce() -> R + Send) -> R {
    rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap().install(body)
}

fn frame(x: i32, bgra: [u8; 4]) -> Frame {
    let pixels: Vec<u8> = std::iter::repeat_n(bgra, 16).flatten().collect();
    Frame {
        monitor: Monitor {
            id: MonitorId(format!("monitor-{x}")),
            name: format!("Monitor {x}"),
            rect: PhysRect { x, y: 0, width: 4, height: 4 },
            scale: 1.0,
            primary: x == 0,
        },
        image: Bitmap::new(4, 4, PixelFormat::Bgra8, pixels.into()).unwrap(),
    }
}

/// Two 4x4 monitors side by side: an 8x4 desktop.
fn two_frames() -> Vec<Frame> {
    vec![frame(0, LEFT_BGRA), frame(4, RIGHT_BGRA)]
}

fn options() -> RegionCaptureOptions {
    // The monitors are 4 px wide. The default minimum of 5 would turn every drag into a click.
    RegionCaptureOptions { min_size: 2, ..RegionCaptureOptions::default() }
}

fn test_ports(overlay: Arc<FakeOverlay>, hover: Arc<dyn HoverSource>) -> Ports {
    Ports {
        screen: Arc::new(FakeScreen::new(Vec::new())),
        clipboard: Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed)),
        shell: Arc::new(FakeShell::default()),
        clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0))),
        spawner: Arc::new(InlineSpawner::default()),
        overlay,
        tools: Arc::new(laqta_ports::fakes::FakeToolHost::default()),
        prompt: Arc::new(laqta_ports::fakes::FakePrompt::default()),
        editor: Arc::new(laqta_ports::fakes::FakeEditorHost::default()),
        media: Arc::new(laqta_ports::fakes::FakeMedia::default()),
        stream: Arc::new(laqta_ports::fakes::FakeScreenStream::default()),
        ocr: Arc::new(laqta_ports::fakes::FakeOcr::default()),
        window_ops: Arc::new(laqta_ports::fakes::FakeWindowOps::default()),
        input: Arc::new(laqta_ports::fakes::FakeInputSynth::default()),
        highlighter: Arc::new(laqta_ports::fakes::FakeHighlighter::default()),
        background: Arc::new(laqta_ports::fakes::FakeBackgroundRemover::default()),
        analyzer: Arc::new(laqta_ports::fakes::FakeAnalyzer::default()),
        inpainter: Arc::new(laqta_ports::fakes::FakeInpainter),
        watcher: Arc::new(laqta_ports::fakes::FakeFileWatcher::default()),
        uploaders: Arc::new(laqta_ports::fakes::FakeUploaders::default()),
        hover,
        notifier: Arc::new(FakeNotifier::default()),
        rng: Arc::new(CountingRng::default()),
        host: HostInfo::default(),
        codecs: Codecs::builtin(),
    }
}

/// Polls the session once. Returns its result if it finished.
fn step<F: Future>(session: Pin<&mut F>) -> Option<F::Output> {
    block_on(poll_once(session))
}

fn pixel(image: &Bitmap, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * image.width() + x) * 4) as usize;
    image.pixels()[i..i + 4].try_into().unwrap()
}

fn pixels(image: &Bitmap) -> Vec<[u8; 4]> {
    image.pixels().as_chunks::<4>().0.to_vec()
}

fn drag(overlay: &FakeOverlay, from: (i32, i32), to: (i32, i32)) {
    overlay.send(RegionInput::PointerDown { x: from.0, y: from.1, button: Button::Left });
    overlay.send(RegionInput::PointerMove { x: to.0, y: to.1 });
    overlay.send(RegionInput::PointerUp { x: to.0, y: to.1, button: Button::Left });
}

fn click(overlay: &FakeOverlay, at: (i32, i32)) {
    overlay.send(RegionInput::PointerDown { x: at.0, y: at.1, button: Button::Left });
    overlay.send(RegionInput::PointerUp { x: at.0, y: at.1, button: Button::Left });
}

fn press(overlay: &FakeOverlay, key: RegionKey) {
    overlay.send(RegionInput::Key { key, mods: KeyMods::default() });
}

fn window_target(rect: PhysRect, title: &str) -> HoverTarget {
    HoverTarget { handle: 0, rect, title: title.to_owned(), process: "notes.exe".to_owned(), kind: TargetKind::Window }
}

/// A hover source that records how many overlay views existed when the targets were asked for.
#[derive(Debug)]
struct ProbeHover {
    overlay: Arc<FakeOverlay>,
    targets: Vec<HoverTarget>,
    views_when_asked: Mutex<Option<usize>>,
}

impl HoverSource for ProbeHover {
    fn targets(&self) -> BoxFuture<'_, Result<Vec<HoverTarget>, PortError>> {
        *self.views_when_asked.lock().unwrap() = Some(self.overlay.views.lock().unwrap().len());
        let targets = self.targets.clone();
        Box::pin(async move { Ok::<_, PortError>(targets) })
    }

    fn active_window(&self) -> BoxFuture<'_, Result<Option<HoverTarget>, PortError>> {
        Box::pin(async { Ok::<_, PortError>(None) })
    }
}

fn probe(overlay: &Arc<FakeOverlay>, targets: Vec<HoverTarget>) -> Arc<ProbeHover> {
    Arc::new(ProbeHover { overlay: overlay.clone(), targets, views_when_asked: Mutex::new(None) })
}

#[test]
fn drag_on_the_second_monitor_crops_the_second_frame() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let opts = options();
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());
        assert!(overlay.is_open());

        drag(&overlay, (5, 1), (6, 2));
        let selection = step(session.as_mut()).expect("the drag confirms").unwrap();

        assert_eq!(selection.rects, vec![PhysRect { x: 5, y: 1, width: 2, height: 2 }]);
        assert_eq!(selection.target, None);
        assert_eq!((selection.image.width(), selection.image.height()), (2, 2));
        assert_eq!(pixels(&selection.image), vec![RIGHT; 4]);
        assert!(!overlay.is_open());
    });
}

#[test]
fn click_on_a_window_target_returns_that_window() {
    in_pool(|| {
        let target = window_target(PhysRect { x: 2, y: 0, width: 4, height: 4 }, "Notes");
        let overlay = Arc::new(FakeOverlay::default());
        let hover = Arc::new(FakeHoverSource::new(vec![target.clone()]));
        let ports = test_ports(overlay.clone(), hover);
        let opts = options();
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());

        click(&overlay, (3, 1));
        let selection = step(session.as_mut()).expect("the click confirms").unwrap();

        assert_eq!(selection.rects, vec![target.rect]);
        assert_eq!(selection.target, Some(target));
        assert_eq!((selection.image.width(), selection.image.height()), (4, 4));
        // The window spans x 2..6: its left half is the left monitor, its right half the right one.
        assert_eq!(pixel(&selection.image, 0, 0), LEFT);
        assert_eq!(pixel(&selection.image, 2, 0), RIGHT);
    });
}

#[test]
fn escape_cancels_the_selection() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let opts = options();
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());

        press(&overlay, RegionKey::Escape);

        assert!(matches!(step(session.as_mut()), Some(Err(AppError::Cancelled))));
        assert!(!overlay.is_open());
    });
}

#[test]
fn multi_region_enter_keeps_both_rects_with_a_transparent_gap() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let options = RegionCaptureOptions { multi_region: true, ..options() };
        let mut session = pin!(select(&ports, two_frames(), &options));
        assert!(step(session.as_mut()).is_none());

        // The first region's handles reach x 5 at most, so the second press at x 6 draws a new one.
        drag(&overlay, (0, 0), (1, 3));
        drag(&overlay, (6, 0), (7, 3));
        assert!(step(session.as_mut()).is_none());
        press(&overlay, RegionKey::Enter);
        let selection = step(session.as_mut()).expect("Enter confirms").unwrap();

        assert_eq!(selection.rects, vec![PhysRect { x: 0, y: 0, width: 2, height: 4 }, PhysRect { x: 6, y: 0, width: 2, height: 4 }]);
        assert_eq!(selection.target, None);
        assert_eq!((selection.image.width(), selection.image.height()), (8, 4));
        assert_eq!(pixel(&selection.image, 0, 0), LEFT);
        assert_eq!(pixel(&selection.image, 1, 3), LEFT);
        assert_eq!(pixel(&selection.image, 2, 0), CLEAR);
        assert_eq!(pixel(&selection.image, 5, 3), CLEAR);
        assert_eq!(pixel(&selection.image, 6, 0), RIGHT);
        assert_eq!(pixel(&selection.image, 7, 3), RIGHT);
    });
}

#[test]
fn unsupported_hover_source_still_allows_a_drag() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let opts = options();
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());

        drag(&overlay, (0, 0), (2, 3));
        let selection = step(session.as_mut()).expect("the drag confirms").unwrap();

        assert_eq!(selection.rects, vec![PhysRect { x: 0, y: 0, width: 3, height: 4 }]);
        assert_eq!(selection.target, None);
    });
}

#[test]
fn overlay_show_error_is_returned_before_targets_are_asked_for() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        *overlay.fail_show.lock().unwrap() = Some(PortError::Failed("no display".to_owned()));
        let hover = probe(&overlay, Vec::new());
        let ports = test_ports(overlay.clone(), hover.clone());
        let opts = options();
        let mut session = pin!(select(&ports, two_frames(), &opts));

        match step(session.as_mut()) {
            Some(Err(AppError::Io(message))) => assert_eq!(message, "no display"),
            other => panic!("expected the show error, got {other:?}"),
        }
        assert_eq!(*hover.views_when_asked.lock().unwrap(), None);
    });
}

#[test]
fn the_first_view_is_shown_before_the_targets_load() {
    in_pool(|| {
        // The target covers the cursor's starting point, the centre of the 8x4 desktop.
        let target = window_target(PhysRect { x: 2, y: 0, width: 4, height: 4 }, "Notes");
        let overlay = Arc::new(FakeOverlay::default());
        let hover = probe(&overlay, vec![target.clone()]);
        let ports = test_ports(overlay.clone(), hover.clone());
        let opts = options();
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());

        assert_eq!(*hover.views_when_asked.lock().unwrap(), Some(1), "the overlay is shown before the targets are asked for");
        let views: Vec<RegionView> = overlay.views.lock().unwrap().clone();
        assert_eq!(views.len(), 2, "the shown view, then one update when the targets arrive");
        assert_eq!(views[0].hovered, None);
        assert_eq!(views[1].hovered, Some(target));

        press(&overlay, RegionKey::Escape);
        assert!(matches!(step(session.as_mut()), Some(Err(AppError::Cancelled))));
    });
}

#[test]
fn moves_repaint_the_overlay() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let opts = options();
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());

        overlay.send(RegionInput::PointerMove { x: 1, y: 1 });
        overlay.send(RegionInput::PointerMove { x: 2, y: 1 });
        assert!(step(session.as_mut()).is_none());

        let views: Vec<RegionView> = overlay.views.lock().unwrap().clone();
        assert_eq!(views.len(), 3, "the shown view, then one per move");
        assert_eq!(views[2].cursor, (2, 1));
        assert_eq!(views[2].bounds, PhysRect { x: 0, y: 0, width: 8, height: 4 });

        press(&overlay, RegionKey::Escape);
        assert!(matches!(step(session.as_mut()), Some(Err(AppError::Cancelled))));
    });
}

#[test]
fn view_carries_the_magnifier_and_info_text_only_when_enabled() {
    let bounds = PhysRect { x: 0, y: 0, width: 8, height: 4 };
    let state = RegionState::new(RegionConfig {
        bounds,
        multi_region: false,
        snap_to_targets: true,
        min_size: 2,
        magnifier_pixels: 3,
        info_text: options().info_text,
        right_click: RegionMouseAction::RemoveShapeOrCancel,
        middle_click: RegionMouseAction::CancelCapture,
    });

    let on = view(&state, bounds, &options(), [1, 2, 3]);
    assert_eq!(on.bounds, bounds);
    assert_eq!(on.cursor, (4, 2));
    assert_eq!(on.magnifier, Some(PhysRect { x: 3, y: 1, width: 3, height: 3 }));
    assert_eq!(on.info_text.as_deref(), Some("X: 4 Y: 2\nR: 1 G: 2 B: 3\nHex: 010203"));

    let off = RegionCaptureOptions { show_magnifier: false, show_info: false, ..options() };
    let quiet = view(&state, bounds, &off, [1, 2, 3]);
    assert_eq!(quiet.magnifier, None);
    assert_eq!(quiet.info_text, None);
}

#[test]
fn crop_frames_clears_the_gap_between_two_rects() {
    in_pool(|| {
        let ports = test_ports(Arc::new(FakeOverlay::default()), Arc::new(FakeHoverSource::unsupported()));
        let rects = vec![PhysRect { x: 0, y: 0, width: 2, height: 4 }, PhysRect { x: 6, y: 0, width: 2, height: 4 }];

        let selection = block_on(crop_frames(&ports, two_frames(), rects.clone(), None)).unwrap();

        assert_eq!(selection.rects, rects);
        assert_eq!((selection.image.width(), selection.image.height()), (8, 4));
        assert_eq!(pixel(&selection.image, 0, 0), LEFT);
        assert_eq!(pixel(&selection.image, 2, 0), CLEAR);
        assert_eq!(pixel(&selection.image, 6, 0), RIGHT);
    });
}

#[test]
fn crop_frames_single_rect_on_the_second_monitor() {
    in_pool(|| {
        let ports = test_ports(Arc::new(FakeOverlay::default()), Arc::new(FakeHoverSource::unsupported()));
        let rects = vec![PhysRect { x: 4, y: 0, width: 4, height: 4 }];

        let selection = block_on(crop_frames(&ports, two_frames(), rects, None)).unwrap();

        assert_eq!((selection.image.width(), selection.image.height()), (4, 4));
        assert_eq!(pixels(&selection.image), vec![RIGHT; 16]);
    });
}

#[test]
fn select_future_is_send() {
    fn assert_send<T: Send>(_: &T) {}
    let ports = test_ports(Arc::new(FakeOverlay::default()), Arc::new(FakeHoverSource::unsupported()));
    let opts = options();
    let session = select(&ports, two_frames(), &opts);
    assert_send(&session);
}

#[test]
fn right_click_with_the_fullscreen_action_returns_the_whole_desktop() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let opts = RegionCaptureOptions { right_click: RegionMouseAction::CaptureFullscreen, ..options() };
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());

        overlay.send(RegionInput::PointerDown { x: 1, y: 1, button: Button::Right });
        let selection = step(session.as_mut()).expect("the right click confirms").unwrap();

        assert_eq!(selection.rects, vec![PhysRect { x: 0, y: 0, width: 8, height: 4 }]);
        assert_eq!(selection.target, None);
        assert_eq!((selection.image.width(), selection.image.height()), (8, 4));
        assert_eq!(pixel(&selection.image, 0, 0), LEFT);
        assert_eq!(pixel(&selection.image, 7, 3), RIGHT);
        assert!(!overlay.is_open());
    });
}

/// Middle-clicks at `at` with the active-monitor action, and returns the selection.
fn middle_click_selection(at: (i32, i32)) -> Selection {
    in_pool(move || {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let opts = RegionCaptureOptions { middle_click: RegionMouseAction::CaptureActiveMonitor, ..options() };
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());

        overlay.send(RegionInput::PointerDown { x: at.0, y: at.1, button: Button::Middle });
        step(session.as_mut()).expect("the middle click confirms").unwrap()
    })
}

#[test]
fn middle_click_with_the_active_monitor_action_returns_the_monitor_under_the_cursor() {
    let right = middle_click_selection((6, 1));
    assert_eq!(right.rects, vec![PhysRect { x: 4, y: 0, width: 4, height: 4 }]);
    assert_eq!(right.target, None);
    assert_eq!((right.image.width(), right.image.height()), (4, 4));
    assert_eq!(pixels(&right.image), vec![RIGHT; 16]);

    let left = middle_click_selection((1, 1));
    assert_eq!(left.rects, vec![PhysRect { x: 0, y: 0, width: 4, height: 4 }]);
    assert_eq!(pixels(&left.image), vec![LEFT; 16]);
}
