//! The `Inpainter` port backed by the `lama` family (ADR 023).
//! Inference runs on its own thread,
//! and the result comes back over a oneshot channel, as `ModelOcr` does.

use std::sync::Arc;

use laqta_domain::Bitmap;
use laqta_ports::{BoxFuture, Inpainter, PortError};
use ndarray::Array4;
use ort::inputs;
use ort::value::TensorRef;

use crate::catalog::{Task, catalog};
use crate::error::MlError;
use crate::families::lama::{self, MODEL_SIZE};
use crate::families::{Family, resolve_output};
use crate::runtime::{Runtime, ort_error, run_on_thread};
use crate::tensor::RgbaImage;

/// Inpainting with one Big-LaMa model from the catalog.
#[derive(Debug)]
pub struct ModelInpainter {
    runtime: Arc<Runtime>,
    model_id: String,
}

impl ModelInpainter {
    /// `model_id` is a catalog id of an inpainting model, for example `big-lama`.
    pub fn new(runtime: Arc<Runtime>, model_id: impl Into<String>) -> Self {
        ModelInpainter { runtime, model_id: model_id.into() }
    }

    /// Checks that the model is an installed inpainting model.
    fn prepare(&self) -> Result<(), MlError> {
        let entry = catalog().model(&self.model_id).ok_or_else(|| MlError::UnknownModel(self.model_id.clone()))?;
        if entry.task != Task::Inpaint || Family::parse(&entry.family) != Some(Family::Lama) {
            return Err(MlError::WrongTask { id: self.model_id.clone(), task: Task::Inpaint.as_str() });
        }
        if !self.runtime.is_installed(entry) {
            return Err(MlError::NotInstalled(self.model_id.clone()));
        }
        Ok(())
    }
}

impl Inpainter for ModelInpainter {
    fn inpaint(&self, image: Bitmap, mask: Vec<u8>) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        if let Err(error) = self.prepare() {
            return Box::pin(async move { Err(PortError::from(error)) });
        }
        let runtime = self.runtime.clone();
        let model_id = self.model_id.clone();
        run_on_thread("laqta-inpaint", move || inpaint_blocking(&runtime, &model_id, image, &mask))
    }
}

/// Fills the masked pixels of `image` with the model's result. A mask with no pixels set returns the
/// image unchanged, without loading the model.
fn inpaint_blocking(runtime: &Runtime, model_id: &str, image: Bitmap, mask: &[u8]) -> Result<Bitmap, MlError> {
    let mut rgba = RgbaImage::from_bitmap(&image);
    let Some(plan) = lama::plan(&rgba, mask).map_err(MlError::Inference)? else {
        return Ok(image);
    };
    let lama::Plan { crop, weights, image: image_input, mask: mask_input } = plan;
    let image_tensor =
        Array4::from_shape_vec((1, 3, MODEL_SIZE, MODEL_SIZE), image_input).map_err(|error| MlError::Inference(error.to_string()))?;
    let mask_tensor =
        Array4::from_shape_vec((1, 1, MODEL_SIZE, MODEL_SIZE), mask_input).map_err(|error| MlError::Inference(error.to_string()))?;
    let (shape, values) = runtime.with_session(Task::Inpaint, model_id, |session| {
        let index = resolve_output(0, session.outputs().len()).map_err(MlError::Inference)?;
        let outputs = session
            .run(inputs![
                TensorRef::from_array_view(image_tensor.view()).map_err(ort_error)?,
                TensorRef::from_array_view(mask_tensor.view()).map_err(ort_error)?
            ])
            .map_err(ort_error)?;
        let output = outputs[index].try_extract_array::<f32>().map_err(ort_error)?;
        Ok((output.shape().to_vec(), output.iter().copied().collect::<Vec<f32>>()))
    })?;
    lama::paste(&mut rgba, crop, &weights, &shape, &values).map_err(MlError::Inference)?;
    rgba.into_bitmap(image.format())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExecutionProvider;
    use laqta_domain::PixelFormat;
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

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

    fn runtime() -> Arc<Runtime> {
        Arc::new(Runtime::new("/nonexistent-lib.so", "/nonexistent-models", ExecutionProvider::Cpu))
    }

    fn pixels(width: u32, height: u32) -> Bitmap {
        let bytes: Vec<u8> = (0..width * height * 4).map(|index| (index % 251) as u8).collect();
        Bitmap::new(width, height, PixelFormat::Bgra8, bytes.into()).expect("valid bitmap")
    }

    #[test]
    fn an_unknown_model_is_refused_before_a_thread_starts() {
        let result = block_on(ModelInpainter::new(runtime(), "no-such-model").inpaint(pixels(2, 2), vec![1; 4]));
        assert!(matches!(&result, Err(PortError::Failed(message)) if message.contains("unknown model")), "{result:?}");
    }

    #[test]
    fn a_model_that_is_not_installed_is_refused() {
        let result = block_on(ModelInpainter::new(runtime(), "big-lama").inpaint(pixels(2, 2), vec![1; 4]));
        assert!(matches!(&result, Err(PortError::Failed(message)) if message.contains("is not installed")), "{result:?}");
    }

    #[test]
    fn a_background_model_is_not_an_inpainting_model() {
        let result = block_on(ModelInpainter::new(runtime(), "birefnet-lite").inpaint(pixels(2, 2), vec![1; 4]));
        assert!(matches!(&result, Err(PortError::Failed(message)) if message.contains("does not serve the inpaint task")), "{result:?}");
    }

    #[test]
    fn an_empty_mask_returns_the_image_without_a_model() {
        let image = pixels(3, 2);
        let before = image.pixels().to_vec();
        let after = inpaint_blocking(&runtime(), "big-lama", image, &[0; 6]).expect("nothing to fill");
        assert_eq!(after.pixels(), before.as_slice());
        assert_eq!(after.format(), PixelFormat::Bgra8);
    }
}
