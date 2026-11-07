//! Filter kernels: the classic filters, one function per effect, grouped by kind.
//!
//! Pixels are Rgba8 with straight alpha, in and out. Blurs and the effects that composite a shape
//! work on premultiplied colour, so transparent pixels don't pull in black. The rest work on straight
//! channels and keep the source alpha. Edge pixels clamp. Each parallel task writes its own rows, and
//! randomness is either hashed from `(seed, index)` or drawn from one sequential stream per seed, so
//! the output never depends on the thread count.

mod blur;
mod canvas;
mod cells;
mod convolve;
mod edge;
mod rank;
mod shift;
mod tone;
// ---- X2 ----
mod x2;
// ---- end X2 ----
// ---- X3 ----
mod x3;
// ---- end X3 ----
// ---- X4 ----
mod x4;
// ---- end X4 ----

use laqta_domain::effects::Filter;
use laqta_domain::{Bitmap, PixelFormat};

use super::{EffectContext, EffectError};

/// Caps on parameters that would otherwise ask for unbounded work. Larger values render as the cap.
const MAX_BOX_RADIUS: u32 = 1024;
const MAX_SIGMA: f32 = 64.0;
const MAX_MOTION_DISTANCE: u32 = 256;
const MAX_WINDOW_RADIUS: u32 = 10;

/// Applies one filter. Runs on the compute pool. Empty images come back unchanged.
pub(super) fn apply(image: &Bitmap, effect: &Filter, _cx: &EffectContext<'_>) -> Result<Bitmap, EffectError> {
    debug_assert!(rayon::current_thread_index().is_some(), "filter effects must run inside the compute pool");
    debug_assert_eq!(image.format(), PixelFormat::Rgba8);
    if image.width() == 0 || image.height() == 0 {
        return Ok(image.clone());
    }
    let src = Buf::from_bitmap(image);
    let out = match *effect {
        Filter::Blur { radius } => blur::box_blur(&src, radius.min(MAX_BOX_RADIUS)),
        Filter::ColorDepth { bits } => tone::color_depth(&src, bits),
        Filter::ConvolutionMatrix { kernel, divisor, offset } => convolve::convolve(&src, &kernel, divisor, offset),
        Filter::EdgeDetect => convolve::convolve(&src, &convolve::EDGE_DETECT, 1.0, 0.0),
        Filter::Emboss => convolve::convolve(&src, &convolve::EMBOSS, 1.0, 128.0),
        Filter::GaussianBlur { sigma } => blur::gaussian(&src, cap(sigma, MAX_SIGMA)),
        Filter::Glow { size, strength, color, offset_x, offset_y } => canvas::glow(&src, size, strength, color, (offset_x, offset_y))?,
        Filter::MeanRemoval => convolve::convolve(&src, &convolve::MEAN_REMOVAL, 1.0, 0.0),
        Filter::Outline { size, padding, color, outline_only } => canvas::outline(&src, size, padding, color, outline_only)?,
        Filter::Pixelate { size } => cells::pixelate(&src, size),
        Filter::Reflection { percentage, max_alpha, min_alpha, offset, skew, skew_size } => {
            canvas::reflection(&src, percentage, max_alpha, min_alpha, offset, skew, skew_size)?
        }
        Filter::RgbSplit { offset_red, offset_green, offset_blue } => shift::rgb_split(&src, [offset_red, offset_green, offset_blue]),
        Filter::Shadow { opacity, size, darkness, color, offset_x, offset_y, auto_resize } => {
            canvas::shadow(&src, opacity, size, darkness, color, (offset_x, offset_y), auto_resize)?
        }
        Filter::Sharpen => convolve::convolve(&src, &convolve::SHARPEN, 1.0, 0.0),
        Filter::Slice { min_height, max_height, min_shift, max_shift, seed } => {
            shift::slice(&src, min_height, max_height, min_shift, max_shift, seed)
        }
        Filter::Smooth => convolve::convolve(&src, &convolve::SMOOTH, 16.0, 0.0),
        Filter::TornEdge { depth, range, top, right, bottom, left, curved, seed } => {
            edge::torn(&src, depth, range, [top, right, bottom, left], curved, seed)
        }
        Filter::WaveEdge { depth, range, top, right, bottom, left } => edge::wave(&src, depth, range, [top, right, bottom, left]),
        Filter::Noise { amount, seed } => tone::noise(&src, amount, seed),
        Filter::Posterize { levels } => tone::posterize(&src, levels),
        Filter::Vignette { strength, radius, color } => tone::vignette(&src, strength, radius, color),
        Filter::Halftone { cell } => cells::halftone(&src, cell),
        Filter::Dither { levels } => tone::dither(&src, levels),
        Filter::MotionBlur { distance, angle } => blur::motion(&src, distance.min(MAX_MOTION_DISTANCE), angle),
        Filter::ZoomBlur { amount, center_x, center_y } => blur::zoom(&src, amount, center_x, center_y),
        Filter::Median { radius } => rank::median(&src, radius.min(MAX_WINDOW_RADIUS)),
        Filter::OilPaint { radius, levels } => rank::oil_paint(&src, radius.min(MAX_WINDOW_RADIUS), levels),
        // ---- X2 ----
        Filter::Anaglyph3D { parallax } => x2::anaglyph(&src, parallax),
        Filter::AnamorphicLensFlare { threshold, length, strength, color } => {
            x2::anamorphic_flare(&src, threshold, length, strength, color)
        }
        Filter::AnimeSpeedLines { lines, inner, strength, color, seed } => x2::anime_speed_lines(&src, lines, inner, strength, color, seed),
        Filter::ArtDecoBorder { width, color, accent } => x2::art_deco_border(&src, width, color, accent)?,
        Filter::ASCIIArt { cell, color } => x2::ascii_art(&src, cell, color),
        Filter::Bevel { size, strength } => x2::bevel(&src, size, strength),
        Filter::BlockGlitch { count, max_shift, seed } => x2::block_glitch(&src, count, max_shift, seed),
        Filter::BloodSplash { count, size, color, seed } => x2::blood_splash(&src, count, size, color, seed),
        Filter::Bloom { threshold, radius, strength } => x2::bloom(&src, threshold, radius, strength),
        Filter::BlueprintDrawing { background, line } => x2::blueprint_drawing(&src, background, line),
        Filter::BrickWallBorder { width, brick, mortar } => x2::brick_wall_border(&src, width, brick, mortar)?,
        Filter::CandyCaneBorder { width, color, accent } => x2::candy_cane_border(&src, width, color, accent)?,
        Filter::CartoonStickerCutout { levels, outline, color } => x2::cartoon_sticker(&src, levels, outline, color)?,
        Filter::CelticKnotBorder { width, color, accent } => x2::celtic_knot_border(&src, width, color, accent)?,
        Filter::ChainLinkBorder { width, color, accent } => x2::chain_link_border(&src, width, color, accent)?,
        Filter::ChalkDrawing { threshold, board, chalk, seed } => x2::chalk_drawing(&src, threshold, board, chalk, seed),
        Filter::ChromaticAberration { amount } => x2::chromatic_aberration(&src, amount),
        Filter::ChromeMetallicBorder { width, color } => x2::chrome_border(&src, width, color)?,
        Filter::CircuitBoardBorder { width, board, trace } => x2::circuit_border(&src, width, board, trace)?,
        Filter::ClaymationTexture { radius, depth } => x2::claymation(&src, radius, depth),
        Filter::Clouds { scale, opacity, color, seed } => x2::clouds(&src, scale, opacity, color, seed),
        Filter::ColorHalftone { cell } => x2::color_halftone(&src, cell),
        Filter::ColorSplash { target, tolerance } => x2::color_splash(&src, target, tolerance),
        Filter::CrimsonBorder { width, color, accent } => x2::crimson_border(&src, width, color, accent)?,
        Filter::Crosshatch { spacing } => x2::crosshatch(&src, spacing),
        Filter::CRT { scanlines, mask, vignette } => x2::crt(&src, scanlines, mask, vignette),
        Filter::CrystalizeShards { size, seed } => x2::shards(&src, size, seed),
        Filter::CrystalPrism { size, dispersion, seed } => x2::prism(&src, size, dispersion, seed),
        Filter::Cyanotype { shadow, highlight } => x2::cyanotype(&src, shadow, highlight),
        Filter::DatamoshSmear { block, max_shift, seed } => x2::datamosh(&src, block, max_shift, seed),
        Filter::DiamondPixelate { size } => x2::diamond_pixelate(&src, size),
        Filter::DisposableCamera { warmth, grain, vignette, seed } => x2::disposable_camera(&src, warmth, grain, vignette, seed),
        Filter::DotMatrix { cell, background } => x2::dot_matrix(&src, cell, background),
        Filter::DoubleExposure { offset_x, offset_y, opacity } => x2::double_exposure(&src, (offset_x, offset_y), opacity),
        Filter::DreamGlow { radius, strength } => x2::dream_glow(&src, radius, strength),
        Filter::EdgeFeather { size } => x2::edge_feather(&src, size),
        Filter::Embroidery { cell, cloth } => x2::embroidery(&src, cell, cloth),
        Filter::Engraving { spacing, paper, ink } => x2::engraving(&src, spacing, paper, ink),
        Filter::EtchedGlass { radius, seed } => x2::etched_glass(&src, radius, seed),
        Filter::FakeHair { strands, length, color, seed } => x2::fake_hair(&src, strands, length, color, seed),
        Filter::FilmGrain { amount, seed } => x2::film_grain(&src, amount, seed),
        // ---- end X2 ----
        // ---- X3 ----
        Filter::FilmStripFrame { width, strip, hole } => x3::film_strip_frame(&src, width, strip, hole)?,
        Filter::FrostedGlass { radius, opacity, seed } => x3::frosted_glass(&src, radius, opacity, seed),
        Filter::FrostedGlassIceEdges { radius, edge, ice, seed } => x3::frosted_glass_ice_edges(&src, radius, edge, ice, seed),
        Filter::FrostIceBorder { width, ice, base, seed } => x3::frost_ice_border(&src, width, ice, base, seed)?,
        Filter::GhostTrail { copies, offset_x, offset_y, opacity } => x3::ghost_trail(&src, copies, (offset_x, offset_y), opacity),
        Filter::Glare { threshold, length, strength } => x3::glare(&src, threshold, length, strength),
        Filter::GlitchShift { bands, max_shift, seed } => x3::glitch_shift(&src, bands, max_shift, seed),
        Filter::GoldenBorder { width, color } => x3::golden_border(&src, width, color)?,
        Filter::GradientFadeBorder { width, color } => x3::gradient_fade_border(&src, width, color)?,
        Filter::Halation { threshold, radius, strength } => x3::halation(&src, threshold, radius, strength),
        Filter::HeatHazeRefraction { amplitude, wavelength } => x3::heat_haze(&src, amplitude, wavelength),
        Filter::HeatMap => x3::heat_map(&src),
        Filter::HexagonalPixelate { size } => x3::hexagonal_pixelate(&src, size),
        Filter::HologramScan { line_spacing, strength, tint } => x3::hologram_scan(&src, line_spacing, strength, tint),
        Filter::HolographicFoilShimmer { strength, period } => x3::holographic_foil(&src, strength, period),
        Filter::InfraredFalseColor { strength } => x3::infrared_false_color(&src, strength),
        Filter::InkSplatterDrips { count, size, color, seed } => x3::ink_splatter(&src, count, size, color, seed),
        Filter::InnerShadow { opacity, size, color, offset_x, offset_y } => x3::inner_shadow(&src, opacity, size, color, (offset_x, offset_y)),
        Filter::JpegArtifact { quality } => x3::jpeg_artifact(&src, quality),
        Filter::LavaMoltenBorder { width, hot, cool, seed } => x3::lava_border(&src, width, hot, cool, seed)?,
        Filter::LensBlur { radius, highlights } => x3::lens_blur(&src, radius, highlights),
        Filter::LightLeak { center_x, center_y, radius, strength, color } => x3::light_leak(&src, (center_x, center_y), radius, strength, color),
        Filter::LiquidGlass { strength, scale, seed } => x3::liquid_glass(&src, strength, scale, seed),
        Filter::LiquidMercury { scale, seed } => x3::liquid_mercury(&src, scale, seed),
        Filter::LuminanceContourLines { levels, color } => x3::contour_lines(&src, levels, color),
        Filter::MacOSWindow { bar, radius, bar_color } => x3::macos_window(&src, bar, radius, bar_color)?,
        Filter::MarbleBorder { width, base, vein, seed } => x3::marble_border(&src, width, base, vein, seed)?,
        Filter::MatrixDigitalRain { column, strength, color, seed } => x3::matrix_rain(&src, column, strength, color, seed),
        Filter::MinecraftGrass { block, seed } => x3::minecraft_grass(&src, block, seed)?,
        Filter::Miniature { focus, band, radius, saturation } => x3::miniature(&src, focus, band, radius, saturation),
        Filter::MosaicPolygon { size, grout, seed } => x3::mosaic_polygon(&src, size, grout, seed),
        Filter::MosaicTileBorder { width, tile, color, grout, seed } => x3::mosaic_tile_border(&src, width, tile, color, grout, seed)?,
        Filter::NebulaStarfield { density, nebula, strength, seed } => x3::nebula_starfield(&src, density, nebula, strength, seed),
        Filter::NeonEdgeGlow { color, strength, radius } => x3::neon_edge_glow(&src, color, strength, radius),
        Filter::NeonGlowBorder { width, color } => x3::neon_glow_border(&src, width, color)?,
        Filter::NightVision { gain, grain, seed } => x3::night_vision(&src, gain, grain, seed),
        Filter::OilSlickInterference { strength, scale, seed } => x3::oil_slick(&src, strength, scale, seed),
        Filter::OldCameraFlashBurn { strength, sepia } => x3::old_camera_flash(&src, strength, sepia),
        Filter::OrnateScrollBorder { width, color, accent } => x3::ornate_scroll_border(&src, width, color, accent)?,
        Filter::PaletteMap { shadow, midtone, highlight } => x3::palette_map(&src, shadow, midtone, highlight),
        Filter::PaperStencilMask { paper, ink, threshold, grain, seed } => x3::paper_stencil(&src, paper, ink, threshold, grain, seed),
        Filter::PencilSketch { strength, radius } => x3::pencil_sketch(&src, strength, radius),
        // ---- end X3 ----
        // ---- X4 ----
        Filter::PerlinNoise { scale, strength, seed } => x4::perlin_noise(&src, scale, strength, seed),
        Filter::PixelSorting { threshold, vertical } => x4::pixel_sorting(&src, threshold, vertical),
        Filter::PlasmaEnergyArcs { scale, strength, color, seed } => x4::plasma_energy_arcs(&src, scale, strength, color, seed),
        Filter::Pointillism { cell, seed } => x4::pointillism(&src, cell, seed),
        Filter::PolaroidFrame { border, bottom, color } => x4::polaroid_frame(&src, border, bottom, color)?,
        Filter::PopArtGrid { tiles } => x4::pop_art_grid(&src, tiles),
        Filter::RadialRainbow { strength, bands } => x4::radial_rainbow(&src, strength, bands),
        Filter::RainbowBorder { width } => x4::rainbow_border(&src, width)?,
        Filter::RainyWindow { drops, size, blur_radius, seed } => x4::rainy_window(&src, drops, size, blur_radius, seed),
        Filter::RetroSunset { strength } => x4::retro_sunset(&src, strength),
        Filter::RisoPrint { ink, paper, grain, seed } => x4::riso_print(&src, ink, paper, grain, seed),
        Filter::RopeBorder { width, color, accent } => x4::rope_border(&src, width, color, accent)?,
        Filter::RustCorrosion { amount, scale, seed } => x4::rust_corrosion(&src, amount, scale, seed),
        Filter::Scanline { spacing, strength } => x4::scanline(&src, spacing, strength),
        Filter::SmokeOverlay { opacity, scale, seed } => x4::smoke_overlay(&src, opacity, scale, seed),
        Filter::SnowfallDepthFog { flakes, fog, seed } => x4::snowfall_depth_fog(&src, flakes, fog, seed),
        Filter::SobelEdge { gain } => x4::sobel_edge(&src, gain),
        Filter::SoftDiffusion { radius, strength, seed } => x4::soft_diffusion(&src, radius, strength, seed),
        Filter::SpinBlur { angle, center_x, center_y } => x4::spin_blur(&src, angle, center_x, center_y),
        Filter::StainedGlass { cell, lead, seed } => x4::stained_glass(&src, cell, lead, seed),
        Filter::StainedGlassBorder { width, pane, lead, seed } => x4::stained_glass_border(&src, width, pane, lead, seed)?,
        Filter::StarfieldBorder { width, background, star, seed } => x4::starfield_border(&src, width, background, star, seed)?,
        Filter::StarFilter { threshold, length, strength, color } => x4::star_filter(&src, threshold, length, strength, color),
        Filter::StripeOverlay { period, opacity, color } => x4::stripe_overlay(&src, period, opacity, color),
        Filter::SurfaceBlur { radius, threshold } => x4::surface_blur(&src, radius, threshold),
        Filter::ThermalVision { strength } => x4::thermal_vision(&src, strength),
        Filter::TiltShift { focus, band, radius } => x4::tilt_shift(&src, focus, band, radius),
        Filter::TrianglePixelate { size } => x4::triangle_pixelate(&src, size),
        Filter::TVStatic { amount, seed } => x4::tv_static(&src, amount, seed),
        Filter::UnsharpMask { radius, amount, threshold } => x4::unsharp_mask(&src, radius, amount, threshold),
        Filter::VhsTapeDamage { jitter, bleed, noise, seed } => x4::vhs_tape_damage(&src, jitter, bleed, noise, seed),
        Filter::VintagePrintDamage { fade, scratches, seed } => x4::vintage_print_damage(&src, fade, scratches, seed),
        Filter::VoronoiMosaic { cell, seed } => x4::voronoi_mosaic(&src, cell, seed),
        Filter::WatercolorBorder { width, color, seed } => x4::watercolor_border(&src, width, color, seed)?,
        Filter::WatercolorKuwahara { radius } => x4::watercolor_kuwahara(&src, radius),
        Filter::WetPlateCollodion { vignette, seed } => x4::wet_plate_collodion(&src, vignette, seed),
        Filter::Windows11 { radius, shadow } => x4::windows_11(&src, radius, shadow)?,
        Filter::WindStreaks { streaks, length, strength, seed } => x4::wind_streaks(&src, streaks, length, strength, seed),
        Filter::WoodcutPrint { line, ink, paper } => x4::woodcut_print(&src, line, ink, paper),
        Filter::WoodenFrame { width, color, seed } => x4::wooden_frame(&src, width, color, seed)?,
        Filter::XRayScan { position, tint } => x4::xray_scan(&src, position, tint),
        Filter::ZigZagBorder { width, color, accent } => x4::zigzag_border(&src, width, color, accent)?,
        // ---- end X4 ----
    };
    Ok(out.into_bitmap())
}

/// Rgba8 pixels with their size. Kernels read one `Buf` and build another.
#[derive(Clone, Debug)]
pub(super) struct Buf {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Buf {
    fn from_bitmap(image: &Bitmap) -> Buf {
        Buf { w: image.width() as usize, h: image.height() as usize, px: image.pixels().to_vec() }
    }

    fn into_bitmap(self) -> Bitmap {
        let w = u32::try_from(self.w).expect("widths come from a bitmap or a canvas checked to fit u32");
        let h = u32::try_from(self.h).expect("heights come from a bitmap or a canvas checked to fit u32");
        Bitmap::new(w, h, PixelFormat::Rgba8, self.px.into()).unwrap_or_else(|e| unreachable!("pixel buffer built to size: {e:?}"))
    }
}

/// Premultiplied RGBA as 0..=1 floats: each colour channel is scaled by alpha.
fn premultiply(px: &[u8]) -> Vec<f32> {
    px.as_chunks::<4>().0.iter().flat_map(|p| premultiplied(p)).collect()
}

fn premultiplied(p: &[u8]) -> [f32; 4] {
    let a = f32::from(p[3]) / 255.0;
    [f32::from(p[0]) / 255.0 * a, f32::from(p[1]) / 255.0 * a, f32::from(p[2]) / 255.0 * a, a]
}

/// Straight RGBA8 from premultiplied floats. Pixels with no alpha come out as zeros.
fn unpremultiply(pm: &[f32]) -> Vec<u8> {
    pm.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = p[3].clamp(0.0, 1.0);
            if a <= 0.0 {
                return [0, 0, 0, 0];
            }
            let channel = |v: f32| to_u8(v / a * 255.0);
            [channel(p[0]), channel(p[1]), channel(p[2]), to_u8(a * 255.0)]
        })
        .collect()
}

/// Rounds to the nearest byte, clamping; NaN becomes 0.
fn to_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// Clamps to `0..=1`; NaN becomes 0.
fn unit(v: f32) -> f32 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

/// Clamps to `0..=max`; NaN becomes 0.
fn cap(v: f32, max: f32) -> f32 {
    if v.is_nan() { 0.0 } else { v.min(max) }
}

/// Index `offset - radius` steps from `v`, clamped to `0..len`. Callers pass `offset` in `0..=2 * radius`.
fn window_index(v: usize, offset: usize, radius: usize, len: usize) -> usize {
    (v + offset).saturating_sub(radius).min(len - 1)
}

/// Index `d` steps back from `v` (a positive `d` looks toward the start), clamped to `0..len`.
fn shifted(v: usize, d: i64, len: usize) -> usize {
    (v as i64 - d).clamp(0, len as i64 - 1) as usize
}

/// SplitMix64 finaliser: a bijective mix of 64 bits.
fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The value at `index` under `seed`. A pure function, so a parallel loop can call it from any thread.
fn hash(seed: u64, index: u64) -> u64 {
    mix64(mix64(seed).wrapping_add(index))
}

/// Sequential generator for the kernels that draw a few values per row or edge. The seed goes
/// through `mix64` first and the state is kept odd, so no seed leaves the xorshift stuck at zero.
#[derive(Debug)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(mix64(seed) | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in `lo..=hi`. Reversed bounds are swapped.
    fn range(&mut self, lo: u32, hi: u32) -> u32 {
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        lo + (self.next_u64() % (u64::from(hi - lo) + 1)) as u32
    }
}
