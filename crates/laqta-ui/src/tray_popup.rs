//! The tray menu as a small popup window (`Action::ToggleTrayMenu`), built from the same tray menu
//! presenter as the native tray. A second toggle closes it.

use gpui::{
    App, AppContext as _, Bounds, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, point, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme as _, Root};
use laqta_ports::TrayMenu;

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
    let (w, h) = (260.0, 36.0 * menu.items.len() as f32 + 16.0);
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
        window_background: WindowBackgroundAppearance::Opaque,
        app_id: Some("laqta-menu".into()),
        ..Default::default()
    };
    match crate::popup::open(options, cx, |window, cx| {
        crate::runtime::dismiss_on_escape(window, cx);
        let view = cx.new(|_| TrayPopup { menu });
        cx.new(|cx| Root::new(view, window, cx))
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
        let mut col =
            div().size_full().flex().flex_col().gap(px(2.)).p(px(8.)).bg(cx.theme().background).border_1().border_color(cx.theme().border);
        for (i, item) in self.menu.items.iter().enumerate() {
            let d = dispatch.clone();
            let action = item.action;
            col = col.child(Button::new(SharedString::from(format!("tray-{i}"))).label(item.label.clone()).ghost().w_full().on_click(
                move |_, window, cx| {
                    let _ = cx.global_mut::<UiState>().tray_popup.take();
                    window.remove_window();
                    d(action);
                },
            ));
        }
        col
    }
}
