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
    target(display, area).map(VideoSource::Target)
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
}
