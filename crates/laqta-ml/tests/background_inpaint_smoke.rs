//! Inference smoke tests for the background family (`matting-parametric`) and the inpainting family
//! (`lama`). They run only when ONNX Runtime and the model are on this machine:
//!
//! - `LAQTA_ORT_LIB`: the path to the ONNX Runtime shared library (1.23 or newer);
//! - `LAQTA_ML_MODELS`: a directory holding `<id>.onnx` for the model under test, under the file names
//!   `Runtime::model_path` uses (`birefnet-lite.onnx`, `big-lama.onnx`).
//!
//! Without them, or without the model file, each test logs why it skipped and returns. The assertions
//! on the model's output are expectations, not golden values: they have not been run against the
//! owner's copies of the models yet.

use std::future::Future;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use laqta_domain::{Bitmap, PixelFormat};
use laqta_ml::{ExecutionProvider, ModelBackground, ModelInpainter, Runtime, validate_custom};
use laqta_ports::{BackgroundRemover, Inpainter};

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

/// The runtime for the models folder and the path of model `id`, or `None` after logging a skip.
fn setup(test: &str, id: &str) -> Option<(Arc<Runtime>, PathBuf)> {
    let (Some(lib), Some(models)) = (std::env::var_os("LAQTA_ORT_LIB"), std::env::var_os("LAQTA_ML_MODELS")) else {
        eprintln!("skipped {test}: set LAQTA_ORT_LIB and LAQTA_ML_MODELS to run it");
        return None;
    };
    let runtime = Arc::new(Runtime::new(lib, models, ExecutionProvider::Cpu));
    let path = runtime.model_path(id);
    if !path.is_file() {
        eprintln!("skipped {test}: {} is not in LAQTA_ML_MODELS", path.display());
        return None;
    }
    runtime.ensure_ort().expect("ONNX Runtime loads from LAQTA_ORT_LIB");
    Some((runtime, path))
}

/// An opaque RGBA bitmap whose colour at `(x, y)` is `fill(x, y)` in all three colour channels.
fn bitmap(width: u32, height: u32, fill: impl Fn(u32, u32) -> u8) -> Bitmap {
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
fn birefnet_lite_keeps_a_bright_disc_opaque_and_the_corner_clear() {
    let test = "birefnet_lite_keeps_a_bright_disc_opaque_and_the_corner_clear";
    let Some((runtime, path)) = setup(test, "birefnet-lite") else {
        return;
    };
    validate_custom(&path, "matting-parametric").expect("the model has the matting-parametric signature");
    let size = 256;
    let disc = bitmap(size, size, |x, y| {
        let (dx, dy) = (i64::from(x) - 128, i64::from(y) - 128);
        if dx * dx + dy * dy < 60 * 60 { 255 } else { 0 }
    });
    let output = block_on(ModelBackground::new(runtime, "birefnet-lite").remove(disc)).expect("the background model runs");
    assert_eq!((output.width(), output.height()), (size, size));
    let alpha = |x: usize, y: usize| output.pixels()[(y * size as usize + x) * 4 + 3];
    assert!(alpha(128, 128) > 128, "the disc's centre stays opaque, got {}", alpha(128, 128));
    assert!(alpha(4, 4) < 128, "the corner is background, got {}", alpha(4, 4));
}

#[test]
fn big_lama_changes_nothing_outside_the_mask() {
    let test = "big_lama_changes_nothing_outside_the_mask";
    let Some((runtime, path)) = setup(test, "big-lama") else {
        return;
    };
    validate_custom(&path, "lama").expect("the model has the lama signature");
    let size = 256;
    let image = bitmap(size, size, |x, y| ((x + y) % 256) as u8);
    let mask: Vec<u8> = (0..size * size)
        .map(|index| {
            let (x, y) = (index % size, index / size);
            u8::from((64..128).contains(&x) && (64..128).contains(&y))
        })
        .collect();
    let output =
        block_on(ModelInpainter::new(runtime, "big-lama").inpaint(image.clone(), mask.clone())).expect("the inpainting model runs");
    assert_eq!((output.width(), output.height()), (size, size));
    for (index, &masked) in mask.iter().enumerate() {
        let range = index * 4..index * 4 + 4;
        if masked == 0 {
            assert_eq!(&output.pixels()[range.clone()], &image.pixels()[range], "pixel {index} is outside the mask");
        } else {
            assert_eq!(output.pixels()[index * 4 + 3], 255, "alpha is kept inside the mask");
        }
    }
}
