//! Drawing effects. Kernels live in `laqta-imaging::effects`.
//!
//! Conventions: angles are degrees, clockwise on screen. Offsets are pixels, +x right, +y down.
//! Opacity is 0.0..=1.0.

use super::TextSpec;
use crate::color::Rgba;

/// Where an overlay sits in the canvas: one of nine anchors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Placement {
    TopLeft,
    TopCenter,
    TopRight,
    MiddleLeft,
    MiddleCenter,
    MiddleRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

/// Whether a border is added around the image or drawn over its edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BorderKind {
    Outside,
    Inside,
}

/// How an overlaid image is sized before it is placed.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SizeMode {
    /// Keep the overlay at its own size.
    DontResize,
    /// Exact pixel size. A 0 side keeps the overlay's own size on that axis.
    Absolute(u32, u32),
    /// Percent of the overlay's own size on each axis (100.0 keeps it).
    Percentage(f32),
}

/// The outline `DrawShape` draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
    Triangle,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Draw {
    /// Fills behind transparent pixels. With `gradient`, a linear blend from the first colour to the
    /// second, along `angle` degrees (0 runs left to right).
    Background { color: Rgba, gradient: Option<(Rgba, Rgba, f32)> },
    /// An asset behind the image. `tile` repeats it at its own size from the top-left. Otherwise
    /// `center` keeps its size in the middle, and neither stretches it to fill the image.
    BackgroundImage { path: String, center: bool, tile: bool },
    /// A solid frame `size` pixels wide. `dash` draws 3 units on and 1 off.
    Border { kind: BorderKind, size: u32, color: Rgba, dash: bool },
    /// Squares of `size` pixels in two colours, behind transparent pixels.
    Checkerboard { size: u32, color1: Rgba, color2: Rgba },
    /// An asset placed over the image. `opacity` multiplies its alpha.
    Image { path: String, placement: Placement, offset: (i32, i32), size_mode: SizeMode, opacity: f32 },
    /// `count` copies of an asset at random positions. Each copy is `min_size..=max_size` pixels wide
    /// (aspect kept), and turned by a random angle if `random_angle`. `seed` fixes the layout.
    Particles { path: String, count: u32, min_size: u32, max_size: u32, random_angle: bool, seed: u64 },
    /// Text on an optional rounded background of `corner_radius` pixels, with `padding` pixels around the text.
    Text { text: TextSpec, placement: Placement, offset: (i32, i32), background: Option<Rgba>, padding: u32, corner_radius: u32 },
    /// Text turned by `angle`, with an optional outline (colour, width in pixels) and drop shadow
    /// (colour, dx, dy).
    TextEx {
        text: TextSpec,
        angle: f32,
        placement: Placement,
        offset: (i32, i32),
        outline: Option<(Rgba, f32)>,
        shadow: Option<(Rgba, i32, i32)>,
    },
    /// `count` small coloured pieces of paper at seeded positions and angles. Each is `min_size` to
    /// `max_size` pixels long, with a seeded colour from a fixed palette.
    Confetti { count: u32, min_size: u32, max_size: u32, seed: u64 },
    /// A straight line from `from` to `to` (pixel coordinates, +y down), `width` pixels thick.
    DrawLine { from: (i32, i32), to: (i32, i32), width: u32, color: Rgba },
    /// A `shape` in the box at (`x`, `y`) of `width` by `height` pixels, with an optional `fill` and a
    /// `stroke` of `stroke_width` pixels centred on the outline.
    DrawShape { shape: ShapeKind, x: i32, y: i32, width: u32, height: u32, fill: Option<Rgba>, stroke: Rgba, stroke_width: u32 },
    /// A mouse pointer with its tip at (`x`, `y`), `size` pixels tall, with an optional drop shadow.
    FakeCursor { x: i32, y: i32, size: u32, shadow: bool },
    /// A linear gradient from `start` to `end` laid over the image, at `angle` degrees (0 runs left to
    /// right). `opacity` (0..1) multiplies the colours' alpha.
    GradientOverlay { start: Rgba, end: Rgba, angle: f32, opacity: f32 },
    /// `count` soft white flakes at seeded positions, `min_size` to `max_size` pixels across.
    Snowflakes { count: u32, min_size: u32, max_size: u32, seed: u64 },
}

impl Draw {
    pub fn name(&self) -> &'static str {
        match self {
            Draw::Background { .. } => "Background",
            Draw::BackgroundImage { .. } => "BackgroundImage",
            Draw::Border { .. } => "Border",
            Draw::Checkerboard { .. } => "Checkerboard",
            Draw::Image { .. } => "Image",
            Draw::Particles { .. } => "Particles",
            Draw::Text { .. } => "Text",
            Draw::TextEx { .. } => "TextEx",
            Draw::Confetti { .. } => "Confetti",
            Draw::DrawLine { .. } => "Draw line",
            Draw::DrawShape { .. } => "Draw shape",
            Draw::FakeCursor { .. } => "Fake cursor",
            Draw::GradientOverlay { .. } => "Gradient overlay",
            Draw::Snowflakes { .. } => "Snowflakes",
        }
    }

    /// One entry per variant, with default parameters.
    pub fn defaults() -> Vec<Self> {
        vec![
            Draw::Background { color: Rgba::BLACK, gradient: None },
            Draw::BackgroundImage { path: String::new(), center: false, tile: false },
            Draw::Border { kind: BorderKind::Outside, size: 10, color: Rgba::BLACK, dash: false },
            Draw::Checkerboard { size: 10, color1: Rgba::WHITE, color2: Rgba::rgb(0xcc, 0xcc, 0xcc) },
            Draw::Image {
                path: String::new(),
                placement: Placement::MiddleCenter,
                offset: (0, 0),
                size_mode: SizeMode::DontResize,
                opacity: 1.0,
            },
            Draw::Particles { path: String::new(), count: 20, min_size: 8, max_size: 32, random_angle: true, seed: 1 },
            Draw::Text {
                text: TextSpec::default(),
                placement: Placement::BottomRight,
                offset: (0, 0),
                background: None,
                padding: 0,
                corner_radius: 0,
            },
            Draw::TextEx {
                text: TextSpec::default(),
                angle: 0.0,
                placement: Placement::MiddleCenter,
                offset: (0, 0),
                outline: None,
                shadow: None,
            },
            Draw::Confetti { count: 40, min_size: 2, max_size: 5, seed: 1 },
            Draw::DrawLine { from: (0, 0), to: (64, 64), width: 2, color: Rgba::rgb(255, 0, 0) },
            Draw::DrawShape {
                shape: ShapeKind::Rectangle,
                x: 2,
                y: 2,
                width: 16,
                height: 12,
                fill: None,
                stroke: Rgba::rgb(255, 0, 0),
                stroke_width: 2,
            },
            Draw::FakeCursor { x: 4, y: 4, size: 16, shadow: true },
            Draw::GradientOverlay { start: Rgba::TRANSPARENT, end: Rgba { r: 0, g: 0, b: 0, a: 160 }, angle: 90.0, opacity: 1.0 },
            Draw::Snowflakes { count: 60, min_size: 1, max_size: 3, seed: 1 },
        ]
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn defaults_round_trip_and_names_are_unique() {
        let all = Draw::defaults();
        assert_eq!(all.len(), 14);
        let names: std::collections::BTreeSet<&str> = all.iter().map(Draw::name).collect();
        assert_eq!(names.len(), 14);
        for effect in all {
            let json = serde_json::to_string(&effect).unwrap();
            let back: Draw = serde_json::from_str(&json).unwrap();
            assert_eq!(back, effect, "{json}");
        }
    }
}
