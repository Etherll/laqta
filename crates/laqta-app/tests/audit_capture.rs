#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Capture-area checks for after-capture ordering, clipboard and print behaviour. Each `audit_bug_*`
//! test states the expected behaviour and fails until the defect is fixed.

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use futures_lite::future::block_on;
use laqta_app::{App, Ports};
use laqta_domain::region::{HoverTarget, TargetKind};
use laqta_domain::workflow::AfterCaptureTasks;
use laqta_domain::{Action, Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat, Settings};
use laqta_ports::TaskEvent;
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeEditorHost, FakeHoverSource, FakeNotifier, FakeOverlay, FakePrompt, FakeScreen,
    FakeShell, FakeUploaders, InlineSpawner, ManualClock,
};
use tempfile::TempDir;

fn in_compute_pool<R: Send>(body: impl FnOnce() -> R + Send) -> R {
    rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap().install(body)
}

fn frame(x: i32) -> Frame {
    let pixels: Vec<u8> = std::iter::repeat_n([10u8, 20, 30, 255], 16).flatten().collect();
    Frame {
        monitor: Monitor {
            id: MonitorId(format!("m{x}")),
            name: format!("M{x}"),
            rect: PhysRect { x, y: 0, width: 4, height: 4 },
            scale: 1.0,
            primary: x == 0,
        },
        image: Bitmap::new(4, 4, PixelFormat::Bgra8, pixels.into()).unwrap(),
    }
}

struct Rig {
    app: Arc<App>,
    clipboard: Arc<FakeClipboard>,
    shell: Arc<FakeShell>,
    editor: Arc<FakeEditorHost>,
    spawner: Arc<InlineSpawner>,
}

fn rig(dir: &Path, flags: AfterCaptureTasks, tweak: impl FnOnce(&mut Settings)) -> Rig {
    rig_with(dir, flags, Vec::new(), tweak)
}

fn rig_with(dir: &Path, flags: AfterCaptureTasks, targets: Vec<HoverTarget>, tweak: impl FnOnce(&mut Settings)) -> Rig {
    let clipboard = Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed));
    let shell = Arc::new(FakeShell::default());
    let editor = Arc::new(FakeEditorHost::default());
    let spawner = Arc::new(InlineSpawner::default());
    let ports = Ports {
        screen: Arc::new(FakeScreen::new(vec![frame(0), frame(4)])),
        clipboard: clipboard.clone(),
        shell: shell.clone(),
        clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0))),
        spawner: spawner.clone(),
        overlay: Arc::new(FakeOverlay::default()),
        tools: Arc::new(laqta_ports::fakes::FakeToolHost::default()),
        prompt: Arc::new(FakePrompt::default()),
        editor: editor.clone(),
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
        uploaders: Arc::new(FakeUploaders::default()),
        hover: Arc::new(FakeHoverSource::new(targets)),
        notifier: Arc::new(FakeNotifier::default()),
        rng: Arc::new(CountingRng::default()),
        host: laqta_ports::HostInfo::default(),
        codecs: laqta_imaging::Codecs::builtin(),
    };
    let mut settings = Settings { screenshots_dir: dir.to_path_buf(), ..Settings::default_for(dir.to_path_buf()) };
    settings.defaults.after_capture = flags;
    settings.defaults.general.show_toast = false;
    tweak(&mut settings);
    Rig { app: App::new(ports, settings), clipboard, shell, editor, spawner }
}

fn run(rig: &Rig) -> Vec<TaskEvent> {
    run_action(rig, Action::PrintScreen)
}

fn run_action(rig: &Rig, action: Action) -> Vec<TaskEvent> {
    let mut events = rig.app.events();
    block_on(rig.app.dispatch(action, None)).unwrap();
    loop {
        let jobs = rig.spawner.take_spawned();
        if jobs.is_empty() {
            break;
        }
        for job in jobs {
            block_on(job);
        }
    }
    let mut out = Vec::new();
    while let Ok(event) = events.try_recv() {
        out.push(event);
    }
    out
}

fn saved_size(dir: &Path) -> (u32, u32) {
    let bytes = std::fs::read(dir.join("2026-11/Laqta_2026-11-07_14-03-22.png")).unwrap();
    let reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
    (reader.info().width, reader.info().height)
}

/// The expected order is Beautify, then Effects, then Annotate, so what the user draws in the editor
/// is the final image. Laqta runs Annotate first and beautifies the edited image afterwards
/// (`task/run.rs` `edit`).
#[test]
#[ignore = "known issue, not fixed yet"]
fn audit_bug_beautify_runs_after_annotate() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let flags = AfterCaptureTasks::BEAUTIFY_IMAGE | AfterCaptureTasks::ANNOTATE_IMAGE | AfterCaptureTasks::SAVE_IMAGE_TO_FILE;
        let r = rig(tmp.path(), flags, |s| s.defaults.beautify.padding = 10);
        let edited = Bitmap::new(6, 2, PixelFormat::Rgba8, vec![1u8; 6 * 2 * 4].into()).unwrap();
        *r.editor.result.lock().unwrap() = Some(Ok(edited));
        run(&r);
        assert_eq!(saved_size(tmp.path()), (6, 2), "ShareX: the editor's result is the last image step, nothing pads it afterwards");
    });
}

/// The expected behaviour: copying the file, its path and its folder path is an if / else-if chain,
/// so with file and path both set only the file is copied.
/// Laqta runs all of them and the path text overwrites the copied file.
#[test]
#[ignore = "known issue, not fixed yet"]
fn audit_bug_copy_file_and_path_are_exclusive() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let flags = AfterCaptureTasks::SAVE_IMAGE_TO_FILE
            | AfterCaptureTasks::COPY_FILE_TO_CLIPBOARD
            | AfterCaptureTasks::COPY_FILE_PATH_TO_CLIPBOARD;
        let r = rig(tmp.path(), flags, |_| {});
        run(&r);
        assert_eq!(r.clipboard.files.lock().unwrap().len(), 1);
        assert!(r.clipboard.texts.lock().unwrap().is_empty(), "ShareX leaves the file on the clipboard; Laqta overwrote it with the path");
    });
}

/// The expected behaviour: printing uses the in-memory image, with no file needed. Laqta's print step
/// needs a saved file and fails without SAVE_IMAGE_TO_FILE.
#[test]
#[ignore = "known issue, not fixed yet"]
fn audit_bug_print_needs_no_saved_file() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let r = rig(tmp.path(), AfterCaptureTasks::SEND_IMAGE_TO_PRINTER, |_| {});
        let events = run(&r);
        let failed = events.iter().any(|e| matches!(e, TaskEvent::StepFailed { step: "print", .. }));
        assert!(!failed && !r.shell.printed.lock().unwrap().is_empty(), "print-only workflow printed nothing: {events:?}");
    });
}

/// The expected behaviour: a custom window capture finds a top-level window by its title.
/// Laqta takes the first hover target whose title contains the text, and the Linux/Windows hover
/// sources list UI elements (TargetKind::Element) before windows, so an element named like the
/// window wins and only that element is captured.
#[test]
#[ignore = "known issue, not fixed yet"]
fn audit_bug_custom_window_matches_a_ui_element() {
    in_compute_pool(|| {
        let tmp = TempDir::new().unwrap();
        let element = HoverTarget {
            rect: PhysRect { x: 1, y: 1, width: 2, height: 1 },
            handle: 0,
            title: "Notes".into(),
            process: "notes".into(),
            kind: TargetKind::Element,
        };
        let window = HoverTarget {
            rect: PhysRect { x: 0, y: 0, width: 6, height: 4 },
            handle: 7,
            title: "Notes - Editor".into(),
            process: "notes".into(),
            kind: TargetKind::Window,
        };
        let r = rig_with(tmp.path(), AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD, vec![element, window], |s| {
            s.defaults.capture.custom_window_title = "notes".into();
        });
        run_action(&r, Action::CustomWindow);
        let copied = r.clipboard.images.lock().unwrap();
        assert_eq!((copied[0].width(), copied[0].height()), (6, 4), "captured the element, not the window");
    });
}
