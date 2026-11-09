#![allow(clippy::unwrap_used)]

//! Scripted-input tests for the Region session. A poll runs the session until it waits for input.
//! The test sends input and polls again, and the session finishes. A script that never finishes
//! fails its assertion instead of hanging.

use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex};

use futures_lite::future::{block_on, poll_once};
use laqta_domain::capture::{RegionCaptureOptions, RegionMouseAction};
use laqta_domain::region::{
    Button, HoverTarget, KeyMods, RegionCommand, RegionConfig, RegionInput, RegionKey, RegionState, SelectionAction, TargetKind,
};
use laqta_domain::{Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat};
use laqta_imaging::Codecs;
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeHoverSource, FakeNotifier, FakeOverlay, FakeScreen, FakeShell, InlineSpawner,
    ManualClock,
};
use laqta_ports::{AppError, BoxFuture, HostInfo, HoverSource, PortError, RegionView};

use super::{Selection, crop_frames, pixel_at, pixel_in_frames, select, view};
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
    // The monitors are 4 px wide. The default minimum of 5 would turn every drag into a click. Most tests
    // end the session with the drag; the selected-mode tests turn the tools back on.
    RegionCaptureOptions { min_size: 2, show_tools_after_select: false, ..RegionCaptureOptions::default() }
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
        assert_eq!(selection.action, SelectionAction::Default);
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
        confirm_on_select: true,
        pick_point: false,
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

#[test]
fn the_first_views_pixel_matches_the_composite_everywhere() {
    in_pool(|| {
        // Side by side, and overlapping (the later frame paints on top).
        for (frames, width) in [(two_frames(), 8), (vec![frame(0, LEFT_BGRA), frame(2, RIGHT_BGRA)], 6)] {
            let bounds = PhysRect { x: 0, y: 0, width, height: 4 };
            let composed = laqta_imaging::compose_monitors(&frames);
            for y in -1..=4 {
                for x in -1..=width as i32 {
                    assert_eq!(pixel_in_frames(&frames, (x, y)), pixel_at(&composed, bounds, (x, y)), "at ({x}, {y})");
                }
            }
        }
    });
}

/// Options for a session that keeps the dragged region selected, with the capture tools beside it.
fn selecting() -> RegionCaptureOptions {
    RegionCaptureOptions { show_tools_after_select: true, ..options() }
}

/// How many views the overlay was given with a selection image.
fn views_with_an_image(overlay: &FakeOverlay) -> usize {
    overlay.views.lock().unwrap().iter().filter(|v| v.selection_image.is_some()).count()
}

/// A 2x2 image no desktop pixel matches.
fn annotated_crop() -> Bitmap {
    Bitmap::new(2, 2, PixelFormat::Rgba8, [1, 2, 3, 255].repeat(4).into()).unwrap()
}

#[test]
fn a_dragged_region_stays_selected_until_finish_and_carries_the_action() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let opts = selecting();
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());

        drag(&overlay, (5, 1), (6, 2));
        assert!(step(session.as_mut()).is_none(), "the drag only selects the region");
        assert!(overlay.is_open());
        let shown = overlay.last_view().unwrap();
        assert_eq!(shown.selected, Some(PhysRect { x: 5, y: 1, width: 2, height: 2 }));
        assert!(shown.tools_visible);
        assert!(!shown.locked);
        let image = shown.selection_image.expect("the overlay gets the selection's pixels");
        assert_eq!((image.width(), image.height()), (2, 2));
        assert_eq!(pixels(&image), vec![RIGHT; 4]);

        overlay.send(RegionInput::Command(RegionCommand::Finish(SelectionAction::Copy)));
        let selection = step(session.as_mut()).expect("Finish confirms").unwrap();

        assert_eq!(selection.action, SelectionAction::Copy);
        assert_eq!(selection.rects, vec![PhysRect { x: 5, y: 1, width: 2, height: 2 }]);
        assert_eq!(pixels(&selection.image), vec![RIGHT; 4]);
        assert!(!overlay.is_open());
    });
}

#[test]
fn enter_on_a_selected_region_confirms_it_with_the_default_action() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let opts = selecting();
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());

        drag(&overlay, (5, 1), (6, 2));
        press(&overlay, RegionKey::Enter);
        let selection = step(session.as_mut()).expect("Enter confirms").unwrap();

        assert_eq!(selection.action, SelectionAction::Default);
        assert_eq!(selection.rects, vec![PhysRect { x: 5, y: 1, width: 2, height: 2 }]);
    });
}

#[test]
fn cancel_after_selecting_closes_the_overlay() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let opts = selecting();
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());

        drag(&overlay, (5, 1), (6, 2));
        overlay.send(RegionInput::Command(RegionCommand::Cancel));

        assert!(matches!(step(session.as_mut()), Some(Err(AppError::Cancelled))));
        assert!(!overlay.is_open());
    });
}

#[test]
fn an_annotated_crop_replaces_the_plain_crop() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let opts = selecting();
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());

        drag(&overlay, (5, 1), (6, 2));
        overlay.send(RegionInput::Command(RegionCommand::Lock));
        assert!(step(session.as_mut()).is_none());
        let views_before = overlay.views.lock().unwrap().len();
        overlay.send(RegionInput::Annotated(annotated_crop()));
        assert!(step(session.as_mut()).is_none());
        assert_eq!(overlay.views.lock().unwrap().len(), views_before, "the annotated crop needs no repaint");

        overlay.send(RegionInput::Command(RegionCommand::Finish(SelectionAction::Save)));
        let selection = step(session.as_mut()).expect("Finish confirms").unwrap();

        assert_eq!(selection.action, SelectionAction::Save);
        assert_eq!(selection.rects, vec![PhysRect { x: 5, y: 1, width: 2, height: 2 }], "the rect is still the selected one");
        assert_eq!(pixels(&selection.image), pixels(&annotated_crop()));
    });
}

#[test]
fn the_latest_annotated_crop_wins() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let opts = selecting();
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());

        let newer = Bitmap::new(2, 2, PixelFormat::Rgba8, [9, 8, 7, 255].repeat(4).into()).unwrap();
        drag(&overlay, (5, 1), (6, 2));
        overlay.send(RegionInput::Annotated(annotated_crop()));
        overlay.send(RegionInput::Annotated(newer.clone()));
        press(&overlay, RegionKey::Enter);
        let selection = step(session.as_mut()).expect("Enter confirms").unwrap();

        assert_eq!(pixels(&selection.image), pixels(&newer));
    });
}

#[test]
fn an_annotated_crop_is_dropped_when_the_selection_changes() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let opts = selecting();
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());

        drag(&overlay, (4, 1), (5, 2));
        overlay.send(RegionInput::Annotated(annotated_crop()));
        press(&overlay, RegionKey::Right);
        press(&overlay, RegionKey::Enter);
        let selection = step(session.as_mut()).expect("Enter confirms").unwrap();

        assert_eq!(selection.rects, vec![PhysRect { x: 5, y: 1, width: 2, height: 2 }], "the nudge moved the selection");
        assert_eq!(pixels(&selection.image), vec![RIGHT; 4], "the crop of the moved selection, not the stale annotated one");
    });
}

#[test]
fn an_annotated_crop_does_not_change_a_session_that_ends_without_a_selection() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let opts = selecting();
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());

        overlay.send(RegionInput::Annotated(annotated_crop()));
        overlay.send(RegionInput::Command(RegionCommand::Fullscreen));
        let selection = step(session.as_mut()).expect("Fullscreen confirms").unwrap();

        assert_eq!((selection.image.width(), selection.image.height()), (8, 4), "a fullscreen capture is not the annotated crop");
        assert_eq!(selection.action, SelectionAction::Default);
    });
}

#[test]
fn the_selection_image_is_sent_once_per_distinct_selected_rect() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let opts = selecting();
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());
        assert_eq!(views_with_an_image(&overlay), 0, "nothing is selected yet");

        // The press and the drag move are mid-gesture; the release selects.
        drag(&overlay, (4, 1), (5, 2));
        assert!(step(session.as_mut()).is_none());
        assert_eq!(views_with_an_image(&overlay), 1);

        // Moving the pointer, toggling the tools and locking change the view but not the selection.
        overlay.send(RegionInput::PointerMove { x: 4, y: 1 });
        press(&overlay, RegionKey::Space);
        overlay.send(RegionInput::Command(RegionCommand::Lock));
        overlay.send(RegionInput::Command(RegionCommand::Unlock));
        overlay.send(RegionInput::PointerMove { x: 5, y: 2 });
        assert!(step(session.as_mut()).is_none());
        assert_eq!(views_with_an_image(&overlay), 1, "the overlay keeps the image it was given");
        let quiet = overlay.last_view().unwrap();
        assert_eq!(quiet.selected, Some(PhysRect { x: 4, y: 1, width: 2, height: 2 }));
        assert!(quiet.selection_image.is_none());
        assert!(!quiet.tools_visible);

        // A nudge is a new selection, and so is the one after it.
        press(&overlay, RegionKey::Right);
        assert!(step(session.as_mut()).is_none());
        assert_eq!(views_with_an_image(&overlay), 2);
        let moved = overlay.last_view().unwrap();
        assert_eq!(moved.selected, Some(PhysRect { x: 5, y: 1, width: 2, height: 2 }));
        assert_eq!(pixels(&moved.selection_image.expect("a new rect gets a new image")), vec![RIGHT; 4]);

        // Drawing a new region: no image mid-gesture, one when it is released.
        overlay.send(RegionInput::PointerDown { x: 0, y: 0, button: Button::Left });
        overlay.send(RegionInput::PointerMove { x: 1, y: 1 });
        assert!(step(session.as_mut()).is_none());
        assert_eq!(views_with_an_image(&overlay), 2);
        assert_eq!(overlay.last_view().unwrap().selected, None, "the selection is not held while drawing");
        overlay.send(RegionInput::PointerUp { x: 1, y: 1, button: Button::Left });
        assert!(step(session.as_mut()).is_none());
        assert_eq!(views_with_an_image(&overlay), 3);
        assert_eq!(pixels(&overlay.last_view().unwrap().selection_image.unwrap()), vec![LEFT; 4]);

        press(&overlay, RegionKey::Escape);
        assert!(matches!(step(session.as_mut()), Some(Err(AppError::Cancelled))));
    });
}

#[test]
fn a_session_that_confirms_on_select_never_sends_a_selection_image() {
    in_pool(|| {
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::unsupported()));
        let opts = options();
        let mut session = pin!(select(&ports, two_frames(), &opts));
        assert!(step(session.as_mut()).is_none());

        overlay.send(RegionInput::PointerDown { x: 5, y: 1, button: Button::Left });
        overlay.send(RegionInput::PointerMove { x: 6, y: 2 });
        assert!(step(session.as_mut()).is_none());
        assert_eq!(overlay.last_view().unwrap().selected, None);
        overlay.send(RegionInput::PointerUp { x: 6, y: 2, button: Button::Left });
        assert!(step(session.as_mut()).is_some());

        assert_eq!(views_with_an_image(&overlay), 0);
    });
}

#[test]
fn view_carries_the_selection_the_tools_and_the_lock() {
    let bounds = PhysRect { x: 0, y: 0, width: 8, height: 4 };
    let mut state = RegionState::new(RegionConfig {
        bounds,
        multi_region: false,
        confirm_on_select: false,
        pick_point: false,
        snap_to_targets: true,
        min_size: 2,
        magnifier_pixels: 3,
        info_text: options().info_text,
        right_click: RegionMouseAction::RemoveShapeOrCancel,
        middle_click: RegionMouseAction::CancelCapture,
    });

    let idle = view(&state, bounds, &selecting(), [0; 3]);
    assert_eq!(idle.selected, None);
    assert!(idle.tools_visible, "the tools start shown");
    assert!(!idle.locked);

    state.handle(RegionInput::PointerDown { x: 1, y: 1, button: Button::Left });
    state.handle(RegionInput::PointerMove { x: 2, y: 2 });
    state.handle(RegionInput::PointerUp { x: 2, y: 2, button: Button::Left });
    state.handle(RegionInput::Key { key: RegionKey::Space, mods: KeyMods::default() });
    state.handle(RegionInput::Command(RegionCommand::Lock));

    let held = view(&state, bounds, &selecting(), [0; 3]);
    assert_eq!(held.selected, Some(PhysRect { x: 1, y: 1, width: 2, height: 2 }));
    assert!(!held.tools_visible);
    assert!(held.locked);
    assert!(held.selection_image.is_none(), "the session adds the crop, not the view builder");
}

/// A 400x300 monitor showing a window with a dark 100 px sidebar beside a light content area.
fn sidebar_frame() -> Frame {
    let (w, h) = (400u32, 300u32);
    let pixels: Vec<u8> = (0..w * h).flat_map(|i| if i % w < 100 { [40, 40, 40, 255] } else { [220, 220, 220, 255] }).collect();
    Frame {
        monitor: Monitor {
            id: MonitorId("m".to_owned()),
            name: "M".to_owned(),
            rect: PhysRect { x: 0, y: 0, width: w, height: h },
            scale: 1.0,
            primary: true,
        },
        image: Bitmap::new(w, h, PixelFormat::Bgra8, pixels.into()).unwrap(),
    }
}

fn click_in_sidebar(opts: RegionCaptureOptions) -> Selection {
    in_pool(|| {
        let window = window_target(PhysRect { x: 0, y: 0, width: 400, height: 300 }, "Chat");
        let overlay = Arc::new(FakeOverlay::default());
        let ports = test_ports(overlay.clone(), Arc::new(FakeHoverSource::new(vec![window])));
        let mut session = pin!(select(&ports, vec![sidebar_frame()], &opts));
        assert!(step(session.as_mut()).is_none());
        // A second poll runs the region analysis, which waits for the composed desktop.
        assert!(step(session.as_mut()).is_none());
        click(&overlay, (50, 150));
        step(session.as_mut()).expect("the click confirms").unwrap()
    })
}

#[test]
fn a_click_inside_a_window_takes_the_region_under_it() {
    let selection = click_in_sidebar(options());
    let target = selection.target.unwrap();
    assert_eq!(target.kind, TargetKind::Element);
    assert_eq!(target.process, "notes.exe", "a region carries its window's identity");
    let r = selection.rects[0];
    assert!(r.x <= 50 && r.right() > 50 && r.width <= 110 && r.height > 250, "the sidebar, not the window: {r:?}");
}

#[test]
fn region_detection_can_be_turned_off() {
    let selection = click_in_sidebar(RegionCaptureOptions { detect_regions: false, ..options() });
    assert_eq!(selection.rects, vec![PhysRect { x: 0, y: 0, width: 400, height: 300 }]);
}
