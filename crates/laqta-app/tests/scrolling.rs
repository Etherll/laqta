#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Scrolling capture: the region is captured, scrolled and stitched until the page stops moving.
//! The page is a synthetic tall image; the screen shows a window of it that moves down with each capture.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_lite::future::{block_on, poll_once};
use laqta_app::{App, Dispatched, Ports};
use laqta_domain::capture::ScrollMethod;
use laqta_domain::region::{Button, KeyMods, RegionInput, RegionKey};
use laqta_domain::{Action, Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat, Settings};
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeHoverSource, FakeInputSynth, FakeNotifier, FakeOverlay, FakeShell, FakeToolHost,
    InlineSpawner, ManualClock,
};
use laqta_ports::{AppError, BoxFuture, Clock, HostInfo, InputSynth, PortError, ScreenSource};
use tempfile::TempDir;

/// The scrolled region: 10 x 20 pixels at (2, 3).
const REGION: PhysRect = PhysRect { x: 2, y: 3, width: 10, height: 20 };
/// The desktop is one 16 x 40 monitor at the origin, so the region sits inside it.
const DESKTOP_WIDTH: u32 = 16;
const DESKTOP_HEIGHT: u32 = 40;
/// The notice the user sees when scrolling falls to them.
const NOTICE: &str = "Scroll the page yourself; capture stops when it stops moving";

/// A tall page of `rows` rows, seen through `REGION`. Capture `n` (0-based) shows the page `offset(n)` rows down.
/// Captures 0 and 1 (the region selection and frame 1) are at the top, and each later capture is one scroll
/// further. The page moves with the captures, whichever way the scroll was sent.
#[derive(Debug)]
struct Page {
    rows: u32,
    step: u32,
    captures: Mutex<u32>,
}

impl Page {
    fn new(rows: u32, step: u32) -> Arc<Self> {
        Arc::new(Page { rows, step, captures: Mutex::new(0) })
    }

    fn offset(&self, capture: u32) -> u32 {
        capture.saturating_sub(1).saturating_mul(self.step).min(self.rows - REGION.height)
    }
}

impl ScreenSource for Page {
    fn capture_all(&self) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
        let n = {
            let mut count = self.captures.lock().unwrap();
            *count += 1;
            *count - 1
        };
        let frame = desktop(self.offset(n));
        Box::pin(async move { Ok(vec![frame]) })
    }
}

/// The page's pixel at column `x`, row `y`. Every row has its own colour, so row matches are exact.
fn page_pixel(x: u32, y: u32) -> [u8; 4] {
    [y as u8, (y >> 8) as u8, x as u8, 255]
}

/// The RGBA rows `from..to` of the page, `REGION.width` wide.
fn page_rows(from: u32, to: u32) -> Vec<u8> {
    (from..to).flat_map(|y| (0..REGION.width).flat_map(move |x| page_pixel(x, y))).collect()
}

/// The monitor frame with the page scrolled to `offset`. Inside the region it shows the page; outside, black.
fn desktop(offset: u32) -> Frame {
    let mut pixels = Vec::with_capacity((DESKTOP_WIDTH * DESKTOP_HEIGHT * 4) as usize);
    for yd in 0..DESKTOP_HEIGHT as i32 {
        for xd in 0..DESKTOP_WIDTH as i32 {
            let inside = (REGION.x..REGION.right()).contains(&xd) && (REGION.y..REGION.bottom()).contains(&yd);
            let px = if inside { page_pixel((xd - REGION.x) as u32, offset + (yd - REGION.y) as u32) } else { [0, 0, 0, 255] };
            pixels.extend_from_slice(&px);
        }
    }
    Frame {
        monitor: Monitor {
            id: MonitorId("m0".into()),
            name: "M0".into(),
            rect: PhysRect { x: 0, y: 0, width: DESKTOP_WIDTH, height: DESKTOP_HEIGHT },
            scale: 1.0,
            primary: true,
        },
        image: Bitmap::new(DESKTOP_WIDTH, DESKTOP_HEIGHT, PixelFormat::Rgba8, pixels.into()).unwrap(),
    }
}

/// A window that takes no injected input (Wayland): every scroll is refused.
#[derive(Debug, Default)]
struct NoInput {
    calls: Mutex<u32>,
}

impl InputSynth for NoInput {
    fn scroll(&self, _x: i32, _y: i32, _method: ScrollMethod, _amount: u32) -> Result<(), PortError> {
        *self.calls.lock().unwrap() += 1;
        Err(PortError::Unsupported("scroll input".into()))
    }
}

struct H {
    app: Arc<App>,
    overlay: Arc<FakeOverlay>,
    spawner: Arc<InlineSpawner>,
    notifier: Arc<FakeNotifier>,
    clock: Arc<ManualClock>,
}

fn harness(dir: &Path, page: Arc<Page>, input: Arc<dyn InputSynth>, edit: impl FnOnce(&mut Settings)) -> H {
    let overlay = Arc::new(FakeOverlay::default());
    let spawner = Arc::new(InlineSpawner::default());
    let notifier = Arc::new(FakeNotifier::default());
    let clock = Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0)));
    let ports = Ports {
        screen: page,
        clipboard: Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed)),
        shell: Arc::new(FakeShell::default()),
        clock: clock.clone(),
        spawner: spawner.clone(),
        overlay: overlay.clone(),
        tools: Arc::new(FakeToolHost::default()),
        prompt: Arc::new(laqta_ports::fakes::FakePrompt::default()),
        editor: Arc::new(laqta_ports::fakes::FakeEditorHost::default()),
        media: Arc::new(laqta_ports::fakes::FakeMedia::default()),
        stream: Arc::new(laqta_ports::fakes::FakeScreenStream::default()),
        ocr: Arc::new(laqta_ports::fakes::FakeOcr::default()),
        window_ops: Arc::new(laqta_ports::fakes::FakeWindowOps::default()),
        input,
        highlighter: Arc::new(laqta_ports::fakes::FakeHighlighter::default()),
        background: Arc::new(laqta_ports::fakes::FakeBackgroundRemover::default()),
        analyzer: Arc::new(laqta_ports::fakes::FakeAnalyzer::default()),
        inpainter: Arc::new(laqta_ports::fakes::FakeInpainter),
        watcher: Arc::new(laqta_ports::fakes::FakeFileWatcher::default()),
        uploaders: Arc::new(laqta_ports::fakes::FakeUploaders::default()),
        hover: Arc::new(FakeHoverSource::new(Vec::new())),
        notifier: notifier.clone(),
        rng: Arc::new(CountingRng::default()),
        host: HostInfo::default(),
        codecs: laqta_imaging::Codecs::builtin(),
    };
    let mut settings = Settings::default_for(dir.to_path_buf());
    settings.screenshots_dir = dir.to_path_buf();
    // Releasing the drag selects the region at once; there is no capture toolbar to confirm it.
    settings.defaults.capture.region.show_tools_after_select = false;
    edit(&mut settings);
    H { app: App::new(ports, settings), overlay, spawner, notifier, clock }
}

/// Drags out `REGION` (inclusive points, so 10 x 20) and confirms it with the release.
fn drag_region() -> Vec<RegionInput> {
    vec![
        RegionInput::PointerMove { x: 2, y: 3 },
        RegionInput::PointerDown { x: 2, y: 3, button: Button::Left },
        RegionInput::PointerMove { x: 11, y: 22 },
        RegionInput::PointerUp { x: 11, y: 22, button: Button::Left },
    ]
}

/// Runs a scrolling capture: the overlay gets `input`, then the capture and its task run to the end.
fn scroll(h: &H, input: Vec<RegionInput>) -> Result<Dispatched, AppError> {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    pool.install(|| {
        block_on(async {
            let mut fut = h.app.dispatch(Action::ScrollingCapture, None);
            while !h.overlay.is_open() {
                assert!(poll_once(&mut fut).await.is_none());
            }
            for i in input {
                h.overlay.send(i);
            }
            let out = fut.await;
            loop {
                let jobs = h.spawner.take_spawned();
                if jobs.is_empty() {
                    break;
                }
                for job in jobs {
                    job.await;
                }
            }
            out
        })
    })
}

/// The one saved capture under `dir`, as (width, height, RGBA pixels).
fn saved(dir: &Path) -> (u32, u32, Vec<u8>) {
    let month = dir.join("2026-11");
    let file = std::fs::read_dir(&month).unwrap().next().unwrap().unwrap().path();
    let mut reader = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(file).unwrap())).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba);
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

#[test]
fn scrolling_stitches_the_page_until_it_stops_moving() {
    let tmp = TempDir::new().unwrap();
    let input = Arc::new(FakeInputSynth::default());
    let h = harness(tmp.path(), Page::new(60, 5), input.clone(), |_| {});
    let start = h.clock.now();

    assert!(matches!(scroll(&h, drag_region()), Ok(Dispatched::Task(_))));

    let (width, height, pixels) = saved(tmp.path());
    assert_eq!((width, height), (REGION.width, 60));
    assert!(pixels == page_rows(0, 60), "stitched pixels differ from the page");
    // Eight scrolls move the page to its end, and two more find it unchanged.
    let calls = input.calls.lock().unwrap();
    assert_eq!(calls.len(), 10);
    assert!(calls.iter().all(|c| *c == (7, 13, ScrollMethod::MouseWheel, 2)), "{calls:?}");
    // The start delay and each scroll's delay ran on the clock: 300 + 10 * 300 ms.
    assert_eq!(h.clock.now(), start.checked_add(Duration::from_millis(3300)).unwrap());
}

#[test]
fn scrolling_max_frames_caps_the_capture() {
    let tmp = TempDir::new().unwrap();
    let input = Arc::new(FakeInputSynth::default());
    let h = harness(tmp.path(), Page::new(60, 5), input.clone(), |s| s.defaults.capture.scrolling.max_frames = 4);

    scroll(&h, drag_region()).unwrap();

    // Four frames in all: the first and three scrolled ones.
    let (_, height, pixels) = saved(tmp.path());
    assert_eq!(height, 35);
    assert!(pixels == page_rows(0, 35), "stitched pixels differ from the page");
    assert_eq!(input.calls.lock().unwrap().len(), 3);
}

#[test]
fn scrolling_a_page_that_stays_put_stops_after_two_unchanged_frames() {
    let tmp = TempDir::new().unwrap();
    let input = Arc::new(FakeInputSynth::default());
    let h = harness(tmp.path(), Page::new(20, 5), input.clone(), |_| {});

    scroll(&h, drag_region()).unwrap();

    let (_, height, pixels) = saved(tmp.path());
    assert_eq!(height, 20);
    assert!(pixels == page_rows(0, 20), "stitched pixels differ from the page");
    assert_eq!(input.calls.lock().unwrap().len(), 2);
}

#[test]
fn scrolling_a_scroll_past_the_overlap_stops_and_keeps_the_seam() {
    // A step of 25 rows is more than the region's 20, so the second frame shares no rows with the first.
    let tmp = TempDir::new().unwrap();
    let input = Arc::new(FakeInputSynth::default());
    let h = harness(tmp.path(), Page::new(60, 25), input.clone(), |_| {});

    scroll(&h, drag_region()).unwrap();

    let (_, height, pixels) = saved(tmp.path());
    assert_eq!(height, 40);
    assert!(pixels == [page_rows(0, 20), page_rows(25, 45)].concat(), "stitched pixels differ from the page");
    assert_eq!(input.calls.lock().unwrap().len(), 1);
}

#[test]
fn scrolling_a_window_that_refuses_scroll_input_is_captured_by_hand() {
    let tmp = TempDir::new().unwrap();
    let input = Arc::new(NoInput::default());
    let h = harness(tmp.path(), Page::new(60, 5), input.clone(), |_| {});

    scroll(&h, drag_region()).unwrap();

    // The page moves with the user's own scrolling, and the capture stops once it has been still for six frames.
    let (_, height, pixels) = saved(tmp.path());
    assert_eq!(height, 60);
    assert!(pixels == page_rows(0, 60), "stitched pixels differ from the page");
    // Scrolling is tried once, then the capture runs by hand and the notice is shown once.
    assert_eq!(*input.calls.lock().unwrap(), 1);
    let notices = h.notifier.toasts.lock().unwrap().iter().filter(|t| t.body == NOTICE).count();
    assert_eq!(notices, 1);
}

#[test]
fn scrolling_cancelling_the_selection_captures_nothing() {
    let tmp = TempDir::new().unwrap();
    let input = Arc::new(FakeInputSynth::default());
    let h = harness(tmp.path(), Page::new(60, 5), input.clone(), |_| {});

    let r = scroll(&h, vec![RegionInput::Key { key: RegionKey::Escape, mods: KeyMods::default() }]);

    assert!(matches!(r, Err(AppError::Cancelled)));
    assert!(input.calls.lock().unwrap().is_empty());
    assert!(h.notifier.toasts.lock().unwrap().is_empty());
    assert!(!tmp.path().join("2026-11").exists());
}
