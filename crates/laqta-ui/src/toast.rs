//! Toasts (`Notifier` port): a small window at the configured screen corner that closes itself.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, App, AppContext as _, Bounds, Context, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Render, RenderImage, StatefulInteractiveElement as _, Styled as _, Window, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions, div, img, point, px, size,
};
use gpui_component::ActiveTheme as _;
use laqta_domain::workflow::{ToastClickAction, ToastPlacement};
use laqta_ports::Toast;

use crate::runtime::UiState;

const MARGIN: f32 = 16.0;

pub(crate) use crate::gpu_image::render_image;

pub(crate) fn show(toast: Toast, cx: &mut App) {
    // Wayland names no primary display: take the first then, or nothing would ever show.
    let Some(display) = crate::placement::main_display(cx) else { return };
    // The work area keeps the toast clear of the Windows taskbar. Where the platform reports no
    // work area (it equals the whole display), a panel's height is left free at the bottom right.
    let area = display.visible_bounds();
    let panel = if area == display.bounds() { 40.0 } else { 0.0 };
    let has_image = toast.image.is_some();
    let w = (toast.options.width as f32).clamp(240.0, 600.0);
    let h = if has_image { (toast.options.height as f32).clamp(120.0, 500.0) } else { 72.0 };
    let (ax, ay) = (f32::from(area.origin.x), f32::from(area.origin.y));
    let (aw, ah) = (f32::from(area.size.width), f32::from(area.size.height));
    let (x, y) = match toast.options.placement {
        ToastPlacement::TopLeft => (ax + MARGIN, ay + MARGIN),
        ToastPlacement::TopRight => (ax + aw - w - MARGIN, ay + MARGIN),
        ToastPlacement::BottomLeft => (ax + MARGIN, ay + ah - h - MARGIN),
        ToastPlacement::BottomRight => (ax + aw - w - MARGIN, ay + ah - h - MARGIN - panel),
    };
    let bounds = Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) };
    let options = WindowOptions {
        titlebar: None,
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        focus: false,
        show: true,
        kind: toast_kind(&toast),
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: Some(display.id()),
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("laqta-toast".into()),
        ..Default::default()
    };
    let duration = Duration::from_millis(toast.options.duration_ms.max(500) as u64);
    // Reduced motion: the toast closes without fading.
    let fade = if cx.reduce_motion() { Duration::ZERO } else { Duration::from_millis(toast.options.fade_ms.min(5000) as u64) };
    let result = crate::popup::open(options, cx, move |window, cx| {
        let view = cx.new(|_| ToastView { image: toast.image.as_ref().and_then(render_image), toast, fading: false });
        // Stay for `duration`, fade out over `fade`, then close.
        let v = view.clone();
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(duration).await;
                if !fade.is_zero() {
                    let _ = cx.update(|_, cx| {
                        v.update(cx, |t, cx| {
                            t.fading = true;
                            cx.notify();
                        })
                    });
                    cx.background_executor().timer(fade).await;
                }
                let _ = cx.update(|window, _| window.remove_window());
            })
            .detach();
        view
    });
    if let Err(e) = result {
        tracing::warn!("toast window failed: {e:#}");
    }
}

/// How long the offer to restart as administrator stays when nobody answers it.
const OFFER_SECONDS: u64 = 20;

/// A toast-sized window at the bottom right with two buttons: restart Laqta as administrator, or
/// not now. `app` is the program running as administrator that has the focus.
pub(crate) fn offer_admin_restart(app: String, restart: Arc<dyn Fn() + Send + Sync>, cx: &mut App) {
    let Some(display) = crate::placement::main_display(cx) else { return };
    let area = display.visible_bounds();
    let (w, h) = (380.0, 128.0);
    let x = f32::from(area.origin.x) + f32::from(area.size.width) - w - MARGIN;
    let y = f32::from(area.origin.y) + f32::from(area.size.height) - h - MARGIN;
    let options = WindowOptions {
        titlebar: None,
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) })),
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: Some(display.id()),
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("laqta-toast".into()),
        ..Default::default()
    };
    let result = crate::popup::open(options, cx, move |window, cx| {
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(Duration::from_secs(OFFER_SECONDS)).await;
                let _ = cx.update(|window, _| window.remove_window());
            })
            .detach();
        cx.new(|_| AdminOffer { app, restart })
    });
    if let Err(e) = result {
        tracing::warn!("offer window failed: {e:#}");
    }
}

struct AdminOffer {
    app: String,
    restart: Arc<dyn Fn() + Send + Sync>,
}

impl Render for AdminOffer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::widgets::{Pill, pill};
        let x = crate::theme::extra(cx);
        let t = cx.theme();
        let restart = self.restart.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_between()
            .gap(px(10.))
            .p(px(14.))
            .rounded(crate::theme::radius::TOAST)
            .bg(x.card)
            .text_color(t.foreground)
            .font_family(crate::theme::text::UI_FONT)
            .border_1()
            .border_color(x.ring)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(crate::theme::text::ROW)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .truncate()
                            .child(format!("{} runs as administrator", self.app)),
                    )
                    .child(
                        div()
                            .text_size(crate::theme::text::CAPTION)
                            .text_color(t.muted_foreground)
                            .child("Laqta's shortcuts can't reach it. Restart Laqta as administrator to use them there."),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(pill("not-now", "Not now", Pill::Ghost, true, |_, window, _| window.remove_window(), cx))
                    .child(pill(
                        "restart-admin",
                        "Restart as administrator",
                        Pill::Primary,
                        true,
                        move |_, window, _| {
                            window.remove_window();
                            restart();
                        },
                        cx,
                    )),
            )
    }
}

#[cfg(target_os = "linux")]
fn toast_kind(toast: &Toast) -> WindowKind {
    use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()) {
        let anchor = match toast.options.placement {
            ToastPlacement::TopLeft => Anchor::TOP | Anchor::LEFT,
            ToastPlacement::TopRight => Anchor::TOP | Anchor::RIGHT,
            ToastPlacement::BottomLeft => Anchor::BOTTOM | Anchor::LEFT,
            ToastPlacement::BottomRight => Anchor::BOTTOM | Anchor::RIGHT,
        };
        WindowKind::LayerShell(LayerShellOptions {
            // The surface's purpose: KWin would otherwise zoom it in like a normal window, on top of
            // the toast's own fade.
            namespace: "notification".into(),
            layer: Layer::Overlay,
            anchor,
            margin: Some((px(MARGIN), px(MARGIN), px(MARGIN), px(MARGIN))),
            keyboard_interactivity: KeyboardInteractivity::None,
            ..Default::default()
        })
    } else {
        WindowKind::PopUp
    }
}

#[cfg(not(target_os = "linux"))]
fn toast_kind(_toast: &Toast) -> WindowKind {
    WindowKind::PopUp
}

struct ToastView {
    toast: Toast,
    image: Option<Arc<RenderImage>>,
    fading: bool,
}

impl ToastView {
    fn click(&self, action: ToastClickAction, window: &mut Window, cx: &mut App) {
        let shell = cx.global::<UiState>().services.shell.clone();
        let path = self.toast.path.clone();
        let result = match (action, path) {
            (ToastClickAction::OpenFile, Some(p)) => shell.open_path(&p),
            (ToastClickAction::OpenFolder, Some(p)) => shell.reveal(&p),
            (ToastClickAction::OpenUrl, _) | (ToastClickAction::CopyUrl, _) => match &self.toast.url {
                Some(u) if action == ToastClickAction::OpenUrl => shell.open_url(u),
                Some(u) => {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(u.clone()));
                    Ok(())
                }
                None => Ok(()),
            },
            _ => Ok(()),
        };
        if let Err(e) = result {
            tracing::warn!("toast click: {e}");
        }
        if action != ToastClickAction::None {
            window.remove_window();
        }
    }
}

impl Render for ToastView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let o = self.toast.options.clone();
        let x = crate::theme::extra(cx);
        let t = cx.theme();
        let mut body = div()
            .id("laqta-toast")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(12.))
            .pb(px(14.))
            .rounded(crate::theme::radius::TOAST)
            .bg(x.card)
            .text_color(t.foreground)
            .font_family(crate::theme::text::UI_FONT)
            .border_1()
            .border_color(x.ring)
            .cursor_pointer();
        if let Some(image) = &self.image {
            body = body.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .rounded(crate::theme::radius::MD)
                    .bg(t.muted)
                    .overflow_hidden()
                    // Definite bounds, so the preview scales to fill the box instead of keeping its
                    // natural size; the default fit (contain) keeps the whole capture visible.
                    .child(img(image.clone()).size_full()),
            );
        }
        body = body.child(
            div()
                .px(px(4.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(crate::theme::text::ROW).font_weight(gpui::FontWeight::MEDIUM).child(self.toast.title.clone()))
                .child(
                    div().text_size(crate::theme::text::CAPTION).text_color(t.muted_foreground).truncate().child(self.toast.body.clone()),
                ),
        );
        let (l, r, m) = (o.left_click, o.right_click, o.middle_click);
        let body = body
            .on_click(cx.listener(move |this, _, window, cx| this.click(l, window, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, window, cx| this.click(r, window, cx)))
            .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, window, cx| this.click(m, window, cx)));
        if self.fading {
            body.with_animation("fade", Animation::new(Duration::from_millis(o.fade_ms.max(1) as u64)), |el, delta| el.opacity(1.0 - delta))
                .into_any_element()
        } else {
            body.into_any_element()
        }
    }
}
