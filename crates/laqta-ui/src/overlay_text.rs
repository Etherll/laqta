//! Text read in place on the region overlay (ADR 031).
//!
//! The Region session runs the recogniser on the selection and sends what it found in
//! `RegionView::text`. This module shows it on the frozen screen: a band of light crosses the
//! selection while it is read, then each word lights up where it is. A drag picks words, and the
//! bar under the selection copies them. What a pointer position means, the text a selection
//! copies and the timing of the motion are in the GPUI-free presenter (`presenters/text_layer`).
//! This module keeps the clock and paints. The clipboard and the recogniser stay with the
//! session: a copy goes to it as `TextCommand::Copy`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px,
};
use gpui_component::tooltip::Tooltip;
use laqta_domain::region::{RegionInput, TextCommand};
use laqta_ports::{OcrResult, TextLayer};

use crate::assets::IconName;
use crate::overlay::OverlayModel;
use crate::overlay_tools::{ACCENT, BAR_H};
use crate::presenters::capture_toolbar as bar;
use crate::presenters::text_layer::{Layout, Notice, Pick, Rect, motion};
use crate::theme::radius;

/// How dark the selection gets under the band of light, and once its words are lit. The second
/// is close to the dimming outside the selection, so the lit words are what stands out.
const SCRIM_READING: f32 = 0.16;
const SCRIM_READ: f32 = 0.36;
/// How bright a word's highlight is, and how much brighter under the pointer.
const WORD: f32 = 0.22;
const WORD_HOVER: f32 = 0.36;
/// How strong the picked words' colour is.
const PICKED: f32 = 0.42;
/// The highlight reaches this far past a word's box, in logical pixels.
const PAD_X: f32 = 2.0;
const PAD_Y: f32 = 1.0;
/// The colour of a finished copy.
const DONE: u32 = 0x4ade80;
const FAILED: u32 = 0xff6b6b;

enum Phase {
    Reading,
    Read,
    Failed(String),
}

/// The text layer's state on the GPUI side.
pub(crate) struct TextUi {
    phase: Phase,
    /// When the phase began. Its animations run from here.
    since: Instant,
    /// The words and the ones the user picked.
    pub(crate) pick: Pick,
    /// The result the words were laid out from, to tell a new result from a repaint of the same.
    result: Option<Arc<OcrResult>>,
    /// The clipboard holds the whole text.
    copied_all: bool,
    /// The user's last copy: when, and how many words (`None` is everything).
    copy: Option<(Instant, Option<usize>)>,
    /// The session said that copy did not reach the clipboard.
    copy_failed: bool,
    /// Enter came while reading: leave the text once it is read.
    leave_when_read: bool,
}

fn ms(from: Instant, now: Instant) -> f32 {
    now.saturating_duration_since(from).as_secs_f32() * 1000.0
}

fn rgba(rgb: u32, alpha: f32) -> Hsla {
    let mut c: Hsla = gpui::rgb(rgb).into();
    c.a = alpha.clamp(0.0, 1.0);
    c
}

impl TextUi {
    fn new(phase: Phase, now: Instant) -> Self {
        TextUi {
            phase,
            since: now,
            pick: Pick::default(),
            result: None,
            copied_all: false,
            copy: None,
            copy_failed: false,
            leave_when_read: false,
        }
    }

    /// Words are shown and can be picked.
    fn has_words(&self) -> bool {
        matches!(self.phase, Phase::Read) && !self.pick.layout().is_empty()
    }

    /// What the bar says at `now`.
    pub(crate) fn notice(&self, now: Instant) -> Notice {
        match &self.phase {
            Phase::Reading => Notice::Reading,
            Phase::Failed(message) => Notice::Failed(message.clone()),
            Phase::Read if self.pick.layout().is_empty() => Notice::NoText,
            Phase::Read => match (self.copy, self.pick.count()) {
                (Some((at, _)), _) if ms(at, now) < motion::NOTICE_MS && self.copy_failed => Notice::CopyFailed,
                (Some((at, count)), _) if ms(at, now) < motion::NOTICE_MS => Notice::Copied(count),
                (_, 0) => Notice::Found { words: self.pick.layout().tokens().len(), copied: self.copied_all },
                (_, count) => Notice::Picked(count),
            },
        }
    }

    /// Something is still moving at `now`, so the overlay must paint another frame.
    pub(crate) fn animating(&self, now: Instant) -> bool {
        let copied = self.copy.is_some_and(|(at, _)| ms(at, now) < motion::FLASH_MS);
        match self.phase {
            Phase::Reading => true,
            Phase::Read => ms(self.since, now) < motion::REVEAL_MS || copied,
            Phase::Failed(_) => ms(self.since, now) < motion::ENTER_MS,
        }
    }

    /// The brightness a copy adds at `now`.
    fn flash(&self, now: Instant) -> f32 {
        if self.copy_failed {
            return 0.0;
        }
        self.copy.map_or(0.0, |(at, _)| motion::flash(ms(at, now)))
    }
}

impl OverlayModel {
    /// Follows `RegionView::text` after a new view: starts the reading animation, lays out a new
    /// result, or drops the layer. A repaint of the same result keeps the user's selection.
    pub(crate) fn sync_text(&mut self) {
        let now = Instant::now();
        match &self.view.text {
            None => self.text = None,
            Some(TextLayer::Reading) => {
                if !matches!(self.text, Some(TextUi { phase: Phase::Reading, .. })) {
                    self.text = Some(TextUi::new(Phase::Reading, now));
                }
            }
            Some(TextLayer::Read { result, copied, copy_failed }) => {
                let same = self.text.as_ref().and_then(|t| t.result.as_ref()).is_some_and(|shown| Arc::ptr_eq(shown, result));
                if let Some(text) = self.text.as_mut().filter(|_| same) {
                    text.copy_failed = *copy_failed;
                    // A refused copy of everything leaves the clipboard without the text.
                    if *copy_failed {
                        text.copied_all = false;
                    }
                } else {
                    let leave = self.text.as_ref().is_some_and(|t| t.leave_when_read);
                    self.text = Some(TextUi {
                        pick: Pick::new(Layout::new(result)),
                        result: Some(result.clone()),
                        copied_all: *copied,
                        ..TextUi::new(Phase::Read, now)
                    });
                    // The session copied the whole text with the read, which is what Enter asked for.
                    // When it could not, the text stays so the user sees what was read.
                    if leave && *copied {
                        self.close_text();
                    }
                }
            }
            Some(TextLayer::Failed(message)) => {
                if !matches!(self.text, Some(TextUi { phase: Phase::Failed(_), .. })) {
                    self.text = Some(TextUi::new(Phase::Failed(message.clone()), now));
                }
            }
        }
    }

    /// The text button: read the selection's text in place, or leave the text if it is showing.
    pub(crate) fn toggle_text(&mut self) {
        if self.text.is_some() { self.close_text() } else { self.read_text() }
    }

    /// Asks the session to read the selection. The pointer inside the selection belongs to the
    /// text from then on, so a drawing tool is put away first.
    pub(crate) fn read_text(&mut self) {
        self.region_tool();
        (self.on_input)(RegionInput::Text(TextCommand::Read));
    }

    /// Leaves the text: back to the selection and its toolbar, or the end of a text capture.
    pub(crate) fn close_text(&mut self) {
        self.text_mode = false;
        (self.on_input)(RegionInput::Text(TextCommand::Close));
    }

    /// Copies the picked words, or everything when none are picked. The session owns the
    /// clipboard, so the text goes to it; the bar says "Copied" for a moment.
    pub(crate) fn copy_text(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.text.as_mut().filter(|t| t.has_words()) else { return };
        let (words, count) = match text.pick.text() {
            Some(picked) => (picked, Some(text.pick.count())),
            None => (text.pick.layout().all_text(), None),
        };
        text.copy = Some((Instant::now(), count));
        text.copy_failed = false;
        text.copied_all = count.is_none();
        (self.on_input)(RegionInput::Text(TextCommand::Copy(words)));
        // One more paint when the notice is over, with or without animation frames.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(motion::NOTICE_MS as u64 + 40)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
    }

    fn pick_all_text(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.text.as_mut().filter(|t| t.has_words()) {
            text.pick.pick_all();
            cx.notify();
        }
    }

    /// Ends the session and shows the text in the text window, where it can be edited and saved.
    fn open_text_window(&mut self, _: &mut Context<Self>) {
        (self.on_input)(RegionInput::Text(TextCommand::OpenWindow));
    }

    /// A key while the text is showing. Escape drops the picked words, then leaves the text.
    /// Enter copies and leaves, once the text is read and copied. Ctrl+C copies, Ctrl+A picks everything. Other keys do nothing:
    /// nudging the selection would read it again on every press.
    pub(crate) fn text_key(&mut self, key: &str, ctrl: bool, cx: &mut Context<Self>) {
        match key {
            "escape" => {
                if self.text.as_mut().is_some_and(|t| t.pick.clear()) {
                    cx.notify();
                } else {
                    self.close_text();
                }
            }
            "enter" => {
                if let Some(text) = self.text.as_mut().filter(|t| matches!(t.phase, Phase::Reading)) {
                    text.leave_when_read = true;
                    return;
                }
                self.copy_text(cx);
                self.close_text();
            }
            "c" if ctrl => self.copy_text(cx),
            "a" if ctrl => self.pick_all_text(cx),
            _ => {}
        }
    }

    /// The desktop point (`x`, `y`) in the selection's pixels, when the text takes the pointer
    /// there: inside the selection and off its handles, which still resize it. A locked selection
    /// hides its handles and cannot be resized, so there the words reach its edges.
    pub(crate) fn text_point(&self, x: i32, y: i32) -> Option<(f32, f32)> {
        self.text.as_ref()?;
        let sel = self.view.selected?;
        let within = x >= sel.x && x < sel.right() && y >= sel.y && y < sel.bottom();
        (within && self.view.handle_at((x, y)).is_none()).then(|| ((x - sel.x) as f32, (y - sel.y) as f32))
    }

    /// A left press on the text at `at` (`text_point`).
    pub(crate) fn text_press(&mut self, at: (f32, f32), extend: bool, clicks: usize, cx: &mut Context<Self>) {
        if let Some(text) = self.text.as_mut().filter(|t| t.has_words()) {
            text.pick.press(at.0, at.1, extend, clicks);
            cx.notify();
        }
    }

    /// The pointer moved to the desktop point (`x`, `y`). Returns true while a drag over the text
    /// is under way, wherever the pointer is: it may leave the selection and still pick.
    pub(crate) fn text_drag(&mut self, x: i32, y: i32, cx: &mut Context<Self>) -> bool {
        let Some(sel) = self.view.selected else { return false };
        let Some(text) = self.text.as_mut().filter(|t| t.pick.dragging()) else { return false };
        if text.pick.drag_to((x - sel.x) as f32, (y - sel.y) as f32) {
            cx.notify();
        }
        true
    }

    /// The left button came up. Returns true if it ended a drag over the text.
    pub(crate) fn text_release(&mut self, cx: &mut Context<Self>) -> bool {
        let ended = self.text.as_mut().is_some_and(|t| t.pick.release());
        if ended {
            cx.notify();
        }
        ended
    }
}

fn fill(color: Hsla) -> gpui::Div {
    div().absolute().left_0().top_0().size_full().bg(color)
}

/// The text over the selection: `at` is the selection in this window's logical pixels and `s` the
/// window's scale. With `still` (the system asks for less motion) everything shows at once.
pub(crate) fn layer(text: &TextUi, at: bar::Rect, s: f32, now: Instant, still: bool) -> Vec<AnyElement> {
    let elapsed = if still { f32::MAX } else { ms(text.since, now) };
    let mut inside = div().absolute().left(px(at.x)).top(px(at.y)).w(px(at.w)).h(px(at.h)).overflow_hidden().cursor_default();
    let mut out = Vec::new();
    match &text.phase {
        Phase::Reading => {
            inside = inside.child(fill(rgba(0x000000, SCRIM_READING)));
            if !still {
                // A soft band of light, brightest in its middle, crossing left to right.
                let half = (motion::BAND * at.w / 2.0).max(40.0);
                let centre = motion::sweep(elapsed) * at.w;
                let (clear, lit) = (rgba(0xffffff, 0.0), rgba(0xffffff, 0.30));
                let side = |x: f32, from: Hsla, to: Hsla| {
                    div().absolute().left(px(x)).top_0().w(px(half)).h_full().bg(linear_gradient(
                        90.,
                        linear_color_stop(from, 0.),
                        linear_color_stop(to, 1.),
                    ))
                };
                inside = inside.child(side(centre - half, clear, lit)).child(side(centre, lit, clear));
                // The outline breathes while the selection is read.
                let glow = 0.25 + 0.45 * motion::breath(elapsed);
                out.push(
                    div()
                        .absolute()
                        .left(px(at.x - 3.0))
                        .top(px(at.y - 3.0))
                        .w(px(at.w + 6.0))
                        .h(px(at.h + 6.0))
                        .rounded(px(5.))
                        .border_3()
                        .border_color(rgba(ACCENT, glow))
                        .into_any_element(),
                );
            }
        }
        Phase::Failed(_) => inside = inside.child(fill(rgba(0x000000, SCRIM_READING))),
        Phase::Read => {
            let layout = text.pick.layout();
            let dim = if layout.is_empty() { 0.0 } else { motion::ease_out(elapsed / motion::ENTER_MS) };
            inside = inside.child(fill(rgba(0x000000, SCRIM_READING + (SCRIM_READ - SCRIM_READING) * dim)));
            let picked = text.pick.picked();
            let flash = text.flash(now);
            let size = (at.w * s, at.h * s);
            // A copy of everything brightens every word; a copy of some brightens those.
            let all = if picked.is_none() { flash } else { 0.0 };
            for (index, token) in layout.tokens().iter().enumerate() {
                if picked.as_ref().is_some_and(|picked| picked.contains(&index)) {
                    continue;
                }
                let shown = motion::reveal(elapsed, layout.along(index, size));
                if shown > 0.0 {
                    inside = inside.child(word(token.rect, s, shown, all));
                }
            }
            if let Some(picked) = &picked {
                for line in layout.bars(picked) {
                    inside = inside.child(boxed(line, s, 0.0).bg(rgba(ACCENT, PICKED + 0.35 * flash)).cursor_text());
                }
            }
        }
    }
    out.insert(0, inside.into_any_element());
    out
}

/// A rounded box over `rect` (the selection's physical pixels), `rise` logical pixels lower.
fn boxed(rect: Rect, s: f32, rise: f32) -> gpui::Div {
    div()
        .absolute()
        .left(px(rect.x / s - PAD_X))
        .top(px(rect.y / s - PAD_Y + rise))
        .w(px(rect.w / s + 2.0 * PAD_X))
        .h(px(rect.h / s + 2.0 * PAD_Y))
        .rounded(px(4.))
}

/// One word's highlight, `shown` of the way in: it fades in and settles upwards.
fn word(rect: Rect, s: f32, shown: f32, flash: f32) -> gpui::Div {
    boxed(rect, s, (1.0 - shown) * 3.0)
        .bg(rgba(0xffffff, (WORD + 0.30 * flash) * shown))
        .cursor_text()
        .hover(|style| style.bg(rgba(0xffffff, WORD_HOVER)))
}

/// The bar's size for what it says, on a window `screen_w` wide. On a narrow one it shrinks to
/// fit, and its line is cut short rather than its buttons pushed off the edge.
pub(crate) fn bar_size(notice: &Notice, screen_w: f32) -> (f32, f32) {
    let width: f32 = match notice {
        Notice::Reading => 196.0,
        Notice::NoText => 360.0,
        Notice::Failed(_) | Notice::CopyFailed => 480.0,
        Notice::Copied(_) | Notice::Picked(_) | Notice::Found { .. } => 560.0,
    };
    (width.min((screen_w - 2.0 * bar::GAP).max(0.0)), BAR_H)
}

/// The bar under the selection: what happened on the left, what to do about it on the right.
pub(crate) fn bar(model: &Entity<OverlayModel>, at: bar::Rect, now: Instant, still: bool, cx: &App) -> AnyElement {
    let Some(text) = model.read(cx).text.as_ref() else { return div().into_any_element() };
    let notice = text.notice(now);
    let elapsed = if still { f32::MAX } else { ms(text.since, now) };
    let enter = motion::ease_out(elapsed / motion::ENTER_MS);
    let (icon, tint, breath) = match &notice {
        Notice::Reading => (IconName::Sparkles, ACCENT, if still { 1.0 } else { 0.45 + 0.55 * motion::breath(elapsed) }),
        Notice::NoText => (IconName::ScanLine, 0xffffff, 0.7),
        Notice::Failed(_) | Notice::CopyFailed => (IconName::CircleX, FAILED, 1.0),
        _ if notice.done() => (IconName::Check, DONE, 1.0),
        _ => (IconName::ScanText, 0xffffff, 0.9),
    };
    let (mut line, hint) = notice.text();
    if matches!(notice, Notice::Reading) && !still {
        line.push_str(&".".repeat(motion::dots(elapsed)));
    }
    let said = div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(8.))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_size(px(12.5))
        .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(rgba(0xffffff, 0.94)).child(line))
        .when_some(hint, |d, hint| d.child(div().min_w_0().truncate().text_color(rgba(0xffffff, 0.52)).child(hint)));
    let mut row = div()
        .id("text-bar")
        .absolute()
        .left(px(at.x))
        .top(px(at.y + (1.0 - enter) * 6.0))
        .w(px(at.w))
        .h(px(at.h))
        .pl(px(12.))
        .pr(px(6.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(radius::LG)
        .bg(rgba(0x1c1c1f, 0.97))
        .border_1()
        .border_color(rgba(0xffffff, 0.10))
        .shadow_lg()
        .cursor_default()
        .opacity(enter)
        // Clicks on the bar never reach the words or the selection under it.
        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(div().flex_none().opacity(breath).child(gpui_component::Icon::new(icon).size(px(16.)).text_color(rgba(tint, 1.0))))
        .child(said);
    if text.has_words() {
        let some = text.pick.count() > 0;
        row = row
            .child(button(
                model,
                "text-copy",
                Some(IconName::Copy),
                Some(if some { "Copy" } else { "Copy all" }),
                tip(if some { "Copy the selected words" } else { "Copy all the text" }, "Ctrl+C"),
                true,
                OverlayModel::copy_text,
            ))
            .child(button(
                model,
                "text-all",
                None,
                Some("Select all"),
                tip("Select every word", "Ctrl+A"),
                false,
                OverlayModel::pick_all_text,
            ))
            .child(button(
                model,
                "text-window",
                Some(IconName::SquarePen),
                None,
                "Open as text you can edit and save".to_owned(),
                false,
                OverlayModel::open_text_window,
            ))
            .child(div().flex_none().w(px(1.)).h(px(20.)).bg(rgba(0xffffff, 0.14)));
    }
    row.child(button(model, "text-close", Some(IconName::X), None, tip("Close", "Esc"), false, |m, _| m.close_text())).into_any_element()
}

fn tip(label: &str, keys: &str) -> String {
    format!("{label}  ·  {}", crate::presenters::hotkeys::shortcut_text(keys, crate::host_os()))
}

/// A button on the bar: an icon, a label, or both. `primary` is the one the bar is there for.
fn button(
    model: &Entity<OverlayModel>,
    id: &'static str,
    icon: Option<IconName>,
    label: Option<&'static str>,
    tip: String,
    primary: bool,
    act: fn(&mut OverlayModel, &mut Context<OverlayModel>),
) -> AnyElement {
    let (bg, fg, hover) = if primary {
        (rgba(ACCENT, 1.0), rgba(0xffffff, 1.0), rgba(ACCENT, 0.85))
    } else {
        (gpui::transparent_black(), rgba(0xffffff, 0.88), rgba(0xffffff, 0.12))
    };
    let model = model.clone();
    div()
        .id(id)
        .h(px(30.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .rounded(px(8.))
        .bg(bg)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .map(|d| if label.is_some() { d.px(px(10.)) } else { d.w(px(30.)) })
        .when_some(icon, |d, icon| d.child(gpui_component::Icon::new(icon).size(px(15.)).text_color(fg)))
        .when_some(label, |d, label| d.child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(fg).child(label)))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .on_click(move |_, _, cx| model.update(cx, act))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_fits_a_narrow_window() {
        let found = Notice::Found { words: 3, copied: true };
        assert_eq!(bar_size(&found, 1920.0).0, 560.0);
        // A 1024-pixel monitor at 200%.
        let (w, _) = bar_size(&found, 512.0);
        assert!(w + 2.0 * bar::GAP <= 512.0, "{w}");
    }
}
