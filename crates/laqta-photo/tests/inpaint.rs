#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]
//! The classic inpainting fallback: fills masked pixels from the boundary inward.

mod support;

use laqta_domain::Bitmap;
use laqta_photo::{Command, Mask, PhotoEditor, inpaint_fallback};

use support::{from_fn, px, solid};

/// A 16x12 image of one colour, with a 4x4 hole in the middle painted a different colour, and the
/// mask that marks the hole.
fn holed(colour: [u8; 4], hole_colour: [u8; 4]) -> (Bitmap, Mask) {
    let (w, h) = (16u32, 12u32);
    let in_hole = |x: u32, y: u32| (6..10).contains(&x) && (4..8).contains(&y);
    let image = from_fn(w, h, |x, y| if in_hole(x, y) { hole_colour } else { colour });
    let mask = Mask::from_vec(w, h, (0..w * h).map(|i| if in_hole(i % w, i / w) { 255 } else { 0 }).collect()).unwrap();
    (image, mask)
}

#[test]
fn a_hole_in_a_flat_colour_fills_with_that_colour() {
    let colour = [40, 120, 200, 255];
    // The hole is painted red, so the test fails if the masked pixels leak into the result.
    let (image, mask) = holed(colour, [255, 0, 0, 255]);
    let out = inpaint_fallback(&image, &mask);
    assert_eq!((out.width(), out.height()), (16, 12));
    for y in 0..12 {
        for x in 0..16 {
            assert_eq!(px(&out, x, y), colour, "at ({x}, {y})");
        }
    }
}

#[test]
fn known_pixels_are_left_alone() {
    let (image, mask) = holed([10, 10, 10, 255], [255, 0, 0, 255]);
    let out = inpaint_fallback(&image, &mask);
    for y in 0..12 {
        for x in 0..16 {
            if mask.get(x as i32, y as i32) == 0 {
                assert_eq!(px(&out, x, y), px(&image, x, y), "known pixel ({x}, {y}) changed");
            }
        }
    }
}

#[test]
fn a_hole_in_a_gradient_fills_between_its_neighbours() {
    // A horizontal ramp, 0 at the left to 150 at the right. The filled values stay between the
    // left and right neighbours of the hole.
    let image = from_fn(16, 12, |x, _| [(x * 10) as u8, 0, 0, 255]);
    let mask = Mask::from_vec(16, 12, (0..16 * 12).map(|i| if (6..10).contains(&(i % 16)) { 255 } else { 0 }).collect()).unwrap();
    let out = inpaint_fallback(&image, &mask);
    for x in 6..10 {
        let v = px(&out, x, 6)[0];
        assert!(v <= 150, "column {x} filled with {v}, above the ramp");
    }
    assert!(px(&out, 9, 6)[0] > px(&out, 6, 6)[0], "the right side of the hole is brighter");
}

#[test]
fn a_fully_masked_image_has_no_known_pixels_and_is_returned_unchanged() {
    let image = solid(4, 4, [1, 2, 3, 4]);
    let out = inpaint_fallback(&image, &Mask::filled(4, 4, 255));
    assert_eq!(out.pixels(), image.pixels());
}

#[test]
fn the_editor_applies_an_inpainting_result_as_one_undo_step() {
    let (image, _) = holed([40, 120, 200, 255], [255, 0, 0, 255]);
    let (filled, mask) = holed([40, 120, 200, 255], [255, 0, 0, 255]);
    let filled = inpaint_fallback(&filled, &mask);
    let mut ed = PhotoEditor::from_bitmap(image);
    let layer = ed.document().active.unwrap();
    ed.execute(Command::InpaintResult { layer, bitmap: filled.clone() }).unwrap();
    assert_eq!(ed.render().pixels(), filled.pixels());
    assert!(ed.undo());
    assert_eq!(px(&ed.render(), 7, 5), [255, 0, 0, 255]);
}
