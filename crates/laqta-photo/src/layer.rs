//! Layers: what a layer draws, where it sits, and how it composites.

use laqta_domain::effects::{Effect, EffectItem};
use laqta_domain::color::Rgba;
use laqta_domain::{Bitmap, PixelFormat};

use crate::blend::BlendMode;
use crate::mask::Mask;
use crate::style::LayerStyle;
use crate::surface::Surface;

/// A layer's identity. Ids are never reused within one editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayerId(pub u64);

/// One layer. `offset` places the layer's content (its bitmap, or its shape's local coordinates) in
/// the document; a group's offset moves all its children.
#[derive(Clone, Debug)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub kind: LayerKind,
    pub visible: bool,
    /// 0..=1, applied to the whole layer including its styles.
    pub opacity: f32,
    pub blend: BlendMode,
    /// Document-sized. Hides the layer where it is 0. For an adjustment layer it sets how much of
    /// the adjustment shows.
    pub mask: Option<Mask>,
    pub styles: Vec<LayerStyle>,
    /// A locked layer refuses every edit except `SetLocked` and `SetVisible`.
    pub locked: bool,
    pub offset: (i32, i32),
}

#[derive(Clone, Debug)]
pub enum LayerKind {
    /// Straight RGBA8 pixels, placed at the layer's offset. Painting edits these.
    Raster(Bitmap),
    Shape(Shape),
    Text(TextLayer),
    /// Applies an effect to everything below it in the same list (the whole canvas for the root).
    Adjustment(Effect),
    /// A fill over the whole canvas.
    Fill(Fill),
    /// A border around the canvas.
    Frame(FrameLayer),
    /// Children composite in isolation, then the group blends as one layer.
    Group(Vec<Layer>),
    /// A source bitmap with effects re-applied on every render, so the source is never lost.
    Smart { source: Bitmap, effects: Vec<EffectItem> },
}

impl Layer {
    /// A visible, unlocked, fully opaque `Normal` layer at offset (0, 0) with no mask or styles.
    pub fn new(id: LayerId, name: impl Into<String>, kind: LayerKind) -> Layer {
        Layer {
            id,
            name: name.into(),
            kind: normalise(kind),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            mask: None,
            styles: Vec::new(),
            locked: false,
            offset: (0, 0),
        }
    }
}

/// Bitmaps enter the engine as straight RGBA8, whatever their input format.
pub(crate) fn normalise(kind: LayerKind) -> LayerKind {
    match kind {
        LayerKind::Raster(b) => LayerKind::Raster(rgba(b)),
        LayerKind::Smart { source, effects } => LayerKind::Smart { source: rgba(source), effects },
        other => other,
    }
}

fn rgba(b: Bitmap) -> Bitmap {
    if b.format() == PixelFormat::Rgba8 {
        b
    } else {
        Surface::from_bitmap(&b).into_bitmap()
    }
}

/// A vector shape. Each kind carries its own geometry in layer-local coordinates.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Shape {
    pub kind: ShapeKind,
    /// Filled with this colour; `Line` ignores it.
    pub fill: Option<Rgba>,
    /// Stroke colour and width, centred on the outline.
    pub stroke: Option<(Rgba, f32)>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ShapeKind {
    Rect { x: f32, y: f32, width: f32, height: f32 },
    Ellipse { x: f32, y: f32, width: f32, height: f32 },
    Line { from: (f32, f32), to: (f32, f32) },
    /// Closed for fill and stroke.
    Polygon(Vec<(f32, f32)>),
}

/// Text, rendered through the editor's injected fonts. Without fonts it draws nothing.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TextLayer {
    pub text: String,
    pub font_family: String,
    pub size: f32,
    pub color: Rgba,
    pub bold: bool,
    pub italic: bool,
    /// Top-left of the text, in layer-local coordinates.
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Fill {
    Solid(Rgba),
    /// `angle` in degrees, measured from the +x axis towards +y. The gradient spans the canvas.
    LinearGradient { from: Rgba, to: Rgba, angle: f32 },
    /// From `inner` at the canvas centre to `outer` at the corners.
    RadialGradient { inner: Rgba, outer: Rgba },
}

/// A border of `width` pixels around the canvas, with corners rounded to `radius`.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FrameLayer {
    pub width: f32,
    pub color: Rgba,
    pub radius: f32,
}
