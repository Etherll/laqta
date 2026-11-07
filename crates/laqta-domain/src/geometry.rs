//! Physical-pixel geometry.

/// A rectangle in physical pixels, in the virtual-desktop coordinate space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    fn right_bottom() {
        let r = PhysRect { x: -10, y: 5, width: 20, height: 30 };
        assert_eq!(r.right(), 10);
        assert_eq!(r.bottom(), 35);
    }
}
