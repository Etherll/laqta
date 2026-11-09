#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! `Command::ReplaceImage` and the Object eraser tool, plus the tool key map.

mod support;

use laqta_domain::geometry::PhysRect;
use laqta_domain::{Bitmap, PixelFormat};
use laqta_editor::{Command, EditorInput, EditorKey, EditorOutput, EditorRequest, Mods, PointerButton, RectF, Tool};
use support::*;

fn solid(w: u32, h: u32, rgba: [u8; 4], format: PixelFormat) -> Bitmap {
    let px: Vec<u8> = std::iter::repeat_n(rgba, (w * h) as usize).flatten().collect();
    Bitmap::new(w, h, format, px.into()).unwrap()
}

fn pixels(b: &Bitmap) -> Vec<u8> {
    b.pixels().to_vec()
}

fn down(e: &mut laqta_editor::Editor, at: (f32, f32)) -> EditorOutput {
    e.handle(EditorInput::PointerDown { x: at.0, y: at.1, button: PointerButton::Left, mods: Mods::default() })
}

fn moved(e: &mut laqta_editor::Editor, at: (f32, f32)) -> EditorOutput {
    e.handle(EditorInput::PointerMove { x: at.0, y: at.1, mods: Mods::default() })
}

fn up(e: &mut laqta_editor::Editor, at: (f32, f32)) -> EditorOutput {
    e.handle(EditorInput::PointerUp { x: at.0, y: at.1, button: PointerButton::Left, mods: Mods::default() })
}

/// A full drag with the Object eraser; returns the output of the release.
fn erase_drag(e: &mut laqta_editor::Editor, from: (f32, f32), to: (f32, f32)) -> EditorOutput {
    e.execute(Command::SetTool(Tool::ObjectEraser));
    down(e, from);
    moved(e, to);
    up(e, to)
}

fn erase_rect(out: &EditorOutput) -> Option<PhysRect> {
    match out.request {
        Some(EditorRequest::EraseObject { rect }) => Some(rect),
        _ => None,
    }
}

// ---------------------------------------------------------------- ReplaceImage

#[test]
fn replace_image_swaps_the_base_and_one_undo_restores_everything() {
    in_pool(|| {
        let mut e = editor();
        drag(&mut e, (5.0, 5.0), (30.0, 30.0), Mods::default());
        assert_eq!(e.annotations().len(), 1);
        let old = pixels(&e.export());

        let new = solid(20, 10, [10, 200, 30, 255], PixelFormat::Rgba8);
        let out = e.execute(Command::ReplaceImage(new.clone()));
        assert_eq!(e.canvas_size(), (20, 10));
        assert_eq!(out.chrome.canvas, (20, 10));
        assert_eq!(out.dirty, vec![PhysRect { x: 0, y: 0, width: 20, height: 10 }], "the whole new canvas repaints");
        assert_eq!(e.annotations().len(), 0, "annotations are flattened away");
        assert_eq!(pixels(&e.export()), pixels(&new));
        assert!(e.is_modified());

        e.execute(Command::Undo);
        assert_eq!(e.canvas_size(), (64, 48));
        assert_eq!(e.annotations().len(), 1, "the annotation is back");
        assert_eq!(pixels(&e.export()), old);

        e.execute(Command::Redo);
        assert_eq!(e.canvas_size(), (20, 10));
        assert_eq!(e.annotations().len(), 0);
        assert_eq!(pixels(&e.export()), pixels(&new));
    });
}

#[test]
fn replace_image_is_a_single_undo_step() {
    in_pool(|| {
        let mut e = editor();
        let old = pixels(&e.export());
        e.execute(Command::ReplaceImage(solid(16, 16, [1, 2, 3, 255], PixelFormat::Rgba8)));
        assert!(e.can_undo());
        e.execute(Command::Undo);
        assert!(!e.can_undo(), "one undo step, nothing else was recorded");
        assert_eq!(e.canvas_size(), (64, 48));
        assert_eq!(pixels(&e.export()), old);
    });
}

#[test]
fn replace_image_reads_bgra_input() {
    in_pool(|| {
        let mut e = editor();
        e.execute(Command::ReplaceImage(solid(4, 4, [30, 20, 10, 255], PixelFormat::Bgra8)));
        let out = e.export();
        assert_eq!(&out.pixels()[..4], &[10, 20, 30, 255], "BGRA is converted to the engine's RGBA");
    });
}

#[test]
fn replace_image_counts_against_the_snapshot_cap() {
    in_pool(|| {
        let mut e = editor();
        e.execute(Command::ReplaceImage(solid(8, 8, [9, 9, 9, 255], PixelFormat::Rgba8)));
        assert!(e.snapshot_bytes() > 0, "the old and the new base are kept as an undo snapshot");
    });
}

// ---------------------------------------------------------------- Object eraser

#[test]
fn object_eraser_emits_the_image_pixel_rect_and_leaves_nothing_behind() {
    in_pool(|| {
        let mut e = editor();
        let before = pixels(&e.export());
        let out = erase_drag(&mut e, (10.0, 8.0), (30.0, 20.0));
        assert_eq!(erase_rect(&out), Some(PhysRect { x: 10, y: 8, width: 20, height: 12 }));
        assert_eq!(e.annotations().len(), 0);
        assert!(!e.can_undo(), "no history entry");
        assert!(!e.is_modified());
        assert_eq!(pixels(&e.export()), before, "the pixels are the host's to change");
        assert_eq!(out.chrome.selection, None, "the rubber band is gone after release");
        assert_eq!(out.chrome.annotation_count, 0);
    });
}

#[test]
fn object_eraser_shows_a_rubber_band_while_dragging() {
    in_pool(|| {
        let mut e = editor();
        e.execute(Command::SetTool(Tool::ObjectEraser));
        let start = down(&mut e, (10.0, 8.0));
        assert_eq!(start.request, None);
        let mid = moved(&mut e, (30.0, 20.0));
        assert_eq!(mid.chrome.selection, Some(RectF::new(10.0, 8.0, 20.0, 12.0)));
        assert!(mid.dirty.is_empty(), "chrome only, no canvas repaint");
        assert!(mid.chrome.handles.is_empty());
        let back = moved(&mut e, (4.0, 2.0));
        assert_eq!(back.chrome.selection, Some(RectF::new(4.0, 2.0, 6.0, 6.0)), "dragging up-left normalises the box");
        let end = up(&mut e, (4.0, 2.0));
        assert_eq!(end.chrome.selection, None);
    });
}

#[test]
fn object_eraser_rect_is_clamped_to_the_image() {
    in_pool(|| {
        let mut e = editor();
        let out = erase_drag(&mut e, (-10.0, -5.0), (100.0, 60.0));
        assert_eq!(erase_rect(&out), Some(PhysRect { x: 0, y: 0, width: 64, height: 48 }));
        let out = erase_drag(&mut e, (50.0, 40.0), (90.0, 70.0));
        assert_eq!(erase_rect(&out), Some(PhysRect { x: 50, y: 40, width: 14, height: 8 }));
        let out = erase_drag(&mut e, (30.0, 20.0), (10.0, 8.0));
        assert_eq!(erase_rect(&out), Some(PhysRect { x: 10, y: 8, width: 20, height: 12 }), "drag direction does not matter");
    });
}

#[test]
fn object_eraser_ignores_empty_and_outside_boxes() {
    in_pool(|| {
        let mut e = editor();
        assert_eq!(erase_rect(&erase_drag(&mut e, (20.0, 20.0), (20.0, 20.0))), None, "a click");
        assert_eq!(erase_rect(&erase_drag(&mut e, (20.0, 20.0), (21.0, 21.0))), None, "a one pixel nudge");
        assert_eq!(erase_rect(&erase_drag(&mut e, (20.0, 20.0), (20.0, 40.0))), None, "a zero-width line");
        assert_eq!(erase_rect(&erase_drag(&mut e, (70.0, 10.0), (90.0, 30.0))), None, "entirely right of the image");
        assert_eq!(erase_rect(&erase_drag(&mut e, (-30.0, -30.0), (-5.0, -5.0))), None, "entirely above and left");
        assert!(!e.can_undo());
    });
}

#[test]
fn object_eraser_gesture_can_be_cancelled() {
    in_pool(|| {
        let mut e = editor();
        e.execute(Command::SetTool(Tool::ObjectEraser));
        down(&mut e, (10.0, 8.0));
        moved(&mut e, (30.0, 20.0));
        let esc = e.handle(EditorInput::Key { key: EditorKey::Escape, mods: Mods::default() });
        assert_eq!(esc.chrome.selection, None);
        assert_eq!(erase_rect(&up(&mut e, (30.0, 20.0))), None, "Escape cancelled the drag");

        down(&mut e, (10.0, 8.0));
        moved(&mut e, (30.0, 20.0));
        let right = e.handle(EditorInput::PointerDown { x: 30.0, y: 20.0, button: PointerButton::Right, mods: Mods::default() });
        assert_eq!(right.chrome.selection, None);
        assert_eq!(erase_rect(&up(&mut e, (30.0, 20.0))), None, "a right click cancelled the drag");
    });
}

#[test]
fn erase_request_then_replace_image_is_one_undo_step() {
    in_pool(|| {
        let mut e = editor();
        let old = pixels(&e.export());
        let out = erase_drag(&mut e, (10.0, 8.0), (30.0, 20.0));
        let rect = erase_rect(&out).unwrap();
        // The host erased `rect` and hands back the same-size image.
        let erased = solid(64, 48, [0, 0, 0, 255], PixelFormat::Rgba8);
        e.execute(Command::ReplaceImage(erased.clone()));
        assert_eq!(rect.width * rect.height, 240);
        assert_eq!(pixels(&e.export()), pixels(&erased));
        e.execute(Command::Undo);
        assert_eq!(pixels(&e.export()), old);
        assert!(!e.can_undo());
    });
}

// ---------------------------------------------------------------- key map

#[test]
fn tool_shortcuts_round_trip_and_are_unique() {
    assert_eq!(Tool::ALL.len(), 22);
    let mut seen = Vec::new();
    for t in Tool::ALL {
        if let Some(k) = t.shortcut() {
            assert!(!seen.contains(&k), "{k} is used twice");
            seen.push(k);
            assert_eq!(Tool::from_key(k), Some(t), "{t:?}");
        }
    }
    assert_eq!(Tool::ObjectEraser.shortcut(), None);
    assert_eq!(Tool::from_key('z'), None);
    assert_eq!(Tool::from_key('9'), Some(Tool::Step));
}

#[test]
fn the_engine_key_map_is_the_tool_table() {
    in_pool(|| {
        let mut e = editor();
        for c in ('a'..='z').chain('0'..='9') {
            e.execute(Command::SetTool(Tool::ObjectEraser));
            let out = e.handle(EditorInput::Key { key: EditorKey::Char(c), mods: Mods::default() });
            let expected = Tool::from_key(c).unwrap_or(Tool::ObjectEraser);
            assert_eq!(out.chrome.tool, expected, "key {c}");
        }
        for (c, tool) in [('b', Tool::Blur), ('p', Tool::Pixelate), ('s', Tool::SpeechBalloon), ('f', Tool::Freehand), ('i', Tool::Step)] {
            e.handle(EditorInput::Key { key: EditorKey::Char(c), mods: Mods::default() });
            assert_eq!(e.chrome().tool, tool, "key {c}");
        }
    });
}
