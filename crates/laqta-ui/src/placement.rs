//! GPUI's display list as [monitor_placement] [Screen]s, so window code asks the presenter which
//! monitor a window goes on and never falls back to the primary display by itself.

use gpui::{App, DisplayId};
use laqta_domain::geometry::PhysRect;

use crate::presenters::monitor_placement::{Rect, Screen};

/// Every display GPUI knows. GPUI reports no scale, so a display takes the scale of the monitor in
/// `monitors` (physical rect and scale) whose origin divided by its scale is the display's
/// origin, as Windows and X11 lay them out; 1 where none is.
pub(crate) fn screens(cx: &App, monitors: &[(PhysRect, f32)]) -> Vec<Screen> {
    cx.displays()
        .into_iter()
        .map(|d| {
            let bounds = rect(d.bounds());
            let scale = monitors
                .iter()
                .find(|(r, s)| {
                    let s = s.max(0.1);
                    (r.x as f32 / s - bounds.x).abs() < 1.0 && (r.y as f32 / s - bounds.y).abs() < 1.0
                })
                .map_or(1.0, |(_, s)| *s);
            Screen { id: u64::from(d.id()), bounds, work: rect(d.visible_bounds()), scale }
        })
        .collect()
}

/// The display holding the centre of these (logical) bounds.
pub(crate) fn display_at(cx: &App, b: gpui::Bounds<gpui::Pixels>) -> Option<std::rc::Rc<dyn gpui::PlatformDisplay>> {
    let c = b.center();
    cx.displays().into_iter().find(|d| d.bounds().contains(&c))
}

/// The primary display, or the first one where the platform names none (Wayland).
pub(crate) fn main_display(cx: &App) -> Option<std::rc::Rc<dyn gpui::PlatformDisplay>> {
    cx.primary_display().or_else(|| cx.displays().into_iter().next())
}

/// The display with this id, as GPUI's id type.
pub(crate) fn display_id(id: u64) -> DisplayId {
    DisplayId::from(id)
}

fn rect(b: gpui::Bounds<gpui::Pixels>) -> Rect {
    Rect { x: f32::from(b.origin.x), y: f32::from(b.origin.y), w: f32::from(b.size.width), h: f32::from(b.size.height) }
}

/// A presenter rect as GPUI bounds.
pub(crate) fn bounds(r: Rect) -> gpui::Bounds<gpui::Pixels> {
    gpui::Bounds { origin: gpui::point(gpui::px(r.x), gpui::px(r.y)), size: gpui::size(gpui::px(r.w), gpui::px(r.h)) }
}
