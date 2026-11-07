//! Family `lama` (ADR 023): Big-LaMa inpainting, Apache-2.0. The model takes a fixed 512×512 image and
//! mask, so the work runs on a crop around the mask and only the masked pixels of the result are
//! written back. Pure: no ONNX Runtime, no threads. The session steps live in `inpaint.rs`.

use crate::tensor::{RgbaImage, bilinear, source_coord};

/// The side of the model's square input and output, in pixels.
pub const MODEL_SIZE: usize = 512;
/// The margin added around the mask's box, in pixels.
pub const PAD: usize = 32;
/// The feather width at the mask edge, in pixels. A masked pixel one step inside the edge gets half
/// weight, and one two or more steps in gets the full model colour.
pub const FEATHER: u32 = 2;

/// A rectangle of the image, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crop {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// The model inputs for one mask, and what the paste step needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub crop: Crop,
    /// Blend weight of each crop pixel, row-major and `crop.width` wide. Zero outside the mask.
    pub weights: Vec<f32>,
    /// The image crop as NCHW `[1, 3, 512, 512]`, RGB in 0..1.
    pub image: Vec<f32>,
    /// The mask crop as `[1, 1, 512, 512]`: 1 where the model fills, 0 elsewhere.
    pub mask: Vec<f32>,
}

/// The crop for a mask: the box of its set pixels, padded by `PAD` on each side and then widened to
/// at least `MODEL_SIZE`, or the image side where the image is smaller. The crop stays inside the image.
/// Returns `None` if no pixel is set.
pub fn crop_for(mask: &[u8], width: usize, height: usize) -> Option<Crop> {
    let mut found: Option<[usize; 4]> = None;
    for (index, &value) in mask.iter().enumerate() {
        if value == 0 {
            continue;
        }
        let (x, y) = (index % width, index / width);
        found = Some(match found {
            None => [x, y, x, y],
            Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
        });
    }
    let [x0, y0, x1, y1] = found?;
    let (x0, x1) = widen(x0.saturating_sub(PAD), (x1 + 1 + PAD).min(width), width);
    let (y0, y1) = widen(y0.saturating_sub(PAD), (y1 + 1 + PAD).min(height), height);
    Some(Crop { x: x0, y: y0, width: x1 - x0, height: y1 - y0 })
}

/// Widens `start..end` to at least `min(MODEL_SIZE, limit)` long. The extra room is split evenly, and
/// the result is moved back inside `0..limit` when it would stick out.
fn widen(start: usize, end: usize, limit: usize) -> (usize, usize) {
    let want = MODEL_SIZE.min(limit);
    let len = end - start;
    if len >= want {
        return (start, end);
    }
    let grow = want - len;
    let low = start.saturating_sub(grow / 2).min(limit - want);
    (low, low + want)
}

/// The model inputs for `image` and `mask` (one byte per pixel, row-major, non-zero = fill). Returns
/// `None` if the mask is empty, because there is nothing to fill.
pub fn plan(image: &RgbaImage, mask: &[u8]) -> Result<Option<Plan>, String> {
    let (width, height) = (image.width(), image.height());
    if mask.len() != width * height {
        return Err(format!("the mask has {} bytes for a {width}x{height} image", mask.len()));
    }
    let Some(crop) = crop_for(mask, width, height) else {
        return Ok(None);
    };
    let size = MODEL_SIZE;
    let mut image_input = vec![0.0; 3 * size * size];
    for channel in 0..3 {
        for dy in 0..size {
            let sy = source_coord(dy, crop.height, size);
            for dx in 0..size {
                let sx = source_coord(dx, crop.width, size);
                let value = bilinear(crop.width, crop.height, sx, sy, |x, y| f32::from(image.channel(crop.x + x, crop.y + y, channel)));
                image_input[(channel * size + dy) * size + dx] = value / 255.0;
            }
        }
    }
    let mut mask_input = vec![0.0; size * size];
    for dy in 0..size {
        let sy = source_coord(dy, crop.height, size);
        for dx in 0..size {
            let sx = source_coord(dx, crop.width, size);
            let value =
                bilinear(crop.width, crop.height, sx, sy, |x, y| if mask[(crop.y + y) * width + crop.x + x] != 0 { 1.0 } else { 0.0 });
            // The model was trained on hard masks, so the resampled mask is thresholded again.
            mask_input[dy * size + dx] = if value >= 0.5 { 1.0 } else { 0.0 };
        }
    }
    let weights = feather_weights(mask, width, crop);
    Ok(Some(Plan { crop, weights, image: image_input, mask: mask_input }))
}

/// Blend weights for the crop. Each masked pixel gets its 4-connected distance to the nearest unmasked
/// pixel, capped at `FEATHER`, divided by `FEATHER`. The crop's own edges do not count as unmasked, so
/// the image border is never feathered.
fn feather_weights(mask: &[u8], width: usize, crop: Crop) -> Vec<f32> {
    const FAR: u32 = u32::MAX / 2;
    let (cw, ch) = (crop.width, crop.height);
    let mut distance: Vec<u32> = (0..cw * ch)
        .map(|index| {
            let (x, y) = (crop.x + index % cw, crop.y + index / cw);
            if mask[y * width + x] != 0 { FAR } else { 0 }
        })
        .collect();
    // Two-pass city-block distance: top-left to bottom-right, then back.
    for y in 0..ch {
        for x in 0..cw {
            let i = y * cw + x;
            if distance[i] == 0 {
                continue;
            }
            let mut best = distance[i];
            if x > 0 {
                best = best.min(distance[i - 1] + 1);
            }
            if y > 0 {
                best = best.min(distance[i - cw] + 1);
            }
            distance[i] = best;
        }
    }
    for y in (0..ch).rev() {
        for x in (0..cw).rev() {
            let i = y * cw + x;
            if distance[i] == 0 {
                continue;
            }
            let mut best = distance[i];
            if x + 1 < cw {
                best = best.min(distance[i + 1] + 1);
            }
            if y + 1 < ch {
                best = best.min(distance[i + cw] + 1);
            }
            distance[i] = best;
        }
    }
    distance.iter().map(|&steps| steps.min(FEATHER) as f32 / FEATHER as f32).collect()
}

/// The factor that takes the model output to 0..255. The output is in 0..1 when its largest value is
/// at most 1, and in 0..255 otherwise. Exporters differ, so the range is read from the values.
pub fn output_scale(output: &[f32]) -> f32 {
    let largest = output.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if largest <= 1.0 { 255.0 } else { 1.0 }
}

/// Writes the model's inpainted colour into the masked pixels of the crop. Each pixel becomes
/// `original * (1 - weight) + fill * weight`. Alpha and unmasked pixels are left alone. `shape` and
/// `output` are the model's `[1, 3, 512, 512]` output.
pub fn paste(image: &mut RgbaImage, crop: Crop, weights: &[f32], shape: &[usize], output: &[f32]) -> Result<(), String> {
    let size = MODEL_SIZE;
    if shape != &[1, 3, size, size][..] || output.len() != 3 * size * size {
        return Err(format!("the output has shape {shape:?}, expected [1, 3, {size}, {size}]"));
    }
    if weights.len() != crop.width * crop.height {
        return Err(format!("the weights do not match the {}x{} crop", crop.width, crop.height));
    }
    let scale = output_scale(output);
    for y in 0..crop.height {
        let sy = source_coord(y, size, crop.height);
        for x in 0..crop.width {
            let weight = weights[y * crop.width + x];
            if weight <= 0.0 {
                continue;
            }
            let sx = source_coord(x, size, crop.width);
            let (px, py) = (crop.x + x, crop.y + y);
            for channel in 0..3 {
                let plane = &output[channel * size * size..(channel + 1) * size * size];
                let fill = (bilinear(size, size, sx, sy, |mx, my| plane[my * size + mx]) * scale).clamp(0.0, 255.0);
                let original = f32::from(image.channel(px, py, channel));
                image.set_channel(px, py, channel, (original * (1.0 - weight) + fill * weight).round().clamp(0.0, 255.0) as u8);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use laqta_domain::{Bitmap, PixelFormat};

    fn solid(width: u32, height: u32, pixel: [u8; 4]) -> RgbaImage {
        let bytes: Vec<u8> = pixel.iter().copied().cycle().take(width as usize * height as usize * 4).collect();
        RgbaImage::from_bitmap(&Bitmap::new(width, height, PixelFormat::Rgba8, bytes.into()).expect("valid bitmap"))
    }

    /// A mask of `width` x `height` with the given pixels set.
    fn mask_with(width: usize, height: usize, set: &[(usize, usize)]) -> Vec<u8> {
        let mut mask = vec![0u8; width * height];
        for &(x, y) in set {
            mask[y * width + x] = 1;
        }
        mask
    }

    /// A model output of one constant value.
    fn constant_output(value: f32) -> Vec<f32> {
        vec![value; 3 * MODEL_SIZE * MODEL_SIZE]
    }

    #[test]
    fn a_small_mask_is_widened_to_the_model_size() {
        let mask = mask_with(2000, 2000, &[(100, 100)]);
        let crop = crop_for(&mask, 2000, 2000).expect("the mask is not empty");
        assert_eq!(crop, Crop { x: 0, y: 0, width: 512, height: 512 });
    }

    #[test]
    fn the_crop_stays_inside_the_image_at_the_far_corner() {
        let mask = mask_with(1000, 1000, &[(990, 990)]);
        let crop = crop_for(&mask, 1000, 1000).expect("the mask is not empty");
        assert_eq!(crop, Crop { x: 488, y: 488, width: 512, height: 512 });
    }

    #[test]
    fn a_small_image_is_cropped_whole_when_it_is_smaller_than_the_model() {
        let mask = mask_with(300, 200, &[(10, 10)]);
        let crop = crop_for(&mask, 300, 200).expect("the mask is not empty");
        assert_eq!(crop, Crop { x: 0, y: 0, width: 300, height: 200 });
    }

    #[test]
    fn a_large_box_keeps_its_padded_size() {
        // Set pixels at x = 100 and 1100: the box is 1001 wide, padded to 1065 px.
        let mask = mask_with(2000, 300, &[(100, 150), (1100, 150)]);
        let crop = crop_for(&mask, 2000, 300).expect("the mask is not empty");
        assert_eq!((crop.x, crop.width), (68, 1065));
        assert_eq!((crop.y, crop.height), (0, 300));
    }

    #[test]
    fn an_empty_mask_has_no_crop() {
        assert_eq!(crop_for(&[0; 4], 2, 2), None);
        let image = solid(2, 2, [0, 0, 0, 255]);
        assert_eq!(plan(&image, &[0; 4]), Ok(None));
    }

    #[test]
    fn a_mask_of_the_wrong_length_is_rejected() {
        let image = solid(4, 4, [0, 0, 0, 255]);
        assert!(plan(&image, &[1; 15]).is_err());
    }

    #[test]
    fn the_feather_is_half_at_the_edge_and_full_two_steps_in() {
        let mask = mask_with(5, 5, &[(1, 1), (2, 1), (3, 1), (1, 2), (2, 2), (3, 2), (1, 3), (2, 3), (3, 3)]);
        let weights = feather_weights(&mask, 5, Crop { x: 0, y: 0, width: 5, height: 5 });
        assert_eq!(weights[0], 0.0);
        assert_eq!(weights[5 + 1], 0.5, "the corner of the block is one step from the outside");
        assert_eq!(weights[5 + 2], 0.5, "an edge pixel is one step from the outside");
        assert_eq!(weights[10 + 2], 1.0, "the centre is two steps from the outside");
    }

    #[test]
    fn the_plan_fills_the_model_inputs() {
        let image = solid(4, 4, [255, 255, 255, 255]);
        let mask = mask_with(4, 4, &[(1, 1)]);
        let plan = plan(&image, &mask).expect("valid inputs").expect("the mask is not empty");
        assert_eq!(plan.crop, Crop { x: 0, y: 0, width: 4, height: 4 });
        assert!(plan.image.iter().all(|value| (value - 1.0).abs() < 1e-5), "white is 1.0 in every channel");
        let at = |x: usize, y: usize| plan.mask[y * MODEL_SIZE + x];
        assert_eq!(at(192, 192), 1.0, "inside the masked pixel");
        assert_eq!(at(10, 10), 0.0, "far from the mask");
    }

    #[test]
    fn paste_blends_only_the_masked_pixels_with_the_model_colour() {
        let mut image = solid(4, 4, [255, 0, 0, 255]);
        let mask = mask_with(4, 4, &[(1, 1)]);
        let plan = plan(&image, &mask).expect("valid inputs").expect("the mask is not empty");
        // Output 0.25 is 0..1 (largest value at most 1), so the fill is 63.75 in every channel.
        paste(&mut image, plan.crop, &plan.weights, &[1, 3, MODEL_SIZE, MODEL_SIZE], &constant_output(0.25)).expect("valid output");
        // One step in, the weight is 0.5: red 255 * 0.5 + 63.75 * 0.5 = 159.4, green and blue 31.9.
        assert_eq!((image.channel(1, 1, 0), image.channel(1, 1, 1), image.channel(1, 1, 2)), (159, 32, 32));
        assert_eq!(image.channel(1, 1, 3), 255, "alpha is kept");
        assert_eq!((image.channel(0, 0, 0), image.channel(0, 0, 1)), (255, 0), "unmasked pixels are unchanged");
        assert_eq!((image.channel(2, 2, 0), image.channel(2, 2, 1)), (255, 0));
    }

    #[test]
    fn paste_takes_the_full_model_colour_two_steps_in() {
        let mut image = solid(5, 5, [0, 0, 0, 255]);
        let mask = mask_with(5, 5, &[(1, 1), (2, 1), (3, 1), (1, 2), (2, 2), (3, 2), (1, 3), (2, 3), (3, 3)]);
        let plan = plan(&image, &mask).expect("valid inputs").expect("the mask is not empty");
        paste(&mut image, plan.crop, &plan.weights, &[1, 3, MODEL_SIZE, MODEL_SIZE], &constant_output(0.25)).expect("valid output");
        assert_eq!(image.channel(2, 2, 0), 64, "the centre takes the 63.75 fill");
        assert_eq!(image.channel(1, 2, 0), 32, "the edge pixel is half blended");
        assert_eq!(image.channel(0, 0, 0), 0, "outside the mask nothing changes");
    }

    #[test]
    fn the_output_range_is_read_from_its_values() {
        assert_eq!(output_scale(&[0.0, 0.5]), 255.0);
        assert_eq!(output_scale(&[0.0, 200.0]), 1.0);
    }

    #[test]
    fn paste_rejects_an_output_of_the_wrong_shape() {
        let mut image = solid(4, 4, [0, 0, 0, 255]);
        let weights = vec![0.0; 16];
        let crop = Crop { x: 0, y: 0, width: 4, height: 4 };
        assert!(paste(&mut image, crop, &weights, &[1, 3, 256, 256], &constant_output(0.0)).is_err());
    }
}
