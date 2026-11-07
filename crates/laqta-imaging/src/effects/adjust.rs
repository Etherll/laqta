//! Adjustment kernels for `laqta_domain::effects::Adjust`. Colour effects are 5x4 matrices on
//! straight-alpha RGBA in 0..1 units; gamma is a 256-entry table; the replace effects compare
//! per-pixel RGB distance. Every pass is a row loop on the compute pool.

use laqta_domain::color::Rgba;
use laqta_domain::effects::Adjust;
use laqta_domain::{Bitmap, PixelFormat};
use rayon::prelude::*;

use super::adjust_m7;
use super::{EffectContext, EffectError};

/// Luma weights (the SVG `feColorMatrix` values, which the hue rotation also uses).
pub(super) const LUMA: [f32; 3] = [0.213, 0.715, 0.072];
/// Rows of the grey target: every output channel takes the same luma weights.
const GRAY: [[f32; 3]; 3] = [[LUMA[0]; 3], [LUMA[1]; 3], [LUMA[2]; 3]];
/// Sepia target, indexed [input][output].
const SEPIA: [[f32; 3]; 3] = [[0.393, 0.349, 0.272], [0.769, 0.686, 0.534], [0.189, 0.168, 0.131]];
/// Polaroid matrix, indexed [input][output] in the 5x4 layout (the last row is the offset).
const POLAROID: Matrix = [
    [1.438, -0.122, -0.016, 0.0],
    [-0.062, 1.378, -0.016, 0.0],
    [-0.062, -0.122, 1.483, 0.0],
    [0.0, 0.0, 0.0, 1.0],
    [0.03, 0.05, -0.02, 0.0],
];
/// Black & White pushes luma through this slope around mid grey, so it saturates to near black or
/// near white.
const BLACK_WHITE_SLOPE: f32 = 4.0;

/// The 5x4 matrix as rows: input r, g, b, a, then the constant; columns are the outputs.
type Matrix = [[f32; 4]; 5];

pub(super) fn apply(image: &Bitmap, effect: &Adjust, _cx: &EffectContext<'_>) -> Result<Bitmap, EffectError> {
    debug_assert!(rayon::current_thread_index().is_some(), "adjust must run inside the compute pool");
    debug_assert_eq!(image.format(), PixelFormat::Rgba8);
    if image.width() == 0 || image.height() == 0 {
        return Ok(image.clone());
    }
    let out = match effect {
        Adjust::Alpha { alpha, add } => apply_matrix(image, &flat(&alpha_matrix(*alpha, *add))),
        Adjust::BlackWhite => apply_matrix(image, &flat(&black_white())),
        Adjust::Brightness { value } => apply_matrix(image, &flat(&rgb_matrix(diag(1.0), [value / 100.0; 3]))),
        Adjust::Colorize { color, value } => apply_matrix(image, &flat(&blend(&colorize_target(*color), value / 100.0))),
        Adjust::Contrast { value } => {
            let c = (100.0 + value) / 100.0;
            apply_matrix(image, &flat(&rgb_matrix(diag(c), [0.5 * (1.0 - c); 3])))
        }
        Adjust::Gamma { value } => gamma(image, *value),
        Adjust::Grayscale { value } => apply_matrix(image, &flat(&blend(&GRAY, value / 100.0))),
        Adjust::Hue { angle } => apply_matrix(image, &flat(&hue(*angle))),
        Adjust::Inverse => apply_matrix(image, &flat(&rgb_matrix(diag(-1.0), [1.0; 3]))),
        Adjust::MatrixColor { matrix } => apply_matrix(image, matrix),
        Adjust::Polaroid => apply_matrix(image, &flat(&POLAROID)),
        // Saturation is the grey blend run backwards: t = -value / 100 gives 1 - s.
        Adjust::Saturation { value } => apply_matrix(image, &flat(&blend(&GRAY, -value / 100.0))),
        Adjust::SelectiveColor { from, to, tolerance } => selective(image, *from, *to, *tolerance),
        Adjust::Sepia { value } => apply_matrix(image, &flat(&blend(&SEPIA, value / 100.0))),
        Adjust::ReplaceColor { source, target, tolerance, auto_source_from_corner } => {
            replace(image, *source, *target, *tolerance, *auto_source_from_corner)
        }
        Adjust::AutoContrast { clip } => adjust_m7::auto_contrast(image, *clip),
        Adjust::ChannelSwap { red, green, blue } => adjust_m7::channel_swap(image, *red, *green, *blue),
        Adjust::Clarity { strength, radius } => adjust_m7::clarity(image, *strength, *radius),
        Adjust::ColorBalance { shadows, midtones, highlights } => adjust_m7::color_balance(image, *shadows, *midtones, *highlights),
        Adjust::CrossProcess { strength } => adjust_m7::cross_process(image, *strength),
        Adjust::Dehaze { strength, radius } => adjust_m7::dehaze(image, *strength, *radius),
        Adjust::DuotoneGradientMap { shadow, highlight, strength } => adjust_m7::duotone(image, *shadow, *highlight, *strength),
        Adjust::Exposure { stops } => adjust_m7::exposure(image, *stops),
        Adjust::FilmEmulation { strength, grain, seed } => adjust_m7::film(image, *strength, *grain, *seed),
        Adjust::Levels { in_black, in_white, gamma, out_black, out_white } => {
            adjust_m7::levels(image, *in_black, *in_white, *gamma, *out_black, *out_white)
        }
        Adjust::Lomo { strength } => adjust_m7::lomo(image, *strength),
        Adjust::ShadowsHighlights { shadows, highlights, radius } => adjust_m7::shadows_highlights(image, *shadows, *highlights, *radius),
        Adjust::Solarize { threshold } => adjust_m7::solarize(image, *threshold),
        Adjust::SplitToning { shadow, highlight, strength, balance } => {
            adjust_m7::split_toning(image, *shadow, *highlight, *strength, *balance)
        }
        Adjust::TemperatureTint { temperature, tint } => adjust_m7::temperature_tint(image, *temperature, *tint),
        Adjust::Threshold { level } => adjust_m7::threshold(image, *level),
        Adjust::Vibrance { value } => adjust_m7::vibrance(image, *value),
    };
    Ok(out)
}

/// Applies a row-major 5x4 colour matrix (see `Adjust::MatrixColor`) to every pixel. Alpha goes
/// through the same matrix as the colour channels.
fn apply_matrix(image: &Bitmap, m: &[f32; 20]) -> Bitmap {
    map_pixels(image, |p| {
        let x = p.map(|c| f32::from(c) / 255.0);
        std::array::from_fn(|j| {
            let v = x[0] * m[j] + x[1] * m[4 + j] + x[2] * m[8 + j] + x[3] * m[12 + j] + m[16 + j];
            to_u8(v)
        })
    })
}

/// Gamma through a 256-entry table per colour channel. Alpha is kept.
fn gamma(image: &Bitmap, value: f32) -> Bitmap {
    let inv = 1.0 / value.clamp(0.1, 5.0);
    let lut: [u8; 256] = std::array::from_fn(|i| ((i as f32 / 255.0).powf(inv) * 255.0).round() as u8);
    let ch = |c: u8| lut[usize::from(c)];
    map_pixels(image, |p| [ch(p[0]), ch(p[1]), ch(p[2]), p[3]])
}

/// Pixels within `tolerance` of `from` are shifted by `to - from`. Alpha is kept.
fn selective(image: &Bitmap, from: Rgba, to: Rgba, tolerance: u8) -> Bitmap {
    let shift = [i32::from(to.r) - i32::from(from.r), i32::from(to.g) - i32::from(from.g), i32::from(to.b) - i32::from(from.b)];
    map_pixels(image, |p| {
        if !near(p, from, tolerance) {
            return p;
        }
        let [r, g, b, a] = p;
        [shifted(r, shift[0]), shifted(g, shift[1]), shifted(b, shift[2]), a]
    })
}

/// Pixels within `tolerance` of `source` take the RGB of `target`. Alpha is kept.
fn replace(image: &Bitmap, source: Rgba, target: Rgba, tolerance: u8, auto_source_from_corner: bool) -> Bitmap {
    let source = if auto_source_from_corner {
        let c = &image.pixels()[..4];
        Rgba { r: c[0], g: c[1], b: c[2], a: c[3] }
    } else {
        source
    };
    map_pixels(image, |p| if near(p, source, tolerance) { [target.r, target.g, target.b, p[3]] } else { p })
}

/// Whether `p` is within `tolerance` of `c` in RGB (Euclidean). Alpha is ignored.
fn near(p: [u8; 4], c: Rgba, tolerance: u8) -> bool {
    let sq = |a: u8, b: u8| {
        let d = i32::from(a) - i32::from(b);
        d * d
    };
    let t = i32::from(tolerance);
    sq(p[0], c.r) + sq(p[1], c.g) + sq(p[2], c.b) <= t * t
}

fn shifted(c: u8, delta: i32) -> u8 {
    (i32::from(c) + delta).clamp(0, 255) as u8
}

/// Runs `f` on every RGBA pixel, one row per task.
pub(super) fn map_pixels(image: &Bitmap, f: impl Fn([u8; 4]) -> [u8; 4] + Sync) -> Bitmap {
    let row_len = image.width() as usize * 4;
    let mut px = image.pixels().to_vec();
    px.par_chunks_mut(row_len).for_each(|row| {
        for p in row.as_chunks_mut::<4>().0 {
            *p = f(*p);
        }
    });
    bitmap(image.width(), image.height(), px)
}

/// Clamps a 0..1 value and rounds it to the nearest 8-bit level.
pub(super) fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub(super) fn bitmap(width: u32, height: u32, pixels: Vec<u8>) -> Bitmap {
    Bitmap::new(width, height, PixelFormat::Rgba8, pixels.into()).unwrap_or_else(|e| unreachable!("pixel buffer built to size: {e:?}"))
}

fn diag(v: f32) -> [[f32; 3]; 3] {
    [[v, 0.0, 0.0], [0.0, v, 0.0], [0.0, 0.0, v]]
}

/// Output channel `j` is `sum_i rgb[i][j] * input_i + offset[j]`. Alpha passes through.
fn rgb_matrix(rgb: [[f32; 3]; 3], offset: [f32; 3]) -> Matrix {
    let mut m = [[0.0; 4]; 5];
    for (row, src) in m.iter_mut().zip(rgb) {
        row[..3].copy_from_slice(&src);
    }
    m[3][3] = 1.0;
    m[4][..3].copy_from_slice(&offset);
    m
}

/// `(1 - t) * identity + t * target`. Exact at t = 0 and t = 1. At t = 1 every output channel uses
/// the same weights, so grey stays exactly grey.
fn blend(target: &[[f32; 3]; 3], t: f32) -> Matrix {
    let mut mixed = [[0.0; 3]; 3];
    for (i, row) in target.iter().enumerate() {
        for (j, &goal) in row.iter().enumerate() {
            let keep = if i == j { 1.0 } else { 0.0 };
            mixed[i][j] = keep * (1.0 - t) + goal * t;
        }
    }
    rgb_matrix(mixed, [0.0; 3])
}

fn alpha_matrix(alpha: f32, add: f32) -> Matrix {
    let mut m = rgb_matrix(diag(1.0), [0.0; 3]);
    m[3][3] = alpha;
    m[4][3] = add;
    m
}

fn black_white() -> Matrix {
    rgb_matrix(GRAY.map(|row| row.map(|v| v * BLACK_WHITE_SLOPE)), [0.5 * (1.0 - BLACK_WHITE_SLOPE); 3])
}

/// Each input channel's luma, tinted by the colour's channel for the matching output.
fn colorize_target(color: Rgba) -> [[f32; 3]; 3] {
    let tint = [color.r, color.g, color.b].map(|c| f32::from(c) / 255.0);
    LUMA.map(|l| tint.map(|t| t * l))
}

/// The SVG `hueRotate` matrix for `angle` degrees.
fn hue(angle: f32) -> Matrix {
    let (s, c) = angle.to_radians().sin_cos();
    rgb_matrix(
        [
            [0.213 + c * 0.787 - s * 0.213, 0.213 - c * 0.213 + s * 0.143, 0.213 - c * 0.213 - s * 0.787],
            [0.715 - c * 0.715 - s * 0.715, 0.715 + c * 0.285 + s * 0.140, 0.715 - c * 0.715 + s * 0.715],
            [0.072 - c * 0.072 + s * 0.928, 0.072 - c * 0.072 - s * 0.283, 0.072 + c * 0.928 + s * 0.072],
        ],
        [0.0; 3],
    )
}

fn flat(m: &Matrix) -> [f32; 20] {
    std::array::from_fn(|k| m[k / 4][k % 4])
}
