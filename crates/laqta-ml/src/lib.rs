//! The model runtime (ADR 023): the model catalog, downloads through laqta-http's verified fetch,
//! ONNX Runtime sessions with the release rule, the model families, and the custom-model check.
//! `ModelOcr` implements the `Ocr` port (PP-OCR, family O1). `ModelBackground` implements
//! `BackgroundRemover` (family `matting-parametric`) and `ModelInpainter` implements `Inpainter`
//! (family `lama`).
#![forbid(unsafe_code)]

pub mod background;
pub mod catalog;
pub mod error;
pub mod families;
pub mod inpaint;
pub mod install;
pub mod ocr;
pub mod runtime;
pub mod tensor;
pub mod validate;

pub use background::ModelBackground;
pub use catalog::{Catalog, ModelEntry, OcrPreset, OrtArchive, Speed, Task, catalog};
pub use error::MlError;
pub use inpaint::ModelInpainter;
pub use ocr::ModelOcr;
pub use runtime::{ExecutionProvider, Progress, Runtime};
pub use validate::validate_custom;
