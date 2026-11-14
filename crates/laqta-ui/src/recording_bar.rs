//! The recording bar: elapsed time, pause/resume, stop, abort, and a border around the
//! recorded area. Shown on `RecordingStarted`, closed on `RecordingStopped`/`RecordingFailed`.

use gpui::prelude::FluentBuilder as _;
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

impl RecordingWindows {
    /// True when `window` is the bar, the border or the countdown.
    pub(crate) fn holds(&self, window: AnyWindowHandle) -> bool {
        self.bar == Some(window) || self.countdown == Some(window) || self.border.contains(&window)
    }
}

#[derive(Clone, Copy)]
struct Bar {
    elapsed: u32,
    paused: bool,
    /// The recorded area, and whether to draw the border around it.
    area: PhysRect,
    border: bool,
    /// Stop was pressed and the file is being written (a GIF takes a while).
    saving: bool,
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

/// Physical desktop pixels → logical pixels on a display with this scale. The platform's own
/// mapping wins where it has one (`UiServices::logical_rect`).
fn logical(r: PhysRect, scale: f32, cx: &App) -> Bounds<gpui::Pixels> {
    let mapped = cx.try_global::<UiState>().and_then(|state| state.services.logical_rect.as_ref()).and_then(|map| map(r));
    logical_from(r, scale, mapped)
}

fn logical_from(r: PhysRect, scale: f32, mapped: Option<[f32; 4]>) -> Bounds<gpui::Pixels> {
    let [x, y, width, height] = mapped.unwrap_or([r.x as f32 / scale, r.y as f32 / scale, r.width as f32 / scale, r.height as f32 / scale]);
    Bounds { origin: point(px(x), px(y)), size: size(px(width), px(height)) }
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

fn is_wayland() -> bool {
    cfg!(target_os = "linux")
        && std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        && std::env::var_os("LAQTA_NO_LAYER_SHELL").is_none()
}

/// A layer-shell surface on Wayland, so the compositor neither places it nor lists it as a window;
/// elsewhere a popup at the given bounds. On the display `display` (a layer surface's margins are
/// relative to its output).
fn popup_on(place: Place, size: gpui::Size<gpui::Pixels>, display: Option<gpui::DisplayId>, cx: &App) -> WindowOptions {
    let (bounds, kind, display) = if is_wayland() {
        (Bounds { origin: point(px(0.), px(0.)), size }, layer_kind(place), display)
    } else {
        let bounds = x11_bounds(place, size, cx);
        // Without its display GPUI checks the bounds against the primary one and, when they are
        // off it, opens the window there at a default size.
        let display = display.or_else(|| crate::placement::display_at(cx, bounds).map(|d| d.id()));
        (bounds, WindowKind::PopUp, display)
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
    let screen = crate::placement::main_display(cx).map(|d| d.bounds()).unwrap_or_default();
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
        // The surface's purpose: KWin would otherwise zoom it in like a normal window.
        namespace: "utility".into(),
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
        TaskEvent::RecordingCountdown { seconds_left, area, .. } => countdown(*seconds_left, *area, cx),
        TaskEvent::RecordingStarted { area, show_border, .. } => start(*area, *show_border, cx),
        TaskEvent::RecordingFinishing { .. } => update(cx, |b| b.saving = true),
        TaskEvent::RecordingTick { elapsed_secs, .. } => update(cx, |b| b.elapsed = *elapsed_secs),
        TaskEvent::RecordingPaused { .. } => update(cx, |b| b.paused = true),
        TaskEvent::RecordingResumed { .. } => update(cx, |b| b.paused = false),
        TaskEvent::RecordingStopped { .. } | TaskEvent::RecordingFailed { .. } => close_all(cx),
        _ => {}
    }
}

fn countdown(seconds: u32, area: PhysRect, cx: &mut App) {
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
    // Centred on the area about to be recorded, so it shows on that area's monitor.
    let a = logical(area, open_scale(cx), cx);
    let side = size(px(160.), px(160.));
    let at = Bounds {
        origin: point(a.origin.x + (a.size.width - side.width) / 2.0, a.origin.y + (a.size.height - side.height) / 2.0),
        size: side,
    };
    let display = crate::placement::display_at(cx, a).map(|d| d.id());
    let place = if is_wayland() { Place::Center } else { Place::At(at) };
    if let Ok(handle) = crate::popup::open(popup_on(place, side, display, cx), cx, |window, cx| {
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

/// The scale of a window that is already open: the best first guess for a new window's display.
fn open_scale(cx: &mut App) -> f32 {
    cx.windows().into_iter().find_map(|w| w.update(cx, |_, window, _| window.scale_factor()).ok()).unwrap_or(1.0)
}

/// `border` is the started task's own setting (a workflow can override the defaults).
fn start(area: PhysRect, border: bool, cx: &mut App) {
    if let Some(c) = cx.global_mut::<UiState>().recording.countdown.take() {
        let _ = c.update(cx, |_, window, _| window.remove_window());
    }
    cx.global_mut::<UiState>().recording = RecordingWindows::default();
    let scale = open_scale(cx);
    let bar =
        Bar { elapsed: 0, paused: false, area, border, saving: false, border_scale: None, placed_scale: scale, moves: 0, wanted_y: None };
    open_bar(bar, cx);
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
    // On the area's monitor (on Wayland the compositor would pick one).
    let display = crate::placement::display_at(cx, logical(state.area, state.placed_scale, cx)).map(|d| d.id());
    if let Ok(handle) = crate::popup::open(popup_on(place, bar, display, cx), cx, |window, cx| {
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
    let a = logical(area, scale, cx);
    let center = point(a.origin.x + a.size.width / 2.0, a.origin.y + a.size.height / 2.0);
    let display = cx.displays().into_iter().find(|d| d.bounds().contains(&center)).or_else(|| crate::placement::main_display(cx));
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
    /// A line as thick as the border, this far below the window's top.
    Line(f32),
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
    use crate::presenters::monitor_placement::{Rect, Side, recording_border};
    let a = logical(bar.area, scale, cx);
    let center = point(a.origin.x + a.size.width / 2.0, a.origin.y + a.size.height / 2.0);
    let as_rect = |b: Bounds<gpui::Pixels>| Rect {
        x: f32::from(b.origin.x),
        y: f32::from(b.origin.y),
        w: f32::from(b.size.width),
        h: f32::from(b.size.height),
    };
    // On Wayland a strip is placed on the display holding the area, relative to its origin, and
    // cannot reach another display. Elsewhere strips are in desktop coordinates (negative on a
    // monitor left of or above the primary one), and each goes on the display it lies on.
    let (origin, screens, wayland_display) = if is_wayland() {
        let display = cx.displays().into_iter().find(|d| d.bounds().contains(&center));
        let screen = display.as_ref().map(|d| as_rect(d.bounds()));
        let origin = screen.map_or((0.0, 0.0), |s| (s.x, s.y));
        let local = screen.map_or(Rect { x: 0.0, y: 0.0, w: f32::MAX / 4.0, h: f32::MAX / 4.0 }, |s| Rect { x: 0.0, y: 0.0, ..s });
        (origin, vec![local], display.map(|d| d.id()))
    } else {
        ((0.0, 0.0), cx.displays().iter().map(|d| as_rect(d.bounds())).collect(), None)
    };
    let t = 3.0;
    let area = Rect { x: f32::from(a.origin.x) - origin.0, y: f32::from(a.origin.y) - origin.1, ..as_rect(a) };
    // Windows also rounds a narrow window up by a pixel, so the left strip starts one pixel out.
    let (tall, slack) = if cfg!(windows) { (TALL_STRIP - t, 1.0) } else { (0.0, 0.0) };
    let mut strips = Vec::new();
    for strip in recording_border(area, t, tall, slack, &screens) {
        let r = strip.rect;
        // A tall strip paints only its line next to the area: the top one at its bottom (which is
        // nearer its top when it was cut at the screen's edge), the bottom one at its top.
        let (paint, edge) = match strip.side {
            Side::Top if cfg!(windows) => (Paint::Line(area.y - t - r.y), Edge::Top),
            Side::Bottom if cfg!(windows) => (Paint::Line(0.0), Edge::Bottom),
            Side::Top => (Paint::All, Edge::Top),
            Side::Bottom => (Paint::All, Edge::Bottom),
            Side::Left | Side::Right => (Paint::All, Edge::Side),
        };
        let b = Bounds { origin: point(px(r.x), px(r.y)), size: size(px(r.w), px(r.h)) };
        // `popup_on` finds the display a strip lies on where none is given.
        if let Ok(handle) = crate::popup::open(popup_on(Place::At(b), b.size, wayland_display, cx), cx, |_, cx| {
            cx.new(|_| BorderStrip { paint, edge, thickness: t })
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

/// Which side of the area a strip runs along; the top and bottom ones round their outer corners.
#[derive(Clone, Copy)]
enum Edge {
    Top,
    Bottom,
    Side,
}

struct BorderStrip {
    paint: Paint,
    edge: Edge,
    thickness: f32,
}

impl Render for BorderStrip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let r = px(self.thickness);
        let round = |d: gpui::Div| match self.edge {
            Edge::Top => d.rounded_t(r),
            Edge::Bottom => d.rounded_b(r),
            Edge::Side => d,
        };
        match self.paint {
            Paint::All => round(div().size_full().bg(cx.theme().danger)),
            Paint::Line(offset) => div()
                .size_full()
                .relative()
                .child(round(div().absolute().left_0().top(px(offset)).w_full().h(px(self.thickness)).bg(cx.theme().danger))),
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
        let (dot, state) = if self.saving {
            (t.muted_foreground, "Saving…")
        } else if paused {
            (x.warning, "Paused")
        } else {
            (t.danger, "REC")
        };
        let saving = self.saving;
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
            // Once stopped, the file is still being written: nothing left to press.
            .when(saving, |d| {
                d.child(div().text_size(text::CAPTION).text_color(t.muted_foreground).child("Writing the file, this can take a moment"))
            })
            .when(!saving, |d| {
                d.child(widgets::pill(
                    "pause",
                    if paused { "Resume" } else { "Pause" },
                    Pill::Default,
                    true,
                    move |_, _, _| if paused { a1.recording_resume() } else { a1.recording_pause() },
                    cx,
                ))
                .child(widgets::pill("stop", "Stop", Pill::Dark, true, move |_, _, _| a2.recording_stop(), cx))
                .child(widgets::pill("abort", "Abort", Pill::Ghost, true, move |_, _, _| a3.recording_abort(), cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rect_is_divided_by_its_scale_unless_the_platform_maps_it() {
        let rect = PhysRect { x: 3024, y: 100, width: 800, height: 600 };
        let plain = logical_from(rect, 2.0, None);
        assert_eq!((plain.origin.x, plain.origin.y, plain.size.width, plain.size.height), (px(1512.), px(50.), px(400.), px(300.)));
        // macOS, a 1x display to the right of a Retina one: the origin is not the rect over its scale.
        let mapped = logical_from(rect, 1.0, Some([1512., 100., 800., 600.]));
        assert_eq!((mapped.origin.x, mapped.size.width), (px(1512.), px(800.)));
    }
}
