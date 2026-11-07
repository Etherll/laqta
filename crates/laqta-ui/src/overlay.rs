//! The region overlay (`Overlay` port, ADR 012, M1 D8): one borderless fullscreen window per
//! monitor showing the frozen frame. It forwards pointer and keyboard input in physical desktop
//! pixels and paints the Region session's `RegionView`. It computes nothing itself.

use std::sync::Arc;

use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Div, Entity, FocusHandle, Hsla, InteractiveElement as _, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render, RenderImage, Styled as _,
    Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, img, point, px, size,
};
use laqta_domain::geometry::PhysRect;
use laqta_domain::region::{Button, KeyMods, RegionInput, RegionKey};
use laqta_domain::{Bitmap, Frame, PixelFormat};
use laqta_ports::{InputCallback, RegionView};
use smallvec::SmallVec;

use crate::runtime::UiState;

#[derive(Default)]
pub(crate) struct OverlayWindows {
    model: Option<Entity<OverlayModel>>,
    windows: Vec<AnyWindowHandle>,
}

struct MonitorImage {
    rect: PhysRect,
    image: Arc<RenderImage>,
    bitmap: Bitmap,
}

pub(crate) struct OverlayModel {
    monitors: Vec<MonitorImage>,
    view: RegionView,
    on_input: InputCallback,
}

/// GPUI wants BGRA. Frames are BGRA (Windows, X11) or RGBA (Wayland).
fn render_image(bitmap: &Bitmap) -> Option<Arc<RenderImage>> {
    let mut bytes = bitmap.pixels().to_vec();
    if bitmap.format() == PixelFormat::Rgba8 {
        for px in bytes.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
    }
    let buffer = image::RgbaImage::from_raw(bitmap.width(), bitmap.height(), bytes)?;
    let frames: SmallVec<[image::Frame; 1]> = SmallVec::from_elem(image::Frame::new(buffer), 1);
    Some(Arc::new(RenderImage::new(frames)))
}

/// Pairs each frame with a display: both sorted by position (left to right, then top to bottom).
fn match_displays(frames: &[Frame], cx: &App) -> Vec<Option<Bounds<Pixels>>> {
    let mut displays: Vec<_> = cx.displays().into_iter().map(|d| (d.bounds(), d.id())).collect();
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

fn is_wayland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

pub(crate) fn show(frames: Vec<Frame>, view: RegionView, on_input: InputCallback, cx: &mut App) {
    close(cx);
    let display_bounds = match_displays(&frames, cx);
    let monitors: Vec<MonitorImage> = frames
        .iter()
        .filter_map(|f| Some(MonitorImage { rect: f.monitor.rect, image: render_image(&f.image)?, bitmap: f.image.clone() }))
        .collect();
    let count = monitors.len();
    let model = cx.new(|_| OverlayModel { monitors, view, on_input });
    let mut windows = Vec::new();
    for index in 0..count {
        let bounds = display_bounds.get(index).copied().flatten();
        match open_window(model.clone(), index, bounds, cx) {
            Ok(w) => windows.push(w),
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
    cx: &mut App,
) -> anyhow_like::Result<AnyWindowHandle> {
    let bounds = bounds.unwrap_or_else(|| {
        let r = model.read(cx).monitors[index].rect;
        Bounds { origin: point(px(r.x as f32), px(r.y as f32)), size: size(px(r.width as f32), px(r.height as f32)) }
    });
    let kind = overlay_kind();
    let options = WindowOptions {
        titlebar: None,
        window_bounds: Some(if matches!(kind, WindowKind::PopUp) {
            WindowBounds::Windowed(bounds)
        } else {
            WindowBounds::Fullscreen(bounds)
        }),
        focus: true,
        show: true,
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
            let view = cx.new(|cx| OverlayView::new(model, index, cx));
            let focus = view.read(cx).focus.clone();
            window.focus(&focus, cx);
            view
        })
        .map_err(|e| anyhow_like::Error(format!("{e:#}")))?;
    Ok(handle.into())
}

#[cfg(target_os = "linux")]
fn overlay_kind() -> WindowKind {
    use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
    if is_wayland() && std::env::var_os("LAQTA_NO_LAYER_SHELL").is_none() {
        WindowKind::LayerShell(LayerShellOptions {
            namespace: "laqta-overlay".into(),
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

#[cfg(not(target_os = "linux"))]
fn overlay_kind() -> WindowKind {
    WindowKind::PopUp
}

pub(crate) fn update(view: RegionView, cx: &mut App) {
    let model = cx.global::<UiState>().overlay.model.clone();
    if let Some(model) = model {
        model.update(cx, |m, cx| {
            m.view = view;
            cx.notify();
        });
    }
}

pub(crate) fn close(cx: &mut App) {
    if !cx.has_global::<UiState>() {
        return;
    }
    let windows = std::mem::take(&mut cx.global_mut::<UiState>().overlay);
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
}

const SELECTION: u32 = 0x3d8bfd;
const HOVER: u32 = 0x8ab4f8;

fn color(rgb: u32, alpha: f32) -> Hsla {
    let mut c: Hsla = gpui::rgb(rgb).into();
    c.a = alpha;
    c
}

impl OverlayView {
    fn new(model: Entity<OverlayModel>, index: usize, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        OverlayView { model, index, focus: cx.focus_handle() }
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
        let dim = color(0x000000, view.dim_alpha as f32 / 255.0);

        let mut root = div()
            .id("laqta-overlay")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .cursor_crosshair()
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

        if view.drawing.is_none()
            && let Some(t) = &view.hovered
        {
            root = root.child(boxed(&local(t.rect)).border_2().border_color(color(HOVER, 1.0)).bg(color(HOVER, 0.12)));
        }
        for (i, r) in view.regions.iter().enumerate() {
            let active = Some(i) == view.active;
            root = root.child(boxed(&local(*r)).border_2().border_color(color(SELECTION, if active { 1.0 } else { 0.7 })));
        }
        if let Some(d) = view.drawing {
            root = root.child(boxed(&local(d)).border_2().border_color(color(SELECTION, 1.0)));
        }
        for (_, h) in &view.handles {
            root = root.child(boxed(&local(*h)).bg(gpui::white()).border_1().border_color(color(SELECTION, 1.0)));
        }

        let (cx_phys, cy_phys) = view.cursor;
        let cursor_here = cx_phys >= origin.x && cx_phys < origin.right() && cy_phys >= origin.y && cy_phys < origin.bottom();
        if cursor_here {
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

        root.on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
            let (x, y) = this.to_phys(ev.position, window, cx);
            this.send(RegionInput::PointerMove { x, y }, cx);
        }))
        .on_any_mouse_down(cx.listener(|this, ev: &MouseDownEvent, window, cx| {
            if let Some(button) = button(ev.button) {
                let (x, y) = this.to_phys(ev.position, window, cx);
                this.send(RegionInput::PointerDown { x, y, button }, cx);
            }
        }))
        .capture_any_mouse_up(cx.listener(|this, ev: &MouseUpEvent, window, cx| {
            if let Some(button) = button(ev.button) {
                let (x, y) = this.to_phys(ev.position, window, cx);
                this.send(RegionInput::PointerUp { x, y, button }, cx);
            }
        }))
        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
            if let Some(key) = region_key(ev.keystroke.key.as_str()) {
                let m = &ev.keystroke.modifiers;
                this.send(RegionInput::Key { key, mods: KeyMods { shift: m.shift, ctrl: m.control, alt: m.alt } }, cx);
            }
        }))
    }
}

const CELL: f32 = 8.0;

/// The magnifier grid (one quad per sampled pixel) and the info text, placed beside the cursor.
fn magnifier_and_info(view: &RegionView, mon: &MonitorImage, lx: f32, ly: f32, win_w: f32, win_h: f32, scale: f32) -> Div {
    let mut panel = div().flex().flex_col().gap(px(4.));
    let mut width = 0.0;
    let mut height = 0.0;
    if let Some(m) = view.magnifier {
        let n_w = m.width.min(31);
        let n_h = m.height.min(31);
        let mut grid = div().relative().w(px(n_w as f32 * CELL)).h(px(n_h as f32 * CELL)).border_1().border_color(gpui::white());
        for row in 0..n_h {
            for col in 0..n_w {
                let px_x = m.x + col as i32 - mon.rect.x;
                let px_y = m.y + row as i32 - mon.rect.y;
                let c = sample(&mon.bitmap, px_x, px_y);
                grid = grid.child(
                    div().absolute().left(px(col as f32 * CELL)).top(px(row as f32 * CELL)).w(px(CELL)).h(px(CELL)).bg(gpui::rgb(c)),
                );
            }
        }
        // Centre marker.
        grid = grid.child(
            div()
                .absolute()
                .left(px((n_w / 2) as f32 * CELL))
                .top(px((n_h / 2) as f32 * CELL))
                .w(px(CELL))
                .h(px(CELL))
                .border_1()
                .border_color(gpui::black()),
        );
        width = n_w as f32 * CELL;
        height = n_h as f32 * CELL;
        panel = panel.child(grid);
    }
    if let Some(info) = &view.info_text {
        height += 16.0 * (info.lines().count() as f32) + 8.0;
        width = f32::max(width, 160.0);
        let mut text = div().px(px(6.)).py(px(4.)).rounded(px(4.)).bg(color(0x000000, 0.75)).text_color(gpui::white()).text_xs();
        for line in info.lines() {
            text = text.child(div().child(line.to_string()));
        }
        panel = panel.child(text);
    }
    let _ = scale;
    let gap = 20.0;
    let x = if lx + gap + width > win_w { lx - gap - width } else { lx + gap };
    let y = if ly + gap + height > win_h { ly - gap - height } else { ly + gap };
    div().absolute().left(px(x.max(0.0))).top(px(y.max(0.0))).child(panel)
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
