//! X11 recording: the area is a `Target` that `laqta-media` captures with `x11grab` (ADR 016), so
//! this module never reads pixels itself.

use laqta_domain::PhysRect;
use laqta_ports::recording::{OsScreen, OsScreenRef};
use laqta_ports::{PortError, VideoSource};

/// The target for `area` on the display in `$DISPLAY`.
pub fn open(area: PhysRect) -> Result<VideoSource, PortError> {
    let display = std::env::var("DISPLAY")
        .ok()
        .filter(|display| !display.is_empty())
        .ok_or_else(|| PortError::Unsupported("DISPLAY is not set".into()))?;
    let area = match root_size() {
        Some(size) => clip(area, size)?,
        None => area,
    };
    target(display, area).map(VideoSource::Target)
}

/// The root window's size, when the display answers.
fn root_size() -> Option<(u32, u32)> {
    use x11rb::connection::Connection as _;
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?;
    Some((u32::from(root.width_in_pixels), u32::from(root.height_in_pixels)))
}

/// The part of `area` on the screen: `x11grab` refuses an area that reaches past it, as an active
/// window partly off-screen does.
fn clip(area: PhysRect, (width, height): (u32, u32)) -> Result<PhysRect, PortError> {
    let left = i64::from(area.x).clamp(0, i64::from(width));
    let top = i64::from(area.y).clamp(0, i64::from(height));
    let right = (i64::from(area.x) + i64::from(area.width)).clamp(0, i64::from(width));
    let bottom = (i64::from(area.y) + i64::from(area.height)).clamp(0, i64::from(height));
    if right <= left || bottom <= top {
        return Err(PortError::Failed("the area is off the screen".into()));
    }
    // In range: every value lies between 0 and the u16 screen size.
    Ok(PhysRect { x: left as i32, y: top as i32, width: (right - left) as u32, height: (bottom - top) as u32 })
}

fn target(display: String, area: PhysRect) -> Result<OsScreenRef, PortError> {
    let area = super::even_area(area)?;
    Ok(OsScreenRef::new(OsScreen::X11 { display, x: area.x, y: area.y, width: area.width, height: area.height }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_carries_the_even_area() {
        let area = PhysRect { x: 10, y: -20, width: 801, height: 600 };
        let target = target(":0".into(), area).ok();
        let expected = OsScreen::X11 { display: ":0".into(), x: 10, y: -20, width: 800, height: 600 };
        assert_eq!(target.as_ref().map(OsScreenRef::screen), Some(&expected));
    }

    #[test]
    fn an_area_past_the_screen_is_clipped_to_it() {
        let area = PhysRect { x: -100, y: 900, width: 800, height: 400 };
        assert_eq!(clip(area, (1920, 1080)).ok(), Some(PhysRect { x: 0, y: 900, width: 700, height: 180 }));
        let inside = PhysRect { x: 10, y: 10, width: 100, height: 100 };
        assert_eq!(clip(inside, (1920, 1080)).ok(), Some(inside));
        assert!(clip(PhysRect { x: 2000, y: 0, width: 100, height: 100 }, (1920, 1080)).is_err());
    }
}
