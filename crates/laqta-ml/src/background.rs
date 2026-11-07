//! The `BackgroundRemover` port backed by a catalog model of family `matting-parametric` (ADR 023,
//! M7 D6). Inference runs on its own thread, and the result comes back over a oneshot channel, as
//! `ModelOcr` does.

use std::sync::Arc;

use laqta_domain::Bitmap;
use laqta_ports::{BackgroundRemover, BoxFuture, PortError};
use ndarray::Array4;
use ort::inputs;
use ort::value::TensorRef;

use crate::catalog::{Task, catalog};
use crate::error::MlError;
use crate::families::matting::{self, MattingParams};
use crate::families::{Family, resolve_output};
use crate::runtime::{Runtime, ort_error, run_on_thread};
use crate::tensor::RgbaImage;

/// Background removal with one matting model from the catalog, for example `birefnet-lite`.
#[derive(Debug)]
pub struct ModelBackground {
    runtime: Arc<Runtime>,
    model_id: String,
}

impl ModelBackground {
    /// `model_id` is a catalog id of a background model.
    pub fn new(runtime: Arc<Runtime>, model_id: impl Into<String>) -> Self {
        ModelBackground { runtime, model_id: model_id.into() }
    }

    /// Checks that the model is an installed background model, and returns its parameters.
    fn prepare(&self) -> Result<MattingParams, MlError> {
        let entry = catalog().model(&self.model_id).ok_or_else(|| MlError::UnknownModel(self.model_id.clone()))?;
        if entry.task != Task::Background || Family::parse(&entry.family) != Some(Family::MattingParametric) {
            return Err(MlError::WrongTask { id: self.model_id.clone(), task: Task::Background.as_str() });
        }
        if !self.runtime.is_installed(entry) {
            return Err(MlError::NotInstalled(self.model_id.clone()));
        }
        entry.matting.ok_or_else(|| MlError::Catalog(format!("model `{}` has no matting parameters", self.model_id)))
    }
}

impl BackgroundRemover for ModelBackground {
    fn remove(&self, image: Bitmap) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        let params = match self.prepare() {
            Ok(params) => params,
            Err(error) => return Box::pin(async move { Err(PortError::from(error)) }),
        };
        let runtime = self.runtime.clone();
        let model_id = self.model_id.clone();
        run_on_thread("laqta-background", move || remove_blocking(&runtime, &model_id, &params, image))
    }
}

/// Runs the matting model on `image` and returns the image with its alpha replaced by the matte.
fn remove_blocking(runtime: &Runtime, model_id: &str, params: &MattingParams, image: Bitmap) -> Result<Bitmap, MlError> {
    if image.width() == 0 || image.height() == 0 {
        return Ok(image);
    }
    let mut rgba = RgbaImage::from_bitmap(&image);
    let size = params.input_size as usize;
    let input = matting::input_tensor(&rgba, params);
    let input = Array4::from_shape_vec((1, 3, size, size), input).map_err(|error| MlError::Inference(error.to_string()))?;
    let (shape, values) = runtime.with_session(Task::Background, model_id, |session| {
        let index = resolve_output(params.output_index, session.outputs().len()).map_err(MlError::Inference)?;
        let outputs = session.run(inputs![TensorRef::from_array_view(input.view()).map_err(ort_error)?]).map_err(ort_error)?;
        let output = outputs[index].try_extract_array::<f32>().map_err(ort_error)?;
        Ok((output.shape().to_vec(), output.iter().copied().collect::<Vec<f32>>()))
    })?;
    matting::apply_alpha(&mut rgba, &shape, &values, params).map_err(MlError::Inference)?;
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

    fn pixel() -> Bitmap {
        Bitmap::new(1, 1, PixelFormat::Rgba8, vec![0, 0, 0, 255].into()).expect("valid bitmap")
    }

    #[test]
    fn an_unknown_model_is_refused_before_a_thread_starts() {
        let result = block_on(ModelBackground::new(runtime(), "no-such-model").remove(pixel()));
        assert!(matches!(&result, Err(PortError::Failed(message)) if message.contains("unknown model")), "{result:?}");
    }

    #[test]
    fn a_model_that_is_not_installed_is_refused() {
        let result = block_on(ModelBackground::new(runtime(), "birefnet-lite").remove(pixel()));
        assert!(matches!(&result, Err(PortError::Failed(message)) if message.contains("is not installed")), "{result:?}");
    }

    #[test]
    fn an_inpainting_model_is_not_a_background_model() {
        let result = block_on(ModelBackground::new(runtime(), "big-lama").remove(pixel()));
        assert!(matches!(&result, Err(PortError::Failed(message)) if message.contains("does not serve the background task")), "{result:?}");
    }
}
