//! The shared view components every window builds from, styled from the theme tokens
//! (`theme.rs`). Interactive pieces are tab stops with a visible keyboard focus ring; Enter and
//! Space activate them.

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px, relative,
};
use gpui_component::{ActiveTheme as _, Icon};

use crate::assets::IconName;
use crate::theme::{self, radius, space, text};

/// The window body: page colour, UI font, body text.
pub(crate) fn page(cx: &App) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(cx.theme().background)
        .text_color(cx.theme().foreground)
        .font_family(text::UI_FONT)
        .text_size(text::BODY)
}

/// A titled group of settings rows: a muted heading over its rows, no card. Pages space sections
/// apart; rows inside sit close together.
pub(crate) fn section(title: impl Into<SharedString>, cx: &App) -> Div {
    div().flex().flex_col().gap(px(2.)).child(
        div()
            .pb(space::S2)
            .font_family(text::HEADING_FONT)
            .font_weight(FontWeight::MEDIUM)
            .text_size(text::HEADING)
            .text_color(cx.theme().muted_foreground)
            .child(title.into()),
    )
}

/// How wide a row's control is. Controls sit right-aligned in a fixed slot so rows line up.
#[derive(Clone, Copy)]
pub(crate) enum Slot {
    /// Text, path and select fields.
    Field,
    /// Numbers.
    Number,
    /// Switches, segmented controls and buttons: as wide as they are.
    Hug,
}

/// A settings row: the label (and an optional description under it) on the left, the control on
/// the right.
pub(crate) fn row(label: impl Into<SharedString>, control: impl IntoElement, cx: &App) -> Div {
    row_with(label, None::<SharedString>, Slot::Field, control, cx)
}

/// A settings row with a description line and an explicit control slot.
pub(crate) fn row_with(
    label: impl Into<SharedString>,
    description: Option<impl Into<SharedString>>,
    slot: Slot,
    control: impl IntoElement,
    cx: &App,
) -> Div {
    row_full(label, description, slot, None, control, cx)
}

/// A settings row whose "Reset to default" button sits just left of the control's slot, so the
/// control keeps its width (only pass `reset` while the value differs from its default).
pub(crate) fn row_full(
    label: impl Into<SharedString>,
    description: Option<impl Into<SharedString>>,
    slot: Slot,
    reset: Option<Reset>,
    control: impl IntoElement,
    cx: &App,
) -> Div {
    let t = cx.theme();
    let mut text_col = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(div().text_size(text::BODY).font_weight(FontWeight::MEDIUM).text_color(t.foreground).child(label.into()));
    if let Some(d) = description {
        text_col = text_col.child(div().max_w(px(512.)).text_size(text::LABEL).text_color(t.muted_foreground).child(d.into()));
    }
    let slot_el = match slot {
        Slot::Field => div().w(px(300.)),
        Slot::Number => div().w(px(112.)),
        Slot::Hug => div(),
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::S6)
        .min_h(px(52.))
        .py(px(8.))
        .child(text_col)
        .children(reset.map(|r| reset_button(r, cx)))
        .child(slot_el.flex_none().flex().justify_end().child(control))
}

/// A row whose control is a switch, toggled from the whole row. `reset` shows the "Reset to
/// default" button beside the switch (only pass it while the value differs from its default).
pub(crate) fn switch_row(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    description: Option<impl Into<SharedString>>,
    checked: bool,
    reset: Option<Reset>,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let id: ElementId = id.into();
    let switch = gpui_component::switch::Switch::new(id).checked(checked).on_click(on_change);
    row_full(label, description, Slot::Hug, reset, switch, cx)
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The small ↺ button that puts one setting back to its default.
pub(crate) struct Reset {
    id: SharedString,
    tooltip: SharedString,
    on_click: ClickHandler,
}

impl Reset {
    /// `tooltip` names the default: "Reset to default (on)".
    pub(crate) fn new(
        id: impl Into<SharedString>,
        tooltip: impl Into<SharedString>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Reset { id: id.into(), tooltip: tooltip.into(), on_click: Box::new(on_click) }
    }
}

/// `control` with the reset button before it, when there is one.
pub(crate) fn with_reset(reset: Option<Reset>, control: impl IntoElement, cx: &App) -> Div {
    div().flex().items_center().gap(space::S2).children(reset.map(|r| reset_button(r, cx))).child(control)
}

/// The ↺ button itself.
fn reset_button(Reset { id, tooltip, on_click }: Reset, cx: &App) -> gpui::Stateful<Div> {
    let t = cx.theme();
    let (fg, hover_bg, hover_fg) = (t.muted_foreground, t.secondary, t.foreground);
    let selector = id.to_string();
    div()
        .id(ElementId::Name(id))
        .debug_selector(move || selector)
        .size(px(26.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::SM)
        .text_color(fg)
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        .on_click(move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx))
        .child(Icon::new(IconName::RotateCcw).size(px(14.)))
}

/// A row whose control is a button.
pub(crate) fn action_row(
    label: impl Into<SharedString>,
    description: Option<impl Into<SharedString>>,
    button: impl IntoElement,
    cx: &App,
) -> Div {
    row_with(label, description, Slot::Hug, button, cx)
}

/// The heading of a settings page: a title and one line that says what the page is for.
pub(crate) fn settings_header(title: impl Into<SharedString>, description: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(space::S1)
        .pb(space::S4)
        .child(div().font_family(text::HEADING_FONT).font_weight(FontWeight::MEDIUM).text_size(px(22.5)).child(title.into()))
        .child(div().text_size(text::LABEL).text_color(cx.theme().muted_foreground).child(description.into()))
}

/// Secondary text.
pub(crate) fn muted(text: impl Into<SharedString>, cx: &App) -> Div {
    div().text_size(text::CAPTION).text_color(cx.theme().muted_foreground).child(text.into())
}

/// The flat card surface: 24px corners, a hairline ring, no shadow.
pub(crate) fn card(cx: &App) -> Div {
    let x = theme::extra(cx);
    div().rounded(radius::CARD).bg(x.card).border_1().border_color(x.ring)
}

/// A page heading with a one-line description.
pub(crate) fn page_title(title: impl Into<SharedString>, description: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(
            div()
                .font_family(text::HEADING_FONT)
                .font_weight(FontWeight::MEDIUM)
                .text_size(text::PAGE_TITLE)
                .line_height(relative(1.04))
                .child(title.into()),
        )
        .child(div().text_size(text::BODY).text_color(cx.theme().muted_foreground).child(description.into()))
}

/// A heading over a group on a page, with an optional trailing element.
pub(crate) fn group_title(title: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .pb(space::S3)
        .text_color(cx.theme().foreground)
        .child(div().font_family(text::HEADING_FONT).font_weight(FontWeight::MEDIUM).text_size(text::SECTION).child(title.into()))
}

pub(crate) fn icon(name: IconName, color: Hsla) -> Icon {
    Icon::new(name).size(px(16.)).text_color(color)
}

/// Makes an element a keyboard tab stop with a focus ring, a pointer cursor and a click handler.
pub(crate) fn clickable(el: Stateful<Div>, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static, cx: &App) -> Stateful<Div> {
    // Every caller gives the element a 1px border, so the ring never shifts layout.
    let ring = cx.theme().foreground.opacity(0.6);
    el.tab_index(0).cursor_pointer().focus_visible(move |s| s.border_color(ring)).on_click(on_click)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pill {
    /// Neutral grey fill: routine actions.
    Default,
    /// Brand green: the one key action in a region.
    Primary,
    /// Ink fill: the strong neutral action.
    Dark,
    /// No fill until hovered.
    Ghost,
}

/// A one-line pill button (36px, or 28px when `small`).
pub(crate) fn pill(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: Pill,
    small: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.theme();
    let x = theme::extra(cx);
    let (bg, fg, hover) = match kind {
        Pill::Default => (t.secondary, t.secondary_foreground, t.secondary_hover),
        Pill::Primary => (t.primary, t.primary_foreground, t.primary_hover),
        Pill::Dark => (x.action, t.background, x.action.opacity(0.85)),
        Pill::Ghost => (gpui::transparent_black(), t.foreground, x.wash),
    };
    let el = div()
        .id(id)
        .h(if small { px(28.) } else { theme::CONTROL_H })
        .px(if small { space::S3 } else { space::S4 })
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(space::S2)
        .rounded(radius::PILL)
        .border_1()
        .border_color(gpui::transparent_black())
        .bg(bg)
        .text_color(fg)
        .text_size(if small { px(12.5) } else { text::LABEL })
        .font_weight(FontWeight::MEDIUM)
        .hover(move |s| s.bg(hover))
        .child(label.into());
    clickable(el, on_click, cx)
}

/// A pill nav row (sidebar, settings pages). The active row sits on the accent surface.
pub(crate) fn nav_row(
    id: impl Into<ElementId>,
    glyph: IconName,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let t = cx.theme();
    let wash = theme::extra(cx).wash;
    let (fg, ic) = if active { (t.foreground, t.foreground) } else { (t.foreground.opacity(0.86), t.muted_foreground) };
    let id: ElementId = id.into();
    let selector = id.to_string();
    let mut row = div()
        .id(id)
        .debug_selector(move || selector)
        .h(theme::NAV_ROW_H)
        .px(space::S3)
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S3)
        .rounded(radius::PILL)
        .border_1()
        .border_color(gpui::transparent_black())
        .text_size(text::ROW)
        .text_color(fg)
        .hover(move |s| s.bg(wash))
        .child(icon(glyph, ic))
        .child(label.into());
    if active {
        row = row.bg(t.accent).font_weight(FontWeight::MEDIUM);
    }
    clickable(row, on_click, cx).into_any_element()
}

/// A small group label in a nav list.
pub(crate) fn nav_group(label: &'static str, cx: &App) -> Div {
    div()
        .px(space::S3)
        .pt(space::S4)
        .pb(space::S1)
        .text_size(text::CAPTION)
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(label)
}

/// A 36px rounded square behind an icon. `accent` tints it with the brand colour.
pub(crate) fn icon_tile(glyph: IconName, accent: bool, cx: &App) -> Div {
    let t = cx.theme();
    let (bg, fg) = if accent { (theme::extra(cx).chip, t.primary) } else { (t.secondary, t.foreground.opacity(0.8)) };
    div().size(theme::CONTROL_H).flex_none().rounded(radius::MD).bg(bg).flex().items_center().justify_center().child(icon(glyph, fg))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Neutral,
    Success,
    Danger,
    Busy,
}

/// A 20px status badge with a coloured dot, so colour is never the only cue.
pub(crate) fn badge(label: impl Into<SharedString>, tone: Tone, cx: &App) -> Div {
    let t = cx.theme();
    let dot = match tone {
        Tone::Neutral => t.muted_foreground,
        Tone::Success => t.primary,
        Tone::Danger => t.danger,
        Tone::Busy => theme::extra(cx).info,
    };
    div()
        .flex_none()
        .h(px(20.))
        .px(space::S2)
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(radius::PILL)
        .bg(t.secondary)
        .text_size(text::CAPTION)
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.muted_foreground)
        .child(div().size(px(6.)).rounded(radius::PILL).bg(dot))
        .child(label.into())
}

/// A key-chord hint, such as a hotkey: quiet text, so it never competes with the label.
pub(crate) fn kbd(keys: impl Into<SharedString>, cx: &App) -> Div {
    div().flex_none().text_size(text::SMALL).text_color(cx.theme().muted_foreground).child(keys.into())
}

/// An empty state: an icon tile, a title and a line telling the user what to do.
pub(crate) fn empty_state(glyph: IconName, title: impl Into<SharedString>, body: impl Into<SharedString>, cx: &App) -> Div {
    let t = cx.theme();
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(space::S2)
        .py(space::S8)
        .px(space::S4)
        .child(
            div()
                .size(px(48.))
                .rounded(radius::CARD)
                .bg(t.muted)
                .flex()
                .items_center()
                .justify_center()
                .child(icon(glyph, t.muted_foreground)),
        )
        .child(div().pt(space::S1).text_size(text::ROW).font_weight(FontWeight::MEDIUM).child(title.into()))
        .child(div().text_size(text::LABEL).text_color(t.muted_foreground).text_center().child(body.into()))
}

/// A message line under a form: neutral, or danger for errors.
pub(crate) fn notice(text_: impl Into<SharedString>, ok: bool, cx: &App) -> Div {
    let t = cx.theme();
    div().text_size(text::LABEL).text_color(if ok { t.muted_foreground } else { t.danger }).child(text_.into())
}

/// Tab and Shift+Tab move focus between tab stops. Put it on a window's root element and focus
/// that element when the window opens.
pub(crate) fn tab_keys(el: Div, focus: &gpui::FocusHandle) -> Div {
    el.track_focus(focus).on_key_down(|ev: &gpui::KeyDownEvent, window, cx| {
        if ev.keystroke.key == "tab" {
            if ev.keystroke.modifiers.shift {
                window.focus_prev(cx);
            } else {
                window.focus_next(cx);
            }
            cx.stop_propagation();
        }
    })
}

/// A click handler stored for later, as a segmented control's options hold them.
pub(crate) type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// One option of a segmented control.
pub(crate) struct Segment {
    pub id: SharedString,
    pub label: SharedString,
    pub selected: bool,
    pub on_click: OnClick,
}

/// A segmented control: a pill track where one segment is chosen.
pub(crate) fn segmented(options: Vec<Segment>, cx: &App) -> Div {
    let t = cx.theme();
    let x = theme::extra(cx);
    let track_bg = if t.is_dark() { t.foreground.opacity(0.06) } else { t.secondary };
    let mut track = div().flex().flex_none().items_center().gap(px(2.)).p(px(3.)).rounded(radius::PILL).bg(track_bg);
    for Segment { id, label, selected, on_click } in options {
        let fg = if selected { t.foreground } else { t.muted_foreground };
        let hover_fg = t.foreground;
        let mut seg = div()
            .id(ElementId::Name(id))
            .h(px(30.))
            .px(space::S3)
            .flex()
            .items_center()
            .rounded(radius::PILL)
            .border_1()
            .border_color(gpui::transparent_black())
            .text_size(text::LABEL)
            .font_weight(FontWeight::MEDIUM)
            .text_color(fg)
            .hover(move |s| s.text_color(hover_fg))
            .child(label);
        if selected {
            // Light: a white chip on the grey track. Dark: a lighter step, since the card is darker.
            let chip = if t.is_dark() { t.foreground.opacity(0.16) } else { x.card };
            seg = seg.bg(chip).shadow(vec![gpui::BoxShadow {
                color: gpui::black().opacity(0.08),
                offset: gpui::point(px(0.), px(1.)),
                blur_radius: px(2.),
                spread_radius: px(0.),
                inset: false,
            }]);
        }
        track = track.child(clickable(seg, on_click, cx));
    }
    // A flex row around the track keeps it as wide as its segments.
    div().flex().child(track)
}

/// A click handler several menu items can share.
pub(crate) type SharedClick = std::rc::Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// One option of a [`choice`].
pub(crate) struct Choice {
    pub label: SharedString,
    pub selected: bool,
    pub on_click: SharedClick,
}

impl Choice {
    pub(crate) fn new(
        label: impl Into<SharedString>,
        selected: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Choice { label: label.into(), selected, on_click: std::rc::Rc::new(on_click) }
    }
}

/// Pick one of several options. Up to four short options show as a segmented control; more, or
/// long labels, become a dropdown that shows the current choice.
pub(crate) fn choice(id: impl Into<SharedString>, options: Vec<Choice>, cx: &App) -> AnyElement {
    let id: SharedString = id.into();
    let total_len: usize = options.iter().map(|o| o.label.len()).sum();
    if options.len() <= 4 && total_len <= 40 {
        let segs = options
            .into_iter()
            .enumerate()
            .map(|(i, o)| {
                let h = o.on_click.clone();
                Segment {
                    id: format!("{id}-{i}").into(),
                    label: o.label,
                    selected: o.selected,
                    on_click: Box::new(move |e, w, cx| h(e, w, cx)),
                }
            })
            .collect();
        return segmented(segs, cx).into_any_element();
    }
    dropdown(id, options, cx)
}

/// A dropdown that shows the current choice and lists every option, with a tick on the chosen one.
pub(crate) fn dropdown(id: impl Into<SharedString>, options: Vec<Choice>, _cx: &App) -> AnyElement {
    use gpui_component::button::Button;
    use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
    let id: SharedString = id.into();
    let current = options.iter().find(|o| o.selected).map(|o| o.label.clone()).unwrap_or_else(|| "Choose…".into());
    let options = std::rc::Rc::new(options);
    Button::new(ElementId::Name(id))
        .label(current)
        .outline()
        .dropdown_caret(true)
        .w_full()
        .dropdown_menu(move |mut menu, _, _| {
            for o in options.iter() {
                let h = o.on_click.clone();
                menu = menu.item(PopupMenuItem::new(o.label.clone()).checked(o.selected).on_click(move |e, w, cx| h(e, w, cx)));
            }
            menu
        })
        .into_any_element()
}

/// A button that opens a menu of actions, such as "Add account".
pub(crate) fn menu_button(id: impl Into<SharedString>, label: impl Into<SharedString>, items: Vec<Choice>) -> AnyElement {
    menu_button_with(id, label, IconName::Plus, items)
}

/// `menu_button` with its own icon; an empty label makes a compact icon-only "more" button.
pub(crate) fn menu_button_with(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    glyph: IconName,
    items: Vec<Choice>,
) -> AnyElement {
    use gpui_component::button::{Button, ButtonVariants as _};
    use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
    let id: SharedString = id.into();
    let label: SharedString = label.into();
    let items = std::rc::Rc::new(items);
    let button = Button::new(ElementId::Name(id)).icon(Icon::new(glyph));
    let button = if label.is_empty() { button.ghost() } else { button.label(label).outline() };
    button
        .dropdown_menu(move |mut menu, _, _| {
            for o in items.iter() {
                let h = o.on_click.clone();
                menu = menu.item(PopupMenuItem::new(o.label.clone()).on_click(move |e, w, cx| h(e, w, cx)));
            }
            menu
        })
        .into_any_element()
}

/// A square icon button for toolbars and tool rails, with a tooltip. `active` marks the chosen
/// tool or a toggle that is on.
pub(crate) fn icon_button(
    id: impl Into<ElementId>,
    glyph: IconName,
    tooltip: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.theme();
    let wash = theme::extra(cx).wash;
    let tip: SharedString = tooltip.into();
    let (bg, fg) =
        if active { (t.foreground.opacity(0.14), t.foreground) } else { (gpui::transparent_black(), t.foreground.opacity(0.72)) };
    let hover_fg = t.foreground;
    let el = div()
        .id(id)
        .size(px(34.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::MD)
        .border_1()
        .border_color(gpui::transparent_black())
        .bg(bg)
        .text_color(fg)
        .when(!active, move |d| d.hover(move |s| s.bg(wash).text_color(hover_fg)))
        .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
        .child(Icon::new(glyph).size(px(18.)));
    clickable(el, on_click, cx)
}

/// A thin vertical line between toolbar groups.
pub(crate) fn divider_v(cx: &App) -> Div {
    div().w(px(1.)).h(px(20.)).mx(space::S1).flex_none().bg(cx.theme().foreground.opacity(0.1))
}

/// A thin horizontal line between rail groups.
pub(crate) fn divider_h(cx: &App) -> Div {
    div().h(px(1.)).mx(space::S2).my(px(6.)).flex_none().bg(cx.theme().foreground.opacity(0.1))
}

/// A colour swatch: a circle with a ring around the chosen one.
pub(crate) fn swatch(
    id: impl Into<ElementId>,
    color: Hsla,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.theme();
    let ring = if selected { t.foreground } else { gpui::transparent_black() };
    let hover_ring = t.foreground.opacity(0.35);
    let el = div()
        .id(id)
        .size(px(24.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::PILL)
        .border_2()
        .border_color(ring)
        .when(!selected, move |d| d.hover(move |s| s.border_color(hover_ring)))
        .child(div().size(px(16.)).rounded(radius::PILL).bg(color).border_1().border_color(t.foreground.opacity(0.18)));
    clickable(el, on_click, cx)
}

/// A compact number stepper: a short label, minus, the value, plus, on one pill track.
pub(crate) fn stepper(
    id: &str,
    label: &'static str,
    tooltip: &'static str,
    value: impl Into<SharedString>,
    on_less: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_more: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.theme();
    let wash = theme::extra(cx).wash;
    let step = |sid: String, glyph: IconName, on: OnClick| {
        let el = div()
            .id(ElementId::Name(sid.into()))
            .size(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius::PILL)
            .border_1()
            .border_color(gpui::transparent_black())
            .text_color(t.foreground.opacity(0.8))
            .hover(move |s| s.bg(wash))
            .child(Icon::new(glyph).size(px(13.)));
        clickable(el, on, cx)
    };
    div()
        .id(ElementId::Name(format!("{id}-stepper").into()))
        .h(px(34.))
        .pl(space::S3)
        .pr(px(4.))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(2.))
        .rounded(radius::PILL)
        .bg(t.foreground.opacity(0.06))
        .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tooltip).build(window, cx))
        .child(div().pr(space::S1).text_size(text::SMALL).text_color(t.muted_foreground).child(label))
        .child(step(format!("{id}-less"), IconName::Minus, Box::new(on_less)))
        .child(div().min_w(px(40.)).text_center().text_size(text::SMALL).font_weight(FontWeight::MEDIUM).child(value.into()))
        .child(step(format!("{id}-more"), IconName::Plus, Box::new(on_more)))
}

/// A floating pill on a canvas stage: a raised, ringed surface that holds small controls.
pub(crate) fn floating(cx: &App) -> Div {
    let t = cx.theme();
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .p(px(4.))
        .rounded(radius::PILL)
        .bg(t.sidebar)
        .border_1()
        .border_color(t.foreground.opacity(0.1))
        .shadow(vec![gpui::BoxShadow {
            color: gpui::black().opacity(0.28),
            offset: gpui::point(px(0.), px(6.)),
            blur_radius: px(18.),
            spread_radius: px(0.),
            inset: false,
        }])
}

/// An on/off chip with an icon and a label, for toolbar toggles such as Fill and Shadow.
pub(crate) fn toggle_chip(
    id: impl Into<ElementId>,
    glyph: IconName,
    label: &'static str,
    on: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.theme();
    let wash = theme::extra(cx).wash;
    // On reads in the brand tint, so the state never depends on the label alone.
    let (bg, fg) = if on { (theme::extra(cx).chip, t.primary) } else { (gpui::transparent_black(), t.foreground.opacity(0.72)) };
    let hover_fg = t.foreground;
    let el = div()
        .id(id)
        .h(px(34.))
        .pl(space::S3)
        .pr(space::S3)
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S2)
        .rounded(radius::PILL)
        .border_1()
        .border_color(if on { gpui::transparent_black() } else { t.foreground.opacity(0.12) })
        .bg(bg)
        .text_color(fg)
        .text_size(text::SMALL)
        .font_weight(FontWeight::MEDIUM)
        .when(!on, move |d| d.hover(move |s| s.bg(wash).text_color(hover_fg)))
        .child(Icon::new(glyph).size(px(15.)))
        .child(label);
    clickable(el, on_click, cx)
}

/// One option in a list where a single one is chosen (a model, a preset): a radio dot, the name,
/// a muted detail line, and an optional control at the end. The radio carries the name and the
/// detail for screen readers; the control sits beside it, not inside it. With no `on_pick` the
/// row can't be chosen: no tab stop, pointer or hover, and its text is dimmed.
pub(crate) fn option_row(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    selected: bool,
    trailing: Option<AnyElement>,
    on_pick: Option<OnClick>,
    cx: &App,
) -> Div {
    use gpui::accesskit::{Role, Toggled};
    let t = cx.theme();
    let x = theme::extra(cx);
    let (name, detail): (SharedString, SharedString) = (name.into(), detail.into());
    let enabled = on_pick.is_some();
    let dot = div()
        .flex_none()
        .size(px(16.))
        .rounded(radius::PILL)
        .border_1()
        .border_color(if selected { t.primary } else { t.muted_foreground })
        .flex()
        .items_center()
        .justify_center()
        .when(selected, |d| d.child(div().size(px(8.)).rounded(radius::PILL).bg(t.primary)));
    let radio = div()
        .id(id)
        .role(Role::RadioButton)
        .aria_label(name.clone())
        .aria_description(detail.clone())
        .aria_toggled(if selected { Toggled::True } else { Toggled::False })
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(space::S3)
        .py(space::S2)
        .rounded(radius::MD)
        .border_1()
        .border_color(gpui::transparent_black())
        .when(!enabled, |d| d.opacity(0.6))
        .child(dot)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(text::BODY).font_weight(FontWeight::MEDIUM).text_color(t.foreground).child(name))
                .child(div().text_size(text::LABEL).text_color(t.muted_foreground).child(detail)),
        );
    let radio = match on_pick {
        Some(on_pick) => clickable(radio, on_pick, cx).into_any_element(),
        None => radio.into_any_element(),
    };
    div()
        .flex()
        .items_center()
        .gap(space::S3)
        .min_h(px(52.))
        .px(space::S3)
        .rounded(radius::MD)
        .border_1()
        .border_color(if selected { t.primary.opacity(0.5) } else { gpui::transparent_black() })
        .when(selected, |d| d.bg(x.wash))
        .when(enabled, |d| d.hover(|s| s.bg(x.wash)))
        .child(radio)
        .children(trailing.map(|c| div().flex_none().flex().items_center().gap(space::S2).child(c)))
}
