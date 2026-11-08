#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Audit probes for the M3 editor. `audit_bug_*` tests fail on purpose where Laqta diverges from
//! the expected editor behaviour; `audit_ok_*` tests are independent pixel checks (not self-snapshots).

mod support;

use laqta_domain::Bitmap;
use laqta_domain::geometry::PhysRect;
use laqta_editor::{AnnotationKind, Command, EditorInput, EditorKey, Mods, Tool};
use support::*;

fn px(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * b.width() + x) * 4) as usize;
    b.pixels()[i..i + 4].try_into().unwrap()
}

fn steps(e: &laqta_editor::Editor) -> Vec<i32> {
    e.annotations()
        .iter()
        .filter_map(|a| match a.kind {
            AnnotationKind::Step { number, .. } => Some(number),
            _ => None,
        })
        .collect()
}

/// ShareX SmartEraserAnnotation: "the color sampled when drawing started". Laqta samples the
/// top-left of the normalised rect, which is the start point only for a down-right drag.
#[test]
fn audit_bug_smart_eraser_samples_drag_start() {
    in_pool(|| {
        let base = base();
        let mut e = editor();
        e.execute(Command::SetTool(Tool::SmartEraser));
        drag(&mut e, (40.0, 30.0), (10.0, 8.0), Mods::default());
        let AnnotationKind::SmartEraser { color, .. } = e.annotations()[0].kind else { panic!() };
        let start = px(&base, 40, 30);
        assert_eq!([color.r, color.g, color.b, color.a], start, "eraser colour should come from the drag start (40,30)");
    });
}

/// ShareX EditorCore.RemoveAnnotation → HandleStepRenumbering: higher steps shift down by one.
#[test]
fn audit_bug_step_renumbers_after_delete() {
    in_pool(|| {
        let mut e = editor();
        e.execute(Command::SetTool(Tool::Step));
        click(&mut e, (10.0, 10.0));
        click(&mut e, (30.0, 24.0));
        click(&mut e, (54.0, 38.0));
        assert_eq!(steps(&e), vec![1, 2, 3]);
        e.execute(Command::SetTool(Tool::Select));
        click(&mut e, (30.0, 24.0));
        e.execute(Command::Delete);
        assert_eq!(steps(&e), vec![1, 2], "ShareX renumbers 3 → 2 after deleting step 2");
    });
}

/// ShareX MainViewModel.RecalculateNumberCounter runs on every history change: the next step is
/// max(existing)+1. After undoing the only step, the next one is 1 again.
#[test]
fn audit_bug_step_counter_after_undo() {
    in_pool(|| {
        let mut e = editor();
        e.execute(Command::SetTool(Tool::Step));
        click(&mut e, (10.0, 10.0));
        e.execute(Command::Undo);
        click(&mut e, (30.0, 24.0));
        assert_eq!(steps(&e), vec![1], "ShareX recalculates the counter from the remaining steps");
    });
}

/// ShareX keeps annotations editable through rotate/flip/resize/canvas/auto-crop/effects
/// (ApplyImageOperation with transformAnnotations). Laqta flattens them into the base.
#[test]
fn audit_bug_rotate_keeps_annotations_editable() {
    in_pool(|| {
        let mut e = editor();
        e.execute(Command::SetTool(Tool::Rectangle));
        drag(&mut e, (5.0, 5.0), (25.0, 20.0), Mods::default());
        e.execute(Command::Rotate90 { clockwise: true });
        assert_eq!(e.annotations().len(), 1, "the rectangle should survive the rotate as an annotation");
    });
}

/// ShareX discards a text box that is left empty (EditorInputController OnCreationLostFocus).
#[test]
fn audit_bug_empty_text_box_is_kept() {
    in_pool(|| {
        let mut e = editor();
        e.execute(Command::SetTool(Tool::Text));
        drag(&mut e, (4.0, 4.0), (60.0, 30.0), Mods::default());
        e.handle(EditorInput::Key { key: EditorKey::Escape, mods: Mods::default() });
        assert_eq!(e.annotations().len(), 0, "an empty text box should not stay as an invisible annotation");
    });
}

// ---------------------------------------------------------------- independent pixel checks

#[test]
fn audit_ok_rectangle_border_at_known_pixels() {
    in_pool(|| {
        let base = base();
        let mut e = editor();
        e.execute(Command::SetTool(Tool::Rectangle));
        drag(&mut e, (10.0, 10.0), (40.0, 30.0), Mods::default());
        let out = e.export();
        // Default border: #ef4444, 4 px, centred on the edge (8..12 on the left edge).
        assert_eq!(px(&out, 10, 20), [0xef, 0x44, 0x44, 255]);
        assert_eq!(px(&out, 25, 20), px(&base, 25, 20), "interior untouched (no fill)");
        assert_eq!(px(&out, 2, 2), px(&base, 2, 2), "outside untouched");
    });
}

#[test]
fn audit_ok_blur_and_pixelate_stay_inside_their_rect() {
    in_pool(|| {
        let base = base();
        let mut e = editor();
        e.execute(Command::SetTool(Tool::Blur));
        drag(&mut e, (4.0, 4.0), (30.0, 30.0), Mods::default());
        let out = e.export();
        assert_ne!(px(&out, 16, 12), px(&base, 16, 12), "a block corner inside the blur changes");
        assert_eq!(px(&out, 50, 40), px(&base, 50, 40), "outside the blur is unchanged");

        let mut e = editor();
        e.execute(Command::SetTool(Tool::Pixelate));
        drag(&mut e, (0.0, 0.0), (20.0, 20.0), Mods::default());
        let out = e.export();
        assert_ne!(px(&base, 0, 0), px(&base, 9, 9));
        assert_eq!(px(&out, 0, 0), px(&out, 9, 9), "one 10 px cell is uniform");
    });
}

#[test]
fn audit_ok_crop_and_rotate_map_pixels() {
    in_pool(|| {
        let base = base();
        let mut e = editor();
        e.execute(Command::Crop(PhysRect { x: 8, y: 6, width: 32, height: 24 }));
        assert_eq!(px(&e.export(), 0, 0), px(&base, 8, 6));
        assert_eq!(px(&e.export(), 31, 23), px(&base, 39, 29));

        let mut e = editor();
        e.execute(Command::Rotate90 { clockwise: true });
        let out = e.export();
        assert_eq!((out.width(), out.height()), (48, 64));
        // Clockwise: (x, y) → (h - 1 - y, x).
        assert_eq!(px(&out, 47, 0), px(&base, 0, 0));
        assert_eq!(px(&out, 47 - 5, 7), px(&base, 7, 5));
    });
}
