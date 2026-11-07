//! Effect kernels (M3 D4), one module per category, matching `laqta_domain::effects`.

use laqta_domain::Bitmap;
use laqta_domain::effects::{Effect, Preset, TextSpec};

mod adjust;
mod draw;
mod filter;
mod manip;

#[derive(Debug, Clone, thiserror::Error)]
pub enum EffectError {
    #[error("missing asset {0}")]
    MissingAsset(String),
    #[error("text rendering unavailable")]
    NoText,
    #[error("{0}")]
    Invalid(String),
}

/// What the kernels can't do themselves: load images named by path, and render text (fonts are
/// injected, ADR 024).
pub struct EffectContext<'a> {
    pub asset: &'a dyn Fn(&str) -> Option<Bitmap>,
    pub text: &'a dyn Fn(&TextSpec) -> Option<Bitmap>,
}

impl std::fmt::Debug for EffectContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EffectContext")
    }
}

impl EffectContext<'static> {
    /// No assets, no text.
    pub fn none() -> EffectContext<'static> {
        EffectContext { asset: &|_| None, text: &|_| None }
    }
}

/// Applies one effect. Results are Rgba8 straight alpha. Must run inside the compute pool.
pub fn apply(image: &Bitmap, effect: &Effect, cx: &EffectContext<'_>) -> Result<Bitmap, EffectError> {
    let rgba = crate::ops::to_rgba(image);
    match effect {
        Effect::Adjust(e) => adjust::apply(&rgba, e, cx),
        Effect::Draw(e) => draw::apply(&rgba, e, cx),
        Effect::Filter(e) => filter::apply(&rgba, e, cx),
        Effect::Manip(e) => manip::apply(&rgba, e, cx),
    }
}

/// Applies the enabled effects of a preset in order.
pub fn apply_preset(image: &Bitmap, preset: &Preset, cx: &EffectContext<'_>) -> Result<Bitmap, EffectError> {
    let mut img = crate::ops::to_rgba(image);
    for item in preset.effects.iter().filter(|i| i.enabled) {
        img = apply(&img, &item.effect, cx)?;
    }
    Ok(img)
}
