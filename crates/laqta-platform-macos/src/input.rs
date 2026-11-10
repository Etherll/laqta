//! Synthetic scrolling for scrolling capture (`InputSynth`): a scroll-wheel `CGEvent` at the
//! region's point, or Down Arrow and Page Down key presses. Posting events needs Accessibility.

use laqta_domain::capture::ScrollMethod;
use laqta_domain::geometry::PhysRect;
use laqta_ports::{InputSynth, PortError};
use objc2_core_foundation::CGPoint;
use objc2_core_graphics::{CGEvent, CGEventTapLocation, CGScrollEventUnit};

use crate::displays::{self, Display};
use crate::permissions;

/// Virtual key codes (`kVK_DownArrow`, `kVK_PageDown`). They name physical keys, so they hold on
/// every keyboard layout.
const KEY_DOWN_ARROW: u16 = 0x7D;
const KEY_PAGE_DOWN: u16 = 0x79;

#[derive(Debug, Default)]
pub struct MacInput;

impl InputSynth for MacInput {
    fn scroll(&self, x: i32, y: i32, method: ScrollMethod, amount: u32) -> Result<(), PortError> {
        permissions::accessibility()?;
        match method {
            // `WM_VSCROLL` has no counterpart here.
            ScrollMethod::ScrollMessage => Err(PortError::Unsupported("scroll messages are Windows-only".into())),
            ScrollMethod::MouseWheel => scroll_wheel(point_at(&displays::list(), x, y)?, amount),
            ScrollMethod::DownArrow => press(KEY_DOWN_ARROW, amount),
            ScrollMethod::PageDown => press(KEY_PAGE_DOWN, amount),
        }
    }
}

/// The physical-pixel point as global points, using the scale of the display that holds it.
fn point_at(displays: &[Display], x: i32, y: i32) -> Result<CGPoint, PortError> {
    let (_, px, py, _, _) = displays::to_points(displays, PhysRect { x, y, width: 1, height: 1 })
        .ok_or_else(|| PortError::Failed("no display holds the scroll point".into()))?;
    Ok(CGPoint::new(px, py))
}

/// The wheel delta in lines for `amount` steps down. Negative scrolls down.
fn wheel_lines(amount: u32) -> i32 {
    -i32::try_from(amount).unwrap_or(i32::MAX)
}

/// One scroll-wheel event of `amount` lines down, located at `at`. The pointer itself is not moved.
fn scroll_wheel(at: CGPoint, amount: u32) -> Result<(), PortError> {
    if amount == 0 {
        return Ok(());
    }
    let event = CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Line, 1, wheel_lines(amount), 0, 0)
        .ok_or_else(|| PortError::Failed("CGEventCreateScrollWheelEvent2 failed".into()))?;
    CGEvent::set_location(Some(&event), at);
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    Ok(())
}

/// `amount` presses of `key`, each a key down followed by a key up, to the focused app.
fn press(key: u16, amount: u32) -> Result<(), PortError> {
    for _ in 0..amount {
        for down in [true, false] {
            let event = CGEvent::new_keyboard_event(None, key, down)
                .ok_or_else(|| PortError::Failed("CGEventCreateKeyboardEvent failed".into()))?;
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn external() -> Display {
        Display {
            id: 2,
            name: "External".into(),
            x: 1512.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
            scale: 1.0,
            layout_scale: 2.0,
            primary: false,
        }
    }

    #[test]
    fn steps_become_negative_lines() {
        assert_eq!(wheel_lines(3), -3);
        assert_eq!(wheel_lines(0), 0);
        assert_eq!(wheel_lines(u32::MAX), -i32::MAX);
    }

    #[test]
    fn pixels_become_points_by_the_display_scale() {
        let displays = [retina(), external()];
        let at = |x, y| point_at(&displays, x, y).ok().map(|p| (p.x, p.y));
        assert_eq!(at(600, 400), Some((300.0, 200.0)));
        // Past the Retina display's 3024 pixels, the 1x display holds the point: 176 pixels in,
        // one point each, from its origin at 1512 points.
        assert_eq!(at(3200, 40), Some((1688.0, 40.0)));
    }

    #[test]
    fn no_display_is_an_error() {
        assert!(point_at(&[], 10, 10).is_err());
    }
}
