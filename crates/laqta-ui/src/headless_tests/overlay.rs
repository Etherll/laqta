//! The region overlay on a fractionally scaled monitor (D8). GPUI sizes displays by the integer
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

/// UX1 L1: a settled selection gets the engine and the toolbar; a drawing tool locks the
/// selection, and an action without annotations finishes straight away.
#[gpui::test]
fn the_capture_toolbar_locks_for_drawing_and_finishes_with_its_action(cx: &mut TestAppContext) {
    use laqta_domain::region::{RegionCommand, RegionInput, SelectionAction};
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

    // The mode bar: Full screen and Close go to the session; Text on a settled selection copies its text.
    use crate::overlay_tools::Mode;
    cx.update(|cx| model.update(cx, |m, cx| m.mode(Mode::FullScreen, cx)));
    assert!(matches!(sent.lock().unwrap().last(), Some(RegionInput::Command(RegionCommand::Fullscreen))));
    cx.update(|cx| model.update(cx, |m, cx| m.mode(Mode::Text, cx)));
    assert!(matches!(sent.lock().unwrap().last(), Some(RegionInput::Command(RegionCommand::Finish(SelectionAction::CopyText)))));
    cx.update(|cx| model.update(cx, |m, cx| m.mode(Mode::Close, cx)));
    assert!(matches!(sent.lock().unwrap().last(), Some(RegionInput::Command(RegionCommand::Cancel))));
}

/// LAYOUT: the wheel over the overlay grows the hover (one notch up is `Wheel(1)`), and touchpad
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
