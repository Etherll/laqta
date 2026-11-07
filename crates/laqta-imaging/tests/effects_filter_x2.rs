#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]
//! Filters X2 (M7 D5): one PNG snapshot per filter on the 24x24 test card, plus property checks.
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

/// The 41 X2 variants, by enum variant name.
const X2_VARIANTS: [&str; 41] = [
    "Anaglyph3D",
    "AnamorphicLensFlare",
    "AnimeSpeedLines",
    "ArtDecoBorder",
    "ASCIIArt",
    "Bevel",
    "BlockGlitch",
    "BloodSplash",
    "Bloom",
    "BlueprintDrawing",
    "BrickWallBorder",
    "CandyCaneBorder",
    "CartoonStickerCutout",
    "CelticKnotBorder",
    "ChainLinkBorder",
    "ChalkDrawing",
    "ChromaticAberration",
    "ChromeMetallicBorder",
    "CircuitBoardBorder",
    "ClaymationTexture",
    "Clouds",
    "ColorHalftone",
    "ColorSplash",
    "CrimsonBorder",
    "Crosshatch",
    "CRT",
    "CrystalizeShards",
    "CrystalPrism",
    "Cyanotype",
    "DatamoshSmear",
    "DiamondPixelate",
    "DisposableCamera",
    "DotMatrix",
    "DoubleExposure",
    "DreamGlow",
    "EdgeFeather",
    "Embroidery",
    "Engraving",
    "EtchedGlass",
    "FakeHair",
    "FilmGrain",
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

/// Every X2 filter, with parameters sized for the 24x24 card. The file name is `x2_<name>.png`.
fn cases() -> Vec<(&'static str, Filter)> {
    vec![
        ("anaglyph_3d", Filter::Anaglyph3D { parallax: 2 }),
        (
            "anamorphic_lens_flare",
            Filter::AnamorphicLensFlare { threshold: 0.6, length: 12, strength: 0.8, color: Rgba::rgb(120, 180, 255) },
        ),
        ("anime_speed_lines", Filter::AnimeSpeedLines { lines: 24, inner: 0.3, strength: 0.7, color: Rgba::WHITE, seed: 3 }),
        ("art_deco_border", Filter::ArtDecoBorder { width: 4, color: Rgba::rgb(212, 175, 55), accent: Rgba::rgb(20, 20, 20) }),
        ("ascii_art", Filter::ASCIIArt { cell: 3, color: Rgba::rgb(120, 255, 120) }),
        ("bevel", Filter::Bevel { size: 3, strength: 0.6 }),
        ("block_glitch", Filter::BlockGlitch { count: 4, max_shift: 5, seed: 9 }),
        ("blood_splash", Filter::BloodSplash { count: 3, size: 0.25, color: Rgba::rgb(138, 3, 3), seed: 4 }),
        ("bloom", Filter::Bloom { threshold: 0.6, radius: 2, strength: 0.8 }),
        ("blueprint_drawing", Filter::BlueprintDrawing { background: Rgba::rgb(20, 60, 130), line: Rgba::rgb(230, 240, 255) }),
        ("brick_wall_border", Filter::BrickWallBorder { width: 8, brick: Rgba::rgb(170, 70, 50), mortar: Rgba::rgb(200, 195, 185) }),
        ("candy_cane_border", Filter::CandyCaneBorder { width: 4, color: Rgba::rgb(200, 30, 40), accent: Rgba::WHITE }),
        ("cartoon_sticker_cutout", Filter::CartoonStickerCutout { levels: 3, outline: 2, color: Rgba::WHITE }),
        ("celtic_knot_border", Filter::CelticKnotBorder { width: 6, color: Rgba::rgb(40, 110, 70), accent: Rgba::rgb(230, 200, 90) }),
        ("chain_link_border", Filter::ChainLinkBorder { width: 6, color: Rgba::rgb(170, 175, 180), accent: Rgba::rgb(60, 60, 65) }),
        ("chalk_drawing", Filter::ChalkDrawing { threshold: 0.2, board: Rgba::rgb(40, 66, 56), chalk: Rgba::rgb(240, 240, 232), seed: 2 }),
        ("chromatic_aberration", Filter::ChromaticAberration { amount: 1.0 }),
        ("chrome_metallic_border", Filter::ChromeMetallicBorder { width: 4, color: Rgba::rgb(210, 215, 225) }),
        ("circuit_board_border", Filter::CircuitBoardBorder { width: 6, board: Rgba::rgb(10, 60, 30), trace: Rgba::rgb(200, 160, 60) }),
        ("claymation_texture", Filter::ClaymationTexture { radius: 1, depth: 2.0 }),
        ("clouds", Filter::Clouds { scale: 6, opacity: 0.7, color: Rgba::WHITE, seed: 5 }),
        ("color_halftone", Filter::ColorHalftone { cell: 4 }),
        ("color_splash", Filter::ColorSplash { target: Rgba::rgb(220, 40, 40), tolerance: 0.2 }),
        ("crimson_border", Filter::CrimsonBorder { width: 5, color: Rgba::rgb(140, 20, 40), accent: Rgba::rgb(220, 180, 120) }),
        ("crosshatch", Filter::Crosshatch { spacing: 3 }),
        ("crt", Filter::CRT { scanlines: 0.4, mask: 0.3, vignette: 0.5 }),
        ("crystalize_shards", Filter::CrystalizeShards { size: 6, seed: 7 }),
        ("crystal_prism", Filter::CrystalPrism { size: 6, dispersion: 0.6, seed: 7 }),
        ("cyanotype", Filter::Cyanotype { shadow: Rgba::rgb(12, 45, 94), highlight: Rgba::rgb(240, 246, 255) }),
        ("datamosh_smear", Filter::DatamoshSmear { block: 6, max_shift: 4, seed: 2 }),
        ("diamond_pixelate", Filter::DiamondPixelate { size: 4 }),
        ("disposable_camera", Filter::DisposableCamera { warmth: 0.6, grain: 0.3, vignette: 0.5, seed: 3 }),
        ("dot_matrix", Filter::DotMatrix { cell: 4, background: Rgba::rgb(10, 10, 10) }),
        ("double_exposure", Filter::DoubleExposure { offset_x: 3, offset_y: 2, opacity: 0.5 }),
        ("dream_glow", Filter::DreamGlow { radius: 2, strength: 0.6 }),
        ("edge_feather", Filter::EdgeFeather { size: 4 }),
        ("embroidery", Filter::Embroidery { cell: 4, cloth: Rgba::rgb(235, 225, 205) }),
        ("engraving", Filter::Engraving { spacing: 3, paper: Rgba::rgb(245, 238, 220), ink: Rgba::rgb(30, 25, 20) }),
        ("etched_glass", Filter::EtchedGlass { radius: 2, seed: 4 }),
        ("fake_hair", Filter::FakeHair { strands: 12, length: 10, color: Rgba::rgb(40, 25, 15), seed: 6 }),
        ("film_grain", Filter::FilmGrain { amount: 0.3, seed: 8 }),
    ]
}

/// The enum variant's name, read from the Debug form (`Bevel { size: 8, .. }` gives `Bevel`).
fn variant(filter: &Filter) -> String {
    let debug = format!("{filter:?}");
    debug.split([' ', '{']).next().unwrap_or_default().to_string()
}

fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots").join(format!("x2_{name}.png"))
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
fn x2_filters_match_their_snapshots() {
    let input = pattern();
    for (name, filter) in &cases() {
        let out = run(filter, &input);
        let changed = out.width() != input.width() || out.height() != input.height() || out.pixels() != input.pixels();
        assert!(changed, "{name} left the test card unchanged");
        snapshot(name, &out);
    }
}

#[test]
fn x2_cases_cover_every_x2_default() {
    let cases = cases();
    assert_eq!(cases.len(), X2_VARIANTS.len());
    let defaults: Vec<String> = Filter::defaults().iter().map(variant).collect();
    assert_eq!(defaults.iter().filter(|d| X2_VARIANTS.contains(&d.as_str())).count(), X2_VARIANTS.len());
    for name in X2_VARIANTS {
        assert!(defaults.iter().any(|d| d == name), "{name} is not in defaults()");
        let cased = cases.iter().filter(|(_, f)| variant(f) == name).count();
        assert_eq!(cased, 1, "{name} needs exactly one snapshot case");
    }
}

#[test]
fn x2_filters_run_on_odd_sizes() {
    for (w, h) in [(1, 1), (3, 1), (1, 5), (7, 2)] {
        let input = card(w, h);
        for (name, filter) in cases() {
            let out = try_run(&filter, &input).unwrap_or_else(|e| panic!("{name} on {w}x{h}: {e}"));
            assert!(out.width() >= w && out.height() >= h, "{name} on {w}x{h} shrank to {}x{}", out.width(), out.height());
        }
    }
}

#[test]
fn x2_seeded_filters_repeat_and_depend_on_the_seed() {
    let input = pattern();
    let seeded = |seed: u64| {
        vec![
            Filter::AnimeSpeedLines { lines: 24, inner: 0.3, strength: 0.7, color: Rgba::WHITE, seed },
            Filter::BlockGlitch { count: 4, max_shift: 5, seed },
            Filter::BloodSplash { count: 3, size: 0.25, color: Rgba::rgb(138, 3, 3), seed },
            Filter::ChalkDrawing { threshold: 0.2, board: Rgba::BLACK, chalk: Rgba::WHITE, seed },
            Filter::Clouds { scale: 6, opacity: 0.7, color: Rgba::WHITE, seed },
            Filter::CrystalizeShards { size: 6, seed },
            Filter::CrystalPrism { size: 6, dispersion: 0.6, seed },
            Filter::DatamoshSmear { block: 6, max_shift: 4, seed },
            Filter::DisposableCamera { warmth: 0.6, grain: 0.3, vignette: 0.5, seed },
            Filter::EtchedGlass { radius: 2, seed },
            Filter::FakeHair { strands: 12, length: 10, color: Rgba::BLACK, seed },
            Filter::FilmGrain { amount: 0.3, seed },
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
fn x2_frames_grow_the_canvas_by_their_width() {
    let input = pattern();
    let frames = [
        Filter::ArtDecoBorder { width: 4, color: Rgba::BLACK, accent: Rgba::WHITE },
        Filter::BrickWallBorder { width: 4, brick: Rgba::BLACK, mortar: Rgba::WHITE },
        Filter::CandyCaneBorder { width: 4, color: Rgba::BLACK, accent: Rgba::WHITE },
        Filter::CelticKnotBorder { width: 4, color: Rgba::BLACK, accent: Rgba::WHITE },
        Filter::ChainLinkBorder { width: 4, color: Rgba::BLACK, accent: Rgba::WHITE },
        Filter::ChromeMetallicBorder { width: 4, color: Rgba::BLACK },
        Filter::CircuitBoardBorder { width: 4, board: Rgba::BLACK, trace: Rgba::WHITE },
        Filter::CrimsonBorder { width: 4, color: Rgba::BLACK, accent: Rgba::WHITE },
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
    let sticker = run(&Filter::CartoonStickerCutout { levels: 3, outline: 2, color: Rgba::WHITE }, &input);
    assert_eq!((sticker.width(), sticker.height()), (N + 4, N + 4));
}

#[test]
fn x2_frames_past_the_pixel_budget_are_refused() {
    let input = pattern();
    let huge = [
        Filter::ArtDecoBorder { width: u32::MAX, color: Rgba::BLACK, accent: Rgba::WHITE },
        Filter::BrickWallBorder { width: 20_000, brick: Rgba::BLACK, mortar: Rgba::WHITE },
        Filter::ChromeMetallicBorder { width: 20_000, color: Rgba::BLACK },
    ];
    for filter in &huge {
        assert!(matches!(try_run(filter, &input), Err(EffectError::Invalid(_))), "{filter:?}");
    }
}

#[test]
fn x2_degenerate_parameters_render_without_panicking() {
    let input = pattern();
    let odd = [
        Filter::AnamorphicLensFlare { threshold: 1.0, length: u32::MAX, strength: f32::NAN, color: Rgba::WHITE },
        Filter::AnimeSpeedLines { lines: u32::MAX, inner: f32::INFINITY, strength: f32::NAN, color: Rgba::WHITE, seed: 0 },
        Filter::ASCIIArt { cell: 0, color: Rgba::WHITE },
        Filter::Bevel { size: u32::MAX, strength: f32::INFINITY },
        Filter::BlockGlitch { count: u32::MAX, max_shift: u32::MAX, seed: 0 },
        Filter::BloodSplash { count: u32::MAX, size: f32::NAN, color: Rgba::WHITE, seed: 0 },
        Filter::BloodSplash { count: u32::MAX, size: 1e9, color: Rgba::WHITE, seed: 0 },
        Filter::Bloom { threshold: f32::NAN, radius: u32::MAX, strength: f32::INFINITY },
        Filter::ChromaticAberration { amount: f32::NAN },
        Filter::ClaymationTexture { radius: u32::MAX, depth: f32::NAN },
        Filter::Clouds { scale: 0, opacity: f32::NAN, color: Rgba::WHITE, seed: 0 },
        Filter::ColorHalftone { cell: 0 },
        Filter::ColorSplash { target: Rgba::WHITE, tolerance: f32::NAN },
        Filter::Crosshatch { spacing: 0 },
        Filter::CRT { scanlines: f32::NAN, mask: 9.0, vignette: -3.0 },
        Filter::CrystalizeShards { size: 0, seed: 0 },
        Filter::CrystalPrism { size: u32::MAX, dispersion: f32::INFINITY, seed: 0 },
        Filter::DatamoshSmear { block: 0, max_shift: u32::MAX, seed: 0 },
        Filter::DiamondPixelate { size: 0 },
        Filter::DotMatrix { cell: 0, background: Rgba::BLACK },
        Filter::DoubleExposure { offset_x: i32::MIN, offset_y: i32::MAX, opacity: f32::NAN },
        Filter::DreamGlow { radius: u32::MAX, strength: f32::INFINITY },
        Filter::EdgeFeather { size: u32::MAX },
        Filter::Embroidery { cell: 0, cloth: Rgba::WHITE },
        Filter::Engraving { spacing: 0, paper: Rgba::WHITE, ink: Rgba::BLACK },
        Filter::EtchedGlass { radius: u32::MAX, seed: 0 },
        Filter::FakeHair { strands: u32::MAX, length: u32::MAX, color: Rgba::BLACK, seed: 0 },
        Filter::FilmGrain { amount: f32::NAN, seed: 0 },
    ];
    for filter in &odd {
        let out = try_run(filter, &input).unwrap_or_else(|e| panic!("{filter:?}: {e}"));
        assert!(out.width() >= 1 && out.height() >= 1, "{filter:?}");
    }
}
