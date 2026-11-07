//! Alpha from two backdrops (ShareX's Screenshot_Transparent): a window captured over white and over
//! black, with the same pixels both times. Pure, so it builds and is tested on every target.

/// Straight-alpha BGRA pixels from a capture over white and the same capture over black. The two
/// inputs are BGRA buffers of equal length; their alpha bytes are ignored.
///
/// Over white a pixel is `a * c + (1 - a)` and over black it is `a * c`, so the per-channel
/// difference is `1 - a`. The alpha is `255 - max(white - black)` over the colour channels, and the
/// colour is `black / a`, clamped to 255.
pub(crate) fn alpha_from_backdrops(white: &[u8], black: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(black.len());
    for (w, k) in white.as_chunks::<4>().0.iter().zip(black.as_chunks::<4>().0) {
        let spread = w.iter().zip(k).take(3).map(|(&w, &k)| i32::from(w) - i32::from(k)).fold(0, i32::max).clamp(0, 255);
        let alpha = 255 - spread;
        if alpha == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let colour = |c: u8| ((i32::from(c) * 255 + alpha / 2) / alpha).clamp(0, 255) as u8;
        out.extend_from_slice(&[colour(k[0]), colour(k[1]), colour(k[2]), alpha as u8]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_pixel_keeps_its_colour() {
        // Identical over both backdrops means nothing shows through.
        let px = [10, 20, 30, 255];
        assert_eq!(alpha_from_backdrops(&px, &px), vec![10, 20, 30, 255]);
    }

    #[test]
    fn fully_transparent_pixel_is_clear() {
        assert_eq!(alpha_from_backdrops(&[255, 255, 255, 255], &[0, 0, 0, 255]), vec![0, 0, 0, 0]);
    }

    #[test]
    fn half_transparent_red_recovers_alpha_and_colour() {
        // Pure red at alpha 128: black gives R = 128, white gives R = 255 and G = B = 127.
        let black = [0, 0, 128, 255];
        let white = [127, 127, 255, 255];
        assert_eq!(alpha_from_backdrops(&white, &black), vec![0, 0, 255, 128]);
    }

    #[test]
    fn brighter_black_than_white_is_opaque_not_wrapped() {
        // Noise between the two captures can make black brighter than white. That reads as opaque.
        assert_eq!(alpha_from_backdrops(&[0, 0, 0, 255], &[5, 5, 5, 255]), vec![5, 5, 5, 255]);
    }

    #[test]
    fn output_length_follows_the_pixel_count() {
        let white = [255u8; 8];
        let black = [0u8; 8];
        assert_eq!(alpha_from_backdrops(&white, &black).len(), 8);
        assert!(alpha_from_backdrops(&[], &[]).is_empty());
    }
}
