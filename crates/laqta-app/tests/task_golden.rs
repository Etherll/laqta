#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Golden event traces for the Capture job (M0 §7.1). Each test renders the `TaskEvent`s as lines
//! and compares them with `tests/golden/<name>.trace`.

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use async_broadcast::Receiver;
use futures_lite::future::{block_on, poll_once, yield_now};
use laqta_app::{App, Dispatched, Ports};
use laqta_domain::{Action, Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat, Settings};
use laqta_ports::fakes::{ClipboardBehaviour, FakeClipboard, FakeScreen, FakeShell, InlineSpawner, ManualClock};
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

fn build_app(dir: &Path, behaviour: ClipboardBehaviour, spawner: Arc<dyn Spawner>) -> (Arc<App>, Arc<FakeClipboard>) {
    let clipboard = Arc::new(FakeClipboard::new(behaviour));
    let ports = Ports {
        screen: Arc::new(FakeScreen::new(two_frames())),
        clipboard: clipboard.clone(),
        shell: Arc::new(FakeShell::default()),
        clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0))),
        spawner,
    };
    let settings = Settings { screenshots_dir: dir.to_path_buf(), ..Settings::default_for(dir.to_path_buf()) };
    (App::new(ports, settings), clipboard)
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
        TaskEvent::Copied { id } => (id.0, "copied".to_owned()),
        TaskEvent::CopyFailed { id, .. } => (id.0, "copy_failed".to_owned()),
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

fn has_part_file(dir: &Path) -> bool {
    std::fs::read_dir(dir).unwrap().any(|entry| {
        let path = entry.unwrap().path();
        if path.is_dir() {
            has_part_file(&path)
        } else {
            path.extension().is_some_and(|ext| ext == "part")
        }
    })
}

#[test]
fn capture_ok() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let (app, clipboard) = build_app(tmp.path(), ClipboardBehaviour::Succeed, spawner.clone());
        let mut events = app.events();

        assert_eq!(block_on(app.dispatch(Action::PrintScreen, None)).unwrap(), Dispatched::Task(TaskId(1)));
        drive(&spawner);

        assert_eq!(render(&mut events, tmp.path(), false), include_str!("golden/capture_ok.trace"));
        let saved = tmp.path().join("2026-11/Laqta_2026-11-07_14-03-22.png");
        assert_eq!(decoded_size(&saved), (8, 4));
        let copied = clipboard.images.lock().unwrap();
        assert_eq!((copied[0].width(), copied[0].height()), (8, 4));
    });
}

#[test]
fn clipboard_fails() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let (app, _clipboard) = build_app(tmp.path(), ClipboardBehaviour::Fail, spawner.clone());
        let mut events = app.events();

        block_on(app.dispatch(Action::PrintScreen, None)).unwrap();
        drive(&spawner);

        assert_eq!(render(&mut events, tmp.path(), false), include_str!("golden/clipboard_fails.trace"));
        assert!(tmp.path().join("2026-11/Laqta_2026-11-07_14-03-22.png").is_file());
    });
}

#[test]
fn save_fails() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        // The screenshots folder is a regular file, so nothing can be written under it.
        let shots = tmp.path().join("shots");
        std::fs::write(&shots, b"not a folder").unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let (app, _clipboard) = build_app(&shots, ClipboardBehaviour::Succeed, spawner.clone());
        let mut events = app.events();

        block_on(app.dispatch(Action::PrintScreen, None)).unwrap();
        drive(&spawner);

        assert_eq!(render(&mut events, &shots, false), include_str!("golden/save_fails.trace"));
        assert!(!has_part_file(tmp.path()));
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
        let spawner = Arc::new(InlineSpawner::default());
        let (app, _clipboard) = build_app(tmp.path(), ClipboardBehaviour::Succeed, spawner.clone());
        let mut events = app.events();

        block_on(app.dispatch(Action::PrintScreen, None)).unwrap();
        drive(&spawner);

        assert_eq!(render(&mut events, tmp.path(), false), include_str!("golden/name_collision.trace"));
        assert_eq!(std::fs::read(&existing).unwrap(), b"existing");
        assert_eq!(decoded_size(&folder.join("Laqta_2026-11-07_14-03-22_2.png")), (8, 4));
    });
}

#[test]
fn panic_isolated() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let spawner = Arc::new(InlineSpawner::default());
        let (app, clipboard) = build_app(tmp.path(), ClipboardBehaviour::Panic, spawner.clone());
        let mut events = app.events();

        assert_eq!(block_on(app.dispatch(Action::PrintScreen, None)).unwrap(), Dispatched::Task(TaskId(1)));
        drive(&spawner);

        *clipboard.behaviour.lock().unwrap() = ClipboardBehaviour::Succeed;
        assert_eq!(block_on(app.dispatch(Action::PrintScreen, None)).unwrap(), Dispatched::Task(TaskId(2)));
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
        let (app, _clipboard) = build_app(tmp.path(), ClipboardBehaviour::Succeed, spawner.clone());
        let mut events = app.events();

        for expected in 1..=3 {
            assert_eq!(block_on(app.dispatch(Action::PrintScreen, None)).unwrap(), Dispatched::Task(TaskId(expected)));
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
