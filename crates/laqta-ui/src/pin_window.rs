//! Pin to screen (M5) and tool text results. A pin is a borderless pop-up window showing an image:
//! drag to move, scroll to zoom, double-click, right-click or Escape to close.

use std::sync::Arc;

use gpui::{
    AnyWindowHandle, App, AppContext as _, Context, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement as _, Render, RenderImage, ScrollEvent, Styled as _, TitlebarOptions, Window, WindowBounds,
    WindowDecorations, WindowKind, WindowOptions, div, img, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme as _, Root};
use laqta_domain::{Bitmap, PixelFormat};
use smallvec::SmallVec;

use crate::widgets;

/// Open pins, so "close all" can find them.
#[derive(Default)]
pub(crate) struct Pins(Vec<AnyWindowHandle>);

impl gpui::Global for Pins {}

fn render_image(bitmap: &Bitmap) -> Option<Arc<RenderImage>> {
    let mut bytes = bitmap.pixels().to_vec();
    if bitmap.format() == PixelFormat::Rgba8 {
        for px in bytes.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
    }
    let buffer = image::RgbaImage::from_raw(bitmap.width(), bitmap.height(), bytes)?;
    Some(Arc::new(RenderImage::new(SmallVec::<[image::Frame; 1]>::from_elem(image::Frame::new(buffer), 1))))
}

pub(crate) fn pin(image: Bitmap, cx: &mut App) {
    let Some(render) = render_image(&image) else { return };
    // Physical pixels shown at 1:1 on a scale-1 display; HiDPI displays show them at logical size.
    let (w, h) = (image.width().max(16) as f32, image.height().max(16) as f32);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(w), px(h)), cx)),
        titlebar: None,
        kind: WindowKind::PopUp,
        focus: true,
        is_movable: true,
        is_resizable: false,
        window_decorations: Some(WindowDecorations::Client),
        app_id: Some("laqta-pin".into()),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        let _ = window;
        cx.new(|cx| PinView { image: render, base: (w, h), zoom: 1.0, focus: cx.focus_handle() })
    }) {
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
            .border_color(gpui::rgb(0x3b82f6))
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

pub(crate) fn show_text(title: String, text: String, cx: &mut App) {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(560.), px(420.)), cx)),
        titlebar: Some(TitlebarOptions { title: Some(title.clone().into()), ..Default::default() }),
        app_id: Some("laqta".into()),
        focus: true,
        ..Default::default()
    };
    if let Err(e) = cx.open_window(options, |window, cx| {
        let view = cx.new(|_| TextView { title, text });
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        tracing::warn!("text window failed: {e:#}");
    }
}

struct TextView {
    title: String,
    text: String,
}

impl Render for TextView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let copy = self.text.clone();
        widgets::page(cx)
            .child(div().flex_1().overflow_y_scrollbar().p(px(14.)).child(
                div().p(px(8.)).rounded(px(6.)).bg(cx.theme().secondary).font_family("monospace").text_sm().child(self.text.clone()),
            ))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .p(px(10.))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(widgets::muted(self.title.clone(), cx))
                    .child(div().flex_1())
                    .child(
                        Button::new("copy")
                            .label("Copy")
                            .on_click(move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()))),
                    )
                    .child(Button::new("close").label("Close").primary().on_click(|_, w, _| w.remove_window())),
            )
    }
}
