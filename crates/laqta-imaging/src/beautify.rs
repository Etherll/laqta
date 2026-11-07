//! The image beautifier: the image on a larger canvas, with a solid or
//! diagonal gradient background, rounded corners and a drop shadow. Pixel work only. It runs on the compute
//! pool with the other kernels and starts no threads of its own.

use laqta_domain::color::Rgba;
use laqta_domain::workflow::BeautifySettings;
use laqta_domain::{Bitmap, PixelFormat};

/// Box blur passes for the shadow. Three passes approximate a Gaussian.
const BLUR_PASSES: usize = 3;

/// The image `img` on a canvas `padding` pixels larger on every side, with rounded corners and a shadow.
/// The result is Rgba8, `2 * padding` pixels larger in each direction. The shadow is clipped at the canvas
/// edge when the offset is larger than the padding.
pub fn beautify(img: &Bitmap, s: &BeautifySettings) -> Bitmap {
    let src = crate::ops::to_rgba(img);
    let (w, h) = (src.width() as usize, src.height() as usize);
    let pad = s.padding as usize;
    let (cw, ch) = (w + 2 * pad, h + 2 * pad);
    let (wf, hf) = (w as f32, h as f32);
    let radius = (s.corner_radius as f32).min(wf / 2.0).min(hf / 2.0);
    let src_px = src.pixels().as_chunks::<4>().0;

    let mut canvas = background(cw, ch, s);

    let opacity = s.shadow_opacity.clamp(0.0, 1.0);
    if s.shadow_size > 0 && opacity > 0.0 {
        let offset = s.shadow_size as usize / 3;
        let mut mask = vec![0.0f32; cw * ch];
        for y in 0..h {
            let ty = y + pad + offset;
            if ty >= ch {
                break;
            }
            for x in 0..w {
                let alpha = f32::from(src_px[y * w + x][3]) / 255.0;
                mask[ty * cw + x + pad] = alpha * coverage(x as f32 + 0.5, y as f32 + 0.5, wf, hf, radius);
            }
        }
        let blur_radius = s.shadow_size as usize / 2;
        if blur_radius > 0 {
            for _ in 0..BLUR_PASSES {
                blur(&mut mask, cw, ch, blur_radius);
            }
        }
        for (px, shadow) in canvas.iter_mut().zip(&mask) {
            over(px, [0.0; 3], shadow * opacity);
        }
    }

    for y in 0..h {
        for x in 0..w {
            let [r, g, b, a] = src_px[y * w + x];
            let alpha = f32::from(a) / 255.0 * coverage(x as f32 + 0.5, y as f32 + 0.5, wf, hf, radius);
            if alpha > 0.0 {
                over(&mut canvas[(y + pad) * cw + x + pad], [f32::from(r), f32::from(g), f32::from(b)], alpha);
            }
        }
    }

    let pixels: Vec<u8> = canvas.as_flattened().to_vec();
    let width = u32::try_from(cw).unwrap_or(u32::MAX);
    let height = u32::try_from(ch).unwrap_or(u32::MAX);
    Bitmap::new(width, height, PixelFormat::Rgba8, pixels.into()).unwrap_or_else(|e| unreachable!("pixel buffer built to size: {e:?}"))
}

/// The canvas before anything is drawn on it: the background, solid or running diagonally from the top-left
/// corner (`background`) to the bottom-right corner (`background_end`).
fn background(cw: usize, ch: usize, s: &BeautifySettings) -> Vec<[u8; 4]> {
    let span = (cw + ch).saturating_sub(2).max(1) as f32;
    let mut out = Vec::with_capacity(cw * ch);
    for y in 0..ch {
        for x in 0..cw {
            out.push(match s.background_end {
                Some(end) => lerp(s.background, end, (x + y) as f32 / span),
                None => [s.background.r, s.background.g, s.background.b, s.background.a],
            });
        }
    }
    out
}

fn lerp(from: Rgba, to: Rgba, t: f32) -> [u8; 4] {
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round().clamp(0.0, 255.0) as u8;
    [mix(from.r, to.r), mix(from.g, to.g), mix(from.b, to.b), mix(from.a, to.a)]
}

/// Composites colour `rgb` (0..255) at alpha `a` (0..1) over the straight-alpha pixel `px`.
fn over(px: &mut [u8; 4], rgb: [f32; 3], a: f32) {
    let da = f32::from(px[3]) / 255.0;
    let out_a = a + da * (1.0 - a);
    if out_a <= 0.0 {
        *px = [0; 4];
        return;
    }
    for (channel, src) in px.iter_mut().take(3).zip(rgb) {
        let v = (src * a + f32::from(*channel) * da * (1.0 - a)) / out_a;
        *channel = v.round().clamp(0.0, 255.0) as u8;
    }
    px[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

/// Coverage (0..1) of the pixel centred at `(x, y)` in a `w` x `h` rectangle with corner radius `r`. The edge
/// is a one-pixel ramp of the signed distance to the rounded rectangle.
fn coverage(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let qx = (x - w / 2.0).abs() - (w / 2.0 - r);
    let qy = (y - h / 2.0).abs() - (h / 2.0 - r);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    let inside = qx.max(qy).min(0.0);
    let distance = outside + inside - r;
    (0.5 - distance).clamp(0.0, 1.0)
}

/// One box blur pass of radius `r` over every row, then every column, of the `w` x `h` buffer. Each output
/// averages its window, clamped to the edges.
fn blur(buf: &mut [f32], w: usize, h: usize, r: usize) {
    if w == 0 || h == 0 {
        return;
    }
    let mut prefix = Vec::with_capacity(w.max(h) + 1);
    for row in buf.chunks_exact_mut(w) {
        box_line(row, r, &mut prefix);
    }
    let mut column = vec![0.0f32; h];
    for x in 0..w {
        for (y, v) in column.iter_mut().enumerate() {
            *v = buf[y * w + x];
        }
        box_line(&mut column, r, &mut prefix);
        for (y, v) in column.iter().enumerate() {
            buf[y * w + x] = *v;
        }
    }
}

/// Averages `line` in place over windows of `2r + 1` samples, clamped to the ends of the line.
fn box_line(line: &mut [f32], r: usize, prefix: &mut Vec<f64>) {
    let n = line.len();
    prefix.clear();
    prefix.push(0.0);
    let mut sum = 0.0f64;
    for v in line.iter() {
        sum += f64::from(*v);
        prefix.push(sum);
    }
    for (i, v) in line.iter_mut().enumerate() {
        let lo = i.saturating_sub(r);
        let hi = (i + r + 1).min(n);
        *v = ((prefix[hi] - prefix[lo]) / (hi - lo) as f64) as f32;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn solid(w: u32, h: u32, px: [u8; 4]) -> Bitmap {
        let pixels: Vec<u8> = std::iter::repeat_n(px, (w * h) as usize).flatten().collect();
        Bitmap::new(w, h, PixelFormat::Rgba8, pixels.into()).unwrap()
    }

    fn at(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * b.width() + x) * 4) as usize;
        b.pixels()[i..i + 4].try_into().unwrap()
    }

    /// No shadow, no corners, solid background: only the padding and the image are drawn.
    fn plain(padding: u32) -> BeautifySettings {
        BeautifySettings {
            padding,
            background: Rgba::rgb(10, 20, 30),
            background_end: None,
            corner_radius: 0,
            shadow_size: 0,
            shadow_opacity: 0.0,
        }
    }

    #[test]
    fn output_is_two_paddings_larger() {
        let out = beautify(&solid(8, 4, [1, 2, 3, 255]), &plain(5));
        assert_eq!((out.width(), out.height()), (18, 14));
        assert_eq!(out.format(), PixelFormat::Rgba8);
    }

    #[test]
    fn corner_is_background() {
        let s = BeautifySettings { background_end: None, ..BeautifySettings::default() };
        let out = beautify(&solid(8, 4, [200, 100, 50, 255]), &s);
        assert_eq!(at(&out, 0, 0), [s.background.r, s.background.g, s.background.b, 255]);
    }

    #[test]
    fn centre_is_the_image() {
        let out = beautify(&solid(8, 4, [200, 100, 50, 255]), &BeautifySettings::default());
        // The default padding is 64 and the image sits at (64, 64); its centre is at (68, 66).
        assert_eq!(at(&out, 68, 66), [200, 100, 50, 255]);
    }

    #[test]
    fn rounded_corner_of_the_image_shows_the_background() {
        let s = BeautifySettings { corner_radius: 8, ..plain(4) };
        let out = beautify(&solid(16, 16, [200, 100, 50, 255]), &s);
        assert_eq!(at(&out, 4, 4), [10, 20, 30, 255], "the image's corner is rounded away");
        assert_eq!(at(&out, 12, 12), [200, 100, 50, 255], "the middle of the image is kept");
    }

    #[test]
    fn gradient_runs_from_background_to_end() {
        let s =
            BeautifySettings { background: Rgba::rgb(0, 0, 0), background_end: Some(Rgba::rgb(254, 102, 52)), shadow_size: 0, ..plain(64) };
        let out = beautify(&solid(8, 4, [1, 2, 3, 255]), &s);
        let (w, h) = (out.width() - 1, out.height() - 1);
        assert_eq!(at(&out, 0, 0), [0, 0, 0, 255]);
        assert_eq!(at(&out, w, h), [254, 102, 52, 255]);
    }

    #[test]
    fn shadow_darkens_the_padding_under_the_image() {
        let s = BeautifySettings {
            background: Rgba::rgb(255, 255, 255),
            background_end: None,
            corner_radius: 0,
            shadow_size: 24,
            shadow_opacity: 1.0,
            padding: 32,
        };
        let out = beautify(&solid(8, 4, [1, 2, 3, 255]), &s);
        // The shadow is the image moved down by 24 / 3 = 8 pixels, so its bottom edge is at y = 32 + 8 + 4 = 44.
        assert!(at(&out, 36, 46)[0] < 255, "the padding just below the shadow is darkened");
        assert_eq!(at(&out, 36, 0), [255, 255, 255, 255], "the padding above the image is not");
    }
}
