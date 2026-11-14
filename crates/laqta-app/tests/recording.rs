#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Recording session: the start actions and their areas, the countdown, pause and resume, stop and abort, the
//! fixed duration, an encoder that dies, and the hand-off of the finished file to Task. The driver is a spawned
//! future that runs until it is told to stop (or its duration runs out), so each test steps the spawned futures
//! one poll at a time.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use async_broadcast::Receiver;
use futures_lite::future::{block_on, poll_once};
use laqta_app::{App, Dispatched, Ports};
use laqta_domain::record::{AudioDevice, RecordSpec, RecordingSettings};
use laqta_domain::region::{Button, HoverTarget, RegionInput, TargetKind};
use laqta_domain::workflow::{AfterCaptureTasks, TaskSettingsPatch, Workflow, WorkflowId};
use laqta_domain::{Action, Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat, Settings};
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeHoverSource, FakeMedia, FakeNotifier, FakeOverlay, FakeScreen, FakeScreenStream,
    FakeShell, FakeToolHost, InlineSpawner, ManualClock,
};
use laqta_ports::{
    AppError, BoxFuture, HostInfo, Media, MediaInfo, PortError, RecordingHandle, RecordingInput, ScreenStream, TaskEvent, TaskId,
    TaskStatus, TranscodeSpec,
};
use tempfile::TempDir;

struct H {
    app: Arc<App>,
    highlighter: Arc<laqta_ports::fakes::FakeHighlighter>,
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

/// Ports a test swaps in for the fakes the harness builds. A swapped port is used by the app; the fake of the same
/// name in `H` then sees nothing.
#[derive(Default)]
struct Swap {
    stream: Option<Arc<dyn ScreenStream>>,
    media: Option<Arc<dyn Media>>,
}

fn harness(dir: &Path, edit: impl FnOnce(&mut Settings)) -> H {
    harness_with(dir, edit, Swap::default())
}

fn harness_with(dir: &Path, edit: impl FnOnce(&mut Settings), swap: Swap) -> H {
    let overlay = Arc::new(FakeOverlay::default());
    let highlighter = Arc::new(laqta_ports::fakes::FakeHighlighter::default());
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
        media: swap.media.unwrap_or_else(|| media.clone()),
        stream: swap.stream.unwrap_or_else(|| stream.clone()),
        ocr: Arc::new(laqta_ports::fakes::FakeOcr::default()),
        window_ops: Arc::new(laqta_ports::fakes::FakeWindowOps::default()),
        input: Arc::new(laqta_ports::fakes::FakeInputSynth::default()),
        highlighter: highlighter.clone(),
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
    H { app, highlighter, overlay, spawner, media, stream, hover, clipboard, shell, notifier, events, log: Vec::new(), running: Vec::new() }
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
fn an_unnamed_camera_is_the_first_one_the_stream_lists() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let camera_on = |s: &mut Settings| {
            custom_region(s);
            s.defaults.recording.camera.enabled = true;
        };
        // A platform that lists cameras: the first one is named for the recorder.
        let h = harness(tmp.path(), camera_on);
        *h.stream.cameras.lock().unwrap() = Some(vec!["Integrated Camera".into(), "USB Camera".into()]);
        assert_eq!(start(&h, Action::ScreenRecorderCustomRegion).unwrap(), Dispatched::Done);
        assert_eq!(calls(&h), vec!["start X264 4x4 camera=Integrated Camera".to_owned()]);
        // A platform whose recorder picks the camera itself: the name stays empty.
        let h = harness(tmp.path(), camera_on);
        assert_eq!(start(&h, Action::ScreenRecorderCustomRegion).unwrap(), Dispatched::Done);
        assert_eq!(calls(&h), vec!["start X264 4x4 camera=".to_owned()]);
        // A camera the user named is kept.
        let h = harness(tmp.path(), |s| {
            camera_on(s);
            s.defaults.recording.camera.device = "USB Camera".into();
        });
        *h.stream.cameras.lock().unwrap() = Some(vec!["Integrated Camera".into()]);
        assert_eq!(start(&h, Action::ScreenRecorderCustomRegion).unwrap(), Dispatched::Done);
        assert_eq!(calls(&h), vec!["start X264 4x4 camera=USB Camera".to_owned()]);
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
        assert!(
            !seen.iter().any(|e| matches!(e, TaskEvent::PathCopied { id, .. } if *id == file_task)),
            "the file copy wins over the path copy, as for an image"
        );
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
        assert!(laqta_ports::PointerHighlighter::is_enabled(&*h.highlighter));
        h.app.recording_stop();
        settle(&mut h);
        assert!(!laqta_ports::PointerHighlighter::is_enabled(&*h.highlighter));
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
                TaskEvent::RecordingStarted { .. } => "started",
                TaskEvent::RecordingFailed { error: AppError::Cancelled, partial: None, .. } => "cancelled",
                _ => "other",
            })
            .collect();
        // `RecordingStarted` closes the countdown before the stream opens, so the failed open comes after it.
        assert_eq!(kinds, vec!["count 2", "count 1", "started", "cancelled"]);
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
            assert!(!seen.iter().any(|e| matches!(e, TaskEvent::RecordingStopped { .. } | TaskEvent::Saved { .. })), "{name}");
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
        let kept = |e: &&TaskEvent| matches!(e, TaskEvent::RecordingStopped { .. } | TaskEvent::Saved { .. });
        assert!(!seen.iter().any(|e| kept(&e)), "nothing was stopped or saved");
        assert_eq!(calls(&h), vec!["start X264 4x4".to_owned(), "abort".to_owned()], "the started encoder is aborted, not stopped");
        assert!(!h.app.is_recording());
        assert!(h.notifier.toasts.lock().unwrap().is_empty());
    });
}

/// A media port whose recording takes as long to finish as the test says: `stop` waits for `gate`. It keeps the
/// specs it was started with.
struct GatedMedia {
    gate: Arc<AtomicBool>,
    specs: Mutex<Vec<RecordSpec>>,
}

impl GatedMedia {
    fn new(open: bool) -> Arc<Self> {
        Arc::new(GatedMedia { gate: Arc::new(AtomicBool::new(open)), specs: Mutex::new(Vec::new()) })
    }
}

struct GatedHandle {
    gate: Arc<AtomicBool>,
    output: PathBuf,
}

/// Pending until the gate opens.
struct WaitFor(Arc<AtomicBool>);

impl Future for WaitFor {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0.load(Ordering::SeqCst) {
            return Poll::Ready(());
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

impl RecordingHandle for GatedHandle {
    fn pause(&mut self) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }
    fn resume(&mut self) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }
    fn stop(self: Box<Self>) -> BoxFuture<'static, Result<PathBuf, PortError>> {
        Box::pin(async move {
            WaitFor(self.gate.clone()).await;
            Ok(self.output)
        })
    }
    fn abort(self: Box<Self>) -> BoxFuture<'static, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }
    fn is_alive(&mut self) -> bool {
        true
    }
}

impl Media for GatedMedia {
    fn start(
        &self,
        _input: RecordingInput,
        spec: RecordSpec,
        output: PathBuf,
    ) -> BoxFuture<'_, Result<Box<dyn RecordingHandle>, PortError>> {
        self.specs.lock().unwrap().push(spec);
        let handle: Box<dyn RecordingHandle> = Box::new(GatedHandle { gate: self.gate.clone(), output });
        Box::pin(async move { Ok(handle) })
    }
    fn transcode(&self, _input: &Path, _output: &Path, _spec: TranscodeSpec) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }
    fn probe(&self, _input: &Path) -> BoxFuture<'_, Result<MediaInfo, PortError>> {
        Box::pin(async {
            Ok(MediaInfo {
                duration_secs: 3.0,
                width: 640,
                height: 480,
                fps: 30.0,
                video_codec: "h264".into(),
                audio_codec: None,
                size_bytes: 1000,
            })
        })
    }
}

#[test]
fn finishing_is_reported_before_a_slow_stop_ends() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let media = GatedMedia::new(false);
        let swap = Swap { media: Some(media.clone()), ..Swap::default() };
        let mut h = harness_with(tmp.path(), custom_region, swap);
        start(&h, Action::ScreenRecorderCustomRegion).unwrap();

        h.app.recording_stop();
        step(&mut h);

        // The encoder is still finishing the file: the driver has said so, and has not said it is done.
        let seen = events(&mut h);
        assert!(seen.iter().any(|e| matches!(e, TaskEvent::RecordingFinishing { .. })), "{seen:?}");
        assert!(!seen.iter().any(|e| matches!(e, TaskEvent::RecordingStopped { .. })), "{seen:?}");
        assert!(h.app.is_recording(), "the slot is held until the file is done");

        media.gate.store(true, Ordering::SeqCst);
        settle(&mut h);

        let seen = events(&mut h);
        let finishing = seen.iter().position(|e| matches!(e, TaskEvent::RecordingFinishing { .. })).expect("finishing");
        let stopped = seen.iter().position(|e| matches!(e, TaskEvent::RecordingStopped { .. })).expect("stopped");
        assert!(finishing < stopped, "{seen:?}");
        assert_eq!(seen.iter().filter(|e| matches!(e, TaskEvent::RecordingFinishing { .. })).count(), 1);
        let started_id = recording_id(&mut h);
        assert!(
            seen.iter().any(|e| matches!(e, TaskEvent::RecordingFinishing { id } if *id == started_id)),
            "it is the recording's own id"
        );
        assert!(!h.app.is_recording());
    });
}

#[test]
fn an_abort_does_not_report_finishing() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut h = harness(tmp.path(), custom_region);
        start(&h, Action::ScreenRecorderCustomRegion).unwrap();
        h.app.recording_abort();
        settle(&mut h);
        assert!(!events(&mut h).iter().any(|e| matches!(e, TaskEvent::RecordingFinishing { .. })));
    });
}

/// A stream port that notes which task events had been emitted when `open` was called.
struct SnoopStream {
    inner: FakeScreenStream,
    events: Mutex<Option<Receiver<TaskEvent>>>,
    at_open: Mutex<Option<Vec<TaskEvent>>>,
}

impl ScreenStream for SnoopStream {
    fn open(&self, area: PhysRect, show_cursor: bool, fps: u32, audio: &[AudioDevice]) -> BoxFuture<'_, Result<RecordingInput, PortError>> {
        if let Some(events) = self.events.lock().unwrap().as_mut() {
            let mut seen = Vec::new();
            while let Ok(event) = events.try_recv() {
                seen.push(event);
            }
            *self.at_open.lock().unwrap() = Some(seen);
        }
        self.inner.open(area, show_cursor, fps, audio)
    }
    fn audio_devices(&self) -> BoxFuture<'_, Result<Vec<AudioDevice>, PortError>> {
        self.inner.audio_devices()
    }
    fn camera_devices(&self) -> BoxFuture<'_, Result<Vec<String>, PortError>> {
        self.inner.camera_devices()
    }
}

#[test]
fn the_countdown_is_closed_before_the_stream_opens() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let snoop = Arc::new(SnoopStream { inner: FakeScreenStream::default(), events: Mutex::new(None), at_open: Mutex::new(None) });
        let swap = Swap { stream: Some(snoop.clone()), ..Swap::default() };
        let h = harness_with(
            tmp.path(),
            |s| {
                custom_region(s);
                s.defaults.recording.countdown_secs = 3;
            },
            swap,
        );
        *snoop.events.lock().unwrap() = Some(h.app.events());

        start(&h, Action::ScreenRecorderCustomRegion).unwrap();

        // `RecordingStarted` is the event that closes the countdown window. The stream saw it, and the whole countdown,
        // before it was asked to open.
        let before_open = snoop.at_open.lock().unwrap().clone().expect("the stream was opened");
        let kinds: Vec<String> = before_open
            .iter()
            .map(|e| match e {
                TaskEvent::RecordingCountdown { seconds_left, .. } => format!("count {seconds_left}"),
                TaskEvent::RecordingStarted { .. } => "started".to_owned(),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(kinds, vec!["count 3", "count 2", "count 1", "started"]);
        assert_eq!(calls(&h), vec!["start X264 4x4".to_owned()], "and the recording went on to start");
    });
}

#[test]
fn recording_started_carries_the_border_setting_of_the_task() {
    in_pool(|| {
        let border_of = |seen: &[TaskEvent]| {
            seen.iter()
                .find_map(|e| match e {
                    TaskEvent::RecordingStarted { show_border, .. } => Some(*show_border),
                    _ => None,
                })
                .expect("the recording started")
        };
        let tmp = TempDir::new().unwrap();
        for show in [true, false] {
            let mut h = harness(tmp.path(), |s| {
                custom_region(s);
                s.defaults.recording.show_border = show;
            });
            start(&h, Action::ScreenRecorderCustomRegion).unwrap();
            assert_eq!(border_of(&events(&mut h)), show, "the default setting");
        }

        // A workflow's own recording section wins over the defaults.
        let workflow = Workflow {
            id: WorkflowId(7),
            name: "No border".into(),
            action: Action::ScreenRecorderCustomRegion,
            hotkey: None,
            enabled: true,
            patch: TaskSettingsPatch {
                recording: Some(RecordingSettings { show_border: false, ..RecordingSettings::default() }),
                ..TaskSettingsPatch::default()
            },
        };
        let mut h = harness(tmp.path(), |s| {
            custom_region(s);
            s.defaults.recording.show_border = true;
            s.workflows.push(workflow);
        });
        block_on(h.app.dispatch(Action::ScreenRecorderCustomRegion, Some(WorkflowId(7)))).unwrap();
        assert!(!border_of(&events(&mut h)), "the workflow turns the border off");
    });
}

#[test]
fn the_gif_width_limit_reaches_the_encoder() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let media = GatedMedia::new(true);
        let swap = Swap { media: Some(media.clone()), ..Swap::default() };
        let h = harness_with(
            tmp.path(),
            |s| {
                custom_region(s);
                s.defaults.recording.gif_max_width = 800;
            },
            swap,
        );
        start(&h, Action::ScreenRecorderGifCustomRegion).unwrap();
        assert_eq!(media.specs.lock().unwrap().iter().map(|spec| spec.gif_max_width).collect::<Vec<_>>(), vec![800]);
    });
}

/// A stream like the Windows one: it takes an area on one 8 px wide monitor and refuses an area over the edge.
struct OneMonitorStream {
    inner: FakeScreenStream,
}

impl ScreenStream for OneMonitorStream {
    fn open(&self, area: PhysRect, show_cursor: bool, fps: u32, audio: &[AudioDevice]) -> BoxFuture<'_, Result<RecordingInput, PortError>> {
        if (area.x < 8) != (area.right() <= 8) {
            return Box::pin(async { Err(PortError::Unsupported("the area spans more than one display".into())) });
        }
        self.inner.open(area, show_cursor, fps, audio)
    }
    fn audio_devices(&self) -> BoxFuture<'_, Result<Vec<AudioDevice>, PortError>> {
        self.inner.audio_devices()
    }
}

#[test]
fn an_area_over_two_monitors_is_clipped_to_the_larger_part_with_a_warning() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let stream = Arc::new(OneMonitorStream { inner: FakeScreenStream::default() });
        let swap = Swap { stream: Some(stream.clone()), ..Swap::default() };
        // The monitors are 8x8 at x = 0 and x = 8. The area has 4 columns on the first and 6 on the second.
        let mut h =
            harness_with(tmp.path(), |s| s.defaults.capture.custom_region = Some(PhysRect { x: 4, y: 0, width: 10, height: 6 }), swap);
        assert_eq!(start(&h, Action::ScreenRecorderCustomRegion).unwrap(), Dispatched::Done);

        assert_eq!(*stream.inner.opened.lock().unwrap(), vec![PhysRect { x: 8, y: 0, width: 6, height: 6 }]);
        assert_eq!(calls(&h), vec!["start X264 6x6".to_owned()]);
        let seen = events(&mut h);
        assert!(
            seen.iter().any(|event| matches!(event, TaskEvent::StepFailed { step: "recording area", .. })),
            "the clip is reported: {seen:?}"
        );
        assert!(h.app.is_recording());
    });
}

#[test]
fn an_unsupported_stream_with_no_monitor_part_fails_the_recording() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let stream = Arc::new(OneMonitorStream { inner: FakeScreenStream::default() });
        let swap = Swap { stream: Some(stream), ..Swap::default() };
        // Off every monitor: there is nothing to clip to, so the refusal stands.
        let h = harness_with(tmp.path(), |s| s.defaults.capture.custom_region = Some(PhysRect { x: 7, y: 20, width: 4, height: 4 }), swap);
        assert!(matches!(start(&h, Action::ScreenRecorderCustomRegion), Err(AppError::Unsupported(_))));
        assert!(calls(&h).is_empty());
    });
}

#[test]
fn a_workflows_recorder_backend_reaches_the_media_spec() {
    in_pool(|| {
        let tmp = TempDir::new().unwrap();
        let workflow = Workflow {
            id: WorkflowId(8),
            name: "Native".into(),
            action: Action::ScreenRecorderCustomRegion,
            hotkey: None,
            enabled: true,
            patch: TaskSettingsPatch {
                recording: Some(RecordingSettings {
                    backend: laqta_domain::record::RecorderBackend::Native,
                    ..RecordingSettings::default()
                }),
                ..TaskSettingsPatch::default()
            },
        };
        let media = GatedMedia::new(true);
        let swap = Swap { media: Some(media.clone()), ..Swap::default() };
        let h = harness_with(
            tmp.path(),
            |s| {
                custom_region(s);
                // The defaults say FFmpeg; only the workflow asks for the native recorder.
                s.defaults.recording.backend = laqta_domain::record::RecorderBackend::Ffmpeg;
                s.workflows.push(workflow);
            },
            swap,
        );

        block_on(h.app.dispatch(Action::ScreenRecorderCustomRegion, Some(WorkflowId(8)))).unwrap();
        assert_eq!(media.specs.lock().unwrap().last().unwrap().backend, laqta_domain::record::RecorderBackend::Native);

        // The same action without the workflow uses the defaults.
        let media = GatedMedia::new(true);
        let swap = Swap { media: Some(media.clone()), ..Swap::default() };
        let h = harness_with(tmp.path(), custom_region, swap);
        start(&h, Action::ScreenRecorderCustomRegion).unwrap();
        assert_eq!(media.specs.lock().unwrap().last().unwrap().backend, laqta_domain::record::RecorderBackend::Ffmpeg);
    });
}
