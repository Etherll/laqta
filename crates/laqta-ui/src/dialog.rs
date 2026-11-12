//! The in-window question dialog behind `window.prompt` on Linux, where GPUI has no native one and
//! falls back to an unthemed white box that clips its text. This one uses the theme tokens and the
//! shared pill buttons: the first answer is the main action and sits on the right, a "Cancel"
//! answer sits furthest left, Enter picks the main action and Escape cancels.

use gpui::accesskit::Role;
use gpui::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, PromptButton, PromptHandle, PromptLevel, PromptResponse, Render, RenderablePromptHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::ActiveTheme as _;

use crate::theme::{self, radius, space, text};
use crate::widgets::{self, Pill};

/// Makes every `window.prompt` in the app use [`Dialog`]. Linux only: macOS and Windows show
/// their native dialogs, which already follow the system theme.
pub(crate) fn install(cx: &mut App) {
    if cfg!(target_os = "linux") {
        cx.set_prompt_builder(build);
    }
}

fn build(
    level: PromptLevel,
    message: &str,
    detail: Option<&str>,
    answers: &[PromptButton],
    handle: PromptHandle,
    window: &mut Window,
    cx: &mut App,
) -> RenderablePromptHandle {
    let view = cx.new(|cx| Dialog {
        level,
        message: message.to_owned().into(),
        detail: detail.map(|d| d.to_owned().into()),
        answers: answers.iter().map(|a| a.label().clone()).collect(),
        cancel: cancel_index(answers),
        focus: cx.focus_handle(),
    });
    handle.with_view(view, window, cx)
}

/// The answer Escape gives: a Cancel button, else one labelled "Cancel", else none.
fn cancel_index(answers: &[PromptButton]) -> Option<usize> {
    answers.iter().position(PromptButton::is_cancel).or_else(|| answers.iter().position(|a| a.label().eq_ignore_ascii_case("cancel")))
}

pub(crate) struct Dialog {
    level: PromptLevel,
    message: SharedString,
    detail: Option<SharedString>,
    answers: Vec<SharedString>,
    cancel: Option<usize>,
    focus: FocusHandle,
}

impl EventEmitter<PromptResponse> for Dialog {}

impl Focusable for Dialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Dialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let x = theme::extra(cx);
        // Destructive questions (reset, remove, discard) get the ink button, not the brand green.
        let main = if matches!(self.level, PromptLevel::Info) { Pill::Primary } else { Pill::Dark };
        let mut buttons = div().flex().flex_wrap().justify_end().gap(space::S2).pt(space::S5);
        // Cancel first (left), then the others from last to first, so answer 0 ends on the right.
        let mut order: Vec<usize> = (0..self.answers.len()).rev().collect();
        if let Some(c) = self.cancel {
            order.retain(|i| *i != c);
            order.insert(0, c);
        }
        for i in order {
            let kind = if i == 0 { main } else { Pill::Default };
            buttons = buttons.child(widgets::pill(
                SharedString::from(format!("prompt-answer-{i}")),
                self.answers[i].clone(),
                kind,
                false,
                cx.listener(move |_, _, _, cx| {
                    cx.emit(PromptResponse(i));
                    cx.stop_propagation();
                }),
                cx,
            ));
        }
        let card = div()
            .id("prompt")
            .role(Role::AlertDialog)
            .aria_label(self.message.clone())
            .w(px(420.))
            .max_w_full()
            .p(space::S6)
            .rounded(radius::DIALOG)
            .bg(x.card)
            .border_1()
            .border_color(x.ring)
            .shadow_lg()
            .text_color(t.foreground)
            .font_family(text::UI_FONT)
            .flex()
            .flex_col()
            .gap(space::S2)
            // Clicks inside the card don't reach the scrim.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div().font_family(text::HEADING_FONT).font_weight(FontWeight::MEDIUM).text_size(text::HEADING).child(self.message.clone()),
            )
            .children(
                self.detail
                    .clone()
                    .map(|d| div().text_size(text::LABEL).line_height(gpui::relative(1.5)).text_color(t.muted_foreground).child(d)),
            )
            .child(buttons);
        let cancel = self.cancel;
        div()
            .track_focus(&self.focus)
            .key_context("Prompt")
            .on_key_down(cx.listener(move |this, ev: &gpui::KeyDownEvent, window, cx| match ev.keystroke.key.as_str() {
                "escape" => {
                    if let Some(c) = cancel {
                        cx.emit(PromptResponse(c));
                    }
                    cx.stop_propagation();
                }
                // Enter on a focused button clicks that button; elsewhere it gives the main answer.
                "enter" if !this.focus.contains_focused(window, cx) || this.focus.is_focused(window) => {
                    cx.emit(PromptResponse(0));
                    cx.stop_propagation();
                }
                "tab" => {
                    if ev.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                }
                _ => {}
            }))
            .size_full()
            .absolute()
            .top_0()
            .left_0()
            .flex()
            .items_center()
            .justify_center()
            .p(space::S4)
            .bg(gpui::black().opacity(if t.is_dark() { 0.55 } else { 0.35 }))
            .child(card)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_answers_the_cancel_button() {
        assert_eq!(cancel_index(&[PromptButton::new("Reset"), PromptButton::new("Cancel")]), Some(1));
        assert_eq!(cancel_index(&[PromptButton::cancel("Not now"), PromptButton::new("Go")]), Some(0));
        assert_eq!(cancel_index(&[PromptButton::new("Keep"), PromptButton::new("Discard")]), None);
    }
}
