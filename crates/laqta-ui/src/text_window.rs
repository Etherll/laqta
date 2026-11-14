//! A tool's text result, beside the image it came from: OCR, image analysis, QR codes and the
//! window inspector. The text is editable, so the user can fix a misread word before copying.

use std::fs;
use std::sync::Arc;

use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, IntoElement, ParentElement as _, Render, RenderImage, SharedString, Styled as _,
    StyledImage as _, Window, WindowOptions, div, img, px, relative, size,
};
use gpui_component::input::{Textarea, TextareaState};
use gpui_component::{ActiveTheme as _, Root};
use laqta_domain::Bitmap;

use crate::assets::IconName;
use crate::theme::{space, text};
use crate::widgets::{self, Pill};

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
        window_bounds: Some(crate::chrome::centered(if wide { 920. } else { 600. }, 560., cx)),
        titlebar: Some(crate::chrome::titlebar(result.title.clone())),
        window_decorations: crate::chrome::decorations(),
        app_owns_titlebar_drag: true,
        app_id: Some(crate::APP_ID.into()),
        window_min_size: Some(size(px(480.), px(320.))),
        focus: true,
        ..Default::default()
    };
    // The preview is converted before the window opens, so its first frame already has it.
    let preview = result.image.as_ref().and_then(crate::gpu_image::render_image);
    if let Err(e) = cx.open_window(options, move |window, cx| {
        crate::runtime::dismiss_on_escape(window, cx);
        let editor = cx.new(|cx| TextareaState::new(window, cx).default_value(result.text.clone()));
        let clip = result.copied.then(|| result.text.clone());
        let view =
            cx.new(|_| TextWindow { title: result.title, preview, editor, original: result.text, joined: false, clip, notice: None });
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        tracing::warn!("text window failed: {e:#}");
    }
}

struct TextWindow {
    title: String,
    preview: Option<Arc<RenderImage>>,
    editor: Entity<TextareaState>,
    /// The text as read, so "Keep lines" can bring it back after joining.
    original: String,
    joined: bool,
    /// What this window last put on the clipboard: the footer says whether the editor still matches it.
    clip: Option<String>,
    /// A short confirmation after Save.
    notice: Option<&'static str>,
}

impl TextWindow {
    fn text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        self.clip = Some(text);
        self.notice = None;
        cx.notify();
    }

    /// Swaps between the lines as read and the text joined into paragraphs.
    fn set_joined(&mut self, joined: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.joined == joined {
            return;
        }
        self.joined = joined;
        let next = if joined { join_lines(&self.original) } else { self.original.clone() };
        self.editor.update(cx, |s, cx| s.replace_all(next, window, cx));
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

/// Joins the lines OCR breaks at the image's line ends into paragraphs. A blank line still
/// separates paragraphs; a word split with a hyphen at a line end is rejoined; Chinese and
/// Japanese lines join without a space.
fn join_lines(text: &str) -> String {
    let wide = |c: char| matches!(c as u32, 0x3000..=0x30FF | 0x3400..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF);
    let mut paragraphs: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            if !cur.is_empty() {
                paragraphs.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if cur.is_empty() {
            cur.push_str(line);
            continue;
        }
        let next_lower = line.chars().next().is_some_and(char::is_lowercase);
        if cur.ends_with('-') && next_lower {
            cur.pop();
        } else if !(cur.chars().last().is_some_and(wide) && line.chars().next().is_some_and(wide)) {
            cur.push(' ');
        }
        cur.push_str(line);
    }
    if !cur.is_empty() {
        paragraphs.push(cur);
    }
    paragraphs.join("\n\n")
}

impl Render for TextWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let x = crate::theme::extra(cx);
        let border = t.border;
        let value = self.text(cx);
        let empty = value.trim().is_empty();
        // Several lines are worth joining; one line is already a paragraph.
        let multiline = self.original.lines().filter(|l| !l.trim().is_empty()).count() > 1;

        // The image the text came from, on the stage colour, edge to edge.
        let preview = self.preview.clone().map(|image| {
            div()
                .w(relative(0.42))
                .flex_none()
                .h_full()
                .bg(x.stage)
                .border_r_1()
                .border_color(border)
                .p(space::S5)
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                // No shadow: a contained image's box is the whole pane, so a shadow would blur the
                // pane rather than the picture.
                .child(img(image).max_w_full().max_h_full().object_fit(gpui::ObjectFit::Contain))
        });

        // Above the text: how the lines are laid out, and how much was read.
        let this = cx.entity().downgrade();
        let joined = self.joined;
        let mut bar = div().flex_none().h(px(52.)).px(space::S4).flex().items_center().gap(space::S3).border_b_1().border_color(border);
        if multiline && !empty {
            let (a, b) = (this.clone(), this.clone());
            bar = bar.child(widgets::segmented(
                vec![
                    widgets::Segment {
                        id: "text-lines".into(),
                        label: "Lines as read".into(),
                        selected: !joined,
                        on_click: Box::new(move |_, w, cx| {
                            let _ = a.update(cx, |this, cx| this.set_joined(false, w, cx));
                        }),
                    },
                    widgets::Segment {
                        id: "text-joined".into(),
                        label: "Join lines".into(),
                        selected: joined,
                        on_click: Box::new(move |_, w, cx| {
                            let _ = b.update(cx, |this, cx| this.set_joined(true, w, cx));
                        }),
                    },
                ],
                cx,
            ));
        }
        bar = bar.child(div().flex_1()).child(widgets::muted(counts(&value), cx));

        let text_panel = if empty {
            div().flex_1().flex().items_center().justify_center().p(space::S6).child(widgets::empty_state(
                IconName::ScanLine,
                "No text found",
                "Try a larger area, or zoom in so the text is sharper.",
                cx,
            ))
        } else {
            // No box around the editor: the pane is the text field.
            div()
                .flex_1()
                .min_h_0()
                .px(space::S2)
                .py(space::S2)
                .text_size(text::BODY)
                .child(Textarea::new(&self.editor).appearance(false).h_full().aria_label("Recognized text"))
        };

        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .children(preview)
            .child(div().flex_1().min_w_0().flex().flex_col().child(bar).child(text_panel));

        // The footer says where the text stands: on the clipboard, changed since, or not copied.
        let in_sync = self.clip.as_deref() == Some(value.as_str());
        let note: SharedString = match (self.notice, &self.clip) {
            (Some(n), _) => n.into(),
            _ if empty => "Nothing to copy".into(),
            (None, Some(_)) if in_sync => "✓ On the clipboard".into(),
            (None, Some(_)) => "Edited since copying".into(),
            (None, None) => "Edit the text here before copying it".into(),
        };
        let mut actions =
            vec![widgets::FooterAction::new("text-save", "Save as .txt…", Pill::Ghost, cx.listener(|this, _, _, cx| this.save(cx)))];
        if !empty {
            let label = if in_sync { "Copy again" } else { "Copy text" };
            actions.push(widgets::FooterAction::new("text-copy", label, Pill::Primary, cx.listener(|this, _, _, cx| this.copy(cx))));
        }
        let footer = widgets::footer(Some(note), actions, cx).flex_none();

        crate::chrome::rounded(widgets::page(cx), window)
            .child(crate::chrome::TitleBar::new().title(self.title.clone()).flat())
            .child(body)
            .child(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::{counts, join_lines};

    #[test]
    fn counts_skip_blank_lines_and_spaces() {
        assert_eq!(counts("Hello world\n\nsecond"), "2 lines · 16 characters");
        assert_eq!(counts("a"), "1 line · 1 character");
        assert_eq!(counts(""), "0 lines · 0 characters");
    }

    #[test]
    fn join_lines_makes_paragraphs() {
        assert_eq!(join_lines("Writes resized\ncopies next to\nthe originals."), "Writes resized copies next to the originals.");
        assert_eq!(join_lines("one\ntwo\n\nthree"), "one two\n\nthree");
        assert_eq!(join_lines("hyphen-\nated word"), "hyphenated word");
        assert_eq!(join_lines("Well-\nKnown"), "Well- Known");
        assert_eq!(join_lines("日本\n語"), "日本語");
    }
}
