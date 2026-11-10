//! The recording bar: elapsed time, pause/resume, stop, abort, and a border around the
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

#[derive(Clone, Copy)]
struct Bar {
    elapsed: u32,
    paused: bool,
    /// The recorded area, and whether to draw the border around it.
    area: PhysRect,
    border: bool,
    /// The scale the border was last placed at. It is placed from the bar's render, the first
    /// place the real (possibly fractional) scale is known, and again if that scale changes.
    border_scale: Option<f32>,
    /// The display scale the bar's position was worked out with, and how often it was reopened
    /// because the display it landed on has another one.
    placed_scale: f32,
    moves: u8,
    /// Where the bar was meant to open (not on Wayland), until its first render checked it.
    wanted_y: Option<gpui::Pixels>,
}

struct Countdown {
    seconds: u32,
}

/// Physical desktop pixels → logical pixels on a display with this scale.
fn logical(r: PhysRect, scale: f32) -> Bounds<gpui::Pixels> {
    Bounds {
        origin: point(px(r.x as f32 / scale), px(r.y as f32 / scale)),
        size: size(px(r.width as f32 / scale), px(r.height as f32 / scale)),
    }
}

/// Where a recording window goes.
#[derive(Clone, Copy)]
enum Place {
    /// The middle of the screen (the countdown).
    Center,
    /// Bottom middle of the screen, above the panel (the bar on Wayland).
    BottomCenter,
    /// Exact desktop bounds, in logical pixels.
    At(Bounds<gpui::Pixels>),
}

/// Leaves a recording window with only what it paints: no outline or shadow from the OS.
fn plain(window: &Window, cx: &App) {
    (cx.global::<UiState>().services.native_window)(window, crate::runtime::NativeWindow::PlainPopup);
}

fn is_wayland() -> bool {
    cfg!(target_os = "linux")
        && std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        && std::env::var_os("LAQTA_NO_LAYER_SHELL").is_none()
}

/// A layer-shell surface on Wayland, so the compositor neither places it nor lists it as a window;
/// elsewhere a popup at the given bounds.
fn popup(place: Place, size: gpui::Size<gpui::Pixels>, cx: &App) -> WindowOptions {
    popup_on(place, size, None, cx)
}

/// Like `popup`, on the display `display` (a layer surface's margins are relative to its output).
fn popup_on(place: Place, size: gpui::Size<gpui::Pixels>, display: Option<gpui::DisplayId>, cx: &App) -> WindowOptions {
    let (bounds, kind) = if is_wayland() {
        (Bounds { origin: point(px(0.), px(0.)), size }, layer_kind(place))
    } else {
        (crate::popup::placed(x11_bounds(place, size, cx)), WindowKind::PopUp)
    };
    WindowOptions {
        titlebar: None,
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        focus: false,
        kind,
        display_id: display,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("laqta-recording".into()),
        ..Default::default()
    }
}

fn x11_bounds(place: Place, size: gpui::Size<gpui::Pixels>, cx: &App) -> Bounds<gpui::Pixels> {
    let screen = cx.primary_display().map(|d| d.bounds()).unwrap_or_default();
    match place {
        Place::At(b) => b,
        Place::Center => Bounds {
            origin: point(
                screen.origin.x + (screen.size.width - size.width) / 2.0,
                screen.origin.y + (screen.size.height - size.height) / 2.0,
            ),
            size,
        },
        Place::BottomCenter => Bounds {
            origin: point(
                screen.origin.x + (screen.size.width - size.width) / 2.0,
                screen.origin.y + screen.size.height - size.height - px(BAR_MARGIN),
            ),
            size,
        },
    }
}

#[cfg(target_os = "linux")]
fn layer_kind(place: Place) -> WindowKind {
    use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
    let (anchor, margin) = match place {
        Place::Center => (Anchor::empty(), None),
        Place::BottomCenter => (Anchor::BOTTOM, Some((px(0.), px(0.), px(BAR_MARGIN), px(0.)))),
        Place::At(b) => (Anchor::TOP | Anchor::LEFT, Some((b.origin.y, px(0.), px(0.), b.origin.x))),
    };
    // Exact placement measures from the screen edge, not from the panels.
    let exclusive_zone = matches!(place, Place::At(_)).then(|| px(-1.));
    WindowKind::LayerShell(LayerShellOptions {
        namespace: "laqta-recording".into(),
        layer: Layer::Overlay,
        anchor,
        exclusive_zone,
        margin,
        keyboard_interactivity: KeyboardInteractivity::None,
        ..Default::default()
    })
}

#[cfg(not(target_os = "linux"))]
fn layer_kind(_: Place) -> WindowKind {
    WindowKind::PopUp
}

/// Space between the bar and the screen bottom or the area.
const BAR_MARGIN: f32 = 48.0;
const BAR_SIZE: (f32, f32) = (360.0, 44.0);

fn close_all(cx: &mut App) {
    let w = std::mem::take(&mut cx.global_mut::<UiState>().recording);
    for h in w.bar.into_iter().chain(w.border).chain(w.countdown) {
        let _ = h.update(cx, |_, window, _| window.remove_window());
    }
}

/// Closes the recording windows when no recording runs, after task events were skipped.
pub(crate) fn resync(cx: &mut App) {
    if !cx.global::<UiState>().services.app.is_recording() {
        close_all(cx);
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
    if let Ok(handle) = cx.open_window(popup(Place::Center, size(px(160.), px(160.)), cx), |window, cx| {
        plain(window, cx);
        let view = cx.new(|_| Countdown { seconds });
        // The view is rounded: keep the root's square theme background out of the corners.
        cx.new(|cx| Root::new(view, window, cx).bg(gpui::transparent_black()))
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
    let border = cx.global::<UiState>().services.app.settings().defaults.recording.show_border;
    cx.global_mut::<UiState>().recording = RecordingWindows::default();
    // The scale of a window that is already open is the best first guess for the bar's display.
    let scale = cx.windows().into_iter().find_map(|w| w.update(cx, |_, window, _| window.scale_factor()).ok()).unwrap_or(1.0);
    open_bar(Bar { elapsed: 0, paused: false, area, border, border_scale: None, placed_scale: scale, moves: 0, wanted_y: None }, cx);
}

/// Opens the bar for `state`, in place of any bar already open.
fn open_bar(mut state: Bar, cx: &mut App) {
    let bar = size(px(BAR_SIZE.0), px(BAR_SIZE.1));
    let place = if is_wayland() { Place::BottomCenter } else { Place::At(bar_bounds(state.area, state.placed_scale, bar, cx)) };
    state.wanted_y = match place {
        Place::At(bounds) => Some(bounds.origin.y),
        _ => None,
    };
    if let Some(old) = cx.global_mut::<UiState>().recording.bar.take() {
        let _ = old.update(cx, |_, window, _| window.remove_window());
    }
    if let Ok(handle) = cx.open_window(popup(place, bar, cx), |window, cx| {
        plain(window, cx);
        let view = cx.new(|_| state);
        // The view is rounded: keep the root's square theme background out of the corners.
        cx.new(|cx| Root::new(view, window, cx).bg(gpui::transparent_black()))
    }) {
        cx.global_mut::<UiState>().recording.bar = Some(handle.into());
    }
}

/// Windows and X11: the bar under the area, above it when that is off the screen's work area (the
/// part clear of the taskbar), else inside its bottom edge. `scale` turns the physical area into
/// the logical pixels of its display.
fn bar_bounds(area: PhysRect, scale: f32, bar: gpui::Size<gpui::Pixels>, cx: &App) -> Bounds<gpui::Pixels> {
    let a = logical(area, scale);
    let center = point(a.origin.x + a.size.width / 2.0, a.origin.y + a.size.height / 2.0);
    let display = cx.displays().into_iter().find(|d| d.bounds().contains(&center)).or_else(|| cx.primary_display());
    let screen = display.map(|d| d.visible_bounds()).unwrap_or(a);
    let bottom = screen.origin.y + screen.size.height;
    let below = a.origin.y + a.size.height + px(8.0);
    let above = a.origin.y - bar.height - px(8.0);
    let y = if below + bar.height <= bottom {
        below
    } else if above >= screen.origin.y {
        above
    } else {
        (a.origin.y + a.size.height).min(bottom) - bar.height - px(8.0)
    };
    let right = (screen.origin.x + screen.size.width - bar.width).max(screen.origin.x);
    Bounds { origin: point(a.origin.x.clamp(screen.origin.x, right), y), size: bar }
}

/// Windows can't make a window shorter than a title bar. There the top and bottom strips are this
/// tall and transparent, and paint only the edge next to the area.
const TALL_STRIP: f32 = 40.0;

/// The part of its window a border strip paints.
#[derive(Clone, Copy)]
enum Paint {
    All,
    TopEdge,
    BottomEdge,
}

/// Four thin windows just outside the area, so the border never appears in the recording. Any
/// border from an earlier placement is closed first. `bar` is the bar's state at `scale`.
fn place_border(bar: Bar, scale: f32, cx: &mut App) {
    if cx.global::<UiState>().recording.bar.is_none() {
        return;
    }
    for h in std::mem::take(&mut cx.global_mut::<UiState>().recording.border) {
        let _ = h.update(cx, |_, window, _| window.remove_window());
    }
    let a = logical(bar.area, scale);
    // On Wayland each strip is placed on the display holding the area, relative to its origin.
    let center = point(a.origin.x + a.size.width / 2.0, a.origin.y + a.size.height / 2.0);
    let display = cx.displays().into_iter().find(|d| d.bounds().contains(&center));
    let (display_id, screen) = match (&display, is_wayland()) {
        (Some(d), true) => (Some(d.id()), d.bounds()),
        _ => (None, Bounds::default()),
    };
    let (ox, oy) = (f32::from(screen.origin.x), f32::from(screen.origin.y));
    let t = 3.0;
    let (x, y, w, h) = (f32::from(a.origin.x) - ox, f32::from(a.origin.y) - oy, f32::from(a.size.width), f32::from(a.size.height));
    // Windows also rounds a narrow window up by a pixel, so the left strip starts one pixel out.
    let (tall, slack) = if cfg!(windows) { (TALL_STRIP - t, 1.0) } else { (0.0, 0.0) };
    let (top, bottom) = if cfg!(windows) { (Paint::BottomEdge, Paint::TopEdge) } else { (Paint::All, Paint::All) };
    let mut strips = Vec::new();
    for (bx, by, bw, bh, paint) in [
        (x - t, y - t - tall, w + 2.0 * t, t + tall, top),
        (x - t, y + h, w + 2.0 * t, t + tall, bottom),
        (x - t - slack, y, t, h, Paint::All),
        (x + w, y, t, h, Paint::All),
    ] {
        // A strip that would start off the screen is left out; shifting it would put it over the area.
        if bx < 0. || by < 0. {
            continue;
        }
        let b = Bounds { origin: point(px(bx), px(by)), size: size(px(bw), px(bh)) };
        if let Ok(handle) = cx.open_window(popup_on(Place::At(b), b.size, display_id, cx), |window, cx| {
            plain(window, cx);
            cx.new(|_| BorderStrip { paint, thickness: t })
        }) {
            strips.push(handle.into());
        }
    }
    cx.global_mut::<UiState>().recording.border = strips;
    // The transparent part of the bottom strip covers the bar and would take its clicks, so the
    // bar is opened again, above the strips.
    if cfg!(windows) {
        open_bar(bar, cx);
    }
}

struct BorderStrip {
    paint: Paint,
    thickness: f32,
}

impl Render for BorderStrip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let line = div().w_full().h(px(self.thickness)).bg(cx.theme().danger);
        match self.paint {
            Paint::All => div().size_full().bg(cx.theme().danger),
            Paint::TopEdge => div().size_full().flex().flex_col().justify_start().child(line),
            Paint::BottomEdge => div().size_full().flex().flex_col().justify_end().child(line),
        }
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        if let Some(wanted_y) = self.wanted_y.take() {
            crate::popup::observe(window, wanted_y);
        }
        // The bar landed on a display with another scale than its position assumed: place it again.
        if !is_wayland() && (scale - self.placed_scale).abs() > 0.01 && self.moves < 2 {
            let again = Bar { border_scale: None, placed_scale: scale, moves: self.moves + 1, ..*self };
            cx.defer(move |cx| open_bar(again, cx));
        } else if self.border && self.border_scale != Some(scale) {
            self.border_scale = Some(scale);
            let bar = *self;
            cx.defer(move |cx| place_border(bar, scale, cx));
        }
        let app = cx.global::<UiState>().services.app.clone();
        let (a1, a2, a3) = (app.clone(), app.clone(), app.clone());
        let paused = self.paused;
        let time = format!("{:02}:{:02}", self.elapsed / 60, self.elapsed % 60);
        let t = cx.theme();
        let x = crate::theme::extra(cx);
        // Paused reads as amber plus the word, so colour is never the only cue.
        let (dot, state) = if paused { (x.warning, "Paused") } else { (t.danger, "REC") };
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
            .child(div().text_size(text::CAPTION).font_weight(gpui::FontWeight::SEMIBOLD).text_color(t.muted_foreground).child(state))
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
