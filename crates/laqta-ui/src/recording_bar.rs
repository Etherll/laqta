//! The recording bar (M4): elapsed time, pause/resume, stop, abort, and a border around the
//! recorded area. Shown on `RecordingStarted`, closed on `RecordingStopped`/`RecordingFailed`.

use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, IntoElement, ParentElement as _, Render, Styled as _, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, point, px, size,
};
use gpui_component::{ActiveTheme as _, Root};

use crate::theme::{radius, text};
use crate::widgets::{self, Pill};
use laqta_domain::geometry::PhysRect;
use laqta_ports::TaskEvent;

use crate::runtime::UiState;

#[derive(Default)]
pub(crate) struct RecordingWindows {
    bar: Option<AnyWindowHandle>,
    border: Vec<AnyWindowHandle>,
    countdown: Option<AnyWindowHandle>,
}

struct Bar {
    elapsed: u32,
    paused: bool,
}

struct Countdown {
    seconds: u32,
}

/// Physical desktop pixels → this display's logical pixels (scale from the primary display).
fn logical(r: PhysRect, scale: f32) -> Bounds<gpui::Pixels> {
    Bounds {
        origin: point(px(r.x as f32 / scale), px(r.y as f32 / scale)),
        size: size(px(r.width as f32 / scale), px(r.height as f32 / scale)),
    }
}

fn popup(bounds: Bounds<gpui::Pixels>, focus: bool) -> WindowOptions {
    WindowOptions {
        titlebar: None,
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        focus,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("laqta-recording".into()),
        ..Default::default()
    }
}

fn close_all(cx: &mut App) {
    let w = std::mem::take(&mut cx.global_mut::<UiState>().recording);
    for h in w.bar.into_iter().chain(w.border).chain(w.countdown) {
        let _ = h.update(cx, |_, window, _| window.remove_window());
    }
}

pub(crate) fn on_event(event: &TaskEvent, cx: &mut App) {
    match event {
        TaskEvent::RecordingCountdown { seconds_left, .. } => countdown(*seconds_left, cx),
        TaskEvent::RecordingStarted { area, .. } => start(*area, cx),
        TaskEvent::RecordingTick { elapsed_secs, .. } => update(cx, |b| b.elapsed = *elapsed_secs),
        TaskEvent::RecordingPaused { .. } => update(cx, |b| b.paused = true),
        TaskEvent::RecordingResumed { .. } => update(cx, |b| b.paused = false),
        TaskEvent::RecordingStopped { .. } | TaskEvent::RecordingFailed { .. } => close_all(cx),
        _ => {}
    }
}

fn countdown(seconds: u32, cx: &mut App) {
    if let Some(h) = cx.global::<UiState>().recording.countdown {
        let ok = h
            .update(cx, |root, _, cx| {
                if let Ok(root) = root.downcast::<Root>()
                    && let Ok(view) = root.read(cx).view().clone().downcast::<Countdown>()
                {
                    view.update(cx, |c, cx| {
                        c.seconds = seconds;
                        cx.notify();
                    });
                }
            })
            .is_ok();
        if ok {
            return;
        }
    }
    let Some(display) = cx.primary_display() else { return };
    let b = display.bounds();
    let (w, h) = (160.0, 160.0);
    let bounds = Bounds {
        origin: point(b.origin.x + (b.size.width - px(w)) / 2.0, b.origin.y + (b.size.height - px(h)) / 2.0),
        size: size(px(w), px(h)),
    };
    if let Ok(handle) = cx.open_window(popup(bounds, false), |window, cx| {
        let view = cx.new(|_| Countdown { seconds });
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        cx.global_mut::<UiState>().recording.countdown = Some(handle.into());
    }
}

fn update(cx: &mut App, f: impl FnOnce(&mut Bar)) {
    let Some(h) = cx.global::<UiState>().recording.bar else { return };
    let _ = h.update(cx, |root, _, cx| {
        if let Ok(root) = root.downcast::<Root>()
            && let Ok(view) = root.read(cx).view().clone().downcast::<Bar>()
        {
            view.update(cx, |b, cx| {
                f(b);
                cx.notify();
            });
        }
    });
}

fn start(area: PhysRect, cx: &mut App) {
    if let Some(c) = cx.global_mut::<UiState>().recording.countdown.take() {
        let _ = c.update(cx, |_, window, _| window.remove_window());
    }
    let scale = 1.0;
    let show_border = cx.global::<UiState>().services.app.settings().defaults.recording.show_border;
    let a = logical(area, scale);
    // The bar sits under the area (or above it at the screen bottom).
    let bar_w = 360.0;
    let bar_h = 44.0;
    let below = a.origin.y + a.size.height + px(8.0);
    let bounds = Bounds { origin: point(a.origin.x, below), size: size(px(bar_w), px(bar_h)) };
    let mut windows = RecordingWindows::default();
    if let Ok(handle) = cx.open_window(popup(bounds, false), |window, cx| {
        let view = cx.new(|_| Bar { elapsed: 0, paused: false });
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        windows.bar = Some(handle.into());
    }
    if show_border {
        // Four thin windows around the area so the border never appears in the recording.
        let t = 3.0;
        let (x, y, w, h) = (f32::from(a.origin.x), f32::from(a.origin.y), f32::from(a.size.width), f32::from(a.size.height));
        for (bx, by, bw, bh) in [(x - t, y - t, w + 2.0 * t, t), (x - t, y + h, w + 2.0 * t, t), (x - t, y, t, h), (x + w, y, t, h)] {
            let b = Bounds { origin: point(px(bx), px(by)), size: size(px(bw), px(bh)) };
            if let Ok(handle) = cx.open_window(popup(b, false), |_, cx| cx.new(|_| BorderStrip)) {
                windows.border.push(handle.into());
            }
        }
    }
    cx.global_mut::<UiState>().recording = windows;
}

struct BorderStrip;

impl Render for BorderStrip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(cx.theme().danger)
    }
}

impl Render for Countdown {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius::PILL)
            .bg(gpui::black().opacity(0.7))
            .text_color(gpui::white())
            .font_family(text::HEADING_FONT)
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_size(px(72.))
            .child(self.seconds.to_string())
    }
}

impl Render for Bar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = cx.global::<UiState>().services.app.clone();
        let (a1, a2, a3) = (app.clone(), app.clone(), app.clone());
        let paused = self.paused;
        let time = format!("{:02}:{:02}", self.elapsed / 60, self.elapsed % 60);
        let t = cx.theme();
        let x = crate::theme::extra(cx);
        // Paused reads as amber plus the word, so colour is never the only cue.
        let (dot, state) = if paused { (gpui::rgb(0xEAB308).into(), "Paused") } else { (t.danger, "REC") };
        div()
            .size_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .pl(px(14.))
            .pr(px(4.))
            .rounded(radius::PILL)
            .bg(x.card)
            .border_1()
            .border_color(x.ring)
            .font_family(text::UI_FONT)
            .text_color(t.foreground)
            .child(div().size(px(8.)).rounded(radius::PILL).bg(dot))
            .child(div().text_size(px(11.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(t.muted_foreground).child(state))
            .child(div().w(px(48.)).text_size(text::LABEL).font_weight(gpui::FontWeight::MEDIUM).child(time))
            .child(widgets::pill(
                "pause",
                if paused { "Resume" } else { "Pause" },
                Pill::Default,
                true,
                move |_, _, _| if paused { a1.recording_resume() } else { a1.recording_pause() },
                cx,
            ))
            .child(widgets::pill("stop", "Stop", Pill::Dark, true, move |_, _, _| a2.recording_stop(), cx))
            .child(widgets::pill("abort", "Abort", Pill::Ghost, true, move |_, _, _| a3.recording_abort(), cx))
    }
}
