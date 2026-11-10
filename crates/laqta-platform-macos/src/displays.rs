//! Displays and the conversion between macOS points and Laqta's physical pixels.
//!
//! Core Graphics places every display in one space of points: origin at the top-left corner of the
//! main display, y down. Laqta's ports use physical pixels, which macOS has no global space for.
//! So this module lays one out: a display's pixel origin is its point origin times the largest
//! scale among the displays, and its pixel size is its point size times its own scale. With one
//! scale that is plain points times scale. With a Retina display beside a 1x one, the 1x display
//! leaves a gap after it, and no two pixel rects overlap, so a pixel names one display. AppKit's
//! bottom-left coordinates never leave this crate.

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
    /// The largest scale among the active displays: what point origins are multiplied by.
    pub layout_scale: f64,
    pub primary: bool,
}

impl Display {
    /// The display's rect in physical pixels.
    pub(crate) fn phys_rect(&self) -> PhysRect {
        PhysRect {
            x: (self.x * self.layout_scale).round() as i32,
            y: (self.y * self.layout_scale).round() as i32,
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

/// A rect in global points as physical pixels, inside the pixel rect of the display that holds
/// its centre.
pub(crate) fn to_phys(displays: &[Display], x: f64, y: f64, width: f64, height: f64) -> PhysRect {
    let Some(d) = display_at(displays, x + width / 2.0, y + height / 2.0) else {
        return PhysRect {
            x: x.round() as i32,
            y: y.round() as i32,
            width: width.round().max(0.0) as u32,
            height: height.round().max(0.0) as u32,
        };
    };
    let origin = d.phys_rect();
    PhysRect {
        x: origin.x + ((x - d.x) * d.scale).round() as i32,
        y: origin.y + ((y - d.y) * d.scale).round() as i32,
        width: (width * d.scale).round().max(0.0) as u32,
        height: (height * d.scale).round().max(0.0) as u32,
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
    let d = displays.iter().find(holds).or_else(|| displays.iter().find(|d| d.primary)).or_else(|| displays.first())?;
    let (origin, s) = (d.phys_rect(), d.scale);
    let x = d.x + f64::from(rect.x - origin.x) / s;
    let y = d.y + f64::from(rect.y - origin.y) / s;
    Some((d, x, y, f64::from(rect.width) / s, f64::from(rect.height) / s))
}

/// `to_points`, cut to the display it picked, as `[x, y, width, height]`. A recording covers one
/// display (ADR 030), so this is the part of a recording area that is in the file, and where its
/// border and bar belong. `None` when no display is known or nothing of the rect is on the display.
pub(crate) fn to_points_on_display(displays: &[Display], rect: PhysRect) -> Option<[f64; 4]> {
    let (d, x, y, width, height) = to_points(displays, rect)?;
    let (left, top) = (x.max(d.x), y.max(d.y));
    let (right, bottom) = ((x + width).min(d.x + d.width), (y + height).min(d.y + d.height));
    (right > left && bottom > top).then_some([left, top, right - left, bottom - top])
}

/// Turns premultiplied BGRA into straight alpha in place: Core Graphics draws premultiplied, and
/// a `Bitmap` holds straight alpha.
pub(crate) fn unpremultiply_bgra(pixels: &mut [u8]) {
    for px in pixels.as_chunks_mut::<4>().0 {
        let alpha = u32::from(px[3]);
        if alpha == 0 || alpha == 255 {
            continue;
        }
        for channel in &mut px[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
}

/// Sets every pixel's alpha to 255. A display image has no transparency, whatever its alpha byte says.
pub(crate) fn make_opaque_bgra(pixels: &mut [u8]) {
    for px in pixels.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
}

/// The active displays, the primary first. Empty when Core Graphics reports none.
#[cfg(target_os = "macos")]
pub(crate) fn list() -> Vec<Display> {
    use objc2_core_graphics::{
        CGDirectDisplayID, CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayMode, CGError, CGGetActiveDisplayList, CGMainDisplayID,
    };

    // Far more displays than a Mac can drive.
    const MAX: usize = 32;
    let mut ids: [CGDirectDisplayID; MAX] = [0; MAX];
    let mut count: u32 = 0;
    // SAFETY: `ids` has room for MAX entries, which is the capacity passed in, and `count` is a
    // valid out pointer.
    let status = unsafe { CGGetActiveDisplayList(MAX as u32, ids.as_mut_ptr(), &mut count) };
    if status != CGError::Success {
        return Vec::new();
    }
    let main = CGMainDisplayID();
    let mut displays: Vec<Display> = ids
        .iter()
        .take(count as usize)
        .map(|&id| {
            let bounds = CGDisplayBounds(id);
            let (width, height) = (bounds.size.width, bounds.size.height);
            // The mode's pixel width is the backing store, so a scaled "looks like" mode still reports 2.0 on Retina.
            let mode = CGDisplayCopyDisplayMode(id);
            let pixels = CGDisplayMode::pixel_width(mode.as_deref()) as f64;
            let scale = if width > 0.0 && pixels > 0.0 { pixels / width } else { 1.0 };
            Display {
                id,
                name: format!("Display {id}"),
                x: bounds.origin.x,
                y: bounds.origin.y,
                width,
                height,
                scale,
                layout_scale: scale,
                primary: id == main,
            }
        })
        .collect();
    let largest = displays.iter().map(|d| d.scale).fold(1.0, f64::max);
    displays.iter_mut().for_each(|d| d.layout_scale = largest);
    // The sort is stable, so the other displays keep Core Graphics' order.
    displays.sort_by_key(|d| !d.primary);
    displays
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
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

    #[test]
    fn a_rect_across_two_displays_is_cut_to_the_one_under_its_centre() {
        let displays = [retina(), external()];
        // 2000 pixels wide, starting 600 pixels before the Retina display's right edge (3024):
        // the centre is on the external display, whose pixels are one point each from 1512.
        let across = PhysRect { x: 2424, y: 100, width: 2000, height: 400 };
        assert_eq!(to_points_on_display(&displays, across), Some([1512.0, 100.0, 1400.0, 400.0]));
        // A rect inside one display is unchanged.
        let inside = PhysRect { x: 200, y: 100, width: 400, height: 200 };
        assert_eq!(to_points_on_display(&displays, inside), Some([100.0, 50.0, 200.0, 100.0]));
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
        // The 1x display starts at point 1512, which is pixel 3024: past the Retina display's pixels.
        assert_eq!(external().phys_rect(), PhysRect { x: 3024, y: 0, width: 1920, height: 1080 });
        let phys = to_phys(&displays, 1600.0, 10.0, 100.0, 100.0);
        assert_eq!(phys, PhysRect { x: 3112, y: 10, width: 100, height: 100 });
        let (d, x, y, w, h) = to_points(&displays, phys).unwrap();
        assert_eq!((d.id, x, y, w, h), (2, 1600.0, 10.0, 100.0, 100.0));
        // A pixel in the Retina display's right half still names the Retina display.
        let (d, x, ..) = to_points(&displays, PhysRect { x: 2000, y: 0, width: 10, height: 10 }).unwrap();
        assert_eq!((d.id, x), (1, 1000.0));
    }

    #[test]
    fn unpremultiplying_restores_straight_colour() {
        // 50% alpha: a premultiplied 100 is straight 200; opaque and clear pixels are untouched.
        let mut px = [100, 50, 0, 128, 10, 20, 30, 255, 0, 0, 0, 0];
        unpremultiply_bgra(&mut px);
        assert_eq!(px, [199, 100, 0, 128, 10, 20, 30, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn unpremultiplying_never_overflows_a_channel() {
        let mut px = [255, 255, 255, 1];
        unpremultiply_bgra(&mut px);
        assert_eq!(px, [255, 255, 255, 1]);
    }

    #[test]
    fn opaque_forces_alpha_and_keeps_colour() {
        let mut px = [1, 2, 3, 0, 4, 5, 6, 77];
        make_opaque_bgra(&mut px);
        assert_eq!(px, [1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn a_point_off_every_display_falls_back_to_the_primary() {
        let displays = [external(), retina()];
        assert_eq!(display_at(&displays, -50.0, -50.0).map(|d| d.id), Some(1));
        assert!(display_at(&[], 0.0, 0.0).is_none());
    }
}
