//! The colour picker window: a large swatch, a field that takes any colour notation, every
//! format with one-click copy, tints and shades, and the colours used this session. "Pick from
//! screen" hides the window, lets the user click a pixel (the overlay in point mode), and brings
//! the window back with that colour.

use gpui::{
    AnyWindowHandle, App, AppContext as _, ClipboardItem, Context, Entity, FontWeight, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, WindowOptions, div,
    prelude::FluentBuilder as _, px, size,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::tooltip::Tooltip;
use gpui_component::{ActiveTheme as _, Root};
use laqta_app::{ColorHistory, parse_color};
use laqta_domain::Action;
use laqta_domain::color::Rgba;

use crate::assets::IconName;
use crate::presenters::color::{dark_ink, rows, shades};
use crate::runtime::UiState;
use crate::theme::{radius, space, text};
use crate::widgets::{self, Pill};

/// The open picker and the colours used this session (they outlive the window).
#[derive(Default)]
struct Picker {
    window: Option<AnyWindowHandle>,
    view: Option<WeakEntity<ColorWindow>>,
    recent: ColorHistory,
}

impl Global for Picker {}

fn gpui_color(c: Rgba) -> gpui::Hsla {
    gpui::Rgba { r: f32::from(c.r) / 255.0, g: f32::from(c.g) / 255.0, b: f32::from(c.b) / 255.0, a: f32::from(c.a) / 255.0 }.into()
}

/// Opens the picker, or brings it forward. With `color`, shows that colour and adds it to the
/// session's colours (a pick from the screen).
pub(crate) fn open(color: Option<Rgba>, cx: &mut App) {
    if let Some(c) = color {
        cx.default_global::<Picker>().recent.push(c);
    }
    let existing = cx.default_global::<Picker>().view.clone().and_then(|v| v.upgrade());
    let handle = cx.default_global::<Picker>().window;
    if let (Some(view), Some(handle)) = (existing, handle) {
        let ok = handle
            .update(cx, |_, window, cx| {
                if let Some(c) = color {
                    view.update(cx, |v, cx| v.set(c, true, window, cx));
                }
                // It was blanked for the screen pick; draw it again even if the compositor sends
                // no activation change.
                gpui_component::set_window_blank(window, false, cx);
                window.activate_window();
            })
            .is_ok();
        if ok {
            return;
        }
    }
    crate::theme::ensure_fonts(cx);
    let options = WindowOptions {
        window_bounds: Some(crate::chrome::centered(460., 640., cx)),
        titlebar: Some(crate::chrome::titlebar("Color picker")),
        window_decorations: crate::chrome::decorations(),
        app_owns_titlebar_drag: true,
        app_id: Some("laqta".into()),
        window_min_size: Some(size(px(400.), px(520.))),
        focus: true,
        ..Default::default()
    };
    let start = color.or_else(|| cx.default_global::<Picker>().recent.iter().next()).unwrap_or(Rgba::rgb(0x3b, 0x82, 0xf6));
    let result = cx.open_window(options, move |window, cx| {
        let view = cx.new(|cx| ColorWindow::new(start, color.is_some(), window, cx));
        cx.default_global::<Picker>().view = Some(view.downgrade());
        cx.new(|cx| Root::new(view, window, cx))
    });
    match result {
        Ok(handle) => cx.default_global::<Picker>().window = Some(handle.into()),
        Err(e) => tracing::warn!("colour picker failed: {e:#}"),
    }
}

struct ColorWindow {
    color: Rgba,
    input: Entity<InputState>,
    /// The field holds text that isn't a colour.
    invalid: bool,
    /// "Copied #336699", shown for a moment.
    notice: Option<SharedString>,
    notice_seq: u64,
}

impl ColorWindow {
    fn new(color: Rgba, picked: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("#336699, rgb(…), hsl(…) or a name").default_value(color.to_hex()));
        cx.subscribe_in(&input, window, |this: &mut Self, input, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                let text = input.read(cx).value().to_string();
                match parse_color(&text) {
                    Some(c) => {
                        this.color = c;
                        this.invalid = false;
                    }
                    None => this.invalid = !text.trim().is_empty(),
                }
                cx.notify();
            }
        })
        .detach();
        crate::vanish::restore_on_activation(window, cx).detach();
        let mut this = ColorWindow { color, input, invalid: false, notice: None, notice_seq: 0 };
        if picked {
            this.flash(format!("Picked {} · copied", color.to_hex()), cx);
        }
        this
    }

    /// Shows `color`; `picked` marks a pick from the screen (its hex is already on the clipboard).
    fn set(&mut self, color: Rgba, picked: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.color = color;
        self.invalid = false;
        self.input.update(cx, |i, cx| i.set_value(color.to_hex(), window, cx));
        if picked {
            self.flash(format!("Picked {} · copied", color.to_hex()), cx);
        }
        cx.notify();
    }

    fn copy(&mut self, value: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
        cx.default_global::<Picker>().recent.push(self.color);
        self.flash(format!("Copied {value}"), cx);
    }

    fn flash(&mut self, text: String, cx: &mut Context<Self>) {
        self.notice = Some(text.into());
        self.notice_seq += 1;
        let seq = self.notice_seq;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(2200)).await;
            let _ = this.update(cx, |this, cx| {
                if this.notice_seq == seq {
                    this.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Hides this window (no trace in the frozen screen) and starts the screen picker.
    fn pick_from_screen(window: &mut Window, cx: &mut App) {
        let dispatch = cx.global::<UiState>().services.dispatch.clone();
        crate::vanish::then_dispatch(Action::ScreenColorPicker, dispatch, window, cx);
    }
}

/// A clickable swatch with a tooltip of its hex.
fn swatch(
    id: SharedString,
    c: Rgba,
    current: bool,
    size_px: f32,
    on_click: impl Fn(&mut ColorWindow, &mut Window, &mut Context<ColorWindow>) + 'static,
    cx: &mut Context<ColorWindow>,
) -> impl IntoElement {
    let ring = cx.theme().foreground;
    let hex = c.to_hex();
    // A zero size fills the parent's width (the tints row); otherwise a fixed square.
    let el = div().id(id);
    let el = if size_px == 0.0 { el.w_full().h(px(34.)) } else { el.size(px(size_px)).flex_none() };
    el.rounded(radius::SM)
        .bg(gpui_color(c))
        .border_1()
        .border_color(ring.opacity(0.12))
        .when(current, |d| d.border_2().border_color(ring))
        .cursor_pointer()
        .hover(|s| s.opacity(0.85))
        .tooltip(move |window, cx| Tooltip::new(hex.clone()).build(window, cx))
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
}

impl Render for ColorWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let x = crate::theme::extra(cx);
        let c = self.color;
        let ink = if dark_ink(c) { gpui::black() } else { gpui::white() };
        let hex = c.to_hex();

        // The swatch: the colour itself, its hex in a readable ink, and the screen picker.
        let hero = div()
            .h(px(128.))
            .flex_none()
            .rounded(radius::CARD)
            .bg(gpui_color(c))
            .border_1()
            .border_color(t.foreground.opacity(0.10))
            .p(space::S4)
            .flex()
            .flex_col()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(text::HEADING).font_weight(FontWeight::SEMIBOLD).text_color(ink).child(hex.clone()))
                    .child(
                        div()
                            .id("copy-hex")
                            .size(px(32.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(radius::PILL)
                            .bg(ink.opacity(0.12))
                            .hover(|s| s.opacity(0.8))
                            .cursor_pointer()
                            .tooltip(|window, cx| Tooltip::new("Copy HEX").build(window, cx))
                            .child(widgets::icon(IconName::Copy, ink).size(px(16.)))
                            .on_click(cx.listener(move |this, _, _, cx| this.copy(this.color.to_hex(), cx))),
                    ),
            )
            .child(
                div()
                    .text_size(text::SMALL)
                    .text_color(ink.opacity(0.8))
                    .child(self.notice.clone().unwrap_or_else(|| "Click a format to copy it".into())),
            );

        let pick =
            widgets::pill("pick-screen", "Pick from screen", Pill::Primary, false, |_, window, cx| Self::pick_from_screen(window, cx), cx)
                .child(widgets::icon(IconName::Pipette, t.primary_foreground).size(px(16.)));

        let field = div().flex().flex_col().gap(space::S1).child(Input::new(&self.input)).when(self.invalid, |d| {
            d.child(widgets::notice("Not a colour. Try #336699, rgb(51, 102, 153) or hsl(210, 50%, 40%).", false, cx))
        });

        let mut formats = div().flex().flex_col().rounded(radius::XL).bg(x.card).border_1().border_color(x.ring).overflow_hidden();
        for (i, row) in rows(c).into_iter().enumerate() {
            let value = row.value.clone();
            formats = formats.child(
                div()
                    .id(("fmt", i))
                    .h(px(40.))
                    .px(space::S4)
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .when(i > 0, |d| d.border_t_1().border_color(x.ring))
                    .cursor_pointer()
                    .hover(|s| s.bg(x.wash))
                    .child(
                        div()
                            .w(px(64.))
                            .flex_none()
                            .text_size(text::SMALL)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.muted_foreground)
                            .child(row.label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(text::LABEL)
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(row.value),
                    )
                    .child(widgets::icon(IconName::Copy, t.muted_foreground).size(px(14.)))
                    .on_click(cx.listener(move |this, _, _, cx| this.copy(value.clone(), cx))),
            );
        }

        let mut tones = div().flex().gap(px(6.));
        for (i, s) in shades(c).into_iter().enumerate() {
            tones = tones.child(div().flex_1().child(swatch(
                SharedString::from(format!("tone-{i}")),
                s,
                s == c,
                0.,
                move |this, window, cx| this.set(s, false, window, cx),
                cx,
            )));
        }

        let recent: Vec<Rgba> = cx.default_global::<Picker>().recent.iter().collect();
        let mut used = div().flex().flex_wrap().gap(px(6.));
        for (i, r) in recent.iter().copied().enumerate() {
            used = used.child(swatch(
                SharedString::from(format!("recent-{i}")),
                r,
                r == c,
                28.,
                move |this, window, cx| this.set(r, false, window, cx),
                cx,
            ));
        }

        let section =
            |title: &'static str| div().text_size(text::SMALL).font_weight(FontWeight::MEDIUM).text_color(t.muted_foreground).child(title);
        let body = div()
            .id("color-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(space::S5)
            .flex()
            .flex_col()
            .gap(space::S4)
            .child(hero)
            .child(div().flex().gap(space::S2).items_start().child(div().flex_1().child(field)).child(pick))
            .child(div().flex().flex_col().gap(space::S2).child(section("Formats")).child(formats))
            .child(div().flex().flex_col().gap(space::S2).child(section("Tints and shades")).child(tones))
            .child(div().flex().flex_col().gap(space::S2).child(section("Used this session")).child(if recent.is_empty() {
                widgets::muted("Colors you pick or copy show up here.", cx).into_any_element()
            } else {
                used.into_any_element()
            }));

        crate::chrome::rounded(widgets::page(cx), window).child(crate::chrome::TitleBar::new().title("Color picker").flat()).child(body)
    }
}
