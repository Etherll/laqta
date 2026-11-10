//! Text recognition (`Ocr`) through Vision.

use laqta_domain::Bitmap;
use laqta_ports::{BoxFuture, Ocr, OcrResult, PortError};

#[derive(Debug, Default)]
pub struct VisionOcr;

impl Ocr for VisionOcr {
    fn recognize(&self, _image: Bitmap, _language: String) -> BoxFuture<'_, Result<OcrResult, PortError>> {
        Box::pin(async { Err(PortError::Unsupported("text recognition on macOS".into())) })
    }

    fn languages(&self) -> Vec<String> {
        Vec::new()
    }
}
