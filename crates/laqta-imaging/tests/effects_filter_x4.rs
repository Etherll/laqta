#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]
//! Filters X4 (M7 D5): one PNG snapshot per filter on the 24x24 test card, plus property checks.
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

/// The 42 X4 variants, by enum variant name.
const X4_VARIANTS: [&str; 42] = [
    "PerlinNoise",
    "PixelSorting",
    "PlasmaEnergyArcs",
    "Pointillism",
    "PolaroidFrame",
    "PopArtGrid",
    "RadialRainbow",
    "RainbowBorder",
    "RainyWindow",
    "RetroSunset",
    "RisoPrint",
    "RopeBorder",
    "RustCorrosion",
    "Scanline",
    "SmokeOverlay",
    "SnowfallDepthFog",
    "SobelEdge",
    "SoftDiffusion",
    "SpinBlur",
    "StainedGlass",
    "StainedGlassBorder",
    "StarfieldBorder",
    "StarFilter",
    "StripeOverlay",
    "SurfaceBlur",
    "ThermalVision",
    "TiltShift",
    "TrianglePixelate",
    "TVStatic",
    "UnsharpMask",
    "VhsTapeDamage",
    "VintagePrintDamage",
    "VoronoiMosaic",
    "WatercolorBorder",
    "WatercolorKuwahara",
    "WetPlateCollodion",
    "Windows11",
    "WindStreaks",
    "WoodcutPrint",
    "WoodenFrame",
    "XRayScan",
    "ZigZagBorder",
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

/// Every X4 filter, with parameters sized for the 24x24 card. The file name is `x4_<name>.png`.
fn cases() -> Vec<(&'static str, Filter)> {
    vec![
        ("perlin_noise", Filter::PerlinNoise { scale: 4, strength: 0.5, seed: 3 }),
        ("pixel_sorting", Filter::PixelSorting { threshold: 0.3, vertical: false }),
        ("plasma_energy_arcs", Filter::PlasmaEnergyArcs { scale: 3, strength: 0.9, color: Rgba::rgb(120, 220, 255), seed: 2 }),
        ("pointillism", Filter::Pointillism { cell: 4, seed: 2 }),
        ("polaroid_frame", Filter::PolaroidFrame { border: 3, bottom: 6, color: Rgba::rgb(244, 240, 228) }),
        ("pop_art_grid", Filter::PopArtGrid { tiles: 2 }),
        ("radial_rainbow", Filter::RadialRainbow { strength: 0.5, bands: 2 }),
        ("rainbow_border", Filter::RainbowBorder { width: 4 }),
        ("rainy_window", Filter::RainyWindow { drops: 6, size: 5, blur_radius: 2, seed: 4 }),
        ("retro_sunset", Filter::RetroSunset { strength: 0.7 }),
        ("riso_print", Filter::RisoPrint { ink: Rgba::rgb(230, 60, 120), paper: Rgba::rgb(245, 240, 230), grain: 0.5, seed: 5 }),
        ("rope_border", Filter::RopeBorder { width: 6, color: Rgba::rgb(190, 150, 90), accent: Rgba::rgb(120, 85, 45) }),
        ("rust_corrosion", Filter::RustCorrosion { amount: 0.8, scale: 6, seed: 6 }),
        ("scanline", Filter::Scanline { spacing: 3, strength: 0.5 }),
        ("smoke_overlay", Filter::SmokeOverlay { opacity: 0.7, scale: 8, seed: 7 }),
        ("snowfall_depth_fog", Filter::SnowfallDepthFog { flakes: 30, fog: 0.6, seed: 8 }),
        ("sobel_edge", Filter::SobelEdge { gain: 2.0 }),
        ("soft_diffusion", Filter::SoftDiffusion { radius: 2, strength: 0.8, seed: 9 }),
        ("spin_blur", Filter::SpinBlur { angle: 30.0, center_x: 0.5, center_y: 0.5 }),
        ("stained_glass", Filter::StainedGlass { cell: 6, lead: 1, seed: 10 }),
        ("stained_glass_border", Filter::StainedGlassBorder { width: 4, pane: 4, lead: Rgba::rgb(30, 25, 25), seed: 11 }),
        (
            "starfield_border",
            Filter::StarfieldBorder { width: 4, background: Rgba::rgb(12, 16, 48), star: Rgba::rgb(255, 250, 220), seed: 12 },
        ),
        ("star_filter", Filter::StarFilter { threshold: 0.4, length: 8, strength: 1.0, color: Rgba::rgb(255, 230, 170) }),
        ("stripe_overlay", Filter::StripeOverlay { period: 4, opacity: 0.5, color: Rgba::WHITE }),
        ("surface_blur", Filter::SurfaceBlur { radius: 3, threshold: 40 }),
        ("thermal_vision", Filter::ThermalVision { strength: 1.0 }),
        ("tilt_shift", Filter::TiltShift { focus: 0.5, band: 0.2, radius: 2 }),
        ("triangle_pixelate", Filter::TrianglePixelate { size: 4 }),
        ("tv_static", Filter::TVStatic { amount: 0.3, seed: 13 }),
        ("unsharp_mask", Filter::UnsharpMask { radius: 2, amount: 1.5, threshold: 0 }),
        ("vhs_tape_damage", Filter::VhsTapeDamage { jitter: 3, bleed: 2, noise: 0.5, seed: 14 }),
        ("vintage_print_damage", Filter::VintagePrintDamage { fade: 0.6, scratches: 3, seed: 15 }),
        ("voronoi_mosaic", Filter::VoronoiMosaic { cell: 5, seed: 16 }),
        ("watercolor_border", Filter::WatercolorBorder { width: 4, color: Rgba::rgb(70, 130, 190), seed: 17 }),
        ("watercolor_kuwahara", Filter::WatercolorKuwahara { radius: 2 }),
        ("wet_plate_collodion", Filter::WetPlateCollodion { vignette: 0.6, seed: 18 }),
        ("windows_11", Filter::Windows11 { radius: 4, shadow: 4 }),
        ("wind_streaks", Filter::WindStreaks { streaks: 12, length: 14, strength: 0.8, seed: 19 }),
        ("woodcut_print", Filter::WoodcutPrint { line: 3, ink: Rgba::BLACK, paper: Rgba::rgb(245, 238, 222) }),
        ("wooden_frame", Filter::WoodenFrame { width: 4, color: Rgba::rgb(139, 90, 43), seed: 20 }),
        ("xray_scan", Filter::XRayScan { position: 0.5, tint: Rgba::rgb(120, 255, 200) }),
        ("zigzag_border", Filter::ZigZagBorder { width: 4, color: Rgba::rgb(200, 40, 60), accent: Rgba::WHITE }),
    ]
}

/// The enum variant's name, read from the Debug form (`Bevel { size: 8, .. }` gives `Bevel`).
fn variant(filter: &Filter) -> String {
    let debug = format!("{filter:?}");
    debug.split([' ', '{']).next().unwrap_or_default().to_string()
}

fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots").join(format!("x4_{name}.png"))
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
fn x4_filters_match_their_snapshots() {
    let input = pattern();
    for (name, filter) in &cases() {
        let out = run(filter, &input);
        let changed = out.width() != input.width() || out.height() != input.height() || out.pixels() != input.pixels();
        assert!(changed, "{name} left the test card unchanged");
        snapshot(name, &out);
    }
}

#[test]
fn x4_cases_cover_every_x4_default() {
    let cases = cases();
    assert_eq!(cases.len(), X4_VARIANTS.len());
    let defaults: Vec<String> = Filter::defaults().iter().map(variant).collect();
    assert_eq!(defaults.iter().filter(|d| X4_VARIANTS.contains(&d.as_str())).count(), X4_VARIANTS.len());
    for name in X4_VARIANTS {
        assert!(defaults.iter().any(|d| d == name), "{name} is not in defaults()");
        let cased = cases.iter().filter(|(_, f)| variant(f) == name).count();
        assert_eq!(cased, 1, "{name} needs exactly one snapshot case");
    }
}

#[test]
fn x4_filters_run_on_odd_sizes() {
    for (w, h) in [(1, 1), (3, 1), (1, 5), (7, 2)] {
        let input = card(w, h);
        for (name, filter) in cases() {
            let out = try_run(&filter, &input).unwrap_or_else(|e| panic!("{name} on {w}x{h}: {e}"));
            assert!(out.width() >= w && out.height() >= h, "{name} on {w}x{h} shrank to {}x{}", out.width(), out.height());
        }
    }
}

#[test]
fn x4_seeded_filters_repeat_and_depend_on_the_seed() {
    let input = pattern();
    let seeded = |seed: u64| {
        vec![
            Filter::PerlinNoise { scale: 4, strength: 0.5, seed },
            Filter::PlasmaEnergyArcs { scale: 4, strength: 0.9, color: Rgba::rgb(120, 220, 255), seed },
            Filter::Pointillism { cell: 4, seed },
            Filter::RainyWindow { drops: 6, size: 5, blur_radius: 2, seed },
            Filter::RisoPrint { ink: Rgba::BLACK, paper: Rgba::WHITE, grain: 0.5, seed },
            Filter::RustCorrosion { amount: 0.8, scale: 6, seed },
            Filter::SmokeOverlay { opacity: 0.7, scale: 8, seed },
            Filter::SnowfallDepthFog { flakes: 30, fog: 0.6, seed },
            Filter::SoftDiffusion { radius: 2, strength: 0.8, seed },
            Filter::StainedGlass { cell: 6, lead: 1, seed },
            Filter::StainedGlassBorder { width: 4, pane: 4, lead: Rgba::BLACK, seed },
            Filter::StarfieldBorder { width: 4, background: Rgba::BLACK, star: Rgba::WHITE, seed },
            Filter::TVStatic { amount: 0.3, seed },
            Filter::VhsTapeDamage { jitter: 3, bleed: 2, noise: 0.5, seed },
            Filter::VintagePrintDamage { fade: 0.6, scratches: 3, seed },
            Filter::VoronoiMosaic { cell: 5, seed },
            Filter::WatercolorBorder { width: 4, color: Rgba::rgb(70, 130, 190), seed },
            Filter::WetPlateCollodion { vignette: 0.6, seed },
            Filter::WindStreaks { streaks: 6, length: 8, strength: 0.8, seed },
            Filter::WoodenFrame { width: 4, color: Rgba::rgb(139, 90, 43), seed },
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
fn x4_frames_grow_the_canvas_by_their_width() {
    let input = pattern();
    let frames = [
        Filter::RainbowBorder { width: 4 },
        Filter::RopeBorder { width: 4, color: Rgba::BLACK, accent: Rgba::WHITE },
        Filter::StainedGlassBorder { width: 4, pane: 4, lead: Rgba::BLACK, seed: 1 },
        Filter::StarfieldBorder { width: 4, background: Rgba::BLACK, star: Rgba::WHITE, seed: 1 },
        Filter::WatercolorBorder { width: 4, color: Rgba::BLACK, seed: 1 },
        Filter::WoodenFrame { width: 4, color: Rgba::BLACK, seed: 1 },
        Filter::ZigZagBorder { width: 4, color: Rgba::BLACK, accent: Rgba::WHITE },
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
    // The paper mount is thicker underneath: 4 px on the sides and top, 4 + 6 px below.
    let polaroid = run(&Filter::PolaroidFrame { border: 4, bottom: 6, color: Rgba::WHITE }, &input);
    assert_eq!((polaroid.width(), polaroid.height()), (N + 8, N + 14));
    for y in 0..N {
        for x in 0..N {
            assert_eq!(px(&polaroid, x + 4, y + 4), px(&input, x, y), "polaroid: pixel {x},{y}");
        }
    }
    // The window grows by its shadow on every side.
    let window = run(&Filter::Windows11 { radius: 4, shadow: 4 }, &input);
    assert_eq!((window.width(), window.height()), (N + 8, N + 8));
}

#[test]
fn x4_frames_past_the_pixel_budget_are_refused() {
    let input = pattern();
    let huge = [
        Filter::RainbowBorder { width: u32::MAX },
        Filter::PolaroidFrame { border: 20_000, bottom: 0, color: Rgba::BLACK },
        Filter::WoodenFrame { width: 20_000, color: Rgba::BLACK, seed: 0 },
        Filter::Windows11 { radius: 4, shadow: 20_000 },
    ];
    for filter in &huge {
        assert!(matches!(try_run(filter, &input), Err(EffectError::Invalid(_))), "{filter:?}");
    }
}

#[test]
fn x4_degenerate_parameters_render_without_panicking() {
    let input = pattern();
    let odd = [
        Filter::PerlinNoise { scale: 0, strength: f32::NAN, seed: 0 },
        Filter::PixelSorting { threshold: f32::NAN, vertical: true },
        Filter::PlasmaEnergyArcs { scale: 0, strength: f32::INFINITY, color: Rgba::WHITE, seed: 0 },
        Filter::Pointillism { cell: 0, seed: 0 },
        Filter::PolaroidFrame { border: 0, bottom: 0, color: Rgba::BLACK },
        Filter::PopArtGrid { tiles: 0 },
        Filter::RadialRainbow { strength: f32::NAN, bands: 0 },
        Filter::RainbowBorder { width: 0 },
        Filter::RainyWindow { drops: u32::MAX, size: 0, blur_radius: u32::MAX, seed: 0 },
        Filter::RetroSunset { strength: f32::INFINITY },
        Filter::RisoPrint { ink: Rgba::BLACK, paper: Rgba::WHITE, grain: f32::NAN, seed: 0 },
        Filter::RopeBorder { width: 0, color: Rgba::BLACK, accent: Rgba::WHITE },
        Filter::RustCorrosion { amount: f32::NAN, scale: 0, seed: 0 },
        Filter::Scanline { spacing: 0, strength: f32::NAN },
        Filter::SmokeOverlay { opacity: f32::NAN, scale: 0, seed: 0 },
        Filter::SnowfallDepthFog { flakes: u32::MAX, fog: f32::NAN, seed: 0 },
        Filter::SobelEdge { gain: f32::NAN },
        Filter::SoftDiffusion { radius: u32::MAX, strength: f32::NAN, seed: 0 },
        Filter::SpinBlur { angle: f32::INFINITY, center_x: f32::NAN, center_y: f32::NAN },
        Filter::StainedGlass { cell: 0, lead: u32::MAX, seed: 0 },
        Filter::StainedGlassBorder { width: 0, pane: 0, lead: Rgba::BLACK, seed: 0 },
        Filter::StarfieldBorder { width: 0, background: Rgba::BLACK, star: Rgba::WHITE, seed: 0 },
        Filter::StarFilter { threshold: 1.0, length: u32::MAX, strength: f32::NAN, color: Rgba::WHITE },
        Filter::StripeOverlay { period: 0, opacity: f32::NAN, color: Rgba::WHITE },
        Filter::SurfaceBlur { radius: u32::MAX, threshold: 0 },
        Filter::ThermalVision { strength: f32::NAN },
        Filter::TiltShift { focus: f32::NAN, band: f32::NAN, radius: u32::MAX },
        Filter::TrianglePixelate { size: 0 },
        Filter::TVStatic { amount: f32::NAN, seed: 0 },
        Filter::UnsharpMask { radius: u32::MAX, amount: f32::INFINITY, threshold: 255 },
        Filter::VhsTapeDamage { jitter: u32::MAX, bleed: u32::MAX, noise: f32::NAN, seed: 0 },
        Filter::VintagePrintDamage { fade: f32::NAN, scratches: u32::MAX, seed: 0 },
        Filter::VoronoiMosaic { cell: 0, seed: 0 },
        Filter::WatercolorBorder { width: 0, color: Rgba::BLACK, seed: 0 },
        Filter::WatercolorKuwahara { radius: u32::MAX },
        Filter::WetPlateCollodion { vignette: f32::NAN, seed: 0 },
        Filter::Windows11 { radius: u32::MAX, shadow: 0 },
        Filter::WindStreaks { streaks: u32::MAX, length: u32::MAX, strength: f32::NAN, seed: 0 },
        Filter::WoodcutPrint { line: 0, ink: Rgba::BLACK, paper: Rgba::WHITE },
        Filter::WoodenFrame { width: 0, color: Rgba::BLACK, seed: 0 },
        Filter::XRayScan { position: f32::NAN, tint: Rgba::WHITE },
        Filter::ZigZagBorder { width: 0, color: Rgba::BLACK, accent: Rgba::WHITE },
    ];
    for filter in &odd {
        let out = try_run(filter, &input).unwrap_or_else(|e| panic!("{filter:?}: {e}"));
        assert!(out.width() >= 1 && out.height() >= 1, "{filter:?}");
    }
}
