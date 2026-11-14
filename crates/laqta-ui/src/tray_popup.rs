//! The tray menu as a small popup window (`Action::ToggleTrayMenu`), built from the same tray menu
//! presenter as the native tray. A second toggle closes it.

use gpui::{
    App, AppContext as _, Bounds, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowKind, WindowOptions, div, point, px, size,
};
use gpui_component::{ActiveTheme as _, Icon};
use laqta_domain::Action;
use laqta_ports::TrayMenu;

use crate::assets::IconName;
use crate::theme::{radius, space, text};

/// One menu row's height.
const ROW_H: f32 = 34.0;
/// A separator between groups, with its margins.
const SEP_H: f32 = 9.0;
/// Clear space around the menu for its shadow.
const SHADOW_PAD: f32 = 12.0;

use crate::runtime::UiState;

pub(crate) fn toggle(cx: &mut App) {
    if let Some(h) = cx.global_mut::<UiState>().tray_popup.take()
        && h.update(cx, |_, window, _| window.remove_window()).is_ok()
    {
        return;
    }
    let menu = crate::presenters::tray::tray_menu(&cx.global::<UiState>().services.app.settings());
    // Wayland names no primary display: take the first then, or nothing would ever show.
    let Some(display) = crate::placement::main_display(cx) else { return };
    let area = display.bounds();
    let seps = separators(&menu).iter().filter(|s| **s).count() as f32;
    let (w, h) = (264.0 + 2.0 * SHADOW_PAD, ROW_H * menu.items.len() as f32 + SEP_H * seps + 12.0 + 2.0 * SHADOW_PAD);
    let x = f32::from(area.origin.x) + f32::from(area.size.width) - w - 16.0;
    let y = f32::from(area.origin.y) + f32::from(area.size.height) - h - 56.0;
    let options = WindowOptions {
        titlebar: None,
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) })),
        focus: true,
        kind: WindowKind::PopUp,
        is_resizable: false,
        is_minimizable: false,
        display_id: Some(display.id()),
        window_background: WindowBackgroundAppearance::Transparent,
        // Drawn by Laqta: no system title bar over the menu.
        window_decorations: Some(WindowDecorations::Client),
        app_id: Some("laqta-menu".into()),
        ..Default::default()
    };
    match crate::popup::open(options, cx, |window, cx| {
        crate::runtime::dismiss_on_escape(window, cx);
        // No Root: it paints the page colour, and the shadow area around the menu stays clear.
        cx.new(|_| TrayPopup { menu })
    }) {
        Ok(handle) => cx.global_mut::<UiState>().tray_popup = Some(handle.into()),
        Err(e) => tracing::warn!("tray menu popup failed: {e:#}"),
    }
}

struct TrayPopup {
    menu: TrayMenu,
}

impl Render for TrayPopup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Close when the popup loses focus, like a menu.
        if !window.is_window_active() {
            let _ = cx.global_mut::<UiState>().tray_popup.take();
        }
        let dispatch = cx.global::<UiState>().services.dispatch.clone();
        let t = cx.theme();
        let x = crate::theme::extra(cx);
        let wash = x.wash;
        let seps = separators(&self.menu);
        let mut col = div()
            .flex()
            .flex_col()
            .p(px(6.))
            .rounded(radius::XL)
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
            .font_family(text::UI_FONT)
            .text_color(t.foreground);
        for (i, item) in self.menu.items.iter().enumerate() {
            if seps[i] {
                col = col.child(div().h(px(1.)).mx(space::S2).my(px(4.)).bg(t.border));
            }
            let d = dispatch.clone();
            let action = item.action;
            let quit = matches!(action, Action::ExitLaqta);
            let row = div()
                .id(SharedString::from(format!("tray-{i}")))
                .h(px(ROW_H))
                .flex()
                .items_center()
                .gap(space::S3)
                .px(space::S3)
                .rounded(radius::MD)
                .cursor_pointer()
                .hover(move |s| s.bg(wash))
                .child(Icon::new(icon(action)).size(crate::theme::icon::MD).text_color(if quit { t.danger } else { t.muted_foreground }))
                .child(div().flex_1().min_w_0().truncate().text_size(text::ROW).font_weight(FontWeight::MEDIUM).child(item.label.clone()));
            col = col.child(crate::widgets::clickable(
                row,
                move |_, window, cx| {
                    let _ = cx.global_mut::<UiState>().tray_popup.take();
                    window.remove_window();
                    d(action);
                },
                cx,
            ));
        }
        div().size_full().p(px(SHADOW_PAD)).child(col)
    }
}

/// Which actions belong together in the menu: captures, then windows and folders, then quitting.
fn group(a: Action) -> u8 {
    match a {
        Action::ExitLaqta => 2,
        Action::OpenMainWindow | Action::OpenHistory | Action::OpenSettings | Action::OpenScreenshotsFolder | Action::OpenImageHistory => 1,
        _ => 0,
    }
}

/// For each item, whether a separator goes above it: where the group changes.
fn separators(menu: &TrayMenu) -> Vec<bool> {
    menu.items.iter().enumerate().map(|(i, it)| i > 0 && group(menu.items[i - 1].action) != group(it.action)).collect()
}

fn icon(a: Action) -> IconName {
    match a {
        Action::LastRegion => IconName::RotateCcw,
        Action::OpenSettings => IconName::Settings,
        Action::OpenScreenshotsFolder => IconName::FolderOpen,
        Action::ExitLaqta => IconName::CircleX,
        other => crate::actions_toolbar::action_icon(other),
    }
}
