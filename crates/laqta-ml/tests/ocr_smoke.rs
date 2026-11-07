//! Inference smoke tests for the PP-OCR family. They run only when ONNX Runtime and the OCR models are
//! available on this machine:
//!
//! - `LAQTA_ORT_LIB`: the path to the ONNX Runtime shared library (1.23 or newer).
//! - `LAQTA_OCR_MODELS`: a directory holding `<id>.onnx` for the default detector and recogniser,
//!   and `<id>.dict.txt` for the recogniser (the file names `Runtime::model_path` and `dict_path`
//!   use).
//!
//! Without them each test logs why it skipped and returns. The golden test (a fixture image with its
//! expected text) is still to come: it needs a fixture image that the owner supplies.

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
    let det = catalog.default_for(Task::OcrDet).expect("a default detector");
    let rec = catalog.default_for(Task::OcrRec).expect("a default recogniser");
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
