//! The region overlay (`Overlay` port, ADR 012): one borderless fullscreen window per
//! monitor showing the frozen frame. It forwards pointer and keyboard input in physical desktop
//! pixels and paints the Region session's `RegionView`. It computes nothing itself.

use std::sync::Arc;

use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Div, Entity, FocusHandle, Hsla, InteractiveElement as _, IntoElement,
    KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render, RenderImage,
    ScrollDelta, ScrollEvent, Styled as _, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, img, point,
    prelude::FluentBuilder as _, px, size,
};
use laqta_domain::geometry::PhysRect;
use laqta_domain::region::{Button, KeyMods, RegionInput, RegionKey, TargetKind};
use laqta_domain::{Bitmap, Frame, PixelFormat};
use laqta_editor::EditorInput;
use laqta_ports::{InputCallback, RegionView};

use crate::mark_chrome::pointer_style;
use crate::presenters::capture_pointer::{self, Route};
use crate::presenters::capture_toolbar as bar;
use crate::presenters::mark_pointer::Pointer;
use crate::runtime::UiState;

#[derive(Default)]
pub(crate) struct OverlayWindows {
    model: Option<Entity<OverlayModel>>,
    windows: Vec<AnyWindowHandle>,
}

impl OverlayWindows {
    /// True when `window` is one of the overlay's windows.
    pub(crate) fn holds(&self, window: AnyWindowHandle) -> bool {
        self.windows.contains(&window)
    }

    /// Cancels the selection the way Escape does. The session then closes the windows.
    pub(crate) fn cancel(&self, cx: &App) {
        if let Some(model) = &self.model {
            let on_input = model.read(cx).on_input.clone();
            on_input(RegionInput::Key { key: RegionKey::Escape, mods: KeyMods::default() });
        }
    }

    #[cfg(test)]
    pub(crate) fn model(&self) -> Option<Entity<OverlayModel>> {
        self.model.clone()
    }
}

struct MonitorImage {
    rect: PhysRect,
    image: Arc<RenderImage>,
    bitmap: Bitmap,
}

pub(crate) struct OverlayModel {
    monitors: Vec<MonitorImage>,
    pub(crate) view: RegionView,
    pub(crate) on_input: InputCallback,
    /// The editor engine over the settled selection (`overlay_tools`).
    pub(crate) annot: Option<crate::overlay_tools::Annot>,
    /// The drawing tool in use; `None` is the region tool (adjust the selection).
    pub(crate) tool: Option<laqta_editor::Tool>,
    /// The colour and size picked for each drawing tool (`Settings::capture_marks`).
    pub(crate) marks: Vec<laqta_domain::capture::MarkStyle>,
    /// `marks` changed since the overlay opened; they are saved when it closes.
    pub(crate) marks_changed: bool,
    /// The button that went down on a bar (toolbar, style strip, mode bar), so its release is the
    /// bar's click and not the end of a drag on the canvas or the selection. Kept per button: a
    /// bar press during another button's drag can't swallow that drag's release.
    pub(crate) chrome_press: Option<MouseButton>,
    /// The mode bar's Text button is on: the next settled selection is read in place.
    pub(crate) text_mode: bool,
    /// The selection's text, read in place (`overlay_text`).
    pub(crate) text: Option<crate::overlay_text::TextUi>,
    /// When Esc (or a close button) last warned that the marks would go (`capture_escape`).
    pub(crate) discard_armed: Option<std::time::Instant>,
    /// Who took the press under way (`capture_pointer::route`): its moves and release go there too,
    /// on whichever monitor's window they reach.
    pub(crate) pressed: Option<Route>,
    /// The button that made `pressed`: another button's press and release during the drag are
    /// ignored, so they can't take the route from it.
    pub(crate) pressed_button: Option<MouseButton>,
    /// How far a picked mark's handle reaches, in desktop pixels: `mark_pointer::GRAB` times the
    /// scale of the monitor the pointer is on, so the grab square is the same size on every screen.
    pub(crate) grab: f32,
    /// The annotated crop is being exported for an action: the frame and the marks hold still until
    /// it lands, so the picture sent is of the selection the session finishes with.
    pub(crate) finishing: bool,
    /// The last press on the selection held Alt: a move leaves the marks where they are on screen.
    pub(crate) frame_only: bool,
    /// The gesture settling now was a resize: its marks stay over their pixels, whatever size it
    /// ends at.
    pub(crate) resized: bool,
    /// Holding Space lends the Pointer (`None`) while a drawing tool is out.
    pub(crate) space: crate::presenters::space_lend::SpaceLend<Option<laqta_editor::Tool>>,
}

/// Converts each frame for GPUI, off the GPUI thread: `UiHandle::show` calls this on the caller's
/// thread, one scoped thread per extra monitor.
pub(crate) fn render_images(frames: &[Frame]) -> Vec<Option<Arc<RenderImage>>> {
    match frames {
        [] => Vec::new(),
        [one] => vec![render_image(&one.image)],
        many => std::thread::scope(|scope| {
            let handles: Vec<_> = many.iter().map(|f| scope.spawn(|| render_image(&f.image))).collect();
            handles.into_iter().map(|h| h.join().ok().flatten()).collect()
        }),
    }
}

use crate::gpu_image::render_image;

/// Pairs each frame with a display. Windows and X11 put a display at its physical origin divided
/// by its scale, so a frame's monitor names its display exactly, whatever the mix of scales. Where
/// that doesn't hold for every frame (Wayland lays displays out itself), both are sorted by
/// position (left to right, then top to bottom).
fn match_displays(frames: &[Frame], cx: &App) -> Vec<Option<Bounds<Pixels>>> {
    let mut displays: Vec<_> = cx.displays().into_iter().map(|d| (d.bounds(), d.id())).collect();
    let exact: Vec<_> = frames
        .iter()
        .filter_map(|f| {
            let scale = f.monitor.scale.max(0.1);
            let (x, y) = (f.monitor.rect.x as f32 / scale, f.monitor.rect.y as f32 / scale);
            displays.iter().find(|(b, _)| (f32::from(b.origin.x) - x).abs() < 1.0 && (f32::from(b.origin.y) - y).abs() < 1.0).copied()
        })
        .collect();
    let distinct = exact.iter().enumerate().all(|(i, a)| exact[..i].iter().all(|b| b.1 != a.1));
    if exact.len() == frames.len() && distinct {
        return exact.into_iter().map(|(bounds, _)| Some(bounds)).collect();
    }
    displays.sort_by(|a, b| {
        let (ax, ay) = (f32::from(a.0.origin.x), f32::from(a.0.origin.y));
        let (bx, by) = (f32::from(b.0.origin.x), f32::from(b.0.origin.y));
        ax.total_cmp(&bx).then(ay.total_cmp(&by))
    });
    let mut order: Vec<usize> = (0..frames.len()).collect();
    order.sort_by_key(|&i| (frames[i].monitor.rect.x, frames[i].monitor.rect.y));
    let mut out = vec![None; frames.len()];
    if displays.len() == frames.len() {
        for (slot, &i) in order.iter().enumerate() {
            out[i] = Some(displays[slot].0);
        }
    }
    out
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn is_wayland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

/// `images` holds `render_images(&frames)`, converted before the command reached the GPUI thread.
pub(crate) fn show(frames: Vec<Frame>, images: Vec<Option<Arc<RenderImage>>>, view: RegionView, on_input: InputCallback, cx: &mut App) {
    close(cx);
    let display_bounds = match_displays(&frames, cx);
    let monitors: Vec<MonitorImage> = frames
        .iter()
        .zip(images)
        .filter_map(|(f, image)| Some(MonitorImage { rect: f.monitor.rect, image: image?, bitmap: f.image.clone() }))
        .collect();
    let count = monitors.len();
    let marks = cx.global::<UiState>().services.app.settings().capture_marks;
    let model = cx.new(|_| OverlayModel {
        monitors,
        view,
        on_input,
        annot: None,
        tool: None,
        marks,
        marks_changed: false,
        chrome_press: None,
        text_mode: false,
        text: None,
        discard_armed: None,
        pressed: None,
        pressed_button: None,
        frame_only: false,
        resized: false,
        grab: crate::presenters::mark_pointer::GRAB,
        finishing: false,
        space: crate::presenters::space_lend::SpaceLend::new(None),
    });
    let mut windows = Vec::new();
    for index in 0..count {
        let bounds = display_bounds.get(index).copied().flatten();
        match open_window(model.clone(), index, bounds, overlay_kind(), cx) {
            Ok(w) => windows.push(w),
            // A compositor without layer-shell (GNOME): fall back to a fullscreen window (ADR 019).
            Err(e) if !matches!(overlay_kind(), WindowKind::PopUp) => {
                tracing::warn!("overlay layer surface {index} failed ({e:#}); using a fullscreen window");
                match open_window(model.clone(), index, bounds, WindowKind::PopUp, cx) {
                    Ok(w) => windows.push(w),
                    Err(e) => tracing::error!("overlay window {index} failed: {e:#}"),
                }
            }
            Err(e) => tracing::error!("overlay window {index} failed: {e:#}"),
        }
    }
    if windows.is_empty() {
        // Nothing to draw on: cancel the selection so the session doesn't wait forever.
        let cb = model.read(cx).on_input.clone();
        cb(RegionInput::Key { key: RegionKey::Escape, mods: KeyMods::default() });
    }
    let state = cx.global_mut::<UiState>();
    state.overlay = OverlayWindows { model: Some(model), windows };
}

fn open_window(
    model: Entity<OverlayModel>,
    index: usize,
    bounds: Option<Bounds<Pixels>>,
    kind: WindowKind,
    cx: &mut App,
) -> anyhow_like::Result<AnyWindowHandle> {
    let bounds = bounds.unwrap_or_else(|| {
        let r = model.read(cx).monitors[index].rect;
        Bounds { origin: point(px(r.x as f32), px(r.y as f32)), size: size(px(r.width as f32), px(r.height as f32)) }
    });
    let wanted_y = matches!(kind, WindowKind::PopUp).then_some(bounds.origin.y);
    // macOS opens the overlay hidden: `NativeWindow::CoverScreen` restyles it and then shows it and
    // gives it the keyboard. Restyling a window that already has the keyboard crashes AppKit later
    // (its Touch Bar observer still watches the frame view the restyle threw away).
    let shown_at_once = !cfg!(target_os = "macos");
    let options = WindowOptions {
        titlebar: None,
        window_bounds: Some(if matches!(kind, WindowKind::PopUp) {
            WindowBounds::Windowed(crate::popup::placed(bounds))
        } else {
            WindowBounds::Fullscreen(bounds)
        }),
        focus: shown_at_once,
        show: shown_at_once,
        kind,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Opaque,
        app_id: Some("laqta-overlay".into()),
        ..Default::default()
    };
    let handle = cx
        .open_window(options, |window, cx| {
            crate::perf::first_frame("overlay", window);
            let view = cx.new(|cx| OverlayView::new(model, index, wanted_y, cx));
            // Space let go in another window never comes back as a key-up here: losing activation
            // gives the lent tool back.
            view.update(cx, |_, cx| {
                cx.observe_window_activation(window, |this, window, cx| {
                    if !window.is_window_active() {
                        this.model.update(cx, |m, cx| m.space_lost(cx));
                    }
                })
                .detach();
            });
            let focus = view.read(cx).focus.clone();
            window.focus(&focus, cx);
            view
        })
        .map_err(|e| anyhow_like::Error(format!("{e:#}")))?;
    if !shown_at_once {
        let native = cx.global::<UiState>().services.native_window.clone();
        let _ = handle.update(cx, |_, window, _| {
            if !native(window, crate::runtime::NativeWindow::CoverScreen) {
                window.activate_window();
            }
        });
    }
    Ok(handle.into())
}

#[cfg(target_os = "linux")]
fn overlay_kind() -> WindowKind {
    use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
    if is_wayland() && std::env::var_os("LAQTA_NO_LAYER_SHELL").is_none() {
        WindowKind::LayerShell(LayerShellOptions {
            // The namespace states the surface's purpose, and KWin animates by purpose: one it
            // doesn't know opens like a normal window (zooming in over the frozen screen), and an
            // on-screen display fades in. A utility surface appears at once.
            namespace: "utility".into(),
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            exclusive_zone: Some(px(-1.)),
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            ..Default::default()
        })
    } else {
        WindowKind::PopUp
    }
}

/// Whether the shortcut modifier is held: Ctrl, or Command on macOS.
fn ctrl(m: &gpui::Modifiers) -> bool {
    m.control || (cfg!(target_os = "macos") && m.platform)
}

#[cfg(not(target_os = "linux"))]
fn overlay_kind() -> WindowKind {
    WindowKind::PopUp
}

pub(crate) fn update(view: RegionView, cx: &mut App) {
    let model = cx.global::<UiState>().overlay.model.clone();
    if let Some(model) = model {
        model.update(cx, |m, cx| {
            // A resize keeps the marks over their pixels even when it ends at the old size.
            if view.resizing.is_some() {
                m.resized = true;
            }
            m.view = view;
            m.sync_annot(cx);
            m.sync_text();
            cx.notify();
        });
    }
}

pub(crate) fn close(cx: &mut App) {
    if !cx.has_global::<UiState>() {
        return;
    }
    let windows = std::mem::take(&mut cx.global_mut::<UiState>().overlay);
    // The colours and sizes picked this time, saved once rather than on every click.
    if let Some(model) = &windows.model
        && let (true, marks) = model.update(cx, |m, _| (std::mem::take(&mut m.marks_changed), m.marks.clone()))
    {
        let services = cx.global::<UiState>().services.clone();
        let mut settings = services.app.settings();
        settings.capture_marks = marks;
        if let Err(e) = (services.save_settings)(settings) {
            tracing::warn!("saving the capture toolbar's colours failed: {e}");
        }
    }
    for w in windows.windows {
        let _ = w.update(cx, |_, window, _| window.remove_window());
    }
}

/// Errors from opening windows, without pulling in `anyhow`.
mod anyhow_like {
    #[derive(Debug)]
    pub struct Error(pub String);
    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    pub type Result<T> = std::result::Result<T, Error>;
}

struct OverlayView {
    model: Entity<OverlayModel>,
    index: usize,
    focus: FocusHandle,
    /// Wheel travel not yet sent, in notches; touchpads scroll in fractions of one.
    wheel: f32,
    /// Where a popup overlay was meant to open, until its first render checked it.
    wanted_y: Option<Pixels>,
    /// Whether the window was brought to the front after it opened.
    raised: bool,
    /// Where the pointer last moved over this window, in desktop pixels. The hover cursor reads it
    /// here rather than waiting for the session to echo the move back.
    pointer: Option<(i32, i32)>,
}

/// Pixels of touchpad scrolling that count as one wheel notch.
const PIXELS_PER_NOTCH: f32 = 40.0;

/// Adds `delta` notches to `acc` and returns the whole notches to send (positive grows the hover),
/// keeping the remainder.
fn wheel_steps(acc: &mut f32, delta: f32) -> i32 {
    *acc += delta;
    let steps = acc.trunc();
    *acc -= steps;
    steps as i32
}

const SELECTION: u32 = 0x3d8bfd;
const HOVER: u32 = 0x8ab4f8;

fn color(rgb: u32, alpha: f32) -> Hsla {
    let mut c: Hsla = gpui::rgb(rgb).into();
    c.a = alpha;
    c
}

impl OverlayView {
    fn new(model: Entity<OverlayModel>, index: usize, wanted_y: Option<Pixels>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        OverlayView { model, index, focus: cx.focus_handle(), wheel: 0.0, wanted_y, raised: false, pointer: None }
    }

    fn send(&self, input: RegionInput, cx: &App) {
        let cb = self.model.read(cx).on_input.clone();
        cb(input);
    }

    fn to_phys(&self, pos: gpui::Point<Pixels>, window: &Window, cx: &App) -> (i32, i32) {
        let s = window.scale_factor();
        let r = self.model.read(cx).monitors[self.index].rect;
        (r.x + (f32::from(pos.x) * s).round() as i32, r.y + (f32::from(pos.y) * s).round() as i32)
    }
}

fn button(b: MouseButton) -> Option<Button> {
    match b {
        MouseButton::Left => Some(Button::Left),
        MouseButton::Right => Some(Button::Right),
        MouseButton::Middle => Some(Button::Middle),
        _ => None,
    }
}

fn region_key(key: &str) -> Option<RegionKey> {
    Some(match key {
        "escape" => RegionKey::Escape,
        "enter" => RegionKey::Enter,
        "left" => RegionKey::Left,
        "right" => RegionKey::Right,
        "up" => RegionKey::Up,
        "down" => RegionKey::Down,
        "delete" | "backspace" => RegionKey::Delete,
        "tab" => RegionKey::Tab,
        "space" => RegionKey::Space,
        _ => return None,
    })
}

/// A rect in this window's logical pixels.
struct Local {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

fn boxed(l: &Local) -> Div {
    div().absolute().left(px(l.x)).top(px(l.y)).w(px(l.w)).h(px(l.h))
}

impl Render for OverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(wanted_y) = self.wanted_y.take() {
            crate::popup::observe(window, wanted_y);
        }
        // Once, when the overlay is on screen: Esc and the tool keys must reach it.
        if !std::mem::replace(&mut self.raised, true) {
            let native = cx.global::<UiState>().services.native_window.clone();
            native(window, crate::runtime::NativeWindow::BringToFront);
        }
        let s = window.scale_factor();
        let model = self.model.read(cx);
        let mon = &model.monitors[self.index];
        let view = &model.view;
        let origin = mon.rect;
        let local = |r: PhysRect| Local {
            x: (r.x - origin.x) as f32 / s,
            y: (r.y - origin.y) as f32 / s,
            w: r.width as f32 / s,
            h: r.height as f32 / s,
        };
        let win_w = origin.width as f32 / s;
        let win_h = origin.height as f32 / s;
        // GPUI sizes displays by the integer output scale, so on a fractionally scaled output
        // (KDE at 125%) the window opens too small. Once the real scale is known, grow it to
        // cover the whole monitor.
        let viewport = window.viewport_size();
        if (f32::from(viewport.width) - win_w).abs() > 1.0 || (f32::from(viewport.height) - win_h).abs() > 1.0 {
            window.resize(size(px(win_w), px(win_h)));
        }
        let dim = color(0x000000, view.dim_alpha as f32 / 255.0);

        let mut root = div()
            .id("laqta-overlay")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .cursor(pointer_style(self.pointer_shape(model), view.gesture))
            .child(img(mon.image.clone()).absolute().left_0().top_0().w(px(win_w)).h(px(win_h)));

        // Dim everything except the main selection (the rect being drawn, else the active region).
        let main = view.drawing.or_else(|| view.active.and_then(|i| view.regions.get(i).copied()));
        match main.map(local) {
            Some(sel) => {
                let x0 = sel.x.clamp(0.0, win_w);
                let y0 = sel.y.clamp(0.0, win_h);
                let x1 = (sel.x + sel.w).clamp(0.0, win_w);
                let y1 = (sel.y + sel.h).clamp(0.0, win_h);
                for l in [
                    Local { x: 0.0, y: 0.0, w: win_w, h: y0 },
                    Local { x: 0.0, y: y1, w: win_w, h: win_h - y1 },
                    Local { x: 0.0, y: y0, w: x0, h: y1 - y0 },
                    Local { x: x1, y: y0, w: win_w - x1, h: y1 - y0 },
                ] {
                    if l.w > 0.0 && l.h > 0.0 {
                        root = root.child(boxed(&l).bg(dim));
                    }
                }
            }
            None => root = root.child(div().absolute().left_0().top_0().size_full().bg(dim)),
        }

        // Once a selection settles, window snapping and the magnifier step aside for the tools. While
        // it is moved or resized the magnifier comes back, for precision.
        let settled = view.selected.is_some() && !view.gesture;
        if view.drawing.is_none()
            && !settled
            && !view.pick_point
            && let Some(t) = &view.hovered
        {
            let l = local(t.rect);
            if t.kind == TargetKind::Element {
                // A region inside a window: a thinner outline, a lighter tint, and its size, since
                // nested regions are told apart by size. The wheel grows it to the enclosing one.
                root = root.child(boxed(&l).border_1().border_color(color(HOVER, 0.95)).bg(color(HOVER, 0.08)));
                let y = if l.y >= 30.0 { l.y - 28.0 } else { l.y + l.h + 4.0 };
                let text = bar::badge_text(t.rect, None);
                let at = bar::Rect { x: l.x.max(4.0), y, w: crate::overlay_tools::badge_width(&text), h: crate::overlay_tools::BADGE_H };
                root = root.child(crate::overlay_tools::size_badge(text, at));
            } else {
                root = root.child(boxed(&l).border_2().border_color(color(HOVER, 1.0)).bg(color(HOVER, 0.12)));
            }
        }
        // The annotation canvas sits under the selection's border and handles, clipped to the frame.
        // While a move carries the marks along it steps aside: its pixels are the old crop's, and
        // the marks come back on the new crop at release.
        let carrying = view.gesture && view.resizing.is_none() && !model.frame_only;
        if !carrying
            && let Some(sel) = view.selected
            && let Some(a) = &model.annot
            && let Some(canvas) = crate::overlay_tools::canvas(
                a,
                |r| {
                    let l = local(r);
                    bar::Rect { x: l.x, y: l.y, w: l.w, h: l.h }
                },
                {
                    let l = local(sel);
                    bar::Rect { x: l.x, y: l.y, w: l.w, h: l.h }
                },
                s,
            )
        {
            root = root.child(canvas);
        }
        // The selection's text, read in place: over the canvas, under the border and handles.
        let now = std::time::Instant::now();
        let still = cx.reduce_motion();
        if let Some(sel) = view.selected
            && let Some(text) = &model.text
            && sel.x < origin.right()
            && sel.right() > origin.x
            && sel.y < origin.bottom()
            && sel.bottom() > origin.y
        {
            let l = local(sel);
            let at = crate::presenters::capture_toolbar::Rect { x: l.x, y: l.y, w: l.w, h: l.h };
            root = root.children(crate::overlay_text::layer(text, at, s, now, still));
            if !still && text.animating(now) {
                window.request_animation_frame();
            }
        }
        // Borders sit just outside the selection, so they never cover the pixels it captures.
        for (i, r) in view.regions.iter().enumerate() {
            let active = Some(i) == view.active;
            root = root.child(boxed(&outset(local(*r), BORDER)).border_2().border_color(color(SELECTION, if active { 1.0 } else { 0.7 })));
        }
        if let Some(d) = view.drawing {
            let l = local(d);
            root = root.child(boxed(&outset(local(d), BORDER)).border_2().border_color(color(SELECTION, 1.0)));
            // The size follows the rectangle while it is drawn.
            if d.width > 0 && d.height > 0 {
                let y = if l.y >= 32.0 { l.y - 30.0 } else { l.y + l.h + 6.0 };
                let text = bar::badge_text(d, None);
                let at = bar::Rect { x: l.x.max(4.0), y, w: crate::overlay_tools::badge_width(&text), h: crate::overlay_tools::BADGE_H };
                root = root.child(crate::overlay_tools::size_badge(text, at));
            }
        }
        // Each dot fills the session's hit rect; the grab area around it (`grab_at`) is wider.
        for (_, h) in &view.handles {
            let l = local(*h);
            root = root.child(
                div()
                    .absolute()
                    .left(px(l.x))
                    .top(px(l.y))
                    .w(px(l.w))
                    .h(px(l.h))
                    .rounded(crate::theme::radius::PILL)
                    .bg(gpui::white())
                    .border_2()
                    .border_color(color(SELECTION, 1.0))
                    .shadow_sm(),
            );
        }

        let (cx_phys, cy_phys) = view.cursor;
        let cursor_here = cx_phys >= origin.x && cx_phys < origin.right() && cy_phys >= origin.y && cy_phys < origin.bottom();
        if cursor_here && !settled {
            let lx = (cx_phys - origin.x) as f32 / s;
            let ly = (cy_phys - origin.y) as f32 / s;
            if view.show_crosshair {
                let line = color(0xffffff, 0.6);
                root = root
                    .child(div().absolute().left_0().top(px(ly)).w(px(win_w)).h(px(1.)).bg(line))
                    .child(div().absolute().left(px(lx)).top_0().w(px(1.)).h(px(win_h)).bg(line));
            }
            root = root.child(magnifier_and_info(view, mon, lx, ly, win_w, win_h, s));
        }

        // In-place annotation and the capture toolbar, on the monitor that holds the selection.
        if let Some(sel) = view.selected {
            let to_local = |r: PhysRect| {
                let l = local(r);
                bar::Rect { x: l.x, y: l.y, w: l.w, h: l.h }
            };
            let (cx_sel, cy_sel) = (sel.x + sel.width as i32 / 2, sel.y + sel.height as i32 / 2);
            let holds = cx_sel >= origin.x && cx_sel < origin.right() && cy_sel >= origin.y && cy_sel < origin.bottom();
            if holds || (self.index == 0 && !model.monitors.iter().any(|m| contains(m.rect, (cx_sel, cy_sel)))) {
                let sel_l = to_local(sel);
                let screen = (win_w, win_h);
                // While the text shows, its bar takes the toolbar's place.
                let bar_size = match &model.text {
                    Some(text) => crate::overlay_text::bar_size(&text.notice(now), win_w),
                    None => (crate::overlay_tools::bar_width(), crate::overlay_tools::BAR_H),
                };
                let (at, side) = bar::place(sel_l, bar_size, screen);
                // The size stays live while the selection is moved (with its position) or resized.
                let moving = view.gesture && view.resizing.is_none();
                let text = bar::badge_text(sel, moving.then(|| (sel.x - view.bounds.x, sel.y - view.bounds.y)));
                let badge = bar::badge(sel_l, side, (crate::overlay_tools::badge_width(&text), crate::overlay_tools::BADGE_H), screen);
                root = root.child(crate::overlay_tools::size_badge(text, badge));
                // The bars wait for the gesture to end, so they never jump along with it.
                if view.gesture {
                } else if model.text.is_some() {
                    root = root.child(crate::overlay_text::bar(&self.model, at, now, still, cx));
                } else if model.toolbar_shown() {
                    root = root.child(crate::overlay_tools::toolbar(&self.model, at, cx));
                    if let Some(strip) = crate::overlay_tools::style_strip(&self.model, at, side, screen, cx) {
                        root = root.child(strip);
                    }
                }
                // Beside the toolbar, where the eye already is; the mode bar covers the screen's top.
                if let Some(text) = model.discard_notice() {
                    let (w, h) = (crate::overlay_tools::notice_width(&text), crate::overlay_tools::NOTICE_H);
                    let spot = bar::strip(at, side, (w, h), screen, at.x + at.w / 2.0);
                    root = root.child(crate::overlay_tools::notice(text, spot));
                }
            }
        } else if view.pick_point {
            if contains(origin, view.cursor) {
                root = root.child(crate::overlay_tools::hint(
                    win_w,
                    "Click to pick a color  ·  Arrow keys move one pixel  ·  Enter picks  ·  Esc closes",
                ));
            }
        } else if self.index == 0 && view.regions.is_empty() && view.drawing.is_none() && !view.mode_bar {
            root = root.child(crate::overlay_tools::hint(
                win_w,
                if view.text_only {
                    "Drag over the text you want  ·  Click a window to read all of it  ·  Esc to close"
                } else {
                    "Drag to select, or click a window or a part of one  ·  Scroll to grow  ·  Esc to cancel"
                },
            ));
        }
        // The mode bar sits at the top of the monitor under the pointer (the first one otherwise).
        let pointer_here = contains(origin, view.cursor);
        let pointer_anywhere = model.monitors.iter().any(|m| contains(m.rect, view.cursor));
        // It steps aside while a rectangle is being drawn, so it never covers the selection.
        if view.mode_bar && view.drawing.is_none() && (pointer_here || (self.index == 0 && !pointer_anywhere)) {
            root = root.child(crate::overlay_tools::mode_bar(&self.model, win_w, cx));
            if view.regions.is_empty() && view.selected.is_none() {
                root = root.child(crate::overlay_tools::mode_hint(win_w));
            }
        }

        // Every press goes where `capture_pointer::route` says, and its moves and release follow it.
        root.on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
            let (x, y) = this.to_phys(ev.position, window, cx);
            this.model.update(cx, |m, _| m.set_grab_scale(window.scale_factor()));
            let before = this.pointer_shape(this.model.read(cx));
            this.pointer = Some((x, y));
            if this.pointer_shape(this.model.read(cx)) != before {
                cx.notify();
            }
            this.send(RegionInput::PointerMove { x, y }, cx);
            if this.model.update(cx, |m, _| m.hover((x, y))) {
                cx.notify();
            }
            // A drag over the text picks words, wherever the pointer goes.
            if this.model.update(cx, |m, cx| m.text_drag(x, y, cx)) {
                return;
            }
            // The engine follows its own drags, and the hover while a tool is out.
            let to_editor = match this.model.read(cx).pressed {
                Some(route) => route == Route::Editor,
                None => this.model.read(cx).tool.is_some(),
            };
            if to_editor && let Some((ix, iy)) = this.model.read(cx).canvas_point(x, y) {
                let mods = crate::editor_window::mods(&ev.modifiers);
                this.model.update(cx, |m, cx| m.editor_input(EditorInput::PointerMove { x: ix, y: iy, mods }, cx));
            }
        }))
        .on_any_mouse_down(cx.listener(|this, ev: &MouseDownEvent, window, cx| {
            let (x, y) = this.to_phys(ev.position, window, cx);
            let Some(pressed) = button(ev.button) else { return };
            // Another button mid-drag keeps the drag's route. A selection drag still hands it to the
            // session, where a right or middle click cancels the gesture. The same button again
            // means its release was lost (outside the overlay), so this press starts afresh.
            let m = this.model.read(cx);
            if m.finishing {
                return;
            }
            if m.pressed_button.is_some_and(|b| b != ev.button) {
                if m.pressed == Some(Route::Session) {
                    this.send(RegionInput::PointerDown { x, y, button: pressed }, cx);
                }
                return;
            }
            let mut scene = this.model.read(cx).pointer_scene((x, y), true);
            scene.frame_only = ev.modifiers.alt;
            let route = capture_pointer::route(&scene, (x, y), pressed);
            // A press the overlay sees is not on a bar: a bar press whose release never came back
            // can't swallow this one's release and leave the drag running.
            this.model.update(cx, |m, _| {
                m.pressed = Some(route);
                m.pressed_button = Some(ev.button);
                m.chrome_press = None;
            });
            match route {
                Route::Text => {
                    if let Some(at) = this.model.read(cx).text_point(x, y) {
                        let (extend, clicks) = (ev.modifiers.shift, ev.click_count);
                        this.model.update(cx, |m, cx| m.text_press(at, extend, clicks, cx));
                    }
                }
                Route::Editor => {
                    if let (Some((ix, iy)), Some(button)) =
                        (this.model.read(cx).canvas_point(x, y), crate::editor_window::button(ev.button))
                    {
                        let mods = crate::editor_window::mods(&ev.modifiers);
                        this.model.update(cx, |m, cx| m.editor_input(EditorInput::PointerDown { x: ix, y: iy, button, mods }, cx));
                    }
                }
                Route::Session => {
                    let alt = ev.modifiers.alt;
                    this.model.update(cx, |m, cx| {
                        m.frame_only = alt;
                        m.let_go_of_mark(cx);
                    });
                    this.send(RegionInput::PointerDown { x, y, button: pressed }, cx);
                }
                Route::Ignore => {}
            }
        }))
        .capture_any_mouse_up(cx.listener(|this, ev: &MouseUpEvent, window, cx| {
            if this.model.update(cx, |m, _| m.chrome_press.take_if(|b| *b == ev.button).is_some()) {
                return;
            }
            if this.model.read(cx).pressed_button.is_some_and(|b| b != ev.button) {
                return;
            }
            let (x, y) = this.to_phys(ev.position, window, cx);
            let route = this.model.update(cx, |m, _| {
                m.pressed_button = None;
                m.pressed.take()
            });
            if ev.button == MouseButton::Left && this.model.update(cx, |m, cx| m.text_release(cx)) {
                return;
            }
            match route {
                Some(Route::Editor) => {
                    if let (Some((ix, iy)), Some(button)) =
                        (this.model.read(cx).canvas_point(x, y), crate::editor_window::button(ev.button))
                    {
                        let mods = crate::editor_window::mods(&ev.modifiers);
                        this.model.update(cx, |m, cx| m.editor_input(EditorInput::PointerUp { x: ix, y: iy, button, mods }, cx));
                    }
                }
                Some(Route::Ignore | Route::Text) => {}
                // A release with no press seen (it began outside the overlay) still ends the gesture.
                Some(Route::Session) | None => {
                    if let Some(button) = button(ev.button) {
                        this.send(RegionInput::PointerUp { x, y, button }, cx);
                    }
                }
            }
            // Space came up during the drag: the drawing tool comes back now it is over.
            this.model.update(cx, |m, cx| m.after_release(cx));
            cx.notify();
        }))
        // Scrolling up grows the hover to the enclosing region, down shrinks it back.
        .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| {
            let notches = match ev.delta {
                ScrollDelta::Lines(d) => d.y,
                ScrollDelta::Pixels(d) => f32::from(d.y) / PIXELS_PER_NOTCH,
            };
            let steps = wheel_steps(&mut this.wheel, notches);
            if steps != 0 {
                this.send(RegionInput::Wheel(steps), cx);
            }
        }))
        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.key(ev, cx)))
        .on_key_up(cx.listener(|this, ev: &KeyUpEvent, _, cx| {
            if ev.keystroke.key == "space" {
                this.model.update(cx, |m, cx| m.return_pointer(cx));
            }
        }))
    }
}

fn contains(r: PhysRect, (x, y): (i32, i32)) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

/// The width of the selection's border, drawn just outside it.
const BORDER: f32 = 2.0;

/// `l` grown by `by` on every side.
fn outset(l: Local, by: f32) -> Local {
    Local { x: l.x - by, y: l.y - by, w: l.w + 2.0 * by, h: l.h + 2.0 * by }
}

impl OverlayView {
    /// The pointer shape at this window's pointer, or at the session's cursor for a window the
    /// pointer is not over (a resize shows its arrow on every monitor).
    fn pointer_shape(&self, model: &OverlayModel) -> Pointer {
        let at = self.pointer.unwrap_or(model.view.cursor);
        capture_pointer::pointer(&model.pointer_scene(at, false), at)
    }

    /// Keys: the text read in place while it shows, typing into a text annotation, the toolbar's
    /// shortcuts, the Esc cascade, then the region session.
    fn key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = ev.keystroke.key.as_str();
        let m = &ev.keystroke.modifiers;
        if self.model.read(cx).finishing {
            return;
        }
        if self.model.read(cx).text.is_some() {
            let ctrl = ctrl(m);
            self.model.update(cx, |model, cx| model.text_key(key, ctrl, cx));
            return;
        }
        let (editing, drawing, marks, shown) = {
            let model = self.model.read(cx);
            let a = model.annot.as_ref();
            (
                a.is_some_and(|a| a.chrome.editing_text),
                model.tool.is_some(),
                a.is_some_and(crate::overlay_tools::Annot::has_marks),
                model.toolbar_shown(),
            )
        };
        let mods = crate::editor_window::mods(m);
        if editing {
            if !ctrl(m)
                && let Some(ch) = ev.keystroke.key_char.clone().filter(|c| !c.is_empty() && !c.chars().any(char::is_control))
            {
                self.model.update(cx, |model, cx| model.editor_input(EditorInput::Text(ch), cx));
                return;
            }
            if let Some(k) = crate::editor_window::editor_key(key) {
                self.model.update(cx, |model, cx| model.editor_input(EditorInput::Key { key: k, mods }, cx));
                return;
            }
        }
        // Mid-press on the canvas only Esc acts, and it drops the stroke or puts the dragged mark
        // back: Enter would finish without it, and putting the tool away would leave it to land on
        // the release.
        if self.model.read(cx).pressed == Some(Route::Editor) {
            if key == "escape" {
                self.model.update(cx, |model, cx| model.editor_input(EditorInput::Key { key: laqta_editor::EditorKey::Escape, mods }, cx));
            }
            return;
        }
        // Holding Space lends the Pointer while a drawing tool is out (repeats keep it lent).
        if key == "space" && !ctrl(m) && (drawing || self.model.read(cx).space.is_lent()) {
            self.model.update(cx, |model, cx| {
                model.borrow_pointer();
                cx.notify();
            });
            return;
        }
        // Number keys pick the drawing tool's colours. They never reach the engine, whose digits
        // would switch its tool behind the toolbar's back.
        if drawing && key.len() == 1 && key.chars().all(|c| c.is_ascii_digit()) && !ctrl(m) && !m.alt {
            self.model.update(cx, |model, cx| {
                if let Some(tool) = model.tool
                    && let Some(color) = crate::presenters::capture_toolbar::color_key(key, tool)
                {
                    model.restyle(|st, t| crate::presenters::capture_toolbar::with_color(st, t, color), cx);
                }
            });
            return;
        }
        if shown
            && let Some(item) = crate::presenters::capture_toolbar::key_item(
                key,
                crate::presenters::capture_toolbar::Mods { ctrl: ctrl(m), shift: m.shift, alt: m.alt },
            )
        {
            self.model.update(cx, |model, cx| model.pick(item, cx));
            return;
        }
        match key {
            "escape" => {
                self.model.update(cx, |model, cx| model.escape(cx));
                return;
            }
            "enter" if marks => {
                self.model.update(cx, |model, cx| model.finish(laqta_domain::region::SelectionAction::Default, cx));
                return;
            }
            _ => {}
        }
        // A picked mark takes the arrows and Delete under any tool; the selection under it does not.
        // With nothing picked they nudge the frame, whatever tool is out, and other keys never
        // reach the engine, whose letters would switch its tool behind the toolbar's back.
        // Only mark edits: a bare letter is an engine tool shortcut, which would switch its tool
        // behind the toolbar's back; Ctrl letters are commands (copy, duplicate).
        let mark = self.model.read(cx).mark_selected();
        let edit = |k: &laqta_editor::EditorKey| {
            use laqta_editor::EditorKey as K;
            matches!(k, K::Left | K::Right | K::Up | K::Down | K::Delete | K::Backspace) || (matches!(k, K::Char(_)) && mods.ctrl)
        };
        if mark && let Some(k) = crate::editor_window::editor_key(key).filter(edit) {
            self.model.update(cx, |model, cx| model.editor_input(EditorInput::Key { key: k, mods }, cx));
            return;
        }
        // Delete would remove the selection, and every mark on it with no way back.
        if marks && matches!(key, "delete" | "backspace") {
            return;
        }
        if let Some(key) = region_key(key) {
            // A nudge carries the marks: an Alt press that never moved must not leave its
            // frame-only mode behind for it.
            self.model.update(cx, |m, _| m.frame_only = false);
            self.send(RegionInput::Key { key, mods: KeyMods { shift: m.shift, ctrl: ctrl(m), alt: m.alt } }, cx);
        }
    }
}

const CELL: f32 = 8.0;
/// The magnifier card's width.
const CARD_W: f32 = 168.0;

/// The magnifier and the pixel info as one card beside the cursor: a zoomed grid with faint
/// pixel lines and the centre pixel marked, then the coordinates and colour with a swatch.
fn magnifier_and_info(view: &RegionView, mon: &MonitorImage, lx: f32, ly: f32, win_w: f32, win_h: f32, scale: f32) -> Div {
    let _ = scale;
    let mut card = div()
        .flex()
        .flex_col()
        .rounded(crate::theme::radius::MD)
        .overflow_hidden()
        .bg(color(0x1c1c1f, 0.96))
        .border_1()
        .border_color(color(0xffffff, 0.14))
        .shadow_lg();
    let mut width: f32 = 0.0;
    let mut height: f32 = 0.0;
    let mut centre = 0u32;
    if let Some(m) = view.magnifier {
        let n_w = m.width.min(31);
        let n_h = m.height.min(31);
        // Cells stretch so the grid fills the card's width, whatever the pixel count.
        let cell = (CARD_W / n_w.max(1) as f32).max(CELL);
        let (gw, gh) = (n_w as f32 * cell, n_h as f32 * cell);
        let mut grid = div().relative().w(px(gw)).h(px(gh));
        for row in 0..n_h {
            for col in 0..n_w {
                let c = sample(&mon.bitmap, m.x + col as i32 - mon.rect.x, m.y + row as i32 - mon.rect.y);
                if row == n_h / 2 && col == n_w / 2 {
                    centre = c;
                }
                grid = grid.child(
                    div().absolute().left(px(col as f32 * cell)).top(px(row as f32 * cell)).w(px(cell)).h(px(cell)).bg(gpui::rgb(c)),
                );
            }
        }
        // Faint pixel lines through the centre row and column, then the centre pixel's frame.
        let (mx, my) = ((n_w / 2) as f32 * cell, (n_h / 2) as f32 * cell);
        let guide = color(0x3d8bfd, 0.25);
        grid = grid
            .child(div().absolute().left_0().top(px(my)).w(px(gw)).h(px(cell)).bg(guide))
            .child(div().absolute().left(px(mx)).top_0().w(px(cell)).h(px(gh)).bg(guide))
            .child(div().absolute().left(px(mx - 1.0)).top(px(my - 1.0)).size(px(cell + 2.0)).border_1().border_color(gpui::white()))
            .child(div().absolute().left(px(mx)).top(px(my)).size(px(cell)).border_1().border_color(gpui::black()));
        width = gw;
        height = gh;
        card = card.child(grid);
    }
    if let Some(info) = &view.info_text {
        let mut text = div().flex().flex_col().gap(px(1.)).px(px(8.)).py(px(6.)).text_color(color(0xffffff, 0.92)).text_size(px(11.5));
        for line in info.lines() {
            text = text.child(div().child(line.to_string()));
        }
        let row = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .when(view.magnifier.is_some(), |d| {
                d.pl(px(8.)).child(
                    div().size(px(14.)).flex_none().rounded(px(4.)).border_1().border_color(color(0xffffff, 0.3)).bg(gpui::rgb(centre)),
                )
            })
            .child(text);
        height += 16.0 * (info.lines().count() as f32) + 14.0;
        width = width.max(CARD_W);
        card = card.w(px(width)).child(row);
    }
    let gap = 22.0;
    let x = if lx + gap + width > win_w { lx - gap - width } else { lx + gap };
    let y = if ly + gap + height > win_h { ly - gap - height } else { ly + gap };
    div().absolute().left(px(x.max(4.0))).top(px(y.max(4.0))).child(card)
}

/// 0xRRGGBB of a pixel; black outside the bitmap.
fn sample(b: &Bitmap, x: i32, y: i32) -> u32 {
    if x < 0 || y < 0 || x as u32 >= b.width() || y as u32 >= b.height() {
        return 0;
    }
    let i = ((y as u32 * b.width() + x as u32) * 4) as usize;
    let p = &b.pixels()[i..i + 4];
    let (r, g, bl) = match b.format() {
        PixelFormat::Rgba8 => (p[0], p[1], p[2]),
        PixelFormat::Bgra8 => (p[2], p[1], p[0]),
    };
    (r as u32) << 16 | (g as u32) << 8 | bl as u32
}
