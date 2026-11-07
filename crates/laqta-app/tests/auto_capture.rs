#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Auto capture (M6 D5): the loop runs the captured action `count` times, waits at least a second between runs, stops
//! on the stop action (also during a wait), toggles, keeps one loop per start, and ends on a cancelled capture.

use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures_lite::future::block_on;
use laqta_app::{App, Dispatched, Ports};
use laqta_domain::{Action, Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat, Settings};
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeFileWatcher, FakeHoverSource, FakeNotifier, FakeOverlay, FakeScreen, FakeShell,
    FakeToolHost, FakeUploaders, InlineSpawner, ManualClock,
};
use laqta_ports::{AppError, BoxFuture, Clock, HostInfo, PortError};
use tempfile::TempDir;

/// A manual clock that can dispatch a stop during its `stop_on.0`th wait, the wait after a run.
struct HookClock {
    clock: ManualClock,
    waits: Mutex<u32>,
    stop_on: Mutex<Option<(u32, Weak<App>)>>,
}

impl HookClock {
    fn new() -> Self {
        HookClock {
            clock: ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0)),
            waits: Mutex::new(0),
            stop_on: Mutex::new(None),
        }
    }
}

impl Clock for HookClock {
    fn now(&self) -> jiff::civil::DateTime {
        self.clock.now()
    }

    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        let waits = {
            let mut count = self.waits.lock().unwrap();
            *count += 1;
            *count
        };
        let stop_app = self.stop_on.lock().unwrap().as_ref().filter(|(at, _)| *at == waits).and_then(|(_, app)| app.upgrade());
        let wait = self.clock.sleep(duration);
        Box::pin(async move {
            wait.await;
            if let Some(app) = stop_app {
                let _ = app.dispatch(Action::StopAutoCapture, None).await;
            }
        })
    }
}

struct H {
    app: Arc<App>,
    screen: Arc<FakeScreen>,
    clock: Arc<HookClock>,
    spawner: Arc<InlineSpawner>,
}

fn frame() -> Frame {
    Frame {
        monitor: Monitor {
            id: MonitorId("m0".into()),
            name: "M0".into(),
            rect: PhysRect { x: 0, y: 0, width: 8, height: 8 },
            scale: 1.0,
            primary: true,
        },
        image: Bitmap::new(8, 8, PixelFormat::Bgra8, vec![10; 256].into()).unwrap(),
    }
}

/// The app with `screen` as its screen, and the settings after `edit`.
fn harness(dir: &Path, screen: FakeScreen, edit: impl FnOnce(&mut Settings)) -> H {
    let screen = Arc::new(screen);
    let clock = Arc::new(HookClock::new());
    let spawner = Arc::new(InlineSpawner::default());
    let ports = Ports {
        screen: screen.clone(),
        clipboard: Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed)),
        shell: Arc::new(FakeShell::default()),
        clock: clock.clone(),
        spawner: spawner.clone(),
        overlay: Arc::new(FakeOverlay::default()),
        tools: Arc::new(FakeToolHost::default()),
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
        watcher: Arc::new(FakeFileWatcher::default()),
        uploaders: Arc::new(FakeUploaders::default()),
        hover: Arc::new(FakeHoverSource::new(Vec::new())),
        notifier: Arc::new(FakeNotifier::default()),
        rng: Arc::new(CountingRng::default()),
        host: HostInfo::default(),
        codecs: laqta_imaging::Codecs::builtin(),
    };
    let mut settings = Settings { screenshots_dir: dir.to_path_buf(), ..Settings::default_for(dir.to_path_buf()) };
    edit(&mut settings);
    H { app: App::new(ports, settings), screen, clock, spawner }
}

/// Runs the queued jobs, the auto capture loop and the captures it submits, until none is left. Imaging needs the
/// compute pool.
fn drain(h: &H) {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    pool.install(|| {
        loop {
            let jobs = h.spawner.take_spawned();
            if jobs.is_empty() {
                break;
            }
            for job in jobs {
                block_on(job);
            }
        }
    });
}

fn dispatch(h: &H, action: Action) -> Result<Dispatched, AppError> {
    block_on(h.app.dispatch(action, None))
}

/// The number of captures the screen has taken.
fn captures(h: &H) -> u32 {
    *h.screen.calls.lock().unwrap()
}

fn queued_loops(h: &H) -> usize {
    h.spawner.spawned.lock().unwrap().len()
}

#[test]
fn auto_capture_runs_count_times_and_frees_the_slot() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), FakeScreen::new(vec![frame()]), |s| {
        s.auto_capture.count = 3;
        s.auto_capture.interval_ms = 2000;
    });
    let before = h.clock.now();

    assert_eq!(dispatch(&h, Action::StartAutoCapture).unwrap(), Dispatched::Done);
    assert!(h.app.auto_capture_running());
    drain(&h);

    assert_eq!(captures(&h), 3);
    assert!(!h.app.auto_capture_running());
    // Two waits between three runs, and none after the last run.
    assert_eq!(h.clock.now(), before.checked_add(Duration::from_secs(4)).unwrap());
}

#[test]
fn auto_capture_waits_at_least_one_second_between_runs() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), FakeScreen::new(vec![frame()]), |s| {
        s.auto_capture.count = 2;
        s.auto_capture.interval_ms = 10;
    });
    let before = h.clock.now();

    dispatch(&h, Action::StartAutoCapture).unwrap();
    drain(&h);

    assert_eq!(captures(&h), 2);
    assert_eq!(h.clock.now(), before.checked_add(Duration::from_secs(1)).unwrap());
}

#[test]
fn auto_capture_stop_action_ends_the_loop_during_its_wait() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), FakeScreen::new(vec![frame()]), |s| s.auto_capture.interval_ms = 1000);
    // The stop arrives during the wait after the second run, so there is no third run.
    *h.clock.stop_on.lock().unwrap() = Some((2, Arc::downgrade(&h.app)));

    dispatch(&h, Action::StartAutoCapture).unwrap();
    drain(&h);

    assert_eq!(captures(&h), 2);
    assert!(!h.app.auto_capture_running());
}

#[test]
fn auto_capture_stopped_before_its_first_run_never_captures() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), FakeScreen::new(vec![frame()]), |s| s.auto_capture.count = 3);

    assert_eq!(dispatch(&h, Action::StartAutoCapture).unwrap(), Dispatched::Done);
    assert_eq!(queued_loops(&h), 1);
    assert_eq!(dispatch(&h, Action::StopAutoCapture).unwrap(), Dispatched::Done);
    assert!(!h.app.auto_capture_running());
    drain(&h);

    assert_eq!(captures(&h), 0);
}

#[test]
fn auto_capture_toggle_starts_then_stops_then_starts_again() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), FakeScreen::new(vec![frame()]), |s| s.auto_capture.count = 2);

    assert_eq!(dispatch(&h, Action::AutoCapture).unwrap(), Dispatched::Done);
    assert!(h.app.auto_capture_running());
    assert_eq!(dispatch(&h, Action::AutoCapture).unwrap(), Dispatched::Done);
    assert!(!h.app.auto_capture_running());
    drain(&h);
    assert_eq!(captures(&h), 0, "the stopped loop must not run");

    dispatch(&h, Action::AutoCapture).unwrap();
    assert!(h.app.auto_capture_running());
    drain(&h);
    assert_eq!(captures(&h), 2);
    assert!(!h.app.auto_capture_running());
}

#[test]
fn auto_capture_start_while_running_is_a_no_op() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), FakeScreen::new(vec![frame()]), |s| s.auto_capture.count = 1);

    assert_eq!(dispatch(&h, Action::StopAutoCapture).unwrap(), Dispatched::Done, "stop while idle");
    assert_eq!(dispatch(&h, Action::StartAutoCapture).unwrap(), Dispatched::Done);
    assert_eq!(dispatch(&h, Action::StartAutoCapture).unwrap(), Dispatched::Done);
    assert_eq!(queued_loops(&h), 1);
    drain(&h);
    assert_eq!(captures(&h), 1);
}

#[test]
fn auto_capture_failed_runs_are_skipped_and_the_loop_goes_on() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), FakeScreen::failing(PortError::Failed("no display".into())), |s| s.auto_capture.count = 3);

    assert_eq!(dispatch(&h, Action::StartAutoCapture).unwrap(), Dispatched::Done);
    drain(&h);

    assert_eq!(captures(&h), 3);
    assert!(!h.app.auto_capture_running());
}

#[test]
fn auto_capture_cancelled_run_ends_the_loop_even_without_a_count() {
    let tmp = TempDir::new().unwrap();
    // The default count is 0 (until stopped). A cancelled capture ends the loop anyway.
    let h = harness(tmp.path(), FakeScreen::failing(PortError::Cancelled), |_| {});

    dispatch(&h, Action::StartAutoCapture).unwrap();
    drain(&h);

    assert_eq!(captures(&h), 1);
    assert!(!h.app.auto_capture_running());
}

#[test]
fn auto_capture_refuses_to_capture_its_own_actions() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), FakeScreen::new(vec![frame()]), |s| s.auto_capture.action = Action::StartAutoCapture);

    let err = dispatch(&h, Action::StartAutoCapture).unwrap_err();
    assert!(matches!(err, AppError::Unsupported(_)), "got {err:?}");
    assert!(!h.app.auto_capture_running());
    assert_eq!(queued_loops(&h), 0);
}
