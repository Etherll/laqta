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

/// A settled selection gets the engine and the toolbar; a drawing tool and the region tool switch
/// without touching the session, and an action without annotations finishes straight away.
#[gpui::test]
fn the_capture_toolbar_switches_tools_and_finishes_with_its_action(cx: &mut TestAppContext) {
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
    assert_eq!(cx.update(|cx| model.read(cx).tool), Some(Tool::Rectangle));
    cx.update(|cx| model.update(cx, |m, cx| m.pick(Item::Pointer, cx)));
    assert_eq!(cx.update(|cx| model.read(cx).tool), None, "back to the Move tool");
    assert!(sent.lock().unwrap().is_empty(), "switching tools is the overlay's own business");

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
                vertical: false,
            },
            OcrLine {
                rect: PhysRect { x: 20, y: 60, width: 120, height: 20 },
                text: "second line".into(),
                confidence: 1.0,
                words: Vec::new(),
                vertical: false,
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

    // The grab area along the selection's edges resizes it; inside it are words.
    let grabbing = RegionView { grab_size: 8, ..read.clone() };
    vcx.update(|_, cx| crate::overlay::update(grabbing, cx));
    assert_eq!(vcx.update(|_, cx| model.read(cx).text_point(101, 101)), None, "the corner resizes");
    assert_eq!(vcx.update(|_, cx| model.read(cx).text_point(150, 150)), Some((50.0, 50.0)), "inside are words");
    vcx.update(|_, cx| crate::overlay::update(read.clone(), cx));
    assert_eq!(picked(&mut vcx).as_deref(), Some("big world\nsecond"));

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
    let later = |vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| model.read(cx).text.as_ref().map(|t| t.notice(std::time::Instant::now() + std::time::Duration::from_secs(5))))
    };
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

    // Copy all, refused: once its notice is over the bar does not say the text was copied.
    vcx.simulate_keystrokes("ctrl-c");
    assert_eq!(sent.lock().unwrap().last(), Some(&RegionInput::Text(TextCommand::Copy("Hello big world\nsecond line".to_owned()))));
    let refused = RegionView { text: Some(TextLayer::Read { result: result.clone(), copied: true, copy_failed: true }), ..read.clone() };
    vcx.update(|_, cx| crate::overlay::update(refused, cx));
    assert_eq!(later(&mut vcx), Some(Notice::Found { words: 5, copied: false }));
    vcx.update(|_, cx| crate::overlay::update(read.clone(), cx));
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

    // The whole text did not reach the clipboard: the text stays, so the user sees what was read.
    let refused = OcrResult {
        lines: vec![laqta_ports::OcrLine {
            rect: PhysRect { x: 0, y: 0, width: 50, height: 20 },
            text: "hello".into(),
            confidence: 1.0,
            words: Vec::new(),
            vertical: false,
        }],
    };
    let not_copied =
        RegionView { text: Some(TextLayer::Read { result: Arc::new(refused.clone()), copied: false, copy_failed: false }), ..view.clone() };
    vcx.update(|_, cx| crate::overlay::update(not_copied, cx));
    assert!(!sent.lock().unwrap().iter().any(|i| matches!(i, RegionInput::Text(TextCommand::Close))), "nothing was copied");
    assert!(vcx.update(|_, cx| model.read(cx).text.is_some()));

    // Read again, Enter, and this time the text is copied: the text is left.
    vcx.update(|_, cx| crate::overlay::update(RegionView { text: Some(TextLayer::Reading), ..view.clone() }, cx));
    vcx.simulate_keystrokes("enter");
    let copied = RegionView { text: Some(TextLayer::Read { result: Arc::new(refused), copied: true, copy_failed: false }), ..view };
    vcx.update(|_, cx| crate::overlay::update(copied, cx));
    assert_eq!(sent.lock().unwrap().last(), Some(&RegionInput::Text(TextCommand::Close)));
}

/// A one-monitor overlay over an 800×600 frame, logging what it sends to the session.
fn settled_overlay(
    cx: &mut TestAppContext,
    sel: PhysRect,
) -> (gpui::Entity<crate::overlay::OverlayModel>, Arc<std::sync::Mutex<Vec<laqta_domain::region::RegionInput>>>) {
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
    let sent: Arc<std::sync::Mutex<Vec<laqta_domain::region::RegionInput>>> = Arc::default();
    let log = sent.clone();
    let images = crate::overlay::render_images(std::slice::from_ref(&frame));
    cx.update(|cx| crate::overlay::show(vec![frame], images, RegionView::default(), Arc::new(move |i| log.lock().unwrap().push(i)), cx));
    cx.update(|cx| crate::overlay::update(settled_view(sel), cx));
    let model = cx.update(|cx| cx.global::<crate::runtime::UiState>().overlay.model()).expect("an overlay model");
    (model, sent)
}

/// The view of a settled selection `sel` with a fresh crop of it.
fn settled_view(sel: PhysRect) -> RegionView {
    let crop = Bitmap::new(sel.width, sel.height, PixelFormat::Rgba8, vec![255u8; (sel.width * sel.height * 4) as usize].into()).unwrap();
    RegionView {
        regions: vec![sel],
        active: Some(0),
        selected: Some(sel),
        selection_image: Some(Arc::new(crop)),
        tools_visible: true,
        ..RegionView::default()
    }
}

/// Drags a mark from `from` to `to` in canvas pixels with the tool in use.
fn draw(cx: &mut impl gpui::AppContext, model: &gpui::Entity<crate::overlay::OverlayModel>, from: (f32, f32), to: (f32, f32)) {
    use laqta_editor::{EditorInput, Mods, PointerButton};
    let mods = Mods::default();
    model.update(cx, |m, cx| {
        m.editor_input(EditorInput::PointerDown { x: from.0, y: from.1, button: PointerButton::Left, mods }, cx);
        m.editor_input(EditorInput::PointerMove { x: to.0, y: to.1, mods }, cx);
        m.editor_input(EditorInput::PointerUp { x: to.0, y: to.1, button: PointerButton::Left, mods }, cx);
    });
}

/// Moving the selection carries its marks along, and Alt leaves them over the same desktop pixels;
/// a resize always leaves them there (ADR 032). The tool carries over to the new crop.
#[gpui::test]
fn a_moved_selection_carries_its_marks_unless_alt_leaves_them(cx: &mut TestAppContext) {
    use laqta_editor::{RectF, Tool};

    use crate::presenters::capture_toolbar::Item;

    let sel = PhysRect { x: 100, y: 100, width: 200, height: 100 };
    let (model, sent) = settled_overlay(cx, sel);
    cx.update(|cx| model.update(cx, |m, cx| m.pick(Item::Tool(Tool::Rectangle), cx)));
    draw(cx, &model, (10.0, 10.0), (50.0, 40.0));
    let marks = |cx: &mut TestAppContext| cx.update(|cx| model.read(cx).annot.as_ref().map(|a| a.mark_rects()).unwrap_or_default());
    assert_eq!(marks(cx), vec![RectF::new(10.0, 10.0, 40.0, 30.0)]);

    // The session moved the selection 20 px right and 10 px down, then sent the new crop.
    let moved = PhysRect { x: 120, y: 110, ..sel };
    cx.update(|cx| crate::overlay::update(settled_view(moved), cx));
    assert_eq!(cx.update(|cx| model.read(cx).annot.as_ref().map(|a| a.rect)), Some(moved));
    assert_eq!(marks(cx), vec![RectF::new(10.0, 10.0, 40.0, 30.0)], "the mark moved with the selection");

    // With Alt, the same kind of move leaves the mark on the desktop.
    cx.update(|cx| model.update(cx, |m, _| m.frame_only = true));
    let back = PhysRect { x: 100, y: 100, ..sel };
    cx.update(|cx| crate::overlay::update(settled_view(back), cx));
    assert_eq!(marks(cx), vec![RectF::new(30.0, 20.0, 40.0, 30.0)], "the mark stayed put on the desktop");
    assert!(!cx.update(|cx| model.read(cx).frame_only), "Alt counts for one move");

    // A resize from the left edge keeps the mark over its pixels.
    let wider = PhysRect { x: 90, width: 210, ..back };
    cx.update(|cx| crate::overlay::update(settled_view(wider), cx));
    assert_eq!(marks(cx), vec![RectF::new(40.0, 20.0, 40.0, 30.0)]);
    assert_eq!(cx.update(|cx| model.read(cx).tool), Some(Tool::Rectangle));
    assert!(sent.lock().unwrap().is_empty());
}

/// Esc steps back one layer at a time and warns once before the marks are thrown away.
#[gpui::test]
fn escape_steps_back_and_warns_before_discarding_marks(cx: &mut TestAppContext) {
    use laqta_domain::region::{RegionCommand, RegionInput};
    use laqta_editor::Tool;

    use crate::presenters::capture_toolbar::Item;

    let (model, sent) = settled_overlay(cx, PhysRect { x: 100, y: 100, width: 200, height: 100 });
    cx.update(|cx| model.update(cx, |m, cx| m.pick(Item::Tool(Tool::Rectangle), cx)));
    draw(cx, &model, (10.0, 10.0), (50.0, 40.0));
    let cancelled = |sent: &std::sync::Mutex<Vec<RegionInput>>| {
        sent.lock().unwrap().iter().any(|i| matches!(i, RegionInput::Command(RegionCommand::Cancel)))
    };
    let escape = |cx: &mut TestAppContext| cx.update(|cx| model.update(cx, |m, cx| m.escape(cx)));

    // The new mark is picked: the first Esc lets it go, the next puts the tool away.
    if cx.update(|cx| model.read(cx).annot.as_ref().is_some_and(|a| a.chrome.selection.is_some())) {
        escape(cx);
        assert!(cx.update(|cx| model.read(cx).annot.as_ref().is_some_and(|a| a.chrome.selection.is_none())));
    }
    escape(cx);
    assert_eq!(cx.update(|cx| model.read(cx).tool), None, "the tool is put away");
    escape(cx);
    assert_eq!(cx.update(|cx| model.read(cx).discard_notice()).as_deref(), Some("Press Esc again to discard 1 annotation"));
    assert!(!cancelled(&sent), "the first Esc over marks only warns");
    escape(cx);
    assert!(cancelled(&sent), "the second one ends the capture");
}

/// The toolbar's close button ends the capture at once, marks or not: a click on ✕ is a choice.
#[gpui::test]
fn the_close_button_cancels_at_once_even_with_marks(cx: &mut TestAppContext) {
    use laqta_domain::region::{RegionCommand, RegionInput};
    use laqta_editor::Tool;

    use crate::presenters::capture_toolbar::Item;

    let (model, sent) = settled_overlay(cx, PhysRect { x: 100, y: 100, width: 200, height: 100 });
    cx.update(|cx| model.update(cx, |m, cx| m.pick(Item::Tool(Tool::Rectangle), cx)));
    draw(cx, &model, (10.0, 10.0), (50.0, 40.0));
    cx.update(|cx| model.update(cx, |m, cx| m.pick(Item::Cancel, cx)));
    assert!(matches!(sent.lock().unwrap().last(), Some(RegionInput::Command(RegionCommand::Cancel))));
}

/// Delete removes the picked mark, never the selection with its marks; an action waits while the
/// selection is dragged, since the edited crop would be of the old frame.
#[gpui::test]
fn delete_and_actions_never_lose_the_marks(cx: &mut TestAppContext) {
    use laqta_domain::region::{RegionInput, RegionKey, SelectionAction};
    use laqta_editor::Tool;

    use crate::presenters::capture_toolbar::Item;

    let sel = PhysRect { x: 100, y: 100, width: 200, height: 100 };
    let (model, sent) = settled_overlay(cx, sel);
    let window = *cx.windows().last().expect("the overlay window");
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.update(|_, cx| model.update(cx, |m, cx| m.pick(Item::Tool(Tool::Rectangle), cx)));
    draw(&mut vcx, &model, (10.0, 10.0), (50.0, 40.0));
    vcx.update(|_, cx| model.update(cx, |m, cx| m.pick(Item::Pointer, cx)));
    let count = |vcx: &mut VisualTestContext| vcx.update(|_, cx| model.read(cx).annot.as_ref().map_or(0, |a| a.chrome.annotation_count));

    let picked = vcx.update(|_, cx| model.read(cx).mark_selected());
    vcx.simulate_keystrokes("delete");
    if picked {
        assert_eq!(count(&mut vcx), 0, "Delete removed the picked mark");
    }
    vcx.simulate_keystrokes("delete");
    let deletes = sent.lock().unwrap().iter().filter(|i| matches!(i, RegionInput::Key { key: RegionKey::Delete, .. })).count();
    assert_eq!(deletes, 0, "the session never saw a Delete while marks (or their undo) exist");

    vcx.update(|_, cx| crate::overlay::update(RegionView { gesture: true, ..settled_view(sel) }, cx));
    let before = sent.lock().unwrap().len();
    vcx.update(|_, cx| model.update(cx, |m, cx| m.finish(SelectionAction::Copy, cx)));
    assert_eq!(sent.lock().unwrap().len(), before, "no action mid-drag");
}

/// Each drawing tool keeps its own colour; a finished mark lets go, so the next colour picked
/// doesn't recolour it; number keys pick colours; the picks are saved when the overlay closes.
#[gpui::test]
fn capture_marks_keep_their_colour_per_tool_and_are_saved_on_close(cx: &mut TestAppContext) {
    use laqta_editor::{EditorInput, Mods, PointerButton, Tool};
    use std::sync::Mutex;

    use crate::presenters::capture_toolbar::{self as bar, Item};

    // SAFETY: tests in this binary don't read this variable concurrently with writing it.
    unsafe { std::env::set_var("LAQTA_NO_LAYER_SHELL", "1") };
    let dir = tempfile::tempdir().unwrap();
    let saved: Arc<Mutex<Option<laqta_domain::Settings>>> = Arc::default();
    let mut s = services(QueuedSpawner::default(), dir.path());
    let store = saved.clone();
    s.save_settings = Arc::new(move |settings| {
        *store.lock().unwrap() = Some(settings);
        Ok(())
    });
    install(cx, s);
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
    let images = crate::overlay::render_images(std::slice::from_ref(&frame));
    cx.update(|cx| crate::overlay::show(vec![frame], images, RegionView::default(), Arc::new(|_| {}), cx));
    let sel = PhysRect { x: 50, y: 50, width: 200, height: 100 };
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
    let style = |cx: &mut TestAppContext| cx.update(|cx| model.read(cx).annot.as_ref().unwrap().chrome.clone());

    cx.update(|cx| model.update(cx, |m, cx| m.pick(Item::Tool(Tool::Freehand), cx)));
    let blue = bar::palette(Tool::Freehand)[4];
    cx.update(|cx| model.update(cx, |m, cx| m.restyle(|st, t| bar::with_color(st, t, blue), cx)));
    let (left, plain) = (PointerButton::Left, Mods::default());
    cx.update(|cx| {
        model.update(cx, |m, cx| {
            m.editor_input(EditorInput::PointerDown { x: 10.0, y: 10.0, button: left, mods: plain }, cx);
            m.editor_input(EditorInput::PointerMove { x: 60.0, y: 40.0, mods: plain }, cx);
            m.editor_input(EditorInput::PointerUp { x: 60.0, y: 40.0, button: left, mods: plain }, cx);
        })
    });
    assert_eq!(style(cx).selection, None, "a finished stroke lets go: no stray box");

    cx.update(|cx| model.update(cx, |m, cx| m.pick(Item::Tool(Tool::Rectangle), cx)));
    assert_eq!(style(cx).style.border_color, bar::base_style(Tool::Rectangle).border_color, "the rectangle keeps its own colour");
    cx.update(|cx| model.update(cx, |m, cx| m.pick(Item::Tool(Tool::Freehand), cx)));
    assert_eq!(style(cx).style.border_color, blue, "the pen comes back blue");

    cx.update(|cx| model.update(cx, |m, cx| m.pick(Item::Tool(Tool::Highlight), cx)));
    let window = cx.update(|cx| cx.windows()[0]);
    cx.simulate_keystrokes(window, "3");
    assert_eq!(style(cx).style.highlight_color, bar::palette(Tool::Highlight)[2], "3 picks the third marker colour");
    assert_eq!(style(cx).tool, Tool::Highlight, "the digit didn't switch the engine's tool");

    cx.update(crate::overlay::close);
    let marks = saved.lock().unwrap().take().expect("saved on close").capture_marks;
    assert!(marks.iter().any(|m| m.tool == "Freehand" && m.color == blue));
    assert!(marks.iter().any(|m| m.tool == "Highlight"));
}

/// A finished mark lets go; the Pointer picks a mark by its stroke and keeps it after the release,
/// so its handles, the arrows and Delete work on it. Hovering a mark outlines it.
#[gpui::test]
fn the_pointer_keeps_the_mark_it_picks(cx: &mut TestAppContext) {
    use laqta_editor::Tool;

    use crate::presenters::capture_toolbar::Item;

    let sel = PhysRect { x: 100, y: 100, width: 200, height: 100 };
    let (model, _sent) = settled_overlay(cx, sel);
    let picked = |cx: &mut TestAppContext| cx.update(|cx| model.read(cx).annot.as_ref().is_some_and(|a| a.chrome.selection.is_some()));
    cx.update(|cx| model.update(cx, |m, cx| m.pick(Item::Tool(Tool::Rectangle), cx)));
    draw(cx, &model, (10.0, 10.0), (50.0, 40.0));
    assert!(!picked(cx), "a finished mark lets go");
    cx.update(|cx| model.update(cx, |m, cx| m.pick(Item::Pointer, cx)));
    // The rectangle's left side, in desktop pixels.
    assert!(cx.update(|cx| model.update(cx, |m, _| m.hover((sel.x + 10, sel.y + 25)))), "hovering the mark outlines it");
    assert!(cx.update(|cx| model.read(cx).annot.as_ref().is_some_and(|a| a.hover.is_some())));
    draw(cx, &model, (10.0, 25.0), (10.0, 25.0));
    assert!(picked(cx), "the Pointer keeps the mark it picked");
    draw(cx, &model, (12.0, 25.0), (22.0, 30.0));
    assert!(picked(cx), "and keeps it after dragging it");
}

/// Holding Space lends the Pointer while a drawing tool is out; the tool comes back when Space
/// comes up, or after the drag under way when it comes up mid-drag.
#[gpui::test]
fn space_lends_the_pointer_while_held(cx: &mut TestAppContext) {
    use laqta_editor::Tool;

    use crate::presenters::capture_pointer::Route;
    use crate::presenters::capture_toolbar::Item;

    let (model, _sent) = settled_overlay(cx, PhysRect { x: 100, y: 100, width: 200, height: 100 });
    let tool = |cx: &mut TestAppContext| cx.update(|cx| model.read(cx).tool);
    cx.update(|cx| model.update(cx, |m, cx| m.pick(Item::Tool(Tool::Arrow), cx)));
    cx.update(|cx| model.update(cx, |m, _| m.borrow_pointer()));
    assert_eq!(tool(cx), None, "Space down: the Pointer");
    cx.update(|cx| model.update(cx, |m, cx| m.return_pointer(cx)));
    assert_eq!(tool(cx), Some(Tool::Arrow), "Space up: the arrow again");
    assert_eq!(cx.update(|cx| model.read(cx).annot.as_ref().map(|a| a.chrome.tool)), Some(Tool::Arrow));

    cx.update(|cx| model.update(cx, |m, _| m.borrow_pointer()));
    cx.update(|cx| {
        model.update(cx, |m, cx| {
            m.pressed = Some(Route::Session);
            m.return_pointer(cx);
        })
    });
    assert_eq!(tool(cx), None, "the drag under way keeps the Pointer");
    cx.update(|cx| {
        model.update(cx, |m, cx| {
            m.pressed = None;
            m.return_pointer(cx);
        })
    });
    assert_eq!(tool(cx), Some(Tool::Arrow));
}
