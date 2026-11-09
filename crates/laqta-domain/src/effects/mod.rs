//! Image effects as data. Kernels live in `laqta-imaging::effects`.
//! Each category is its own file and enum.

pub mod adjust;
pub mod draw;
pub mod filter;
pub mod manip;

pub use adjust::Adjust;
pub use draw::Draw;
pub use filter::Filter;
pub use manip::Manip;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "category", content = "effect")]
pub enum Effect {
    Adjust(Adjust),
    Draw(Draw),
    Filter(Filter),
    Manip(Manip),
}

impl Effect {
    pub fn name(&self) -> &'static str {
        match self {
            Effect::Adjust(e) => e.name(),
            Effect::Draw(e) => e.name(),
            Effect::Filter(e) => e.name(),
            Effect::Manip(e) => e.name(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EffectItem {
    pub effect: Effect,
    pub enabled: bool,
}

/// A named list of effects. Mirrors the `ImageEffectPreset` JSON that `.sxie` files hold.
#[derive(Clone, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Preset {
    pub name: String,
    pub effects: Vec<EffectItem>,
}

/// Text to draw, for the drawing effects; rendered by a callback that has fonts (ADR 024).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TextSpec {
    pub text: String,
    pub font_family: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub color: crate::color::Rgba,
}

impl Default for TextSpec {
    fn default() -> Self {
        TextSpec {
            text: String::new(),
            font_family: "sans-serif".into(),
            size: 18.0,
            bold: false,
            italic: false,
            color: crate::color::Rgba::WHITE,
        }
    }
}
