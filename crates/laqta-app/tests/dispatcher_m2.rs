#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! M2 dispatcher routes: each upload action reads the clipboard, the prompt or the uploaders it needs, and
//! submits the jobs it names. The jobs themselves are traced in `task_golden.rs`.

use std::path::Path;
use std::sync::Arc;

use async_broadcast::Receiver;
use futures_lite::future::block_on;
use laqta_app::{App, Dispatched, Ports};
use laqta_domain::{Action, Bitmap, PixelFormat, Settings};
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeDestination, FakeHoverSource, FakeNotifier, FakeOverlay, FakePrompt, FakeScreen,
    FakeShell, FakeShortener, FakeToolHost, FakeUploaders, InlineSpawner, ManualClock,
};
use laqta_ports::{AppError, ClipboardContent, HostInfo, PortError, TaskEvent, TaskId, UploadData, UploadError, UploadKind, UploadResult};
use tempfile::TempDir;

struct H {
    app: Arc<App>,
    clipboard: Arc<FakeClipboard>,
    prompt: Arc<FakePrompt>,
    uploaders: Arc<FakeUploaders>,
    spawner: Arc<InlineSpawner>,
}

fn harness(dir: &Path) -> H {
    harness_with(dir, |_| {})
}

/// The harness, with `edit` applied to the settings first.
fn harness_with(dir: &Path, edit: impl FnOnce(&mut Settings)) -> H {
    let clipboard = Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed));
    let prompt = Arc::new(FakePrompt::default());
    let uploaders = Arc::new(FakeUploaders::default());
    let spawner = Arc::new(InlineSpawner::default());
    let ports = Ports {
        screen: Arc::new(FakeScreen::new(Vec::new())),
        clipboard: clipboard.clone(),
        shell: Arc::new(FakeShell::default()),
        clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0))),
        spawner: spawner.clone(),
        overlay: Arc::new(FakeOverlay::default()),
        tools: Arc::new(FakeToolHost::default()),
        prompt: prompt.clone(),
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
        uploaders: uploaders.clone(),
        hover: Arc::new(FakeHoverSource::new(Vec::new())),
        notifier: Arc::new(FakeNotifier::default()),
        rng: Arc::new(CountingRng::default()),
        host: HostInfo::default(),
        codecs: laqta_imaging::Codecs::builtin(),
    };
    let mut settings = Settings { screenshots_dir: dir.to_path_buf(), ..Settings::default_for(dir.to_path_buf()) };
    edit(&mut settings);
    H { app: App::new(ports, settings), clipboard, prompt, uploaders, spawner }
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

fn destination(h: &H, id: &str, results: Vec<Result<UploadResult, UploadError>>) -> Arc<FakeDestination> {
    let destination = Arc::new(FakeDestination::new(id, results));
    h.uploaders.destinations.lock().unwrap().push(destination.clone());
    destination
}

fn uploaded(url: &str) -> UploadResult {
    UploadResult { url: Some(url.to_owned()), ..UploadResult::default() }
}

fn drain(events: &mut Receiver<TaskEvent>) -> Vec<TaskEvent> {
    let mut out = Vec::new();
    while let Ok(event) = events.try_recv() {
        out.push(event);
    }
    out
}

#[test]
fn upload_text_sends_the_clipboard_text() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());
    *h.clipboard.contents.lock().unwrap() = Some(ClipboardContent::Text("hi".to_owned()));
    let paste = destination(&h, "sxcu:paste-ee", vec![Ok(uploaded("https://paste.example/1"))]);

    assert_eq!(run(&h, Action::UploadText).unwrap(), Dispatched::Task(TaskId(1)));
    let requests = paste.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].data, UploadData::Bytes(b"hi".to_vec()));
    assert!(h.prompt.shown.lock().unwrap().is_empty());
}

#[test]
fn upload_text_asks_when_the_clipboard_has_no_text() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());
    h.prompt.texts.lock().unwrap().push_back("typed".to_owned());
    let paste = destination(&h, "sxcu:paste-ee", vec![Ok(uploaded("https://paste.example/2"))]);

    assert_eq!(run(&h, Action::UploadText).unwrap(), Dispatched::Task(TaskId(1)));
    assert_eq!(paste.requests.lock().unwrap()[0].data, UploadData::Bytes(b"typed".to_vec()));
}

#[test]
fn upload_text_with_nothing_to_send_is_unsupported() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());

    let err = run(&h, Action::UploadText).unwrap_err();
    assert!(matches!(err, AppError::Unsupported(_)), "got {err:?}");
    assert!(h.spawner.spawned.lock().unwrap().is_empty());
}

#[test]
fn clipboard_upload_with_an_empty_clipboard_is_unsupported() {
    for action in [Action::ClipboardUpload, Action::ClipboardUploadWithContentViewer] {
        let tmp = TempDir::new().unwrap();
        let h = harness(tmp.path());
        let err = run(&h, action).unwrap_err();
        assert!(matches!(err, AppError::Unsupported(ref s) if s == "clipboard is empty"), "{action:?}: {err:?}");
    }
}

#[test]
fn clipboard_upload_of_text_is_a_text_upload() {
    for action in [Action::ClipboardUpload, Action::ClipboardUploadWithContentViewer] {
        let tmp = TempDir::new().unwrap();
        let h = harness(tmp.path());
        *h.clipboard.contents.lock().unwrap() = Some(ClipboardContent::Text("notes".to_owned()));
        let paste = destination(&h, "sxcu:paste-ee", vec![Ok(uploaded("https://paste.example/3"))]);

        assert_eq!(run(&h, action).unwrap(), Dispatched::Task(TaskId(1)));
        assert_eq!(paste.requests.lock().unwrap()[0].data, UploadData::Bytes(b"notes".to_vec()), "{action:?}");
    }
}

#[test]
fn clipboard_upload_of_a_url_downloads_and_uploads_it() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());
    *h.clipboard.contents.lock().unwrap() = Some(ClipboardContent::Text("https://example.com/a.txt".to_owned()));
    *h.uploaders.download.lock().unwrap() = Some((b"body".to_vec(), "a.txt".to_owned(), "text/plain".to_owned()));
    let text = destination(&h, "sxcu:paste-ee", vec![Ok(uploaded("https://paste.example/4"))]);

    assert_eq!(run(&h, Action::ClipboardUpload).unwrap(), Dispatched::Task(TaskId(1)));
    let requests = text.requests.lock().unwrap();
    assert_eq!(requests[0].file_name, "a.txt");
    assert_eq!(requests[0].kind, UploadKind::Text);
    assert_eq!(requests[0].data, UploadData::Bytes(b"body".to_vec()));
}

#[test]
fn clipboard_upload_of_files_submits_one_job_per_file() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());
    let first = tmp.path().join("one.zip");
    let second = tmp.path().join("two.zip");
    std::fs::write(&first, b"1").unwrap();
    std::fs::write(&second, b"22").unwrap();
    *h.clipboard.contents.lock().unwrap() = Some(ClipboardContent::Files(vec![first.clone(), second.clone()]));
    let uguu = destination(&h, "sxcu:uguu", vec![Ok(uploaded("https://u.example/1")), Ok(uploaded("https://u.example/2"))]);

    assert_eq!(run(&h, Action::ClipboardUpload).unwrap(), Dispatched::Task(TaskId(1)));
    let requests = uguu.requests.lock().unwrap();
    assert_eq!(requests.iter().map(|r| r.file_name.as_str()).collect::<Vec<_>>(), vec!["one.zip", "two.zip"]);
    assert_eq!(requests[1].data, UploadData::Path(second));
}

#[test]
fn clipboard_upload_of_an_image_is_a_capture_with_the_upload_flag() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());
    let image = Bitmap::new(4, 3, PixelFormat::Rgba8, vec![90; 4 * 3 * 4].into()).unwrap();
    *h.clipboard.contents.lock().unwrap() = Some(ClipboardContent::Image(image));
    let imgur = destination(&h, "imgur", vec![Ok(uploaded("https://img.example/9"))]);

    assert_eq!(run(&h, Action::ClipboardUpload).unwrap(), Dispatched::Task(TaskId(1)));
    let requests = imgur.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].kind, UploadKind::Image);
    assert_eq!(requests[0].file_name, "Laqta_2026-11-07_14-03-22.png");
    assert!(tmp.path().join("2026-11/Laqta_2026-11-07_14-03-22.png").is_file(), "the default flags still save the image");
}

#[test]
fn upload_url_asks_for_the_url_and_uploads_the_download() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());
    h.prompt.texts.lock().unwrap().push_back("https://example.com/a.jpg".to_owned());
    *h.uploaders.download.lock().unwrap() = Some((vec![7, 7], "a.jpg".to_owned(), String::new()));
    let imgur = destination(&h, "imgur", vec![Ok(uploaded("https://img.example/6"))]);

    assert_eq!(run(&h, Action::UploadUrl).unwrap(), Dispatched::Task(TaskId(1)));
    let requests = imgur.requests.lock().unwrap();
    assert_eq!(requests[0].file_name, "a.jpg");
    assert_eq!(requests[0].mime, "image/jpeg", "an empty download type is worked out from the name");
}

#[test]
fn upload_url_rejects_text_that_is_not_a_url() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());
    h.prompt.texts.lock().unwrap().push_back("not a url".to_owned());

    let err = run(&h, Action::UploadUrl).unwrap_err();
    assert!(matches!(err, AppError::Unsupported(_)), "got {err:?}");
    assert!(h.spawner.spawned.lock().unwrap().is_empty());
}

#[test]
fn shorten_url_uses_the_clipboard_url_when_there_is_one() {
    let tmp = TempDir::new().unwrap();
    let h = harness_with(tmp.path(), |settings| settings.defaults.upload.url_shortener = "fake".into());
    h.uploaders.shorteners.lock().unwrap().push(Arc::new(FakeShortener));
    *h.clipboard.contents.lock().unwrap() = Some(ClipboardContent::Text("https://example.com/long".to_owned()));
    let mut events = h.app.events();

    assert_eq!(run(&h, Action::ShortenUrl).unwrap(), Dispatched::Task(TaskId(1)));
    assert!(h.prompt.shown.lock().unwrap().is_empty());
    assert!(drain(&mut events).iter().any(|e| matches!(e, TaskEvent::Shortened { url, .. } if url == "https://example.com/long/s")));
}

#[test]
fn shorten_url_asks_when_the_clipboard_holds_no_url() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());
    *h.clipboard.contents.lock().unwrap() = Some(ClipboardContent::Text("just words".to_owned()));
    h.prompt.texts.lock().unwrap().push_back("https://example.com/asked".to_owned());
    h.uploaders.shorteners.lock().unwrap().push(Arc::new(FakeShortener));

    assert_eq!(run(&h, Action::ShortenUrl).unwrap(), Dispatched::Task(TaskId(1)));
    assert!(h.prompt.texts.lock().unwrap().is_empty(), "the prompt was asked");
}

#[test]
fn file_upload_asks_for_a_path_and_uploads_that_file() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());
    let file = tmp.path().join("archive.zip");
    std::fs::write(&file, b"zip").unwrap();
    h.prompt.texts.lock().unwrap().push_back(file.display().to_string());
    let uguu = destination(&h, "sxcu:uguu", vec![Ok(uploaded("https://u.example/3"))]);

    assert_eq!(run(&h, Action::FileUpload).unwrap(), Dispatched::Task(TaskId(1)));
    let requests = uguu.requests.lock().unwrap();
    assert_eq!(requests[0].file_name, "archive.zip");
    assert_eq!(requests[0].kind, UploadKind::File);
    assert_eq!(requests[0].data, UploadData::Path(file));
}

#[test]
fn file_upload_with_no_path_is_unsupported() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());

    let err = run(&h, Action::FileUpload).unwrap_err();
    assert!(matches!(err, AppError::Unsupported(_)), "got {err:?}");
    assert!(h.spawner.spawned.lock().unwrap().is_empty());
}

#[test]
fn file_upload_uploads_each_picked_file() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());
    let first = tmp.path().join("one.zip");
    let second = tmp.path().join("two.zip");
    std::fs::write(&first, b"1").unwrap();
    std::fs::write(&second, b"22").unwrap();
    h.prompt.files.lock().unwrap().push_back(Ok(vec![first.clone(), second.clone()]));
    let uguu = destination(&h, "sxcu:uguu", vec![Ok(uploaded("https://u.example/7")), Ok(uploaded("https://u.example/8"))]);

    assert_eq!(run(&h, Action::FileUpload).unwrap(), Dispatched::Task(TaskId(1)));
    assert!(h.prompt.texts.lock().unwrap().is_empty(), "the file picker answered, so no path was typed");
    let requests = uguu.requests.lock().unwrap();
    assert_eq!(requests.iter().map(|r| r.file_name.as_str()).collect::<Vec<_>>(), vec!["one.zip", "two.zip"]);
    assert_eq!(requests[1].data, UploadData::Path(second));
}

#[test]
fn file_upload_cancelled_in_the_picker_submits_nothing() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());
    h.prompt.files.lock().unwrap().push_back(Err(PortError::Cancelled));

    assert_eq!(run(&h, Action::FileUpload).unwrap(), Dispatched::Done);
    assert!(h.spawner.spawned.lock().unwrap().is_empty());
}

#[test]
fn folder_upload_submits_one_file_job_per_file_in_the_folder() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());
    let folder = tmp.path().join("pics");
    std::fs::create_dir_all(folder.join("nested")).unwrap();
    std::fs::write(folder.join("b.zip"), b"b").unwrap();
    std::fs::write(folder.join("a.zip"), b"a").unwrap();
    std::fs::write(folder.join("nested").join("deep.zip"), b"d").unwrap();
    h.prompt.folders.lock().unwrap().push_back(Ok(folder));
    let uguu = destination(&h, "sxcu:uguu", vec![Ok(uploaded("https://u.example/9")), Ok(uploaded("https://u.example/10"))]);

    assert_eq!(run(&h, Action::FolderUpload).unwrap(), Dispatched::Task(TaskId(1)));
    // Non-recursive and sorted: the subfolder's file is not uploaded, and the two others go in name order.
    let requests = uguu.requests.lock().unwrap();
    assert_eq!(requests.iter().map(|r| r.file_name.as_str()).collect::<Vec<_>>(), vec!["a.zip", "b.zip"]);
}

#[test]
fn folder_upload_of_an_empty_folder_is_unsupported() {
    let tmp = TempDir::new().unwrap();
    let h = harness(tmp.path());
    let folder = tmp.path().join("empty");
    std::fs::create_dir_all(&folder).unwrap();
    h.prompt.folders.lock().unwrap().push_back(Ok(folder));

    let err = run(&h, Action::FolderUpload).unwrap_err();
    assert!(matches!(err, AppError::Unsupported(ref s) if s == "the folder has no files"), "got {err:?}");
    assert!(h.spawner.spawned.lock().unwrap().is_empty());
}
