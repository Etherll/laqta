#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]
//! Brush strokes on raster layers: paint, eraser, clone, heal, the fill bucket, selection clipping,
//! and layer offsets.

mod support;

use laqta_domain::color::Rgba;
use laqta_photo::{Brush, BrushTool, Command, Document, LayerId, LayerKind, PhotoEditor, SelectOp, SelectShape};

use support::{from_fn, px, solid};

const RED: Rgba = Rgba { r: 255, g: 0, b: 0, a: 255 };
const BLUE: Rgba = Rgba { r: 0, g: 0, b: 255, a: 255 };

/// A brush with a hard edge, so the dab's coverage is 1 inside its radius.
fn hard(size: f32, color: Rgba) -> Brush {
    Brush { size, hardness: 1.0, opacity: 1.0, spacing: 0.25, color }
}

/// An editor over a `w` x `h` transparent canvas with one raster layer, `pixels` as its bitmap.
fn with_layer(w: u32, pixels: laqta_domain::Bitmap) -> (PhotoEditor, LayerId) {
    let mut ed = PhotoEditor::new(Document::new(w, pixels.height()).unwrap());
    ed.execute(Command::AddLayer { name: "paint".into(), kind: LayerKind::Raster(pixels) }).unwrap();
    let id = ed.document().active.unwrap();
    (ed, id)
}

fn stroke(ed: &mut PhotoEditor, layer: LayerId, path: &[(f32, f32)], brush: Brush, tool: BrushTool) {
    ed.execute(Command::Paint { layer, path: path.to_vec(), brush, tool }).unwrap();
}

#[test]
fn a_hard_dab_paints_a_disc_of_its_colour() {
    let (mut ed, id) = with_layer(16, solid(16, 16, [0, 0, 0, 0]));
    stroke(&mut ed, id, &[(8.0, 8.0)], hard(6.0, RED), BrushTool::Paint);
    let out = ed.render();
    assert_eq!(px(&out, 8, 8), [255, 0, 0, 255]);
    assert_eq!(px(&out, 13, 8)[3], 0, "outside the radius");
}

#[test]
fn opacity_scales_the_paint() {
    let (mut ed, id) = with_layer(16, solid(16, 16, [0, 0, 0, 0]));
    let brush = Brush { opacity: 0.5, ..hard(6.0, RED) };
    stroke(&mut ed, id, &[(8.0, 8.0)], brush, BrushTool::Paint);
    let a = i32::from(px(&ed.render(), 8, 8)[3]);
    assert!((a - 128).abs() <= 1, "alpha {a}");
}

#[test]
fn overlapping_dabs_in_one_stroke_do_not_stack() {
    // Dabs overlap heavily along the path. Each pixel takes the strongest dab once, so the stroke
    // reads 50% everywhere on its centre line, not a saturated 100%.
    let (mut ed, id) = with_layer(16, solid(16, 16, [0, 0, 0, 0]));
    let brush = Brush { opacity: 0.5, ..hard(6.0, RED) };
    stroke(&mut ed, id, &[(2.0, 8.0), (14.0, 8.0)], brush, BrushTool::Paint);
    let out = ed.render();
    for x in 5..11 {
        let a = i32::from(px(&out, x, 8)[3]);
        assert!((a - 128).abs() <= 1, "x={x} alpha {a}");
    }
}

#[test]
fn eraser_makes_pixels_transparent() {
    let (mut ed, id) = with_layer(16, solid(16, 16, [255, 255, 255, 255]));
    stroke(&mut ed, id, &[(8.0, 8.0)], hard(6.0, RED), BrushTool::Erase);
    let out = ed.render();
    assert_eq!(px(&out, 8, 8)[3], 0);
    assert_eq!(px(&out, 0, 0), [255, 255, 255, 255]);
}

#[test]
fn clone_copies_from_the_offset_source() {
    // Left half red, right half blue. Clone a dab at (12, 8) from (4, 8): offset (8, 0).
    let base = from_fn(16, 16, |x, _| if x < 8 { [255, 0, 0, 255] } else { [0, 0, 255, 255] });
    let (mut ed, id) = with_layer(16, base);
    stroke(&mut ed, id, &[(12.0, 8.0)], hard(4.0, RED), BrushTool::Clone { offset: (8, 0) });
    let out = ed.render();
    assert_eq!(px(&out, 12, 8), [255, 0, 0, 255], "cloned from the red side");
    assert_eq!(px(&out, 9, 8), [0, 0, 255, 255], "outside the dab stays blue");
}

#[test]
fn heal_takes_the_destination_brightness_and_the_source_texture() {
    // Source (x < 8) is 200 grey, destination (x >= 8) is 50 grey. Healing shifts the source by
    // the difference of the means, so the dab lands at 50.
    let base = from_fn(16, 16, |x, _| {
        let g = if x < 8 { 200 } else { 50 };
        [g, g, g, 255]
    });
    let (mut ed, id) = with_layer(16, base);
    stroke(&mut ed, id, &[(12.0, 8.0)], hard(4.0, RED), BrushTool::Heal { offset: (8, 0) });
    let out = px(&ed.render(), 12, 8);
    for c in &out[..3] {
        assert!((i32::from(*c) - 50).abs() <= 1, "healed pixel {out:?}");
    }
}

#[test]
fn bucket_fills_one_region_or_every_match() {
    // Two separate red blocks on a transparent canvas, left (x 0..2) and right (x 4..6).
    let blocks = || from_fn(8, 4, |x, y| if y < 2 && (x < 2 || (4..6).contains(&x)) { [255, 0, 0, 255] } else { [0, 0, 0, 0] });

    let (mut ed, id) = with_layer(8, blocks());
    ed.execute(Command::FloodFill { layer: id, x: 0, y: 0, tolerance: 0, color: BLUE, contiguous: true }).unwrap();
    let out = ed.render();
    assert_eq!(px(&out, 0, 0), [0, 0, 255, 255], "the seeded block is filled");
    assert_eq!(px(&out, 4, 0), [255, 0, 0, 255], "the other block is not");

    let (mut ed, id) = with_layer(8, blocks());
    ed.execute(Command::FloodFill { layer: id, x: 0, y: 0, tolerance: 0, color: BLUE, contiguous: false }).unwrap();
    assert_eq!(px(&ed.render(), 4, 0), [0, 0, 255, 255], "global: every match is filled");
}

#[test]
fn strokes_are_clipped_by_the_selection() {
    let (mut ed, id) = with_layer(16, solid(16, 16, [0, 0, 0, 0]));
    ed.execute(Command::Select { shape: SelectShape::Rect { x: 0.0, y: 0.0, width: 8.0, height: 16.0 }, op: SelectOp::Replace }).unwrap();
    stroke(&mut ed, id, &[(0.0, 8.0), (15.0, 8.0)], hard(4.0, RED), BrushTool::Paint);
    let out = ed.render();
    assert_eq!(px(&out, 4, 8), [255, 0, 0, 255], "inside the selection");
    assert_eq!(px(&out, 12, 8)[3], 0, "outside the selection");
}

#[test]
fn the_layer_offset_places_its_pixels() {
    // Layer-local (8, 8) with the layer at offset (5, 0) is document (13, 8).
    let (mut ed, id) = with_layer(16, solid(16, 16, [0, 0, 0, 0]));
    ed.execute(Command::SetOffset { layer: id, offset: (5, 0) }).unwrap();
    stroke(&mut ed, id, &[(8.0, 8.0)], hard(6.0, RED), BrushTool::Paint);
    let out = ed.render();
    assert_eq!(px(&out, 13, 8), [255, 0, 0, 255]);
    assert_eq!(px(&out, 8, 8)[3], 0);
}

#[test]
fn an_empty_path_or_a_stroke_outside_the_layer_changes_nothing() {
    let (mut ed, id) = with_layer(8, solid(8, 8, [0, 0, 0, 0]));
    let before = ed.render();
    stroke(&mut ed, id, &[], hard(4.0, RED), BrushTool::Paint);
    stroke(&mut ed, id, &[(100.0, 100.0)], hard(4.0, RED), BrushTool::Paint);
    assert_eq!(ed.render().pixels(), before.pixels());
    // Nothing was recorded: the one undo step left is the layer's creation.
    assert!(ed.undo());
    assert!(ed.document().layers.is_empty());
}
