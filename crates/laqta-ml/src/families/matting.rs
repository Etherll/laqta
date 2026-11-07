//! Family `matting-parametric` (ADR 023): background matting models that differ only in their numbers
//! (BiRefNet lite, BEN2, U²-Net-p). Everything here is pure: no ONNX Runtime, no threads. The session
//! steps live in `background.rs`.
//!
//! Before inference the RGB image is resampled to `input_size`² (bilinear), scaled to 0..1 and
//! normalised with the entry's mean and std, as NCHW. After inference one output is read, activated,
//! and resampled back to the image size. The result multiplies the image's own alpha. Colour is kept.

use serde::Deserialize;

use crate::tensor::{RgbaImage, bilinear, source_coord};

/// How the output read as alpha is turned into 0..1 values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Activation {
    /// The output holds logits: the logistic function is applied.
    Sigmoid,
    /// The output is scaled to 0..1 by its own minimum and maximum (rembg's U²-Net recipe).
    Minmax,
    /// The output is already 0..1 and is only clamped.
    #[serde(rename = "none")]
    Identity,
}

/// The entry parameters of a matting model: the `[model.matting]` table in the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MattingParams {
    /// The side of the square model input, in pixels.
    pub input_size: u32,
    /// Per-channel mean of the 0..1 RGB input, in R, G, B order.
    pub mean: [f32; 3],
    /// Per-channel standard deviation, in R, G, B order. Must be above zero.
    pub std: [f32; 3],
    /// The output that holds the matte. A negative index counts from the end, so the default -1 is
    /// the last output.
    #[serde(default = "last_output")]
    pub output_index: i64,
    pub activation: Activation,
}

fn last_output() -> i64 {
    -1
}

impl MattingParams {
    /// Checks the numbers the catalog must get right: a positive size, finite statistics, and a
    /// positive standard deviation (a zero one would divide by zero).
    pub fn check(&self) -> Result<(), String> {
        if self.input_size == 0 {
            return Err("input_size must be above 0".into());
        }
        if self.mean.iter().chain(&self.std).any(|value| !value.is_finite()) {
            return Err("mean and std must be finite".into());
        }
        if self.std.iter().any(|value| *value <= 0.0) {
            return Err("std must be above 0".into());
        }
        Ok(())
    }
}

/// The model input for `image`: a flat NCHW buffer of shape `[1, 3, S, S]` (S = `input_size`), with
/// each value `(channel / 255 - mean) / std`.
pub fn input_tensor(image: &RgbaImage, params: &MattingParams) -> Vec<f32> {
    let size = params.input_size as usize;
    let (width, height) = (image.width(), image.height());
    let mut out = vec![0.0; 3 * size * size];
    for (channel, (&mean, &std)) in params.mean.iter().zip(&params.std).enumerate() {
        for dy in 0..size {
            let sy = source_coord(dy, height, size);
            for dx in 0..size {
                let sx = source_coord(dx, width, size);
                let value = bilinear(width, height, sx, sy, |x, y| f32::from(image.channel(x, y, channel))) / 255.0;
                out[(channel * size + dy) * size + dx] = (value - mean) / std;
            }
        }
    }
    out
}

/// Writes the matte into the image's alpha: each pixel's alpha becomes `matte * alpha`, where the matte
/// is the activated model output resampled (bilinear) to the image size. `shape` and `output` are the
/// output tensor the entry selected.
pub fn apply_alpha(image: &mut RgbaImage, shape: &[usize], output: &[f32], params: &MattingParams) -> Result<(), String> {
    let (map_height, map_width) = output_map(shape, output.len())?;
    let map = activate(output, params.activation);
    let (width, height) = (image.width(), image.height());
    for y in 0..height {
        let sy = source_coord(y, map_height, height);
        for x in 0..width {
            let sx = source_coord(x, map_width, width);
            let matte = bilinear(map_width, map_height, sx, sy, |mx, my| map[my * map_width + mx]).clamp(0.0, 1.0);
            let alpha = f32::from(image.channel(x, y, 3));
            image.set_channel(x, y, 3, (matte * alpha).round().clamp(0.0, 255.0) as u8);
        }
    }
    Ok(())
}

/// The output after its activation. Every value is in 0..1. A constant min-max output has no range to
/// scale, so it becomes all zeros (fully transparent) instead of NaN.
pub fn activate(output: &[f32], activation: Activation) -> Vec<f32> {
    match activation {
        Activation::Sigmoid => output.iter().map(|value| 1.0 / (1.0 + (-value).exp())).collect(),
        Activation::Minmax => {
            let (low, high) =
                output.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), &value| (low.min(value), high.max(value)));
            let range = high - low;
            if range > 0.0 { output.iter().map(|value| (value - low) / range).collect() } else { vec![0.0; output.len()] }
        }
        Activation::Identity => output.iter().map(|value| value.clamp(0.0, 1.0)).collect(),
    }
}

/// The height and width of a matte output, which is `[1, 1, H, W]` or `[1, H, W]` (one image in the
/// batch). `len` is the number of values, which must match the shape.
pub fn output_map(shape: &[usize], len: usize) -> Result<(usize, usize), String> {
    let (height, width) = match shape {
        [1, 1, height, width] | [1, height, width] => (*height, *width),
        _ => return Err(format!("the output has shape {shape:?}, expected [1, 1, H, W] or [1, H, W]")),
    };
    if height.checked_mul(width) != Some(len) || len == 0 {
        return Err(format!("the output {shape:?} does not match its {len} values"));
    }
    Ok((height, width))
}

#[cfg(test)]
mod tests {
    use super::*;
    use laqta_domain::{Bitmap, PixelFormat};

    /// An RGBA image from pixels given in row-major order.
    fn image(width: u32, height: u32, pixels: &[[u8; 4]]) -> RgbaImage {
        let bytes: Vec<u8> = pixels.iter().flatten().copied().collect();
        let bitmap = Bitmap::new(width, height, PixelFormat::Rgba8, bytes.into()).expect("valid bitmap");
        RgbaImage::from_bitmap(&bitmap)
    }

    fn params(input_size: u32, mean: [f32; 3], std: [f32; 3], activation: Activation) -> MattingParams {
        MattingParams { input_size, mean, std, output_index: -1, activation }
    }

    const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

    #[test]
    fn input_is_normalised_with_the_imagenet_statistics() {
        let pixel = image(1, 1, &[[255, 0, 128, 255]]);
        let input = input_tensor(&pixel, &params(1, IMAGENET_MEAN, IMAGENET_STD, Activation::Sigmoid));
        assert_eq!(input.len(), 3);
        assert!((input[0] - (1.0 - 0.485) / 0.229).abs() < 1e-4);
        assert!((input[1] - (0.0 - 0.456) / 0.224).abs() < 1e-4);
        assert!((input[2] - (128.0 / 255.0 - 0.406) / 0.225).abs() < 1e-4);
    }

    #[test]
    fn input_is_resized_bilinearly_before_normalising() {
        // A 2x1 image with red 0 and 255. With mean 0 and std 1 the values are the 0..1 samples.
        let pixels = image(2, 1, &[[0, 0, 0, 255], [255, 0, 0, 255]]);
        let input = input_tensor(&pixels, &params(4, [0.0; 3], [1.0; 3], Activation::Sigmoid));
        // Destination x maps to source x / 2 - 0.25, clamped at the edges: 0, 0.25, 0.75, 1.
        let red_row = &input[..4];
        let expected = [0.0, 0.25, 0.75, 1.0];
        for (got, want) in red_row.iter().zip(expected) {
            assert!((got - want).abs() < 1e-5, "{red_row:?}");
        }
    }

    #[test]
    fn sigmoid_of_zero_halves_the_alpha_and_keeps_the_colour() {
        let mut pixel = image(1, 1, &[[9, 8, 7, 200]]);
        apply_alpha(&mut pixel, &[1, 1, 1, 1], &[0.0], &params(1, [0.0; 3], [1.0; 3], Activation::Sigmoid)).expect("valid output");
        assert_eq!(pixel.channel(0, 0, 3), 100);
        assert_eq!((pixel.channel(0, 0, 0), pixel.channel(0, 0, 1), pixel.channel(0, 0, 2)), (9, 8, 7));
    }

    #[test]
    fn minmax_scales_the_output_to_zero_and_one() {
        assert_eq!(activate(&[2.0, 4.0], Activation::Minmax), vec![0.0, 1.0]);
        assert_eq!(activate(&[3.0, 3.0], Activation::Minmax), vec![0.0, 0.0]);
        let mut pixels = image(2, 1, &[[0, 0, 0, 255], [0, 0, 0, 255]]);
        apply_alpha(&mut pixels, &[1, 1, 2], &[2.0, 4.0], &params(2, [0.0; 3], [1.0; 3], Activation::Minmax)).expect("valid output");
        assert_eq!(pixels.channel(0, 0, 3), 0);
        assert_eq!(pixels.channel(1, 0, 3), 255);
    }

    #[test]
    fn identity_only_clamps_the_output() {
        assert_eq!(activate(&[-1.0, 0.5, 2.0], Activation::Identity), vec![0.0, 0.5, 1.0]);
        let mut pixels = image(3, 1, &[[0, 0, 0, 254], [0, 0, 0, 254], [0, 0, 0, 254]]);
        apply_alpha(&mut pixels, &[1, 1, 3], &[-1.0, 0.5, 2.0], &params(3, [0.0; 3], [1.0; 3], Activation::Identity))
            .expect("valid output");
        let alphas: Vec<u8> = (0..3).map(|x| pixels.channel(x, 0, 3)).collect();
        assert_eq!(alphas, vec![0, 127, 254]);
    }

    #[test]
    fn the_matte_is_resized_back_to_the_image_bilinearly() {
        // A 2-value matte over a 4-pixel row: destination x maps to source x / 2 - 0.25.
        let mut pixels = image(4, 1, &[[0, 0, 0, 255]; 4]);
        apply_alpha(&mut pixels, &[1, 1, 2], &[0.0, 1.0], &params(2, [0.0; 3], [1.0; 3], Activation::Identity)).expect("valid output");
        let alphas: Vec<u8> = (0..4).map(|x| pixels.channel(x, 0, 3)).collect();
        assert_eq!(alphas, vec![0, 64, 191, 255]);
    }

    #[test]
    fn output_shapes_are_checked_against_the_value_count() {
        assert_eq!(output_map(&[1, 1, 2, 3], 6), Ok((2, 3)));
        assert_eq!(output_map(&[1, 2, 3], 6), Ok((2, 3)));
        assert!(output_map(&[1, 3, 2, 3], 18).is_err());
        assert!(output_map(&[2, 1, 2, 3], 12).is_err());
        assert!(output_map(&[1, 2, 3], 5).is_err());
        assert!(output_map(&[0, 0], 0).is_err());
    }

    #[test]
    fn a_bad_parameter_fails_the_check() {
        assert!(params(1024, IMAGENET_MEAN, IMAGENET_STD, Activation::Sigmoid).check().is_ok());
        assert!(params(0, IMAGENET_MEAN, IMAGENET_STD, Activation::Sigmoid).check().is_err());
        assert!(params(1024, IMAGENET_MEAN, [0.0, 1.0, 1.0], Activation::Sigmoid).check().is_err());
        assert!(params(1024, [f32::NAN, 0.0, 0.0], [1.0; 3], Activation::Sigmoid).check().is_err());
    }
}
