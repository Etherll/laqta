//! Adjustment effects. Parameters are data here; the kernels
//! are in `laqta-imaging::effects::adjust`.

use crate::color::Rgba;

/// One adjustment and its parameters. Ranges are ShareX's, and defaults are too where ShareX
/// sets them; the rest are chosen here.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Adjust {
    /// Scales alpha by `alpha` and adds `add`, both in 0..1. Colour is unchanged.
    Alpha { alpha: f32, add: f32 },
    /// Luma pushed to high contrast, so every pixel is near black or near white.
    BlackWhite,
    /// Adds `value / 100` to each colour channel. -100..100.
    Brightness { value: f32 },
    /// Tints toward `color`; `value` is the strength in percent, 0..100.
    Colorize { color: Rgba, value: f32 },
    /// Scales around mid grey. -100..100.
    Contrast { value: f32 },
    /// Output is `input ^ (1 / value)`, for 0.1..5. Values above 1 brighten.
    Gamma { value: f32 },
    /// Blends toward luma by `value` percent, 0..100.
    Grayscale { value: f32 },
    /// Rotates hue by `angle` degrees, 0..360.
    Hue { angle: f32 },
    /// Inverts each colour channel. Alpha is kept.
    Inverse,
    /// An arbitrary colour matrix. Row-major 5x4: index `4 * i + j`, where `i` is the input (r, g,
    /// b, a, and at `i = 4` the constant 1) and `j` is the output (r, g, b, a). Offsets are in
    /// 0..1 units, so the identity has 1 at indices 0, 5, 10 and 15.
    MatrixColor { matrix: [f32; 20] },
    /// A fixed warm, faded-film matrix.
    Polaroid,
    /// Moves colour away from (or toward) luma. -100 is grey, 0 is unchanged, 100 doubles the
    /// distance from luma.
    Saturation { value: f32 },
    /// Pixels within `tolerance` (Euclidean RGB distance) of `from` are shifted by `to - from`.
    SelectiveColor { from: Rgba, to: Rgba, tolerance: u8 },
    /// Blends toward sepia by `value` percent, 0..100.
    Sepia { value: f32 },
    /// Pixels within `tolerance` of `source` take the RGB of `target`; their alpha is kept. With
    /// `auto_source_from_corner`, `source` is the top-left pixel of the image instead.
    ReplaceColor { source: Rgba, target: Rgba, tolerance: u8, auto_source_from_corner: bool },
}

/// Identity for `MatrixColor`: 1 at indices 0, 5, 10 and 15.
const IDENTITY_MATRIX: [f32; 20] = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0, //
    0.0, 0.0, 0.0, 0.0,
];

impl Adjust {
    /// One of each adjustment with its default parameters, in declaration order.
    pub fn defaults() -> Vec<Adjust> {
        vec![
            Adjust::Alpha { alpha: 1.0, add: 0.0 },
            Adjust::BlackWhite,
            Adjust::Brightness { value: 0.0 },
            Adjust::Colorize { color: Rgba::rgb(255, 0, 0), value: 50.0 },
            Adjust::Contrast { value: 0.0 },
            Adjust::Gamma { value: 1.0 },
            Adjust::Grayscale { value: 100.0 },
            Adjust::Hue { angle: 0.0 },
            Adjust::Inverse,
            Adjust::MatrixColor { matrix: IDENTITY_MATRIX },
            Adjust::Polaroid,
            Adjust::Saturation { value: 0.0 },
            Adjust::SelectiveColor { from: Rgba::rgb(255, 0, 0), to: Rgba::rgb(0, 0, 255), tolerance: 32 },
            Adjust::Sepia { value: 100.0 },
            Adjust::ReplaceColor { source: Rgba::WHITE, target: Rgba::BLACK, tolerance: 20, auto_source_from_corner: false },
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            Adjust::Alpha { .. } => "Alpha",
            Adjust::BlackWhite => "Black & White",
            Adjust::Brightness { .. } => "Brightness",
            Adjust::Colorize { .. } => "Colorize",
            Adjust::Contrast { .. } => "Contrast",
            Adjust::Gamma { .. } => "Gamma",
            Adjust::Grayscale { .. } => "Grayscale",
            Adjust::Hue { .. } => "Hue",
            Adjust::Inverse => "Inverse",
            Adjust::MatrixColor { .. } => "Matrix color",
            Adjust::Polaroid => "Polaroid",
            Adjust::Saturation { .. } => "Saturation",
            Adjust::SelectiveColor { .. } => "Selective color",
            Adjust::Sepia { .. } => "Sepia",
            Adjust::ReplaceColor { .. } => "Replace color",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::effects::Effect;

    #[test]
    fn defaults_round_trip_through_serde() {
        for adj in Adjust::defaults() {
            let json = serde_json::to_string(&adj).unwrap();
            assert_eq!(serde_json::from_str::<Adjust>(&json).unwrap(), adj, "{json}");
            let effect = Effect::Adjust(adj);
            let json = serde_json::to_string(&effect).unwrap();
            assert_eq!(serde_json::from_str::<Effect>(&json).unwrap(), effect, "{json}");
        }
    }

    #[test]
    fn defaults_cover_every_variant_with_a_unique_name() {
        let mut names: Vec<&str> = Adjust::defaults().iter().map(Adjust::name).collect();
        assert_eq!(names.len(), 15);
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 15);
    }
}
