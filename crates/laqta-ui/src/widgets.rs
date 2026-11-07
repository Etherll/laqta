//! Small layout helpers shared by the windows.

use gpui::{App, Div, IntoElement, ParentElement as _, SharedString, Styled as _, div, px};
use gpui_component::ActiveTheme as _;

pub(crate) fn page(cx: &App) -> Div {
    div().size_full().flex().flex_col().bg(cx.theme().background).text_color(cx.theme().foreground).text_sm()
}

pub(crate) fn section(title: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(12.))
        .rounded(px(8.))
        .border_1()
        .border_color(cx.theme().border)
        .child(div().text_base().font_weight(gpui::FontWeight::SEMIBOLD).child(title.into()))
}

pub(crate) fn row(label: impl Into<SharedString>, control: impl IntoElement, cx: &App) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(12.))
        .child(div().w(px(180.)).text_color(cx.theme().muted_foreground).child(label.into()))
        .child(div().flex_1().child(control))
}

pub(crate) fn muted(text: impl Into<SharedString>, cx: &App) -> Div {
    div().text_xs().text_color(cx.theme().muted_foreground).child(text.into())
}
