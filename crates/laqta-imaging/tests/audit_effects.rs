//! Compatibility checks: adjustment kernels compared with the colour matrices ShareX applies, so
//! imported `.sxie` effects look the same. In a matrix, `values[i][j]` maps input channel i to
//! output channel j, with row 4 the constant offset (in 0..1 units). The expected values below are
//! computed here from those matrices, not from Laqta's output.
#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

use laqta_domain::effects::{Adjust, Effect};
use laqta_domain::{Bitmap, PixelFormat};
use laqta_imaging::effects::{self, EffectContext};

type Rows = [[f32; 5]; 5];

/// `SkiaColorMatrixManager.Polaroid()`.
const SHAREX_POLAROID: Rows = [
    [1.438, -0.062, -0.062, 0.0, 0.0],
    [-0.122, 1.378, -0.122, 0.0, 0.0],
    [-0.016, -0.016, 1.483, 0.0, 0.0],
    [0.0, 0.0, 0.0, 1.0, 0.0],
    [-0.03, 0.05, -0.02, 0.0, 1.0],
];

/// `SkiaColorMatrixManager.BlackWhite()`.
const SHAREX_BLACK_WHITE: Rows = [
    [1.5, 1.5, 1.5, 0.0, 0.0],
    [1.5, 1.5, 1.5, 0.0, 0.0],
    [1.5, 1.5, 1.5, 0.0, 0.0],
    [0.0, 0.0, 0.0, 1.0, 0.0],
    [-1.0, -1.0, -1.0, 0.0, 1.0],
];

/// `SkiaColorMatrixManager.Contrast(value)`: a plain multiply, no pivot.
fn sharex_contrast(value: f32) -> Rows {
    [
        [value, 0.0, 0.0, 0.0, 0.0],
        [0.0, value, 0.0, 0.0, 0.0],
        [0.0, 0.0, value, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 1.0],
    ]
}

/// A mid-tone pixel, so nothing clamps and a transposed matrix shows.
const PIXEL: [u8; 4] = [128, 100, 60, 255];

fn sharex_apply(m: &Rows, p: [u8; 4]) -> [u8; 4] {
    let x = p.map(|c| f32::from(c) / 255.0);
    std::array::from_fn(|j| {
        let v = x[0] * m[0][j] + x[1] * m[1][j] + x[2] * m[2][j] + x[3] * m[3][j] + m[4][j];
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    })
}

fn laqta_apply(adjust: Adjust) -> [u8; 4] {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let image = Bitmap::new(1, 1, PixelFormat::Rgba8, PIXEL.to_vec().into()).unwrap();
    let out = pool.install(|| effects::apply(&image, &Effect::Adjust(adjust), &EffectContext::none())).unwrap();
    out.pixels()[..4].try_into().unwrap()
}

fn close(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 2)
}

#[test]
fn audit_bug_polaroid_matrix_is_transposed() {
    let want = sharex_apply(&SHAREX_POLAROID, PIXEL);
    let got = laqta_apply(Adjust::Polaroid);
    assert!(close(got, want), "Polaroid of {PIXEL:?}: ShareX {want:?}, Laqta {got:?}");
}

#[test]
fn audit_bug_black_white_differs_from_sharex() {
    let want = sharex_apply(&SHAREX_BLACK_WHITE, PIXEL);
    let got = laqta_apply(Adjust::BlackWhite);
    assert!(close(got, want), "BlackWhite of {PIXEL:?}: ShareX {want:?}, Laqta {got:?}");
}

/// ShareX Contrast 1.5 multiplies R, G and B by 1.5, with no pivot. Laqta's `Adjust::Contrast` pivots on
/// mid grey, so its own scale gives a different picture and stays as it is. The `.sxie` import reads a
/// ShareX `Contrast` as `Adjust::MatrixColor` holding ShareX's matrix, and that effect must match ShareX
/// pixel for pixel.
#[test]
fn audit_bug_contrast_pivots_where_sharex_does_not() {
    let rows = sharex_contrast(1.5);
    let matrix: [f32; 20] = std::array::from_fn(|k| rows[k / 4][k % 4]);
    let want = sharex_apply(&rows, PIXEL);
    let got = laqta_apply(Adjust::MatrixColor { matrix });
    assert!(close(got, want), "Contrast x1.5 of {PIXEL:?}: ShareX {want:?}, MatrixColor {got:?}");
}

/// ShareX `Scale.Apply`: a zero percentage keeps the aspect ratio (`ApplyAspectRatio`), and ShareX's
/// default is `HeightPercentage = 0`. The import reads the same property names, so a ShareX Scale
/// preset reaches this kernel as `height_pct: 0`.
#[test]
fn audit_bug_scale_zero_percent_is_not_keep_aspect() {
    use laqta_domain::effects::Manip;
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let image = Bitmap::new(8, 4, PixelFormat::Rgba8, vec![200u8; 8 * 4 * 4].into()).unwrap();
    let effect = Effect::Manip(Manip::Scale { width_pct: 50.0, height_pct: 0.0 });
    let out = pool.install(|| effects::apply(&image, &effect, &EffectContext::none())).unwrap();
    assert_eq!((out.width(), out.height()), (4, 2), "Scale 50% x 0% of 8x4");
}
