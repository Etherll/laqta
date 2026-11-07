#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]
//! Blend modes, checked on single pixels. The expected values come from the W3C formulas, worked
//! out independently in Python (not from this crate). Opaque pairs and their outputs are in 8-bit,
//! so the separable and most non-separable checks allow +-1.

mod support;

use laqta_domain::Bitmap;
use laqta_photo::{BlendMode, Command, LayerKind, PhotoEditor};

use support::{px, solid};

/// Composites `source` (with alpha) over `backdrop` in `mode`, through a 1x1 document.
fn blend(mode: BlendMode, backdrop: [u8; 4], source: [u8; 4]) -> [u8; 4] {
    let mut ed = PhotoEditor::from_bitmap(solid(1, 1, backdrop));
    ed.execute(Command::AddLayer { name: "top".into(), kind: LayerKind::Raster(solid(1, 1, source)) }).unwrap();
    let top = ed.document().active.unwrap();
    ed.execute(Command::SetBlend { layer: top, blend: mode }).unwrap();
    px(&ed.render(), 0, 0)
}

fn assert_near(got: [u8; 4], want: [u8; 3], what: &str) {
    for c in 0..3 {
        assert!((i32::from(got[c]) - i32::from(want[c])).abs() <= 1, "{what}: got {got:?}, want {want:?}");
    }
    assert_eq!(got[3], 255, "{what}: alpha");
}

/// Backdrop (51, 128, 204), source (179, 77, 128), both opaque. The rows are the W3C B(Cb, Cs)
/// results in 8-bit.
const BACKDROP: [u8; 4] = [51, 128, 204, 255];
const SOURCE: [u8; 4] = [179, 77, 128, 255];

#[test]
fn opaque_pairs_match_the_w3c_formulas() {
    let table: [(BlendMode, [u8; 3]); 16] = [
        (BlendMode::Normal, [179, 77, 128]),
        (BlendMode::Multiply, [36, 39, 102]),
        (BlendMode::Screen, [194, 166, 230]),
        (BlendMode::Overlay, [72, 78, 204]),
        (BlendMode::Darken, [51, 77, 128]),
        (BlendMode::Lighten, [179, 128, 204]),
        (BlendMode::ColorDodge, [171, 183, 255]),
        (BlendMode::ColorBurn, [0, 0, 153]),
        (BlendMode::HardLight, [133, 77, 204]),
        (BlendMode::SoftLight, [77, 103, 204]),
        (BlendMode::Difference, [128, 51, 76]),
        (BlendMode::Exclusion, [158, 128, 127]),
        (BlendMode::Hue, [212, 59, 135]),
        (BlendMode::Saturation, [72, 123, 174]),
        (BlendMode::Color, [179, 77, 128]),
        (BlendMode::Luminosity, [51, 128, 204]),
    ];
    for (mode, want) in table {
        assert_near(blend(mode, BACKDROP, SOURCE), want, &format!("{mode:?}"));
    }
}

#[test]
fn transparent_backdrop_shows_the_source_colour_in_every_mode() {
    // An isolated group starts transparent. The (1 - ab) term makes every mode show the source
    // there, not black.
    let modes = [
        BlendMode::Normal,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::ColorDodge,
        BlendMode::ColorBurn,
        BlendMode::HardLight,
        BlendMode::SoftLight,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
    ];
    for mode in modes {
        assert_eq!(blend(mode, [0, 0, 0, 0], SOURCE), SOURCE, "{mode:?}");
    }
}

#[test]
fn partial_source_alpha_mixes_with_the_backdrop() {
    // Source alpha 128 (= 128/255) over an opaque backdrop: Cr = (1 - as) Cb + as B(Cb, Cs).
    let half = [179, 77, 128, 128];
    assert_near(blend(BlendMode::Multiply, BACKDROP, half), [43, 83, 153], "multiply half");
    assert_near(blend(BlendMode::HardLight, BACKDROP, half), [92, 103, 204], "hard-light half");
    assert_near(blend(BlendMode::Color, BACKDROP, half), [115, 102, 166], "color half");
}

#[test]
fn partial_alpha_over_transparent_keeps_the_source_colour_and_alpha() {
    let got = blend(BlendMode::Multiply, [0, 0, 0, 0], [179, 77, 128, 128]);
    assert_eq!(&got[..3], &[179, 77, 128]);
    assert!((i32::from(got[3]) - 128).abs() <= 1, "alpha {}", got[3]);
}

#[test]
fn clipping_in_the_non_separable_modes() {
    // Color with a yellow source over a blue backdrop: SetLum leaves a negative channel, and
    // ClipColor pulls it back to 0.
    assert_near(blend(BlendMode::Color, [51, 51, 230, 255], [255, 255, 0, 255]), [79, 79, 0], "color clip");

    // A second pair with Hue, Saturation, Color and Luminosity all distinct from one another.
    let backdrop = [26, 230, 102, 255];
    let source = [230, 51, 153, 255];
    assert_near(blend(BlendMode::Hue, backdrop, source), [255, 98, 187], "hue");
    assert_near(blend(BlendMode::Saturation, backdrop, source), [42, 221, 108], "saturation");
    assert_near(blend(BlendMode::Color, backdrop, source), [255, 98, 187], "color");
    assert_near(blend(BlendMode::Luminosity, backdrop, source), [0, 184, 68], "luminosity");
}

#[test]
fn layer_opacity_scales_the_source_alpha() {
    let mut ed = PhotoEditor::from_bitmap(solid(1, 1, BACKDROP));
    ed.execute(Command::AddLayer { name: "top".into(), kind: LayerKind::Raster(solid(1, 1, SOURCE)) }).unwrap();
    let top = ed.document().active.unwrap();
    ed.execute(Command::SetOpacity { layer: top, opacity: 0.5 }).unwrap();
    let got = px(&ed.render(), 0, 0);
    // Normal at half opacity: the average of the backdrop and the source, to within rounding.
    assert_near(got, [115, 103, 166], "half opacity");
}

#[test]
fn render_of_a_bgra_bitmap_matches_rgba() {
    let rgba = solid(2, 2, [10, 20, 30, 255]);
    let px_bgra: Vec<u8> = std::iter::repeat_n([30u8, 20, 10, 255], 4).flatten().collect();
    let bgra = Bitmap::new(2, 2, laqta_domain::PixelFormat::Bgra8, px_bgra.into()).unwrap();
    assert_eq!(PhotoEditor::from_bitmap(bgra).render().pixels(), rgba.pixels());
}
