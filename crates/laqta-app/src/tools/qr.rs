//! QR tools: making a QR code, and reading the codes in a screen capture, an image or a file (M5 D5).

use std::path::Path;

use laqta_domain::Bitmap;
use laqta_imaging::compose_monitors;
use laqta_imaging::qr::{qr_decode, qr_encode};
use laqta_ports::AppError;

use super::common::{codec_error, read_file};
use crate::Ports;
use crate::task::compute;

/// A black-on-white QR code for `text`, `module` pixels per module, with the standard quiet zone.
pub fn qr_image(text: &str, module: u32) -> Result<Bitmap, AppError> {
    qr_encode(text, module).map_err(|error| AppError::Io(error.to_string()))
}

/// Every QR code on the screen: all monitors are captured and composed into one image, then decoded.
pub async fn qr_decode_screen(ports: &Ports) -> Result<Vec<String>, AppError> {
    let frames = ports.screen.capture_all().await?;
    compute(ports.spawner.as_ref(), move || Ok(qr_decode(&compose_monitors(&frames)))).await
}

/// Every QR code in `image`, such as a region the user selected.
pub async fn qr_decode_image(ports: &Ports, image: Bitmap) -> Result<Vec<String>, AppError> {
    compute(ports.spawner.as_ref(), move || Ok(qr_decode(&image))).await
}

/// Every QR code in the image file at `path`.
pub async fn qr_decode_file(ports: &Ports, path: &Path) -> Result<Vec<String>, AppError> {
    let source = read_file(ports, path).await?;
    let codecs = ports.codecs.clone();
    compute(ports.spawner.as_ref(), move || {
        let image = codecs.decode(&source).map_err(codec_error)?;
        Ok(qr_decode(&image))
    })
    .await
}
