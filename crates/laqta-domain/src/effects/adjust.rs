//! Adjustment effects. Parameters are data here; the kernels
//! are in `laqta-imaging::effects::adjust`.

use crate::color::Rgba;

/// One colour channel, as an input for `ChannelSwap`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Channel {
    Red,
    Green,
    Blue,
}

/// One adjustment and its parameters. Ranges match the values in `.sxie` packages; the
/// defaults are chosen here.
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
    /// Stretches the luma histogram to the full range. `clip` is the fraction of pixels cut at each
    /// end (0..0.5), so a few outliers don't set the range. Every channel uses one mapping, so hue holds.
    AutoContrast { clip: f32 },
    /// Output `red`, `green` and `blue` each take one of the input channels.
    ChannelSwap { red: Channel, green: Channel, blue: Channel },
    /// Local midtone contrast: `strength` (0..1) scales the detail above a blur of `radius` pixels.
    Clarity { strength: f32, radius: u32 },
    /// Offsets (-1..1) per RGB channel for shadows, midtones and highlights, weighted by luma.
    ColorBalance { shadows: [f32; 3], midtones: [f32; 3], highlights: [f32; 3] },
    /// A cross-processed film look: per-channel S-curves with a colour cast. `strength` is 0..1.
    CrossProcess { strength: f32 },
    /// Removes haze with the dark channel prior. `strength` (0..1) is how much haze goes, and
    /// `radius` is the patch size in pixels.
    Dehaze { strength: f32, radius: u32 },
    /// Maps luma onto a gradient from `shadow` to `highlight`, blended over the image by `strength`.
    DuotoneGradientMap { shadow: Rgba, highlight: Rgba, strength: f32 },
    /// Multiplies linear light by 2^stops, for -5..5.
    Exposure { stops: f32 },
    /// A film stock look: lifted blacks, a soft top end, a colour cast and grain. `grain` (0..1)
    /// sets the grain amount and `seed` its pattern. `strength` (0..1) blends the result over the image.
    FilmEmulation { strength: f32, grain: f32, seed: u64 },
    /// Input black and white, gamma, and output black and white, on 0..255 levels.
    Levels { in_black: u8, in_white: u8, gamma: f32, out_black: u8, out_white: u8 },
    /// Saturation up, an S-curve and dark corners. `strength` is 0..1.
    Lomo { strength: f32 },
    /// Lifts shadows and pulls highlights down, judged against the blurred luma within `radius`.
    /// Both strengths are 0..1.
    ShadowsHighlights { shadows: f32, highlights: f32, radius: u32 },
    /// Channel values above `threshold` are inverted.
    Solarize { threshold: u8 },
    /// Tints shadows toward `shadow` and highlights toward `highlight` by `strength` (0..1). `balance`
    /// (-1..1) moves the split between them.
    SplitToning { shadow: Rgba, highlight: Rgba, strength: f32, balance: f32 },
    /// White balance: `temperature` from -1 (cool) to 1 (warm), `tint` from -1 (magenta) to 1 (green).
    TemperatureTint { temperature: f32, tint: f32 },
    /// Pixels with luma at or above `level` become white, the rest black.
    Threshold { level: u8 },
    /// Saturates the least saturated pixels most. `value` is 0..1.
    Vibrance { value: f32 },
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
            Adjust::AutoContrast { clip: 0.005 },
            Adjust::ChannelSwap { red: Channel::Blue, green: Channel::Green, blue: Channel::Red },
            Adjust::Clarity { strength: 0.5, radius: 8 },
            Adjust::ColorBalance { shadows: [0.0, 0.0, 0.1], midtones: [0.0; 3], highlights: [0.1, 0.05, 0.0] },
            Adjust::CrossProcess { strength: 0.8 },
            Adjust::Dehaze { strength: 0.7, radius: 7 },
            Adjust::DuotoneGradientMap { shadow: Rgba::rgb(20, 20, 60), highlight: Rgba::rgb(255, 220, 140), strength: 1.0 },
            Adjust::Exposure { stops: 0.5 },
            Adjust::FilmEmulation { strength: 0.7, grain: 0.3, seed: 1 },
            Adjust::Levels { in_black: 16, in_white: 235, gamma: 1.0, out_black: 0, out_white: 255 },
            Adjust::Lomo { strength: 0.7 },
            Adjust::ShadowsHighlights { shadows: 0.3, highlights: 0.3, radius: 8 },
            Adjust::Solarize { threshold: 128 },
            Adjust::SplitToning { shadow: Rgba::rgb(40, 80, 140), highlight: Rgba::rgb(255, 200, 120), strength: 0.5, balance: 0.0 },
            Adjust::TemperatureTint { temperature: 0.2, tint: 0.0 },
            Adjust::Threshold { level: 128 },
            Adjust::Vibrance { value: 0.5 },
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
            Adjust::AutoContrast { .. } => "Auto contrast",
            Adjust::ChannelSwap { .. } => "Channel swap",
            Adjust::Clarity { .. } => "Clarity",
            Adjust::ColorBalance { .. } => "Color balance",
            Adjust::CrossProcess { .. } => "Cross process",
            Adjust::Dehaze { .. } => "Dehaze",
            Adjust::DuotoneGradientMap { .. } => "Duotone gradient map",
            Adjust::Exposure { .. } => "Exposure",
            Adjust::FilmEmulation { .. } => "Film emulation",
            Adjust::Levels { .. } => "Levels",
            Adjust::Lomo { .. } => "Lomo",
            Adjust::ShadowsHighlights { .. } => "Shadows & highlights",
            Adjust::Solarize { .. } => "Solarize",
            Adjust::SplitToning { .. } => "Split toning",
            Adjust::TemperatureTint { .. } => "Temperature & tint",
            Adjust::Threshold { .. } => "Threshold",
            Adjust::Vibrance { .. } => "Vibrance",
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
        assert_eq!(names.len(), 32);
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 32);
    }
}
