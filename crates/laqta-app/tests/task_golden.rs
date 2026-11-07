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
use laqta_domain::workflow::{AfterCaptureTasks, AfterUploadTasks, FileExistsAction};
use laqta_domain::{Action, Bitmap, Frame, ImageFormat, Monitor, MonitorId, PhysRect, PixelFormat, Settings};
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeDestination, FakeHoverSource, FakeNotifier, FakeOverlay, FakePrompt, FakeScreen,
    FakeShell, FakeShortener, FakeUploaders, InlineSpawner, ManualClock,
};
use laqta_ports::{
    AppError, BoxFuture, ClipboardContent, Clock, Spawner, TaskEvent, TaskId, UploadData, UploadError, UploadKind, UploadResult,
};
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
    uploaders: Arc<FakeUploaders>,
    prompt: Arc<FakePrompt>,
    clock: Arc<ManualClock>,
}

fn build(behaviour: ClipboardBehaviour, spawner: Arc<dyn Spawner>, settings: Settings) -> Rig {
    let clipboard = Arc::new(FakeClipboard::new(behaviour));
    let shell = Arc::new(FakeShell::default());
    let notifier = Arc::new(FakeNotifier::default());
    let uploaders = Arc::new(FakeUploaders::default());
    let prompt = Arc::new(FakePrompt::default());
    let clock = Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0)));
    let ports = Ports {
        screen: Arc::new(FakeScreen::new(two_frames())),
        clipboard: clipboard.clone(),
        shell: shell.clone(),
        clock: clock.clone(),
        spawner,
        overlay: Arc::new(FakeOverlay::default()),
        tools: Arc::new(laqta_ports::fakes::FakeToolHost::default()),
        prompt: prompt.clone(),
        uploaders: uploaders.clone(),
        hover: Arc::new(FakeHoverSource::new(Vec::new())),
        notifier: notifier.clone(),
        rng: Arc::new(CountingRng::default()),
        host: laqta_ports::HostInfo::default(),
        codecs: laqta_imaging::Codecs::builtin(),
    };
    Rig { app: App::new(ports, settings), clipboard, shell, notifier, uploaders, prompt, clock }
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
        TaskEvent::UploadStarted { id, destination } => (id.0, format!("upload_started {destination}")),
        TaskEvent::Progress { id, sent, total } => (id.0, format!("progress {sent} {total:?}")),
        TaskEvent::UploadRetry { id, attempt, .. } => (id.0, format!("upload_retry {attempt}")),
        TaskEvent::Uploaded { id, .. } => (id.0, "uploaded".to_owned()),
        TaskEvent::Shortened { id, .. } => (id.0, "shortened".to_owned()),
        TaskEvent::UrlCopied { id, .. } => (id.0, "url_copied".to_owned()),
        TaskEvent::UrlOpened { id, .. } => (id.0, "url_opened".to_owned()),
        TaskEvent::Shared { id, .. } => (id.0, "shared".to_owned()),
        TaskEvent::ShowUrl { id, qr, .. } => (id.0, if *qr { "show_qr" } else { "show_url_window" }.to_owned()),
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
        AppError::Network(_) => "network",
        AppError::Auth(_) => "auth",
        AppError::Cancelled => "cancelled",
        AppError::Unsupported(_) => "unsupported",
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

/// A successful upload: the URL, a deletion URL, and a response body.
fn uploaded(url: &str) -> UploadResult {
    UploadResult {
        url: Some(url.to_owned()),
        thumbnail_url: None,
        deletion_url: Some(format!("{url}/delete")),
        response: format!("{url} ok"),
    }
}

/// Registers a destination with scripted results. The test keeps the handle to read the requests it got.
fn destination(rig: &Rig, id: &str, results: Vec<Result<UploadResult, UploadError>>) -> Arc<FakeDestination> {
    let destination = Arc::new(FakeDestination::new(id, results));
    rig.uploaders.destinations.lock().unwrap().push(destination.clone());
    destination
}

/// M0 settings for an upload: a capture that saves its image, copies it, and uploads it.
fn upload_settings(dir: &Path) -> Settings {
    let mut settings = settings_in(dir);
    settings.defaults.after_capture =
        AfterCaptureTasks::SAVE_IMAGE_TO_FILE | AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD | AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST;
    settings
}

/// Dispatches `action`, runs the jobs it submits, and returns the trace of their events.
fn dispatch_and_trace(rig: &Rig, spawner: &InlineSpawner, events: &mut Receiver<TaskEvent>, dir: &Path, action: Action) -> String {
    block_on(rig.app.dispatch(action, None)).unwrap();
    drive(spawner);
    render(events, dir, false)
}

/// The fake clock's time at 14:03:`second` on 2026-11-07.
fn at_second(second: i8) -> jiff::civil::DateTime {
    jiff::civil::date(2026, 11, 7).at(14, 3, second, 0)
}

#[test]
fn upload_ok() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let rig = build(ClipboardBehaviour::Succeed, spawner.clone(), upload_settings(tmp.path()));
        let imgur = destination(&rig, "imgur", vec![Ok(uploaded("https://img.example/1"))]);
        let mut events = rig.app.events();

        let trace = dispatch_and_trace(&rig, &spawner, &mut events, tmp.path(), Action::PrintScreen);

        assert_eq!(trace, include_str!("golden/upload_ok.trace"));
        let requests = imgur.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].file_name, "Laqta_2026-11-07_14-03-22.png");
        assert_eq!(requests[0].kind, UploadKind::Image);
        assert_eq!(requests[0].mime, "image/png");
        // The bytes sent are the bytes saved: the image is encoded once.
        assert_eq!(requests[0].data, UploadData::Bytes(std::fs::read(saved_png(tmp.path())).unwrap()));
        assert_eq!(*rig.clipboard.texts.lock().unwrap(), vec!["https://img.example/1".to_owned()]);
        // The toast carries the URL, so the UI can offer it.
        assert_eq!(rig.notifier.toasts.lock().unwrap()[0].url.as_deref(), Some("https://img.example/1"));
    });
}

#[test]
fn upload_retry_then_ok() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let rig = build(ClipboardBehaviour::Succeed, spawner.clone(), upload_settings(tmp.path()));
        let imgur =
            destination(&rig, "imgur", vec![Err(UploadError::Network("connection reset".into())), Ok(uploaded("https://img.example/2"))]);
        let mut events = rig.app.events();

        let trace = dispatch_and_trace(&rig, &spawner, &mut events, tmp.path(), Action::PrintScreen);

        assert_eq!(trace, include_str!("golden/upload_retry_then_ok.trace"));
        assert_eq!(imgur.requests.lock().unwrap().len(), 2);
        assert_eq!(rig.clock.now(), at_second(23), "the first retry waited 1 s");
    });
}

#[test]
fn upload_fails_after_retries() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let rig = build(ClipboardBehaviour::Succeed, spawner.clone(), upload_settings(tmp.path()));
        let server_error = UploadError::Server { status: 503, body: "busy".into() };
        let imgur = destination(&rig, "imgur", vec![Err(server_error.clone()), Err(server_error.clone()), Err(server_error)]);
        let mut events = rig.app.events();

        let trace = dispatch_and_trace(&rig, &spawner, &mut events, tmp.path(), Action::PrintScreen);

        assert_eq!(trace, include_str!("golden/upload_fails_after_retries.trace"));
        assert_eq!(imgur.requests.lock().unwrap().len(), 3, "one attempt and two retries");
        assert_eq!(rig.clock.now(), at_second(26), "the waits were 1 s and then 3 s");
        assert!(saved_png(tmp.path()).is_file(), "a failed upload keeps the saved file");
    });
}

#[test]
fn upload_auth_fails_no_retry() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let rig = build(ClipboardBehaviour::Succeed, spawner.clone(), upload_settings(tmp.path()));
        let imgur = destination(&rig, "imgur", vec![Err(UploadError::Auth("bad token".into()))]);
        let mut events = rig.app.events();

        let trace = dispatch_and_trace(&rig, &spawner, &mut events, tmp.path(), Action::PrintScreen);

        assert_eq!(trace, include_str!("golden/upload_auth_fails_no_retry.trace"));
        assert_eq!(imgur.requests.lock().unwrap().len(), 1, "authentication errors are not retried");
        assert_eq!(rig.clock.now(), at_second(22), "no wait happened");
    });
}

#[test]
fn upload_shorten_and_open() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let mut settings = upload_settings(tmp.path());
        settings.defaults.after_upload =
            AfterUploadTasks::USE_URL_SHORTENER | AfterUploadTasks::COPY_URL_TO_CLIPBOARD | AfterUploadTasks::OPEN_URL;
        settings.defaults.upload.url_shortener = "fake".into();
        settings.defaults.upload.clipboard_format = "$shorturl".into();
        let rig = build(ClipboardBehaviour::Succeed, spawner.clone(), settings);
        rig.uploaders.shorteners.lock().unwrap().push(Arc::new(FakeShortener));
        destination(&rig, "imgur", vec![Ok(uploaded("https://img.example/3"))]);
        let mut events = rig.app.events();

        let trace = dispatch_and_trace(&rig, &spawner, &mut events, tmp.path(), Action::PrintScreen);

        assert_eq!(trace, include_str!("golden/upload_shorten_and_open.trace"));
        // The short URL is the one copied and opened, and `$shorturl` names it.
        assert_eq!(*rig.clipboard.texts.lock().unwrap(), vec!["https://img.example/3/s".to_owned()]);
        assert_eq!(*rig.shell.urls.lock().unwrap(), vec!["https://img.example/3/s".to_owned()]);
    });
}

#[test]
fn upload_no_url_skips_after_upload() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let mut settings = upload_settings(tmp.path());
        settings.defaults.after_upload = AfterUploadTasks::all();
        let rig = build(ClipboardBehaviour::Succeed, spawner.clone(), settings);
        let no_url = UploadResult { url: None, response: "accepted".into(), ..UploadResult::default() };
        destination(&rig, "imgur", vec![Ok(no_url)]);
        let mut events = rig.app.events();

        let trace = dispatch_and_trace(&rig, &spawner, &mut events, tmp.path(), Action::PrintScreen);

        assert_eq!(trace, include_str!("golden/upload_no_url_skips_after_upload.trace"));
        assert!(rig.clipboard.texts.lock().unwrap().is_empty());
        assert!(rig.shell.urls.lock().unwrap().is_empty());
    });
}

#[test]
fn large_file_declined() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let mut settings = upload_settings(tmp.path());
        settings.defaults.upload.large_file_warning_bytes = 1;
        let rig = build(ClipboardBehaviour::Succeed, spawner.clone(), settings);
        let imgur = destination(&rig, "imgur", Vec::new());
        rig.prompt.confirms.lock().unwrap().push_back(false);
        let mut events = rig.app.events();

        let trace = dispatch_and_trace(&rig, &spawner, &mut events, tmp.path(), Action::PrintScreen);

        assert_eq!(trace, include_str!("golden/large_file_declined.trace"));
        assert!(imgur.requests.lock().unwrap().is_empty(), "a declined upload sends nothing");
        assert!(saved_png(tmp.path()).is_file());
    });
}

#[test]
fn missing_destination() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let mut settings = upload_settings(tmp.path());
        settings.defaults.upload.image_destination = "nowhere".into();
        let rig = build(ClipboardBehaviour::Succeed, spawner.clone(), settings);
        let mut events = rig.app.events();

        let trace = dispatch_and_trace(&rig, &spawner, &mut events, tmp.path(), Action::PrintScreen);

        assert_eq!(trace, include_str!("golden/missing_destination.trace"));
    });
}

#[test]
fn text_job() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let rig = build(ClipboardBehaviour::Succeed, spawner.clone(), settings_in(tmp.path()));
        *rig.clipboard.contents.lock().unwrap() = Some(ClipboardContent::Text("hello".to_owned()));
        let paste = destination(&rig, "sxcu:paste_ee", vec![Ok(uploaded("https://paste.example/abc"))]);
        let mut events = rig.app.events();

        let trace = dispatch_and_trace(&rig, &spawner, &mut events, tmp.path(), Action::UploadText);

        assert_eq!(trace, include_str!("golden/text_job.trace"));
        let requests = paste.requests.lock().unwrap();
        assert_eq!(requests[0].file_name, "Laqta_2026-11-07_14-03-22.txt");
        assert_eq!(requests[0].kind, UploadKind::Text);
        assert_eq!(requests[0].mime, "text/plain");
        assert_eq!(requests[0].data, UploadData::Bytes(b"hello".to_vec()));
    });
}

#[test]
fn file_job() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let rig = build(ClipboardBehaviour::Succeed, spawner.clone(), settings_in(tmp.path()));
        let file = tmp.path().join("shot.png");
        std::fs::write(&file, [1u8, 2, 3]).unwrap();
        rig.prompt.texts.lock().unwrap().push_back(file.display().to_string());
        let imgur = destination(&rig, "imgur", vec![Ok(uploaded("https://img.example/4"))]);
        let mut events = rig.app.events();

        let trace = dispatch_and_trace(&rig, &spawner, &mut events, tmp.path(), Action::FileUpload);

        assert_eq!(trace, include_str!("golden/file_job.trace"));
        let requests = imgur.requests.lock().unwrap();
        assert_eq!(requests[0].file_name, "shot.png");
        assert_eq!(requests[0].kind, UploadKind::Image);
        assert_eq!(requests[0].mime, "image/png");
        assert_eq!(requests[0].data, UploadData::Path(file.clone()));
        assert!(file.is_file(), "an upload never deletes the user's file");
    });
}

#[test]
fn url_job() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let rig = build(ClipboardBehaviour::Succeed, spawner.clone(), settings_in(tmp.path()));
        *rig.uploaders.download.lock().unwrap() = Some((vec![1, 2, 3], "pic.jpg".to_owned(), "image/jpeg".to_owned()));
        rig.prompt.texts.lock().unwrap().push_back("https://example.com/pic.jpg".to_owned());
        let imgur = destination(&rig, "imgur", vec![Ok(uploaded("https://img.example/5"))]);
        let mut events = rig.app.events();

        let trace = dispatch_and_trace(&rig, &spawner, &mut events, tmp.path(), Action::UploadUrl);

        assert_eq!(trace, include_str!("golden/url_job.trace"));
        let requests = imgur.requests.lock().unwrap();
        assert_eq!(requests[0].file_name, "pic.jpg");
        assert_eq!(requests[0].mime, "image/jpeg");
        assert_eq!(requests[0].data, UploadData::Bytes(vec![1, 2, 3]));
    });
}

#[test]
fn shorten_url_job() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let mut settings = settings_in(tmp.path());
        settings.defaults.upload.url_shortener = "fake".into();
        let rig = build(ClipboardBehaviour::Succeed, spawner.clone(), settings);
        rig.uploaders.shorteners.lock().unwrap().push(Arc::new(FakeShortener));
        *rig.clipboard.contents.lock().unwrap() = Some(ClipboardContent::Text("https://example.com/long".to_owned()));
        let mut events = rig.app.events();

        let trace = dispatch_and_trace(&rig, &spawner, &mut events, tmp.path(), Action::ShortenUrl);

        assert_eq!(trace, include_str!("golden/shorten_url_job.trace"));
        // The default clipboard format is `$url`, the URL that was given, not the shortened one.
        assert_eq!(*rig.clipboard.texts.lock().unwrap(), vec!["https://example.com/long".to_owned()]);
    });
}
