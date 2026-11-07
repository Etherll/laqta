#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]
//! Undo and redo: round trips through every command kind, redo restoring the same layer id, the
//! byte cap that drops the oldest entries, and canvas resizes.

mod support;

use laqta_domain::color::Rgba;
use laqta_photo::{
    BlendMode, Brush, BrushTool, Command, Direction, LayerKind, PhotoEditor, SelectOp, SelectShape,
};

use support::{px, solid};

fn editor() -> PhotoEditor {
    PhotoEditor::from_bitmap(solid(8, 8, [255, 255, 255, 255]))
}

fn red_brush() -> Brush {
    Brush { size: 4.0, hardness: 1.0, opacity: 1.0, spacing: 0.25, color: Rgba { r: 255, g: 0, b: 0, a: 255 } }
}

#[test]
fn every_command_kind_round_trips() {
    let mut ed = editor();
    let base = ed.document().active.unwrap();
    let original = ed.render();

    ed.execute(Command::AddLayer { name: "paint".into(), kind: LayerKind::Raster(solid(8, 8, [0, 0, 0, 0])) }).unwrap();
    let top = ed.document().active.unwrap();
    ed.execute(Command::Paint { layer: top, path: vec![(1.0, 1.0), (6.0, 6.0)], brush: red_brush(), tool: BrushTool::Paint })
        .unwrap();
    ed.execute(Command::SetOpacity { layer: top, opacity: 0.5 }).unwrap();
    ed.execute(Command::SetBlend { layer: top, blend: BlendMode::Multiply }).unwrap();
    ed.execute(Command::RenameLayer { layer: top, name: "renamed".into() }).unwrap();
    ed.execute(Command::Select { shape: SelectShape::Rect { x: 0.0, y: 0.0, width: 4.0, height: 8.0 }, op: SelectOp::Replace })
        .unwrap();
    ed.execute(Command::FloodFill { layer: base, x: 7, y: 7, tolerance: 0, color: Rgba::BLACK, contiguous: true }).unwrap();
    ed.execute(Command::DuplicateLayer { layer: top }).unwrap();
    ed.execute(Command::MoveLayer { layer: top, direction: Direction::Up }).unwrap();
    ed.execute(Command::MergeDown { layer: top }).unwrap();
    let after = ed.render();
    assert_eq!(ed.document().layers.len(), 2);

    let mut steps = 0;
    while ed.undo() {
        steps += 1;
    }
    assert_eq!(steps, 10, "each of the ten commands is one undo step");
    assert_eq!(ed.render().pixels(), original.pixels(), "undo all restores the original");
    assert_eq!(ed.document().layers.len(), 1);

    while ed.redo() {}
    assert_eq!(ed.render().pixels(), after.pixels(), "redo all restores the result");
}

#[test]
fn redo_reinserts_the_same_layer_id() {
    let mut ed = editor();
    ed.execute(Command::AddLayer { name: "x".into(), kind: LayerKind::Raster(solid(8, 8, [0, 0, 0, 255])) }).unwrap();
    let id = ed.document().active.unwrap();
    assert!(ed.undo());
    assert!(ed.document().layer(id).is_none());
    assert!(ed.redo());
    assert_eq!(ed.document().layer(id).unwrap().name, "x");
}

#[test]
fn a_new_command_clears_the_redo_stack() {
    let mut ed = editor();
    let bg = ed.document().active.unwrap();
    ed.execute(Command::SetOpacity { layer: bg, opacity: 0.5 }).unwrap();
    assert!(ed.undo());
    assert!(ed.can_redo());
    ed.execute(Command::SetOpacity { layer: bg, opacity: 0.25 }).unwrap();
    assert!(!ed.can_redo());
}

#[test]
fn undo_with_nothing_to_undo_is_false() {
    let mut ed = editor();
    assert!(!ed.can_undo());
    assert!(!ed.undo());
    assert!(!ed.redo());
}

#[test]
fn property_edits_hold_no_pixels_but_paint_does() {
    let mut ed = editor();
    let bg = ed.document().active.unwrap();
    ed.execute(Command::SetOpacity { layer: bg, opacity: 0.5 }).unwrap();
    ed.execute(Command::SetVisible { layer: bg, visible: false }).unwrap();
    assert_eq!(ed.snapshot_bytes(), 0);
    ed.execute(Command::Paint { layer: bg, path: vec![(4.0, 4.0)], brush: red_brush(), tool: BrushTool::Paint }).unwrap();
    assert!(ed.snapshot_bytes() > 0);
}

#[test]
fn the_cap_drops_the_oldest_entries_first() {
    let mut ed = PhotoEditor::from_bitmap(solid(64, 64, [255, 255, 255, 255]));
    let bg = ed.document().active.unwrap();
    // A stroke across the whole layer keeps about 2 * 64 * 64 * 4 bytes per entry.
    ed.set_snapshot_cap(40 * 1024);
    let sweep = Brush { size: 30.0, ..red_brush() };
    for y in [8.0, 24.0, 40.0, 56.0] {
        ed.execute(Command::Paint { layer: bg, path: vec![(0.0, y), (63.0, y)], brush: sweep, tool: BrushTool::Paint })
            .unwrap();
    }
    assert!(ed.snapshot_bytes() <= 40 * 1024, "held {}", ed.snapshot_bytes());
    let mut undone = 0;
    while ed.undo() {
        undone += 1;
    }
    assert!(undone < 4, "the oldest entries were dropped: undid {undone} of 4");
    assert!(undone >= 1, "the newest entry is kept");
}

#[test]
fn canvas_resize_and_undo() {
    let mut ed = PhotoEditor::from_bitmap(solid(4, 4, [9, 9, 9, 255]));
    let bg = ed.document().active.unwrap();
    ed.execute(Command::ResizeCanvas { width: 6, height: 6, offset: (1, 1) }).unwrap();
    assert_eq!((ed.render().width(), ed.render().height()), (6, 6));
    assert_eq!(ed.document().layer(bg).unwrap().offset, (1, 1));
    assert_eq!(px(&ed.render(), 0, 0)[3], 0, "the new border is transparent");
    assert!(ed.undo());
    assert_eq!((ed.document().width, ed.document().height), (4, 4));
    assert_eq!(ed.document().layer(bg).unwrap().offset, (0, 0));
}

#[test]
fn crop_moves_the_content_and_undoes() {
    let mut ed = PhotoEditor::from_bitmap(support::from_fn(8, 8, |x, _| [x as u8 * 10, 0, 0, 255]));
    ed.execute(Command::Crop { x: 2, y: 0, width: 4, height: 8 }).unwrap();
    assert_eq!(px(&ed.render(), 0, 0)[0], 20, "pixel x=2 is now at 0");
    assert!(ed.undo());
    assert_eq!(ed.render().width(), 8);
}

#[test]
fn a_no_op_command_is_not_recorded() {
    let mut ed = editor();
    let bg = ed.document().active.unwrap();
    // The background is the bottom layer, so it cannot move down.
    ed.execute(Command::MoveLayer { layer: bg, direction: Direction::Down }).unwrap();
    assert!(!ed.can_undo());
}
