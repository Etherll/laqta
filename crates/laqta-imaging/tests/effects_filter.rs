#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]
//! Filter kernels (M3 D8): one PNG snapshot per filter on a 24x24 test card, plus property checks.
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

/// A blank image of the given size, all pixels `[r, g, b, a]`.
fn solid(w: u32, h: u32, p: [u8; 4]) -> Bitmap {
    let px: Vec<u8> = std::iter::repeat_n(p, (w * h) as usize).flatten().collect();
    Bitmap::new(w, h, PixelFormat::Rgba8, px.into()).unwrap()
}

fn px(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * b.width() + x) * 4) as usize;
    b.pixels()[i..i + 4].try_into().unwrap()
}

fn assert_same(out: &Bitmap, expected: &Bitmap, what: &str) {
    assert_eq!((out.width(), out.height()), (expected.width(), expected.height()), "{what}: size");
    assert!(out.pixels() == expected.pixels(), "{what}: pixels");
}

/// Every filter the snapshot suite covers, with parameters sized for the 24x24 card. The file name is
/// `filter_<name>.png`.
fn cases() -> Vec<(&'static str, Filter)> {
    vec![
        ("blur", Filter::Blur { radius: 2 }),
        ("color_depth", Filter::ColorDepth { bits: 2 }),
        (
            "convolution_matrix",
            Filter::ConvolutionMatrix { kernel: [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0], divisor: 1.0, offset: 128.0 },
        ),
        ("edge_detect", Filter::EdgeDetect),
        ("emboss", Filter::Emboss),
        ("gaussian_blur", Filter::GaussianBlur { sigma: 1.5 }),
        ("glow", Filter::Glow { size: 3, strength: 1.5, color: Rgba::rgb(255, 230, 0), offset_x: 0, offset_y: 0 }),
        ("mean_removal", Filter::MeanRemoval),
        ("outline", Filter::Outline { size: 2, padding: 2, color: Rgba::rgb(255, 0, 0), outline_only: false }),
        ("pixelate", Filter::Pixelate { size: 4 }),
        ("reflection", Filter::Reflection { percentage: 40, max_alpha: 200, min_alpha: 0, offset: 2, skew: true, skew_size: 4 }),
        ("rgb_split", Filter::RgbSplit { offset_red: (-2, 0), offset_green: (0, 1), offset_blue: (2, 0) }),
        (
            "shadow",
            Filter::Shadow { opacity: 0.8, size: 2, darkness: 0.0, color: Rgba::BLACK, offset_x: 3, offset_y: 3, auto_resize: true },
        ),
        ("sharpen", Filter::Sharpen),
        ("slice", Filter::Slice { min_height: 2, max_height: 5, min_shift: 2, max_shift: 5, seed: 7 }),
        ("smooth", Filter::Smooth),
        ("torn_edge", Filter::TornEdge { depth: 5, range: 4, top: true, right: true, bottom: true, left: true, curved: true, seed: 3 }),
        ("wave_edge", Filter::WaveEdge { depth: 4, range: 8, top: true, right: false, bottom: true, left: false }),
        ("noise", Filter::Noise { amount: 40, seed: 11 }),
        ("posterize", Filter::Posterize { levels: 3 }),
        ("vignette", Filter::Vignette { strength: 0.8, radius: 0.3, color: Rgba::BLACK }),
        ("halftone", Filter::Halftone { cell: 4 }),
        ("dither", Filter::Dither { levels: 2 }),
        ("motion_blur", Filter::MotionBlur { distance: 5, angle: 30.0 }),
        ("zoom_blur", Filter::ZoomBlur { amount: 0.4, center_x: 0.5, center_y: 0.5 }),
        ("median", Filter::Median { radius: 1 }),
        ("oil_paint", Filter::OilPaint { radius: 2, levels: 8 }),
    ]
}

fn slug(name: &str) -> String {
    name.to_lowercase().replace(' ', "_")
}

fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots").join(format!("filter_{name}.png"))
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
fn every_filter_matches_its_snapshot() {
    let input = pattern();
    let cases = cases();
    assert_eq!(cases.len(), 27);
    for (name, filter) in &cases {
        let out = run(filter, &input);
        let changed = out.width() != input.width() || out.height() != input.height() || out.pixels() != input.pixels();
        assert!(changed, "{name} left the test card unchanged");
        snapshot(name, &out);
    }
}

#[test]
fn snapshot_cases_cover_every_default() {
    let names: Vec<String> = cases().iter().map(|(name, _)| (*name).to_string()).collect();
    // The 27 M3 filters; each M7 batch (X2, X3, X4) checks its own in `effects_filter_x*.rs`.
    for filter in Filter::defaults().into_iter().take(27) {
        assert!(names.contains(&slug(filter.name())), "no snapshot case for {}", filter.name());
    }
}

#[test]
fn pixelate_size_one_is_identity() {
    let input = pattern();
    assert_same(&run(&Filter::Pixelate { size: 1 }, &input), &input, "pixelate 1");
}

#[test]
fn blur_radius_zero_is_identity() {
    let input = pattern();
    assert_same(&run(&Filter::Blur { radius: 0 }, &input), &input, "blur 0");
    assert_same(&run(&Filter::GaussianBlur { sigma: 0.0 }, &input), &input, "gaussian 0");
    assert_same(&run(&Filter::MotionBlur { distance: 0, angle: 45.0 }, &input), &input, "motion 0");
}

#[test]
fn identity_kernel_is_identity() {
    let input = pattern();
    let identity = Filter::ConvolutionMatrix { kernel: [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0], divisor: 1.0, offset: 0.0 };
    assert_same(&run(&identity, &input), &input, "identity kernel");
}

#[test]
fn shadow_grows_the_canvas_as_specified() {
    let input = pattern();
    // The shadow reaches 3 * size = 6 past the image, shifted by the offset. With offset (3, 3) the
    // canvas spans image coordinates -3..33, so it is 36 square and the image sits at (3, 3).
    let shadow = |auto_resize, offset_x, offset_y| Filter::Shadow {
        opacity: 0.8,
        size: 2,
        darkness: 0.0,
        color: Rgba::BLACK,
        offset_x,
        offset_y,
        auto_resize,
    };
    let grown = run(&shadow(true, 3, 3), &input);
    assert_eq!((grown.width(), grown.height()), (36, 36));
    let kept = run(&shadow(false, 3, 3), &input);
    assert_eq!((kept.width(), kept.height()), (N, N));
    // Negative offsets reach further on the other side: lo = -9, so the image sits at (9, 9).
    let negative = run(&shadow(true, -3, -3), &input);
    assert_eq!((negative.width(), negative.height()), (36, 36));

    // Opaque image pixels land unchanged at the origin, with the shadow underneath.
    for y in 0..N {
        for x in 0..N {
            if input.pixels()[((y * N + x) * 4 + 3) as usize] == 255 {
                assert_eq!(px(&grown, x + 3, y + 3), px(&input, x, y), "pixel {x},{y}");
            }
        }
    }
    // Shadow shows past the image's bottom right: canvas (28, 28) is outside the image at (3, 3)..(27, 27).
    assert!(px(&grown, 28, 28)[3] > 0, "no shadow past the bottom right");
}

#[test]
fn reflection_grows_the_canvas_as_specified() {
    let input = pattern();
    // 50% of 24 rows is 12, and the offset adds 2 transparent rows: 24 + 2 + 12 = 38.
    let plain = Filter::Reflection { percentage: 50, max_alpha: 255, min_alpha: 0, offset: 2, skew: false, skew_size: 0 };
    let out = run(&plain, &input);
    assert_eq!((out.width(), out.height()), (N, 38));
    for y in 0..N {
        for x in 0..N {
            assert_eq!(px(&out, x, y), px(&input, x, y), "original row {y}");
        }
    }
    for y in [N, N + 1] {
        for x in 0..N {
            assert_eq!(px(&out, x, y)[3], 0, "gap row {y}");
        }
    }
    // The first reflected row mirrors the image's bottom row, and at full max_alpha it keeps alpha.
    for x in 0..N {
        let mirrored = px(&input, x, N - 1);
        assert_eq!(px(&out, x, N + 2)[..3], mirrored[..3], "mirror colour at {x}");
        assert_eq!(px(&out, x, N + 2)[3], mirrored[3], "mirror alpha at {x}");
    }

    // Skew widens the canvas by skew_size.
    let skewed = Filter::Reflection { percentage: 50, max_alpha: 255, min_alpha: 0, offset: 2, skew: true, skew_size: 4 };
    let out = run(&skewed, &input);
    assert_eq!((out.width(), out.height()), (28, 38));
}

#[test]
fn posterize_and_colour_depth_keep_only_their_levels() {
    let input = pattern();
    let out = run(&Filter::Posterize { levels: 3 }, &input);
    let allowed = [0u8, 127, 255];
    for pixel in out.pixels().as_chunks::<4>().0 {
        for &c in &pixel[..3] {
            assert!(allowed.contains(&c), "posterize produced {c}");
        }
    }
    let out = run(&Filter::ColorDepth { bits: 1 }, &input);
    for pixel in out.pixels().as_chunks::<4>().0 {
        for &c in &pixel[..3] {
            assert!(c == 0 || c == 255, "colour depth 1 produced {c}");
        }
    }
    // Alpha survives per-pixel filters.
    assert_eq!(px(&out, 0, 0)[3], 0);
}

#[test]
fn seeded_filters_repeat_and_depend_on_the_seed() {
    let input = pattern();
    let seeded = |seed: u64| {
        vec![
            Filter::Noise { amount: 40, seed },
            Filter::Slice { min_height: 2, max_height: 5, min_shift: 2, max_shift: 5, seed },
            Filter::TornEdge { depth: 5, range: 4, top: true, right: true, bottom: true, left: true, curved: false, seed },
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
fn output_does_not_depend_on_thread_count() {
    let input = pattern();
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let many = rayon::ThreadPoolBuilder::new().num_threads(5).build().unwrap();
    for filter in Filter::defaults() {
        let effect = Effect::Filter(filter.clone());
        let a = one.install(|| apply(&input, &effect, &EffectContext::none()).unwrap());
        let b = many.install(|| apply(&input, &effect, &EffectContext::none()).unwrap());
        assert!(a.pixels() == b.pixels(), "{} depends on threads", filter.name());
    }
}

#[test]
fn defaults_run_on_one_pixel_and_on_nothing() {
    let one = solid(1, 1, [200, 100, 50, 255]);
    let empty = solid(0, 0, [0, 0, 0, 0]);
    for filter in Filter::defaults() {
        let out = run(&filter, &one);
        assert!(out.width() >= 1 && out.height() >= 1, "{}", filter.name());
        let out = run(&filter, &empty);
        assert_eq!((out.width(), out.height()), (0, 0), "{}", filter.name());
    }
}

#[test]
fn degenerate_parameters_render_without_panicking() {
    let input = pattern();
    let odd = [
        Filter::Blur { radius: u32::MAX },
        Filter::ColorDepth { bits: 0 },
        Filter::ColorDepth { bits: 200 },
        Filter::GaussianBlur { sigma: f32::NAN },
        Filter::GaussianBlur { sigma: 1e9 },
        Filter::ConvolutionMatrix { kernel: [1.0; 9], divisor: 0.0, offset: f32::NAN },
        Filter::Pixelate { size: 0 },
        Filter::Halftone { cell: 0 },
        Filter::Posterize { levels: 0 },
        Filter::Dither { levels: 255 },
        Filter::Noise { amount: 0, seed: 0 },
        Filter::Slice { min_height: 0, max_height: 0, min_shift: 9, max_shift: 3, seed: 0 },
        Filter::TornEdge { depth: u32::MAX, range: 0, top: true, right: true, bottom: true, left: true, curved: true, seed: 0 },
        Filter::WaveEdge { depth: 3, range: 0, top: true, right: true, bottom: true, left: true },
        Filter::Vignette { strength: 9.0, radius: 5.0, color: Rgba::WHITE },
        Filter::MotionBlur { distance: u32::MAX, angle: f32::INFINITY },
        Filter::ZoomBlur { amount: f32::NAN, center_x: f32::NAN, center_y: 3.0 },
        Filter::Median { radius: u32::MAX },
        Filter::OilPaint { radius: 1, levels: 0 },
        Filter::RgbSplit { offset_red: (i32::MAX, i32::MIN), offset_green: (0, 0), offset_blue: (-40, 40) },
        Filter::Glow { size: 1, strength: f32::INFINITY, color: Rgba::WHITE, offset_x: -40, offset_y: 0 },
    ];
    for filter in &odd {
        let out = try_run(filter, &input).unwrap_or_else(|e| panic!("{filter:?}: {e}"));
        assert!(out.width() >= 1 && out.height() >= 1, "{filter:?}");
    }
}

#[test]
fn canvas_growth_past_the_budget_is_refused() {
    let input = pattern();
    let huge = [
        Filter::Glow { size: u32::MAX, strength: 1.0, color: Rgba::WHITE, offset_x: 0, offset_y: 0 },
        Filter::Outline { size: u32::MAX, padding: 0, color: Rgba::WHITE, outline_only: false },
        Filter::Reflection { percentage: 100, max_alpha: 255, min_alpha: 0, offset: u32::MAX, skew: false, skew_size: 0 },
        Filter::Shadow { opacity: 1.0, size: u32::MAX, darkness: 0.0, color: Rgba::BLACK, offset_x: 0, offset_y: 0, auto_resize: false },
    ];
    for filter in &huge {
        assert!(matches!(try_run(filter, &input), Err(EffectError::Invalid(_))), "{filter:?}");
    }
}
