//! Ports used by the tools (M5).

use laqta_domain::Bitmap;
use laqta_domain::geometry::PhysRect;
use laqta_domain::region::HoverTarget;

use crate::{BoxFuture, PortError};

#[derive(Clone, Debug, PartialEq, Default)]
pub struct OcrLine {
    pub rect: PhysRect,
    pub text: String,
    pub confidence: f32,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct OcrResult {
    pub lines: Vec<OcrLine>,
}

impl OcrResult {
    /// The lines joined top to bottom.
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    }
}

/// Text recognition (ADR 023): `laqta-ml`'s model families, or Windows.Media.Ocr.
pub trait Ocr: Send + Sync {
    /// `language` is a BCP-47 tag or the model's script name; empty = the default model.
    fn recognize(&self, image: Bitmap, language: String) -> BoxFuture<'_, Result<OcrResult, PortError>>;
    /// Languages or scripts that are ready (installed) now.
    fn languages(&self) -> Vec<String>;
}

/// Window tools (M5 D7).
pub trait WindowOps: Send + Sync {
    fn set_borderless(&self, window: &HoverTarget, borderless: bool) -> Result<(), PortError>;
    fn set_topmost(&self, window: &HoverTarget, topmost: bool) -> Result<(), PortError>;
    /// Properties for the inspector: (name, value) pairs such as class, pid, style flags.
    fn inspect(&self, window: &HoverTarget) -> Result<Vec<(String, String)>, PortError>;
}
