//! Snapshot and value tests for the adjustment kernels (M3 D8). Each variant runs on a fixed 16x16
//! pattern inside a compute pool and must match `tests/snapshots/adjust_<name>.png` exactly, by
//! decoded pixels. A missing snapshot is written and passes; `LAQTA_BLESS=1` rewrites them all.
#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use laqta_domain::color::Rgba;
use laqta_domain::effects::{Adjust, Effect};
use laqta_domain::{Bitmap, PixelFormat};
use laqta_imaging::effects::{self, EffectContext};

const SIZE: u32 = 16;

const IDENTITY_MATRIX: [f32; 20] = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0, //
    0.0, 0.0, 0.0, 0.0,
];

/// Input r goes to output b and input b to output r.
const CHANNEL_SWAP: [f32; 20] = [
    0.0, 0.0, 1.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    1.0, 0.0, 0.0, 0.0, //
    0.0, 0.0, 0.0, 1.0, //
    0.0, 0.0, 0.0, 0.0,
];

fn pool() -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap()
}

/// Rows 0..4: a gradient. Rows 4..8: primaries and secondaries, with alpha falling off over the last
/// four columns. Rows 8..12: a grey ramp. Rows 12..16: a diagonal pattern.
fn pattern() -> Bitmap {
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
    Bitmap::new(SIZE, SIZE, PixelFormat::Rgba8, px.into()).unwrap()
}

/// One snapshot per variant, with parameters that move the output away from the identity.
fn cases() -> Vec<(&'static str, Adjust)> {
    vec![
        ("alpha", Adjust::Alpha { alpha: 0.5, add: 0.1 }),
        ("black_white", Adjust::BlackWhite),
        ("brightness", Adjust::Brightness { value: 25.0 }),
        ("colorize", Adjust::Colorize { color: Rgba::rgb(0, 128, 255), value: 60.0 }),
        ("contrast", Adjust::Contrast { value: -30.0 }),
        ("gamma", Adjust::Gamma { value: 2.2 }),
        ("grayscale", Adjust::Grayscale { value: 70.0 }),
        ("hue", Adjust::Hue { angle: 120.0 }),
        ("inverse", Adjust::Inverse),
        ("matrix_color", Adjust::MatrixColor { matrix: CHANNEL_SWAP }),
        ("polaroid", Adjust::Polaroid),
        ("saturation", Adjust::Saturation { value: -50.0 }),
        ("selective_color", Adjust::SelectiveColor { from: Rgba::rgb(255, 0, 0), to: Rgba::rgb(0, 255, 0), tolerance: 60 }),
        ("sepia", Adjust::Sepia { value: 80.0 }),
        (
            "replace_color",
            Adjust::ReplaceColor {
                source: Rgba::rgb(0, 0, 255),
                target: Rgba::rgb(255, 128, 0),
                tolerance: 40,
                auto_source_from_corner: false,
            },
        ),
        (
            "replace_color_corner",
            Adjust::ReplaceColor { source: Rgba::BLACK, target: Rgba::rgb(255, 128, 0), tolerance: 40, auto_source_from_corner: true },
        ),
    ]
}

fn run(pool: &rayon::ThreadPool, input: &Bitmap, adj: Adjust) -> Bitmap {
    pool.install(|| effects::apply(input, &Effect::Adjust(adj), &EffectContext::none())).unwrap()
}

fn decode(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("no output buffer size")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!("expected 8-bit RGBA, found {:?} {:?}", info.color_type, info.bit_depth));
    }
    buf.truncate(info.buffer_size());
    Ok((info.width, info.height, buf))
}

/// Compares `out` with the snapshot at `path`. Writes the snapshot when it is missing or when
/// `bless` is set.
fn check_snapshot(pool: &rayon::ThreadPool, path: &Path, out: &Bitmap, bless: bool) -> Result<(), String> {
    let shown = path.display();
    if path.exists() && !bless {
        let bytes = std::fs::read(path).map_err(|e| format!("{shown}: {e}"))?;
        let (w, h, px) = decode(&bytes).map_err(|e| format!("{shown}: {e}"))?;
        if (w, h) != (out.width(), out.height()) || px.as_slice() != out.pixels() {
            return Err(format!("{shown} differs from the output; rerun with LAQTA_BLESS=1 if the change is intended"));
        }
        return Ok(());
    }
    let bytes = pool.install(|| laqta_imaging::encode_png(out)).map_err(|e| format!("{shown}: {e:?}"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{shown}: {e}"))?;
    eprintln!("wrote snapshot {shown}");
    Ok(())
}

#[test]
fn snapshots_match_committed_pngs() {
    let pool = pool();
    let input = pattern();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    let bless = std::env::var_os("LAQTA_BLESS").is_some_and(|v| v == "1");
    let mut failures = Vec::new();
    for (name, adj) in cases() {
        let out = run(&pool, &input, adj);
        let path = dir.join(format!("adjust_{name}.png"));
        if let Err(msg) = check_snapshot(&pool, &path, &out, bless) {
            failures.push(msg);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_variant_has_a_snapshot_case() {
    let cases = cases();
    // The 15 M3 adjustments; each M7 batch checks its own in `effects_x1.rs`.
    for variant in Adjust::defaults().into_iter().take(15) {
        let covered = cases.iter().any(|(_, c)| std::mem::discriminant(c) == std::mem::discriminant(&variant));
        assert!(covered, "no snapshot case for {}", variant.name());
    }
}

#[test]
fn inverse_twice_is_identity() {
    let pool = pool();
    let input = pattern();
    let once = run(&pool, &input, Adjust::Inverse);
    let twice = run(&pool, &once, Adjust::Inverse);
    assert_eq!(twice.pixels(), input.pixels());
}

#[test]
fn zero_brightness_is_identity() {
    let pool = pool();
    let input = pattern();
    assert_eq!(run(&pool, &input, Adjust::Brightness { value: 0.0 }).pixels(), input.pixels());
}

#[test]
fn identity_matrix_is_identity() {
    let pool = pool();
    let input = pattern();
    assert_eq!(run(&pool, &input, Adjust::MatrixColor { matrix: IDENTITY_MATRIX }).pixels(), input.pixels());
}

#[test]
fn unit_gamma_and_zero_contrast_are_identity() {
    let pool = pool();
    let input = pattern();
    assert_eq!(run(&pool, &input, Adjust::Gamma { value: 1.0 }).pixels(), input.pixels());
    assert_eq!(run(&pool, &input, Adjust::Contrast { value: 0.0 }).pixels(), input.pixels());
}

#[test]
fn hue_full_turn_is_identity_within_one_level() {
    let pool = pool();
    let input = pattern();
    let out = run(&pool, &input, Adjust::Hue { angle: 360.0 });
    assert!(out.pixels().iter().zip(input.pixels()).all(|(a, b)| a.abs_diff(*b) <= 1));
}

#[test]
fn full_grayscale_has_equal_channels() {
    let pool = pool();
    let out = run(&pool, &pattern(), Adjust::Grayscale { value: 100.0 });
    for p in out.pixels().as_chunks::<4>().0 {
        assert!(p[0] == p[1] && p[1] == p[2], "{p:?}");
    }
}

#[test]
fn colour_effects_keep_alpha() {
    let pool = pool();
    let input = pattern();
    let out = run(&pool, &input, Adjust::Saturation { value: -100.0 });
    for (a, b) in out.pixels().as_chunks::<4>().0.iter().zip(input.pixels().as_chunks::<4>().0) {
        assert_eq!(a[3], b[3]);
    }
}
