//! Hover targets (`HoverSource`): the on-screen windows front to back from the window server, and,
//! when Accessibility is granted, the UI elements of the frontmost window in front of them.
//!
//! Window rects, owner names and ids need no permission. Window titles come back empty until
//! Screen Recording is granted; that is not an error, and this module never asks for it.

use std::sync::atomic::{AtomicBool, Ordering};

use laqta_domain::region::{HoverTarget, TargetKind};
use laqta_ports::{BoxFuture, HoverSource, PortError};
use objc2_core_foundation::{CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID, kCGWindowAlpha,
    kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer, kCGWindowMemoryUsage, kCGWindowName, kCGWindowNumber, kCGWindowOwnerName,
    kCGWindowOwnerPID,
};

use crate::ax::{self, Frame};
use crate::displays::{self, Display, to_phys};
use crate::permissions;

/// Whether Laqta's own windows are snap targets, set through `HoverSource::set_include_own_windows`.
/// A process-wide flag, so `MacHover` stays a unit struct.
static INCLUDE_OWN_WINDOWS: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Default)]
pub struct MacHover;

impl HoverSource for MacHover {
    fn targets(&self) -> BoxFuture<'_, Result<Vec<HoverTarget>, PortError>> {
        Box::pin(blocking::unblock(current_targets))
    }

    fn active_window(&self) -> BoxFuture<'_, Result<Option<HoverTarget>, PortError>> {
        Box::pin(blocking::unblock(front_target))
    }

    fn set_include_own_windows(&self, include: bool) {
        INCLUDE_OWN_WINDOWS.store(include, Ordering::Relaxed);
    }
}

/// Every snap candidate front to back. The frontmost window's elements go first, since they are in
/// front of it; the Region session sorts elements by size within their window. No monitor targets:
/// the Windows and Linux sources add none either.
fn current_targets() -> Result<Vec<HoverTarget>, PortError> {
    let windows = snap_windows()?;
    let displays = displays::list();
    let mut targets: Vec<HoverTarget> = windows.iter().map(|window| window_target(window, &displays)).collect();
    // Accessibility never prompts here: without it the list is windows only.
    if let Some(front) = windows.first()
        && permissions::accessibility().is_ok()
    {
        let elements = element_targets(front, ax::element_frames(front.pid, front.frame), &displays);
        targets.splice(0..0, elements);
    }
    Ok(targets)
}

/// The frontmost snap candidate, or `None` when no window is up.
fn front_target() -> Result<Option<HoverTarget>, PortError> {
    let windows = snap_windows()?;
    let displays = displays::list();
    Ok(windows.first().map(|window| window_target(window, &displays)))
}

/// One window as the window server describes it, in points.
#[derive(Clone, Debug)]
pub(crate) struct WindowInfo {
    pub id: u32,
    pub pid: i32,
    pub owner: String,
    /// Empty without Screen Recording.
    pub name: String,
    pub layer: i32,
    pub alpha: f64,
    pub frame: Frame,
    pub on_screen: bool,
    pub memory: Option<i64>,
}

/// Whether a window is one the region overlay should snap to: an ordinary window (layer 0, which
/// leaves out the menu bar, the Dock and overlays) that is drawn, not empty, and not this process's
/// (`own_pid`; pass -1 to keep them).
fn is_snap_candidate(window: &WindowInfo, own_pid: i32) -> bool {
    window.layer == 0 && window.alpha > 0.0 && window.frame.width > 0.0 && window.frame.height > 0.0 && window.pid != own_pid
}

/// The on-screen snap candidates, front to back.
fn snap_windows() -> Result<Vec<WindowInfo>, PortError> {
    let option = CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let all = window_list(option, kCGNullWindowID).ok_or_else(|| PortError::Failed("the window server returned no window list".into()))?;
    // No window has pid -1, so including Laqta's windows keeps all of them. Overlays and toasts sit
    // above layer 0 and are never candidates either way.
    let own = if INCLUDE_OWN_WINDOWS.load(Ordering::Relaxed) { -1 } else { i32::try_from(std::process::id()).unwrap_or(-1) };
    Ok(all.into_iter().filter(|window| is_snap_candidate(window, own)).collect())
}

fn window_target(window: &WindowInfo, displays: &[Display]) -> HoverTarget {
    HoverTarget {
        rect: to_phys(displays, window.frame.x, window.frame.y, window.frame.width, window.frame.height),
        handle: u64::from(window.id),
        title: window.name.clone(),
        process: window.owner.clone(),
        kind: TargetKind::Window,
    }
}

/// Element targets for `frames` (in pre-order) inside `window`. Reversed, so a child comes before
/// its parent and a later sibling before an earlier one; the overlay takes the first target under
/// the pointer. Elements that are empty in pixels are dropped.
fn element_targets(window: &WindowInfo, frames: Vec<Frame>, displays: &[Display]) -> Vec<HoverTarget> {
    frames
        .into_iter()
        .rev()
        .map(|frame| to_phys(displays, frame.x, frame.y, frame.width, frame.height))
        .filter(|rect| rect.width > 0 && rect.height > 0)
        .map(|rect| HoverTarget {
            rect,
            handle: u64::from(window.id),
            title: String::new(),
            process: window.owner.clone(),
            kind: TargetKind::Element,
        })
        .collect()
}

type WindowDict = CFDictionary<CFString, CFType>;

/// `CGWindowListCopyWindowInfo` for `option`, in the window server's order (front to back).
/// `None` when the call fails.
pub(crate) fn window_list(option: CGWindowListOption, relative_to: u32) -> Option<Vec<WindowInfo>> {
    let array = CGWindowListCopyWindowInfo(option, relative_to)?;
    // SAFETY: the call returns an array of dictionaries keyed by CFStrings (the kCGWindow* keys).
    let array = unsafe { CFRetained::cast_unchecked::<CFArray<WindowDict>>(array) };
    Some(array.iter().filter_map(|dict| read_window(&dict)).collect())
}

/// A window from its info dictionary. `None` when a field that every window has is missing.
fn read_window(dict: &WindowDict) -> Option<WindowInfo> {
    // SAFETY: the kCGWindow* keys are immutable CFString constants exported by CoreGraphics.
    let (number, pid, owner, name, layer, alpha, bounds, onscreen, memory) = unsafe {
        (
            kCGWindowNumber,
            kCGWindowOwnerPID,
            kCGWindowOwnerName,
            kCGWindowName,
            kCGWindowLayer,
            kCGWindowAlpha,
            kCGWindowBounds,
            kCGWindowIsOnscreen,
            kCGWindowMemoryUsage,
        )
    };
    Some(WindowInfo {
        id: u32::try_from(integer(dict, number)?).ok()?,
        pid: i32::try_from(integer(dict, pid)?).ok()?,
        owner: string(dict, owner).unwrap_or_default(),
        name: string(dict, name).unwrap_or_default(),
        layer: i32::try_from(integer(dict, layer)?).ok()?,
        alpha: float(dict, alpha).unwrap_or(1.0),
        frame: frame(dict, bounds)?,
        // The key is missing from windows that are not on screen.
        on_screen: boolean(dict, onscreen).unwrap_or(false),
        memory: integer(dict, memory),
    })
}

fn integer(dict: &WindowDict, key: &CFString) -> Option<i64> {
    dict.get(key)?.downcast::<CFNumber>().ok()?.as_i64()
}

fn float(dict: &WindowDict, key: &CFString) -> Option<f64> {
    let number = dict.get(key)?.downcast::<CFNumber>().ok()?;
    number.as_f64().or_else(|| number.as_f32().map(f64::from))
}

fn boolean(dict: &WindowDict, key: &CFString) -> Option<bool> {
    Some(dict.get(key)?.downcast::<CFBoolean>().ok()?.as_bool())
}

fn string(dict: &WindowDict, key: &CFString) -> Option<String> {
    Some(dict.get(key)?.downcast::<CFString>().ok()?.to_string())
}

/// The window bounds dictionary (`X`, `Y`, `Width`, `Height`) as a frame.
fn frame(dict: &WindowDict, key: &CFString) -> Option<Frame> {
    let bounds = dict.get(key)?.downcast::<CFDictionary>().ok()?;
    let mut rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0));
    // SAFETY: `rect` is a valid out-pointer and `bounds` is a live CFDictionary.
    let read = unsafe { CGRectMakeWithDictionaryRepresentation(Some(&*bounds), &raw mut rect) };
    read.then_some(Frame { x: rect.origin.x, y: rect.origin.y, width: rect.size.width, height: rect.size.height })
}

#[cfg(test)]
mod tests {
    use laqta_domain::geometry::PhysRect;

    use super::*;

    fn window(layer: i32, alpha: f64, width: f64, height: f64, pid: i32) -> WindowInfo {
        WindowInfo {
            id: 42,
            pid,
            owner: "Notes".into(),
            name: "Groceries".into(),
            layer,
            alpha,
            frame: Frame { x: 100.0, y: 50.0, width, height },
            on_screen: true,
            memory: None,
        }
    }

    fn retina() -> Display {
        Display {
            id: 1,
            name: "Built-in".into(),
            x: 0.0,
            y: 0.0,
            width: 1512.0,
            height: 982.0,
            scale: 2.0,
            layout_scale: 2.0,
            primary: true,
        }
    }

    #[test]
    fn ordinary_windows_of_other_processes_are_candidates() {
        assert!(is_snap_candidate(&window(0, 1.0, 300.0, 200.0, 10), 99));
    }

    #[test]
    fn menu_bar_dock_and_invisible_or_empty_windows_are_skipped() {
        assert!(!is_snap_candidate(&window(24, 1.0, 300.0, 24.0, 10), 99));
        assert!(!is_snap_candidate(&window(0, 0.0, 300.0, 200.0, 10), 99));
        assert!(!is_snap_candidate(&window(0, 1.0, 0.0, 200.0, 10), 99));
        assert!(!is_snap_candidate(&window(0, 1.0, 300.0, 0.0, 10), 99));
    }

    #[test]
    fn windows_of_this_process_are_skipped() {
        assert!(!is_snap_candidate(&window(0, 1.0, 300.0, 200.0, 99), 99));
    }

    #[test]
    fn a_window_target_is_in_physical_pixels() {
        let target = window_target(&window(0, 1.0, 300.0, 200.0, 10), &[retina()]);
        assert_eq!(target.rect, PhysRect { x: 200, y: 100, width: 600, height: 400 });
        assert_eq!((target.handle, target.title.as_str(), target.process.as_str()), (42, "Groceries", "Notes"));
        assert_eq!(target.kind, TargetKind::Window);
    }

    #[test]
    fn elements_come_children_first_and_belong_to_the_window() {
        let parent = Frame { x: 100.0, y: 50.0, width: 300.0, height: 200.0 };
        let child = Frame { x: 110.0, y: 60.0, width: 20.0, height: 10.0 };
        let targets = element_targets(&window(0, 1.0, 300.0, 200.0, 10), vec![parent, child], &[retina()]);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].rect, PhysRect { x: 220, y: 120, width: 40, height: 20 });
        assert_eq!(targets[1].rect, PhysRect { x: 200, y: 100, width: 600, height: 400 });
        assert!(targets.iter().all(|t| t.kind == TargetKind::Element && t.handle == 42 && t.process == "Notes"));
    }

    #[test]
    fn elements_that_round_to_nothing_are_dropped() {
        let sliver = Frame { x: 110.0, y: 60.0, width: 0.1, height: 10.0 };
        assert!(element_targets(&window(0, 1.0, 300.0, 200.0, 10), vec![sliver], &[retina()]).is_empty());
    }
}
