//! The image filters as data. Kernels live in `laqta-imaging::effects::filter`.
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
    /// Sprocket holes in `hole` down the left and right margins of a `strip` frame, `width` px deep.
    FilmStripFrame {
        width: u32,
        strip: Rgba,
        hole: Rgba,
    },
    /// Each pixel averages samples scattered within `radius` px, then clouds toward white by `opacity` (0..=1).
    FrostedGlass {
        radius: u32,
        opacity: f32,
        seed: u64,
    },
    /// Frosted glass, with `ice` crystals within `edge` px of the border that grow inward.
    FrostedGlassIceEdges {
        radius: u32,
        edge: u32,
        ice: Rgba,
        seed: u64,
    },
    /// A `base` frame with `ice` crystals growing in from the outer edge, `width` px deep.
    FrostIceBorder {
        width: u32,
        ice: Rgba,
        base: Rgba,
        seed: u64,
    },
    /// `copies` faded copies, each `offset` px further on, trail behind the image. `opacity` is the nearest copy's strength.
    GhostTrail {
        copies: u32,
        offset_x: i32,
        offset_y: i32,
        opacity: f32,
    },
    /// Pixels brighter than `threshold` (0..=1 luma) streak along eight rays `length` px long (capped at 48), added at `strength`.
    Glare {
        threshold: f32,
        length: u32,
        strength: f32,
    },
    /// `bands` random horizontal bands shifted up to `max_shift` px, with red and blue split further than green.
    GlitchShift {
        bands: u32,
        max_shift: u32,
        seed: u64,
    },
    /// A `color` frame with dark rules and a bright sheen, `width` px deep.
    GoldenBorder {
        width: u32,
        color: Rgba,
    },
    /// The margin fades from `color` into the image beside it, `width` px deep.
    GradientFadeBorder {
        width: u32,
        color: Rgba,
    },
    /// Bright parts (above `threshold`, 0..=1 luma) spill a red-orange halo `radius` px wide, added at `strength`.
    Halation {
        threshold: f32,
        radius: u32,
        strength: f32,
    },
    /// Rows wobble sideways by up to `amplitude` px (capped at 64), with a wavelength of `wavelength` px.
    HeatHazeRefraction {
        amplitude: u32,
        wavelength: u32,
    },
    /// Luma mapped to a thermal palette, from black through red and yellow to white.
    HeatMap,
    /// Average colour in flat-top hexagons of circumradius `size` px.
    HexagonalPixelate {
        size: u32,
    },
    /// Monochrome in `tint`, with scanlines every `line_spacing` rows and a roll band, mixed in at `strength`.
    HologramScan {
        line_spacing: u32,
        strength: f32,
        tint: Rgba,
    },
    /// Diagonal rainbow bands, `period` px per cycle, multiplied in at `strength` (0..=1).
    HolographicFoilShimmer {
        strength: f32,
        period: u32,
    },
    /// Near-infrared false colour: foliage turns red and the sky purple. `strength` 0..=1 mixes from the original.
    InfraredFalseColor {
        strength: f32,
    },
    /// `count` ink splats in `color`, each `size` (0..=1 of the shorter side) across, with droplets and drips.
    InkSplatterDrips {
        count: u32,
        size: f32,
        color: Rgba,
        seed: u64,
    },
    /// The inside edge of the shape darkens to `color`, lit from the offset. `opacity` 0..=1, `size` px blur.
    InnerShadow {
        opacity: f32,
        size: u32,
        color: Rgba,
        offset_x: i32,
        offset_y: i32,
    },
    /// 8x8 DCT block quantisation at `quality` (1..=100, where 100 is near lossless): blocky and ringing.
    JpegArtifact {
        quality: u8,
    },
    /// A `cool` crust frame with `hot` molten cracks, `width` px deep.
    LavaMoltenBorder {
        width: u32,
        hot: Rgba,
        cool: Rgba,
        seed: u64,
    },
    /// Disc blur of `radius` px (capped at 24). Bright taps count for more by `highlights` (up to 16).
    LensBlur {
        radius: u32,
        highlights: f32,
    },
    /// A `color` glow from a point (`center_x`, `center_y` as 0..=1 of the image) falling off over `radius`
    /// (0..=1 of the diagonal), screened over the image at `strength`.
    LightLeak {
        center_x: f32,
        center_y: f32,
        radius: f32,
        strength: f32,
        color: Rgba,
    },
    /// The image seen through a wavy pane: up to `strength` px of displacement over `scale` px of noise.
    LiquidGlass {
        strength: u32,
        scale: u32,
        seed: u64,
    },
    /// The image as a silver liquid with ripples `scale` px across, lit from the top left.
    LiquidMercury {
        scale: u32,
        seed: u64,
    },
    /// Lines in `color` where luma crosses one of `levels` bands (2..=64), over a washed-out image.
    LuminanceContourLines {
        levels: u8,
        color: Rgba,
    },
    /// A `bar_color` title bar `bar` px tall with three buttons, and rounded corners `radius` px across.
    MacOSWindow {
        bar: u32,
        radius: u32,
        bar_color: Rgba,
    },
    /// A marble frame of `base` with `vein` veins, `width` px deep.
    MarbleBorder {
        width: u32,
        base: Rgba,
        vein: Rgba,
        seed: u64,
    },
    /// Falling glyph columns `column` px wide (3..=64) in `color` over a dimmed image, at `strength`.
    MatrixDigitalRain {
        column: u32,
        strength: f32,
        color: Rgba,
        seed: u64,
    },
    /// Image pixelated to `block` px blocks, with a grass row above and a dirt row below.
    MinecraftGrass {
        block: u32,
        seed: u64,
    },
    /// Tilt-shift: rows outside the `focus` band (0..=1 of the height) blur by `radius` px. `band` is the sharp share.
    Miniature {
        focus: f32,
        band: f32,
        radius: u32,
        saturation: f32,
    },
    /// Voronoi cells of about `size` px, each the cell's average colour, with `grout` seams.
    MosaicPolygon {
        size: u32,
        grout: Rgba,
        seed: u64,
    },
    /// A `color` frame of `tile` px squares (2..=4096) with `grout` lines, `width` px deep.
    MosaicTileBorder {
        width: u32,
        tile: u32,
        color: Rgba,
        grout: Rgba,
        seed: u64,
    },
    /// Clouds of `nebula` screened over the image at `strength`, with stars; `density` 0..=1.
    NebulaStarfield {
        density: f32,
        nebula: Rgba,
        strength: f32,
        seed: u64,
    },
    /// Dimmed image with its edges lit in `color`, glowing `radius` px out (capped at 128) at `strength`.
    NeonEdgeGlow {
        color: Rgba,
        strength: f32,
        radius: u32,
    },
    /// A `color` neon tube in a frame `width` px deep, bright at its core.
    NeonGlowBorder {
        width: u32,
        color: Rgba,
    },
    /// Monochrome green with `gain` and `grain` (each 0..=1) and a dark circular vignette.
    NightVision {
        gain: f32,
        grain: f32,
        seed: u64,
    },
    /// Thin-film interference fringes screened over the image at `strength`; `scale` px swirls.
    OilSlickInterference {
        strength: f32,
        scale: u32,
        seed: u64,
    },
    /// A hot centre flash that burns to white, with a sepia cast of `sepia` (0..=1).
    OldCameraFlashBurn {
        strength: f32,
        sepia: f32,
    },
    /// A `color` band `width` px deep with a repeating scroll of `accent` rings and a rail.
    OrnateScrollBorder {
        width: u32,
        color: Rgba,
        accent: Rgba,
    },
    /// Luma mapped to a three-stop gradient: `shadow`, then `midtone`, then `highlight`.
    PaletteMap {
        shadow: Rgba,
        midtone: Rgba,
        highlight: Rgba,
    },
    /// Dark areas cut out as `ink` from `paper`, with paper tooth `grain` and stencil edges that bend with blotches.
    PaperStencilMask {
        paper: Rgba,
        ink: Rgba,
        threshold: f32,
        grain: f32,
        seed: u64,
    },
    /// Grey colour dodge of the image by its blurred inverse (`radius` px), mixed from white at `strength`.
    PencilSketch {
        strength: f32,
        radius: u32,
    },
    /// Perlin gradient noise on a `scale` px lattice, mixed toward grey by `strength` (0..=1).
    PerlinNoise {
        scale: u32,
        strength: f32,
        seed: u64,
    },
    /// Sorts each run of pixels at least `threshold` (0..=1 luma) bright by luma, along rows or columns.
    PixelSorting {
        threshold: f32,
        vertical: bool,
    },
    /// Arcs of `color` where a sine plasma field `scale` px across crosses zero, mixed in at `strength`.
    PlasmaEnergyArcs {
        scale: u32,
        strength: f32,
        color: Rgba,
        seed: u64,
    },
    /// The image rebuilt from one seeded dot per `cell` px square, on white.
    Pointillism {
        cell: u32,
        seed: u64,
    },
    /// The image on a `color` paper mount, `border` px on three sides and `bottom` px more underneath.
    PolaroidFrame {
        border: u32,
        bottom: u32,
        color: Rgba,
    },
    /// The image in `tiles` x `tiles` panels, each a duotone in its own ink, with white gutters.
    PopArtGrid {
        tiles: u32,
    },
    /// A hue that turns `bands` times around the centre, mixed in at `strength` (0..=1).
    RadialRainbow {
        strength: f32,
        bands: u32,
    },
    /// A `width` px frame whose hue turns once around the image.
    RainbowBorder {
        width: u32,
    },
    /// The image blurred by `blur_radius` px, with `drops` water drops up to `size` px across that bend the image.
    RainyWindow {
        drops: u32,
        size: u32,
        blur_radius: u32,
        seed: u64,
    },
    /// A dusk gradient map (violet, magenta, orange, pale yellow) mixed in at `strength`, with dark scan bars.
    RetroSunset {
        strength: f32,
    },
    /// One `ink` on `paper`, with `grain` (0..=1) of random dropout and the ink plate one px off the paper.
    RisoPrint {
        ink: Rgba,
        paper: Rgba,
        grain: f32,
        seed: u64,
    },
    /// A `width` px frame of twisted rope, strands alternating between `color` and `accent`.
    RopeBorder {
        width: u32,
        color: Rgba,
        accent: Rgba,
    },
    /// Rust-brown patches `scale` px across, where the image dulls by `amount` (0..=1).
    RustCorrosion {
        amount: f32,
        scale: u32,
        seed: u64,
    },
    /// Every `spacing`-th row darkened by `strength` (0..=1).
    Scanline {
        spacing: u32,
        strength: f32,
    },
    /// Warped fractal wisps of grey smoke `scale` px across, screened over the image at `opacity` (0..=1).
    SmokeOverlay {
        opacity: f32,
        scale: u32,
        seed: u64,
    },
    /// Haze that thickens toward the top by `fog` (0..=1), with `flakes` white snowflakes falling across.
    SnowfallDepthFog {
        flakes: u32,
        fog: f32,
        seed: u64,
    },
    /// The gradient strength of the luma, drawn as light on black and scaled by `gain`.
    SobelEdge {
        gain: f32,
    },
    /// Each pixel scatters to a seeded spot up to `radius` px away, mixed in by `strength` (0..=1).
    SoftDiffusion {
        radius: u32,
        strength: f32,
        seed: u64,
    },
    /// Rotational blur about a centre (fractions of the image size), averaged over an arc of `angle` degrees.
    SpinBlur {
        angle: f32,
        center_x: f32,
        center_y: f32,
    },
    /// Voronoi panes in their average colour, with `lead` px lines along the cell borders.
    StainedGlass {
        cell: u32,
        lead: u32,
        seed: u64,
    },
    /// A `width` px frame of glass panes, `pane` px square, with `lead` lines between them.
    StainedGlassBorder {
        width: u32,
        pane: u32,
        lead: Rgba,
        seed: u64,
    },
    /// A `width` px frame of `background` night sky with `star` pinpricks scattered over it.
    StarfieldBorder {
        width: u32,
        background: Rgba,
        star: Rgba,
        seed: u64,
    },
    /// Glints in four directions from pixels brighter than `threshold`, up to `length` px long, tinted `color`.
    StarFilter {
        threshold: f32,
        length: u32,
        strength: f32,
        color: Rgba,
    },
    /// Diagonal stripes `period` px apart, tinted `color` at `opacity` (0..=1).
    StripeOverlay {
        period: u32,
        opacity: f32,
        color: Rgba,
    },
    /// Edge-preserving blur: neighbours whose luma is within `threshold` (0..=255) of the pixel are averaged over `radius` px.
    SurfaceBlur {
        radius: u32,
        threshold: u8,
    },
    /// Luma through a heat palette (violet, red, yellow, white), mixed in at `strength` (0..=1).
    ThermalVision {
        strength: f32,
    },
    /// Blur that grows away from a sharp band `band` (0..=1 of the height) around `focus` (0..=1 down the image).
    TiltShift {
        focus: f32,
        band: f32,
        radius: u32,
    },
    /// Square cells `size` px across, each split along its diagonal and filled with each half's average.
    TrianglePixelate {
        size: u32,
    },
    /// `amount` (0..=1) of the pixels turn to random grey snow.
    TVStatic {
        amount: f32,
        seed: u64,
    },
    /// Adds back `amount` times the difference from a `radius` px box blur, where it is at least `threshold`.
    UnsharpMask {
        radius: u32,
        amount: f32,
        threshold: u8,
    },
    /// Rows jitter sideways by up to `jitter` px, red and blue smear `bleed` px apart, and the bottom band fills with `noise`.
    VhsTapeDamage {
        jitter: u32,
        bleed: u32,
        noise: f32,
        seed: u64,
    },
    /// Fades toward a warm sepia by `fade` (0..=1), with `scratches` light vertical scratches and dust specks.
    VintagePrintDamage {
        fade: f32,
        scratches: u32,
        seed: u64,
    },
    /// Irregular cells about `cell` px across, each the colour under its feature point.
    VoronoiMosaic {
        cell: u32,
        seed: u64,
    },
    /// A `width` px frame of `color` wash, heavier in patches, with a darker pigment line on the inner edge.
    WatercolorBorder {
        width: u32,
        color: Rgba,
        seed: u64,
    },
    /// Kuwahara filter: each pixel takes the mean of the most even of four `radius` px quadrants.
    WatercolorKuwahara {
        radius: u32,
    },
    /// Blue-grey monochrome with high contrast, faint mottling and corners darkened by `vignette` (0..=1).
    WetPlateCollodion {
        vignette: f32,
        seed: u64,
    },
    /// A window with rounded corners `radius` px across and a soft `shadow` px wide. The canvas grows by `shadow`.
    Windows11 {
        radius: u32,
        shadow: u32,
    },
    /// `streaks` pale horizontal streaks, up to `length` px long, blown across the image at `strength` (0..=1).
    WindStreaks {
        streaks: u32,
        length: u32,
        strength: f32,
        seed: u64,
    },
    /// Diagonal gouges `line` px wide, inked where the image is dark: `ink` on `paper`.
    WoodcutPrint {
        line: u32,
        ink: Rgba,
        paper: Rgba,
    },
    /// A `width` px frame of planks in `color`, with wandering grain and a bevel.
    WoodenFrame {
        width: u32,
        color: Rgba,
        seed: u64,
    },
    /// The image as an X-ray plate tinted `tint`, with a bright beam across it at `position` (0..=1 down the image).
    XRayScan {
        position: f32,
        tint: Rgba,
    },
    /// A `width` px frame in `color` whose inner edge runs in triangular zigzags, the rest in `accent`.
    ZigZagBorder {
        width: u32,
        color: Rgba,
        accent: Rgba,
    },
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
            Filter::FilmStripFrame { .. } => "Film strip frame",
            Filter::FrostedGlass { .. } => "Frosted glass",
            Filter::FrostedGlassIceEdges { .. } => "Frosted glass ice edges",
            Filter::FrostIceBorder { .. } => "Frost ice border",
            Filter::GhostTrail { .. } => "Ghost trail",
            Filter::Glare { .. } => "Glare",
            Filter::GlitchShift { .. } => "Glitch shift",
            Filter::GoldenBorder { .. } => "Golden border",
            Filter::GradientFadeBorder { .. } => "Gradient fade border",
            Filter::Halation { .. } => "Halation",
            Filter::HeatHazeRefraction { .. } => "Heat haze refraction",
            Filter::HeatMap => "Heat map",
            Filter::HexagonalPixelate { .. } => "Hexagonal pixelate",
            Filter::HologramScan { .. } => "Hologram scan",
            Filter::HolographicFoilShimmer { .. } => "Holographic foil shimmer",
            Filter::InfraredFalseColor { .. } => "Infrared false color",
            Filter::InkSplatterDrips { .. } => "Ink splatter drips",
            Filter::InnerShadow { .. } => "Inner shadow",
            Filter::JpegArtifact { .. } => "JPEG artifact",
            Filter::LavaMoltenBorder { .. } => "Lava molten border",
            Filter::LensBlur { .. } => "Lens blur",
            Filter::LightLeak { .. } => "Light leak",
            Filter::LiquidGlass { .. } => "Liquid glass",
            Filter::LiquidMercury { .. } => "Liquid mercury",
            Filter::LuminanceContourLines { .. } => "Luminance contour lines",
            Filter::MacOSWindow { .. } => "macOS window",
            Filter::MarbleBorder { .. } => "Marble border",
            Filter::MatrixDigitalRain { .. } => "Matrix digital rain",
            Filter::MinecraftGrass { .. } => "Minecraft grass",
            Filter::Miniature { .. } => "Miniature",
            Filter::MosaicPolygon { .. } => "Mosaic polygon",
            Filter::MosaicTileBorder { .. } => "Mosaic tile border",
            Filter::NebulaStarfield { .. } => "Nebula starfield",
            Filter::NeonEdgeGlow { .. } => "Neon edge glow",
            Filter::NeonGlowBorder { .. } => "Neon glow border",
            Filter::NightVision { .. } => "Night vision",
            Filter::OilSlickInterference { .. } => "Oil slick interference",
            Filter::OldCameraFlashBurn { .. } => "Old camera flash burn",
            Filter::OrnateScrollBorder { .. } => "Ornate scroll border",
            Filter::PaletteMap { .. } => "Palette map",
            Filter::PaperStencilMask { .. } => "Paper stencil mask",
            Filter::PencilSketch { .. } => "Pencil sketch",
            Filter::PerlinNoise { .. } => "Perlin noise",
            Filter::PixelSorting { .. } => "Pixel sorting",
            Filter::PlasmaEnergyArcs { .. } => "Plasma energy arcs",
            Filter::Pointillism { .. } => "Pointillism",
            Filter::PolaroidFrame { .. } => "Polaroid frame",
            Filter::PopArtGrid { .. } => "Pop art grid",
            Filter::RadialRainbow { .. } => "Radial rainbow",
            Filter::RainbowBorder { .. } => "Rainbow border",
            Filter::RainyWindow { .. } => "Rainy window",
            Filter::RetroSunset { .. } => "Retro sunset",
            Filter::RisoPrint { .. } => "Riso print",
            Filter::RopeBorder { .. } => "Rope border",
            Filter::RustCorrosion { .. } => "Rust corrosion",
            Filter::Scanline { .. } => "Scanline",
            Filter::SmokeOverlay { .. } => "Smoke overlay",
            Filter::SnowfallDepthFog { .. } => "Snowfall depth fog",
            Filter::SobelEdge { .. } => "Sobel edge",
            Filter::SoftDiffusion { .. } => "Soft diffusion",
            Filter::SpinBlur { .. } => "Spin blur",
            Filter::StainedGlass { .. } => "Stained glass",
            Filter::StainedGlassBorder { .. } => "Stained glass border",
            Filter::StarfieldBorder { .. } => "Starfield border",
            Filter::StarFilter { .. } => "Star filter",
            Filter::StripeOverlay { .. } => "Stripe overlay",
            Filter::SurfaceBlur { .. } => "Surface blur",
            Filter::ThermalVision { .. } => "Thermal vision",
            Filter::TiltShift { .. } => "Tilt shift",
            Filter::TrianglePixelate { .. } => "Triangle pixelate",
            Filter::TVStatic { .. } => "TV static",
            Filter::UnsharpMask { .. } => "Unsharp mask",
            Filter::VhsTapeDamage { .. } => "VHS tape damage",
            Filter::VintagePrintDamage { .. } => "Vintage print damage",
            Filter::VoronoiMosaic { .. } => "Voronoi mosaic",
            Filter::WatercolorBorder { .. } => "Watercolor border",
            Filter::WatercolorKuwahara { .. } => "Watercolor Kuwahara",
            Filter::WetPlateCollodion { .. } => "Wet plate collodion",
            Filter::Windows11 { .. } => "Windows 11",
            Filter::WindStreaks { .. } => "Wind streaks",
            Filter::WoodcutPrint { .. } => "Woodcut print",
            Filter::WoodenFrame { .. } => "Wooden frame",
            Filter::XRayScan { .. } => "X-ray scan",
            Filter::ZigZagBorder { .. } => "Zigzag border",
        }
    }

    /// One filter per variant, with default parameters.
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
            Filter::FilmStripFrame { width: 24, strip: Rgba::rgb(22, 22, 22), hole: Rgba::rgb(240, 240, 240) },
            Filter::FrostedGlass { radius: 4, opacity: 0.5, seed: 1 },
            Filter::FrostedGlassIceEdges { radius: 3, edge: 24, ice: Rgba::rgb(210, 235, 255), seed: 1 },
            Filter::FrostIceBorder { width: 20, ice: Rgba::rgb(225, 245, 255), base: Rgba::rgb(160, 200, 225), seed: 1 },
            Filter::GhostTrail { copies: 4, offset_x: 6, offset_y: 0, opacity: 0.4 },
            Filter::Glare { threshold: 0.8, length: 24, strength: 1.0 },
            Filter::GlitchShift { bands: 8, max_shift: 14, seed: 1 },
            Filter::GoldenBorder { width: 18, color: Rgba::rgb(212, 175, 55) },
            Filter::GradientFadeBorder { width: 30, color: Rgba::rgb(40, 40, 60) },
            Filter::Halation { threshold: 0.75, radius: 8, strength: 0.6 },
            Filter::HeatHazeRefraction { amplitude: 3, wavelength: 24 },
            Filter::HeatMap,
            Filter::HexagonalPixelate { size: 10 },
            Filter::HologramScan { line_spacing: 3, strength: 0.6, tint: Rgba::rgb(90, 220, 255) },
            Filter::HolographicFoilShimmer { strength: 0.5, period: 40 },
            Filter::InfraredFalseColor { strength: 1.0 },
            Filter::InkSplatterDrips { count: 3, size: 0.2, color: Rgba::rgb(20, 20, 60), seed: 1 },
            Filter::InnerShadow { opacity: 0.8, size: 8, color: Rgba::BLACK, offset_x: 4, offset_y: 4 },
            Filter::JpegArtifact { quality: 20 },
            Filter::LavaMoltenBorder { width: 20, hot: Rgba::rgb(255, 110, 20), cool: Rgba::rgb(60, 20, 10), seed: 1 },
            Filter::LensBlur { radius: 8, highlights: 2.0 },
            Filter::LightLeak { center_x: 0.0, center_y: 0.0, radius: 0.6, strength: 0.7, color: Rgba::rgb(255, 120, 40) },
            Filter::LiquidGlass { strength: 6, scale: 24, seed: 1 },
            Filter::LiquidMercury { scale: 40, seed: 1 },
            Filter::LuminanceContourLines { levels: 12, color: Rgba::rgb(20, 40, 90) },
            Filter::MacOSWindow { bar: 28, radius: 10, bar_color: Rgba::rgb(230, 230, 232) },
            Filter::MarbleBorder { width: 20, base: Rgba::rgb(236, 232, 226), vein: Rgba::rgb(120, 110, 100), seed: 1 },
            Filter::MatrixDigitalRain { column: 12, strength: 0.8, color: Rgba::rgb(60, 255, 90), seed: 1 },
            Filter::MinecraftGrass { block: 8, seed: 1 },
            Filter::Miniature { focus: 0.5, band: 0.3, radius: 4, saturation: 0.4 },
            Filter::MosaicPolygon { size: 12, grout: Rgba::rgb(30, 30, 30), seed: 1 },
            Filter::MosaicTileBorder { width: 16, tile: 6, color: Rgba::rgb(40, 120, 170), grout: Rgba::rgb(230, 230, 225), seed: 1 },
            Filter::NebulaStarfield { density: 0.5, nebula: Rgba::rgb(120, 60, 200), strength: 0.7, seed: 1 },
            Filter::NeonEdgeGlow { color: Rgba::rgb(255, 60, 200), strength: 1.0, radius: 3 },
            Filter::NeonGlowBorder { width: 16, color: Rgba::rgb(0, 255, 230) },
            Filter::NightVision { gain: 0.5, grain: 0.4, seed: 1 },
            Filter::OilSlickInterference { strength: 0.5, scale: 60, seed: 1 },
            Filter::OldCameraFlashBurn { strength: 0.6, sepia: 0.6 },
            Filter::OrnateScrollBorder { width: 22, color: Rgba::rgb(60, 40, 20), accent: Rgba::rgb(220, 190, 110) },
            Filter::PaletteMap { shadow: Rgba::rgb(20, 10, 60), midtone: Rgba::rgb(220, 90, 120), highlight: Rgba::rgb(255, 240, 200) },
            Filter::PaperStencilMask { paper: Rgba::rgb(240, 232, 214), ink: Rgba::rgb(35, 30, 28), threshold: 0.5, grain: 0.6, seed: 1 },
            Filter::PencilSketch { strength: 1.0, radius: 6 },
            Filter::PerlinNoise { scale: 6, strength: 0.4, seed: 1 },
            Filter::PixelSorting { threshold: 0.35, vertical: false },
            Filter::PlasmaEnergyArcs { scale: 6, strength: 0.8, color: Rgba::rgb(120, 220, 255), seed: 1 },
            Filter::Pointillism { cell: 6, seed: 1 },
            Filter::PolaroidFrame { border: 10, bottom: 30, color: Rgba::rgb(244, 240, 228) },
            Filter::PopArtGrid { tiles: 2 },
            Filter::RadialRainbow { strength: 0.4, bands: 3 },
            Filter::RainbowBorder { width: 16 },
            Filter::RainyWindow { drops: 24, size: 8, blur_radius: 3, seed: 1 },
            Filter::RetroSunset { strength: 0.6 },
            Filter::RisoPrint { ink: Rgba::rgb(230, 60, 120), paper: Rgba::rgb(245, 240, 230), grain: 0.5, seed: 1 },
            Filter::RopeBorder { width: 16, color: Rgba::rgb(190, 150, 90), accent: Rgba::rgb(120, 85, 45) },
            Filter::RustCorrosion { amount: 0.7, scale: 24, seed: 1 },
            Filter::Scanline { spacing: 3, strength: 0.4 },
            Filter::SmokeOverlay { opacity: 0.6, scale: 32, seed: 1 },
            Filter::SnowfallDepthFog { flakes: 120, fog: 0.5, seed: 1 },
            Filter::SobelEdge { gain: 1.5 },
            Filter::SoftDiffusion { radius: 4, strength: 0.6, seed: 1 },
            Filter::SpinBlur { angle: 20.0, center_x: 0.5, center_y: 0.5 },
            Filter::StainedGlass { cell: 12, lead: 2, seed: 1 },
            Filter::StainedGlassBorder { width: 16, pane: 10, lead: Rgba::rgb(30, 25, 25), seed: 1 },
            Filter::StarfieldBorder { width: 20, background: Rgba::rgb(12, 16, 48), star: Rgba::rgb(255, 250, 220), seed: 1 },
            Filter::StarFilter { threshold: 0.8, length: 16, strength: 0.6, color: Rgba::rgb(255, 230, 170) },
            Filter::StripeOverlay { period: 8, opacity: 0.35, color: Rgba::WHITE },
            Filter::SurfaceBlur { radius: 3, threshold: 40 },
            Filter::ThermalVision { strength: 1.0 },
            Filter::TiltShift { focus: 0.5, band: 0.2, radius: 4 },
            Filter::TrianglePixelate { size: 10 },
            Filter::TVStatic { amount: 0.3, seed: 1 },
            Filter::UnsharpMask { radius: 2, amount: 1.5, threshold: 0 },
            Filter::VhsTapeDamage { jitter: 6, bleed: 2, noise: 0.5, seed: 1 },
            Filter::VintagePrintDamage { fade: 0.6, scratches: 6, seed: 1 },
            Filter::VoronoiMosaic { cell: 8, seed: 1 },
            Filter::WatercolorBorder { width: 14, color: Rgba::rgb(70, 130, 190), seed: 1 },
            Filter::WatercolorKuwahara { radius: 3 },
            Filter::WetPlateCollodion { vignette: 0.6, seed: 1 },
            Filter::Windows11 { radius: 8, shadow: 8 },
            Filter::WindStreaks { streaks: 16, length: 16, strength: 0.7, seed: 1 },
            Filter::WoodcutPrint { line: 3, ink: Rgba::BLACK, paper: Rgba::rgb(245, 238, 222) },
            Filter::WoodenFrame { width: 14, color: Rgba::rgb(139, 90, 43), seed: 1 },
            Filter::XRayScan { position: 0.5, tint: Rgba::rgb(120, 255, 200) },
            Filter::ZigZagBorder { width: 10, color: Rgba::rgb(200, 40, 60), accent: Rgba::WHITE },
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
        // Every variant's default has its own name.
        let defaults = Filter::defaults();
        assert!(defaults.len() >= 27);
        let names: HashSet<&str> = defaults.iter().map(Filter::name).collect();
        assert_eq!(names.len(), defaults.len());
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
