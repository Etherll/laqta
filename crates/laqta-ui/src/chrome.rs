//! The window chrome: Laqta draws its own titlebar instead of the system one.
//!
//! The bar shares the sidebar colour, holds the window title and the window controls, moves the
//! window when dragged, maximizes on double click and opens the window menu on right click.
//! Where the system draws the frame anyway (X11 without a compositor, or a Wayland compositor that
//! insists on server-side decorations), the bar draws nothing, so there are never two.

use gpui::{
    AnyElement, App, Decorations, Hsla, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Pixels,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, TitlebarOptions, Window, WindowControlArea, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{ActiveTheme as _, Icon};

use crate::assets::IconName;
use crate::theme::{radius, text};

pub(crate) const TITLE_BAR_H: Pixels = px(36.);
/// Width of each window control: wide targets, as on Windows.
const CONTROL_W: Pixels = px(46.);
/// Room for the macOS traffic lights, which the system draws over the bar.
const MAC_LEFT: Pixels = px(78.);

/// Window options for a window that draws its own titlebar. `title` is what the taskbar and
/// window switcher show.
pub(crate) fn titlebar(title: impl Into<SharedString>) -> TitlebarOptions {
    TitlebarOptions { title: Some(title.into()), appears_transparent: true, traffic_light_position: Some(gpui::point(px(14.), px(11.))) }
}

/// The fields every custom-chrome window sets, spread into its `WindowOptions`.
pub(crate) fn decorations() -> Option<gpui::WindowDecorations> {
    Some(gpui::WindowDecorations::Client)
}

/// The window's corner radii: rounded while it floats, square on edges it is tiled against.
pub(crate) fn corners(window: &Window) -> gpui::Corners<Pixels> {
    gpui_component::window_corner_radii(window)
}

/// Rounds a window's root element to the frame, so its background stays inside the corners.
pub(crate) fn rounded(el: gpui::Div, window: &Window) -> gpui::Div {
    let c = corners(window);
    el.rounded_tl(c.top_left).rounded_tr(c.top_right).rounded_bl(c.bottom_left).rounded_br(c.bottom_right)
}

/// How tall `TitleBar` is in this window: nothing when the compositor draws the frame.
pub(crate) fn title_bar_height(window: &Window) -> f32 {
    if draws_own_frame(window) { f32::from(TITLE_BAR_H) } else { 0.0 }
}

/// True when this window draws its own frame (client-side decorations).
fn draws_own_frame(window: &Window) -> bool {
    if cfg!(target_os = "linux") { matches!(window.window_decorations(), Decorations::Client { .. }) } else { true }
}

/// The titlebar. `leading` sits over the sidebar (width `leading_w`); `title` is centred.
#[derive(IntoElement, Default)]
pub(crate) struct TitleBar {
    title: Option<SharedString>,
    leading: Option<AnyElement>,
    leading_w: Option<Pixels>,
    flat: bool,
}

impl TitleBar {
    /// A bar in the page colour, for windows without a sidebar.
    pub(crate) fn flat(mut self) -> Self {
        self.flat = true;
        self
    }

    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Content over the sidebar column, such as the app name, `width` wide.
    pub(crate) fn leading(mut self, el: impl IntoElement, width: Pixels) -> Self {
        self.leading = Some(el.into_any_element());
        self.leading_w = Some(width);
        self
    }
}

struct DragState {
    pressed: bool,
}

impl RenderOnce for TitleBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !draws_own_frame(window) {
            return div().into_any_element();
        }
        let state = window.use_state(cx, |_, _| DragState { pressed: false });
        let t = cx.theme().clone();
        let mac = cfg!(target_os = "macos");

        let mut lead = div().h_full().flex_none().flex().items_center().pl(if mac { MAC_LEFT } else { px(16.) });
        if let Some(w) = self.leading_w {
            lead = lead.w(w);
        }
        if let Some(el) = self.leading {
            lead = lead.child(el);
        }
        let title = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_center()
            .text_size(text::SMALL)
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(t.muted_foreground)
            .children(self.title);
        // Balance the leading column so the title stays centred on the window.
        let trail_w = if mac { px(16.) } else { CONTROL_W * 3. };

        let c = corners(window);
        // The drag area and the window controls are siblings. On Windows the system asks which
        // area is under the pointer and takes the first match in paint order, so controls nested
        // inside the drag area would answer "drag" and their clicks would never arrive.
        let drag = div()
            .id("title-bar-drag")
            .h_full()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(
                MouseButton::Left,
                window.listener_for(&state, |state, ev: &MouseDownEvent, window, _| {
                    if ev.click_count == 2 {
                        state.pressed = false;
                        if cfg!(target_os = "macos") { window.titlebar_double_click() } else { window.zoom_window() }
                    } else {
                        state.pressed = true;
                    }
                }),
            )
            .on_mouse_up(MouseButton::Left, window.listener_for(&state, |state, _, _, _| state.pressed = false))
            .on_mouse_down_out(window.listener_for(&state, |state, _, _, _| state.pressed = false))
            .on_mouse_move(window.listener_for(&state, |state, _, window, _| {
                if state.pressed {
                    state.pressed = false;
                    window.start_window_move();
                }
            }))
            .when(cfg!(target_os = "linux"), |bar| {
                bar.on_mouse_down(MouseButton::Right, |ev: &MouseDownEvent, window, _| window.show_window_menu(ev.position))
            })
            .child(lead)
            .child(title);
        div()
            .id("title-bar")
            .rounded_tl(c.top_left)
            .rounded_tr(c.top_right)
            .h(TITLE_BAR_H)
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(if self.flat { t.background } else { t.sidebar })
            .font_family(text::UI_FONT)
            .child(drag)
            .child(div().h_full().flex_none().min_w(trail_w).flex().justify_end().children((!mac).then(|| controls(window, cx))))
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum Control {
    Minimize,
    Maximize,
    Restore,
    Close,
}

fn controls(window: &Window, cx: &App) -> impl IntoElement {
    let supported = window.window_controls();
    div()
        .h_full()
        .flex()
        .when(supported.minimize, |d| d.child(control(Control::Minimize, cx)))
        .when(supported.maximize, |d| d.child(control(if window.is_maximized() { Control::Restore } else { Control::Maximize }, cx)))
        .child(control(Control::Close, cx))
}

fn control(kind: Control, cx: &App) -> impl IntoElement {
    let t = cx.theme();
    let (id, glyph, label, area) = match kind {
        Control::Minimize => ("win-min", IconName::WindowMinimize, "Minimize", WindowControlArea::Min),
        Control::Maximize => ("win-max", IconName::WindowMaximize, "Maximize", WindowControlArea::Max),
        Control::Restore => ("win-restore", IconName::WindowRestore, "Restore", WindowControlArea::Max),
        Control::Close => ("win-close", IconName::WindowClose, "Close", WindowControlArea::Close),
    };
    let close = matches!(kind, Control::Close);
    // Close turns red on hover; the others take the neutral wash.
    let (hover_bg, hover_fg): (Hsla, Hsla) =
        if close { (crate::theme::extra(cx).close_hover, gpui::white()) } else { (t.foreground.opacity(0.07), t.foreground) };
    let icon_color = t.foreground.opacity(0.75);
    div()
        .id(id)
        .w(CONTROL_W)
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(icon_color)
        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        .when(cfg!(target_os = "windows"), |d| d.window_control_area(area))
        .when(cfg!(target_os = "linux"), |d| {
            d.on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                match kind {
                    Control::Minimize => window.minimize_window(),
                    Control::Maximize | Control::Restore => window.zoom_window(),
                    Control::Close => window.remove_window(),
                }
            })
        })
        .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(label).build(window, cx))
        .child(Icon::new(glyph).size(px(14.)))
}

/// The Laqta wordmark for the titlebar's leading slot.
pub(crate) fn wordmark() -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(gpui::img(crate::assets::BRAND_MARK).size(px(18.)).flex_none())
        .child(div().font_family(text::HEADING_FONT).font_weight(gpui::FontWeight::SEMIBOLD).text_size(text::SECTION).child("Laqta"))
}

/// The content sheet under the titlebar: page colour with a rounded corner where it meets the
/// sidebar chrome.
pub(crate) fn sheet(window: &Window, cx: &App) -> gpui::Div {
    let t = cx.theme();
    let sheet = div().bg(t.background).rounded_tl(radius::MD).rounded_br(corners(window).bottom_right);
    // In dark the tone step separates sheet and chrome; in light it is too faint without a line.
    if t.is_dark() { sheet } else { sheet.border_t_1().border_l_1().border_color(t.sidebar_border) }
}
