//! Float geometry in image pixels.

use laqta_domain::geometry::PhysRect;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointF {
    pub x: f32,
    pub y: f32,
}

impl PointF {
    pub const fn new(x: f32, y: f32) -> PointF {
        PointF { x, y }
    }

    pub fn offset(self, dx: f32, dy: f32) -> PointF {
        PointF { x: self.x + dx, y: self.y + dy }
    }

    pub fn distance(self, o: PointF) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
}

/// A rectangle with non-negative width and height (after `normalized`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> RectF {
        RectF { x, y, w, h }
    }

    pub fn from_points(a: PointF, b: PointF) -> RectF {
        RectF { x: a.x.min(b.x), y: a.y.min(b.y), w: (a.x - b.x).abs(), h: (a.y - b.y).abs() }
    }

    pub fn normalized(self) -> RectF {
        RectF::from_points(PointF::new(self.x, self.y), PointF::new(self.x + self.w, self.y + self.h))
    }

    pub fn right(self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(self) -> f32 {
        self.y + self.h
    }

    pub fn center(self) -> PointF {
        PointF::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(self, p: PointF, tolerance: f32) -> bool {
        let r = self.normalized();
        p.x >= r.x - tolerance && p.x <= r.right() + tolerance && p.y >= r.y - tolerance && p.y <= r.bottom() + tolerance
    }

    pub fn offset(self, dx: f32, dy: f32) -> RectF {
        RectF { x: self.x + dx, y: self.y + dy, ..self }
    }

    pub fn inflate(self, by: f32) -> RectF {
        RectF { x: self.x - by, y: self.y - by, w: self.w + 2.0 * by, h: self.h + 2.0 * by }
    }

    pub fn union(self, o: RectF) -> RectF {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        RectF { x, y, w: self.right().max(o.right()) - x, h: self.bottom().max(o.bottom()) - y }
    }

    /// Smallest integer rect covering this one.
    pub fn to_phys(self) -> PhysRect {
        let r = self.normalized();
        let x0 = r.x.floor() as i32;
        let y0 = r.y.floor() as i32;
        let x1 = r.right().ceil() as i32;
        let y1 = r.bottom().ceil() as i32;
        PhysRect { x: x0, y: y0, width: (x1 - x0).max(0) as u32, height: (y1 - y0).max(0) as u32 }
    }

    pub fn from_phys(r: PhysRect) -> RectF {
        RectF::new(r.x as f32, r.y as f32, r.width as f32, r.height as f32)
    }
}

/// Integer intersection; `None` when empty.
pub fn intersect(a: PhysRect, b: PhysRect) -> Option<PhysRect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.right().min(b.right());
    let y1 = a.bottom().min(b.bottom());
    (x1 > x0 && y1 > y0).then(|| PhysRect { x: x0, y: y0, width: (x1 - x0) as u32, height: (y1 - y0) as u32 })
}

/// Distance from `p` to the segment `a`–`b`.
pub fn segment_distance(p: PointF, a: PointF, b: PointF) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.distance(PointF::new(a.x + t * dx, a.y + t * dy))
}

/// Snaps `to` so the angle from `from` is a multiple of 45°.
pub fn snap_45(from: PointF, to: PointF) -> PointF {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let len = (dx * dx + dy * dy).sqrt();
    let angle = dy.atan2(dx);
    let step = std::f32::consts::FRAC_PI_4;
    let a = (angle / step).round() * step;
    PointF::new(from.x + len * a.cos(), from.y + len * a.sin())
}

/// Makes the rect from `anchor` to `to` a square (Shift while drawing).
pub fn square(anchor: PointF, to: PointF) -> PointF {
    let (dx, dy) = (to.x - anchor.x, to.y - anchor.y);
    let s = dx.abs().max(dy.abs());
    PointF::new(anchor.x + s * dx.signum(), anchor.y + s * dy.signum())
}
