//! The 27 classic filters as data. Kernels live in `laqta-imaging::effects::filter`.
//!
//! Parameters are clamped to their usable range by the kernels rather than rejected, so a preset
//! from a live preview or a `.sxie` file always renders. Angles are degrees; offsets and sizes are
//! pixels; `0..=1` fields are fractions.

use crate::color::Rgba;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Filter {
    /// Box blur in three passes (close to a Gaussian). `radius` 0 is identity; larger radii are capped at 1024.
    Blur {
        radius: u32,
    },
    /// Keeps `bits` per channel, 1..=8.
    ColorDepth {
        bits: u8,
    },
    /// 3x3 convolution, row-major from the top left: `sum / divisor + offset`. A divisor of 0 counts as 1.
    ConvolutionMatrix {
        kernel: [f32; 9],
        divisor: f32,
        offset: f32,
    },
    EdgeDetect,
    Emboss,
    /// Gaussian blur with standard deviation `sigma` (capped at 64). `sigma` of 0 or less is identity.
    GaussianBlur {
        sigma: f32,
    },
    /// Blurred halo of the alpha outline, drawn under the image and shifted by the offset. `size` is the
    /// blur radius and `strength` the gain. The canvas grows to hold the halo, which reaches `3 * size`
    /// past the image before the offset is applied.
    Glow {
        size: u32,
        strength: f32,
        color: Rgba,
        offset_x: i32,
        offset_y: i32,
    },
    MeanRemoval,
    /// A ring `size` pixels wide around the opaque shape, on a canvas padded by `size + padding` on
    /// every side. `outline_only` drops the image and keeps just the ring.
    Outline {
        size: u32,
        padding: u32,
        color: Rgba,
        outline_only: bool,
    },
    /// Averages `size` x `size` blocks. `size` 1 is identity.
    Pixelate {
        size: u32,
    },
    /// A flipped copy of the bottom `percentage` (0..=100) of the image, placed under it after `offset`
    /// transparent rows, fading from `max_alpha` to `min_alpha`. With `skew` the copy leans right by
    /// `skew_size` pixels at its far end, and the canvas widens by `skew_size`.
    /// Height grows by `offset + round(height * percentage / 100)`.
    Reflection {
        percentage: u8,
        max_alpha: u8,
        min_alpha: u8,
        offset: u32,
        skew: bool,
        skew_size: u32,
    },
    /// Moves red, green and blue by their own (x, y) offsets.
    RgbSplit {
        offset_red: (i32, i32),
        offset_green: (i32, i32),
        offset_blue: (i32, i32),
    },
    /// Blurred shadow of the alpha outline. `opacity` is 0..=1, `size` the blur radius, and `darkness`
    /// 0..=1 mixes `color` toward black. The shadow reaches `3 * size` past the image before the offset
    /// is applied. With `auto_resize` the canvas becomes the union of the image and the shadow; without
    /// it the canvas stays the same size and the shadow is clipped.
    Shadow {
        opacity: f32,
        size: u32,
        darkness: f32,
        color: Rgba,
        offset_x: i32,
        offset_y: i32,
        auto_resize: bool,
    },
    Sharpen,
    /// Horizontal strips, each `min_height..=max_height` tall (at least 1), shifted left or right by
    /// `min_shift..=max_shift` pixels. Pixels shifted in from outside the image are transparent.
    Slice {
        min_height: u32,
        max_height: u32,
        min_shift: u32,
        max_shift: u32,
        seed: u64,
    },
    Smooth,
    /// Torn edge on the chosen sides. Each side cuts up to `depth` pixels, with random cut points every
    /// `range` pixels. `curved` interpolates smoothly between them instead of straight segments.
    TornEdge {
        depth: u32,
        range: u32,
        top: bool,
        right: bool,
        bottom: bool,
        left: bool,
        curved: bool,
        seed: u64,
    },
    /// Sine-wave edge on the chosen sides, cutting up to `depth` pixels with a period of `range` pixels.
    WaveEdge {
        depth: u32,
        range: u32,
        top: bool,
        right: bool,
        bottom: bool,
        left: bool,
    },
    /// Adds uniform noise in `-amount..=amount` to each colour channel.
    Noise {
        amount: u8,
        seed: u64,
    },
    /// Quantises each channel to `levels` evenly spaced values, 2..=255.
    Posterize {
        levels: u8,
    },
    /// Blends toward `color` toward the corners. `radius` is where the falloff starts (0..=1 of the
    /// half-diagonal) and `strength` how far it goes (0..=1).
    Vignette {
        strength: f32,
        radius: f32,
        color: Rgba,
    },
    /// Each `cell` x `cell` block becomes a dot of its average colour on white. Dot area follows darkness.
    Halftone {
        cell: u32,
    },
    /// Floyd–Steinberg error diffusion to `levels` values per channel, 2..=255.
    Dither {
        levels: u8,
    },
    /// Average along a line `distance` pixels long (capped at 256) at `angle` degrees (0 is horizontal).
    MotionBlur {
        distance: u32,
        angle: f32,
    },
    /// Blurs toward a centre at (`center_x`, `center_y`), given as fractions of the image size. `amount`
    /// 0..=1 is how far along the way to the centre the samples reach.
    ZoomBlur {
        amount: f32,
        center_x: f32,
        center_y: f32,
    },
    /// Median of a `(2 * radius + 1)` square window. `radius` is capped at 10.
    Median {
        radius: u32,
    },
    /// Oil paint: in a `(2 * radius + 1)` square window, the most common of `levels` intensity bins,
    /// averaged. `levels` is 1..=255 and `radius` is capped at 10.
    OilPaint {
        radius: u32,
        levels: u8,
    },
    // ---- X2 ----
    /// Red from a view shifted `parallax` px right, and green and blue from one shifted left, for red and cyan glasses.
    Anaglyph3D {
        parallax: u32,
    },
    /// Horizontal streaks from pixels brighter than `threshold` (0..=1 luma), up to `length` px long (capped at 128),
    /// tinted `color` and added at `strength`.
    AnamorphicLensFlare {
        threshold: f32,
        length: u32,
        strength: f32,
        color: Rgba,
    },
    /// `lines` thin rays from the centre, starting `inner` (0..=1 of the half-diagonal) out. Each ray's width comes
    /// from `seed`. The rays mix the image toward `color` by `strength`.
    AnimeSpeedLines {
        lines: u32,
        inner: f32,
        strength: f32,
        color: Rgba,
        seed: u64,
    },
    /// Concentric bands of `color` and `accent`, `width` px deep, with diamonds on the odd bands. The canvas grows by
    /// `width` on every side.
    ArtDecoBorder {
        width: u32,
        color: Rgba,
        accent: Rgba,
    },
    /// One glyph per `cell` px square, chosen by brightness, in `color` on black.
    ASCIIArt {
        cell: u32,
        color: Rgba,
    },
    /// Light on the top and left, shade on the bottom and right, over the outer `size` px. `strength` 0..=1.
    Bevel {
        size: u32,
        strength: f32,
    },
    /// Copies `count` random blocks to a place up to `max_shift` px to the side.
    BlockGlitch {
        count: u32,
        max_shift: u32,
        seed: u64,
    },
    /// `count` blood splats in `color`, each `size` (0..=1 of the shorter side) across, with drips.
    BloodSplash {
        count: u32,
        size: f32,
        color: Rgba,
        seed: u64,
    },
    /// Blurred copy of the parts brighter than `threshold`, `radius` px, added back at `strength`.
    Bloom {
        threshold: f32,
        radius: u32,
        strength: f32,
    },
    /// Edges drawn in `line` over a `background` with a faint grid.
    BlueprintDrawing {
        background: Rgba,
        line: Rgba,
    },
    /// Running-bond brick frame in `brick` with `mortar` lines, `width` px deep.
    BrickWallBorder {
        width: u32,
        brick: Rgba,
        mortar: Rgba,
    },
    /// Diagonal stripes of `color` and `accent`, `width` px deep.
    CandyCaneBorder {
        width: u32,
        color: Rgba,
        accent: Rgba,
    },
    /// Posterised to `levels`, with dark edge lines, on a `color` sticker outline `outline` px wide.
    CartoonStickerCutout {
        levels: u8,
        outline: u32,
        color: Rgba,
    },
    /// Interlaced ribbon frame in `color` and `accent`, `width` px deep.
    CelticKnotBorder {
        width: u32,
        color: Rgba,
        accent: Rgba,
    },
    /// Interlocking links in `color` on an `accent` ground, `width` px deep.
    ChainLinkBorder {
        width: u32,
        color: Rgba,
        accent: Rgba,
    },
    /// Edges as chalk strokes on a `board`. Chalk lies where an edge is stronger than `threshold` (0..=1).
    ChalkDrawing {
        threshold: f32,
        board: Rgba,
        chalk: Rgba,
        seed: u64,
    },
    /// Red pushed outward and blue pulled inward, growing toward the edges. `amount` 0..=1.
    ChromaticAberration {
        amount: f32,
    },
    /// Brushed metal frame in `color`, `width` px deep.
    ChromeMetallicBorder {
        width: u32,
        color: Rgba,
    },
    /// Circuit board frame on a `board` ground, with `trace` lines and pads, `width` px deep.
    CircuitBoardBorder {
        width: u32,
        board: Rgba,
        trace: Rgba,
    },
    /// Soft, blurred colour lit from the top left. `radius` px of softness, `depth` the relief.
    ClaymationTexture {
        radius: u32,
        depth: f32,
    },
    /// Fractal noise clouds on a `scale` px lattice, mixed toward `color` by `opacity` (0..=1).
    Clouds {
        scale: u32,
        opacity: f32,
        color: Rgba,
        seed: u64,
    },
    /// Three rotated dot screens (cyan, magenta, yellow) of `cell` px.
    ColorHalftone {
        cell: u32,
    },
    /// Keeps colours within `tolerance` (0..=1 of the RGB distance) of `target`; the rest goes grey.
    ColorSplash {
        target: Rgba,
        tolerance: f32,
    },
    /// Crimson double-rule frame with a dotted middle line, `width` px deep.
    CrimsonBorder {
        width: u32,
        color: Rgba,
        accent: Rgba,
    },
    /// Pen hatching on white. Darker areas get more layers; the lines are `spacing` px apart.
    Crosshatch {
        spacing: u32,
    },
    /// Scanlines, a phosphor sub-pixel mask and corner vignetting, each 0..=1.
    CRT {
        scanlines: f32,
        mask: f32,
        vignette: f32,
    },
    /// Faceted triangles over a jittered grid of `size` px cells, each lit at a seeded brightness.
    CrystalizeShards {
        size: u32,
        seed: u64,
    },
    /// The shards with red and blue slid apart along each facet by `dispersion` (0..=1).
    CrystalPrism {
        size: u32,
        dispersion: f32,
        seed: u64,
    },
    /// Two-colour blue print, from `shadow` to `highlight`.
    Cyanotype {
        shadow: Rgba,
        highlight: Rgba,
    },
    /// About half of the `block` px cells drag their pixels sideways by up to `max_shift` px.
    DatamoshSmear {
        block: u32,
        max_shift: u32,
        seed: u64,
    },
    /// Average colour in diamond-shaped cells about `size` px across.
    DiamondPixelate {
        size: u32,
    },
    /// Warm cast, grain and vignetting, each 0..=1.
    DisposableCamera {
        warmth: f32,
        grain: f32,
        vignette: f32,
        seed: u64,
    },
    /// LED panel: a dot of average colour in each `cell` px square, the gaps in `background`.
    DotMatrix {
        cell: u32,
        background: Rgba,
    },
    /// A copy mirrored left to right and moved by the offset, screened over the image at `opacity` (0..=1).
    DoubleExposure {
        offset_x: i32,
        offset_y: i32,
        opacity: f32,
    },
    /// A box-blurred copy, `radius` px, screened over the image at `strength`.
    DreamGlow {
        radius: u32,
        strength: f32,
    },
    /// Alpha fades to nothing over the outer `size` px.
    EdgeFeather {
        size: u32,
    },
    /// Cross stitches `cell` px across, each in the cell's average colour, on `cloth`.
    Embroidery {
        cell: u32,
        cloth: Rgba,
    },
    /// Horizontal lines `spacing` px apart, drawn in `ink` on `paper`, swelling with darkness.
    Engraving {
        spacing: u32,
        paper: Rgba,
        ink: Rgba,
    },
    /// Each pixel takes a neighbour's colour from within `radius` px, lifted toward white.
    EtchedGlass {
        radius: u32,
        seed: u64,
    },
    /// `strands` curling strands of `length` px in `color`, drawn over the image.
    FakeHair {
        strands: u32,
        length: u32,
        color: Rgba,
        seed: u64,
    },
    /// Monochrome grain of up to `amount` (0..=1), strongest in the midtones.
    FilmGrain {
        amount: f32,
        seed: u64,
    },
    // ---- end X2 ----
    // ---- X3 ----
    // ---- end X3 ----
    // ---- X4 ----
    // ---- end X4 ----
}

impl Filter {
    pub fn name(&self) -> &'static str {
        match self {
            Filter::Blur { .. } => "Blur",
            Filter::ColorDepth { .. } => "Color depth",
            Filter::ConvolutionMatrix { .. } => "Convolution matrix",
            Filter::EdgeDetect => "Edge detect",
            Filter::Emboss => "Emboss",
            Filter::GaussianBlur { .. } => "Gaussian blur",
            Filter::Glow { .. } => "Glow",
            Filter::MeanRemoval => "Mean removal",
            Filter::Outline { .. } => "Outline",
            Filter::Pixelate { .. } => "Pixelate",
            Filter::Reflection { .. } => "Reflection",
            Filter::RgbSplit { .. } => "RGB split",
            Filter::Shadow { .. } => "Shadow",
            Filter::Sharpen => "Sharpen",
            Filter::Slice { .. } => "Slice",
            Filter::Smooth => "Smooth",
            Filter::TornEdge { .. } => "Torn edge",
            Filter::WaveEdge { .. } => "Wave edge",
            Filter::Noise { .. } => "Noise",
            Filter::Posterize { .. } => "Posterize",
            Filter::Vignette { .. } => "Vignette",
            Filter::Halftone { .. } => "Halftone",
            Filter::Dither { .. } => "Dither",
            Filter::MotionBlur { .. } => "Motion blur",
            Filter::ZoomBlur { .. } => "Zoom blur",
            Filter::Median { .. } => "Median",
            Filter::OilPaint { .. } => "Oil paint",
            // ---- X2 ----
            Filter::Anaglyph3D { .. } => "Anaglyph 3D",
            Filter::AnamorphicLensFlare { .. } => "Anamorphic lens flare",
            Filter::AnimeSpeedLines { .. } => "Anime speed lines",
            Filter::ArtDecoBorder { .. } => "Art deco border",
            Filter::ASCIIArt { .. } => "ASCII art",
            Filter::Bevel { .. } => "Bevel",
            Filter::BlockGlitch { .. } => "Block glitch",
            Filter::BloodSplash { .. } => "Blood splash",
            Filter::Bloom { .. } => "Bloom",
            Filter::BlueprintDrawing { .. } => "Blueprint drawing",
            Filter::BrickWallBorder { .. } => "Brick wall border",
            Filter::CandyCaneBorder { .. } => "Candy cane border",
            Filter::CartoonStickerCutout { .. } => "Cartoon sticker cutout",
            Filter::CelticKnotBorder { .. } => "Celtic knot border",
            Filter::ChainLinkBorder { .. } => "Chain link border",
            Filter::ChalkDrawing { .. } => "Chalk drawing",
            Filter::ChromaticAberration { .. } => "Chromatic aberration",
            Filter::ChromeMetallicBorder { .. } => "Chrome metallic border",
            Filter::CircuitBoardBorder { .. } => "Circuit board border",
            Filter::ClaymationTexture { .. } => "Claymation texture",
            Filter::Clouds { .. } => "Clouds",
            Filter::ColorHalftone { .. } => "Color halftone",
            Filter::ColorSplash { .. } => "Color splash",
            Filter::CrimsonBorder { .. } => "Crimson border",
            Filter::Crosshatch { .. } => "Crosshatch",
            Filter::CRT { .. } => "CRT",
            Filter::CrystalizeShards { .. } => "Crystalize shards",
            Filter::CrystalPrism { .. } => "Crystal prism",
            Filter::Cyanotype { .. } => "Cyanotype",
            Filter::DatamoshSmear { .. } => "Datamosh smear",
            Filter::DiamondPixelate { .. } => "Diamond pixelate",
            Filter::DisposableCamera { .. } => "Disposable camera",
            Filter::DotMatrix { .. } => "Dot matrix",
            Filter::DoubleExposure { .. } => "Double exposure",
            Filter::DreamGlow { .. } => "Dream glow",
            Filter::EdgeFeather { .. } => "Edge feather",
            Filter::Embroidery { .. } => "Embroidery",
            Filter::Engraving { .. } => "Engraving",
            Filter::EtchedGlass { .. } => "Etched glass",
            Filter::FakeHair { .. } => "Fake hair",
            Filter::FilmGrain { .. } => "Film grain",
            // ---- end X2 ----
            // ---- X3 ----
            // ---- end X3 ----
            // ---- X4 ----
            // ---- end X4 ----
        }
    }

    /// One filter per variant, with ShareX-like default parameters.
    pub fn defaults() -> Vec<Filter> {
        vec![
            Filter::Blur { radius: 5 },
            Filter::ColorDepth { bits: 4 },
            Filter::ConvolutionMatrix { kernel: [0.0, -1.0, 0.0, -1.0, 5.0, -1.0, 0.0, -1.0, 0.0], divisor: 1.0, offset: 0.0 },
            Filter::EdgeDetect,
            Filter::Emboss,
            Filter::GaussianBlur { sigma: 2.0 },
            Filter::Glow { size: 10, strength: 2.0, color: Rgba::WHITE, offset_x: 0, offset_y: 0 },
            Filter::MeanRemoval,
            Filter::Outline { size: 5, padding: 10, color: Rgba::BLACK, outline_only: false },
            Filter::Pixelate { size: 10 },
            Filter::Reflection { percentage: 50, max_alpha: 150, min_alpha: 0, offset: 0, skew: false, skew_size: 0 },
            Filter::RgbSplit { offset_red: (-4, 0), offset_green: (0, 0), offset_blue: (4, 0) },
            Filter::Shadow { opacity: 0.8, size: 10, darkness: 0.0, color: Rgba::BLACK, offset_x: 10, offset_y: 10, auto_resize: true },
            Filter::Sharpen,
            Filter::Slice { min_height: 2, max_height: 10, min_shift: 2, max_shift: 12, seed: 1 },
            Filter::Smooth,
            Filter::TornEdge { depth: 8, range: 12, top: true, right: true, bottom: true, left: true, curved: false, seed: 1 },
            Filter::WaveEdge { depth: 6, range: 16, top: true, right: false, bottom: true, left: false },
            Filter::Noise { amount: 24, seed: 1 },
            Filter::Posterize { levels: 4 },
            Filter::Vignette { strength: 0.6, radius: 0.5, color: Rgba::BLACK },
            Filter::Halftone { cell: 6 },
            Filter::Dither { levels: 2 },
            Filter::MotionBlur { distance: 12, angle: 0.0 },
            Filter::ZoomBlur { amount: 0.3, center_x: 0.5, center_y: 0.5 },
            Filter::Median { radius: 2 },
            Filter::OilPaint { radius: 3, levels: 16 },
            // ---- X2 ----
            Filter::Anaglyph3D { parallax: 3 },
            Filter::AnamorphicLensFlare { threshold: 0.8, length: 40, strength: 0.6, color: Rgba::rgb(120, 180, 255) },
            Filter::AnimeSpeedLines { lines: 72, inner: 0.35, strength: 0.6, color: Rgba::WHITE, seed: 1 },
            Filter::ArtDecoBorder { width: 24, color: Rgba::rgb(212, 175, 55), accent: Rgba::rgb(20, 20, 20) },
            Filter::ASCIIArt { cell: 8, color: Rgba::rgb(120, 255, 120) },
            Filter::Bevel { size: 8, strength: 0.5 },
            Filter::BlockGlitch { count: 6, max_shift: 12, seed: 1 },
            Filter::BloodSplash { count: 4, size: 0.2, color: Rgba::rgb(138, 3, 3), seed: 1 },
            Filter::Bloom { threshold: 0.7, radius: 6, strength: 0.8 },
            Filter::BlueprintDrawing { background: Rgba::rgb(20, 60, 130), line: Rgba::rgb(230, 240, 255) },
            Filter::BrickWallBorder { width: 24, brick: Rgba::rgb(170, 70, 50), mortar: Rgba::rgb(200, 195, 185) },
            Filter::CandyCaneBorder { width: 16, color: Rgba::rgb(200, 30, 40), accent: Rgba::WHITE },
            Filter::CartoonStickerCutout { levels: 5, outline: 8, color: Rgba::WHITE },
            Filter::CelticKnotBorder { width: 24, color: Rgba::rgb(40, 110, 70), accent: Rgba::rgb(230, 200, 90) },
            Filter::ChainLinkBorder { width: 24, color: Rgba::rgb(170, 175, 180), accent: Rgba::rgb(60, 60, 65) },
            Filter::ChalkDrawing { threshold: 0.25, board: Rgba::rgb(40, 66, 56), chalk: Rgba::rgb(240, 240, 232), seed: 1 },
            Filter::ChromaticAberration { amount: 0.5 },
            Filter::ChromeMetallicBorder { width: 16, color: Rgba::rgb(210, 215, 225) },
            Filter::CircuitBoardBorder { width: 20, board: Rgba::rgb(10, 60, 30), trace: Rgba::rgb(200, 160, 60) },
            Filter::ClaymationTexture { radius: 2, depth: 2.0 },
            Filter::Clouds { scale: 16, opacity: 0.6, color: Rgba::WHITE, seed: 1 },
            Filter::ColorHalftone { cell: 6 },
            Filter::ColorSplash { target: Rgba::rgb(220, 40, 40), tolerance: 0.25 },
            Filter::CrimsonBorder { width: 20, color: Rgba::rgb(140, 20, 40), accent: Rgba::rgb(220, 180, 120) },
            Filter::Crosshatch { spacing: 4 },
            Filter::CRT { scanlines: 0.35, mask: 0.3, vignette: 0.5 },
            Filter::CrystalizeShards { size: 8, seed: 1 },
            Filter::CrystalPrism { size: 8, dispersion: 0.5, seed: 1 },
            Filter::Cyanotype { shadow: Rgba::rgb(12, 45, 94), highlight: Rgba::rgb(240, 246, 255) },
            Filter::DatamoshSmear { block: 16, max_shift: 8, seed: 1 },
            Filter::DiamondPixelate { size: 8 },
            Filter::DisposableCamera { warmth: 0.5, grain: 0.3, vignette: 0.5, seed: 1 },
            Filter::DotMatrix { cell: 6, background: Rgba::rgb(10, 10, 10) },
            Filter::DoubleExposure { offset_x: 6, offset_y: 0, opacity: 0.5 },
            Filter::DreamGlow { radius: 6, strength: 0.6 },
            Filter::EdgeFeather { size: 6 },
            Filter::Embroidery { cell: 6, cloth: Rgba::rgb(235, 225, 205) },
            Filter::Engraving { spacing: 3, paper: Rgba::rgb(245, 238, 220), ink: Rgba::rgb(30, 25, 20) },
            Filter::EtchedGlass { radius: 2, seed: 1 },
            Filter::FakeHair { strands: 40, length: 30, color: Rgba::rgb(40, 25, 15), seed: 1 },
            Filter::FilmGrain { amount: 0.25, seed: 1 },
            // ---- end X2 ----
            // ---- X3 ----
            // ---- end X3 ----
            // ---- X4 ----
            // ---- end X4 ----
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::effects::Effect;

    #[test]
    fn defaults_have_one_distinct_name_per_variant() {
        let defaults = Filter::defaults();
        assert_eq!(defaults.len(), 27);
        let names: HashSet<&str> = defaults.iter().map(Filter::name).collect();
        assert_eq!(names.len(), 27);
    }

    #[test]
    fn defaults_survive_a_serde_round_trip_bare_and_wrapped() {
        for filter in Filter::defaults() {
            let json = serde_json::to_string(&filter).expect("filter serializes");
            let back: Filter = serde_json::from_str(&json).expect("filter deserializes");
            assert_eq!(back, filter, "{json}");

            let wrapped = Effect::Filter(filter.clone());
            let json = serde_json::to_string(&wrapped).expect("effect serializes");
            let back: Effect = serde_json::from_str(&json).expect("effect deserializes");
            assert_eq!(back, wrapped, "{json}");
        }
    }
}
