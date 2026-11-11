#![allow(clippy::unwrap_used)]
//! LaMa crop defects that were found and fixed.

use laqta_ml::families::lama::{crop_for, frame_for};

/// A wide mask (1000 x 100 px) on a 2000 x 600 image: no square crop fits, so the crop is
/// letterboxed into the model's 512 x 512 input with one scale on both axes, not squashed 2:1.
#[test]
fn audit_bug_lama_crop_is_not_square_so_the_fill_is_stretched() {
    let (w, h) = (2000usize, 600usize);
    let mut mask = vec![0u8; w * h];
    for y in 200..300 {
        for x in 100..1100 {
            mask[y * w + x] = 255;
        }
    }
    let crop = crop_for(&mask, w, h).unwrap();
    let frame = frame_for(crop);
    let (sx, sy) = (crop.width as f32 / frame.width as f32, crop.height as f32 / frame.height as f32);
    assert!((sx / sy - 1.0).abs() < 0.05, "crop {}x{} scales x by {sx:.2} and y by {sy:.2}", crop.width, crop.height);
}
