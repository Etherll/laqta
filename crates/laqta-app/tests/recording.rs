#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Recording session: the start actions and their areas, the countdown, pause and resume, stop and abort, the
//! fixed duration, an encoder that dies, and the hand-off of the finished file to Task. The driver is a spawned
//! future that runs until it is told to stop (or its duration runs out), so each test steps the spawned futures
//! one poll at a time.

use std::path::Path;
use std::sync::Arc;

use async_broadcast::Receiver;
use futures_lite::future::{block_on, poll_once};
use laqta_app::{App, Dispatched, Ports};
use laqta_domain::region::{Button, HoverTarget, RegionInput, TargetKind};
use laqta_domain::workflow::AfterCaptureTasks;
use laqta_domain::{Action, Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat, Settings};
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeHoverSource, FakeMedia, FakeNotifier, FakeOverlay, FakeScreen, FakeScreenStream,
    FakeShell, FakeToolHost, InlineSpawner, ManualClock,
};
use laqta_ports::{AppError, BoxFuture, HostInfo, PortError, TaskEvent, TaskId, TaskStatus};
use tempfile::TempDir;

struct H {
    app: Arc<App>,
    overlay: Arc<FakeOverlay>,
    spawner: Arc<InlineSpawner>,
    media: Arc<FakeMedia>,
    stream: Arc<FakeScreenStream>,
    hover: Arc<FakeHoverSource>,
    clipboard: Arc<FakeClipboard>,
    shell: Arc<FakeShell>,
    notifier: Arc<FakeNotifier>,
    events: Receiver<TaskEvent>,
    /// Every event seen so far, in order.
    log: Vec<TaskEvent>,
    /// Spawned futures that have not finished: the drivers of running recordings, and the Task jobs they submit.
    running: Vec<BoxFuture<'static, ()>>,
}

/// Runs a test body inside a 2-thread compute pool. Region selection composes frames there (ADR 024).
fn in_pool(body: impl FnOnce() + Send) {
    rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap().install(body);
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

/// The window the active-window actions pick: 4 wide and 3 high, so the even area is 4x2.
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
    let spawner = Arc::new(InlineSpawner::default());
    let hover = Arc::new(FakeHoverSource::new(vec![window()]));
    *hover.active.lock().unwrap() = Some(window());
    let stream = Arc::new(FakeScreenStream::default());
    let media = Arc::new(FakeMedia::default());
    let clipboard = Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed));
    let shell = Arc::new(FakeShell::default());
    let notifier = Arc::new(FakeNotifier::default());
    let ports = Ports {
        screen: Arc::new(FakeScreen::new(vec![frame(0, 10), frame(8, 200)])),
        clipboard: clipboard.clone(),
        shell: shell.clone(),
        clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0))),
        spawner: spawner.clone(),
        overlay: overlay.clone(),
        tools: Arc::new(FakeToolHost::default()),
        prompt: Arc::new(laqta_ports::fakes::FakePrompt::default()),
        editor: Arc::new(laqta_ports::fakes::FakeEditorHost::default()),
        media: media.clone(),
        stream: stream.clone(),
        ocr: Arc::new(laqta_ports::fakes::FakeOcr::default()),
        window_ops: Arc::new(laqta_ports::fakes::FakeWindowOps::default()),
        input: Arc::new(laqta_ports::fakes::FakeInputSynth::default()),
        highlighter: Arc::new(laqta_ports::fakes::FakeHighlighter::default()),
        background: Arc::new(laqta_ports::fakes::FakeBackgroundRemover::default()),
        analyzer: Arc::new(laqta_ports::fakes::FakeAnalyzer::default()),
        inpainter: Arc::new(laqta_ports::fakes::FakeInpainter),
        watcher: Arc::new(laqta_ports::fakes::FakeFileWatcher::default()),
        uploaders: Arc::new(laqta_ports::fakes::FakeUploaders::default()),
        hover: hover.clone(),
        notifier: notifier.clone(),
        rng: Arc::new(CountingRng::default()),
        host: HostInfo::default(),
        codecs: laqta_imaging::Codecs::builtin(),
    };
    let mut settings = Settings::default_for(dir.to_path_buf());
    settings.screenshots_dir = dir.to_path_buf();
    settings.defaults.capture.region.min_size = 2;
    // Drawing the area ends the selection at once; there is no capture toolbar to confirm it.
    settings.defaults.capture.region.show_tools_after_select = false;
    edit(&mut settings);
    let app = App::new(ports, settings);
    let events = app.events();
    H { app, overlay, spawner, media, stream, hover, clipboard, shell, notifier, events, log: Vec::new(), running: Vec::new() }
}

/// Polls every spawned future once, keeping the ones that are still running. A driver with nothing due does not
/// move, and a driver with a command or a second due takes one step.
fn step(h: &mut H) {
    h.running.extend(h.spawner.take_spawned());
    h.running.retain_mut(|job| block_on(poll_once(job)).is_none());
}

/// Steps until nothing runs: the driver ended (after a stop, an abort, its fixed duration or a crash) and the job it
/// submitted has finished.
fn settle(h: &mut H) {
    for _ in 0..50 {
        step(h);
        if h.running.is_empty() && h.spawner.spawned.lock().unwrap().is_empty() {
            return;
        }
    }
    panic!("still running after 50 steps; media calls: {:?}", calls(h));
}

/// Every event so far, in order.
fn events(h: &mut H) -> Vec<TaskEvent> {
    while let Ok(event) = h.events.try_recv() {
        h.log.push(event);
    }
    h.log.clone()
}

fn calls(h: &H) -> Vec<String> {
    h.media.calls.lock().unwrap().clone()
}

/// A start action that needs no drawn area.
fn start(h: &H, action: Action) -> Result<Dispatched, AppError> {
    block_on(h.app.dispatch(action, None))
}

/// A start action that draws an area: the overlay opens, the user's `inputs` play, and the action completes.
fn start_drawn(h: &H, action: Action, inputs: Vec<RegionInput>) -> Result<Dispatched, AppError> {
    let mut fut = h.app.dispatch(action, None);
    block_on(async {
        while !h.overlay.is_open() {
            assert!(poll_once(&mut fut).await.is_none(), "the action finished before the overlay opened");
        }
        for input in inputs {
            h.overlay.send(input);
        }
        fut.await
    })
}

/// A drag from (1, 1) to (6, 5): a 6x5 area, which becomes 6x4.
fn drag() -> Vec<RegionInput> {
    vec![
        RegionInput::PointerMove { x: 1, y: 1 },
        RegionInput::PointerDown { x: 1, y: 1, button: Button::Left },
        RegionInput::PointerMove { x: 6, y: 5 },
        RegionInput::PointerUp { x: 6, y: 5, button: Button::Left },
    ]
}

/// The id of the recording, from its `RecordingStarted` event.
fn recording_id(h: &mut H) -> TaskId {
    events(h)
        .iter()
        .find_map(|event| match event {
            TaskEvent::RecordingStarted { id, .. } => Some(*id),
            _ => None,
        })
        .expect("the recording started")
}

fn custom_region(s: &mut Settings) {
    s.defaults.capture.custom_region = Some(PhysRect { x: 6, y: 2, width: 5, height: 4 });
}

#[test]
fn custom_region_starts_with_an_even_size() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let h = harness(tmp.path(), custom_region);
        assert_eq!(start(&h, Action::ScreenRecorderCustomRegion).unwrap(), Dispatched::Done);
        assert_eq!(calls(&h), vec!["start X264 4x4".to_owned()]);
        assert!(h.app.is_recording());
        assert!(tmp.path().join("2026-11").is_dir(), "the output folder is made before the encoder starts");
    });
}

#[test]
fn drawn_region_starts_with_an_even_size() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let h = harness(tmp.path(), |_| {});
        assert_eq!(start_drawn(&h, Action::ScreenRecorder, drag()).unwrap(), Dispatched::Done);
        assert_eq!(calls(&h), vec!["start X264 6x4".to_owned()]);
    });
}

#[test]
fn active_window_records_the_window_rect() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let h = harness(tmp.path(), |_| {});
        assert_eq!(start(&h, Action::ScreenRecorderActiveWindow).unwrap(), Dispatched::Done);
        assert_eq!(calls(&h), vec!["start X264 4x2".to_owned()]);
    });
}

#[test]
fn missing_custom_region_is_unsupported() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let h = harness(tmp.path(), |_| {});
        assert!(matches!(start(&h, Action::ScreenRecorderCustomRegion), Err(AppError::Unsupported(_))));
        assert!(!h.app.is_recording());
        assert!(calls(&h).is_empty());
    });
}

#[test]
fn countdown_emits_one_event_per_second() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), |s| {
            custom_region(s);
            s.defaults.recording.countdown_secs = 3;
        });
        start(&h, Action::ScreenRecorderCustomRegion).unwrap();
        let seen = events(&mut h);
        let countdown: Vec<u32> = seen
            .iter()
            .filter_map(|event| match event {
                TaskEvent::RecordingCountdown { seconds_left, .. } => Some(*seconds_left),
                _ => None,
            })
            .collect();
        assert_eq!(countdown, vec![3, 2, 1]);
        let started = seen.iter().position(|e| matches!(e, TaskEvent::RecordingStarted { .. })).unwrap();
        let last_count = seen.iter().rposition(|e| matches!(e, TaskEvent::RecordingCountdown { .. })).unwrap();
        assert!(last_count < started, "the countdown ends before the recording starts");
        assert_eq!(calls(&h), vec!["start X264 4x4".to_owned()]);
    });
}

#[test]
fn gif_routes_use_the_gif_codec() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let h = harness(tmp.path(), custom_region);
        assert_eq!(start(&h, Action::ScreenRecorderGifCustomRegion).unwrap(), Dispatched::Done);
        assert_eq!(calls(&h), vec!["start Gif 4x4".to_owned()]);
    });
}

#[test]
fn stop_hands_the_file_to_task() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), custom_region);
        start(&h, Action::ScreenRecorderCustomRegion).unwrap();
        let recording = recording_id(&mut h);

        h.app.recording_stop();
        settle(&mut h);

        let seen = events(&mut h);
        let stopped = seen.iter().find_map(|e| match e {
            TaskEvent::RecordingStopped { path, .. } => Some(path.clone()),
            _ => None,
        });
        let path = stopped.expect("RecordingStopped");
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("mp4"));
        let file_task = seen
            .iter()
            .find_map(|e| match e {
                TaskEvent::Saved { id, .. } => Some(*id),
                _ => None,
            })
            .expect("the file is reported as saved");
        assert_ne!(file_task, recording, "the file goes through a Task job of its own");
        assert!(seen.iter().any(|e| matches!(e, TaskEvent::Notified { id } if *id == file_task)));
        assert!(seen.iter().any(|e| matches!(e, TaskEvent::StatusChanged { id, status: TaskStatus::Completed } if *id == file_task)));
        assert_eq!(h.notifier.toasts.lock().unwrap().first().unwrap().path.as_deref(), Some(path.as_path()));
        assert_eq!(calls(&h), vec!["start X264 4x4".to_owned(), "stop".to_owned()]);
        assert!(!h.app.is_recording());
    });
}

#[test]
fn saved_file_runs_copy_and_reveal_steps() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), |s| {
            custom_region(s);
            s.defaults.after_capture = AfterCaptureTasks::COPY_FILE_TO_CLIPBOARD
                | AfterCaptureTasks::COPY_FILE_PATH_TO_CLIPBOARD
                | AfterCaptureTasks::SHOW_IN_EXPLORER;
        });
        start(&h, Action::ScreenRecorderCustomRegion).unwrap();
        h.app.recording_stop();
        settle(&mut h);

        let seen = events(&mut h);
        let file_task = seen
            .iter()
            .find_map(|e| match e {
                TaskEvent::Saved { id, .. } => Some(*id),
                _ => None,
            })
            .expect("the file is reported as saved");
        assert!(seen.iter().any(|e| matches!(e, TaskEvent::FileCopied { id } if *id == file_task)));
        assert!(seen.iter().any(|e| matches!(e, TaskEvent::PathCopied { id, .. } if *id == file_task)));
        assert!(seen.iter().any(|e| matches!(e, TaskEvent::Revealed { id, .. } if *id == file_task)));
        assert!(seen.iter().any(|e| matches!(e, TaskEvent::StatusChanged { id, status: TaskStatus::Completed } if *id == file_task)));
        assert_eq!(h.clipboard.files.lock().unwrap().len(), 1);
        assert_eq!(h.shell.revealed.lock().unwrap().len(), 1);
    });
}

#[test]
fn pause_and_resume_pass_through_to_the_encoder() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), custom_region);
        start(&h, Action::ScreenRecorderCustomRegion).unwrap();

        h.app.recording_pause();
        step(&mut h);
        h.app.recording_resume();
        step(&mut h);
        h.app.recording_stop();
        settle(&mut h);

        let seen = events(&mut h);
        let paused = seen.iter().position(|e| matches!(e, TaskEvent::RecordingPaused { .. })).expect("paused");
        let resumed = seen.iter().position(|e| matches!(e, TaskEvent::RecordingResumed { .. })).expect("resumed");
        assert!(paused < resumed);
        assert_eq!(calls(&h), vec!["start X264 4x4", "pause", "resume", "stop"]);
    });
}

#[test]
fn pause_action_toggles_pause_and_resume() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), custom_region);
        start(&h, Action::ScreenRecorderCustomRegion).unwrap();

        assert_eq!(start(&h, Action::PauseScreenRecording).unwrap(), Dispatched::Done);
        step(&mut h);
        assert_eq!(start(&h, Action::PauseScreenRecording).unwrap(), Dispatched::Done);
        step(&mut h);
        h.app.recording_stop();
        settle(&mut h);

        let seen = events(&mut h);
        assert!(seen.iter().any(|e| matches!(e, TaskEvent::RecordingPaused { .. })));
        assert!(seen.iter().any(|e| matches!(e, TaskEvent::RecordingResumed { .. })));
        assert_eq!(calls(&h), vec!["start X264 4x4", "pause", "resume", "stop"]);
    });
}

#[test]
fn abort_reports_cancelled_and_hands_nothing_to_task() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), custom_region);
        start(&h, Action::ScreenRecorderCustomRegion).unwrap();

        h.app.recording_abort();
        settle(&mut h);

        let seen = events(&mut h);
        assert!(seen.iter().any(|e| matches!(e, TaskEvent::RecordingFailed { error: AppError::Cancelled, partial: None, .. })));
        assert!(!seen.iter().any(|e| matches!(e, TaskEvent::Saved { .. } | TaskEvent::RecordingStopped { .. })));
        assert!(h.spawner.spawned.lock().unwrap().is_empty());
        assert_eq!(calls(&h), vec!["start X264 4x4".to_owned(), "abort".to_owned()]);
        assert!(!h.app.is_recording());
        assert!(h.notifier.toasts.lock().unwrap().is_empty(), "an abort is the user's choice, so it shows no toast");
    });
}

#[test]
fn second_start_stops_the_running_recording() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), custom_region);
        start(&h, Action::ScreenRecorderCustomRegion).unwrap();

        assert_eq!(start(&h, Action::ScreenRecorderCustomRegion).unwrap(), Dispatched::Done);
        settle(&mut h);

        assert_eq!(calls(&h), vec!["start X264 4x4".to_owned(), "stop".to_owned()]);
        assert!(!h.app.is_recording());
    });
}

#[test]
fn fixed_duration_stops_by_itself() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), |s| {
            custom_region(s);
            s.defaults.recording.fixed_duration_secs = 2;
        });
        start(&h, Action::ScreenRecorderCustomRegion).unwrap();
        settle(&mut h);

        let seen = events(&mut h);
        let ticks: Vec<u32> = seen
            .iter()
            .filter_map(|e| match e {
                TaskEvent::RecordingTick { elapsed_secs, .. } => Some(*elapsed_secs),
                _ => None,
            })
            .collect();
        assert_eq!(ticks, vec![1, 2]);
        assert!(seen.iter().any(|e| matches!(e, TaskEvent::RecordingStopped { .. })));
        assert_eq!(calls(&h), vec!["start X264 4x4".to_owned(), "stop".to_owned()]);
        assert!(!h.app.is_recording());
    });
}

#[test]
fn crashed_encoder_reports_failure_with_the_partial_file() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), custom_region);
        *h.media.crash.lock().unwrap() = true;
        start(&h, Action::ScreenRecorderCustomRegion).unwrap();
        settle(&mut h);

        let seen = events(&mut h);
        let partial = seen.iter().find_map(|e| match e {
            TaskEvent::RecordingFailed { error: AppError::Io(_), partial: Some(path), .. } => Some(path.clone()),
            _ => None,
        });
        let partial = partial.expect("RecordingFailed with the partial file");
        assert_eq!(partial.extension().and_then(|e| e.to_str()), Some("mp4"));
        assert!(!seen.iter().any(|e| matches!(e, TaskEvent::Saved { .. } | TaskEvent::RecordingStopped { .. })));
        assert!(h.spawner.spawned.lock().unwrap().is_empty());
        assert!(!h.app.is_recording());
        let toasts = h.notifier.toasts.lock().unwrap();
        assert_eq!(toasts.len(), 1, "a failed recording shows one toast");
        assert_eq!(toasts[0].title, "Recording failed");
        assert!(toasts[0].body.contains("recorder stopped"), "the toast says why: {}", toasts[0].body);
    });
}

#[test]
fn last_recording_area_is_reused_without_asking() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), custom_region);
        start(&h, Action::ScreenRecorderCustomRegion).unwrap();
        h.app.recording_stop();
        settle(&mut h);

        assert_eq!(start(&h, Action::StartScreenRecorder).unwrap(), Dispatched::Done);
        assert!(!h.overlay.is_open(), "the last area is used, so no overlay opens");
        assert_eq!(calls(&h), vec!["start X264 4x4".to_owned(), "stop".to_owned(), "start X264 4x4".to_owned()]);
    });
}

#[test]
fn control_actions_with_nothing_running_are_done() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let h = harness(tmp.path(), |_| {});
        for action in [Action::StopScreenRecording, Action::PauseScreenRecording, Action::AbortScreenRecording] {
            assert_eq!(start(&h, action).unwrap(), Dispatched::Done);
        }
        assert!(calls(&h).is_empty());
    });
}

#[test]
fn highlighter_is_on_only_while_recording() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), |s| {
            custom_region(s);
            s.defaults.recording.highlight_mouse = true;
        });
        start(&h, Action::ScreenRecorderCustomRegion).unwrap();
        recording_id(&mut h);
        step(&mut h);
        assert!(h.app.ports().highlighter.is_enabled());
        h.app.recording_stop();
        settle(&mut h);
        assert!(!h.app.ports().highlighter.is_enabled());
    });
}

/// The `RecordingFailed` events in `seen`, as (error, partial).
fn failures(seen: &[TaskEvent]) -> Vec<(AppError, Option<std::path::PathBuf>)> {
    seen.iter()
        .filter_map(|e| match e {
            TaskEvent::RecordingFailed { error, partial, .. } => Some((error.clone(), partial.clone())),
            _ => None,
        })
        .collect()
}

/// Polls `fut` once and asserts it is waiting.
fn assert_pending<T>(fut: &mut (impl std::future::Future<Output = T> + Unpin)) {
    assert!(block_on(poll_once(fut)).is_none(), "the action should still be waiting");
}

#[test]
fn an_unsupported_active_window_reports_a_failure_and_a_toast() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), |s| s.defaults.recording.countdown_secs = 2);
        *h.hover.active_error.lock().unwrap() = Some(PortError::Unsupported("active window needs sway".into()));

        assert!(matches!(start(&h, Action::ScreenRecorderActiveWindow), Err(AppError::Unsupported(_))));

        let seen = events(&mut h);
        let failed = failures(&seen);
        assert_eq!(failed.len(), 1, "exactly one terminal event: {seen:?}");
        assert!(matches!(&failed[0], (AppError::Unsupported(message), None) if message.contains("needs sway")));
        assert!(!seen.iter().any(|e| matches!(e, TaskEvent::RecordingCountdown { .. } | TaskEvent::RecordingStarted { .. })));
        assert!(!h.app.is_recording());
        assert!(calls(&h).is_empty());
        let toasts = h.notifier.toasts.lock().unwrap();
        assert_eq!(toasts.len(), 1);
        assert_eq!(toasts[0].title, "Recording failed");
        assert!(toasts[0].body.contains("needs sway"), "the toast says why: {}", toasts[0].body);
    });
}

#[test]
fn a_cancelled_portal_after_the_countdown_ends_with_cancelled_and_no_toast() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), |s| {
            custom_region(s);
            s.defaults.recording.countdown_secs = 2;
        });
        *h.stream.fail.lock().unwrap() = Some(PortError::Cancelled);

        assert!(matches!(start(&h, Action::ScreenRecorderCustomRegion), Err(AppError::Cancelled)));

        let seen = events(&mut h);
        let kinds: Vec<&str> = seen
            .iter()
            .map(|e| match e {
                TaskEvent::RecordingCountdown { seconds_left: 2, .. } => "count 2",
                TaskEvent::RecordingCountdown { seconds_left: 1, .. } => "count 1",
                TaskEvent::RecordingFailed { error: AppError::Cancelled, partial: None, .. } => "cancelled",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["count 2", "count 1", "cancelled"]);
        assert!(!h.app.is_recording());
        assert!(h.notifier.toasts.lock().unwrap().is_empty(), "the user closed the portal, so there is nothing to report");
    });
}

#[test]
fn a_failed_stream_open_reports_a_failure_and_a_toast() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), custom_region);
        *h.stream.fail.lock().unwrap() = Some(PortError::Failed("the portal is gone".into()));

        assert!(matches!(start(&h, Action::ScreenRecorderCustomRegion), Err(AppError::Io(_))));

        let failed = failures(&events(&mut h));
        assert!(matches!(failed.as_slice(), [(AppError::Io(message), None)] if message.contains("portal is gone")));
        assert!(!h.app.is_recording());
        assert_eq!(h.notifier.toasts.lock().unwrap().len(), 1);
        assert!(calls(&h).is_empty(), "the encoder never started");
    });
}

#[test]
fn stop_abort_and_a_second_start_each_cancel_a_stream_that_never_opens() {
    type Cancel = fn(&H);
    let cancels: [(&str, Cancel); 3] = [
        ("stop", |h| h.app.recording_stop()),
        ("abort", |h| h.app.recording_abort()),
        ("second start", |h| assert_eq!(start(h, Action::ScreenRecorderCustomRegion).unwrap(), Dispatched::Done)),
    ];
    for (name, cancel) in cancels {
        in_pool(|| {
            let tmp = TempDir::new().unwrap();
            let mut h = harness(tmp.path(), custom_region);
            *h.stream.hang.lock().unwrap() = true;

            let mut first = h.app.dispatch(Action::ScreenRecorderCustomRegion, None);
            assert_pending(&mut first);
            assert!(h.app.is_recording(), "{name}: the slot is held while the portal is open");

            // Pause and resume mean nothing yet, and must not end the wait.
            h.app.recording_pause();
            h.app.recording_resume();
            assert_pending(&mut first);
            assert!(h.app.is_recording());

            cancel(&h);
            assert_eq!(block_on(first).unwrap(), Dispatched::Done, "{name}");

            let seen = events(&mut h);
            let failed = failures(&seen);
            assert!(matches!(failed.as_slice(), [(AppError::Cancelled, None)]), "{name}: {seen:?}");
            assert!(!seen.iter().any(|e| matches!(e, TaskEvent::RecordingStarted { .. } | TaskEvent::Saved { .. })), "{name}");
            assert!(!h.app.is_recording(), "{name}: the slot is free");
            assert!(calls(&h).is_empty(), "{name}: the encoder never started");
            assert!(h.notifier.toasts.lock().unwrap().is_empty(), "{name}: a cancel shows no toast");

            // The next start works.
            *h.stream.hang.lock().unwrap() = false;
            assert_eq!(start(&h, Action::ScreenRecorderCustomRegion).unwrap(), Dispatched::Done, "{name}");
            assert_eq!(calls(&h), vec!["start X264 4x4".to_owned()], "{name}");
            assert!(h.app.is_recording());
        });
    }
}

#[test]
fn stop_while_the_area_is_drawn_closes_the_overlay_and_records_nothing() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), |_| {});
        let mut fut = h.app.dispatch(Action::ScreenRecorder, None);
        block_on(async {
            while !h.overlay.is_open() {
                assert!(poll_once(&mut fut).await.is_none(), "the action finished before the overlay opened");
            }
        });

        h.app.recording_stop();
        assert_eq!(block_on(fut).unwrap(), Dispatched::Done);

        assert!(!h.overlay.is_open(), "the overlay closes when the start is cancelled");
        let failed = failures(&events(&mut h));
        assert!(matches!(failed.as_slice(), [(AppError::Cancelled, None)]));
        assert!(!h.app.is_recording());
        assert!(calls(&h).is_empty());
    });
}

#[test]
fn stop_while_the_encoder_starts_saves_nothing() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), custom_region);
        *h.media.slow_start.lock().unwrap() = true;

        let mut fut = h.app.dispatch(Action::ScreenRecorderCustomRegion, None);
        assert_pending(&mut fut);
        assert_eq!(calls(&h), vec!["start X264 4x4".to_owned()], "the encoder is starting");

        h.app.recording_stop();
        assert_eq!(block_on(fut).unwrap(), Dispatched::Done);
        settle(&mut h);

        let seen = events(&mut h);
        let failed = failures(&seen);
        assert!(matches!(failed.as_slice(), [(AppError::Cancelled, None)]), "{seen:?}");
        let kept =
            |e: &&TaskEvent| matches!(e, TaskEvent::RecordingStarted { .. } | TaskEvent::RecordingStopped { .. } | TaskEvent::Saved { .. });
        assert!(!seen.iter().any(|e| kept(&e)), "nothing started or saved");
        assert_eq!(calls(&h), vec!["start X264 4x4".to_owned(), "abort".to_owned()], "the started encoder is aborted, not stopped");
        assert!(!h.app.is_recording());
        assert!(h.notifier.toasts.lock().unwrap().is_empty());
    });
}
