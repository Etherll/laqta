//! HDR readback math: FP16 scRGB desktop pixels to 8-bit sRGB. Pure,
//! so it builds and is tested on every target.

/// `DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709`: linear scRGB, the colour space of an HDR desktop that
/// Windows composes in FP16.
const COLOR_SPACE_SCRGB: i32 = 1;

/// `DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020`: HDR10, the colour space a display in HDR mode
/// reports for its output.
const COLOR_SPACE_HDR10: i32 = 12;

/// Whether an output with this raw `DXGI_COLOR_SPACE_TYPE` is duplicated as FP16 and tonemapped.
/// Both the scRGB and the HDR10 colour spaces are HDR: an 8-bit duplication of either is a
/// washed-out image.
pub(crate) fn wants_fp16(color_space: i32) -> bool {
    matches!(color_space, COLOR_SPACE_SCRGB | COLOR_SPACE_HDR10)
}

/// Luminance (in SDR-white units) where the highlight roll-off starts. Everything below it is left alone.
const SHOULDER: f32 = 0.9;

/// The largest normalised value considered. Anything brighter is clamped, which keeps the arithmetic
/// finite for infinite and huge inputs.
const MAX_NORMALISED: f32 = 1.0e4;

/// A normalised channel: NaN becomes 0, and everything else is clamped to `0.0..=MAX_NORMALISED`.
fn normalised_channel(c: f32) -> f32 {
    if c.is_nan() { 0.0 } else { c.clamp(0.0, MAX_NORMALISED) }
}

/// Converts an IEEE 754 binary16 bit pattern to `f32`. NaN payloads are not preserved.
pub(crate) fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1F);
    let mantissa = f32::from(bits & 0x3FF);
    let magnitude = match exponent {
        // Subnormal: mantissa * 2^-14 / 1024.
        0 => mantissa * 2f32.powi(-24),
        31 if mantissa == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        e => (1.0 + mantissa / 1024.0) * 2f32.powi(e - 15),
    };
    sign * magnitude
}

/// Maps luminance above `SHOULDER` onto `(SHOULDER, 1.0)` with a Reinhard curve, so it is continuous
/// with the identity below the knee and never exceeds 1.0.
fn shoulder(luma: f32) -> f32 {
    if luma <= SHOULDER {
        return luma;
    }
    let excess = luma - SHOULDER;
    let headroom = 1.0 - SHOULDER;
    SHOULDER + headroom * excess / (excess + headroom)
}

/// The sRGB transfer function for a linear value in `0.0..=1.0`.
fn srgb_encode(linear: f32) -> f32 {
    if linear <= 0.003_130_8 { 12.92 * linear } else { 1.055 * linear.powf(1.0 / 2.4) - 0.055 }
}

/// One linear scRGB pixel (RGBA, 1.0 = 80 nits) to an opaque 8-bit sRGB pixel in RGBA order.
///
/// `sdr_white` is the scRGB value of SDR white (`SDRWhiteLevel / 1000`, so 3.0 for 240 nits). It is
/// divided out first, so SDR white sits at 1.0 before the curve. The curve works on luminance and
/// scales the colour with it, so hue is kept. SDR white lands at 0.95 of the linear output range
/// (sRGB 249), and highlights roll off over the top 10% towards 255 instead of clipping.
pub(crate) fn tonemap_scrgb_to_srgb8(px: [f32; 4], sdr_white: f32) -> [u8; 4] {
    let white = sdr_white.max(f32::EPSILON);
    let normalised = [px[0], px[1], px[2]].map(|c| normalised_channel(c / white));
    let luma = 0.2126 * normalised[0] + 0.7152 * normalised[1] + 0.0722 * normalised[2];
    let scale = if luma > 0.0 { shoulder(luma) / luma } else { 0.0 };
    let rgb = normalised.map(|c| to_u8(srgb_encode((c * scale).clamp(0.0, 1.0))));
    [rgb[0], rgb[1], rgb[2], 255]
}

fn to_u8(unit: f32) -> u8 {
    (unit * 255.0).round() as u8
}

/// Tonemaps a packed frame of RGBA16F pixels (8 bytes each, little-endian) into BGRA8 bytes (4 bytes
/// each, opaque). A trailing partial pixel is ignored.
pub(crate) fn tonemap_frame(raw: &[u8], sdr_white: f32) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() / 2);
    for px in raw.as_chunks::<8>().0 {
        let [r, g, b, a] = [0, 2, 4, 6].map(|i| f16_to_f32(u16::from_le_bytes([px[i], px[i + 1]])));
        let [r, g, b, _] = tonemap_scrgb_to_srgb8([r, g, b, a], sdr_white);
        out.extend_from_slice(&[b, g, r, 255]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wants_fp16_for_scrgb_and_hdr10() {
        assert!(wants_fp16(1), "G10_NONE_P709 (scRGB)");
        assert!(wants_fp16(12), "G2084_NONE_P2020 (HDR10)");
    }

    #[test]
    fn wants_fp16_is_false_for_sdr_colour_spaces() {
        assert!(!wants_fp16(0), "RGB_FULL_G22_NONE_P709 (sRGB)");
        assert!(!wants_fp16(2), "RGB_STUDIO_G22_NONE_P709");
        assert!(!wants_fp16(3), "RGB_STUDIO_G22_NONE_P2020 (wide-gamut SDR)");
        assert!(!wants_fp16(-1), "out of range");
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn f16_known_values() {
        assert!(close(f16_to_f32(0x3C00), 1.0));
        assert!(close(f16_to_f32(0xC000), -2.0));
        assert!(close(f16_to_f32(0x3800), 0.5));
        assert!(close(f16_to_f32(0x7BFF), 65504.0), "largest finite");
        assert!(close(f16_to_f32(0x0400), 2f32.powi(-14)), "smallest normal");
        assert!(close(f16_to_f32(0x0001), 2f32.powi(-24)), "smallest subnormal");
        assert!(f16_to_f32(0x0000) == 0.0 && f16_to_f32(0x0000).is_sign_positive());
        assert!(f16_to_f32(0x8000) == 0.0 && f16_to_f32(0x8000).is_sign_negative(), "negative zero");
        assert_eq!(f16_to_f32(0x7C00), f32::INFINITY);
        assert_eq!(f16_to_f32(0xFC00), f32::NEG_INFINITY);
        assert!(f16_to_f32(0x7E00).is_nan());
    }

    #[test]
    fn black_stays_black_and_opaque() {
        assert_eq!(tonemap_scrgb_to_srgb8([0.0, 0.0, 0.0, 0.0], 3.0), [0, 0, 0, 255]);
        assert_eq!(tonemap_scrgb_to_srgb8([-1.0, f32::NAN, -0.5, 1.0], 3.0), [0, 0, 0, 255], "out-of-gamut and NaN are black");
    }

    #[test]
    fn sdr_white_lands_near_the_top_not_grey() {
        // scRGB 3.0 is SDR white at 240 nits when the white level is 3.0.
        let [r, g, b, a] = tonemap_scrgb_to_srgb8([3.0, 3.0, 3.0, 1.0], 3.0);
        assert!([r, g, b].iter().all(|&c| (247..=251).contains(&c)), "{r} {g} {b}");
        assert_eq!(a, 255);
    }

    #[test]
    fn linear_mid_grey_keeps_its_srgb_value() {
        // Linear 0.5 is below the knee, so only the sRGB encoding applies: about 188.
        let [r, _, _, _] = tonemap_scrgb_to_srgb8([0.5, 0.5, 0.5, 1.0], 1.0);
        assert!((187..=189).contains(&r), "{r}");
    }

    #[test]
    fn output_never_decreases_as_input_rises() {
        let mut last = [0u8; 3];
        for step in 0..=800 {
            let v = step as f32 * 0.5;
            let [r, g, b, _] = tonemap_scrgb_to_srgb8([v, v, v, 1.0], 3.0);
            let now = [r, g, b];
            assert!(now.iter().zip(last).all(|(n, l)| *n >= l), "dropped at {v}: {last:?} -> {now:?}");
            last = now;
        }
        assert_eq!(last, [255, 255, 255], "very bright input reaches the top of the range");
    }

    #[test]
    fn saturated_red_highlight_keeps_other_channels_at_zero() {
        // Pure red far above SDR white: luminance is compressed and the red channel saturates.
        assert_eq!(tonemap_scrgb_to_srgb8([12.0, 0.0, 0.0, 1.0], 3.0), [255, 0, 0, 255]);
    }

    #[test]
    fn frame_converts_rgba16f_to_bgra8() {
        // One opaque pure-red pixel: R = 1.0, G = B = 0, A = 1.0 in FP16.
        let raw = [0x00, 0x3C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3C];
        assert_eq!(tonemap_frame(&raw, 1.0), vec![0, 0, 255, 255]);
    }

    #[test]
    fn frame_length_is_four_bytes_per_pixel() {
        let raw = vec![0u8; 8 * 3];
        assert_eq!(tonemap_frame(&raw, 3.0).len(), 4 * 3);
    }
}
