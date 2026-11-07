#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Dispatcher routes (M0 §5.1). Each M0 action returns its documented result.

use std::path::Path;
use std::sync::Arc;

use futures_lite::future::block_on;
use laqta_app::{App, Dispatched, Ports};
use laqta_domain::{Action, Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat, Settings};
use laqta_ports::fakes::{ClipboardBehaviour, FakeClipboard, FakeScreen, FakeShell, InlineSpawner, ManualClock};
use laqta_ports::{AppError, PortError, TaskEvent, TaskId, TaskStatus};
use tempfile::TempDir;

struct Harness {
    app: Arc<App>,
    screen: Arc<FakeScreen>,
    shell: Arc<FakeShell>,
    spawner: Arc<InlineSpawner>,
}

fn harness(screenshots_dir: &Path, screen: FakeScreen) -> Harness {
    let screen = Arc::new(screen);
    let shell = Arc::new(FakeShell::default());
    let spawner = Arc::new(InlineSpawner::default());
    let ports = Ports {
        screen: screen.clone(),
        clipboard: Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed)),
        shell: shell.clone(),
        clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0))),
        spawner: spawner.clone(),
        overlay: Arc::new(laqta_ports::fakes::FakeOverlay::default()),
        tools: Arc::new(laqta_ports::fakes::FakeToolHost::default()),
        prompt: Arc::new(laqta_ports::fakes::FakePrompt::default()),
        editor: Arc::new(laqta_ports::fakes::FakeEditorHost::default()),
        media: Arc::new(laqta_ports::fakes::FakeMedia::default()),
        stream: Arc::new(laqta_ports::fakes::FakeScreenStream::default()),
        uploaders: Arc::new(laqta_ports::fakes::FakeUploaders::default()),
        hover: Arc::new(laqta_ports::fakes::FakeHoverSource::new(Vec::new())),
        notifier: Arc::new(laqta_ports::fakes::FakeNotifier::default()),
        rng: Arc::new(laqta_ports::fakes::CountingRng::default()),
        host: laqta_ports::HostInfo::default(),
        codecs: laqta_imaging::Codecs::builtin(),
    };
    let settings = Settings { screenshots_dir: screenshots_dir.to_path_buf(), ..Settings::default_for(screenshots_dir.to_path_buf()) };
    Harness { app: App::new(ports, settings), screen, shell, spawner }
}

fn frame(x: i32) -> Frame {
    Frame {
        monitor: Monitor {
            id: MonitorId(format!("monitor-{x}")),
            name: format!("Monitor {x}"),
            rect: PhysRect { x, y: 0, width: 4, height: 4 },
            scale: 1.0,
            primary: x == 0,
        },
        image: Bitmap::new(4, 4, PixelFormat::Bgra8, vec![0; 64].into()).unwrap(),
    }
}

#[test]
fn print_screen_submits_a_capture_and_reports_in_queue_at_once() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), FakeScreen::new(vec![frame(0), frame(4)]));
    let mut events = h.app.events();

    assert_eq!(block_on(h.app.dispatch(Action::PrintScreen, None)).unwrap(), Dispatched::Task(TaskId(1)));
    assert_eq!(*h.screen.calls.lock().unwrap(), 1);
    assert_eq!(h.spawner.spawned.lock().unwrap().len(), 1);
    assert!(matches!(events.try_recv(), Ok(TaskEvent::StatusChanged { id: TaskId(1), status: TaskStatus::InQueue })));
}

#[test]
fn capture_failure_is_returned_with_no_events_and_no_task_id_used() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), FakeScreen::failing(PortError::Unsupported("x".into())));
    let mut events = h.app.events();

    let err = block_on(h.app.dispatch(Action::PrintScreen, None)).unwrap_err();
    assert!(matches!(err, AppError::Unsupported(ref s) if s == "x"), "got {err:?}");
    assert!(events.try_recv().is_err());
    assert!(h.spawner.spawned.lock().unwrap().is_empty());

    *h.screen.result.lock().unwrap() = Ok(vec![frame(0)]);
    assert_eq!(block_on(h.app.dispatch(Action::PrintScreen, None)).unwrap(), Dispatched::Task(TaskId(1)));
}

#[test]
fn open_screenshots_folder_creates_the_folder_and_opens_it() {
    let tmp = TempDir::new().unwrap();
    let shots = tmp.path().join("Laqta");
    let h = harness(&shots, FakeScreen::new(Vec::new()));
    let mut events = h.app.events();

    assert_eq!(block_on(h.app.dispatch(Action::OpenScreenshotsFolder, None)).unwrap(), Dispatched::Done);
    assert!(shots.is_dir());
    assert_eq!(*h.shell.opened.lock().unwrap(), vec![shots]);
    assert_eq!(*h.spawner.blocking_calls.lock().unwrap(), 1);
    assert!(events.try_recv().is_err());
}

#[test]
fn open_screenshots_folder_reports_io_error_and_opens_nothing() {
    let tmp = TempDir::new().unwrap();
    // A regular file where the folder should be, so it cannot be created.
    let shots = tmp.path().join("Laqta");
    std::fs::write(&shots, b"not a folder").unwrap();
    let h = harness(&shots, FakeScreen::new(Vec::new()));

    let err = block_on(h.app.dispatch(Action::OpenScreenshotsFolder, None)).unwrap_err();
    assert!(matches!(err, AppError::Io(_)), "got {err:?}");
    assert!(h.shell.opened.lock().unwrap().is_empty());
}

#[test]
fn exit_returns_exit_without_side_effects() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), FakeScreen::new(Vec::new()));
    let mut events = h.app.events();

    assert_eq!(block_on(h.app.dispatch(Action::ExitLaqta, None)).unwrap(), Dispatched::Exit);
    assert_eq!(*h.screen.calls.lock().unwrap(), 0);
    assert!(h.spawner.spawned.lock().unwrap().is_empty());
    assert!(h.shell.opened.lock().unwrap().is_empty());
    assert!(events.try_recv().is_err());
}

#[test]
fn settings_returns_the_settings_given_to_new() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path(), FakeScreen::new(Vec::new()));
    assert_eq!(h.app.settings().screenshots_dir, tmp.path());
}
