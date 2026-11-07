#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]
//! Selections (rectangle, ellipse, lasso, magic wand, combine, invert, feather, grow and shrink),
//! layer masks, and the lock rule.

mod support;

use laqta_photo::{Command, LayerKind, Mask, PhotoEditor, PhotoError, SelectOp, SelectShape};

use support::{from_fn, px, solid};

fn white(w: u32, h: u32) -> PhotoEditor {
    PhotoEditor::from_bitmap(solid(w, h, [255, 255, 255, 255]))
}

fn select(ed: &mut PhotoEditor, shape: SelectShape, op: SelectOp) {
    ed.execute(Command::Select { shape, op }).unwrap();
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> SelectShape {
    SelectShape::Rect { x, y, width, height }
}

fn selected_count(ed: &PhotoEditor) -> usize {
    ed.document().selection.as_ref().map_or(0, |m| m.data().iter().filter(|&&v| v == 255).count())
}

fn sel(ed: &PhotoEditor, x: i32, y: i32) -> u8 {
    ed.document().selection.as_ref().unwrap().get(x, y)
}

#[test]
fn rectangle_selects_its_pixels() {
    let mut ed = white(10, 8);
    select(&mut ed, rect(2.0, 2.0, 4.0, 3.0), SelectOp::Replace);
    assert_eq!(selected_count(&ed), 12);
    assert_eq!((sel(&ed, 2, 2), sel(&ed, 5, 4)), (255, 255));
    assert_eq!((sel(&ed, 6, 4), sel(&ed, 2, 5)), (0, 0));
}

#[test]
fn ellipse_selects_the_middle_not_the_corner() {
    let mut ed = white(10, 10);
    select(&mut ed, SelectShape::Ellipse { x: 0.0, y: 0.0, width: 10.0, height: 10.0 }, SelectOp::Replace);
    assert_eq!(sel(&ed, 5, 5), 255);
    assert_eq!(sel(&ed, 0, 0), 0);
}

#[test]
fn lasso_is_filled_even_odd() {
    // The square traced twice winds twice. Even-odd leaves its interior empty.
    let square = vec![(0.0, 0.0), (8.0, 0.0), (8.0, 8.0), (0.0, 8.0), (0.0, 0.0), (8.0, 0.0), (8.0, 8.0), (0.0, 8.0)];
    let mut ed = white(10, 10);
    select(&mut ed, SelectShape::Polygon(square), SelectOp::Replace);
    assert_eq!(sel(&ed, 4, 4), 0, "even-odd: the doubled interior is a hole");
}

#[test]
fn lasso_fills_a_triangle() {
    let tri = vec![(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)];
    let mut ed = white(10, 10);
    select(&mut ed, SelectShape::Polygon(tri), SelectOp::Replace);
    assert_eq!(sel(&ed, 1, 1), 255);
    assert_eq!(sel(&ed, 9, 9), 0);
}

#[test]
fn combine_operations() {
    // A = x in [0, 4), B = x in [2, 6), both y in [0, 4).
    let mut ed = white(8, 8);
    select(&mut ed, rect(0.0, 0.0, 4.0, 4.0), SelectOp::Replace);
    select(&mut ed, rect(2.0, 0.0, 4.0, 4.0), SelectOp::Add);
    assert_eq!(selected_count(&ed), 24, "union");

    let mut ed = white(8, 8);
    select(&mut ed, rect(0.0, 0.0, 4.0, 4.0), SelectOp::Replace);
    select(&mut ed, rect(2.0, 0.0, 4.0, 4.0), SelectOp::Subtract);
    assert_eq!(selected_count(&ed), 8, "A minus B");

    let mut ed = white(8, 8);
    select(&mut ed, rect(0.0, 0.0, 4.0, 4.0), SelectOp::Replace);
    select(&mut ed, rect(2.0, 0.0, 4.0, 4.0), SelectOp::Intersect);
    assert_eq!(selected_count(&ed), 8, "A and B");
}

#[test]
fn select_all_deselect_and_invert() {
    let mut ed = white(8, 8);
    select(&mut ed, rect(0.0, 0.0, 4.0, 4.0), SelectOp::Replace);
    ed.execute(Command::InvertSelection).unwrap();
    assert_eq!(selected_count(&ed), 48);
    ed.execute(Command::Deselect).unwrap();
    assert!(ed.document().selection.is_none());
    ed.execute(Command::InvertSelection).unwrap();
    assert_eq!(selected_count(&ed), 64, "inverting no selection selects all");
}

#[test]
fn feather_softens_the_edge_and_keeps_the_middle() {
    // Columns [2, 12) on a 16-wide canvas. With sigma 1, column 6 is more than 3 px from both
    // edges, so the blur's whole kernel falls inside the selection.
    let mut ed = white(16, 10);
    select(&mut ed, rect(2.0, 0.0, 10.0, 10.0), SelectOp::Replace);
    ed.execute(Command::Feather { radius: 1.0 }).unwrap();
    let outside = sel(&ed, 1, 5);
    let edge = sel(&ed, 2, 5);
    assert!(outside > 0 && outside < 128, "outside the edge is partly selected: {outside}");
    assert!(edge > 128 && edge < 255, "inside the edge is partly selected: {edge}");
    assert_eq!(sel(&ed, 6, 5), 255, "the middle stays selected");
}

#[test]
fn grow_and_shrink_move_the_edge_by_the_radius() {
    let mut ed = white(20, 20);
    select(&mut ed, rect(5.0, 5.0, 10.0, 10.0), SelectOp::Replace);
    ed.execute(Command::Shrink { radius: 2 }).unwrap();
    assert_eq!(selected_count(&ed), 36, "10x10 shrunk by 2 is 6x6");
    ed.execute(Command::Grow { radius: 2 }).unwrap();
    assert_eq!(selected_count(&ed), 100, "6x6 grown by 2 is 10x10");

    let mut ed = white(20, 20);
    select(&mut ed, rect(5.0, 5.0, 10.0, 10.0), SelectOp::Replace);
    ed.execute(Command::Grow { radius: 2 }).unwrap();
    assert_eq!(selected_count(&ed), 196, "10x10 grown by 2 is 14x14");
}

#[test]
fn magic_wand_contiguous_and_global() {
    // Red on the left (x < 4), blue on the right, and one red pixel (6, 2) in the blue.
    let base = from_fn(8, 4, |x, y| if x < 4 || (x, y) == (6, 2) { [200, 0, 0, 255] } else { [0, 0, 200, 255] });
    let mut ed = PhotoEditor::from_bitmap(base);
    select(&mut ed, SelectShape::MagicWand { x: 0, y: 0, tolerance: 0, contiguous: true }, SelectOp::Replace);
    assert_eq!(selected_count(&ed), 16, "contiguous: the left half only");

    select(&mut ed, SelectShape::MagicWand { x: 0, y: 0, tolerance: 0, contiguous: false }, SelectOp::Replace);
    assert_eq!(selected_count(&ed), 17, "global: the left half and the island");
}

#[test]
fn magic_wand_tolerance_widens_the_match() {
    let base = from_fn(4, 1, |x, _| [100 + 10 * x as u8, 0, 0, 255]);
    let mut ed = PhotoEditor::from_bitmap(base);
    select(&mut ed, SelectShape::MagicWand { x: 0, y: 0, tolerance: 0, contiguous: true }, SelectOp::Replace);
    assert_eq!(selected_count(&ed), 1);
    select(&mut ed, SelectShape::MagicWand { x: 0, y: 0, tolerance: 10, contiguous: true }, SelectOp::Replace);
    assert_eq!(selected_count(&ed), 2);
    select(&mut ed, SelectShape::MagicWand { x: 0, y: 0, tolerance: 30, contiguous: true }, SelectOp::Replace);
    assert_eq!(selected_count(&ed), 4);
}

#[test]
fn layer_mask_hides_pixels() {
    let mut ed = PhotoEditor::from_bitmap(solid(4, 4, [0, 0, 0, 255]));
    ed.execute(Command::AddLayer { name: "red".into(), kind: LayerKind::Raster(solid(4, 4, [255, 0, 0, 255])) }).unwrap();
    let top = ed.document().active.unwrap();
    let left_hidden = Mask::from_vec(4, 4, (0..16).map(|i| if i % 4 < 2 { 0 } else { 255 }).collect()).unwrap();
    ed.execute(Command::SetMask { layer: top, mask: Some(left_hidden) }).unwrap();
    let out = ed.render();
    assert_eq!(px(&out, 0, 0), [0, 0, 0, 255], "hidden: the black background shows");
    assert_eq!(px(&out, 3, 0), [255, 0, 0, 255], "visible: the red layer");
}

#[test]
fn mask_of_the_wrong_size_is_refused() {
    let mut ed = white(4, 4);
    let err = ed.execute(Command::SetMask { layer: ed.document().active.unwrap(), mask: Some(Mask::filled(3, 3, 255)) });
    assert!(matches!(err, Err(PhotoError::Invalid(_))));
}

#[test]
fn a_locked_layer_refuses_edits_but_can_be_unlocked() {
    let mut ed = white(4, 4);
    let id = ed.document().active.unwrap();
    ed.execute(Command::SetLocked { layer: id, locked: true }).unwrap();
    let err = ed.execute(Command::SetOpacity { layer: id, opacity: 0.5 });
    assert!(matches!(err, Err(PhotoError::Locked(_))));
    assert_eq!(ed.document().layer(id).unwrap().opacity, 1.0);
    // Visibility is allowed on a locked layer.
    ed.execute(Command::SetVisible { layer: id, visible: false }).unwrap();
    assert!(!ed.document().layer(id).unwrap().visible);
    ed.execute(Command::SetLocked { layer: id, locked: false }).unwrap();
    ed.execute(Command::SetOpacity { layer: id, opacity: 0.5 }).unwrap();
    assert_eq!(ed.document().layer(id).unwrap().opacity, 0.5);
}
