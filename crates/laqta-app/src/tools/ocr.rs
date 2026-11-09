//! Text recognition for the OCR tool, through the `Ocr` port (ADR 023).

use laqta_domain::Bitmap;
use laqta_ports::{AppError, OcrResult};

use crate::Ports;

/// The text in `image`, recognised with `language`: a BCP-47 tag or a script name, or empty for the default model.
pub async fn ocr_image(ports: &Ports, image: Bitmap, language: &str) -> Result<OcrResult, AppError> {
    Ok(ports.ocr.recognize(image, language.to_owned()).await?)
}
