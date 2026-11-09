//! Output layout and the mixed-scale mapping. Pure: no I/O.
//!
//! Compositors report outputs in logical pixels. Laqta's virtual desktop is physical, so every
//! logical rect is scaled by the largest output scale (`s_max`); a lower-scale output is upscaled
//! to match when its frame is captured.

use laqta_domain::PhysRect;

/// One output: its logical rect in the compositor's space and the scale it renders at.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputLayout {
    pub name: String,
    pub description: String,
    pub logical: PhysRect,
    pub scale: f32,
}

/// The largest usable output scale, or 1.0 when no output reports one.
pub fn max_scale(outputs: &[OutputLayout]) -> f32 {
    outputs.iter().map(|o| o.scale).filter(|s| s.is_finite() && *s > 0.0).reduce(f32::max).unwrap_or(1.0)
}

/// Maps a logical rect to physical pixels at `scale`. Each of x, y, width and height is rounded on
/// its own.
pub fn to_physical(r: PhysRect, scale: f32) -> PhysRect {
    let s = f64::from(scale);
    PhysRect {
        x: (f64::from(r.x) * s).round() as i32,
        y: (f64::from(r.y) * s).round() as i32,
        width: (f64::from(r.width) * s).round() as u32,
        height: (f64::from(r.height) * s).round() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(scale: f32) -> OutputLayout {
        OutputLayout { name: String::new(), description: String::new(), logical: PhysRect::default(), scale }
    }

    #[test]
    fn to_physical_rounds_each_field() {
        let r = PhysRect { x: -3, y: 10, width: 7, height: 3 };
        // 1.25: -3.75 → -4, 12.5 → 13 (half away from zero), 8.75 → 9, 3.75 → 4.
        assert_eq!(to_physical(r, 1.25), PhysRect { x: -4, y: 13, width: 9, height: 4 });
        assert_eq!(to_physical(r, 1.0), r);
    }

    #[test]
    fn max_scale_ignores_invalid_values() {
        assert_eq!(max_scale(&[output(1.0), output(2.0), output(1.5)]), 2.0);
        assert_eq!(max_scale(&[output(f32::NAN), output(0.0), output(-1.0)]), 1.0);
        assert_eq!(max_scale(&[output(1.25), output(f32::NAN)]), 1.25);
        assert_eq!(max_scale(&[]), 1.0);
    }
}
