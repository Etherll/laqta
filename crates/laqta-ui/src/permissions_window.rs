//! Onboarding for macOS privacy permissions (ADR 030). It opens at start while Screen Recording
//! is missing, lists each permission with what it is for, and asks for one on a click. The state is
//! read again every second, so a permission turned on in System Settings shows here.

use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _, Window,
    WindowOptions, div, px,
};
use gpui_component::{ActiveTheme as _, Root};
use laqta_ports::Permission;

use crate::assets::IconName;
use crate::runtime::{PermissionHooks, UiState};
use crate::theme::{space, text};
use crate::widgets::{self, Pill, Tone};

/// Opens the window when this platform has permissions and Screen Recording is not granted.
pub(crate) fn open_if_needed(cx: &mut App) {
    let Some(hooks) = cx.global::<UiState>().services.permissions.clone() else { return };
    if (hooks.granted)(Permission::ScreenRecording) {
        return;
    }
    crate::theme::ensure_fonts(cx);
    let options = WindowOptions {
        window_bounds: Some(crate::chrome::centered(560., 520., cx)),
        titlebar: Some(crate::chrome::titlebar("Welcome to Laqta")),
        window_decorations: crate::chrome::decorations(),
        app_owns_titlebar_drag: true,
        app_id: Some(crate::APP_ID.into()),
        focus: true,
        ..Default::default()
    };
    let result = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| PermissionsWindow::new(hooks, cx));
        cx.new(|cx| Root::new(view, window, cx))
    });
    if let Err(e) = result {
        tracing::warn!("permissions window failed: {e:#}");
    }
}

struct PermissionsWindow {
    hooks: PermissionHooks,
    granted: Vec<(Permission, bool)>,
    /// Allow was pressed for Screen Recording: macOS only reports the grant to a new process.
    asked_screen: bool,
}

impl PermissionsWindow {
    fn new(hooks: PermissionHooks, cx: &mut Context<Self>) -> Self {
        let granted = read(&hooks);
        // Poll while the window lives; the task ends with it.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = this.update(cx, |this, cx| {
                    let now = read(&this.hooks);
                    if now != this.granted {
                        this.granted = now;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        PermissionsWindow { hooks, granted, asked_screen: false }
    }
}

fn read(hooks: &PermissionHooks) -> Vec<(Permission, bool)> {
    Permission::ALL.iter().map(|&p| (p, (hooks.granted)(p))).collect()
}

/// The id of a permission's Allow button.
fn allow_id(permission: Permission) -> &'static str {
    match permission {
        Permission::ScreenRecording => "allow-screen-recording",
        Permission::Accessibility => "allow-accessibility",
        Permission::InputMonitoring => "allow-input-monitoring",
    }
}

/// Icon, name, what it is for, and whether Laqta can work without it.
fn about(permission: Permission) -> (IconName, &'static str, &'static str, bool) {
    match permission {
        Permission::ScreenRecording => {
            (IconName::Monitor, "Screen Recording", "Take screenshots and record the screen. Laqta can't capture without it.", true)
        }
        Permission::Accessibility => (
            IconName::AppWindow,
            "Accessibility",
            "Pick windows and their parts while selecting a region, and scroll for scrolling capture.",
            false,
        ),
        Permission::InputMonitoring => {
            (IconName::MousePointerClick, "Input Monitoring", "Show your clicks with the mouse highlighter.", false)
        }
    }
}

impl Render for PermissionsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().flex().flex_col().gap(space::S3);
        for &(permission, granted) in &self.granted {
            let (glyph, name, purpose, required) = about(permission);
            let status = if granted {
                widgets::badge("Allowed", Tone::Success, cx).into_any_element()
            } else {
                let request = self.hooks.request.clone();
                widgets::pill(
                    allow_id(permission),
                    "Allow…",
                    if required { Pill::Primary } else { Pill::Default },
                    true,
                    cx.listener(move |this, _, _, cx| {
                        if permission == Permission::ScreenRecording {
                            this.asked_screen = true;
                            cx.notify();
                        }
                        request(permission);
                    }),
                    cx,
                )
                .debug_selector(move || allow_id(permission).into())
                .into_any_element()
            };
            let label = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .child(div().text_size(text::ROW).font_weight(FontWeight::MEDIUM).child(name))
                        .when(!required, |el| el.child(widgets::muted("Optional", cx))),
                )
                .child(div().text_size(text::LABEL).text_color(cx.theme().muted_foreground).child(purpose));
            list = list.child(
                widgets::card(cx)
                    .p(space::S4)
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .child(widgets::icon_tile(glyph, required, cx))
                    .child(label)
                    .child(status),
            );
        }
        let body = div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(space::S5)
            .p(space::S6)
            .child(widgets::page_title("Permissions", "macOS asks you to allow these for each app.", cx))
            .child(list)
            .child(widgets::muted(
                "After you allow Screen Recording in System Settings, macOS asks to quit and reopen Laqta. The other permissions work at once.",
                cx,
            ));
        let screen_missing = self.granted.iter().any(|&(p, ok)| p == Permission::ScreenRecording && !ok);
        let reopen = (self.asked_screen && screen_missing).then(|| {
            let relaunch = self.hooks.relaunch.clone();
            widgets::pill(
                "reopen",
                "Quit & Reopen",
                Pill::Primary,
                false,
                move |_, _, cx| {
                    relaunch();
                    cx.quit();
                },
                cx,
            )
        });
        let body = body.when(self.asked_screen && screen_missing, |b| {
            b.child(widgets::notice(
                "Already allowed in System Settings but still missing here? Quit & Reopen. If that doesn't help, remove Laqta from the Screen Recording list with −, add it again, then reopen: an update of a build that isn't notarized needs this each time.",
                true,
                cx,
            ))
        });
        let footer = div()
            .flex()
            .justify_end()
            .gap(space::S2)
            .p(space::S3)
            .border_t_1()
            .border_color(cx.theme().border)
            .when_some(reopen, |f, b| f.child(b))
            .child(widgets::pill("done", "Done", Pill::Default, false, |_, window, _| window.remove_window(), cx));
        crate::chrome::rounded(widgets::page(cx), window)
            .child(crate::chrome::TitleBar::new().title("Welcome to Laqta").flat())
            .child(body)
            .child(footer)
    }
}
