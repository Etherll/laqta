//! The AI use cases the editor windows call: text recognition, background removal, object erase and
//! image analysis. Each runs one port call and maps its error to `AppError`; none emits a task event,
//! touches the clipboard or opens a window, so the caller decides what to do with the answer.
//!
//! The futures own what they need (`'static`), like `App::dispatch`, so a window can hand one to its
//! own executor. The port call is made when the future is first polled.

use laqta_domain::Bitmap;
use laqta_ports::{AppError, BoxFuture, OcrResult};

use crate::dispatcher::App;

impl App {
    /// The text in `image`, read with the default OCR model.
    pub fn recognize_text(&self, image: Bitmap) -> BoxFuture<'static, Result<OcrResult, AppError>> {
        let ports = std::sync::Arc::clone(&self.ports);
        Box::pin(async move { crate::tools::ocr_image(&ports, image, "").await })
    }

    /// `image` with its background made transparent.
    pub fn remove_background(&self, image: Bitmap) -> BoxFuture<'static, Result<Bitmap, AppError>> {
        let ports = std::sync::Arc::clone(&self.ports);
        Box::pin(async move { ports.background.remove(image).await.map_err(AppError::from) })
    }

    /// `image` with the pixels where `mask` is non-zero filled in from their surroundings. `mask` has one
    /// byte per pixel, row-major, the size of `image`.
    pub fn erase(&self, image: Bitmap, mask: Vec<u8>) -> BoxFuture<'static, Result<Bitmap, AppError>> {
        let ports = std::sync::Arc::clone(&self.ports);
        Box::pin(async move { ports.inpainter.inpaint(image, mask).await.map_err(AppError::from) })
    }

    /// The analyzer's answer to `prompt` about `image`.
    pub fn analyze(&self, image: Bitmap, prompt: String) -> BoxFuture<'static, Result<String, AppError>> {
        let ports = std::sync::Arc::clone(&self.ports);
        Box::pin(async move { ports.analyzer.analyze(image, prompt).await.map_err(AppError::from) })
    }
}
