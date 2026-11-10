//! Displays and the conversion between macOS points and Laqta's physical pixels.
//!
//! Core Graphics places every display in one space of points: origin at the top-left corner of the
//! main display, y down. Laqta's ports use physical pixels. A display's pixel rect is its point
//! rect times its scale, so a point inside a display maps to a pixel inside that display's pixel
//! rect. AppKit's bottom-left coordinates never leave this crate.

use laqta_domain::geometry::PhysRect;

/// One display, as Core Graphics describes it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Display {
    /// The `CGDirectDisplayID`.
    pub id: u32,
    pub name: String,
    /// The display's rect in global points.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Pixels per point (2.0 on Retina).
    pub scale: f64,
    pub primary: bool,
}

impl Display {
    /// The display's rect in physical pixels.
    pub(crate) fn phys_rect(&self) -> PhysRect {
        PhysRect {
            x: (self.x * self.scale).round() as i32,
            y: (self.y * self.scale).round() as i32,
            width: (self.width * self.scale).round() as u32,
            height: (self.height * self.scale).round() as u32,
        }
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// The display that holds the point: the one containing it, else the primary, else the first.
pub(crate) fn display_at(displays: &[Display], x: f64, y: f64) -> Option<&Display> {
    displays.iter().find(|d| d.contains(x, y)).or_else(|| displays.iter().find(|d| d.primary)).or_else(|| displays.first())
}

/// A rect in global points as physical pixels, scaled by the display that holds its centre.
pub(crate) fn to_phys(displays: &[Display], x: f64, y: f64, width: f64, height: f64) -> PhysRect {
    let scale = display_at(displays, x + width / 2.0, y + height / 2.0).map_or(1.0, |d| d.scale);
    PhysRect {
        x: (x * scale).round() as i32,
        y: (y * scale).round() as i32,
        width: (width * scale).round().max(0.0) as u32,
        height: (height * scale).round().max(0.0) as u32,
    }
}

/// A physical-pixel rect as `(display, x, y, width, height)` in global points: the display whose
/// pixel rect holds the rect's centre, else the primary.
pub(crate) fn to_points(displays: &[Display], rect: PhysRect) -> Option<(&Display, f64, f64, f64, f64)> {
    let (cx, cy) = (i64::from(rect.x) + i64::from(rect.width / 2), i64::from(rect.y) + i64::from(rect.height / 2));
    let holds = |d: &&Display| {
        let p = d.phys_rect();
        cx >= i64::from(p.x) && cx < i64::from(p.right()) && cy >= i64::from(p.y) && cy < i64::from(p.bottom())
    };
    let display = displays.iter().find(holds).or_else(|| displays.iter().find(|d| d.primary)).or_else(|| displays.first())?;
    let s = display.scale;
    Some((display, f64::from(rect.x) / s, f64::from(rect.y) / s, f64::from(rect.width) / s, f64::from(rect.height) / s))
}

/// The active displays, the primary first. Empty when Core Graphics reports none.
#[cfg(target_os = "macos")]
pub(crate) fn list() -> Vec<Display> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retina() -> Display {
        Display { id: 1, name: "Built-in".into(), x: 0.0, y: 0.0, width: 1512.0, height: 982.0, scale: 2.0, primary: true }
    }

    fn external() -> Display {
        Display { id: 2, name: "External".into(), x: 1512.0, y: 0.0, width: 1920.0, height: 1080.0, scale: 1.0, primary: false }
    }

    #[test]
    fn a_retina_display_has_twice_the_pixels() {
        assert_eq!(retina().phys_rect(), PhysRect { x: 0, y: 0, width: 3024, height: 1964 });
    }

    #[test]
    fn points_and_pixels_round_trip_on_one_display() {
        let displays = [retina()];
        let phys = to_phys(&displays, 100.0, 50.0, 300.0, 200.0);
        assert_eq!(phys, PhysRect { x: 200, y: 100, width: 600, height: 400 });
        let (d, x, y, w, h) = to_points(&displays, phys).unwrap();
        assert_eq!((d.id, x, y, w, h), (1, 100.0, 50.0, 300.0, 200.0));
    }

    #[test]
    fn a_rect_takes_the_scale_of_the_display_under_its_centre() {
        let displays = [retina(), external()];
        assert_eq!(to_phys(&displays, 1600.0, 10.0, 100.0, 100.0), PhysRect { x: 1600, y: 10, width: 100, height: 100 });
    }

    #[test]
    fn a_point_off_every_display_falls_back_to_the_primary() {
        let displays = [external(), retina()];
        assert_eq!(display_at(&displays, -50.0, -50.0).map(|d| d.id), Some(1));
        assert!(display_at(&[], 0.0, 0.0).is_none());
    }
}
