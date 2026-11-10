//! The after-upload window and the QR code view (after-upload tasks `ShowAfterUploadWindow`,
//! `ShowQRCode`).

use std::sync::Arc;

use gpui::{
    App, AppContext as _, ClipboardItem, Context, IntoElement, ParentElement as _, Render, RenderImage, Styled as _, Window, WindowOptions,
    div, img, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme as _, Root};

use crate::runtime::UiState;
use crate::widgets;

fn qr_image(url: &str) -> Option<Arc<RenderImage>> {
    let bmp = laqta_imaging::qr::qr_encode(url, 6).ok()?;
    crate::gpu_image::render_image(&bmp)
}

pub(crate) fn open(url: String, qr: bool, cx: &mut App) {
    crate::theme::ensure_fonts(cx);
    let options = WindowOptions {
        window_bounds: Some(crate::chrome::centered(520., if qr { 560. } else { 200. }, cx)),
        titlebar: Some(crate::chrome::titlebar(if qr { "QR code" } else { "Upload finished" })),
        window_decorations: crate::chrome::decorations(),
        app_owns_titlebar_drag: true,
        app_id: Some(crate::APP_ID.into()),
        focus: true,
        ..Default::default()
    };
    let result = cx.open_window(options, |window, cx| {
        let view = cx.new(|_| UrlWindow { qr: if qr { qr_image(&url) } else { None }, url });
        cx.new(|cx| Root::new(view, window, cx))
    });
    if let Err(e) = result {
        tracing::warn!("url window failed: {e:#}");
    }
}

struct UrlWindow {
    url: String,
    qr: Option<Arc<RenderImage>>,
}

impl Render for UrlWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = cx.global::<UiState>().services.shell.clone();
        let (u1, u2) = (self.url.clone(), self.url.clone());
        let mut body = div().flex_1().flex().flex_col().gap(px(10.)).p(px(14.)).items_center();
        if let Some(q) = &self.qr {
            body = body.child(img(q.clone()).w(px(360.)).h(px(360.)).rounded(crate::theme::radius::LG));
        }
        // A one-line URL box is a pill, like every one-line control.
        body = body.child(
            div()
                .h(crate::theme::CONTROL_H)
                .px(px(14.))
                .flex()
                .items_center()
                .rounded(crate::theme::radius::PILL)
                .bg(cx.theme().secondary)
                .w_full()
                .child(div().truncate().text_size(crate::theme::text::LABEL).child(self.url.clone())),
        );
        let footer = div()
            .flex()
            .flex_row()
            .gap(px(8.))
            .p(px(12.))
            .border_t_1()
            .border_color(cx.theme().border)
            .child(widgets::muted("", cx).flex_1())
            .child(Button::new("copy").label("Copy").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(u1.clone()))))
            .child(Button::new("open").label("Open").primary().on_click(move |_, _, _| {
                let _ = shell.open_url(&u2);
            }))
            .child(Button::new("close").label("Close").ghost().on_click(|_, window, _| window.remove_window()));
        let title = if self.qr.is_some() { "QR code" } else { "Upload finished" };
        crate::chrome::rounded(widgets::page(cx), window)
            .child(crate::chrome::TitleBar::new().title(title).flat())
            .child(body)
            .child(footer)
    }
}
