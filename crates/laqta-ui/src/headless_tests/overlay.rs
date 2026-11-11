//! The region overlay on a fractionally scaled monitor. GPUI sizes displays by the integer
//! output scale, so the overlay must grow itself to the monitor's physical size over the real
//! scale (KDE at 125% showed only a 1280×720 corner before the fix).

use std::sync::Arc;

use gpui::{TestAppContext, VisualTestContext, px, size};
use laqta_domain::{Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat};
use laqta_ports::RegionView;

use super::{QueuedSpawner, install, services};

#[gpui::test]
fn overlay_covers_the_monitor_at_125_percent(cx: &mut TestAppContext) {
    // The layer-shell path needs a compositor; the test platform takes the plain-window path.
    // SAFETY: tests in this binary don't read this variable concurrently with writing it.
    unsafe { std::env::set_var("LAQTA_NO_LAYER_SHELL", "1") };
    let dir = tempfile::tempdir().unwrap();
    install(cx, services(QueuedSpawner::default(), dir.path()));
    let (w, h) = (2560u32, 1440u32);
    let frame = Frame {
        monitor: Monitor {
            id: MonitorId("m0".into()),
            name: "m0".into(),
            rect: PhysRect { x: 0, y: 0, width: w, height: h },
            scale: 1.25,
            primary: true,
        },
        image: Bitmap::new(w, h, PixelFormat::Bgra8, vec![0u8; (w * h * 4) as usize].into()).unwrap(),
    };
    let images = crate::overlay::render_images(std::slice::from_ref(&frame));
    cx.update(|cx| crate::overlay::show(vec![frame], images, RegionView::default(), Arc::new(|_| {}), cx));
    let window = *cx.windows().last().expect("the overlay window");
    let mut vcx = VisualTestContext::from_window(window, cx);

    vcx.simulate_scale_factor_change(1.25);
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let requested = vcx.update(|window, _| window.bounds().size);
    assert_eq!(requested, size(px(2048.), px(1152.)), "the overlay must cover 2560×1440 at 125%");

    vcx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(vcx.update(|window, _| window.bounds().size), requested, "no resize loop");
}

/// A settled selection gets the engine and the toolbar; a drawing tool locks the
/// selection, and an action without annotations finishes straight away.
#[gpui::test]
fn the_capture_toolbar_locks_for_drawing_and_finishes_with_its_action(cx: &mut TestAppContext) {
    use laqta_domain::region::{RegionCommand, RegionInput, SelectionAction, TextCommand};
    use laqta_editor::Tool;
    use std::sync::Mutex;

    use crate::presenters::capture_toolbar::Item;

    // SAFETY: tests in this binary don't read this variable concurrently with writing it.
    unsafe { std::env::set_var("LAQTA_NO_LAYER_SHELL", "1") };
    let dir = tempfile::tempdir().unwrap();
    install(cx, services(QueuedSpawner::default(), dir.path()));
    let (w, h) = (800u32, 600u32);
    let frame = Frame {
        monitor: Monitor {
            id: MonitorId("m0".into()),
            name: "m0".into(),
            rect: PhysRect { x: 0, y: 0, width: w, height: h },
            scale: 1.0,
            primary: true,
        },
        image: Bitmap::new(w, h, PixelFormat::Bgra8, vec![0u8; (w * h * 4) as usize].into()).unwrap(),
    };
    let sent: Arc<Mutex<Vec<RegionInput>>> = Arc::default();
    let log = sent.clone();
    let images = crate::overlay::render_images(std::slice::from_ref(&frame));
    cx.update(|cx| crate::overlay::show(vec![frame], images, RegionView::default(), Arc::new(move |i| log.lock().unwrap().push(i)), cx));
    let sel = PhysRect { x: 100, y: 100, width: 200, height: 100 };
    let crop = Bitmap::new(200, 100, PixelFormat::Rgba8, vec![255u8; 200 * 100 * 4].into()).unwrap();
    let view = RegionView {
        regions: vec![sel],
        active: Some(0),
        selected: Some(sel),
        selection_image: Some(Arc::new(crop)),
        tools_visible: true,
        ..RegionView::default()
    };
    cx.update(|cx| crate::overlay::update(view, cx));
    let model = cx.update(|cx| cx.global::<crate::runtime::UiState>().overlay.model()).expect("an overlay model");
    assert!(cx.update(|cx| model.read(cx).toolbar_shown()), "the toolbar shows for a settled selection");

    cx.update(|cx| model.update(cx, |m, cx| m.pick(Item::Tool(Tool::Rectangle), cx)));
    assert!(sent.lock().unwrap().iter().any(|i| matches!(i, RegionInput::Command(RegionCommand::Lock))));

    cx.update(|cx| model.update(cx, |m, cx| m.pick(Item::Region, cx)));
    assert!(
        sent.lock().unwrap().iter().any(|i| matches!(i, RegionInput::Command(RegionCommand::Unlock))),
        "no marks: the selection unlocks"
    );

    cx.update(|cx| model.update(cx, |m, cx| m.pick(Item::Copy, cx)));
    assert!(matches!(sent.lock().unwrap().last(), Some(RegionInput::Command(RegionCommand::Finish(SelectionAction::Copy)))));

    // The mode bar: Full screen and Close go to the session; Text on a settled selection asks for its text.
    use crate::overlay_tools::Mode;
    cx.update(|cx| model.update(cx, |m, cx| m.mode(Mode::FullScreen, cx)));
    assert!(matches!(sent.lock().unwrap().last(), Some(RegionInput::Command(RegionCommand::Fullscreen))));
    cx.update(|cx| model.update(cx, |m, cx| m.mode(Mode::Text, cx)));
    assert!(matches!(sent.lock().unwrap().last(), Some(RegionInput::Text(TextCommand::Read))), "the text is read in place");
    cx.update(|cx| model.update(cx, |m, cx| m.mode(Mode::Close, cx)));
    assert!(matches!(sent.lock().unwrap().last(), Some(RegionInput::Command(RegionCommand::Cancel))));
}

/// The wheel over the overlay grows the hover (one notch up is `Wheel(1)`), and touchpad
/// pixels add up to whole notches before anything is sent.
#[gpui::test]
fn the_wheel_sends_whole_notches(cx: &mut TestAppContext) {
    use gpui::{Modifiers, ScrollDelta, ScrollEvent, TouchPhase, point};
    use laqta_domain::region::RegionInput;
    use std::sync::Mutex;

    // SAFETY: tests in this binary don't read this variable concurrently with writing it.
    unsafe { std::env::set_var("LAQTA_NO_LAYER_SHELL", "1") };
    let dir = tempfile::tempdir().unwrap();
    install(cx, services(QueuedSpawner::default(), dir.path()));
    let (w, h) = (400u32, 300u32);
    let frame = Frame {
        monitor: Monitor {
            id: MonitorId("m0".into()),
            name: "m0".into(),
            rect: PhysRect { x: 0, y: 0, width: w, height: h },
            scale: 1.0,
            primary: true,
        },
        image: Bitmap::new(w, h, PixelFormat::Bgra8, vec![0u8; (w * h * 4) as usize].into()).unwrap(),
    };
    let sent: Arc<Mutex<Vec<RegionInput>>> = Arc::default();
    let log = sent.clone();
    let images = crate::overlay::render_images(std::slice::from_ref(&frame));
    cx.update(|cx| crate::overlay::show(vec![frame], images, RegionView::default(), Arc::new(move |i| log.lock().unwrap().push(i)), cx));
    let window = *cx.windows().last().expect("the overlay window");
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.update(|window, cx| window.draw(cx).clear(cx));

    let scroll = |delta| ScrollEvent {
        position: point(px(100.), px(100.)),
        delta,
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
        ..Default::default()
    };
    let wheels = |sent: &Mutex<Vec<RegionInput>>| -> Vec<i32> {
        sent.lock().unwrap().iter().filter_map(|i| if let RegionInput::Wheel(n) = i { Some(*n) } else { None }).collect()
    };
    vcx.simulate_event(scroll(ScrollDelta::Lines(point(0., 1.))));
    assert_eq!(wheels(&sent), vec![1]);
    vcx.simulate_event(scroll(ScrollDelta::Pixels(point(px(0.), px(-25.)))));
    assert_eq!(wheels(&sent), vec![1], "less than a notch is held back");
    vcx.simulate_event(scroll(ScrollDelta::Pixels(point(px(0.), px(-25.)))));
    assert_eq!(wheels(&sent), vec![1, -1]);
}

/// Command+W on the overlay must end the selection, not only remove the window.
#[gpui::test]
fn closing_the_overlay_window_cancels_the_selection(cx: &mut TestAppContext) {
    // SAFETY: tests in this binary don't read this variable concurrently with writing it.
    unsafe { std::env::set_var("LAQTA_NO_LAYER_SHELL", "1") };
    let dir = tempfile::tempdir().unwrap();
    install(cx, services(QueuedSpawner::default(), dir.path()));
    let frame = Frame {
        monitor: Monitor {
            id: MonitorId("m0".into()),
            name: "m0".into(),
            rect: PhysRect { x: 0, y: 0, width: 80, height: 60 },
            scale: 1.0,
            primary: true,
        },
        image: Bitmap::new(80, 60, PixelFormat::Bgra8, vec![0u8; 80 * 60 * 4].into()).unwrap(),
    };
    let sent: Arc<std::sync::Mutex<Vec<laqta_domain::region::RegionInput>>> = Arc::default();
    let log = sent.clone();
    let images = crate::overlay::render_images(std::slice::from_ref(&frame));
    cx.update(|cx| crate::overlay::show(vec![frame], images, RegionView::default(), Arc::new(move |i| log.lock().unwrap().push(i)), cx));
    let window = cx
        .update(|cx| cx.windows().into_iter().find(|w| cx.global::<crate::runtime::UiState>().overlay.holds(*w)))
        .expect("an overlay window");
    cx.update(|cx| crate::runtime::close_window(window, cx));
    let escaped = |i: &laqta_domain::region::RegionInput| {
        matches!(i, laqta_domain::region::RegionInput::Key { key: laqta_domain::region::RegionKey::Escape, .. })
    };
    assert!(sent.lock().unwrap().iter().any(escaped), "the session is told to cancel");
}

/// Text read in place (ADR 031): the layer follows the session's views, the pointer picks words
/// inside the selection without moving it, and a copy goes to the session, which owns the clipboard.
#[gpui::test]
fn text_read_in_place_is_picked_with_the_pointer_and_copied_through_the_session(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, point};
    use laqta_domain::region::{RegionInput, TextCommand};
    use laqta_ports::{OcrLine, OcrResult, OcrWord, TextLayer};
    use std::sync::Mutex;

    use crate::presenters::capture_toolbar::Item;
    use crate::presenters::text_layer::Notice;

    // SAFETY: tests in this binary don't read this variable concurrently with writing it.
    unsafe { std::env::set_var("LAQTA_NO_LAYER_SHELL", "1") };
    let dir = tempfile::tempdir().unwrap();
    install(cx, services(QueuedSpawner::default(), dir.path()));
    let (w, h) = (800u32, 600u32);
    let frame = Frame {
        monitor: Monitor {
            id: MonitorId("m0".into()),
            name: "m0".into(),
            rect: PhysRect { x: 0, y: 0, width: w, height: h },
            scale: 1.0,
            primary: true,
        },
        image: Bitmap::new(w, h, PixelFormat::Bgra8, vec![0u8; (w * h * 4) as usize].into()).unwrap(),
    };
    let sent: Arc<Mutex<Vec<RegionInput>>> = Arc::default();
    let log = sent.clone();
    let images = crate::overlay::render_images(std::slice::from_ref(&frame));
    cx.update(|cx| crate::overlay::show(vec![frame], images, RegionView::default(), Arc::new(move |i| log.lock().unwrap().push(i)), cx));
    let model = cx.update(|cx| cx.global::<crate::runtime::UiState>().overlay.model()).expect("an overlay model");
    let window = *cx.windows().last().expect("the overlay window");
    let mut vcx = VisualTestContext::from_window(window, cx);

    let sel = PhysRect { x: 100, y: 100, width: 400, height: 200 };
    let settled = RegionView { regions: vec![sel], active: Some(0), selected: Some(sel), tools_visible: true, ..RegionView::default() };
    let crop = Bitmap::new(400, 200, PixelFormat::Rgba8, vec![255u8; 400 * 200 * 4].into()).unwrap();
    vcx.update(|_, cx| crate::overlay::update(RegionView { selection_image: Some(Arc::new(crop)), ..settled.clone() }, cx));
    assert!(vcx.update(|_, cx| model.read(cx).toolbar_shown()));

    // The toolbar's text button asks the session; nothing shows until the session answers.
    vcx.update(|_, cx| model.update(cx, |m, cx| m.pick(Item::CopyText, cx)));
    assert!(matches!(sent.lock().unwrap().last(), Some(RegionInput::Text(TextCommand::Read))));
    assert!(vcx.update(|_, cx| model.read(cx).text.is_none()));

    vcx.update(|_, cx| crate::overlay::update(RegionView { text: Some(TextLayer::Reading), ..settled.clone() }, cx));
    let notice =
        |vcx: &mut VisualTestContext| vcx.update(|_, cx| model.read(cx).text.as_ref().map(|t| t.notice(std::time::Instant::now())));
    assert_eq!(notice(&mut vcx), Some(Notice::Reading));
    assert!(!vcx.update(|_, cx| model.read(cx).toolbar_shown()), "the text bar takes the toolbar's place");
    vcx.update(|window, cx| window.draw(cx).clear(cx));

    // Two lines in the selection's own pixels: the engine placed the words of the first.
    let word = |text: &str, x: i32| OcrWord { rect: PhysRect { x, y: 20, width: 50, height: 20 }, text: text.to_owned() };
    let result = Arc::new(OcrResult {
        lines: vec![
            OcrLine {
                rect: PhysRect { x: 20, y: 20, width: 190, height: 20 },
                text: "Hello big world".into(),
                confidence: 1.0,
                words: vec![word("Hello", 20), word("big", 90), word("world", 160)],
            },
            OcrLine {
                rect: PhysRect { x: 20, y: 60, width: 120, height: 20 },
                text: "second line".into(),
                confidence: 1.0,
                words: Vec::new(),
            },
        ],
    });
    let read = RegionView { text: Some(TextLayer::Read { result: result.clone(), copied: true, copy_failed: false }), ..settled.clone() };
    vcx.update(|_, cx| crate::overlay::update(read.clone(), cx));
    assert_eq!(notice(&mut vcx), Some(Notice::Found { words: 5, copied: true }));
    vcx.update(|window, cx| window.draw(cx).clear(cx));

    // A drag from "big" down to "second": the selection's origin plus the word's place, in the
    // window's logical pixels.
    let before = sent.lock().unwrap().len();
    let scale = vcx.update(|window, _| window.scale_factor());
    let at = |x: f32, y: f32| point(px((100. + x) / scale), px((100. + y) / scale));
    vcx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: at(100., 30.),
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    vcx.simulate_event(MouseMoveEvent { position: at(40., 70.), pressed_button: Some(MouseButton::Left), modifiers: Modifiers::default() });
    vcx.simulate_event(MouseUpEvent { button: MouseButton::Left, position: at(40., 70.), modifiers: Modifiers::default(), click_count: 1 });
    let picked = |vcx: &mut VisualTestContext| vcx.update(|_, cx| model.read(cx).text.as_ref().and_then(|t| t.pick.text()));
    assert_eq!(picked(&mut vcx).as_deref(), Some("big world\nsecond"));
    let presses = |sent: &Mutex<Vec<RegionInput>>| {
        sent.lock().unwrap().iter().filter(|i| matches!(i, RegionInput::PointerDown { .. } | RegionInput::PointerUp { .. })).count()
    };
    assert_eq!(presses(&sent), 0, "the session never saw the drag, so the selection did not move");
    assert!(sent.lock().unwrap()[before..].iter().all(|i| matches!(i, RegionInput::PointerMove { .. })));
    assert_eq!(notice(&mut vcx), Some(Notice::Picked(3)));

    // The session paints the same result again (the pointer moved): the picked words stay.
    vcx.update(|_, cx| crate::overlay::update(RegionView { cursor: (140, 170), ..read.clone() }, cx));
    assert_eq!(picked(&mut vcx).as_deref(), Some("big world\nsecond"));

    vcx.simulate_keystrokes("ctrl-c");
    assert_eq!(sent.lock().unwrap().last(), Some(&RegionInput::Text(TextCommand::Copy("big world\nsecond".to_owned()))));
    assert_eq!(notice(&mut vcx), Some(Notice::Copied(Some(3))));
    // The session says the clipboard refused it: the bar says so instead.
    let refused = RegionView { text: Some(TextLayer::Read { result: result.clone(), copied: true, copy_failed: true }), ..read.clone() };
    vcx.update(|_, cx| crate::overlay::update(refused, cx));
    assert_eq!(notice(&mut vcx), Some(Notice::CopyFailed));
    assert_eq!(picked(&mut vcx).as_deref(), Some("big world\nsecond"), "the picked words stay");
    vcx.simulate_keystrokes("ctrl-c");
    assert_eq!(notice(&mut vcx), Some(Notice::Copied(Some(3))), "a new copy starts over");
    vcx.update(|_, cx| crate::overlay::update(read.clone(), cx));
    assert_eq!(notice(&mut vcx), Some(Notice::Copied(Some(3))));

    // Arrow keys would nudge the selection and read it again on every press: they do nothing here.
    let before = sent.lock().unwrap().len();
    vcx.simulate_keystrokes("left");
    assert_eq!(sent.lock().unwrap().len(), before);

    // Escape drops the picked words first, then leaves the text.
    vcx.simulate_keystrokes("escape");
    assert_eq!(picked(&mut vcx), None);
    assert_eq!(sent.lock().unwrap().len(), before, "the first Escape stays on the overlay");
    vcx.simulate_keystrokes("ctrl-a");
    assert_eq!(picked(&mut vcx).as_deref(), Some("Hello big world\nsecond line"));
    vcx.simulate_keystrokes("escape");
    vcx.simulate_keystrokes("escape");
    assert_eq!(sent.lock().unwrap().last(), Some(&RegionInput::Text(TextCommand::Close)));

    // Outside the selection the pointer is still the session's: a new region can be drawn, and is read.
    vcx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: at(600., 400.),
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    assert!(sent.lock().unwrap().iter().any(|i| matches!(i, RegionInput::PointerDown { x: 700, y: 500, .. })));

    // The session drops the text with the selection it belonged to.
    vcx.update(|_, cx| crate::overlay::update(settled.clone(), cx));
    assert!(vcx.update(|_, cx| model.read(cx).text.is_none() && model.read(cx).toolbar_shown()));
}

/// A text capture has no toolbar: the hint asks for text, the layer comes up with the selection,
/// and leaving the text ends the session.
#[gpui::test]
fn a_text_capture_shows_no_toolbar_and_closing_it_ends_the_session(cx: &mut TestAppContext) {
    use laqta_domain::region::{RegionInput, TextCommand};
    use laqta_ports::{OcrResult, TextLayer};
    use std::sync::Mutex;

    use crate::presenters::text_layer::Notice;

    // SAFETY: tests in this binary don't read this variable concurrently with writing it.
    unsafe { std::env::set_var("LAQTA_NO_LAYER_SHELL", "1") };
    let dir = tempfile::tempdir().unwrap();
    install(cx, services(QueuedSpawner::default(), dir.path()));
    let frame = Frame {
        monitor: Monitor {
            id: MonitorId("m0".into()),
            name: "m0".into(),
            rect: PhysRect { x: 0, y: 0, width: 400, height: 300 },
            scale: 1.0,
            primary: true,
        },
        image: Bitmap::new(400, 300, PixelFormat::Bgra8, vec![0u8; 400 * 300 * 4].into()).unwrap(),
    };
    let sent: Arc<Mutex<Vec<RegionInput>>> = Arc::default();
    let log = sent.clone();
    let images = crate::overlay::render_images(std::slice::from_ref(&frame));
    let first = RegionView { text_only: true, ..RegionView::default() };
    cx.update(|cx| crate::overlay::show(vec![frame], images, first, Arc::new(move |i| log.lock().unwrap().push(i)), cx));
    let model = cx.update(|cx| cx.global::<crate::runtime::UiState>().overlay.model()).expect("an overlay model");
    let window = *cx.windows().last().expect("the overlay window");
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.update(|window, cx| window.draw(cx).clear(cx));

    let sel = PhysRect { x: 50, y: 50, width: 200, height: 100 };
    let crop = Bitmap::new(200, 100, PixelFormat::Rgba8, vec![255u8; 200 * 100 * 4].into()).unwrap();
    let view = RegionView {
        regions: vec![sel],
        active: Some(0),
        selected: Some(sel),
        selection_image: Some(Arc::new(crop)),
        tools_visible: true,
        text_only: true,
        text: Some(TextLayer::Read { result: Arc::new(OcrResult::default()), copied: false, copy_failed: false }),
        ..RegionView::default()
    };
    vcx.update(|_, cx| crate::overlay::update(view, cx));
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(!vcx.update(|_, cx| model.read(cx).toolbar_shown()));
    let notice = vcx.update(|_, cx| model.read(cx).text.as_ref().map(|t| t.notice(std::time::Instant::now())));
    assert_eq!(notice, Some(Notice::NoText));

    // Nothing to copy: Ctrl+C sends nothing. Escape leaves the text, which the session takes as the end.
    vcx.simulate_keystrokes("ctrl-c");
    assert!(!sent.lock().unwrap().iter().any(|i| matches!(i, RegionInput::Text(TextCommand::Copy(_)))));
    vcx.simulate_keystrokes("escape");
    assert_eq!(sent.lock().unwrap().last(), Some(&RegionInput::Text(TextCommand::Close)));
}

/// Enter while the text is still being read waits for it: the session copies the whole text when
/// the read is done, and only then is the text left.
#[gpui::test]
fn enter_while_reading_leaves_once_the_text_is_read(cx: &mut TestAppContext) {
    use laqta_domain::region::{RegionInput, TextCommand};
    use laqta_ports::{OcrResult, TextLayer};
    use std::sync::Mutex;

    // SAFETY: tests in this binary don't read this variable concurrently with writing it.
    unsafe { std::env::set_var("LAQTA_NO_LAYER_SHELL", "1") };
    let dir = tempfile::tempdir().unwrap();
    install(cx, services(QueuedSpawner::default(), dir.path()));
    let frame = Frame {
        monitor: Monitor {
            id: MonitorId("m0".into()),
            name: "m0".into(),
            rect: PhysRect { x: 0, y: 0, width: 400, height: 300 },
            scale: 1.0,
            primary: true,
        },
        image: Bitmap::new(400, 300, PixelFormat::Bgra8, vec![0u8; 400 * 300 * 4].into()).unwrap(),
    };
    let sent: Arc<Mutex<Vec<RegionInput>>> = Arc::default();
    let log = sent.clone();
    let images = crate::overlay::render_images(std::slice::from_ref(&frame));
    let first = RegionView { text_only: true, ..RegionView::default() };
    cx.update(|cx| crate::overlay::show(vec![frame], images, first, Arc::new(move |i| log.lock().unwrap().push(i)), cx));
    let model = cx.update(|cx| cx.global::<crate::runtime::UiState>().overlay.model()).expect("an overlay model");
    let window = *cx.windows().last().expect("the overlay window");
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.update(|window, cx| window.draw(cx).clear(cx));

    let sel = PhysRect { x: 50, y: 50, width: 200, height: 100 };
    let crop = Bitmap::new(200, 100, PixelFormat::Rgba8, vec![255u8; 200 * 100 * 4].into()).unwrap();
    let view = RegionView {
        regions: vec![sel],
        active: Some(0),
        selected: Some(sel),
        selection_image: Some(Arc::new(crop)),
        tools_visible: true,
        text_only: true,
        text: Some(TextLayer::Read { result: Arc::new(OcrResult::default()), copied: false, copy_failed: false }),
        ..RegionView::default()
    };
    let reading = RegionView { text: Some(TextLayer::Reading), ..view.clone() };
    vcx.update(|_, cx| crate::overlay::update(reading, cx));
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.simulate_keystrokes("enter");
    assert!(!sent.lock().unwrap().iter().any(|i| matches!(i, RegionInput::Text(TextCommand::Close))), "the read is not given up");
    assert!(vcx.update(|_, cx| model.read(cx).text.is_some()), "the text layer stays while it reads");

    vcx.update(|_, cx| crate::overlay::update(view, cx));
    assert_eq!(sent.lock().unwrap().last(), Some(&RegionInput::Text(TextCommand::Close)));
}
