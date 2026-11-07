#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]
//! Filters X3 (M7 D5): one PNG snapshot per filter on the 24x24 test card, plus property checks.
//! Snapshots are written when missing and rewritten with `LAQTA_BLESS=1`; otherwise pixels must match
//! exactly.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use laqta_domain::color::Rgba;
use laqta_domain::effects::{Effect, Filter};
use laqta_domain::{Bitmap, PixelFormat};
use laqta_imaging::effects::{EffectContext, EffectError, apply};
use laqta_imaging::encode_png;

const N: u32 = 24;

/// The 42 X3 variants, by enum variant name.
const X3_VARIANTS: [&str; 42] = [
    "FilmStripFrame",
    "FrostedGlass",
    "FrostedGlassIceEdges",
    "FrostIceBorder",
    "GhostTrail",
    "Glare",
    "GlitchShift",
    "GoldenBorder",
    "GradientFadeBorder",
    "Halation",
    "HeatHazeRefraction",
    "HeatMap",
    "HexagonalPixelate",
    "HologramScan",
    "HolographicFoilShimmer",
    "InfraredFalseColor",
    "InkSplatterDrips",
    "InnerShadow",
    "JpegArtifact",
    "LavaMoltenBorder",
    "LensBlur",
    "LightLeak",
    "LiquidGlass",
    "LiquidMercury",
    "LuminanceContourLines",
    "MacOSWindow",
    "MarbleBorder",
    "MatrixDigitalRain",
    "MinecraftGrass",
    "Miniature",
    "MosaicPolygon",
    "MosaicTileBorder",
    "NebulaStarfield",
    "NeonEdgeGlow",
    "NeonGlowBorder",
    "NightVision",
    "OilSlickInterference",
    "OldCameraFlashBurn",
    "OrnateScrollBorder",
    "PaletteMap",
    "PaperStencilMask",
    "PencilSketch",
];

fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| rayon::ThreadPoolBuilder::new().num_threads(3).build().unwrap())
}

fn try_run(filter: &Filter, input: &Bitmap) -> Result<Bitmap, EffectError> {
    pool().install(|| apply(input, &Effect::Filter(filter.clone()), &EffectContext::none()))
}

fn run(filter: &Filter, input: &Bitmap) -> Bitmap {
    try_run(filter, input).unwrap()
}

/// 24x24 test card: a red and green gradient, a blue 4-pixel checkerboard, isolated white speckles,
/// a transparent 6x6 corner and a half-transparent 6x6 corner.
fn pattern() -> Bitmap {
    let mut px = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let r = (x * 255 / (N - 1)) as u8;
            let g = (y * 255 / (N - 1)) as u8;
            let mut b = if (x / 4 + y / 4) % 2 == 0 { 40 } else { 220 };
            let a = if x < 6 && y < 6 {
                0
            } else if x >= 18 && y >= 18 {
                128
            } else {
                255
            };
            let speckle = a == 255 && (x * 7 + y * 11) % 29 == 0;
            if speckle {
                b = 255;
            }
            let (r, g) = if speckle { (255, 255) } else { (r, g) };
            px.extend_from_slice(&[r, g, b, a]);
        }
    }
    Bitmap::new(N, N, PixelFormat::Rgba8, px.into()).unwrap()
}

/// A `w` x `h` card with the same colour rules as `pattern`, for sizes other than 24x24.
fn card(w: u32, h: u32) -> Bitmap {
    let span = |n: u32| n.max(2) - 1;
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let r = (x * 255 / span(w)) as u8;
            let g = (y * 255 / span(h)) as u8;
            let b = if (x / 4 + y / 4) % 2 == 0 { 40 } else { 220 };
            px.extend_from_slice(&[r, g, b, 255]);
        }
    }
    Bitmap::new(w, h, PixelFormat::Rgba8, px.into()).unwrap()
}

fn px(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * b.width() + x) * 4) as usize;
    b.pixels()[i..i + 4].try_into().unwrap()
}

/// Every X3 filter, with parameters sized for the 24x24 card. The file name is `x3_<name>.png`.
fn cases() -> Vec<(&'static str, Filter)> {
    vec![
        ("film_strip_frame", Filter::FilmStripFrame { width: 4, strip: Rgba::rgb(22, 22, 22), hole: Rgba::rgb(240, 240, 240) }),
        ("frosted_glass", Filter::FrostedGlass { radius: 2, opacity: 0.5, seed: 4 }),
        ("frosted_glass_ice_edges", Filter::FrostedGlassIceEdges { radius: 2, edge: 6, ice: Rgba::rgb(210, 235, 255), seed: 4 }),
        ("frost_ice_border", Filter::FrostIceBorder { width: 4, ice: Rgba::rgb(225, 245, 255), base: Rgba::rgb(160, 200, 225), seed: 4 }),
        ("ghost_trail", Filter::GhostTrail { copies: 3, offset_x: 3, offset_y: 2, opacity: 0.6 }),
        ("glare", Filter::Glare { threshold: 0.6, length: 6, strength: 1.0 }),
        ("glitch_shift", Filter::GlitchShift { bands: 4, max_shift: 5, seed: 9 }),
        ("golden_border", Filter::GoldenBorder { width: 4, color: Rgba::rgb(212, 175, 55) }),
        ("gradient_fade_border", Filter::GradientFadeBorder { width: 4, color: Rgba::rgb(40, 40, 60) }),
        ("halation", Filter::Halation { threshold: 0.6, radius: 2, strength: 0.8 }),
        ("heat_haze_refraction", Filter::HeatHazeRefraction { amplitude: 2, wavelength: 8 }),
        ("heat_map", Filter::HeatMap),
        ("hexagonal_pixelate", Filter::HexagonalPixelate { size: 4 }),
        ("hologram_scan", Filter::HologramScan { line_spacing: 3, strength: 0.6, tint: Rgba::rgb(90, 220, 255) }),
        ("holographic_foil_shimmer", Filter::HolographicFoilShimmer { strength: 0.6, period: 8 }),
        ("infrared_false_color", Filter::InfraredFalseColor { strength: 1.0 }),
        ("ink_splatter_drips", Filter::InkSplatterDrips { count: 3, size: 0.25, color: Rgba::rgb(20, 20, 60), seed: 2 }),
        ("inner_shadow", Filter::InnerShadow { opacity: 0.8, size: 2, color: Rgba::BLACK, offset_x: 2, offset_y: 2 }),
        ("jpeg_artifact", Filter::JpegArtifact { quality: 10 }),
        ("lava_molten_border", Filter::LavaMoltenBorder { width: 4, hot: Rgba::rgb(255, 110, 20), cool: Rgba::rgb(60, 20, 10), seed: 3 }),
        ("lens_blur", Filter::LensBlur { radius: 3, highlights: 2.0 }),
        ("light_leak", Filter::LightLeak { center_x: 0.0, center_y: 0.0, radius: 0.6, strength: 0.7, color: Rgba::rgb(255, 120, 40) }),
        ("liquid_glass", Filter::LiquidGlass { strength: 3, scale: 6, seed: 5 }),
        ("liquid_mercury", Filter::LiquidMercury { scale: 6, seed: 5 }),
        ("luminance_contour_lines", Filter::LuminanceContourLines { levels: 6, color: Rgba::rgb(20, 40, 90) }),
        ("macos_window", Filter::MacOSWindow { bar: 6, radius: 4, bar_color: Rgba::rgb(230, 230, 232) }),
        ("marble_border", Filter::MarbleBorder { width: 4, base: Rgba::rgb(236, 232, 226), vein: Rgba::rgb(120, 110, 100), seed: 6 }),
        ("matrix_digital_rain", Filter::MatrixDigitalRain { column: 4, strength: 0.8, color: Rgba::rgb(60, 255, 90), seed: 6 }),
        ("minecraft_grass", Filter::MinecraftGrass { block: 4, seed: 6 }),
        ("miniature", Filter::Miniature { focus: 0.5, band: 0.3, radius: 2, saturation: 0.4 }),
        ("mosaic_polygon", Filter::MosaicPolygon { size: 5, grout: Rgba::rgb(30, 30, 30), seed: 7 }),
        ("mosaic_tile_border", Filter::MosaicTileBorder { width: 4, tile: 2, color: Rgba::rgb(40, 120, 170), grout: Rgba::rgb(230, 230, 225), seed: 7 }),
        ("nebula_starfield", Filter::NebulaStarfield { density: 1.0, nebula: Rgba::rgb(120, 60, 200), strength: 0.8, seed: 8 }),
        ("neon_edge_glow", Filter::NeonEdgeGlow { color: Rgba::rgb(255, 60, 200), strength: 1.0, radius: 1 }),
        ("neon_glow_border", Filter::NeonGlowBorder { width: 4, color: Rgba::rgb(0, 255, 230) }),
        ("night_vision", Filter::NightVision { gain: 0.5, grain: 0.4, seed: 8 }),
        ("oil_slick_interference", Filter::OilSlickInterference { strength: 0.6, scale: 8, seed: 9 }),
        ("old_camera_flash_burn", Filter::OldCameraFlashBurn { strength: 0.6, sepia: 0.6 }),
        ("ornate_scroll_border", Filter::OrnateScrollBorder { width: 6, color: Rgba::rgb(60, 40, 20), accent: Rgba::rgb(220, 190, 110) }),
        ("palette_map", Filter::PaletteMap { shadow: Rgba::rgb(20, 10, 60), midtone: Rgba::rgb(220, 90, 120), highlight: Rgba::rgb(255, 240, 200) }),
        ("paper_stencil_mask", Filter::PaperStencilMask { paper: Rgba::rgb(240, 232, 214), ink: Rgba::rgb(35, 30, 28), threshold: 0.5, grain: 0.6, seed: 10 }),
        ("pencil_sketch", Filter::PencilSketch { strength: 1.0, radius: 2 }),
    ]
}

/// The enum variant's name, read from the Debug form (`Bevel { size: 8, .. }` gives `Bevel`).
fn variant(filter: &Filter) -> String {
    let debug = format!("{filter:?}");
    debug.split([' ', '{']).next().unwrap_or_default().to_string()
}

fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots").join(format!("x3_{name}.png"))
}

/// Width, height and RGBA8 pixels of a PNG.
fn decode(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn snapshot(name: &str, out: &Bitmap) {
    let path = snapshot_path(name);
    let bytes = pool().install(|| encode_png(out).unwrap());
    if std::env::var_os("LAQTA_BLESS").is_some() || !path.exists() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        return;
    }
    let (ew, eh, expected) = decode(&std::fs::read(&path).unwrap());
    let (aw, ah, actual) = decode(&bytes);
    assert_eq!((ew, eh), (aw, ah), "{name}: size differs from {}", path.display());
    assert!(expected == actual, "{name}: pixels differ from {}", path.display());
}

#[test]
fn x3_filters_match_their_snapshots() {
    let input = pattern();
    for (name, filter) in &cases() {
        let out = run(filter, &input);
        let changed = out.width() != input.width() || out.height() != input.height() || out.pixels() != input.pixels();
        assert!(changed, "{name} left the test card unchanged");
        snapshot(name, &out);
    }
}

#[test]
fn x3_cases_cover_every_x3_default() {
    let cases = cases();
    assert_eq!(cases.len(), X3_VARIANTS.len());
    let defaults: Vec<String> = Filter::defaults().iter().map(variant).collect();
    assert_eq!(defaults.iter().filter(|d| X3_VARIANTS.contains(&d.as_str())).count(), X3_VARIANTS.len());
    for name in X3_VARIANTS {
        assert!(defaults.iter().any(|d| d == name), "{name} is not in defaults()");
        let cased = cases.iter().filter(|(_, f)| variant(f) == name).count();
        assert_eq!(cased, 1, "{name} needs exactly one snapshot case");
    }
}

#[test]
fn x3_filters_run_on_odd_sizes() {
    for (w, h) in [(1, 1), (3, 1), (1, 5), (7, 2)] {
        let input = card(w, h);
        for (name, filter) in cases() {
            let out = try_run(&filter, &input).unwrap_or_else(|e| panic!("{name} on {w}x{h}: {e}"));
            assert!(out.width() >= w && out.height() >= h, "{name} on {w}x{h} shrank to {}x{}", out.width(), out.height());
        }
    }
}

#[test]
fn x3_seeded_filters_repeat_and_depend_on_the_seed() {
    let input = pattern();
    let seeded = |seed: u64| {
        vec![
            Filter::FrostedGlass { radius: 2, opacity: 0.5, seed },
            Filter::FrostedGlassIceEdges { radius: 2, edge: 6, ice: Rgba::WHITE, seed },
            Filter::FrostIceBorder { width: 4, ice: Rgba::WHITE, base: Rgba::BLACK, seed },
            Filter::GlitchShift { bands: 4, max_shift: 5, seed },
            Filter::InkSplatterDrips { count: 3, size: 0.25, color: Rgba::BLACK, seed },
            Filter::LavaMoltenBorder { width: 4, hot: Rgba::WHITE, cool: Rgba::BLACK, seed },
            Filter::LiquidGlass { strength: 3, scale: 6, seed },
            Filter::LiquidMercury { scale: 6, seed },
            Filter::MarbleBorder { width: 4, base: Rgba::WHITE, vein: Rgba::BLACK, seed },
            Filter::MatrixDigitalRain { column: 4, strength: 0.8, color: Rgba::WHITE, seed },
            Filter::MinecraftGrass { block: 4, seed },
            Filter::MosaicPolygon { size: 5, grout: Rgba::BLACK, seed },
            Filter::MosaicTileBorder { width: 4, tile: 2, color: Rgba::WHITE, grout: Rgba::BLACK, seed },
            Filter::NebulaStarfield { density: 1.0, nebula: Rgba::WHITE, strength: 0.8, seed },
            Filter::NightVision { gain: 0.5, grain: 0.4, seed },
            Filter::OilSlickInterference { strength: 0.6, scale: 8, seed },
            Filter::PaperStencilMask { paper: Rgba::WHITE, ink: Rgba::BLACK, threshold: 0.5, grain: 0.6, seed },
        ]
    };
    for (a, b) in seeded(5).iter().zip(seeded(5).iter()) {
        assert!(run(a, &input).pixels() == run(b, &input).pixels(), "{a:?} differs for one seed");
    }
    for (a, b) in seeded(5).iter().zip(seeded(6).iter()) {
        assert!(run(a, &input).pixels() != run(b, &input).pixels(), "{a:?} ignores its seed");
    }
}

#[test]
fn x3_frames_grow_the_canvas_by_their_width() {
    let input = pattern();
    let frames = [
        Filter::FilmStripFrame { width: 4, strip: Rgba::BLACK, hole: Rgba::WHITE },
        Filter::FrostIceBorder { width: 4, ice: Rgba::WHITE, base: Rgba::BLACK, seed: 1 },
        Filter::GoldenBorder { width: 4, color: Rgba::BLACK },
        Filter::GradientFadeBorder { width: 4, color: Rgba::BLACK },
        Filter::LavaMoltenBorder { width: 4, hot: Rgba::WHITE, cool: Rgba::BLACK, seed: 1 },
        Filter::MarbleBorder { width: 4, base: Rgba::WHITE, vein: Rgba::BLACK, seed: 1 },
        Filter::MosaicTileBorder { width: 4, tile: 3, color: Rgba::WHITE, grout: Rgba::BLACK, seed: 1 },
        Filter::NeonGlowBorder { width: 4, color: Rgba::WHITE },
        Filter::OrnateScrollBorder { width: 4, color: Rgba::BLACK, accent: Rgba::WHITE },
    ];
    for filter in &frames {
        let out = run(filter, &input);
        assert_eq!((out.width(), out.height()), (N + 8, N + 8), "{}", variant(filter));
        // The image sits unchanged in the middle of the frame.
        for y in 0..N {
            for x in 0..N {
                assert_eq!(px(&out, x + 4, y + 4), px(&input, x, y), "{}: pixel {x},{y}", variant(filter));
            }
        }
    }
}

#[test]
fn x3_macos_window_adds_a_title_bar_above_the_image() {
    let input = pattern();
    let bar = 6;
    let out = run(&Filter::MacOSWindow { bar, radius: 4, bar_color: Rgba::rgb(230, 230, 232) }, &input);
    assert_eq!((out.width(), out.height()), (N, N + bar));
    // Away from the rounded corners the image is unchanged, just moved down by the bar.
    for y in 4..N - 4 {
        for x in 4..N - 4 {
            assert_eq!(px(&out, x, y + bar), px(&input, x, y), "pixel {x},{y}");
        }
    }
}

#[test]
fn x3_frames_past_the_pixel_budget_are_refused() {
    let input = pattern();
    let huge = [
        Filter::FilmStripFrame { width: u32::MAX, strip: Rgba::BLACK, hole: Rgba::WHITE },
        Filter::GoldenBorder { width: 20_000, color: Rgba::BLACK },
        Filter::MarbleBorder { width: 20_000, base: Rgba::BLACK, vein: Rgba::WHITE, seed: 0 },
        Filter::MacOSWindow { bar: u32::MAX, radius: 4, bar_color: Rgba::BLACK },
    ];
    for filter in &huge {
        assert!(matches!(try_run(filter, &input), Err(EffectError::Invalid(_))), "{filter:?}");
    }
}

#[test]
fn x3_degenerate_parameters_render_without_panicking() {
    let input = pattern();
    let odd = [
        Filter::FrostedGlass { radius: u32::MAX, opacity: f32::NAN, seed: 0 },
        Filter::FrostedGlassIceEdges { radius: u32::MAX, edge: u32::MAX, ice: Rgba::WHITE, seed: 0 },
        Filter::FrostIceBorder { width: 0, ice: Rgba::WHITE, base: Rgba::BLACK, seed: 0 },
        Filter::GhostTrail { copies: u32::MAX, offset_x: i32::MIN, offset_y: i32::MAX, opacity: f32::INFINITY },
        Filter::Glare { threshold: f32::NAN, length: u32::MAX, strength: f32::INFINITY },
        Filter::GlitchShift { bands: u32::MAX, max_shift: u32::MAX, seed: 0 },
        Filter::GoldenBorder { width: 0, color: Rgba::WHITE },
        Filter::Halation { threshold: f32::NAN, radius: u32::MAX, strength: f32::INFINITY },
        Filter::HeatHazeRefraction { amplitude: u32::MAX, wavelength: 0 },
        Filter::HexagonalPixelate { size: 0 },
        Filter::HologramScan { line_spacing: 0, strength: f32::NAN, tint: Rgba::WHITE },
        Filter::HolographicFoilShimmer { strength: f32::NAN, period: 0 },
        Filter::InkSplatterDrips { count: u32::MAX, size: f32::NAN, color: Rgba::WHITE, seed: 0 },
        Filter::InkSplatterDrips { count: u32::MAX, size: 1e9, color: Rgba::WHITE, seed: 0 },
        Filter::InnerShadow { opacity: f32::NAN, size: u32::MAX, color: Rgba::BLACK, offset_x: i32::MIN, offset_y: i32::MAX },
        Filter::JpegArtifact { quality: 0 },
        Filter::LensBlur { radius: u32::MAX, highlights: f32::INFINITY },
        Filter::LightLeak { center_x: f32::NAN, center_y: f32::INFINITY, radius: f32::NAN, strength: f32::NAN, color: Rgba::WHITE },
        Filter::LiquidGlass { strength: u32::MAX, scale: 0, seed: 0 },
        Filter::LiquidMercury { scale: 0, seed: 0 },
        Filter::LuminanceContourLines { levels: 0, color: Rgba::WHITE },
        Filter::MacOSWindow { bar: 0, radius: u32::MAX, bar_color: Rgba::WHITE },
        Filter::MatrixDigitalRain { column: 0, strength: f32::NAN, color: Rgba::WHITE, seed: 0 },
        Filter::MinecraftGrass { block: 0, seed: 0 },
        Filter::Miniature { focus: f32::NAN, band: f32::INFINITY, radius: u32::MAX, saturation: f32::NAN },
        Filter::MosaicPolygon { size: 0, grout: Rgba::BLACK, seed: 0 },
        Filter::MosaicPolygon { size: u32::MAX, grout: Rgba::BLACK, seed: 0 },
        Filter::MosaicTileBorder { width: 0, tile: 0, color: Rgba::WHITE, grout: Rgba::BLACK, seed: 0 },
        Filter::NebulaStarfield { density: f32::NAN, nebula: Rgba::BLACK, strength: f32::INFINITY, seed: 0 },
        Filter::NeonEdgeGlow { color: Rgba::WHITE, strength: f32::NAN, radius: u32::MAX },
        Filter::NeonGlowBorder { width: 0, color: Rgba::WHITE },
        Filter::NightVision { gain: f32::NAN, grain: f32::INFINITY, seed: 0 },
        Filter::OilSlickInterference { strength: f32::NAN, scale: 0, seed: 0 },
        Filter::OldCameraFlashBurn { strength: f32::NAN, sepia: f32::INFINITY },
        Filter::PaperStencilMask { paper: Rgba::WHITE, ink: Rgba::BLACK, threshold: f32::NAN, grain: f32::INFINITY, seed: 0 },
        Filter::PencilSketch { strength: f32::NAN, radius: u32::MAX },
    ];
    for filter in &odd {
        let out = try_run(filter, &input).unwrap_or_else(|e| panic!("{filter:?}: {e}"));
        assert!(out.width() >= 1 && out.height() >= 1, "{filter:?}");
    }
}
