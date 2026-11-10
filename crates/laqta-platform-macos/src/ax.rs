//! Accessibility API helpers: the UI elements of one window, for hover snapping.
//!
//! Everything is point-in-time: a walk builds its own `AXUIElement` handles, reads them and drops
//! them before it returns. Nothing is cached, and no thread or timer outlives a call. All
//! coordinates are global points, top-left origin, the same space as `CGWindowList` bounds.
//! Callers check `permissions::accessibility()` first; without it every read fails and the walk is
//! empty.

use std::ptr::{self, NonNull};
use std::time::{Duration, Instant};

use objc2_application_services::{AXError, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{CFArray, CFRetained, CFString, CFType, CGPoint, CGSize};

/// How many levels below the window the walk descends.
const MAX_DEPTH: usize = 8;
/// How many elements one walk keeps.
const MAX_ELEMENTS: usize = 400;
/// How many elements one walk reads, kept or not, so a huge tree with few usable frames still ends.
const MAX_VISITED: usize = 1_000;
/// Seconds one Accessibility call may wait for the target app. The default is several seconds, long
/// enough to stall the overlay when an app hangs.
const CALL_TIMEOUT: f32 = 0.25;
/// How long a whole walk may run. Whatever it found by then is kept.
const WALK_BUDGET: Duration = Duration::from_millis(400);
/// How far an Accessibility window frame may differ from its `CGWindowList` bounds, in points, and
/// still be the same window.
const WINDOW_TOLERANCE: f64 = 3.0;

/// A rect in global points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    /// The part of `self` inside `to`, or `None` when nothing is left (which also drops frames with
    /// no size).
    pub(crate) fn clip(self, to: Frame) -> Option<Frame> {
        let x0 = self.x.max(to.x);
        let y0 = self.y.max(to.y);
        let x1 = (self.x + self.width).min(to.x + to.width);
        let y1 = (self.y + self.height).min(to.y + to.height);
        (x1 > x0 && y1 > y0).then_some(Frame { x: x0, y: y0, width: x1 - x0, height: y1 - y0 })
    }

    /// Whether every edge of `other` is within `tolerance` of the same edge of `self`.
    pub(crate) fn matches(self, other: Frame, tolerance: f64) -> bool {
        (self.x - other.x).abs() <= tolerance
            && (self.y - other.y).abs() <= tolerance
            && (self.width - other.width).abs() <= tolerance
            && (self.height - other.height).abs() <= tolerance
    }
}

/// The attribute names. Their constants (`kAXPositionAttribute` and the rest) are C macros, so
/// there are no bindings for them; the strings are the values from `AXAttributeConstants.h`.
struct Names {
    position: CFRetained<CFString>,
    size: CFRetained<CFString>,
    children: CFRetained<CFString>,
    focused_window: CFRetained<CFString>,
    windows: CFRetained<CFString>,
}

impl Names {
    fn new() -> Self {
        Self {
            position: CFString::from_static_str("AXPosition"),
            size: CFString::from_static_str("AXSize"),
            children: CFString::from_static_str("AXChildren"),
            focused_window: CFString::from_static_str("AXFocusedWindow"),
            windows: CFString::from_static_str("AXWindows"),
        }
    }
}

/// The frames of the elements inside the window of process `pid` whose frame is `window`, in
/// pre-order (parents before their children), each clipped to the window. Empty when the process
/// has no such window, does not answer, or Accessibility is not granted.
pub(crate) fn element_frames(pid: i32, window: Frame) -> Vec<Frame> {
    let names = Names::new();
    // SAFETY: a plain constructor; the returned element is owned and released on drop.
    let app = unsafe { AXUIElement::new_application(pid) };
    limit_wait(&app);
    let Some(root) = find_window(&names, &app, window) else {
        return Vec::new();
    };
    walk(&names, &root, window, Instant::now() + WALK_BUDGET)
}

/// Caps how long calls on `element` wait for its app.
fn limit_wait(element: &AXUIElement) {
    // SAFETY: a call on a live element with a plain number. A failure only leaves the default wait.
    let _ = unsafe { element.set_messaging_timeout(CALL_TIMEOUT) };
}

/// The app's window drawn at `frame`: its focused window if that fits, else any other of its windows.
fn find_window(names: &Names, app: &AXUIElement, frame: Frame) -> Option<CFRetained<AXUIElement>> {
    let fits = |window: &CFRetained<AXUIElement>| {
        limit_wait(window);
        frame_of(names, window).is_some_and(|found| found.matches(frame, WINDOW_TOLERANCE))
    };
    let focused = attribute(app, &names.focused_window).and_then(|value| value.downcast::<AXUIElement>().ok());
    focused.filter(fits).or_else(|| elements(app, &names.windows).into_iter().find(fits))
}

/// Depth-first, pre-order walk below `root` that keeps the frames of elements with a visible part
/// inside `clip`. Stops at the limits or at `deadline`, keeping what it has.
fn walk(names: &Names, root: &AXUIElement, clip: Frame, deadline: Instant) -> Vec<Frame> {
    let mut found = Vec::new();
    let mut visited = 0;
    // The last pushed is popped first, so children go on in reverse to visit the first child first.
    let mut stack: Vec<(CFRetained<AXUIElement>, usize)> =
        elements(root, &names.children).into_iter().rev().map(|child| (child, 1)).collect();
    while let Some((element, depth)) = stack.pop() {
        if found.len() >= MAX_ELEMENTS || visited >= MAX_VISITED || Instant::now() >= deadline {
            break;
        }
        visited += 1;
        limit_wait(&element);
        if let Some(frame) = frame_of(names, &element).and_then(|frame| frame.clip(clip)) {
            found.push(frame);
        }
        if depth < MAX_DEPTH {
            stack.extend(elements(&element, &names.children).into_iter().rev().map(|child| (child, depth + 1)));
        }
    }
    found
}

/// The element's frame from its position and size attributes.
fn frame_of(names: &Names, element: &AXUIElement) -> Option<Frame> {
    let position = attribute(element, &names.position)?;
    let size = attribute(element, &names.size)?;
    let position = ax_value(&position)?;
    let size = ax_value(&size)?;
    let (x, y) = position.point()?;
    let (width, height) = size.size()?;
    Some(Frame { x, y, width, height })
}

/// A list-valued attribute (`AXChildren`, `AXWindows`) as elements, at most `MAX_ELEMENTS` of them.
/// Empty when the attribute is missing or not a list.
fn elements(element: &AXUIElement, attribute_name: &CFString) -> Vec<CFRetained<AXUIElement>> {
    let Some(value) = attribute(element, attribute_name) else {
        return Vec::new();
    };
    let Ok(array) = value.downcast::<CFArray>() else {
        return Vec::new();
    };
    // SAFETY: every element of a CFArray is a CFType, which is all this view of it claims.
    let array = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(array) };
    array.iter().take(MAX_ELEMENTS).filter_map(|item| item.downcast::<AXUIElement>().ok()).collect()
}

/// Copies one attribute. `None` when the element has no such attribute, has no value, or does not
/// answer in time.
fn attribute(element: &AXUIElement, name: &CFString) -> Option<CFRetained<CFType>> {
    let mut value: *const CFType = ptr::null();
    // SAFETY: `value` is a valid out-pointer for the whole call. On success the framework stores
    // a value it has retained for us (a Copy function).
    let status = unsafe { element.copy_attribute_value(name, NonNull::from(&mut value)) };
    if status != AXError::Success {
        return None;
    }
    // SAFETY: the value is owned by us after a successful Copy, so `CFRetained` takes over its release.
    NonNull::new(value.cast_mut()).map(|value| unsafe { CFRetained::from_raw(value) })
}

/// A value that may hold a point or a size.
struct Value<'a>(&'a AXValue);

fn ax_value(value: &CFType) -> Option<Value<'_>> {
    value.downcast_ref::<AXValue>().map(Value)
}

impl Value<'_> {
    fn point(&self) -> Option<(f64, f64)> {
        let mut point = CGPoint::new(0.0, 0.0);
        // SAFETY: `point` is a CGPoint, the layout `AXValueType::CGPoint` asks for. A value of
        // another type makes the call return false and leaves it untouched.
        unsafe { self.0.value(AXValueType::CGPoint, NonNull::from(&mut point).cast()) }.then_some((point.x, point.y))
    }

    fn size(&self) -> Option<(f64, f64)> {
        let mut size = CGSize::new(0.0, 0.0);
        // SAFETY: `size` is a CGSize, the layout `AXValueType::CGSize` asks for. A value of another
        // type makes the call return false and leaves it untouched.
        unsafe { self.0.value(AXValueType::CGSize, NonNull::from(&mut size).cast()) }.then_some((size.width, size.height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: f64, y: f64, width: f64, height: f64) -> Frame {
        Frame { x, y, width, height }
    }

    #[test]
    fn clip_keeps_the_part_inside_the_window() {
        let window = frame(100.0, 100.0, 400.0, 300.0);
        assert_eq!(frame(50.0, 150.0, 100.0, 40.0).clip(window), Some(frame(100.0, 150.0, 50.0, 40.0)));
        assert_eq!(frame(120.0, 120.0, 10.0, 10.0).clip(window), Some(frame(120.0, 120.0, 10.0, 10.0)));
    }

    #[test]
    fn clip_drops_frames_outside_or_without_size() {
        let window = frame(0.0, 0.0, 100.0, 100.0);
        assert_eq!(frame(200.0, 0.0, 10.0, 10.0).clip(window), None);
        assert_eq!(frame(100.0, 0.0, 10.0, 10.0).clip(window), None);
        assert_eq!(frame(10.0, 10.0, 0.0, 20.0).clip(window), None);
        assert_eq!(frame(10.0, 10.0, -5.0, 20.0).clip(window), None);
    }

    #[test]
    fn matches_allows_a_small_difference_per_edge() {
        let window = frame(10.0, 20.0, 800.0, 600.0);
        assert!(window.matches(frame(11.0, 22.0, 799.0, 601.0), WINDOW_TOLERANCE));
        assert!(!window.matches(frame(10.0, 20.0, 800.0, 590.0), WINDOW_TOLERANCE));
    }
}
