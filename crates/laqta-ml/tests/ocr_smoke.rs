//! Inference smoke tests for the PP-OCR family. They run only when ONNX Runtime and the OCR models are
//! available on this machine:
//!
//! - `LAQTA_ORT_LIB`: the path to the ONNX Runtime shared library (1.23 or newer).
//! - `LAQTA_OCR_MODELS`: a directory holding `<id>.onnx` for the default detector and recogniser,
//!   and `<id>.dict.txt` for the recogniser (the file names `Runtime::model_path` and `dict_path`
//!   use).
//!
//! - `LAQTA_OCR_PAIR` (optional): `<det id>,<rec id>` to test a pair other than the defaults, for
//!   example `ppocr-v5-mobile-det,ppocr-v5-mobile-rec-multi`.
//!
//! Without the first two each test logs why it skipped and returns. The golden test reads
//! `fixtures/laqta-home.png`, a screenshot of Laqta's home screen, and checks the lines it must find.

use std::future::Future;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use laqta_domain::{Bitmap, PixelFormat};
use laqta_ml::{ExecutionProvider, ModelOcr, Runtime, Task, catalog, validate_custom};
use laqta_ports::Ocr;

fn block_on<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The runtime and OCR pair for the default models, or `None` after logging a skip.
fn setup(test: &str) -> Option<(Arc<Runtime>, ModelOcr, PathBuf, PathBuf)> {
    let (Some(lib), Some(models)) = (std::env::var_os("LAQTA_ORT_LIB"), std::env::var_os("LAQTA_OCR_MODELS")) else {
        eprintln!("skipped {test}: set LAQTA_ORT_LIB and LAQTA_OCR_MODELS to run it");
        return None;
    };
    let catalog = catalog();
    let pair = std::env::var("LAQTA_OCR_PAIR").ok();
    let (det, rec) = match pair.as_deref().and_then(|p| p.split_once(',')) {
        Some((det, rec)) => (catalog.model(det).expect("a known detector id"), catalog.model(rec).expect("a known recogniser id")),
        None => (
            catalog.default_for(Task::OcrDet).expect("a default detector"),
            catalog.default_for(Task::OcrRec).expect("a default recogniser"),
        ),
    };
    let runtime = Arc::new(Runtime::new(lib, models, ExecutionProvider::Cpu));
    runtime.ensure_ort().expect("ONNX Runtime loads from LAQTA_ORT_LIB");
    let det_path = runtime.model_path(&det.id);
    let rec_path = runtime.model_path(&rec.id);
    let ocr = ModelOcr::new(runtime.clone(), det.id.clone(), rec.id.clone());
    Some((runtime, ocr, det_path, rec_path))
}

fn rgba_image(width: u32, height: u32, fill: impl Fn(u32, u32) -> u8) -> Bitmap {
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let value = fill(x, y);
            pixels.extend_from_slice(&[value, value, value, 255]);
        }
    }
    Bitmap::new(width, height, PixelFormat::Rgba8, pixels.into()).expect("valid bitmap")
}

#[test]
fn the_default_models_match_their_families() {
    let Some((_runtime, _ocr, det_path, rec_path)) = setup("the_default_models_match_their_families") else {
        return;
    };
    validate_custom(&det_path, "o1-det").expect("the detector has the o1-det signature");
    validate_custom(&rec_path, "o1-rec").expect("the recogniser has the o1-rec signature");
    assert!(validate_custom(&det_path, "u2net").is_err(), "an unknown family is refused");
}

#[test]
fn a_blank_image_has_no_text() {
    let Some((_runtime, ocr, _, _)) = setup("a_blank_image_has_no_text") else {
        return;
    };
    let bitmap = rgba_image(320, 120, |_, _| 255);
    let result = block_on(ocr.recognize(bitmap, String::new())).expect("OCR runs on a blank image");
    assert!(result.lines.is_empty(), "a blank image has no text, got {:?}", result.lines);
}

#[test]
fn a_dark_block_runs_detection_and_recognition() {
    let Some((_runtime, ocr, _, _)) = setup("a_dark_block_runs_detection_and_recognition") else {
        return;
    };
    // Glyph-like bars on white, in a single line. The detector may or may not call it text; the point
    // is that the recogniser runs on any region it does find, without an error.
    let bitmap = rgba_image(320, 120, |x, y| {
        let bar = (x / 4) % 2 == 0 && (4..20).contains(&(y % 40));
        if (20..300).contains(&x) && (40..60).contains(&y) && bar { 0 } else { 255 }
    });
    let result = block_on(ocr.recognize(bitmap, String::new())).expect("OCR runs on a dark block");
    eprintln!("dark block: {} line(s): {:?}", result.lines.len(), result.lines);
}

/// The lines a correct reading of `fixtures/laqta-home.png` contains, in reading order. They cover
/// a heading, a long sentence (which a squashed recogniser input garbles), a path, small card text
/// and key names.
const HOME_LINES: [&str; 12] = [
    "New capture",
    "Capture",
    "Grab a region, a window or the whole screen. Every capture lands in History.",
    "Save to ~/Pictures/Laqta",
    "Drag to select an area",
    "Every monitor at once",
    "Repeat the previous area",
    "Ctrl+Shift+PrtSc",
    "A long page, stitched",
    "Pick an area, then record",
    "Text capture (OCR)",
    "or drop files anywhere here",
];

#[test]
fn a_screenshot_reads_its_text() {
    let Some((_runtime, ocr, _, _)) = setup("a_screenshot_reads_its_text") else {
        return;
    };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/laqta-home.png");
    let bytes = std::fs::read(&path).expect("the fixture is readable");
    let bitmap = laqta_imaging::Codecs::builtin().decode(&bytes).expect("the fixture decodes");
    let started = std::time::Instant::now();
    let result = block_on(ocr.recognize(bitmap.clone(), String::new())).expect("OCR runs on the fixture");
    let first = started.elapsed();
    // The second run reuses the loaded models, as the app does after its first OCR.
    let started = std::time::Instant::now();
    block_on(ocr.recognize(bitmap, String::new())).expect("OCR runs again");
    let elapsed = started.elapsed();
    eprintln!("first run (loads the models) {first:?}, warm run {elapsed:?}");
    let text: Vec<String> = result.lines.iter().map(|line| line.text.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    for line in &result.lines {
        eprintln!("{:>5.2} {:?}", line.confidence, line.text);
    }
    let joined = text.join("\n");
    let missing: Vec<&str> = HOME_LINES.iter().copied().filter(|want| !joined.contains(want)).collect();
    eprintln!(
        "{} lines in {elapsed:?}; {} of {} expected lines found",
        result.lines.len(),
        HOME_LINES.len() - missing.len(),
        HOME_LINES.len()
    );
    assert!(missing.is_empty(), "not read: {missing:#?}");
}
