#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Golden traces for the jobs the dispatcher cannot submit yet: a window title, a cropped image, and stop
//! requests. They render the same lines as `tests/task_golden.rs` and compare with the same golden files.

use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_broadcast::Receiver;
use futures_lite::future::{block_on, poll_once, yield_now};
use laqta_domain::workflow::AfterCaptureTasks;
use laqta_domain::{Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat, Settings};
use laqta_imaging::Codecs;
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeHoverSource, FakeNotifier, FakeOverlay, FakeScreen, FakeShell, InlineSpawner,
    ManualClock,
};
use laqta_ports::{AppError, BoxFuture, HostInfo, Spawner, TaskEvent, TaskId};
use tempfile::TempDir;

use super::{CaptureImage, CaptureInfo, Task, TaskJob};
use crate::dispatcher::{Ports, SettingsCell};

fn in_compute_pool<R: Send>(body: impl FnOnce() -> R + Send) -> R {
    rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap().install(body)
}

fn frame(x: i32, pixel: [u8; 4]) -> Frame {
    let pixels: Vec<u8> = std::iter::repeat_n(pixel, 16).flatten().collect();
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

fn two_frames() -> Vec<Frame> {
    vec![frame(0, [10, 20, 30, 255]), frame(4, [40, 50, 60, 255])]
}

fn ports(spawner: Arc<dyn Spawner>) -> Arc<Ports> {
    Arc::new(Ports {
        screen: Arc::new(FakeScreen::new(two_frames())),
        clipboard: Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed)),
        shell: Arc::new(FakeShell::default()),
        clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0))),
        spawner,
        overlay: Arc::new(FakeOverlay::default()),
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
        hover: Arc::new(FakeHoverSource::new(Vec::new())),
        notifier: Arc::new(FakeNotifier::default()),
        rng: Arc::new(CountingRng::default()),
        host: HostInfo::default(),
        codecs: Codecs::builtin(),
    })
}

/// A Task whose events are read through the returned receiver.
fn task_on(spawner: Arc<dyn Spawner>, settings: Settings) -> (Task, Receiver<TaskEvent>) {
    let (mut tx, rx) = async_broadcast::broadcast::<TaskEvent>(256);
    tx.set_overflow(true);
    let task = Task::new(ports(spawner), Arc::new(SettingsCell::new(settings)), tx);
    (task, rx)
}

fn settings_in(dir: &Path) -> Settings {
    Settings { screenshots_dir: dir.to_path_buf(), ..Settings::default_for(dir.to_path_buf()) }
}

/// A Capture job with the default flags and the given capture.
fn job(settings: &Settings, image: CaptureImage, info: CaptureInfo) -> TaskJob {
    TaskJob::Capture { image, settings: settings.task_settings(None), info }
}

fn frames_job(settings: &Settings) -> TaskJob {
    job(settings, CaptureImage::Frames(two_frames()), CaptureInfo::default())
}

/// Runs every job spawned so far, and any jobs those spawn, until none are left.
fn drive(spawner: &InlineSpawner) {
    loop {
        let jobs = spawner.take_spawned();
        if jobs.is_empty() {
            break;
        }
        for job in jobs {
            block_on(job);
        }
    }
}

/// One line per event. With `with_ids`, each line starts with `#<task id> `.
fn render(events: &mut Receiver<TaskEvent>, dir: &Path, with_ids: bool) -> String {
    let mut out = String::new();
    while let Ok(event) = events.try_recv() {
        let (id, line) = describe(&event, dir);
        if with_ids {
            out.push_str(&format!("#{id} "));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn describe(event: &TaskEvent, dir: &Path) -> (u64, String) {
    match event {
        TaskEvent::StatusChanged { id, status } => (id.0, format!("status {status:?}")),
        TaskEvent::Saved { id, path } => (id.0, format!("saved {}", relative(dir, path))),
        TaskEvent::SaveSkipped { id, path } => (id.0, format!("save_skipped {}", relative(dir, path))),
        TaskEvent::Copied { id } => (id.0, "copied".to_owned()),
        TaskEvent::CopyFailed { id, .. } => (id.0, "copy_failed".to_owned()),
        TaskEvent::ThumbnailSaved { id, path } => (id.0, format!("thumbnail {}", relative(dir, path))),
        TaskEvent::FileCopied { id } => (id.0, "file_copied".to_owned()),
        TaskEvent::PathCopied { id, path } => (id.0, format!("path_copied {}", relative(dir, path))),
        TaskEvent::Revealed { id, path } => (id.0, format!("revealed {}", relative(dir, path))),
        TaskEvent::Notified { id } => (id.0, "notified".to_owned()),
        TaskEvent::Deleted { id, path } => (id.0, format!("deleted {}", relative(dir, path))),
        TaskEvent::StepFailed { id, step, .. } => (id.0, format!("step_failed {step}")),
        TaskEvent::Failed { id, error } => (id.0, format!("failed {}", error_kind(error))),
        other => panic!("unexpected event {other:?}"),
    }
}

fn relative(dir: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(dir).unwrap();
    rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

fn error_kind(error: &AppError) -> &'static str {
    match error {
        AppError::Io(_) => "io",
        AppError::Panic(_) => "internal error",
        _ => "other",
    }
}

fn png_size(path: &Path) -> (u32, u32) {
    let bytes = std::fs::read(path).unwrap();
    let reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
    (reader.info().width, reader.info().height)
}

#[test]
fn window_pattern() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let settings = settings_in(tmp.path());
        let (task, mut events) = task_on(spawner.clone(), settings.clone());
        let info = CaptureInfo { window_title: "Editor".to_owned(), process_name: "gedit".to_owned() };
        task.submit(job(&settings, CaptureImage::Frames(two_frames()), info));
        drive(&spawner);

        assert_eq!(render(&mut events, tmp.path(), false), include_str!("../../tests/golden/window_pattern.trace"));
        assert!(tmp.path().join("2026-11/gedit_2026-11-07_14-03-22.png").is_file());
    });
}

#[test]
fn region_image() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let settings = settings_in(tmp.path());
        let (task, mut events) = task_on(spawner.clone(), settings.clone());
        let region = Bitmap::new(3, 2, PixelFormat::Rgba8, vec![200; 3 * 2 * 4].into()).unwrap();
        task.submit(job(&settings, CaptureImage::Image(region), CaptureInfo::default()));
        drive(&spawner);

        assert_eq!(render(&mut events, tmp.path(), false), include_str!("../../tests/golden/region_image.trace"));
        // The cropped image is saved as it was given: its own size, not a composed desktop.
        assert_eq!(png_size(&tmp.path().join("2026-11/Laqta_2026-11-07_14-03-22.png")), (3, 2));
    });
}

/// Like `InlineSpawner`, but each compute and blocking step yields once after it finishes, so jobs stay
/// parked in the middle and the capture slots are visibly held.
#[derive(Debug, Default)]
struct YieldingSpawner {
    inner: InlineSpawner,
}

impl Spawner for YieldingSpawner {
    fn spawn(&self, task: BoxFuture<'static, ()>) {
        self.inner.spawn(task);
    }

    fn run_blocking(&self, work: Box<dyn FnOnce() + Send>) -> BoxFuture<'static, ()> {
        let done = self.inner.run_blocking(work);
        Box::pin(async move {
            done.await;
            yield_now().await;
        })
    }

    fn run_compute(&self, work: Box<dyn FnOnce() + Send>) -> BoxFuture<'static, ()> {
        let done = self.inner.run_compute(work);
        Box::pin(async move {
            done.await;
            yield_now().await;
        })
    }
}

/// Polls each unfinished job once, in submission order. Finished jobs become `None`.
fn poll_pass(jobs: &mut [Option<BoxFuture<'static, ()>>]) {
    for slot in jobs.iter_mut() {
        let finished = match slot {
            Some(job) => block_on(poll_once(job.as_mut())).is_some(),
            None => false,
        };
        if finished {
            *slot = None;
        }
    }
}

#[test]
fn stop_queued() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(YieldingSpawner::default());
        let settings = settings_in(tmp.path());
        let (task, mut events) = task_on(spawner.clone(), settings.clone());
        for _ in 0..3 {
            task.submit(frames_job(&settings));
        }
        let mut jobs: Vec<Option<BoxFuture<'static, ()>>> = spawner.inner.take_spawned().into_iter().map(Some).collect();

        // Pass 1: jobs 1 and 2 take both capture slots and park in compute. Job 3 parks waiting for a slot.
        poll_pass(&mut jobs);
        let mut trace = render(&mut events, tmp.path(), true);
        assert!(!trace.contains("#3 status Preparing"), "job 3 started while two slots were busy:\n{trace}");

        assert!(task.stop(TaskId(3)));
        assert!(!task.stop(TaskId(99)), "an unknown job is not stopped");
        trace.push_str(&render(&mut events, tmp.path(), true));

        for _ in 0..50 {
            if jobs.iter().all(Option::is_none) {
                break;
            }
            poll_pass(&mut jobs);
            trace.push_str(&render(&mut events, tmp.path(), true));
        }
        assert!(jobs.iter().all(Option::is_none), "a job never finished, so the stopped one did not hang");

        assert_eq!(trace, include_str!("../../tests/golden/stop_queued.trace"));
        assert!(!tmp.path().join("2026-11/Laqta_2026-11-07_14-03-22_3.png").exists());
    });
}

#[test]
fn stop_all() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let settings = settings_in(tmp.path());
        let (task, mut events) = task_on(spawner.clone(), settings.clone());
        for _ in 0..3 {
            task.submit(frames_job(&settings));
        }

        task.stop_all();
        drive(&spawner);

        assert_eq!(render(&mut events, tmp.path(), true), include_str!("../../tests/golden/stop_all.trace"));
        assert!(!tmp.path().join("2026-11").exists(), "no job ran, so no folder was made");
    });
}

/// Like `InlineSpawner`, but runs one hook at a chosen point, so a test can stop a job that is running.
#[derive(Default)]
struct HookSpawner {
    inner: InlineSpawner,
    /// Runs at the first compute step, before the work.
    before_compose: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Runs after the second blocking step (the save) has written its file.
    after_save: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    blocking_calls: Mutex<u32>,
}

impl Spawner for HookSpawner {
    fn spawn(&self, task: BoxFuture<'static, ()>) {
        self.inner.spawn(task);
    }

    fn run_blocking(&self, work: Box<dyn FnOnce() + Send>) -> BoxFuture<'static, ()> {
        let call = {
            let mut calls = self.blocking_calls.lock().unwrap();
            *calls += 1;
            *calls
        };
        let hook = if call == 2 { self.after_save.lock().unwrap().take() } else { None };
        self.inner.run_blocking(Box::new(move || {
            work();
            if let Some(hook) = hook {
                hook();
            }
        }))
    }

    fn run_compute(&self, work: Box<dyn FnOnce() + Send>) -> BoxFuture<'static, ()> {
        let hook = self.before_compose.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
        self.inner.run_compute(work)
    }
}

#[test]
fn stop_running() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(HookSpawner::default());
        let settings = settings_in(tmp.path());
        let (task, mut events) = task_on(spawner.clone(), settings.clone());
        let task = Arc::new(task);
        let stopper = Arc::clone(&task);
        *spawner.before_compose.lock().unwrap() = Some(Box::new(move || {
            assert!(stopper.stop(TaskId(1)), "a running job can be stopped");
        }));

        task.submit(frames_job(&settings));
        drive(&spawner.inner);

        assert_eq!(render(&mut events, tmp.path(), true), include_str!("../../tests/golden/stop_running.trace"));
        assert!(!tmp.path().join("2026-11").exists(), "the job stopped before it saved anything");
    });
}

#[test]
fn stop_after_save() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(HookSpawner::default());
        let settings = settings_in(tmp.path());
        let (task, mut events) = task_on(spawner.clone(), settings.clone());
        let task = Arc::new(task);
        let stopper = Arc::clone(&task);
        *spawner.after_save.lock().unwrap() = Some(Box::new(move || {
            assert!(stopper.stop(TaskId(1)), "a running job can be stopped");
        }));

        task.submit(frames_job(&settings));
        drive(&spawner.inner);

        assert_eq!(render(&mut events, tmp.path(), true), include_str!("../../tests/golden/stop_after_save.trace"));
        // The stop came after the save, so the file stays, and the steps after the checkpoint never ran.
        assert_eq!(png_size(&tmp.path().join("2026-11/Laqta_2026-11-07_14-03-22.png")), (8, 4));
    });
}

/// A stop after the save ends the job at its next checkpoint, and `DELETE_FILE` still removes the file.
#[test]
fn delete_file_after_stop() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(HookSpawner::default());
        let mut settings = settings_in(tmp.path());
        settings.defaults.after_capture = AfterCaptureTasks::SAVE_IMAGE_TO_FILE | AfterCaptureTasks::DELETE_FILE;
        let (task, mut events) = task_on(spawner.clone(), settings.clone());
        let task = Arc::new(task);
        let stopper = Arc::clone(&task);
        *spawner.after_save.lock().unwrap() = Some(Box::new(move || {
            assert!(stopper.stop(TaskId(1)), "a running job can be stopped");
        }));

        task.submit(frames_job(&settings));
        drive(&spawner.inner);

        assert_eq!(render(&mut events, tmp.path(), true), include_str!("../../tests/golden/delete_file_after_stop.trace"));
        assert!(!tmp.path().join("2026-11/Laqta_2026-11-07_14-03-22.png").exists(), "the stopped job still deleted its file");
    });
}
