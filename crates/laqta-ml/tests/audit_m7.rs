#![allow(clippy::unwrap_used)]
//! Known LaMa crop defects. Each `audit_bug_*` test fails until the defect it names is fixed.

use laqta_ml::families::lama::{MODEL_SIZE, crop_for};

/// A wide mask (1000 x 100 px) on a 2000 x 600 image: the LaMa crop is not square, so resizing it
/// to the model's 512 x 512 input squashes it about 2:1 horizontally, and the fill is stretched back.
#[test]
#[ignore = "known issue, not fixed yet"]
fn audit_bug_lama_crop_is_not_square_so_the_fill_is_stretched() {
    let (w, h) = (2000usize, 600usize);
    let mut mask = vec![0u8; w * h];
    for y in 200..300 {
        for x in 100..1100 {
            mask[y * w + x] = 255;
        }
    }
    let crop = crop_for(&mask, w, h).unwrap();
    let (sx, sy) = (crop.width as f32 / MODEL_SIZE as f32, crop.height as f32 / MODEL_SIZE as f32);
    assert!((sx / sy - 1.0).abs() < 0.05, "crop {}x{} scales x by {sx:.2} and y by {sy:.2}", crop.width, crop.height);
}
