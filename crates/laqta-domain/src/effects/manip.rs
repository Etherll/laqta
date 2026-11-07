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
        assert_eq!(all.len(), 10);
        let names: std::collections::BTreeSet<&str> = all.iter().map(Manip::name).collect();
        assert_eq!(names.len(), 10);
        for effect in all {
            let json = serde_json::to_string(&effect).unwrap();
            let back: Manip = serde_json::from_str(&json).unwrap();
            assert_eq!(back, effect, "{json}");
        }
    }
}
