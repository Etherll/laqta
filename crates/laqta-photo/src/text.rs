//! Text layers, shaped and rasterised by the Editor engine's injected fonts (ADR 024, ADR 027).

use laqta_domain::effects::TextSpec;
use laqta_editor::{FontStyle, Fonts};

use crate::layer::TextLayer;
use crate::surface::Surface;

/// The text's pixels, or `None` when there are no fonts or no text.
pub(crate) fn render(fonts: &Fonts, t: &TextLayer) -> Option<Surface> {
    let style =
        FontStyle { family: t.font_family.clone(), size: t.size, bold: t.bold, italic: t.italic, color: t.color, ..FontStyle::default() };
    fonts.render_text(&t.text, &style).map(|b| Surface::from_bitmap(&b))
}

/// The same for an effect's text drawing.
pub(crate) fn render_spec(fonts: &Fonts, spec: &TextSpec) -> Option<laqta_domain::Bitmap> {
    let style = FontStyle {
        family: spec.font_family.clone(),
        size: spec.size,
        bold: spec.bold,
        italic: spec.italic,
        color: spec.color,
        ..FontStyle::default()
    };
    fonts.render_text(&spec.text, &style)
}
