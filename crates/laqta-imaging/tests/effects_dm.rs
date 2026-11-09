#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Tests for the 8 drawing and 10 manipulation effects. Each effect has a PNG snapshot in
//! `tests/snapshots`, taken from a 20x16 input with transparent areas, plus pixel checks.
//! A missing snapshot is written on the first run. `LAQTA_BLESS=1` rewrites every snapshot.

use std::path::PathBuf;

use laqta_domain::color::Rgba;
use laqta_domain::effects::draw::{BorderKind, Draw, Placement, SizeMode};
use laqta_domain::effects::manip::{ForceMethod, Manip};
use laqta_domain::effects::{Effect, TextSpec};
use laqta_domain::{Bitmap, PixelFormat};
use laqta_imaging::effects::{EffectContext, EffectError, apply};

const W: u32 = 20;
const H: u32 = 16;

fn pool() -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap()
}

fn bitmap(w: u32, h: u32, px: Vec<u8>) -> Bitmap {
    Bitmap::new(w, h, PixelFormat::Rgba8, px.into()).unwrap()
}

/// The built-in input: an opaque block (with a semi-transparent corner) inside a transparent margin.
fn input() -> Bitmap {
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

fn pixel(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * b.width() + x) * 4) as usize;
    let p = b.pixels();
    [p[i], p[i + 1], p[i + 2], p[i + 3]]
}

/// Assets by path: two small images and a gradient. Anything else is missing.
fn asset(path: &str) -> Option<Bitmap> {
    match path {
        "logo.png" => Some(bitmap(4, 4, (0..16).flat_map(|i| [200, 40, 40, if i % 3 == 0 { 0 } else { 255 }]).collect())),
        "dot.png" => Some(bitmap(3, 3, (0..9).flat_map(|i| [10, 220, 10, (60 + i * 20) as u8]).collect())),
        "bg.png" => Some(bitmap(4, 4, (0..16).flat_map(|i| [(i * 16) as u8, 90, 200 - (i * 8) as u8, 255]).collect())),
        _ => None,
    }
}

/// Glyphs: a 6x8 cell per character, with a diagonal stripe pattern in the spec's colour.
fn glyphs(spec: &TextSpec) -> Option<Bitmap> {
    if spec.text.is_empty() {
        return None;
    }
    let w = 6 * spec.text.chars().count() as u32;
    let h = 8;
    let c = spec.color;
    let px = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            let on = (x + y) % 3 != 0;
            [c.r, c.g, c.b, if on { 255 } else { 0 }]
        })
        .collect();
    Some(bitmap(w, h, px))
}

fn label() -> TextSpec {
    TextSpec { text: "Hi!".into(), ..TextSpec::default() }
}

/// Runs one effect on the pool. The context is built on the pool thread: it holds `&dyn Fn`, which
/// is not `Sync`, so it can't be captured from outside.
fn run(image: &Bitmap, effect: &Effect) -> Bitmap {
    pool().install(|| apply(image, effect, &EffectContext { asset: &asset, text: &glyphs })).unwrap()
}

/// Compares against `tests/snapshots/<name>.png` by pixels, after writing it when it is missing.
fn snapshot(name: &str, actual: &Bitmap) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("snapshots");
    let path = dir.join(format!("{name}.png"));
    let bytes = pool().install(|| laqta_imaging::encode_png(actual)).unwrap();
    let bless = std::env::var("LAQTA_BLESS").is_ok_and(|v| v == "1");
    if bless || !path.exists() {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        return;
    }
    let expected = std::fs::read(&path).unwrap();
    let same = decode(&bytes) == decode(&expected);
    assert!(same, "{name} differs from {} (run with LAQTA_BLESS=1 to accept)", path.display());
}

/// Dimensions and RGBA8 pixels of a PNG.
fn decode(bytes: &[u8]) -> ((u32, u32), Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    ((info.width, info.height), buf[..info.buffer_size()].to_vec())
}

fn draw_cases() -> Vec<(&'static str, Effect)> {
    let d = Effect::Draw;
    vec![
        ("draw_background", d(Draw::Background { color: Rgba::rgb(40, 60, 90), gradient: None })),
        (
            "draw_background_gradient",
            d(Draw::Background { color: Rgba::BLACK, gradient: Some((Rgba::rgb(255, 0, 0), Rgba::rgb(0, 0, 255), 45.0)) }),
        ),
        ("draw_background_image_tile", d(Draw::BackgroundImage { path: "bg.png".into(), center: false, tile: true })),
        ("draw_background_image_center", d(Draw::BackgroundImage { path: "bg.png".into(), center: true, tile: false })),
        ("draw_border_outside", d(Draw::Border { kind: BorderKind::Outside, size: 3, color: Rgba::rgb(200, 0, 0), dash: false })),
        ("draw_border_inside_dash", d(Draw::Border { kind: BorderKind::Inside, size: 2, color: Rgba::rgb(0, 0, 200), dash: true })),
        ("draw_checkerboard", d(Draw::Checkerboard { size: 3, color1: Rgba::WHITE, color2: Rgba::rgb(170, 170, 170) })),
        (
            "draw_image",
            d(Draw::Image {
                path: "logo.png".into(),
                placement: Placement::BottomRight,
                offset: (-1, -1),
                size_mode: SizeMode::Percentage(150.0),
                opacity: 0.75,
            }),
        ),
        ("draw_particles", d(Draw::Particles { path: "dot.png".into(), count: 12, min_size: 2, max_size: 5, random_angle: true, seed: 7 })),
        (
            "draw_text",
            d(Draw::Text {
                text: label(),
                placement: Placement::MiddleCenter,
                offset: (0, 0),
                background: Some(Rgba { r: 0, g: 0, b: 0, a: 160 }),
                padding: 2,
                corner_radius: 3,
            }),
        ),
        (
            "draw_text_ex",
            d(Draw::TextEx {
                text: label(),
                angle: 15.0,
                placement: Placement::TopLeft,
                offset: (1, 1),
                outline: Some((Rgba::BLACK, 1.0)),
                shadow: Some((Rgba { r: 0, g: 0, b: 0, a: 128 }, 1, 2)),
            }),
        ),
    ]
}

fn manip_cases() -> Vec<(&'static str, Effect)> {
    let m = Effect::Manip;
    vec![
        ("manip_auto_crop", m(Manip::AutoCrop { color: Rgba::TRANSPARENT, tolerance: 0, margin: 1 })),
        ("manip_canvas", m(Manip::Canvas { margin: (2, 1, 3, 4), color: Rgba::rgb(0, 255, 0) })),
        ("manip_crop", m(Manip::Crop { margin: (1, 2, 3, 4) })),
        ("manip_flip", m(Manip::Flip { horizontal: true, vertical: false })),
        (
            "manip_force_proportions_grow",
            m(Manip::ForceProportions { width: 1, height: 1, method: ForceMethod::Grow, color: Rgba::rgb(255, 255, 0) }),
        ),
        (
            "manip_force_proportions_crop",
            m(Manip::ForceProportions { width: 1, height: 1, method: ForceMethod::Crop, color: Rgba::TRANSPARENT }),
        ),
        ("manip_resize", m(Manip::Resize { width: 10, height: 0, keep_aspect: true })),
        ("manip_rotate", m(Manip::Rotate { angle: 33.0, grow: true, color: Rgba::rgb(255, 0, 255) })),
        ("manip_rotate_90", m(Manip::Rotate { angle: 90.0, grow: true, color: Rgba::TRANSPARENT })),
        ("manip_rounded_corners", m(Manip::RoundedCorners { radius: 5 })),
        ("manip_scale", m(Manip::Scale { width_pct: 50.0, height_pct: 150.0 })),
        ("manip_skew", m(Manip::Skew { x: 20, y: 0, grow: true })),
    ]
}

#[test]
fn every_drawing_effect_matches_its_snapshot() {
    assert_eq!(draw_cases().len(), 11);
    for (name, effect) in draw_cases() {
        snapshot(name, &run(&input(), &effect));
    }
}

#[test]
fn every_manipulation_effect_matches_its_snapshot() {
    assert_eq!(manip_cases().len(), 12);
    for (name, effect) in manip_cases() {
        snapshot(name, &run(&input(), &effect));
    }
}

#[test]
fn missing_assets_and_text_are_errors() {
    let image = input();
    let missing_assets = [
        Draw::Image {
            path: "nope.png".into(),
            placement: Placement::MiddleCenter,
            offset: (0, 0),
            size_mode: SizeMode::DontResize,
            opacity: 1.0,
        },
        Draw::BackgroundImage { path: "nope.png".into(), center: false, tile: false },
        Draw::Particles { path: "nope.png".into(), count: 3, min_size: 2, max_size: 4, random_angle: false, seed: 1 },
    ];
    for effect in missing_assets {
        let err = pool().install(|| apply(&image, &Effect::Draw(effect), &EffectContext { asset: &asset, text: &glyphs })).unwrap_err();
        assert!(matches!(&err, EffectError::MissingAsset(p) if p == "nope.png"), "{err:?}");
    }
    let text = Effect::Draw(Draw::Text {
        text: label(),
        placement: Placement::MiddleCenter,
        offset: (0, 0),
        background: None,
        padding: 0,
        corner_radius: 0,
    });
    let err = pool().install(|| apply(&image, &text, &EffectContext::none())).unwrap_err();
    assert!(matches!(err, EffectError::NoText), "{err:?}");
}

#[test]
fn background_fills_only_transparent_pixels() {
    let out = run(&input(), &Effect::Draw(Draw::Background { color: Rgba::rgb(10, 20, 30), gradient: None }));
    assert_eq!((out.width(), out.height()), (W, H));
    assert_eq!(pixel(&out, 0, 0), [10, 20, 30, 255]);
    assert_eq!(pixel(&out, 3, 2), [21, 16, 14, 255]);
    assert_eq!(pixel(&out, 10, 7)[3], 255);
}

#[test]
fn border_outside_grows_the_canvas_by_its_size() {
    let effect = Effect::Draw(Draw::Border { kind: BorderKind::Outside, size: 3, color: Rgba::rgb(200, 0, 0), dash: false });
    let out = run(&input(), &effect);
    assert_eq!((out.width(), out.height()), (W + 6, H + 6));
    assert_eq!(pixel(&out, 0, 0), [200, 0, 0, 255]);
    assert_eq!(pixel(&out, W + 5, H + 5), [200, 0, 0, 255]);
    assert_eq!(pixel(&out, 6, 5), pixel(&input(), 3, 2));
}

#[test]
fn flip_moves_pixels_and_twice_is_identity() {
    let flip_h = Effect::Manip(Manip::Flip { horizontal: true, vertical: false });
    let once = run(&input(), &flip_h);
    assert_eq!(pixel(&once, W - 1 - 3, 2), pixel(&input(), 3, 2));
    assert_eq!(run(&once, &flip_h).pixels(), input().pixels());
    let both = Effect::Manip(Manip::Flip { horizontal: true, vertical: true });
    assert_eq!(run(&run(&input(), &both), &both).pixels(), input().pixels());
}

#[test]
fn rotate_90_four_times_is_identity() {
    let rot = Effect::Manip(Manip::Rotate { angle: 90.0, grow: true, color: Rgba::TRANSPARENT });
    let one = run(&input(), &rot);
    assert_eq!((one.width(), one.height()), (H, W));
    // Clockwise: the source's top-left pixel (2, 1) ends up at (H - 1 - 1, 2).
    assert_eq!(pixel(&one, H - 2, 2), pixel(&input(), 2, 1));
    let mut img = input();
    for _ in 0..4 {
        img = run(&img, &rot);
    }
    assert_eq!((img.width(), img.height()), (W, H));
    assert_eq!(img.pixels(), input().pixels());
}

#[test]
fn quarter_turn_without_grow_copies_source_pixels_exactly() {
    // An odd width difference (21 vs 16) puts every sample on a pixel edge. Each output pixel must be
    // a source pixel as it is, or transparent where the source doesn't reach.
    let mut px: Vec<u8> = Vec::new();
    for y in 0..16u32 {
        for x in 0..21u32 {
            px.extend_from_slice(&[x as u8, y as u8, 7, 255]);
        }
    }
    let source = bitmap(21, 16, px);
    let rot = Effect::Manip(Manip::Rotate { angle: 90.0, grow: false, color: Rgba::TRANSPARENT });
    let out = run(&source, &rot);
    assert_eq!((out.width(), out.height()), (21, 16));
    let sources: Vec<[u8; 4]> = source.pixels().as_chunks::<4>().0.to_vec();
    let mut copied = 0;
    for p in out.pixels().as_chunks::<4>().0 {
        if *p == [0, 0, 0, 0] {
            continue;
        }
        assert!(sources.contains(p), "{p:?} is not a source pixel");
        copied += 1;
    }
    assert!(copied > 200, "only {copied} pixels came from the source");
}

#[test]
fn canvas_margins_change_the_size_exactly() {
    let grow = run(&input(), &Effect::Manip(Manip::Canvas { margin: (2, 1, 3, 4), color: Rgba::rgb(0, 255, 0) }));
    assert_eq!((grow.width(), grow.height()), (W + 5, H + 5));
    assert_eq!(pixel(&grow, 0, 0), [0, 255, 0, 255]);
    assert_eq!(pixel(&grow, 5, 3), pixel(&input(), 3, 2));

    let trim = run(&input(), &Effect::Manip(Manip::Canvas { margin: (-2, 0, -2, 0), color: Rgba::BLACK }));
    assert_eq!((trim.width(), trim.height()), (W - 4, H));
    assert_eq!(pixel(&trim, 1, 2), pixel(&input(), 3, 2));
}

#[test]
fn auto_crop_removes_a_uniform_border() {
    // 10x8: a red frame of one pixel around a 6x4 blue block.
    let mut px: Vec<u8> = Vec::new();
    for y in 0..8u32 {
        for x in 0..10u32 {
            let inner = (1..7).contains(&x) && (2..6).contains(&y);
            px.extend_from_slice(&if inner { [0, 0, 255, 255] } else { [255, 0, 0, 255] });
        }
    }
    let framed = bitmap(10, 8, px);
    let auto = Effect::Manip(Manip::AutoCrop { color: Rgba::TRANSPARENT, tolerance: 0, margin: 0 });
    let out = run(&framed, &auto);
    assert_eq!((out.width(), out.height()), (6, 4));
    assert!(out.pixels().as_chunks::<4>().0.iter().all(|p| *p == [0, 0, 255, 255]));

    let with_margin = Effect::Manip(Manip::AutoCrop { color: Rgba::TRANSPARENT, tolerance: 0, margin: 1 });
    let out = run(&framed, &with_margin);
    assert_eq!((out.width(), out.height()), (8, 6));

    // The input's transparent margin is the background: 12x10 of content, 14x12 with a 1px margin.
    let content = Effect::Manip(Manip::AutoCrop { color: Rgba::TRANSPARENT, tolerance: 0, margin: 0 });
    let out = run(&input(), &content);
    assert_eq!((out.width(), out.height()), (12, 10));
    let out = run(&input(), &Effect::Manip(Manip::AutoCrop { color: Rgba::TRANSPARENT, tolerance: 0, margin: 1 }));
    assert_eq!((out.width(), out.height()), (14, 12));
}

#[test]
fn resize_keep_aspect_math() {
    let dims = |width: u32, height: u32, keep_aspect: bool| {
        let out = run(&input(), &Effect::Manip(Manip::Resize { width, height, keep_aspect }));
        (out.width(), out.height())
    };
    assert_eq!(dims(10, 0, true), (10, 8));
    assert_eq!(dims(0, 4, true), (5, 4));
    assert_eq!(dims(10, 10, true), (10, 8));
    assert_eq!(dims(40, 10, true), (13, 10));
    assert_eq!(dims(10, 10, false), (10, 10));
    assert_eq!(dims(0, 0, true), (W, H));
}

#[test]
fn rounded_corners_are_transparent_and_centre_stays_opaque() {
    let solid = bitmap(W, H, [30, 60, 90, 255].repeat((W * H) as usize));
    let out = run(&solid, &Effect::Manip(Manip::RoundedCorners { radius: 5 }));
    assert_eq!(pixel(&out, 0, 0)[3], 0);
    assert_eq!(pixel(&out, 10, 8), [30, 60, 90, 255]);
}

#[test]
fn scale_and_skew_dimensions() {
    let scaled = run(&input(), &Effect::Manip(Manip::Scale { width_pct: 50.0, height_pct: 150.0 }));
    assert_eq!((scaled.width(), scaled.height()), (10, 24));
    let grown = run(&input(), &Effect::Manip(Manip::Skew { x: 20, y: 0, grow: true }));
    assert_eq!((grown.width(), grown.height()), (26, H));
    let kept = run(&input(), &Effect::Manip(Manip::Skew { x: 20, y: 0, grow: false }));
    assert_eq!((kept.width(), kept.height()), (W, H));
    let cancel = Effect::Manip(Manip::Skew { x: 45, y: 45, grow: true });
    let err = pool().install(|| apply(&input(), &cancel, &EffectContext::none())).unwrap_err();
    assert!(matches!(err, EffectError::Invalid(_)), "{err:?}");
}

#[test]
fn particles_depend_only_on_the_seed() {
    let layout = |seed: u64| {
        run(
            &input(),
            &Effect::Draw(Draw::Particles { path: "dot.png".into(), count: 12, min_size: 2, max_size: 5, random_angle: true, seed }),
        )
    };
    assert_eq!(layout(9).pixels(), layout(9).pixels());
    assert_ne!(layout(9).pixels(), layout(10).pixels());
}

#[test]
fn bgra_input_gives_the_same_result() {
    let mut swapped = input().pixels().to_vec();
    for p in swapped.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    let bgra = Bitmap::new(W, H, PixelFormat::Bgra8, swapped.into()).unwrap();
    let flip = Effect::Manip(Manip::Flip { horizontal: true, vertical: false });
    assert_eq!(run(&bgra, &flip).pixels(), run(&input(), &flip).pixels());
}
