#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Pointer interaction against the snapshot render: hover repaints, the draft shape in a
//! render but not an export, and a render job that ignores later edits.

mod support;

use laqta_domain::{Bitmap, PixelFormat};
use laqta_editor::{Command, Editor, EditorInput, Fonts, Mods, PointerButton, Tool};
use support::in_pool;

const GREY: [u8; 4] = [128, 128, 128, 255];

fn base() -> Bitmap {
    let (w, h) = (2560u32, 1440u32);
    let px: Vec<u8> = std::iter::repeat_n(GREY, (w * h) as usize).flatten().collect();
    Bitmap::new(w, h, PixelFormat::Rgba8, px.into()).unwrap()
}

fn editor() -> Editor {
    Editor::new(base(), Fonts::empty())
}

fn pixel(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * b.width() + x) * 4) as usize;
    b.pixels()[i..i + 4].try_into().unwrap()
}

fn press(e: &mut Editor, (x, y): (f32, f32)) {
    e.handle(EditorInput::PointerDown { x, y, button: PointerButton::Left, mods: Mods::default() });
}

fn move_to(e: &mut Editor, (x, y): (f32, f32)) {
    e.handle(EditorInput::PointerMove { x, y, mods: Mods::default() });
}

fn release(e: &mut Editor, (x, y): (f32, f32)) {
    e.handle(EditorInput::PointerUp { x, y, button: PointerButton::Left, mods: Mods::default() });
}

#[test]
fn hover_moves_repaint_nothing() {
    in_pool(|| {
        let mut e = editor();
        for i in 0..25 {
            let from = (100.0 + 20.0 * i as f32, 100.0);
            let to = (from.0 + 10.0, 110.0);
            press(&mut e, from);
            move_to(&mut e, to);
            release(&mut e, to);
        }
        assert_eq!(e.annotations().len(), 25);
        e.execute(Command::SetTool(Tool::Select));
        for k in 0..300 {
            let x = 2000.0 + (k % 200) as f32;
            let out = e.handle(EditorInput::PointerMove { x, y: 1300.0, mods: Mods::default() });
            assert!(out.dirty.is_empty(), "hover move {k} dirtied {:?}", out.dirty);
        }
    });
}

#[test]
fn draft_shows_in_render_but_not_export() {
    in_pool(|| {
        let mut e = editor();
        press(&mut e, (500.0, 500.0));
        move_to(&mut e, (900.0, 800.0));
        let rendered = e.render_job().run();
        let exported = e.export();
        assert_ne!(pixel(&rendered, 500, 500), GREY, "the draft is missing from the render");
        assert_eq!(pixel(&exported, 500, 500), GREY, "the draft leaked into the export");
    });
}

#[test]
fn export_matches_render_after_commit() {
    in_pool(|| {
        let mut e = editor();
        press(&mut e, (500.0, 500.0));
        move_to(&mut e, (900.0, 800.0));
        release(&mut e, (900.0, 800.0));
        let exported = e.export();
        let rendered = e.render_job().run();
        assert!(exported.pixels() == rendered.pixels(), "export and render differ after commit");
    });
}

#[test]
fn render_job_ignores_later_edits() {
    in_pool(|| {
        let mut e = editor();
        press(&mut e, (500.0, 500.0));
        move_to(&mut e, (900.0, 800.0));
        let job = e.render_job();
        release(&mut e, (900.0, 800.0));
        press(&mut e, (1500.0, 1000.0));
        move_to(&mut e, (1600.0, 1100.0));
        release(&mut e, (1600.0, 1100.0));
        assert_eq!(pixel(&job.run(), 1500, 1000), GREY, "the job saw a rectangle drawn after it was taken");
        assert_ne!(pixel(&e.render_job().run(), 1500, 1000), GREY, "a fresh render misses the new rectangle");
    });
}
