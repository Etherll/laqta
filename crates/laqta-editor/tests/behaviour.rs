#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Undo, export, canvas operations, ordering, the effect cache and the snapshot cap.

mod support;

use laqta_domain::color::Rgba;
use laqta_domain::geometry::PhysRect;
use laqta_editor::{Command, Editor, EditorInput, EditorKey, EditorRequest, Fonts, Mods, PointerButton, Style, Tool};
use support::*;

fn pixels(b: &laqta_domain::Bitmap) -> Vec<u8> {
    b.pixels().to_vec()
}

#[test]
fn undo_redo_round_trips() {
    in_pool(|| {
        let mut e = editor();
        let empty = pixels(&e.export());
        drag(&mut e, (5.0, 5.0), (30.0, 30.0), Mods::default());
        let one = pixels(&e.export());
        assert_ne!(empty, one);
        e.execute(Command::Undo);
        assert_eq!(pixels(&e.export()), empty);
        assert!(e.can_redo());
        e.execute(Command::Redo);
        assert_eq!(pixels(&e.export()), one);
        assert!(e.is_modified());
    });
}

#[test]
fn export_matches_view() {
    in_pool(|| {
        let mut e = editor();
        e.execute(Command::SetTool(Tool::Blur));
        drag(&mut e, (4.0, 4.0), (30.0, 30.0), Mods::default());
        e.execute(Command::SetTool(Tool::Arrow));
        drag(&mut e, (2.0, 40.0), (60.0, 2.0), Mods::default());
        let (w, h) = e.canvas_size();
        let view = e.render(PhysRect { x: 0, y: 0, width: w, height: h });
        assert_eq!(pixels(&view), pixels(&e.export()));
        // A sub-view equals the same region of the export.
        let sub = e.render(PhysRect { x: 10, y: 6, width: 20, height: 12 });
        let full = e.export();
        for y in 0..12u32 {
            for x in 0..20u32 {
                let a = ((y * 20 + x) * 4) as usize;
                let b = (((y + 6) * w + x + 10) * 4) as usize;
                assert_eq!(sub.pixels()[a..a + 4], full.pixels()[b..b + 4]);
            }
        }
    });
}

#[test]
fn crop_and_undo_restore_the_document() {
    in_pool(|| {
        let mut e = editor();
        drag(&mut e, (5.0, 5.0), (20.0, 20.0), Mods::default());
        let before = pixels(&e.export());
        e.execute(Command::Crop(PhysRect { x: 10, y: 10, width: 20, height: 15 }));
        assert_eq!(e.canvas_size(), (20, 15));
        assert_eq!(e.annotations().len(), 0, "crop flattens");
        e.execute(Command::Undo);
        assert_eq!(e.canvas_size(), (64, 48));
        assert_eq!(e.annotations().len(), 1);
        assert_eq!(pixels(&e.export()), before);
    });
}

#[test]
fn canvas_operations() {
    in_pool(|| {
        let mut e = editor();
        e.execute(Command::ResizeCanvas { left: 2, top: 3, right: 4, bottom: 5, color: Rgba::WHITE });
        assert_eq!(e.canvas_size(), (70, 56));
        e.execute(Command::Rotate90 { clockwise: true });
        assert_eq!(e.canvas_size(), (56, 70));
        e.execute(Command::Flip { horizontal: true });
        e.execute(Command::ResizeImage { width: 28, height: 35 });
        assert_eq!(e.canvas_size(), (28, 35));
        for _ in 0..4 {
            e.execute(Command::Undo);
        }
        assert_eq!(e.canvas_size(), (64, 48));
    });
}

#[test]
fn auto_crop_trims_the_added_margin() {
    in_pool(|| {
        let mut e = editor();
        e.execute(Command::ResizeCanvas { left: 5, top: 5, right: 5, bottom: 5, color: Rgba::rgb(1, 2, 3) });
        e.execute(Command::AutoCrop);
        assert_eq!(e.canvas_size(), (64, 48));
    });
}

#[test]
fn nudge_order_delete_duplicate() {
    in_pool(|| {
        let mut e = editor();
        drag(&mut e, (5.0, 5.0), (20.0, 20.0), Mods::default());
        drag(&mut e, (30.0, 5.0), (50.0, 20.0), Mods::default());
        let first = e.annotations()[0].id;
        // The second is selected; Tab cycles to the first.
        e.handle(EditorInput::Key { key: EditorKey::Tab, mods: Mods::default() });
        assert_eq!(e.chrome().selection.map(|r| r.x), Some(5.0));
        e.handle(EditorInput::Key { key: EditorKey::Right, mods: Mods { shift: true, ..Mods::default() } });
        assert_eq!(e.annotations()[0].kind.rect().unwrap().x, 15.0);
        e.handle(EditorInput::Key { key: EditorKey::Home, mods: Mods::default() });
        assert_eq!(e.annotations().last().unwrap().id, first);
        e.execute(Command::Duplicate);
        assert_eq!(e.annotations().len(), 3);
        e.handle(EditorInput::Key { key: EditorKey::Delete, mods: Mods::default() });
        assert_eq!(e.annotations().len(), 2);
        e.handle(EditorInput::Key { key: EditorKey::Char('z'), mods: Mods { ctrl: true, ..Mods::default() } });
        assert_eq!(e.annotations().len(), 3);
    });
}

#[test]
fn effect_cache_hits_and_invalidates_on_move() {
    in_pool(|| {
        let mut e = editor();
        e.execute(Command::SetTool(Tool::Pixelate));
        drag(&mut e, (4.0, 4.0), (30.0, 30.0), Mods::default());
        e.export();
        let after_first = e.effect_cache_misses();
        e.export();
        assert_eq!(e.effect_cache_misses(), after_first, "second render hits the cache");
        e.execute(Command::Nudge { dx: 3.0, dy: 0.0 });
        e.export();
        assert_eq!(e.effect_cache_misses(), after_first + 1, "moving invalidates");
    });
}

#[test]
fn snapshot_cap_drops_oldest() {
    in_pool(|| {
        let mut e = editor();
        let one = 64 * 48 * 4;
        e.set_snapshot_cap(one * 4 + 10);
        for _ in 0..4 {
            e.execute(Command::Flip { horizontal: true });
        }
        assert!(e.snapshot_bytes() <= one * 4 + 10);
        let mut undos = 0;
        while e.can_undo() {
            e.execute(Command::Undo);
            undos += 1;
        }
        assert_eq!(undos, 2, "only the two newest destructive entries fit");
    });
}

#[test]
fn keyboard_map() {
    in_pool(|| {
        let mut e = editor();
        for (c, tool) in
            [('v', Tool::Select), ('r', Tool::Rectangle), ('a', Tool::Arrow), ('b', Tool::Blur), ('c', Tool::Crop), ('9', Tool::Step)]
        {
            e.handle(EditorInput::Key { key: EditorKey::Char(c), mods: Mods::default() });
            assert_eq!(e.chrome().tool, tool);
        }
        let ctrl = Mods { ctrl: true, ..Mods::default() };
        assert_eq!(e.handle(EditorInput::Key { key: EditorKey::Char('s'), mods: ctrl }).request, Some(EditorRequest::Save));
        let shift_ctrl = Mods { shift: true, ..ctrl };
        assert_eq!(e.handle(EditorInput::Key { key: EditorKey::Char('s'), mods: shift_ctrl }).request, Some(EditorRequest::SaveAs));
        assert_eq!(e.handle(EditorInput::Key { key: EditorKey::Enter, mods: Mods::default() }).request, Some(EditorRequest::Continue));
    });
}

#[test]
fn resize_handle_and_dirty_rects() {
    in_pool(|| {
        let mut e = editor();
        drag(&mut e, (10.0, 10.0), (20.0, 20.0), Mods::default());
        // Drag the SE handle of the selected rectangle.
        e.handle(EditorInput::PointerDown { x: 20.0, y: 20.0, button: PointerButton::Left, mods: Mods::default() });
        let out = e.handle(EditorInput::PointerMove { x: 30.0, y: 34.0, mods: Mods::default() });
        assert!(!out.dirty.is_empty(), "a live resize repaints");
        e.handle(EditorInput::PointerUp { x: 30.0, y: 34.0, button: PointerButton::Left, mods: Mods::default() });
        let r = e.annotations()[0].kind.rect().unwrap();
        assert_eq!((r.w, r.h), (20.0, 24.0));
        // Chrome-only changes don't repaint.
        let out = e.execute(Command::Deselect);
        assert!(out.dirty.is_empty());
    });
}

#[test]
fn style_applies_to_the_selection() {
    in_pool(|| {
        let mut e = editor();
        drag(&mut e, (10.0, 10.0), (20.0, 20.0), Mods::default());
        let s = Style { border_color: Rgba::rgb(0, 0, 255), ..Style::default() };
        e.execute(Command::SetStyle(s));
        assert_eq!(e.annotations()[0].style.border_color, Rgba::rgb(0, 0, 255));
    });
}

#[test]
fn copy_paste_and_paste_image() {
    in_pool(|| {
        let mut e = editor();
        drag(&mut e, (10.0, 10.0), (20.0, 20.0), Mods::default());
        let ctrl = Mods { ctrl: true, ..Mods::default() };
        e.handle(EditorInput::Key { key: EditorKey::Char('c'), mods: ctrl });
        e.handle(EditorInput::Key { key: EditorKey::Char('v'), mods: ctrl });
        assert_eq!(e.annotations().len(), 2);
        e.execute(Command::Paste(small_image([255, 0, 0, 255])));
        assert_eq!(e.annotations().len(), 3);
    });
}

#[test]
fn text_with_a_real_font_when_available() {
    let candidates = [
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
    ];
    let Some(path) = candidates.into_iter().find(|p| std::path::Path::new(p).exists()) else {
        eprintln!("skipped: no test font installed");
        return;
    };
    let data = std::fs::read(path).unwrap();
    let fonts = Fonts::new(std::sync::Arc::new(move || {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_font_data(data.clone());
        (db, "en-US".to_string())
    }));
    in_pool(|| {
        let mut e = Editor::new(base(), fonts);
        e.execute(Command::SetTool(Tool::Text));
        drag(&mut e, (2.0, 2.0), (62.0, 40.0), Mods::default());
        let empty = pixels(&e.export());
        e.handle(EditorInput::Text("Laqta".into()));
        assert_ne!(pixels(&e.export()), empty, "text drew pixels");
    });
}
