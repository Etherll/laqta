#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! M3 dispatcher routes: the image editor, the effects window and drag-and-drop open their windows, and the image
//! beautifier picks one image and submits it as a capture. The jobs themselves are traced in `task_golden.rs`.

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use futures_lite::future::block_on;
use laqta_app::{App, Dispatched, Ports};
use laqta_domain::{Action, Bitmap, ImageFormat, PixelFormat, Settings};
use laqta_imaging::{Codecs, EncodeOptions};
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeEditorHost, FakeHoverSource, FakeNotifier, FakeOverlay, FakePrompt, FakeScreen,
    FakeShell, FakeToolHost, FakeUploaders, InlineSpawner, ManualClock,
};
use laqta_ports::{AppError, HostInfo, PortError, TaskId, ToolWindow};
use tempfile::TempDir;

struct H {
    app: Arc<App>,
    clipboard: Arc<FakeClipboard>,
    prompt: Arc<FakePrompt>,
    tools: Arc<FakeToolHost>,
    spawner: Arc<InlineSpawner>,
}

fn harness(dir: &Path) -> H {
    let clipboard = Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed));
    let prompt = Arc::new(FakePrompt::default());
    let tools = Arc::new(FakeToolHost::default());
    let spawner = Arc::new(InlineSpawner::default());
    let ports = Ports {
        screen: Arc::new(FakeScreen::new(Vec::new())),
        clipboard: clipboard.clone(),
        shell: Arc::new(FakeShell::default()),
        clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0))),
        spawner: spawner.clone(),
        overlay: Arc::new(FakeOverlay::default()),
        tools: tools.clone(),
        prompt: prompt.clone(),
        editor: Arc::new(FakeEditorHost::default()),
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
        hover: Arc::new(FakeHoverSource::new(Vec::new())),
        notifier: Arc::new(FakeNotifier::default()),
        rng: Arc::new(CountingRng::default()),
        host: HostInfo::default(),
        codecs: Codecs::builtin(),
    };
    let settings = Settings { screenshots_dir: dir.to_path_buf(), ..Settings::default_for(dir.to_path_buf()) };
    H { app: App::new(ports, settings), clipboard, prompt, tools, spawner }
}

/// Dispatches `action` and runs the jobs it submits. Imaging needs the compute pool, so the work runs in one.
fn run(h: &H, action: Action) -> Result<Dispatched, AppError> {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    pool.install(|| {
        let result = block_on(h.app.dispatch(action, None));
        loop {
            let jobs = h.spawner.take_spawned();
            if jobs.is_empty() {
                break;
            }
            for job in jobs {
                block_on(job);
            }
        }
        result
    })
}

#[test]
fn editor_effects_and_drag_drop_open_their_windows() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());

    for action in [Action::ImageEditor, Action::ImageEffects, Action::DragDropUpload] {
        assert_eq!(run(&h, action).unwrap(), Dispatched::Done, "{action:?}");
    }
    assert_eq!(*h.tools.opened.lock().unwrap(), vec![ToolWindow::Editor(None), ToolWindow::Effects(None), ToolWindow::Main]);
    assert!(h.spawner.spawned.lock().unwrap().is_empty(), "opening a window submits no job");
}

#[test]
fn image_beautifier_saves_and_copies_the_beautified_picture() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());
    let picture = tmp.path().join("picture.png");
    let image = Bitmap::new(4, 2, PixelFormat::Rgba8, vec![90; 4 * 2 * 4].into()).unwrap();
    // Encoding must run inside the compute pool (ADR 024), so the fixture is encoded there too.
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let bytes = pool.install(|| Codecs::builtin().encode(&image, &EncodeOptions { format: ImageFormat::Png, jpeg_quality: 90 }).unwrap());
    std::fs::write(&picture, bytes).unwrap();
    h.prompt.files.lock().unwrap().push_back(Ok(vec![picture]));

    assert_eq!(run(&h, Action::ImageBeautifier).unwrap(), Dispatched::Task(TaskId(1)));

    // The default padding is 64 on each side, and the job saves and copies the result.
    let saved = tmp.path().join("2026-11/Laqta_2026-11-07_14-03-22.png");
    let reader = png::Decoder::new(Cursor::new(std::fs::read(&saved).unwrap())).read_info().unwrap();
    assert_eq!((reader.info().width, reader.info().height), (4 + 2 * 64, 2 + 2 * 64));
    let copied = copied_image(&h);
    assert_eq!((copied.width(), copied.height()), (4 + 2 * 64, 2 + 2 * 64));
}

#[test]
fn image_beautifier_cancelled_in_the_picker_submits_nothing() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());
    h.prompt.files.lock().unwrap().push_back(Err(PortError::Cancelled));

    assert_eq!(run(&h, Action::ImageBeautifier).unwrap(), Dispatched::Done);
    assert!(h.spawner.spawned.lock().unwrap().is_empty());
    assert!(h.clipboard.images.lock().unwrap().is_empty());
}

/// The one image the job put on the clipboard.
fn copied_image(h: &H) -> Bitmap {
    let copied = h.clipboard.images.lock().unwrap();
    assert_eq!(copied.len(), 1, "one image was copied");
    copied[0].clone()
}
