//! Manipulation effects (10 effects). Kernels live in `laqta-imaging::effects`.
//!
//! Conventions: angles are degrees, clockwise on screen. Sizes and offsets are pixels.

use crate::color::Rgba;

/// How `ForceProportions` reaches the target aspect ratio.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ForceMethod {
    /// Pads the canvas with `color` until the ratio matches. The image is not changed.
    Grow,
    /// Trims the image until the ratio matches.
    Crop,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Manip {
    /// Trims uniform borders. A `color` with alpha 0 means auto: the top-left pixel is the background.
    /// `tolerance` is the largest per-channel difference that still counts as background. `margin`
    /// pixels of background are kept around the content.
    AutoCrop {
        color: Rgba,
        tolerance: u8,
        margin: u32,
    },
    /// Changes the canvas by (left, top, right, bottom) pixels. Negative values trim.
    Canvas {
        margin: (i32, i32, i32, i32),
        color: Rgba,
    },
    /// Removes (left, top, right, bottom) pixels.
    Crop {
        margin: (u32, u32, u32, u32),
    },
    Flip {
        horizontal: bool,
        vertical: bool,
    },
    /// Makes the image's ratio `width:height`. A zero side disables the effect.
    ForceProportions {
        width: u32,
        height: u32,
        method: ForceMethod,
        color: Rgba,
    },
    /// A zero side is automatic. With `keep_aspect`, the auto side follows the other, and two given
    /// sides fit inside the box. Without it, a zero side keeps the image's own size.
    Resize {
        width: u32,
        height: u32,
        keep_aspect: bool,
    },
    /// Turns the image by `angle` degrees. Bilinear, except that multiples of 90 are exact. With
    /// `grow` the canvas becomes the rotated bounding box, otherwise it keeps its size. Uncovered
    /// pixels take `color`.
    Rotate {
        angle: f32,
        grow: bool,
        color: Rgba,
    },
    /// Makes the corners transparent, anti-aliased, with a radius in pixels.
    RoundedCorners {
        radius: u32,
    },
    /// Scales each axis by a percentage of its size (100.0 keeps it).
    Scale {
        width_pct: f32,
        height_pct: f32,
    },
    /// Shears by `x` and `y` degrees (clamped to ±89). With `grow` the canvas fits the sheared image.
    Skew {
        x: i32,
        y: i32,
        grow: bool,
    },
    /// Bends the image around a vertical cylinder. `strength` (0..1) is how far round the cylinder
    /// the image wraps; 0 keeps it flat. Edges stay put and the centre stretches.
    CylinderWrap {
        strength: f32,
    },
    /// Moves each pixel by seeded value noise: up to `amount` pixels, on cells `scale` pixels wide.
    DisplacementMap {
        amount: u32,
        scale: u32,
        seed: u64,
    },
    /// Magnifies the centre and squeezes the edges. `strength` (0..1) sets the lens curve.
    FisheyeLens {
        strength: f32,
    },
    /// Folds one side of a line over the other. `angle` is the line's normal in degrees, and
    /// `position` (0..1) places the line across the image. `shade` (0..1) darkens the crease.
    FoldCreaseWarp {
        angle: f32,
        position: f32,
        shade: f32,
    },
    /// A glass ball in the middle: refracts what is under it and adds a highlight. `radius` is in
    /// pixels, where 0 is half the shorter side. `refraction` (0..1) is how strong the bend is.
    GlassSphere {
        radius: u32,
        refraction: f32,
    },
    /// Splits the image into `segments` mirrored wedges around the centre. `rotation` turns them, in degrees.
    Kaleidoscope {
        segments: u32,
        rotation: f32,
    },
    /// Pushes pixels near `centre` (fractions of the image size, 0..1) by `push` pixels at the
    /// centre, falling off to nothing at `radius`. `smudge` (0..1) drags colour along the push.
    LiquifyPushSmudge {
        centre: (f32, f32),
        radius: u32,
        push: (f32, f32),
        smudge: f32,
    },
    /// Tiles the image in `columns` by `rows`, mirroring every other tile.
    MirrorTiles {
        columns: u32,
        rows: u32,
    },
    /// Folds the bottom-right corner back along a diagonal. `progress` (0..1) is how far the fold has
    /// come, `shadow` (0..1) darkens the page under it, and `back` is the colour of the page's back.
    PageCurl {
        progress: f32,
        shadow: f32,
        back: Rgba,
    },
    /// Maps the image onto a quadrilateral. Each corner is a fraction of the image size (0..1 is the
    /// image), in the order top left, top right, bottom right, bottom left. Outside is transparent.
    PerspectiveWarp {
        top_left: (f32, f32),
        top_right: (f32, f32),
        bottom_right: (f32, f32),
        bottom_left: (f32, f32),
    },
    /// Pinches (`amount` > 0) or bulges (< 0) the image around its centre, for -1..1. `radius` is in
    /// pixels, where 0 is half the shorter side.
    PinchBulge {
        amount: f32,
        radius: u32,
    },
    /// Unwraps the image into polar coordinates: x becomes the angle, y the distance from the centre.
    /// `strength` (0..1) blends from the flat image to the full warp.
    PolarWarp {
        strength: f32,
    },
    /// Concentric ripples from the centre: `amplitude` pixels of push, `wavelength` pixels between crests.
    RippleRefraction {
        amplitude: u32,
        wavelength: u32,
    },
    /// Turns the image in 3D about the horizontal (`angle_x`) and vertical (`angle_y`) axes, in
    /// degrees, with perspective. Outside the image is transparent.
    Rotate3D {
        angle_x: f32,
        angle_y: f32,
    },
    /// Turns the image as the front of a box `depth` pixels thick, showing the sides as it turns.
    /// Angles are in degrees, as for `Rotate3D`.
    Rotate3DBox {
        angle_x: f32,
        angle_y: f32,
        depth: u32,
    },
    /// Copies one half over the other: `vertical` mirrors left onto right, otherwise top onto bottom.
    /// `reverse` takes the right or bottom half as the source instead.
    Symmetry {
        vertical: bool,
        reverse: bool,
    },
    /// Repeats the whole image, scaled down, in `columns` by `rows` tiles. `gap` pixels of `color`
    /// separate the tiles.
    TileRepeat {
        columns: u32,
        rows: u32,
        gap: u32,
        color: Rgba,
    },
    /// Cuts the image into `columns` by `rows` tiles and shuffles them, in the order `seed` gives.
    /// Remainder pixels stay put.
    TileShuffle {
        columns: u32,
        rows: u32,
        seed: u64,
    },
    /// Twists the centre: the angle, in degrees, fades to zero at `radius` (0 is half the shorter side).
    Twirl {
        angle: f32,
        radius: u32,
    },
    /// Shifts each row sideways by a sine of its height: `amplitude` pixels, `wavelength` pixels
    /// between crests, and `phase` in radians.
    WaveDistortion {
        amplitude: u32,
        wavelength: u32,
        phase: f32,
    },
}

impl Manip {
    pub fn name(&self) -> &'static str {
        match self {
            Manip::AutoCrop { .. } => "AutoCrop",
            Manip::Canvas { .. } => "Canvas",
            Manip::Crop { .. } => "Crop",
            Manip::Flip { .. } => "Flip",
            Manip::ForceProportions { .. } => "ForceProportions",
            Manip::Resize { .. } => "Resize",
            Manip::Rotate { .. } => "Rotate",
            Manip::RoundedCorners { .. } => "RoundedCorners",
            Manip::Scale { .. } => "Scale",
            Manip::Skew { .. } => "Skew",
            Manip::CylinderWrap { .. } => "Cylinder wrap",
            Manip::DisplacementMap { .. } => "Displacement map",
            Manip::FisheyeLens { .. } => "Fisheye lens",
            Manip::FoldCreaseWarp { .. } => "Fold crease",
            Manip::GlassSphere { .. } => "Glass sphere",
            Manip::Kaleidoscope { .. } => "Kaleidoscope",
            Manip::LiquifyPushSmudge { .. } => "Liquify push & smudge",
            Manip::MirrorTiles { .. } => "Mirror tiles",
            Manip::PageCurl { .. } => "Page curl",
            Manip::PerspectiveWarp { .. } => "Perspective warp",
            Manip::PinchBulge { .. } => "Pinch & bulge",
            Manip::PolarWarp { .. } => "Polar warp",
            Manip::RippleRefraction { .. } => "Ripple refraction",
            Manip::Rotate3D { .. } => "Rotate 3D",
            Manip::Rotate3DBox { .. } => "Rotate 3D box",
            Manip::Symmetry { .. } => "Symmetry",
            Manip::TileRepeat { .. } => "Tile repeat",
            Manip::TileShuffle { .. } => "Tile shuffle",
            Manip::Twirl { .. } => "Twirl",
            Manip::WaveDistortion { .. } => "Wave distortion",
        }
    }

    /// One entry per variant, with default parameters.
    pub fn defaults() -> Vec<Self> {
        vec![
            Manip::AutoCrop { color: Rgba::TRANSPARENT, tolerance: 0, margin: 0 },
            Manip::Canvas { margin: (0, 0, 0, 0), color: Rgba::TRANSPARENT },
            Manip::Crop { margin: (0, 0, 0, 0) },
            Manip::Flip { horizontal: false, vertical: false },
            Manip::ForceProportions { width: 16, height: 9, method: ForceMethod::Grow, color: Rgba::TRANSPARENT },
            Manip::Resize { width: 0, height: 0, keep_aspect: true },
            Manip::Rotate { angle: 0.0, grow: true, color: Rgba::TRANSPARENT },
            Manip::RoundedCorners { radius: 10 },
            Manip::Scale { width_pct: 100.0, height_pct: 100.0 },
            Manip::Skew { x: 0, y: 0, grow: true },
            Manip::CylinderWrap { strength: 0.5 },
            Manip::DisplacementMap { amount: 8, scale: 6, seed: 1 },
            Manip::FisheyeLens { strength: 0.5 },
            Manip::FoldCreaseWarp { angle: 0.0, position: 0.5, shade: 0.5 },
            Manip::GlassSphere { radius: 0, refraction: 0.5 },
            Manip::Kaleidoscope { segments: 6, rotation: 0.0 },
            Manip::LiquifyPushSmudge { centre: (0.5, 0.5), radius: 8, push: (4.0, 0.0), smudge: 0.25 },
            Manip::MirrorTiles { columns: 2, rows: 2 },
            Manip::PageCurl { progress: 0.25, shadow: 0.5, back: Rgba::rgb(245, 240, 230) },
            Manip::PerspectiveWarp { top_left: (0.1, 0.0), top_right: (1.0, 0.0), bottom_right: (1.0, 1.0), bottom_left: (0.0, 1.0) },
            Manip::PinchBulge { amount: 0.5, radius: 0 },
            Manip::PolarWarp { strength: 0.5 },
            Manip::RippleRefraction { amplitude: 3, wavelength: 12 },
            Manip::Rotate3D { angle_x: 20.0, angle_y: -25.0 },
            Manip::Rotate3DBox { angle_x: 20.0, angle_y: -25.0, depth: 12 },
            Manip::Symmetry { vertical: true, reverse: false },
            Manip::TileRepeat { columns: 2, rows: 2, gap: 2, color: Rgba::TRANSPARENT },
            Manip::TileShuffle { columns: 4, rows: 4, seed: 1 },
            Manip::Twirl { angle: 90.0, radius: 0 },
            Manip::WaveDistortion { amplitude: 4, wavelength: 16, phase: 0.0 },
        ]
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn defaults_round_trip_and_names_are_unique() {
        let all = Manip::defaults();
        assert_eq!(all.len(), 30);
        let names: std::collections::BTreeSet<&str> = all.iter().map(Manip::name).collect();
        assert_eq!(names.len(), 30);
        for effect in all {
            let json = serde_json::to_string(&effect).unwrap();
            let back: Manip = serde_json::from_str(&json).unwrap();
            assert_eq!(back, effect, "{json}");
        }
    }
}
