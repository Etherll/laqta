//! The 27 classic filters as data. Kernels live in `laqta-imaging::effects::filter`.
//!
//! Parameters are clamped to their usable range by the kernels rather than rejected, so a preset
//! from a live preview or a `.sxie` file always renders. Angles are degrees; offsets and sizes are
//! pixels; `0..=1` fields are fractions.

use crate::color::Rgba;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Filter {
    /// Box blur in three passes (close to a Gaussian). `radius` 0 is identity; larger radii are capped at 1024.
    Blur {
        radius: u32,
    },
    /// Keeps `bits` per channel, 1..=8.
    ColorDepth {
        bits: u8,
    },
    /// 3x3 convolution, row-major from the top left: `sum / divisor + offset`. A divisor of 0 counts as 1.
    ConvolutionMatrix {
        kernel: [f32; 9],
        divisor: f32,
        offset: f32,
    },
    EdgeDetect,
    Emboss,
    /// Gaussian blur with standard deviation `sigma` (capped at 64). `sigma` of 0 or less is identity.
    GaussianBlur {
        sigma: f32,
    },
    /// Blurred halo of the alpha outline, drawn under the image and shifted by the offset. `size` is the
    /// blur radius and `strength` the gain. The canvas grows to hold the halo, which reaches `3 * size`
    /// past the image before the offset is applied.
    Glow {
        size: u32,
        strength: f32,
        color: Rgba,
        offset_x: i32,
        offset_y: i32,
    },
    MeanRemoval,
    /// A ring `size` pixels wide around the opaque shape, on a canvas padded by `size + padding` on
    /// every side. `outline_only` drops the image and keeps just the ring.
    Outline {
        size: u32,
        padding: u32,
        color: Rgba,
        outline_only: bool,
    },
    /// Averages `size` x `size` blocks. `size` 1 is identity.
    Pixelate {
        size: u32,
    },
    /// A flipped copy of the bottom `percentage` (0..=100) of the image, placed under it after `offset`
    /// transparent rows, fading from `max_alpha` to `min_alpha`. With `skew` the copy leans right by
    /// `skew_size` pixels at its far end, and the canvas widens by `skew_size`.
    /// Height grows by `offset + round(height * percentage / 100)`.
    Reflection {
        percentage: u8,
        max_alpha: u8,
        min_alpha: u8,
        offset: u32,
        skew: bool,
        skew_size: u32,
    },
    /// Moves red, green and blue by their own (x, y) offsets.
    RgbSplit {
        offset_red: (i32, i32),
        offset_green: (i32, i32),
        offset_blue: (i32, i32),
    },
    /// Blurred shadow of the alpha outline. `opacity` is 0..=1, `size` the blur radius, and `darkness`
    /// 0..=1 mixes `color` toward black. The shadow reaches `3 * size` past the image before the offset
    /// is applied. With `auto_resize` the canvas becomes the union of the image and the shadow; without
    /// it the canvas stays the same size and the shadow is clipped.
    Shadow {
        opacity: f32,
        size: u32,
        darkness: f32,
        color: Rgba,
        offset_x: i32,
        offset_y: i32,
        auto_resize: bool,
    },
    Sharpen,
    /// Horizontal strips, each `min_height..=max_height` tall (at least 1), shifted left or right by
    /// `min_shift..=max_shift` pixels. Pixels shifted in from outside the image are transparent.
    Slice {
        min_height: u32,
        max_height: u32,
        min_shift: u32,
        max_shift: u32,
        seed: u64,
    },
    Smooth,
    /// Torn edge on the chosen sides. Each side cuts up to `depth` pixels, with random cut points every
    /// `range` pixels. `curved` interpolates smoothly between them instead of straight segments.
    TornEdge {
        depth: u32,
        range: u32,
        top: bool,
        right: bool,
        bottom: bool,
        left: bool,
        curved: bool,
        seed: u64,
    },
    /// Sine-wave edge on the chosen sides, cutting up to `depth` pixels with a period of `range` pixels.
    WaveEdge {
        depth: u32,
        range: u32,
        top: bool,
        right: bool,
        bottom: bool,
        left: bool,
    },
    /// Adds uniform noise in `-amount..=amount` to each colour channel.
    Noise {
        amount: u8,
        seed: u64,
    },
    /// Quantises each channel to `levels` evenly spaced values, 2..=255.
    Posterize {
        levels: u8,
    },
    /// Blends toward `color` toward the corners. `radius` is where the falloff starts (0..=1 of the
    /// half-diagonal) and `strength` how far it goes (0..=1).
    Vignette {
        strength: f32,
        radius: f32,
        color: Rgba,
    },
    /// Each `cell` x `cell` block becomes a dot of its average colour on white. Dot area follows darkness.
    Halftone {
        cell: u32,
    },
    /// Floyd–Steinberg error diffusion to `levels` values per channel, 2..=255.
    Dither {
        levels: u8,
    },
    /// Average along a line `distance` pixels long (capped at 256) at `angle` degrees (0 is horizontal).
    MotionBlur {
        distance: u32,
        angle: f32,
    },
    /// Blurs toward a centre at (`center_x`, `center_y`), given as fractions of the image size. `amount`
    /// 0..=1 is how far along the way to the centre the samples reach.
    ZoomBlur {
        amount: f32,
        center_x: f32,
        center_y: f32,
    },
    /// Median of a `(2 * radius + 1)` square window. `radius` is capped at 10.
    Median {
        radius: u32,
    },
    /// Oil paint: in a `(2 * radius + 1)` square window, the most common of `levels` intensity bins,
    /// averaged. `levels` is 1..=255 and `radius` is capped at 10.
    OilPaint {
        radius: u32,
        levels: u8,
    },
}

impl Filter {
    pub fn name(&self) -> &'static str {
        match self {
            Filter::Blur { .. } => "Blur",
            Filter::ColorDepth { .. } => "Color depth",
            Filter::ConvolutionMatrix { .. } => "Convolution matrix",
            Filter::EdgeDetect => "Edge detect",
            Filter::Emboss => "Emboss",
            Filter::GaussianBlur { .. } => "Gaussian blur",
            Filter::Glow { .. } => "Glow",
            Filter::MeanRemoval => "Mean removal",
            Filter::Outline { .. } => "Outline",
            Filter::Pixelate { .. } => "Pixelate",
            Filter::Reflection { .. } => "Reflection",
            Filter::RgbSplit { .. } => "RGB split",
            Filter::Shadow { .. } => "Shadow",
            Filter::Sharpen => "Sharpen",
            Filter::Slice { .. } => "Slice",
            Filter::Smooth => "Smooth",
            Filter::TornEdge { .. } => "Torn edge",
            Filter::WaveEdge { .. } => "Wave edge",
            Filter::Noise { .. } => "Noise",
            Filter::Posterize { .. } => "Posterize",
            Filter::Vignette { .. } => "Vignette",
            Filter::Halftone { .. } => "Halftone",
            Filter::Dither { .. } => "Dither",
            Filter::MotionBlur { .. } => "Motion blur",
            Filter::ZoomBlur { .. } => "Zoom blur",
            Filter::Median { .. } => "Median",
            Filter::OilPaint { .. } => "Oil paint",
        }
    }

    /// One filter per variant, with ShareX-like default parameters.
    pub fn defaults() -> Vec<Filter> {
        vec![
            Filter::Blur { radius: 5 },
            Filter::ColorDepth { bits: 4 },
            Filter::ConvolutionMatrix { kernel: [0.0, -1.0, 0.0, -1.0, 5.0, -1.0, 0.0, -1.0, 0.0], divisor: 1.0, offset: 0.0 },
            Filter::EdgeDetect,
            Filter::Emboss,
            Filter::GaussianBlur { sigma: 2.0 },
            Filter::Glow { size: 10, strength: 2.0, color: Rgba::WHITE, offset_x: 0, offset_y: 0 },
            Filter::MeanRemoval,
            Filter::Outline { size: 5, padding: 10, color: Rgba::BLACK, outline_only: false },
            Filter::Pixelate { size: 10 },
            Filter::Reflection { percentage: 50, max_alpha: 150, min_alpha: 0, offset: 0, skew: false, skew_size: 0 },
            Filter::RgbSplit { offset_red: (-4, 0), offset_green: (0, 0), offset_blue: (4, 0) },
            Filter::Shadow { opacity: 0.8, size: 10, darkness: 0.0, color: Rgba::BLACK, offset_x: 10, offset_y: 10, auto_resize: true },
            Filter::Sharpen,
            Filter::Slice { min_height: 2, max_height: 10, min_shift: 2, max_shift: 12, seed: 1 },
            Filter::Smooth,
            Filter::TornEdge { depth: 8, range: 12, top: true, right: true, bottom: true, left: true, curved: false, seed: 1 },
            Filter::WaveEdge { depth: 6, range: 16, top: true, right: false, bottom: true, left: false },
            Filter::Noise { amount: 24, seed: 1 },
            Filter::Posterize { levels: 4 },
            Filter::Vignette { strength: 0.6, radius: 0.5, color: Rgba::BLACK },
            Filter::Halftone { cell: 6 },
            Filter::Dither { levels: 2 },
            Filter::MotionBlur { distance: 12, angle: 0.0 },
            Filter::ZoomBlur { amount: 0.3, center_x: 0.5, center_y: 0.5 },
            Filter::Median { radius: 2 },
            Filter::OilPaint { radius: 3, levels: 16 },
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::effects::Effect;

    #[test]
    fn defaults_have_one_distinct_name_per_variant() {
        let defaults = Filter::defaults();
        assert_eq!(defaults.len(), 27);
        let names: HashSet<&str> = defaults.iter().map(Filter::name).collect();
        assert_eq!(names.len(), 27);
    }

    #[test]
    fn defaults_survive_a_serde_round_trip_bare_and_wrapped() {
        for filter in Filter::defaults() {
            let json = serde_json::to_string(&filter).expect("filter serializes");
            let back: Filter = serde_json::from_str(&json).expect("filter deserializes");
            assert_eq!(back, filter, "{json}");

            let wrapped = Effect::Filter(filter.clone());
            let json = serde_json::to_string(&wrapped).expect("effect serializes");
            let back: Effect = serde_json::from_str(&json).expect("effect deserializes");
            assert_eq!(back, wrapped, "{json}");
        }
    }
}
