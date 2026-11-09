//! The actions toolbar: a small always-on-top bar of action buttons (ShareX's actions toolbar).
//! The second toggle closes it.

use gpui::{
    App, AppContext as _, Bounds, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, WindowBounds,
    WindowKind, WindowOptions, div, point, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme as _, Icon, Root, Sizable as _};

use crate::assets::IconName;
use laqta_domain::Action;

use crate::runtime::UiState;

/// The default toolbar actions (ShareX's defaults: region, screen, window, record, colour picker…).
pub(crate) const DEFAULT_ACTIONS: [Action; 8] = [
    Action::RectangleRegion,
    Action::PrintScreen,
    Action::ActiveWindow,
    Action::ScreenRecorder,
    Action::ScreenColorPicker,
    Action::ClipboardUpload,
    Action::OpenHistory,
    Action::OpenMainWindow,
];

pub(crate) fn toggle(cx: &mut App) {
    if let Some(h) = cx.global_mut::<UiState>().actions_toolbar.take()
        && h.update(cx, |_, window, _| window.remove_window()).is_ok()
    {
        return;
    }
    let Some(display) = cx.primary_display() else { return };
    let area = display.bounds();
    let (w, h) = (DEFAULT_ACTIONS.len() as f32 * 118.0 + 16.0, 48.0);
    let x = f32::from(area.origin.x) + (f32::from(area.size.width) - w) / 2.0;
    let y = f32::from(area.origin.y) + 24.0;
    let options = WindowOptions {
        titlebar: None,
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) })),
        focus: false,
        kind: WindowKind::PopUp,
        is_resizable: false,
        is_minimizable: false,
        is_movable: true,
        display_id: Some(display.id()),
        app_id: Some("laqta-toolbar".into()),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        let view = cx.new(|_| ActionsToolbar);
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        Ok(handle) => cx.global_mut::<UiState>().actions_toolbar = Some(handle.into()),
        Err(e) => tracing::warn!("actions toolbar failed: {e:#}"),
    }
}

struct ActionsToolbar;

impl Render for ActionsToolbar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dispatch = cx.global::<UiState>().services.dispatch.clone();
        let mut bar = div()
            .size_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .p(px(6.))
            .bg(crate::theme::extra(cx).card)
            .font_family(crate::theme::text::UI_FONT)
            .border_1()
            .border_color(cx.theme().border);
        for (i, action) in DEFAULT_ACTIONS.into_iter().enumerate() {
            let d = dispatch.clone();
            bar = bar.child(
                Button::new(SharedString::from(format!("act-{i}")))
                    .label(short_label(action))
                    .icon(Icon::new(action_icon(action)))
                    .small()
                    .ghost()
                    .tooltip(crate::i18n::action_label(action))
                    .on_click(move |_, _, _| d(action)),
            );
        }
        bar
    }
}

fn action_icon(a: Action) -> IconName {
    match a {
        Action::RectangleRegion => IconName::Crop,
        Action::PrintScreen => IconName::Monitor,
        Action::ActiveWindow => IconName::AppWindow,
        Action::ScreenRecorder => IconName::Video,
        Action::ScreenColorPicker => IconName::Pipette,
        Action::ClipboardUpload => IconName::Clipboard,
        Action::OpenHistory => IconName::LayoutGrid,
        Action::OpenMainWindow => IconName::House,
        _ => IconName::Sparkles,
    }
}

fn short_label(a: Action) -> &'static str {
    match a {
        Action::RectangleRegion => "Region",
        Action::PrintScreen => "Screen",
        Action::ActiveWindow => "Window",
        Action::ScreenRecorder => "Record",
        Action::ScreenColorPicker => "Colour",
        Action::ClipboardUpload => "Upload clip",
        Action::OpenHistory => "History",
        Action::OpenMainWindow => "Laqta",
        _ => "Action",
    }
}
