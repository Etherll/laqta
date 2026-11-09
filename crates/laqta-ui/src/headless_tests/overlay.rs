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
