//! A tool's text result, beside the image it came from: OCR, image analysis, QR codes and the
//! window inspector. The text is editable, so the user can fix a misread word before copying.

use std::fs;
use std::sync::Arc;

use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render, RenderImage, Styled as _,
    StyledImage as _, Window, WindowBounds, WindowOptions, div, img, prelude::FluentBuilder as _, px, size,
};
use gpui_component::input::{Textarea, TextareaState};
use gpui_component::{ActiveTheme as _, Root};
use laqta_domain::Bitmap;

use crate::assets::IconName;
use crate::theme::{radius, space, text};
use crate::widgets::{self, Pill, Tone};

/// What the window shows.
pub(crate) struct TextResult {
    pub title: String,
    /// The image the text came from, shown as a preview.
    pub image: Option<Bitmap>,
    pub text: String,
    /// The text is already on the clipboard.
    pub copied: bool,
}

pub(crate) fn open(result: TextResult, cx: &mut App) {
    crate::theme::ensure_fonts(cx);
    let wide = result.image.is_some();
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(if wide { 860. } else { 600. }), px(520.)), cx)),
        titlebar: Some(crate::chrome::titlebar(result.title.clone())),
        window_decorations: crate::chrome::decorations(),
        app_owns_titlebar_drag: true,
        app_id: Some("laqta".into()),
        window_min_size: Some(size(px(480.), px(320.))),
        focus: true,
        ..Default::default()
    };
    // The preview is converted before the window opens, so its first frame already has it.
    let preview = result.image.as_ref().and_then(crate::gpu_image::render_image);
    if let Err(e) = cx.open_window(options, move |window, cx| {
        let editor = cx.new(|cx| TextareaState::new(window, cx).default_value(result.text.clone()));
        let view = cx.new(|_| TextWindow { title: result.title, preview, editor, copied: result.copied, notice: None });
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        tracing::warn!("text window failed: {e:#}");
    }
}

struct TextWindow {
    title: String,
    preview: Option<Arc<RenderImage>>,
    editor: Entity<TextareaState>,
    copied: bool,
    /// A short confirmation after Copy or Save.
    notice: Option<&'static str>,
}

impl TextWindow {
    fn text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.text(cx)));
        self.copied = true;
        self.notice = Some("Copied");
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        let dir = cx.global::<crate::runtime::UiState>().services.app.settings().screenshots_dir.clone();
        cx.spawn(async move |this, cx| {
            let picked = rfd::AsyncFileDialog::new().set_directory(&dir).set_file_name("Laqta_text.txt").save_file().await;
            let Some(file) = picked else { return };
            let path = file.path().to_path_buf();
            // Off the UI thread: the write never blocks a frame.
            let saved = cx.background_spawn(async move { fs::write(&path, text) }).await;
            let _ = this.update(cx, |this, cx| {
                this.notice = Some(if saved.is_ok() { "Saved" } else { "Couldn't save the file" });
                cx.notify();
            });
        })
        .detach();
    }
}

/// "3 lines · 120 characters".
fn counts(text: &str) -> String {
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    let chars = text.chars().filter(|c| !c.is_whitespace()).count();
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    format!("{} · {}", plural(lines, "line"), plural(chars, "character"))
}

impl Render for TextWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let x = crate::theme::extra(cx);
        let value = self.text(cx);
        let empty = value.trim().is_empty();

        let mut header = div()
            .flex()
            .items_center()
            .gap(space::S2)
            .child(div().text_size(text::BODY).font_weight(FontWeight::MEDIUM).child("Text"))
            .child(widgets::muted(counts(&value), cx))
            .child(div().flex_1());
        if self.copied {
            header = header.child(widgets::badge("On the clipboard", Tone::Success, cx));
        }

        let text_panel = if empty {
            div().flex_1().flex().items_center().justify_center().child(widgets::empty_state(
                IconName::ScanLine,
                "No text found",
                "Try a larger area, or zoom in so the text is sharper.",
                cx,
            ))
        } else {
            div().flex_1().min_h_0().flex().flex_col().child(Textarea::new(&self.editor).h_full())
        };

        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .gap(space::S4)
            .p(space::S4)
            .when_some(self.preview.clone(), |d, image| {
                d.child(
                    div()
                        .w(px(300.))
                        .flex_none()
                        .rounded(radius::CARD)
                        .bg(x.stage)
                        .p(space::S3)
                        .flex()
                        .items_center()
                        .justify_center()
                        .overflow_hidden()
                        .child(img(image).max_w_full().max_h_full().object_fit(gpui::ObjectFit::Contain)),
                )
            })
            .child(div().flex_1().min_w_0().flex().flex_col().gap(space::S3).child(header).child(text_panel));

        let footer = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(space::S2)
            .px(space::S4)
            .py(space::S3)
            .border_t_1()
            .border_color(t.border)
            .child(widgets::muted(self.notice.unwrap_or("You can edit the text before copying it."), cx))
            .child(div().flex_1())
            .child(widgets::pill("text-save", "Save as .txt", Pill::Ghost, false, cx.listener(|this, _, _, cx| this.save(cx)), cx))
            .child(widgets::pill("text-close", "Close", Pill::Default, false, |_, window, _| window.remove_window(), cx))
            .when(!empty, |d| {
                d.child(widgets::pill("text-copy", "Copy all", Pill::Primary, false, cx.listener(|this, _, _, cx| this.copy(cx)), cx))
            });

        crate::chrome::rounded(widgets::page(cx), window)
            .child(crate::chrome::TitleBar::new().title(self.title.clone()).flat())
            .child(body)
            .child(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::counts;

    #[test]
    fn counts_skip_blank_lines_and_spaces() {
        assert_eq!(counts("Hello world\n\nsecond"), "2 lines · 16 characters");
        assert_eq!(counts("a"), "1 line · 1 character");
        assert_eq!(counts(""), "0 lines · 0 characters");
    }
}
