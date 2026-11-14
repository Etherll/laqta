//! The actions toolbar: a small always-on-top bar of action buttons (ShareX's actions toolbar).
//! The second toggle closes it.

use gpui::{
    App, AppContext as _, Bounds, Context, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowKind,
    WindowOptions, div, point, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme as _, Icon, Sizable as _};

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
    // Wayland names no primary display: take the first then, or nothing would ever show.
    let Some(display) = crate::placement::main_display(cx) else { return };
    let area = display.bounds();
    // The capsule plus room for its shadow on every side.
    let (w, h) = (DEFAULT_ACTIONS.len() as f32 * 86.0 + 2.0 * SHADOW_PAD + 76.0, BAR_H + 2.0 * SHADOW_PAD);
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
        // Drawn by Laqta: no system title bar over the capsule, and the corners stay clear.
        window_decorations: Some(WindowDecorations::Client),
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    match crate::popup::open(options, cx, |window, cx| {
        // No Root: it paints the page colour, and the window around the capsule stays clear.
        let _ = window;
        cx.new(|_| ActionsToolbar)
    }) {
        Ok(handle) => cx.global_mut::<UiState>().actions_toolbar = Some(handle.into()),
        Err(e) => tracing::warn!("actions toolbar failed: {e:#}"),
    }
}

/// The capsule's height.
const BAR_H: f32 = 48.0;
/// Clear space around the capsule for its shadow.
const SHADOW_PAD: f32 = 12.0;

struct ActionsToolbar;

impl Render for ActionsToolbar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dispatch = cx.global::<UiState>().services.dispatch.clone();
        let t = cx.theme();
        // A grip to move the bar: the compositor moves it, since Wayland lets no app place itself.
        let grip = div()
            .id("grip")
            .h_full()
            .w(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_grab()
            .text_color(t.muted_foreground)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("Drag to move").build(window, cx))
            .child(Icon::new(IconName::GripVertical).size(crate::theme::icon::SM));
        let mut bar = crate::widgets::floating(cx).h(px(BAR_H)).px(px(6.)).gap(px(2.)).font_family(crate::theme::text::UI_FONT).child(grip);
        for (i, action) in DEFAULT_ACTIONS.into_iter().enumerate() {
            let d = dispatch.clone();
            bar = bar.child(
                Button::new(SharedString::from(format!("act-{i}")))
                    .label(short_label(action))
                    .icon(Icon::new(action_icon(action)))
                    .small()
                    .ghost()
                    .rounded(crate::theme::radius::PILL)
                    .tooltip(crate::i18n::action_label(action))
                    .on_click(move |_, _, _| d(action)),
            );
        }
        bar = bar.child(crate::widgets::divider_v(cx)).child(
            Button::new("close")
                .icon(Icon::new(IconName::X))
                .small()
                .ghost()
                .rounded(crate::theme::radius::PILL)
                .tooltip("Close the toolbar")
                .on_click(|_, _, cx| toggle(cx)),
        );
        div().size_full().flex().items_center().justify_center().p(px(SHADOW_PAD)).child(bar)
    }
}

pub(crate) fn action_icon(a: Action) -> IconName {
    match a {
        Action::RectangleRegion | Action::CustomRegion | Action::LastRegion => IconName::Crop,
        Action::PrintScreen | Action::ActiveMonitor => IconName::Monitor,
        Action::ScreenRecorderGif => IconName::Film,
        Action::ScrollingCapture => IconName::ScanLine,
        Action::Ocr => IconName::ScanText,
        Action::ImageEditor => IconName::SquarePen,
        Action::ActiveWindow => IconName::AppWindow,
        Action::ScreenRecorder => IconName::Video,
        Action::ScreenColorPicker => IconName::Pipette,
        Action::ClipboardUpload => IconName::Clipboard,
        Action::OpenHistory => IconName::LayoutGrid,
        Action::OpenMainWindow => IconName::House,
        _ => IconName::Sparkles,
    }
}

/// The button text: a short name looked up as `Toolbar_<Action>`. No bundle carries these keys
/// yet (the bundles come from ShareX, which has no short names), so every language shows the
/// English text until a translation adds them.
fn short_label(a: Action) -> String {
    let english = match a {
        Action::RectangleRegion => "Region",
        Action::PrintScreen => "Screen",
        Action::ActiveWindow => "Window",
        Action::ScreenRecorder => "Record",
        Action::ScreenColorPicker => "Color",
        Action::ClipboardUpload => "Upload clip",
        Action::OpenHistory => "History",
        Action::OpenMainWindow => "Laqta",
        _ => "Action",
    };
    crate::i18n::t(&format!("Toolbar_{a:?}"), english)
}
