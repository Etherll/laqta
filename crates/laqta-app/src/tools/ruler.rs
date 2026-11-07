//! Ruler maths: the size of the rectangle two points span, and pixel-to-physical conversions (M5 ruler). Pure values.

/// What the ruler shows for two points, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    pub width: f64,
    pub height: f64,
    /// The length of the segment between the two points.
    pub diagonal: f64,
    /// The direction from the first point to the second, in degrees clockwise from the +x axis (y points down),
    /// in `0.0..360.0`.
    pub angle_deg: f64,
    /// The area of the rectangle the two points span.
    pub area: u64,
    /// The perimeter of that rectangle.
    pub perimeter: u64,
}

/// The measurement from `from` to `to`, in screen pixels.
pub fn measure(from: (i32, i32), to: (i32, i32)) -> Measurement {
    let dx = i64::from(to.0) - i64::from(from.0);
    let dy = i64::from(to.1) - i64::from(from.1);
    let (width, height) = (dx.unsigned_abs(), dy.unsigned_abs());
    Measurement {
        width: width as f64,
        height: height as f64,
        diagonal: (dx as f64).hypot(dy as f64),
        angle_deg: (dy as f64).atan2(dx as f64).to_degrees().rem_euclid(360.0),
        area: width * height,
        perimeter: 2 * (width + height),
    }
}

/// `px` pixels in inches at `dpi`. `None` when `dpi` is not a positive finite number.
pub fn px_to_inches(px: f64, dpi: f64) -> Option<f64> {
    (dpi.is_finite() && dpi > 0.0).then_some(px / dpi)
}

/// `px` pixels in centimetres at `dpi`. `None` when `dpi` is not a positive finite number.
pub fn px_to_cm(px: f64, dpi: f64) -> Option<f64> {
    px_to_inches(px, dpi).map(|inches| inches * 2.54)
}
