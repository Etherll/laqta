//! Known OCR and model catalog defects. A test named `audit_bug_*` fails
//! until the defect it shows is fixed.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use laqta_domain::{Bitmap, PixelFormat};
use laqta_ml::{ExecutionProvider, ModelOcr, Runtime, catalog};
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

fn white(width: u32, height: u32) -> Bitmap {
    Bitmap::new(width, height, PixelFormat::Rgba8, vec![255u8; (width * height * 4) as usize].into()).expect("bitmap")
}

/// No shipped catalog entry uses the "pending-owner" placeholder, so every model can be downloaded.
#[test]
fn audit_catalog_has_no_pending_hashes() {
    for model in &catalog().models {
        assert!(!model.is_pending(), "{} is pending", model.id);
    }
    for archive in &catalog().onnxruntime {
        assert_eq!(archive.sha256.len(), 64, "{} {}", archive.os, archive.arch);
    }
}

/// A custom recogniser added through Settings → Models gets an id like `custom-o1-rec-<name>` and is
/// stored as `<models>/<id>.onnx` (+ `<id>.dict.txt`). Choosing it ("Use") sets `ocr_rec` to that id,
/// and `LazyOcr` builds `ModelOcr::new(runtime, det, <custom id>)`. `ModelOcr::prepare` looks every id
/// up in the catalog. A custom recogniser is not a catalog entry, so it is accepted once its files are installed.
#[test]
fn audit_bug_custom_ocr_recogniser_is_refused_as_unknown() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("ppocr-v5-mobile-det.onnx"), b"placeholder").expect("det");
    std::fs::write(dir.path().join("custom-o1-rec-mine.onnx"), b"placeholder").expect("rec");
    std::fs::write(dir.path().join("custom-o1-rec-mine.dict.txt"), "a\nb\n").expect("dict");
    let runtime = Arc::new(Runtime::new("/nonexistent/libonnxruntime.so", dir.path(), ExecutionProvider::Cpu));
    let ocr = ModelOcr::new(runtime, "ppocr-v5-mobile-det", "custom-o1-rec-mine");
    let error = block_on(ocr.recognize(white(64, 32), String::new())).expect_err("no ONNX Runtime here");
    let message = error.to_string();
    assert!(!message.contains("unknown model"), "the installed custom recogniser was refused: {message}");
}

/// End-to-end OCR on rendered text. Runs only with `LAQTA_ORT_LIB`, `LAQTA_OCR_MODELS` and
/// `LAQTA_AUDIT_TEXT_RGBA` (a raw 640×120 RGBA image of "Hello Laqta 2026" / "Screenshot tool").
#[test]
fn audit_ocr_reads_rendered_text() {
    let (Some(lib), Some(models), Some(raw)) =
        (std::env::var_os("LAQTA_ORT_LIB"), std::env::var_os("LAQTA_OCR_MODELS"), std::env::var_os("LAQTA_AUDIT_TEXT_RGBA"))
    else {
        eprintln!("skipped audit_ocr_reads_rendered_text: needs LAQTA_ORT_LIB, LAQTA_OCR_MODELS, LAQTA_AUDIT_TEXT_RGBA");
        return;
    };
    let pixels = std::fs::read(raw).expect("rgba file");
    let image = Bitmap::new(640, 120, PixelFormat::Rgba8, pixels.into()).expect("640x120 rgba");
    let det = catalog().default_for(laqta_ml::Task::OcrDet).expect("det").id.clone();
    let rec = catalog().default_for(laqta_ml::Task::OcrRec).expect("rec").id.clone();
    let runtime = Arc::new(Runtime::new(lib, models, ExecutionProvider::Cpu));
    let ocr = ModelOcr::new(runtime, det, rec);
    let result = block_on(ocr.recognize(image, String::new())).expect("ocr runs");
    let text = result.text();
    eprintln!("recognised: {text:?}");
    assert!(text.contains("Hello"), "{text:?}");
    assert!(text.contains("Screenshot"), "{text:?}");
}
