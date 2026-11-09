//! AT-SPI UI-element hover targets: the visible elements of the focused window, so the
//! region overlay can snap to a button or a panel as well as to a whole window. Coordinates are
//! AT-SPI screen extents, used as they come. Wayland sessions get no elements: there the extents
//! do not line up with the physical pixels the overlay uses.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use atspi::proxy::accessible::{AccessibleProxy, ObjectRefExt};
use atspi::proxy::proxy_ext::ProxyExt;
use atspi::{AccessibilityConnection, CoordType, ObjectRefOwned, State, StateSet};
use futures_lite::future;
use laqta_domain::PhysRect;
use laqta_domain::region::{HoverTarget, TargetKind};
use laqta_ports::{BoxFuture, HoverSource, PortError};

/// How long the element walk may run, connection included. Whatever it found by then is kept.
const BUDGET: Duration = Duration::from_millis(300);
/// How many levels below the focused window the walk descends.
const MAX_DEPTH: usize = 12;
/// How many elements one query keeps.
const MAX_ELEMENTS: usize = 400;

/// `x`, `y`, `width`, `height` in screen coordinates, as `GetExtents(Screen)` returns them.
type Extents = (i32, i32, i32, i32);

/// Puts the focused application's elements in front of another source's windows.
pub struct WithElements {
    inner: Arc<dyn HoverSource>,
}

impl WithElements {
    pub fn new(inner: Arc<dyn HoverSource>) -> Self {
        Self { inner }
    }
}

impl fmt::Debug for WithElements {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WithElements").finish_non_exhaustive()
    }
}

impl HoverSource for WithElements {
    fn targets(&self) -> BoxFuture<'_, Result<Vec<HoverTarget>, PortError>> {
        Box::pin(async {
            let elements = elements().await;
            combine(elements, self.inner.targets().await)
        })
    }

    fn active_window(&self) -> BoxFuture<'_, Result<Option<HoverTarget>, PortError>> {
        self.inner.active_window()
    }
}

/// The elements first, then the inner source's windows. An inner failure surfaces only when there
/// are no elements to show instead.
fn combine(elements: Vec<HoverTarget>, windows: Result<Vec<HoverTarget>, PortError>) -> Result<Vec<HoverTarget>, PortError> {
    match windows {
        Ok(windows) => Ok(elements.into_iter().chain(windows).collect()),
        Err(_) if !elements.is_empty() => Ok(elements),
        Err(e) => Err(e),
    }
}

/// The focused window's elements, smallest first. The overlay takes the first target under the
/// cursor, so a button wins over the panel around it. Empty on Wayland, without an accessibility
/// bus, or when no window is active.
async fn elements() -> Vec<HoverTarget> {
    if !enabled(std::env::var_os("WAYLAND_DISPLAY").as_deref()) {
        return Vec::new();
    }
    let mut found = blocking::unblock(|| async_io::block_on(within_budget())).await;
    smallest_first(&mut found);
    found
}

/// Runs the walk for at most `BUDGET`. The walk writes into `found` as it goes, so the timeout
/// keeps whatever was reached.
async fn within_budget() -> Vec<HoverTarget> {
    let mut found = Vec::new();
    let timer = async {
        async_io::Timer::after(BUDGET).await;
    };
    future::or(walk(&mut found), timer).await;
    found
}

/// Breadth-first over the focused window's descendants, recording each shown element with a
/// screen extent. Any failure ends the walk early and keeps what is already in `found`.
async fn walk(found: &mut Vec<HoverTarget>) {
    let Ok(bus) = AccessibilityConnection::new().await else {
        return;
    };
    let Some((window, process)) = focused_window(&bus).await else {
        return;
    };
    let Ok(window_proxy) = window.as_accessible_proxy(bus.connection()).await else {
        return;
    };
    let Ok(children) = window_proxy.get_children().await else {
        return;
    };
    let mut queue: VecDeque<(ObjectRefOwned, usize)> = children.into_iter().map(|child| (child, 1)).collect();
    while let Some((object, depth)) = queue.pop_front() {
        if !room(found.len()) {
            break;
        }
        let Ok(proxy) = object.as_accessible_proxy(bus.connection()).await else {
            continue;
        };
        if let Ok(states) = proxy.get_state().await
            && shown(states)
            && let Some(extents) = screen_extents(&proxy).await
            && keep(extents, states)
        {
            found.push(target(&proxy, extents, &process).await);
        }
        if may_descend(depth)
            && let Ok(children) = proxy.get_children().await
        {
            queue.extend(children.into_iter().map(|child| (child, depth + 1)));
        }
    }
}

/// The window in the Active state, and the name of the application it belongs to. Failing objects
/// are skipped.
async fn focused_window(bus: &AccessibilityConnection) -> Option<(ObjectRefOwned, String)> {
    let desktop = bus.root_accessible_on_registry().await.ok()?;
    for app in desktop.get_children().await.ok()? {
        let Ok(app_proxy) = app.as_accessible_proxy(bus.connection()).await else {
            continue;
        };
        let Ok(windows) = app_proxy.get_children().await else {
            continue;
        };
        for window in windows {
            let Ok(window_proxy) = window.as_accessible_proxy(bus.connection()).await else {
                continue;
            };
            if window_proxy.get_state().await.is_ok_and(|states| states.contains(State::Active)) {
                let process = app_proxy.name().await.unwrap_or_default();
                return Some((window.clone(), process));
            }
        }
    }
    None
}

/// The element's screen extent. `None` when it does not implement Component.
async fn screen_extents(proxy: &AccessibleProxy<'_>) -> Option<Extents> {
    let component = proxy.proxies().await.ok()?.component().await.ok()?;
    component.get_extents(CoordType::Screen).await.ok()
}

/// The hover target for a kept element. Its title is the accessible name, or the role name when the
/// element has none.
async fn target(proxy: &AccessibleProxy<'_>, extents: Extents, process: &str) -> HoverTarget {
    let name = proxy.name().await.unwrap_or_default();
    let title = if name.is_empty() { proxy.get_role_name().await.unwrap_or_default() } else { name };
    HoverTarget { rect: phys_rect(extents), handle: 0, title, process: process.to_owned(), kind: TargetKind::Element }
}

/// Wayland sessions skip elements. A set `WAYLAND_DISPLAY` means Wayland.
fn enabled(wayland_display: Option<&OsStr>) -> bool {
    wayland_display.is_none_or(OsStr::is_empty)
}

/// Whether the element is drawn on screen.
fn shown(states: StateSet) -> bool {
    states.contains(State::Visible | State::Showing)
}

/// Whether an element joins the list: shown, with a non-empty extent.
fn keep(extents: Extents, states: StateSet) -> bool {
    let (_, _, width, height) = extents;
    shown(states) && width > 0 && height > 0
}

/// Whether the list can take one more element.
fn room(kept: usize) -> bool {
    kept < MAX_ELEMENTS
}

/// Whether the walk goes below an element at `depth`.
fn may_descend(depth: usize) -> bool {
    depth < MAX_DEPTH
}

/// An extent as a physical rect. `keep` only lets through non-empty extents, so the clamp only
/// guards against a negative size.
fn phys_rect((x, y, width, height): Extents) -> PhysRect {
    PhysRect { x, y, width: width.max(0) as u32, height: height.max(0) as u32 }
}

/// Sorts by area, smallest first. Equal areas keep the walk's order.
fn smallest_first(targets: &mut [HoverTarget]) {
    targets.sort_by_key(|t| u64::from(t.rect.width) * u64::from(t.rect.height));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(rect: PhysRect) -> HoverTarget {
        HoverTarget { rect, handle: 0, title: String::new(), process: String::new(), kind: TargetKind::Element }
    }

    fn rect(width: u32, height: u32) -> PhysRect {
        PhysRect { x: 0, y: 0, width, height }
    }

    fn shown_states() -> StateSet {
        StateSet::new(State::Visible | State::Showing)
    }

    #[test]
    fn wayland_skips_elements() {
        assert!(enabled(None));
        assert!(enabled(Some(OsStr::new(""))));
        assert!(!enabled(Some(OsStr::new("wayland-1"))));
    }

    #[test]
    fn keeps_only_shown_elements_with_extent() {
        assert!(keep((10, 20, 30, 40), shown_states()));
        assert!(!keep((10, 20, 0, 40), shown_states()));
        assert!(!keep((10, 20, 30, -1), shown_states()));
        assert!(!keep((10, 20, 30, 40), StateSet::from(State::Visible)));
        assert!(!keep((10, 20, 30, 40), StateSet::from(State::Showing)));
        assert!(!keep((10, 20, 30, 40), StateSet::empty()));
    }

    #[test]
    fn cap_and_depth_bounds_are_exclusive() {
        assert!(room(MAX_ELEMENTS - 1));
        assert!(!room(MAX_ELEMENTS));
        assert!(may_descend(MAX_DEPTH - 1));
        assert!(!may_descend(MAX_DEPTH));
    }

    #[test]
    fn extent_becomes_physical_rect() {
        assert_eq!(phys_rect((-5, 7, 30, 40)), PhysRect { x: -5, y: 7, width: 30, height: 40 });
        assert_eq!(phys_rect((0, 0, -3, 0)), PhysRect { x: 0, y: 0, width: 0, height: 0 });
    }

    #[test]
    fn smallest_element_comes_first() {
        let mut found = vec![target(rect(100, 100)), target(rect(10, 10)), target(rect(50, 4))];
        smallest_first(&mut found);
        let areas: Vec<_> = found.iter().map(|t| t.rect.width * t.rect.height).collect();
        assert_eq!(areas, [100, 200, 10_000]);
    }

    #[test]
    fn elements_sit_in_front_of_windows() {
        let element = target(rect(10, 10));
        let window = target(rect(800, 600));
        let out = combine(vec![element.clone()], Ok(vec![window.clone()])).expect("windows are Ok");
        assert_eq!(out, vec![element, window]);
    }

    #[test]
    fn inner_failure_is_hidden_only_when_elements_exist() {
        let failure = || PortError::Failed("sway ipc".into());
        let element = target(rect(10, 10));
        assert!(matches!(combine(Vec::new(), Err(failure())), Err(PortError::Failed(_))));
        assert_eq!(combine(vec![element.clone()], Err(failure())).expect("elements win"), vec![element]);
    }

    struct Frontmost;

    impl HoverSource for Frontmost {
        fn targets(&self) -> BoxFuture<'_, Result<Vec<HoverTarget>, PortError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn active_window(&self) -> BoxFuture<'_, Result<Option<HoverTarget>, PortError>> {
            Box::pin(async { Ok(Some(target(rect(640, 480)))) })
        }
    }

    #[test]
    fn active_window_comes_from_the_inner_source() {
        let source = WithElements::new(Arc::new(Frontmost));
        let active = future::block_on(source.active_window()).expect("delegated Ok");
        assert_eq!(active, Some(target(rect(640, 480))));
    }
}
