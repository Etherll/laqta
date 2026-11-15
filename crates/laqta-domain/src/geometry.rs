//! Physical-pixel geometry.

/// A rectangle in physical pixels, in the virtual-desktop coordinate space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PhysRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PhysRect {
    pub fn right(self) -> i32 {
        self.x.saturating_add_unsigned(self.width)
    }

    pub fn bottom(self) -> i32 {
        self.y.saturating_add_unsigned(self.height)
    }

    /// The smallest rect containing both.
    pub fn union(self, other: PhysRect) -> PhysRect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        PhysRect { x, y, width: right.abs_diff(x), height: bottom.abs_diff(y) }
    }

    /// The part both rectangles cover, or `None` when they share no pixel.
    pub fn intersection(self, other: PhysRect) -> Option<PhysRect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > x && bottom > y).then(|| PhysRect { x, y, width: right.abs_diff(x), height: bottom.abs_diff(y) })
    }
}

/// `area` clipped to the monitor it overlaps most (the first on a tie), or `None` when it overlaps none.
/// An area that spans monitors cannot be recorded as one stream, so the recording keeps the biggest part.
pub fn largest_overlap(area: PhysRect, monitors: &[PhysRect]) -> Option<PhysRect> {
    let mut best: Option<(u64, PhysRect)> = None;
    for monitor in monitors {
        if let Some(part) = area.intersection(*monitor) {
            let size = u64::from(part.width) * u64::from(part.height);
            if best.is_none_or(|(most, _)| size > most) {
                best = Some((size, part));
            }
        }
    }
    best.map(|(_, part)| part)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct MonitorId(pub String);

#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub id: MonitorId,
    pub name: String,
    pub rect: PhysRect,
    pub scale: f32,
    pub primary: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_spans_both() {
        let a = PhysRect { x: 0, y: 0, width: 1920, height: 1080 };
        let b = PhysRect { x: 1920, y: -200, width: 2560, height: 1440 };
        assert_eq!(a.union(b), PhysRect { x: 0, y: -200, width: 4480, height: 1440 });
        assert_eq!(b.union(a), a.union(b));
        assert_eq!(a.union(a), a);
    }

    #[test]
    fn largest_overlap_clips_to_the_biggest_monitor() {
        let left = PhysRect { x: 0, y: 0, width: 1920, height: 1080 };
        let right = PhysRect { x: 1920, y: 0, width: 1920, height: 1080 };
        let area = PhysRect { x: 1500, y: 100, width: 1000, height: 400 };
        // 420 px on the left monitor, 580 px on the right one.
        assert_eq!(largest_overlap(area, &[left, right]), Some(PhysRect { x: 1920, y: 100, width: 580, height: 400 }));
        assert_eq!(largest_overlap(area, &[right, left]), Some(PhysRect { x: 1920, y: 100, width: 580, height: 400 }));
        let inside = PhysRect { x: 10, y: 10, width: 100, height: 100 };
        assert_eq!(largest_overlap(inside, &[left, right]), Some(inside));
    }

    #[test]
    fn largest_overlap_is_none_off_every_monitor() {
        let screen = PhysRect { x: 0, y: 0, width: 100, height: 100 };
        assert_eq!(largest_overlap(PhysRect { x: 100, y: 0, width: 10, height: 10 }, &[screen]), None);
        assert_eq!(largest_overlap(PhysRect { x: 0, y: 0, width: 10, height: 10 }, &[]), None);
    }

    #[test]
    fn right_bottom() {
        let r = PhysRect { x: -10, y: 5, width: 20, height: 30 };
        assert_eq!(r.right(), 10);
        assert_eq!(r.bottom(), 35);
    }
}
