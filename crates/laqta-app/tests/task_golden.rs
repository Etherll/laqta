#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Golden event traces for the Capture job (M0 §7.1, M1 §4.3). Each test renders the `TaskEvent`s as lines
//! and compares them with `tests/golden/<name>.trace`. Window titles, cropped images and stops are traced in
//! `src/task/tests.rs`, because the dispatcher does not submit those jobs yet.

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use async_broadcast::Receiver;
use futures_lite::future::{block_on, poll_once, yield_now};
use laqta_app::{App, Dispatched, Ports};
use laqta_domain::workflow::{AfterCaptureTasks, FileExistsAction};
use laqta_domain::{Action, Bitmap, Frame, ImageFormat, Monitor, MonitorId, PhysRect, PixelFormat, Settings};
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeHoverSource, FakeNotifier, FakeOverlay, FakeScreen, FakeShell, InlineSpawner,
    ManualClock,
};
use laqta_ports::{AppError, BoxFuture, Spawner, TaskEvent, TaskId};
use tempfile::TempDir;

/// Runs the test body inside a 2-thread compute pool, because compose and encode check that they
/// run inside one (ADR 024).
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

/// The app and the fakes a test inspects afterwards.
struct Rig {
    app: Arc<App>,
    clipboard: Arc<FakeClipboard>,
    shell: Arc<FakeShell>,
    notifier: Arc<FakeNotifier>,
}

fn build(behaviour: ClipboardBehaviour, spawner: Arc<dyn Spawner>, settings: Settings) -> Rig {
    let clipboard = Arc::new(FakeClipboard::new(behaviour));
    let shell = Arc::new(FakeShell::default());
    let notifier = Arc::new(FakeNotifier::default());
    let ports = Ports {
        screen: Arc::new(FakeScreen::new(two_frames())),
        clipboard: clipboard.clone(),
        shell: shell.clone(),
        clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0))),
        spawner,
        overlay: Arc::new(FakeOverlay::default()),
        tools: Arc::new(laqta_ports::fakes::FakeToolHost::default()),
        prompt: Arc::new(laqta_ports::fakes::FakePrompt::default()),
        uploaders: Arc::new(laqta_ports::fakes::FakeUploaders::default()),
        hover: Arc::new(FakeHoverSource::new(Vec::new())),
        notifier: notifier.clone(),
        rng: Arc::new(CountingRng::default()),
        host: laqta_ports::HostInfo::default(),
        codecs: laqta_imaging::Codecs::builtin(),
    };
    Rig { app: App::new(ports, settings), clipboard, shell, notifier }
}

/// M0 settings with the screenshots folder in `dir`.
fn settings_in(dir: &Path) -> Settings {
    Settings { screenshots_dir: dir.to_path_buf(), ..Settings::default_for(dir.to_path_buf()) }
}

/// Dispatches one PrintScreen with `settings`, runs its job, and returns the rig with the rendered trace.
fn capture(dir: &Path, behaviour: ClipboardBehaviour, settings: Settings) -> (Rig, String) {
    let spawner = Arc::new(InlineSpawner::default());
    let rig = build(behaviour, spawner.clone(), settings);
    let mut events = rig.app.events();
    assert_eq!(block_on(rig.app.dispatch(Action::PrintScreen, None)).unwrap(), Dispatched::Task(TaskId(1)));
    drive(&spawner);
    let trace = render(&mut events, dir, false);
    (rig, trace)
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

fn decoded_size(path: &Path) -> (u32, u32) {
    let bytes = std::fs::read(path).unwrap();
    let reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
    (reader.info().width, reader.info().height)
}

fn sniffed(path: &Path) -> Option<ImageFormat> {
    laqta_imaging::sniff(&std::fs::read(path).unwrap())
}

fn has_part_file(dir: &Path) -> bool {
    std::fs::read_dir(dir).unwrap().any(|entry| {
        let path = entry.unwrap().path();
        if path.is_dir() { has_part_file(&path) } else { path.extension().is_some_and(|ext| ext == "part") }
    })
}

/// The file the default name pattern gives the capture at 14:03:22 on 2026-11-07.
fn saved_png(dir: &Path) -> std::path::PathBuf {
    dir.join("2026-11/Laqta_2026-11-07_14-03-22.png")
}

#[test]
fn capture_ok() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let (rig, trace) = capture(tmp.path(), ClipboardBehaviour::Succeed, settings_in(tmp.path()));

        assert_eq!(trace, include_str!("golden/capture_ok.trace"));
        assert_eq!(decoded_size(&saved_png(tmp.path())), (8, 4));
        let copied = rig.clipboard.images.lock().unwrap();
        assert_eq!((copied[0].width(), copied[0].height()), (8, 4));
        let toasts = rig.notifier.toasts.lock().unwrap();
        assert_eq!(toasts.len(), 1);
        assert_eq!(toasts[0].title, "Screenshot saved");
        assert_eq!(toasts[0].path.as_deref(), Some(saved_png(tmp.path()).as_path()));
    });
}

#[test]
fn clipboard_fails() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let (_rig, trace) = capture(tmp.path(), ClipboardBehaviour::Fail, settings_in(tmp.path()));

        assert_eq!(trace, include_str!("golden/clipboard_fails.trace"));
        assert!(saved_png(tmp.path()).is_file());
    });
}

#[test]
fn save_fails() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        // The screenshots folder is a regular file, so nothing can be written under it.
        let shots = tmp.path().join("shots");
        std::fs::write(&shots, b"not a folder").unwrap();
        let (rig, trace) = capture(&shots, ClipboardBehaviour::Succeed, settings_in(&shots));

        assert_eq!(trace, include_str!("golden/save_fails.trace"));
        assert!(!has_part_file(tmp.path()));
        assert!(rig.clipboard.images.lock().unwrap().is_empty(), "a failed save copies nothing");
    });
}

#[test]
fn name_collision() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let folder = tmp.path().join("2026-11");
        std::fs::create_dir_all(&folder).unwrap();
        let existing = folder.join("Laqta_2026-11-07_14-03-22.png");
        std::fs::write(&existing, b"existing").unwrap();
        let (_rig, trace) = capture(tmp.path(), ClipboardBehaviour::Succeed, settings_in(tmp.path()));

        assert_eq!(trace, include_str!("golden/name_collision.trace"));
        assert_eq!(std::fs::read(&existing).unwrap(), b"existing");
        assert_eq!(decoded_size(&folder.join("Laqta_2026-11-07_14-03-22_2.png")), (8, 4));
    });
}

#[test]
fn panic_isolated() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let rig = build(ClipboardBehaviour::Panic, spawner.clone(), settings_in(tmp.path()));
        let mut events = rig.app.events();

        assert_eq!(block_on(rig.app.dispatch(Action::PrintScreen, None)).unwrap(), Dispatched::Task(TaskId(1)));
        drive(&spawner);

        *rig.clipboard.behaviour.lock().unwrap() = ClipboardBehaviour::Succeed;
        assert_eq!(block_on(rig.app.dispatch(Action::PrintScreen, None)).unwrap(), Dispatched::Task(TaskId(2)));
        drive(&spawner);

        assert_eq!(render(&mut events, tmp.path(), false), include_str!("golden/panic_isolated.trace"));
    });
}

/// Like `InlineSpawner`, but each compute and blocking step yields once after it finishes. The
/// inline futures are always ready, so without this a job never stays in the middle and two slots
/// are never visibly held.
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
fn concurrency() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(YieldingSpawner::default());
        let rig = build(ClipboardBehaviour::Succeed, spawner.clone(), settings_in(tmp.path()));
        let mut events = rig.app.events();

        for expected in 1..=3 {
            assert_eq!(block_on(rig.app.dispatch(Action::PrintScreen, None)).unwrap(), Dispatched::Task(TaskId(expected)));
        }
        let mut jobs: Vec<Option<BoxFuture<'static, ()>>> = spawner.inner.take_spawned().into_iter().map(Some).collect();
        assert_eq!(jobs.len(), 3);

        // Pass 1: jobs 1 and 2 take both capture slots and park in compute. Job 3 waits.
        poll_pass(&mut jobs);
        let mut trace = render(&mut events, tmp.path(), true);
        assert!(!trace.contains("#3 status Preparing"), "job 3 started while two slots were busy:\n{trace}");
        assert!(trace.contains("#1 status Working") && trace.contains("#2 status Working"));

        for _ in 0..50 {
            if jobs.iter().all(Option::is_none) {
                break;
            }
            poll_pass(&mut jobs);
            trace.push_str(&render(&mut events, tmp.path(), true));
        }
        assert!(jobs.iter().all(Option::is_none), "a job never finished");

        assert_eq!(trace, include_str!("golden/concurrency.trace"));
    });
}

#[test]
fn jpeg_format() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut settings = settings_in(tmp.path());
        settings.defaults.image.format = ImageFormat::Jpeg;
        let (_rig, trace) = capture(tmp.path(), ClipboardBehaviour::Succeed, settings);

        assert_eq!(trace, include_str!("golden/jpeg_format.trace"));
        assert_eq!(sniffed(&tmp.path().join("2026-11/Laqta_2026-11-07_14-03-22.jpg")), Some(ImageFormat::Jpeg));
    });
}

#[test]
fn auto_jpeg() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut settings = settings_in(tmp.path());
        // Any PNG is larger than one byte, so the PNG is replaced by a JPEG.
        settings.defaults.image.auto_jpeg_above_bytes = Some(1);
        let (_rig, trace) = capture(tmp.path(), ClipboardBehaviour::Succeed, settings);

        assert_eq!(trace, include_str!("golden/auto_jpeg.trace"));
        assert_eq!(sniffed(&tmp.path().join("2026-11/Laqta_2026-11-07_14-03-22.jpg")), Some(ImageFormat::Jpeg));
        assert!(!saved_png(tmp.path()).exists());
    });
}

#[test]
fn thumbnail() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut settings = settings_in(tmp.path());
        settings.defaults.after_capture = AfterCaptureTasks::SAVE_IMAGE_TO_FILE
            | AfterCaptureTasks::SAVE_THUMBNAIL_IMAGE_TO_FILE
            | AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD;
        settings.defaults.image.thumbnail_width = 4;
        let (_rig, trace) = capture(tmp.path(), ClipboardBehaviour::Succeed, settings);

        assert_eq!(trace, include_str!("golden/thumbnail.trace"));
        // The 8x4 image shrinks to 4x2 at the same aspect ratio.
        let thumb = tmp.path().join("2026-11/Laqta_2026-11-07_14-03-22-thumbnail.png");
        assert_eq!(decoded_size(&thumb), (4, 2));
        assert_eq!(decoded_size(&saved_png(tmp.path())), (8, 4));
    });
}

#[test]
fn copy_file() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut settings = settings_in(tmp.path());
        settings.defaults.after_capture = AfterCaptureTasks::SAVE_IMAGE_TO_FILE | AfterCaptureTasks::COPY_FILE_TO_CLIPBOARD;
        let (rig, trace) = capture(tmp.path(), ClipboardBehaviour::Succeed, settings);

        assert_eq!(trace, include_str!("golden/copy_file.trace"));
        assert_eq!(*rig.clipboard.files.lock().unwrap(), vec![vec![saved_png(tmp.path())]]);
        assert!(rig.clipboard.images.lock().unwrap().is_empty(), "the image itself is not copied");
    });
}

#[test]
fn copy_path() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut settings = settings_in(tmp.path());
        settings.defaults.after_capture = AfterCaptureTasks::SAVE_IMAGE_TO_FILE | AfterCaptureTasks::COPY_FILE_PATH_TO_CLIPBOARD;
        let (rig, trace) = capture(tmp.path(), ClipboardBehaviour::Succeed, settings);

        assert_eq!(trace, include_str!("golden/copy_path.trace"));
        assert_eq!(*rig.clipboard.texts.lock().unwrap(), vec![saved_png(tmp.path()).display().to_string()]);
    });
}

#[test]
fn reveal() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut settings = settings_in(tmp.path());
        settings.defaults.after_capture = AfterCaptureTasks::SAVE_IMAGE_TO_FILE | AfterCaptureTasks::SHOW_IN_EXPLORER;
        let (rig, trace) = capture(tmp.path(), ClipboardBehaviour::Succeed, settings);

        assert_eq!(trace, include_str!("golden/reveal.trace"));
        assert_eq!(*rig.shell.revealed.lock().unwrap(), vec![saved_png(tmp.path())]);
    });
}

#[test]
fn no_toast() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut settings = settings_in(tmp.path());
        settings.defaults.general.show_toast = false;
        let (rig, trace) = capture(tmp.path(), ClipboardBehaviour::Succeed, settings);

        assert_eq!(trace, include_str!("golden/no_toast.trace"));
        assert!(rig.notifier.toasts.lock().unwrap().is_empty());
    });
}

#[test]
fn delete_file() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let mut settings = settings_in(tmp.path());
        settings.defaults.after_capture = AfterCaptureTasks::SAVE_IMAGE_TO_FILE | AfterCaptureTasks::DELETE_FILE;
        let (_rig, trace) = capture(tmp.path(), ClipboardBehaviour::Succeed, settings);

        assert_eq!(trace, include_str!("golden/delete_file.trace"));
        assert!(!saved_png(tmp.path()).exists());
    });
}

#[test]
fn file_exists_overwrite() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let folder = tmp.path().join("2026-11");
        std::fs::create_dir_all(&folder).unwrap();
        let existing = saved_png(tmp.path());
        std::fs::write(&existing, b"existing").unwrap();
        let mut settings = settings_in(tmp.path());
        settings.defaults.image.file_exists = FileExistsAction::Overwrite;
        let (_rig, trace) = capture(tmp.path(), ClipboardBehaviour::Succeed, settings);

        assert_eq!(trace, include_str!("golden/file_exists_overwrite.trace"));
        assert_eq!(decoded_size(&existing), (8, 4), "the existing file was replaced");
        assert!(!folder.join("Laqta_2026-11-07_14-03-22_2.png").exists());
    });
}

#[test]
fn file_exists_skip() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let folder = tmp.path().join("2026-11");
        std::fs::create_dir_all(&folder).unwrap();
        let existing = saved_png(tmp.path());
        std::fs::write(&existing, b"existing").unwrap();
        let mut settings = settings_in(tmp.path());
        settings.defaults.image.file_exists = FileExistsAction::Skip;
        let (rig, trace) = capture(tmp.path(), ClipboardBehaviour::Succeed, settings);

        assert_eq!(trace, include_str!("golden/file_exists_skip.trace"));
        assert_eq!(std::fs::read(&existing).unwrap(), b"existing", "the existing file is untouched");
        assert!(!folder.join("Laqta_2026-11-07_14-03-22_2.png").exists());
        assert_eq!(rig.clipboard.images.lock().unwrap().len(), 1, "the image is still copied");
    });
}
