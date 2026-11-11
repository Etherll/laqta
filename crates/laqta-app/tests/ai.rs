#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! The AI use cases on `App` (text recognition, background removal, object erase, analysis) and the OCR route's
//! result window, over the fakes.

use std::sync::{Arc, Mutex};

use futures_lite::future::{block_on, poll_once};
use laqta_app::{App, Dispatched, Ports};
use laqta_domain::region::{Button, HoverTarget, KeyMods, RegionCommand, RegionInput, RegionKey, TextCommand};
use laqta_domain::{Action, Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat, Settings};
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeAnalyzer, FakeBackgroundRemover, FakeClipboard, FakeHoverSource, FakeNotifier, FakeOcr,
    FakeOverlay, FakeScreen, FakeShell, FakeToolHost, InlineSpawner, ManualClock,
};
use laqta_ports::{
    AppError, BackgroundRemover, BoxFuture, HostInfo, ImageAnalyzer, Inpainter, Ocr, OcrLine, OcrResult, PortError, TextLayer, ToolHost,
    ToolWindow,
};

/// Fails every call with `error`.
struct Failing(PortError);

impl Ocr for Failing {
    fn recognize(&self, _image: Bitmap, _language: String) -> BoxFuture<'_, Result<OcrResult, PortError>> {
        let e = self.0.clone();
        Box::pin(async move { Err(e) })
    }
    fn languages(&self) -> Vec<String> {
        Vec::new()
    }
}

impl BackgroundRemover for Failing {
    fn remove(&self, _image: Bitmap) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        let e = self.0.clone();
        Box::pin(async move { Err(e) })
    }
}

impl ImageAnalyzer for Failing {
    fn analyze(&self, _image: Bitmap, _prompt: String) -> BoxFuture<'_, Result<String, PortError>> {
        let e = self.0.clone();
        Box::pin(async move { Err(e) })
    }
}

impl Inpainter for Failing {
    fn inpaint(&self, _image: Bitmap, _mask: Vec<u8>) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        let e = self.0.clone();
        Box::pin(async move { Err(e) })
    }
}

/// The image size and the mask an inpainter was given.
type InpaintCall = ((u32, u32), Vec<u8>);

/// Records the mask and the image size it was given and paints the whole image white.
#[derive(Default)]
struct SpyInpainter {
    calls: Mutex<Vec<InpaintCall>>,
}

impl Inpainter for SpyInpainter {
    fn inpaint(&self, image: Bitmap, mask: Vec<u8>) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        self.calls.lock().unwrap().push(((image.width(), image.height()), mask));
        Box::pin(async move {
            Bitmap::new(image.width(), image.height(), image.format(), vec![255; image.pixels().len()].into())
                .map_err(|e| PortError::Failed(e.to_string()))
        })
    }
}

struct H {
    app: Arc<App>,
    overlay: Arc<FakeOverlay>,
    tools: Arc<FakeToolHost>,
    clipboard: Arc<FakeClipboard>,
    spawner: Arc<InlineSpawner>,
    ocr: Arc<FakeOcr>,
    background: Arc<FakeBackgroundRemover>,
    analyzer: Arc<FakeAnalyzer>,
    inpainter: Arc<SpyInpainter>,
}

fn frame() -> Frame {
    Frame {
        monitor: Monitor {
            id: MonitorId("m0".into()),
            name: "M0".into(),
            rect: PhysRect { x: 0, y: 0, width: 8, height: 4 },
            scale: 1.0,
            primary: true,
        },
        image: Bitmap::new(8, 4, PixelFormat::Bgra8, vec![10; 8 * 4 * 4].into()).unwrap(),
    }
}

fn harness_with(failing: Option<Arc<Failing>>) -> H {
    let overlay = Arc::new(FakeOverlay::default());
    let tools = Arc::new(FakeToolHost::default());
    let clipboard = Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed));
    let spawner = Arc::new(InlineSpawner::default());
    let ocr = Arc::new(FakeOcr::default());
    let background = Arc::new(FakeBackgroundRemover::default());
    let analyzer = Arc::new(FakeAnalyzer::default());
    let inpainter = Arc::new(SpyInpainter::default());
    let ports = Ports {
        screen: Arc::new(FakeScreen::new(vec![frame()])),
        clipboard: clipboard.clone(),
        shell: Arc::new(FakeShell::default()),
        clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 9).at(10, 0, 0, 0))),
        spawner: spawner.clone(),
        overlay: overlay.clone(),
        tools: tools.clone(),
        prompt: Arc::new(laqta_ports::fakes::FakePrompt::default()),
        editor: Arc::new(laqta_ports::fakes::FakeEditorHost::default()),
        media: Arc::new(laqta_ports::fakes::FakeMedia::default()),
        stream: Arc::new(laqta_ports::fakes::FakeScreenStream::default()),
        ocr: failing.clone().map_or_else(|| ocr.clone() as Arc<dyn Ocr>, |f| f as Arc<dyn Ocr>),
        window_ops: Arc::new(laqta_ports::fakes::FakeWindowOps::default()),
        input: Arc::new(laqta_ports::fakes::FakeInputSynth::default()),
        highlighter: Arc::new(laqta_ports::fakes::FakeHighlighter::default()),
        background: failing.clone().map_or_else(|| background.clone() as Arc<dyn BackgroundRemover>, |f| f as Arc<dyn BackgroundRemover>),
        analyzer: failing.clone().map_or_else(|| analyzer.clone() as Arc<dyn ImageAnalyzer>, |f| f as Arc<dyn ImageAnalyzer>),
        inpainter: failing.map_or_else(|| inpainter.clone() as Arc<dyn Inpainter>, |f| f as Arc<dyn Inpainter>),
        watcher: Arc::new(laqta_ports::fakes::FakeFileWatcher::default()),
        uploaders: Arc::new(laqta_ports::fakes::FakeUploaders::default()),
        hover: Arc::new(FakeHoverSource::new(Vec::<HoverTarget>::new())),
        notifier: Arc::new(FakeNotifier::default()),
        rng: Arc::new(CountingRng::default()),
        host: HostInfo::default(),
        codecs: laqta_imaging::Codecs::builtin(),
    };
    let dir = std::env::temp_dir();
    let app = App::new(ports, Settings::default_for(dir));
    H { app, overlay, tools, clipboard, spawner, ocr, background, analyzer, inpainter }
}

fn harness() -> H {
    harness_with(None)
}

/// A 2x2 RGBA image: the red channel of the pixels is 0, 200, 50 and 255.
fn image() -> Bitmap {
    let px = [0, 1, 2, 255, 200, 1, 2, 255, 50, 1, 2, 255, 255, 1, 2, 255];
    Bitmap::new(2, 2, PixelFormat::Rgba8, px.to_vec().into()).unwrap()
}

fn line(text: &str) -> OcrLine {
    OcrLine { rect: PhysRect { x: 0, y: 0, width: 8, height: 4 }, text: text.into(), confidence: 0.9, words: Vec::new(), vertical: false }
}

fn assert_nothing_else_happened(h: &H) {
    assert!(h.tools.opened.lock().unwrap().is_empty());
    assert!(h.tools.texts.lock().unwrap().is_empty());
    assert!(h.tools.ocr.lock().unwrap().is_empty());
    assert!(h.clipboard.texts.lock().unwrap().is_empty());
    assert!(h.spawner.spawned.lock().unwrap().is_empty());
    assert!(h.overlay.shown.lock().unwrap().is_none());
}

#[test]
fn recognize_text_returns_the_ocr_result_and_does_nothing_else() {
    let h = harness();
    let mut events = h.app.events();
    h.ocr.result.lock().unwrap().lines.extend([line("hello"), line("world")]);

    let result = block_on(h.app.recognize_text(image())).unwrap();

    assert_eq!(result.text(), "hello\nworld");
    assert_nothing_else_happened(&h);
    assert!(events.try_recv().is_err(), "no task event");
}

#[test]
fn remove_background_returns_the_cut_image() {
    let h = harness();
    let mut events = h.app.events();

    let cut = block_on(h.app.remove_background(image())).unwrap();

    // The fake clears the alpha of every pixel whose red channel is below 128.
    let alpha: Vec<u8> = cut.pixels().chunks(4).map(|p| p[3]).collect();
    assert_eq!(alpha, [0, 255, 0, 255]);
    assert_eq!(*h.background.calls.lock().unwrap(), 1);
    assert_nothing_else_happened(&h);
    assert!(events.try_recv().is_err(), "no task event");
}

#[test]
fn erase_passes_the_image_and_mask_to_the_inpainter() {
    let h = harness();
    let mask = vec![0, 255, 0, 0];

    let filled = block_on(h.app.erase(image(), mask.clone())).unwrap();

    assert_eq!(h.inpainter.calls.lock().unwrap().as_slice(), &[((2, 2), mask)]);
    assert!(filled.pixels().iter().all(|&b| b == 255), "the inpainter's bitmap comes back");
    assert_nothing_else_happened(&h);
}

#[test]
fn analyze_passes_the_prompt_and_returns_the_answer() {
    let h = harness();
    *h.analyzer.answer.lock().unwrap() = "a red square".into();

    let answer = block_on(h.app.analyze(image(), "What is this?".into())).unwrap();

    assert_eq!(answer, "a red square");
    assert_eq!(h.analyzer.prompts.lock().unwrap().as_slice(), &["What is this?".to_owned()]);
    assert_nothing_else_happened(&h);
}

#[test]
fn a_use_case_calls_its_port_only_when_polled() {
    let h = harness();

    let fut = h.app.remove_background(image());
    assert_eq!(*h.background.calls.lock().unwrap(), 0, "building the future does no work");
    block_on(fut).unwrap();
    assert_eq!(*h.background.calls.lock().unwrap(), 1);
}

#[test]
fn a_use_case_future_owns_what_it_needs() {
    let h = harness();
    *h.analyzer.answer.lock().unwrap() = "ok".into();
    let analyzer = h.analyzer.clone();
    let fut: BoxFuture<'static, Result<String, AppError>> = h.app.analyze(image(), "p".into());
    drop(h);

    // A window moves the future to its own executor thread.
    let answer = std::thread::spawn(move || block_on(fut)).join().unwrap().unwrap();

    assert_eq!(answer, "ok");
    assert_eq!(analyzer.prompts.lock().unwrap().as_slice(), &["p".to_owned()]);
}

/// Whether an `AppError` is the one a `PortError` should become.
type Expected = fn(&AppError) -> bool;

#[test]
fn port_errors_become_app_errors() {
    let cases: [(PortError, Expected); 4] = [
        (PortError::Unsupported("no model".into()), |e| matches!(e, AppError::Unsupported(s) if s == "no model")),
        (PortError::Failed("boom".into()), |e| matches!(e, AppError::Io(s) if s == "boom")),
        (PortError::Cancelled, |e| matches!(e, AppError::Cancelled)),
        (PortError::PermissionDenied, |e| matches!(e, AppError::PermissionDenied)),
    ];
    for (port_error, is_expected) in cases {
        let h = harness_with(Some(Arc::new(Failing(port_error.clone()))));
        let results = [
            block_on(h.app.recognize_text(image())).err(),
            block_on(h.app.remove_background(image())).err(),
            block_on(h.app.erase(image(), vec![0; 4])).err(),
            block_on(h.app.analyze(image(), String::new())).err(),
        ];
        for (i, error) in results.iter().enumerate() {
            let error = error.as_ref().unwrap_or_else(|| panic!("use case {i} did not fail for {port_error:?}"));
            assert!(is_expected(error), "use case {i}: {port_error:?} became {error:?}");
        }
    }
}

/// Dispatches the OCR action and drags over the whole desktop. The text is read in place: the overlay stays
/// open and shows it. `last` is then sent, which ends the session. Returns what the overlay showed before
/// `last`, and how the route ended.
fn read_in_place(h: &H, last: RegionInput) -> (Option<TextLayer>, Result<Dispatched, AppError>) {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    pool.install(|| {
        block_on(async {
            let mut fut = h.app.dispatch(Action::Ocr, None);
            while !h.overlay.is_open() {
                assert!(poll_once(&mut fut).await.is_none());
            }
            h.overlay.send(RegionInput::PointerDown { x: 0, y: 0, button: Button::Left });
            h.overlay.send(RegionInput::PointerMove { x: 7, y: 3 });
            h.overlay.send(RegionInput::PointerUp { x: 7, y: 3, button: Button::Left });
            assert!(poll_once(&mut fut).await.is_none(), "reading the text does not end the route");
            assert!(h.overlay.is_open(), "the overlay stays up with the text on it");
            let shown = h.overlay.last_view().and_then(|view| view.text);
            h.overlay.send(last);
            (shown, fut.await)
        })
    })
}

fn escape() -> RegionInput {
    RegionInput::Key { key: RegionKey::Escape, mods: KeyMods::default() }
}

/// Enter right after the drag, before the overlay shows the text, confirms the area: the route still
/// reads it, copies the text and opens the text window, as a capture that picks the area first does.
#[test]
fn the_ocr_route_reads_an_area_confirmed_before_its_text_shows() {
    let h = harness();
    h.ocr.result.lock().unwrap().lines.extend([line("hello"), line("world")]);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let ended = pool.install(|| {
        block_on(async {
            let mut fut = h.app.dispatch(Action::Ocr, None);
            while !h.overlay.is_open() {
                assert!(poll_once(&mut fut).await.is_none());
            }
            h.overlay.send(RegionInput::PointerDown { x: 0, y: 0, button: Button::Left });
            h.overlay.send(RegionInput::PointerMove { x: 7, y: 3 });
            h.overlay.send(RegionInput::PointerUp { x: 7, y: 3, button: Button::Left });
            h.overlay.send(RegionInput::Key { key: RegionKey::Enter, mods: KeyMods::default() });
            fut.await
        })
    });
    assert!(matches!(ended, Ok(Dispatched::Done)), "{ended:?}");
    assert!(!h.overlay.is_open());
    assert_eq!(h.clipboard.texts.lock().unwrap().last().map(String::as_str), Some("hello\nworld"));
    assert_eq!(h.tools.ocr.lock().unwrap().len(), 1, "the text window shows what was read");
}

#[test]
fn the_ocr_route_reads_the_text_in_place_and_copies_it() {
    let h = harness();
    h.ocr.result.lock().unwrap().lines.extend([line("hello"), line("world")]);

    let (shown, ended) = read_in_place(&h, escape());

    let Some(TextLayer::Read { result, copied, .. }) = shown else { panic!("the overlay shows what was read, not {shown:?}") };
    assert_eq!(result.text(), "hello\nworld");
    assert!(copied);
    assert_eq!(h.ocr.seen.lock().unwrap().as_slice(), &[(8, 4)], "the selection was read");
    assert_eq!(h.clipboard.texts.lock().unwrap().as_slice(), &["hello\nworld".to_owned()]);
    assert!(matches!(ended, Err(AppError::Cancelled)), "Escape closes the overlay");
    assert!(h.tools.ocr.lock().unwrap().is_empty(), "no window opens");
    assert!(h.tools.texts.lock().unwrap().is_empty());
}

#[test]
fn the_ocr_route_copies_the_words_the_user_picked() {
    let h = harness();
    h.ocr.result.lock().unwrap().lines.extend([line("hello"), line("world")]);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    pool.install(|| {
        block_on(async {
            let mut fut = h.app.dispatch(Action::Ocr, None);
            while !h.overlay.is_open() {
                assert!(poll_once(&mut fut).await.is_none());
            }
            h.overlay.send(RegionInput::PointerDown { x: 0, y: 0, button: Button::Left });
            h.overlay.send(RegionInput::PointerUp { x: 7, y: 3, button: Button::Left });
            // The text is read and shown before the user can pick anything from it.
            assert!(poll_once(&mut fut).await.is_none());
            h.overlay.send(RegionInput::Text(TextCommand::Copy("world".to_owned())));
            h.overlay.send(RegionInput::Key { key: RegionKey::Enter, mods: KeyMods::default() });
            assert_eq!(fut.await.unwrap(), Dispatched::Done, "Enter ends a text capture");
        })
    });
    assert_eq!(h.clipboard.texts.lock().unwrap().as_slice(), &["hello\nworld".to_owned(), "world".to_owned()]);
}

#[test]
fn the_ocr_route_opens_the_text_window_when_asked() {
    let h = harness();
    h.ocr.result.lock().unwrap().lines.extend([line("hello"), line("world")]);

    let (_, ended) = read_in_place(&h, RegionInput::Text(TextCommand::OpenWindow));

    assert!(matches!(ended, Err(AppError::Cancelled)));
    let shown = h.tools.ocr.lock().unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].0, (8, 4), "the window gets the selected image");
    assert_eq!(shown[0].1.text(), "hello\nworld");
}

#[test]
fn the_ocr_route_shows_a_failed_recognition_on_the_overlay() {
    let h = harness_with(Some(Arc::new(Failing(PortError::Failed("no model".into())))));

    let (shown, ended) = read_in_place(&h, escape());

    assert!(matches!(shown, Some(TextLayer::Failed(message)) if message.contains("no model")));
    assert!(matches!(ended, Err(AppError::Cancelled)));
    assert!(h.tools.ocr.lock().unwrap().is_empty());
    assert!(h.clipboard.texts.lock().unwrap().is_empty());
}

/// Dispatches the OCR action with a recogniser that is not ready (it would first ask about its models, under
/// the overlay) and answers its region session with a full-desktop selection.
fn run_ocr_route(h: &H) -> Result<Dispatched, AppError> {
    h.ocr.not_ready.store(true, std::sync::atomic::Ordering::SeqCst);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    pool.install(|| {
        block_on(async {
            let mut fut = h.app.dispatch(Action::Ocr, None);
            while !h.overlay.is_open() {
                assert!(poll_once(&mut fut).await.is_none());
            }
            h.overlay.send(RegionInput::Command(RegionCommand::Fullscreen));
            fut.await
        })
    })
}

#[test]
fn the_ocr_route_of_a_recogniser_that_is_not_ready_shows_the_result_window_and_copies_the_text() {
    let h = harness();
    h.ocr.result.lock().unwrap().lines.extend([line("hello"), line("world")]);

    assert_eq!(run_ocr_route(&h).unwrap(), Dispatched::Done);

    let shown = h.tools.ocr.lock().unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].0, (8, 4), "the window gets the selected image");
    assert_eq!(shown[0].1.text(), "hello\nworld");
    assert!(h.tools.texts.lock().unwrap().is_empty(), "the result goes to show_ocr, not show_text");
    assert_eq!(h.clipboard.texts.lock().unwrap().as_slice(), &["hello\nworld".to_owned()]);
}

#[test]
fn the_ocr_route_of_a_recogniser_that_is_not_ready_shows_an_empty_result_without_touching_the_clipboard() {
    let h = harness();

    assert_eq!(run_ocr_route(&h).unwrap(), Dispatched::Done);

    let shown = h.tools.ocr.lock().unwrap();
    assert_eq!(shown.len(), 1);
    assert!(shown[0].1.lines.is_empty());
    assert!(h.clipboard.texts.lock().unwrap().is_empty());
}

#[test]
fn show_ocr_on_a_host_without_text_windows_is_unsupported() {
    // No result window and no text window: show_ocr falls back to show_text, which is unsupported.
    struct Bare;
    impl ToolHost for Bare {
        fn open(&self, _window: ToolWindow) -> Result<(), PortError> {
            Ok(())
        }
    }
    let error = Bare.show_ocr(image(), OcrResult::default()).unwrap_err();
    assert!(matches!(error, PortError::Unsupported(_)));
}

#[test]
fn show_ocr_defaults_to_the_text_window() {
    #[derive(Default)]
    struct TextOnly {
        texts: Mutex<Vec<(String, String)>>,
    }
    impl ToolHost for TextOnly {
        fn open(&self, _window: ToolWindow) -> Result<(), PortError> {
            Ok(())
        }
        fn show_text(&self, title: &str, text: &str) -> Result<(), PortError> {
            self.texts.lock().unwrap().push((title.to_owned(), text.to_owned()));
            Ok(())
        }
    }
    let host = TextOnly::default();

    host.show_ocr(image(), OcrResult { lines: vec![line("hello"), line("world")] }).unwrap();
    host.show_ocr(image(), OcrResult::default()).unwrap();

    assert_eq!(
        host.texts.lock().unwrap().as_slice(),
        &[("Recognised text".to_owned(), "hello\nworld".to_owned()), ("Recognised text".to_owned(), "No text found.".to_owned())]
    );
}

/// A 6x3 PNG file in a fresh folder.
fn png_file() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot.png");
    let image = Bitmap::new(6, 3, PixelFormat::Rgba8, vec![200; 6 * 3 * 4].into()).unwrap();
    // Encoding must run inside the compute pool (ADR 024), so the fixture is encoded there too.
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    std::fs::write(&path, pool.install(|| laqta_imaging::encode_png(&image)).unwrap()).unwrap();
    (dir, path)
}

fn dispatch_file(h: &H, action: Action, path: &std::path::Path) -> Result<Dispatched, AppError> {
    let input = laqta_app::ActionInput::Files(vec![path.to_path_buf()]);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    pool.install(|| block_on(h.app.dispatch_with(action, input, None)))
}

/// `-OCR shot.png` reads the file: no area is picked on screen.
#[test]
fn the_ocr_route_reads_a_given_file() {
    let h = harness();
    h.ocr.result.lock().unwrap().lines.push(line("from the file"));
    let (_dir, path) = png_file();

    assert_eq!(dispatch_file(&h, Action::Ocr, &path).unwrap(), Dispatched::Done);

    assert!(h.overlay.shown.lock().unwrap().is_none(), "no overlay");
    assert_eq!(h.clipboard.texts.lock().unwrap().as_slice(), &["from the file".to_owned()]);
    assert_eq!(h.tools.ocr.lock().unwrap()[0].0, (6, 3));
}

/// `-PinToScreen shot.png` and `-PinToScreenFromFile shot.png` pin the file.
#[test]
fn pin_routes_pin_a_given_file() {
    let h = harness();
    let (_dir, path) = png_file();

    assert_eq!(dispatch_file(&h, Action::PinToScreen, &path).unwrap(), Dispatched::Done);
    assert_eq!(dispatch_file(&h, Action::PinToScreenFromFile, &path).unwrap(), Dispatched::Done);

    assert!(h.overlay.shown.lock().unwrap().is_none(), "no area is picked");
    assert_eq!(h.tools.pins.lock().unwrap().as_slice(), &[(6, 3), (6, 3)]);
}
