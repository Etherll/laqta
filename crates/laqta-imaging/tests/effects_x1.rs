//! Snapshot and value tests for the 43 M7 X1 effects (M7 D5): 17 adjustments, 20 manipulations and
//! 6 drawings. Each case runs on one of the existing test cards: the 16x16 card of `effects_adjust.rs`
//! or the 20x16 card with transparent margins of `effects_dm.rs`. It must match
//! `tests/snapshots/x1_<name>.png` by decoded pixels. A missing snapshot is written and passes;
//! `LAQTA_BLESS=1` rewrites the X1 snapshots.
#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

use std::path::PathBuf;

use laqta_domain::color::Rgba;
use laqta_domain::effects::Effect;
use laqta_domain::effects::adjust::{Adjust, Channel};
use laqta_domain::effects::draw::{Draw, ShapeKind};
use laqta_domain::effects::manip::Manip;
use laqta_domain::{Bitmap, PixelFormat};
use laqta_imaging::effects::{EffectContext, apply};

/// Which card a case runs on.
#[derive(Clone, Copy)]
enum Card {
    /// The 16x16 adjustment card.
    Small,
    /// The 20x16 card with transparent margins.
    Wide,
}

fn pool() -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap()
}

fn bitmap(w: u32, h: u32, px: Vec<u8>) -> Bitmap {
    Bitmap::new(w, h, PixelFormat::Rgba8, px.into()).unwrap()
}

/// The 16x16 card of `effects_adjust.rs`: a gradient, primaries with alpha falling off, a grey ramp
/// and a diagonal pattern.
fn small_card() -> Bitmap {
    const SIZE: u32 = 16;
    const PRIMARIES: [[u8; 3]; 8] =
        [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0], [0, 255, 255], [255, 0, 255], [255, 255, 255], [0, 0, 0]];
    const ALPHA_TAIL: [u8; 4] = [128, 128, 64, 0];
    let mut px = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let p: [u8; 4] = match y {
                0..4 => [(x * 17) as u8, (y * 64) as u8, (255 - x * 17) as u8, 255],
                4..8 => {
                    let [r, g, b] = PRIMARIES[(x % 8) as usize];
                    let a = if x < 12 { 255 } else { ALPHA_TAIL[(x - 12) as usize] };
                    [r, g, b, a]
                }
                8..12 => {
                    let v = (x * 17) as u8;
                    [v, v, v, 255]
                }
                _ => [(x * y) as u8, ((x + y) * 8) as u8, ((x ^ y) * 16) as u8, 255],
            };
            px.extend_from_slice(&p);
        }
    }
    bitmap(SIZE, SIZE, px)
}

/// The 20x16 card of `effects_dm.rs`: an opaque block, with a semi-transparent corner, inside a
/// transparent margin.
fn wide_card() -> Bitmap {
    const W: u32 = 20;
    const H: u32 = 16;
    let mut px = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for x in 0..W {
            let inside = (2..14).contains(&x) && (1..11).contains(&y);
            let p = if inside {
                let a = if x >= 9 && y >= 6 { 128 } else { 255 };
                [((x - 2) * 21) as u8, ((y - 1) * 16) as u8, ((x - 2 + y - 1) * 7) as u8, a]
            } else {
                [0, 0, 0, 0]
            };
            px.extend_from_slice(&p);
        }
    }
    bitmap(W, H, px)
}

fn card_image(card: Card) -> Bitmap {
    match card {
        Card::Small => small_card(),
        Card::Wide => wide_card(),
    }
}

/// One case per X1 effect: snapshot name, card, and the effect with parameters that change the card.
fn cases() -> Vec<(&'static str, Card, Effect)> {
    let adj = Effect::Adjust;
    let man = Effect::Manip;
    let drw = Effect::Draw;
    vec![
        ("x1_auto_contrast", Card::Wide, adj(Adjust::AutoContrast { clip: 0.01 })),
        ("x1_channel_swap", Card::Small, adj(Adjust::ChannelSwap { red: Channel::Green, green: Channel::Red, blue: Channel::Blue })),
        ("x1_clarity", Card::Small, adj(Adjust::Clarity { strength: 0.8, radius: 3 })),
        (
            "x1_color_balance",
            Card::Small,
            adj(Adjust::ColorBalance { shadows: [0.2, 0.0, -0.2], midtones: [0.0, 0.1, 0.0], highlights: [-0.1, 0.0, 0.2] }),
        ),
        ("x1_cross_process", Card::Small, adj(Adjust::CrossProcess { strength: 1.0 })),
        ("x1_dehaze", Card::Small, adj(Adjust::Dehaze { strength: 0.9, radius: 2 })),
        (
            "x1_duotone_gradient_map",
            Card::Small,
            adj(Adjust::DuotoneGradientMap { shadow: Rgba::rgb(20, 20, 80), highlight: Rgba::rgb(255, 220, 140), strength: 1.0 }),
        ),
        ("x1_exposure", Card::Small, adj(Adjust::Exposure { stops: 1.0 })),
        ("x1_film_emulation", Card::Small, adj(Adjust::FilmEmulation { strength: 0.8, grain: 0.5, seed: 3 })),
        ("x1_levels", Card::Small, adj(Adjust::Levels { in_black: 40, in_white: 200, gamma: 1.4, out_black: 10, out_white: 245 })),
        ("x1_lomo", Card::Small, adj(Adjust::Lomo { strength: 0.8 })),
        ("x1_shadows_highlights", Card::Small, adj(Adjust::ShadowsHighlights { shadows: 0.6, highlights: 0.5, radius: 3 })),
        ("x1_solarize", Card::Small, adj(Adjust::Solarize { threshold: 128 })),
        (
            "x1_split_toning",
            Card::Small,
            adj(Adjust::SplitToning { shadow: Rgba::rgb(0, 64, 160), highlight: Rgba::rgb(255, 180, 80), strength: 1.0, balance: 0.2 }),
        ),
        ("x1_temperature_tint", Card::Small, adj(Adjust::TemperatureTint { temperature: 0.6, tint: -0.4 })),
        ("x1_threshold", Card::Small, adj(Adjust::Threshold { level: 100 })),
        ("x1_vibrance", Card::Small, adj(Adjust::Vibrance { value: 1.0 })),
        ("x1_cylinder_wrap", Card::Wide, man(Manip::CylinderWrap { strength: 0.6 })),
        ("x1_displacement_map", Card::Wide, man(Manip::DisplacementMap { amount: 3, scale: 4, seed: 5 })),
        ("x1_fisheye_lens", Card::Wide, man(Manip::FisheyeLens { strength: 0.8 })),
        ("x1_fold_crease", Card::Wide, man(Manip::FoldCreaseWarp { angle: 20.0, position: 0.45, shade: 0.6 })),
        ("x1_glass_sphere", Card::Wide, man(Manip::GlassSphere { radius: 6, refraction: 0.6 })),
        ("x1_kaleidoscope", Card::Wide, man(Manip::Kaleidoscope { segments: 5, rotation: 15.0 })),
        (
            "x1_liquify_push_smudge",
            Card::Wide,
            man(Manip::LiquifyPushSmudge { centre: (0.4, 0.5), radius: 7, push: (5.0, 2.0), smudge: 0.5 }),
        ),
        ("x1_mirror_tiles", Card::Wide, man(Manip::MirrorTiles { columns: 3, rows: 2 })),
        ("x1_page_curl", Card::Wide, man(Manip::PageCurl { progress: 0.3, shadow: 0.6, back: Rgba::rgb(245, 240, 230) })),
        (
            "x1_perspective_warp",
            Card::Wide,
            man(Manip::PerspectiveWarp {
                top_left: (0.15, 0.1),
                top_right: (0.9, 0.0),
                bottom_right: (1.0, 1.0),
                bottom_left: (0.0, 0.85),
            }),
        ),
        ("x1_pinch_bulge", Card::Wide, man(Manip::PinchBulge { amount: -0.6, radius: 0 })),
        ("x1_polar_warp", Card::Wide, man(Manip::PolarWarp { strength: 1.0 })),
        ("x1_ripple_refraction", Card::Wide, man(Manip::RippleRefraction { amplitude: 2, wavelength: 6 })),
        ("x1_rotate3d", Card::Wide, man(Manip::Rotate3D { angle_x: 25.0, angle_y: 35.0 })),
        ("x1_rotate3d_box", Card::Wide, man(Manip::Rotate3DBox { angle_x: 25.0, angle_y: 35.0, depth: 6 })),
        ("x1_symmetry", Card::Wide, man(Manip::Symmetry { vertical: true, reverse: true })),
        ("x1_tile_repeat", Card::Wide, man(Manip::TileRepeat { columns: 3, rows: 2, gap: 1, color: Rgba::rgb(255, 0, 255) })),
        ("x1_tile_shuffle", Card::Wide, man(Manip::TileShuffle { columns: 4, rows: 4, seed: 9 })),
        ("x1_twirl", Card::Wide, man(Manip::Twirl { angle: 120.0, radius: 0 })),
        ("x1_wave_distortion", Card::Wide, man(Manip::WaveDistortion { amplitude: 3, wavelength: 8, phase: 0.0 })),
        ("x1_confetti", Card::Wide, drw(Draw::Confetti { count: 15, min_size: 2, max_size: 4, seed: 11 })),
        ("x1_draw_line", Card::Wide, drw(Draw::DrawLine { from: (1, 1), to: (18, 13), width: 2, color: Rgba::rgb(255, 0, 0) })),
        (
            "x1_draw_shape",
            Card::Wide,
            drw(Draw::DrawShape {
                shape: ShapeKind::Ellipse,
                x: 2,
                y: 2,
                width: 14,
                height: 10,
                fill: Some(Rgba::rgb(255, 255, 0)),
                stroke: Rgba::BLACK,
                stroke_width: 1,
            }),
        ),
        ("x1_fake_cursor", Card::Wide, drw(Draw::FakeCursor { x: 4, y: 3, size: 12, shadow: true })),
        (
            "x1_gradient_overlay",
            Card::Wide,
            drw(Draw::GradientOverlay { start: Rgba::TRANSPARENT, end: Rgba { r: 0, g: 0, b: 0, a: 200 }, angle: 90.0, opacity: 1.0 }),
        ),
        ("x1_snowflakes", Card::Wide, drw(Draw::Snowflakes { count: 20, min_size: 1, max_size: 3, seed: 4 })),
    ]
}

fn run(image: &Bitmap, effect: &Effect) -> Bitmap {
    pool().install(|| apply(image, effect, &EffectContext::none())).unwrap()
}

/// Dimensions and RGBA8 pixels of a PNG.
fn decode(bytes: &[u8]) -> ((u32, u32), Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    ((info.width, info.height), buf[..info.buffer_size()].to_vec())
}

/// Compares `actual` with `tests/snapshots/<name>.png` by pixels. Writes the snapshot when it is
/// missing or when `LAQTA_BLESS=1`.
fn snapshot(name: &str, actual: &Bitmap) -> Result<(), String> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("snapshots");
    let path = dir.join(format!("{name}.png"));
    let bytes = pool().install(|| laqta_imaging::encode_png(actual)).map_err(|e| format!("{name}: {e:?}"))?;
    let bless = std::env::var("LAQTA_BLESS").is_ok_and(|v| v == "1");
    if bless || !path.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        return Ok(());
    }
    let expected = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if decode(&bytes) == decode(&expected) {
        Ok(())
    } else {
        Err(format!("{name} differs from {} (run with LAQTA_BLESS=1 to accept)", path.display()))
    }
}

#[test]
fn every_x1_effect_matches_its_snapshot() {
    assert_eq!(cases().len(), 43);
    let mut failures = Vec::new();
    for (name, card, effect) in cases() {
        let out = run(&card_image(card), &effect);
        if let Err(msg) = snapshot(name, &out) {
            failures.push(msg);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The X1 variants in `defaults()`. The first 15 adjustments, 10 manipulations and 8 drawings are
/// M3's, and their own tests cover them.
fn x1_defaults() -> Vec<Effect> {
    let x1: Vec<Effect> = Adjust::defaults()
        .into_iter()
        .skip(15)
        .map(Effect::Adjust)
        .chain(Manip::defaults().into_iter().skip(10).map(Effect::Manip))
        .chain(Draw::defaults().into_iter().skip(8).map(Effect::Draw))
        .collect();
    assert_eq!(x1.len(), 43);
    x1
}

#[test]
fn every_x1_variant_has_a_snapshot_case() {
    let cases = cases();
    for effect in x1_defaults() {
        assert!(cases.iter().any(|(_, _, c)| c.name() == effect.name()), "no snapshot case for {}", effect.name());
    }
}

#[test]
fn every_x1_default_runs_on_every_card_and_on_one_pixel() {
    let single = bitmap(1, 1, vec![200, 100, 50, 255]);
    for effect in x1_defaults() {
        for input in [small_card(), wide_card(), single.clone()] {
            let out = run(&input, &effect);
            assert!(out.width() >= 1 && out.height() >= 1, "{} gives an empty image", effect.name());
        }
    }
}

#[test]
fn every_x1_effect_changes_its_card() {
    for (name, card, effect) in cases() {
        let input = card_image(card);
        let out = run(&input, &effect);
        let same_size = (out.width(), out.height()) == (input.width(), input.height());
        assert!(!same_size || out.pixels() != input.pixels(), "{name} leaves the card unchanged");
    }
}

#[test]
fn seeded_effects_depend_only_on_the_seed() {
    let seeded = |seed: u64| {
        vec![
            Effect::Adjust(Adjust::FilmEmulation { strength: 0.8, grain: 0.5, seed }),
            Effect::Manip(Manip::DisplacementMap { amount: 3, scale: 4, seed }),
            Effect::Manip(Manip::TileShuffle { columns: 4, rows: 4, seed }),
            Effect::Draw(Draw::Confetti { count: 15, min_size: 2, max_size: 4, seed }),
            Effect::Draw(Draw::Snowflakes { count: 20, min_size: 1, max_size: 3, seed }),
        ]
    };
    let input = wide_card();
    for ((a, b), other) in seeded(5).iter().zip(seeded(5).iter()).zip(seeded(6).iter()) {
        assert_eq!(run(&input, a).pixels(), run(&input, b).pixels(), "{}", a.name());
        assert_ne!(run(&input, a).pixels(), run(&input, other).pixels(), "{}", a.name());
    }
}
