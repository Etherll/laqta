#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! One input script per tool → export → snapshot.

mod support;

use laqta_editor::{AnnotationKind, Command, Editor, EditorInput, EditorKey, Mods, PointerButton, Style, Tool};
use support::*;

fn run(tool: Tool, script: impl FnOnce(&mut Editor) + Send) -> Editor {
    in_pool(|| {
        let mut e = editor();
        let style = Style { border_size: 3.0, shadow: true, ..Style::default() };
        e.execute(Command::SetStyle(style));
        e.execute(Command::SetTool(tool));
        script(&mut e);
        e
    })
}

fn check(tool: Tool, name: &str, script: impl FnOnce(&mut Editor) + Send) {
    let e = run(tool, script);
    let out = in_pool(|| e.export());
    snapshot(&format!("tool_{name}"), &out);
}

#[test]
fn every_tool_is_listed() {
    assert_eq!(Tool::ALL.len(), 22);
}

#[test]
fn select_moves_an_annotation() {
    check(Tool::Rectangle, "select", |e| {
        drag(e, (5.0, 5.0), (25.0, 20.0), Mods::default());
        e.execute(Command::SetTool(Tool::Select));
        // From the interior (handles sit on the edges and would resize instead).
        drag(e, (15.0, 13.0), (35.0, 31.0), Mods::default());
        assert_eq!(e.annotations().len(), 1);
        let r = e.annotations()[0].kind.rect().unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (25.0, 23.0, 20.0, 15.0));
    });
}

#[test]
fn rectangle() {
    check(Tool::Rectangle, "rectangle", |e| drag(e, (6.0, 6.0), (40.0, 30.0), Mods::default()));
}

#[test]
fn rectangle_shift_is_square() {
    let e = run(Tool::Rectangle, |e| drag(e, (6.0, 6.0), (40.0, 20.0), Mods { shift: true, ..Mods::default() }));
    let r = e.annotations()[0].kind.rect().unwrap();
    assert_eq!(r.w, r.h);
}

#[test]
fn ellipse() {
    check(Tool::Ellipse, "ellipse", |e| drag(e, (6.0, 6.0), (50.0, 40.0), Mods::default()));
}

#[test]
fn line() {
    check(Tool::Line, "line", |e| drag(e, (4.0, 40.0), (60.0, 8.0), Mods::default()));
}

#[test]
fn arrow() {
    check(Tool::Arrow, "arrow", |e| drag(e, (4.0, 40.0), (58.0, 10.0), Mods::default()));
}

#[test]
fn highlight_is_a_marker_stroke() {
    check(Tool::Highlight, "highlight", |e| {
        e.handle(EditorInput::PointerDown { x: 6.0, y: 20.0, button: PointerButton::Left, mods: Mods::default() });
        for i in 1..18 {
            e.handle(EditorInput::PointerMove { x: 6.0 + i as f32 * 3.0, y: 20.0 + (i as f32 * 0.4).sin() * 6.0, mods: Mods::default() });
        }
        e.handle(EditorInput::PointerUp { x: 58.0, y: 22.0, button: PointerButton::Left, mods: Mods::default() });
        assert_eq!(e.annotations().len(), 1);
        assert!(matches!(e.annotations()[0].kind, AnnotationKind::Highlight { .. }));
    });
}

#[test]
fn shift_makes_a_straight_stroke() {
    let shift = Mods { shift: true, ..Mods::default() };
    let e = run(Tool::Highlight, |e| {
        e.handle(EditorInput::PointerDown { x: 6.0, y: 20.0, button: PointerButton::Left, mods: shift });
        for i in 1..10 {
            e.handle(EditorInput::PointerMove { x: 6.0 + i as f32 * 5.0, y: 20.0 + (i % 3) as f32, mods: shift });
        }
        e.handle(EditorInput::PointerUp { x: 56.0, y: 22.0, button: PointerButton::Left, mods: shift });
    });
    let AnnotationKind::Highlight { points } = &e.annotations()[0].kind else { panic!("a marker stroke") };
    assert_eq!(points.len(), 2, "one straight segment");
    assert_eq!(points[0].y, points[1].y, "snapped level");
}

#[test]
fn freehand() {
    check(Tool::Freehand, "freehand", |e| {
        e.handle(EditorInput::PointerDown { x: 5.0, y: 5.0, button: PointerButton::Left, mods: Mods::default() });
        for i in 1..20 {
            let x = 5.0 + i as f32 * 3.0;
            e.handle(EditorInput::PointerMove { x, y: 20.0 + (i as f32 * 0.7).sin() * 15.0, mods: Mods::default() });
        }
        e.handle(EditorInput::PointerUp { x: 60.0, y: 20.0, button: PointerButton::Left, mods: Mods::default() });
        assert_eq!(e.annotations().len(), 1);
    });
}

#[test]
fn text_box_takes_typing() {
    check(Tool::Text, "text", |e| {
        drag(e, (4.0, 4.0), (60.0, 30.0), Mods::default());
        assert!(e.chrome().editing_text);
        assert!(e.chrome().caret.is_some());
        e.handle(EditorInput::Text("Hi".into()));
        e.handle(EditorInput::Key { key: EditorKey::Backspace, mods: Mods::default() });
        e.handle(EditorInput::Key { key: EditorKey::Escape, mods: Mods::default() });
        assert!(!e.chrome().editing_text);
        match &e.annotations()[0].kind {
            AnnotationKind::Text { text, .. } => assert_eq!(text, "H"),
            other => panic!("{other:?}"),
        }
    });
}

#[test]
fn speech_balloon() {
    check(Tool::SpeechBalloon, "speech_balloon", |e| {
        drag(e, (4.0, 4.0), (50.0, 24.0), Mods::default());
        e.handle(EditorInput::Text("Yo".into()));
        e.handle(EditorInput::Key { key: EditorKey::Escape, mods: Mods::default() });
    });
}

#[test]
fn step_numbers_increment() {
    let e = run(Tool::Step, |e| {
        click(e, (12.0, 12.0));
        click(e, (40.0, 30.0));
    });
    let numbers: Vec<i32> = e
        .annotations()
        .iter()
        .map(|a| match a.kind {
            AnnotationKind::Step { number, .. } => number,
            _ => 0,
        })
        .collect();
    assert_eq!(numbers, vec![1, 2]);
    snapshot("tool_step", &in_pool(|| e.export()));
}

#[test]
fn image_emoji_cursor_place_the_tool_image() {
    for (tool, name) in [(Tool::Image, "image"), (Tool::Emoji, "emoji"), (Tool::Cursor, "cursor")] {
        check(tool, name, |e| {
            e.execute(Command::SetToolImage(small_image([20, 200, 60, 255])));
            click(e, (20.0, 20.0));
            assert_eq!(e.annotations().len(), 1);
        });
    }
}

#[test]
fn clone_stamp() {
    check(Tool::Clone, "clone", |e| drag(e, (2.0, 2.0), (18.0, 14.0), Mods::default()));
}

#[test]
fn effect_tools() {
    for (tool, name) in [
        (Tool::SmartEraser, "smart_eraser"),
        (Tool::Blur, "blur"),
        (Tool::Pixelate, "pixelate"),
        (Tool::Magnify, "magnify"),
        (Tool::Spotlight, "spotlight"),
    ] {
        check(tool, name, |e| drag(e, (10.0, 8.0), (44.0, 36.0), Mods::default()));
    }
}

#[test]
fn crop_applies_on_enter() {
    let e = run(Tool::Crop, |e| {
        drag(e, (8.0, 6.0), (40.0, 30.0), Mods::default());
        assert!(e.chrome().pending.is_some());
        e.handle(EditorInput::Key { key: EditorKey::Enter, mods: Mods::default() });
    });
    assert_eq!(e.canvas_size(), (32, 24));
    snapshot("tool_crop", &in_pool(|| e.export()));
}

#[test]
fn cut_out_removes_a_strip() {
    let e = run(Tool::CutOut, |e| {
        drag(e, (16.0, 4.0), (32.0, 8.0), Mods::default());
        e.handle(EditorInput::Key { key: EditorKey::Enter, mods: Mods::default() });
    });
    assert_eq!(e.canvas_size(), (48, 48));
    snapshot("tool_cut_out", &in_pool(|| e.export()));
}
