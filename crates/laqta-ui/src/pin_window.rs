//! Pin to screen. A pin is a borderless pop-up window showing an image:
//! drag to move, scroll to zoom, double-click, right-click or Escape to close.

use std::sync::Arc;

use gpui::{
    AnyWindowHandle, App, AppContext as _, Context, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement as _, Render, RenderImage, ScrollEvent, Styled as _, Window, WindowDecorations, WindowKind,
    WindowOptions, div, img, px, size,
};
use laqta_domain::Bitmap;

/// Open pins, so "close all" can find them.
#[derive(Default)]
pub(crate) struct Pins(Vec<AnyWindowHandle>);

impl gpui::Global for Pins {}

use crate::gpu_image::render_image;

pub(crate) fn pin(image: Bitmap, cx: &mut App) {
    let Some(render) = render_image(&image) else { return };
    // Physical pixels shown at 1:1 on a scale-1 display; HiDPI displays show them at logical size.
    // Never shorter than the shortest window the platform opens.
    let (w, h) = (image.width().max(16) as f32, (image.height().max(16) as f32).max(crate::popup::min_height()));
    let options = WindowOptions {
        window_bounds: Some(crate::chrome::centered(w, h, cx)),
        titlebar: None,
        kind: WindowKind::PopUp,
        focus: true,
        is_movable: true,
        is_resizable: false,
        window_decorations: Some(WindowDecorations::Client),
        app_id: Some("laqta-pin".into()),
        ..Default::default()
    };
    match crate::popup::open(options, cx, |_, cx| cx.new(|cx| PinView { image: render, base: (w, h), zoom: 1.0, focus: cx.focus_handle() }))
    {
        Ok(handle) => {
            cx.default_global::<Pins>().0.push(handle.into());
        }
        Err(e) => tracing::warn!("pin window failed: {e:#}"),
    }
}

pub(crate) fn close_all(cx: &mut App) {
    let handles = std::mem::take(&mut cx.default_global::<Pins>().0);
    for h in handles {
        let _ = h.update(cx, |_, window, _| window.remove_window());
    }
}

struct PinView {
    image: Arc<RenderImage>,
    base: (f32, f32),
    zoom: f32,
    focus: FocusHandle,
}

impl Render for PinView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.focus(&self.focus, cx);
        let (w, h) = (self.base.0 * self.zoom, self.base.1 * self.zoom);
        div()
            .id("pin")
            .track_focus(&self.focus)
            .size_full()
            .border_1()
            .border_color(crate::theme::extra(cx).info)
            .child(img(self.image.clone()).w(px(w)).h(px(h)))
            .on_mouse_down(MouseButton::Left, |ev: &MouseDownEvent, window, _| {
                if ev.click_count >= 2 {
                    window.remove_window();
                } else {
                    window.start_window_move();
                }
            })
            .on_mouse_down(MouseButton::Right, |_, window, _| window.remove_window())
            .on_key_down(|ev: &KeyDownEvent, window, _| {
                if ev.keystroke.key == "escape" {
                    window.remove_window();
                }
            })
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, window, cx| {
                let dy = f32::from(ev.delta.pixel_delta(px(20.)).y);
                let next = (this.zoom * if dy > 0.0 { 1.1 } else { 1.0 / 1.1 }).clamp(0.1, 8.0);
                if (next - this.zoom).abs() > f32::EPSILON {
                    this.zoom = next;
                    window.resize(size(px(this.base.0 * next), px(this.base.1 * next)));
                    cx.notify();
                }
            }))
    }
}
