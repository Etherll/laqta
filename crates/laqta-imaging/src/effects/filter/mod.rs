//! Filter kernels: the classic filters, one function per effect, grouped by kind.
//!
//! Pixels are Rgba8 with straight alpha, in and out. Blurs and the effects that composite a shape
//! work on premultiplied colour, so transparent pixels don't pull in black. The rest work on straight
//! channels and keep the source alpha. Edge pixels clamp. Each parallel task writes its own rows, and
//! randomness is either hashed from `(seed, index)` or drawn from one sequential stream per seed, so
//! the output never depends on the thread count.

mod blur;
mod canvas;
mod cells;
mod convolve;
mod edge;
mod rank;
mod shift;
mod tone;
// ---- X2 ----
// ---- end X2 ----
// ---- X3 ----
// ---- end X3 ----
// ---- X4 ----
// ---- end X4 ----

use laqta_domain::effects::Filter;
use laqta_domain::{Bitmap, PixelFormat};

use super::{EffectContext, EffectError};

/// Caps on parameters that would otherwise ask for unbounded work. Larger values render as the cap.
const MAX_BOX_RADIUS: u32 = 1024;
const MAX_SIGMA: f32 = 64.0;
const MAX_MOTION_DISTANCE: u32 = 256;
const MAX_WINDOW_RADIUS: u32 = 10;

/// Applies one filter. Runs on the compute pool. Empty images come back unchanged.
pub(super) fn apply(image: &Bitmap, effect: &Filter, _cx: &EffectContext<'_>) -> Result<Bitmap, EffectError> {
    debug_assert!(rayon::current_thread_index().is_some(), "filter effects must run inside the compute pool");
    debug_assert_eq!(image.format(), PixelFormat::Rgba8);
    if image.width() == 0 || image.height() == 0 {
        return Ok(image.clone());
    }
    let src = Buf::from_bitmap(image);
    let out = match *effect {
        Filter::Blur { radius } => blur::box_blur(&src, radius.min(MAX_BOX_RADIUS)),
        Filter::ColorDepth { bits } => tone::color_depth(&src, bits),
        Filter::ConvolutionMatrix { kernel, divisor, offset } => convolve::convolve(&src, &kernel, divisor, offset),
        Filter::EdgeDetect => convolve::convolve(&src, &convolve::EDGE_DETECT, 1.0, 0.0),
        Filter::Emboss => convolve::convolve(&src, &convolve::EMBOSS, 1.0, 128.0),
        Filter::GaussianBlur { sigma } => blur::gaussian(&src, cap(sigma, MAX_SIGMA)),
        Filter::Glow { size, strength, color, offset_x, offset_y } => canvas::glow(&src, size, strength, color, (offset_x, offset_y))?,
        Filter::MeanRemoval => convolve::convolve(&src, &convolve::MEAN_REMOVAL, 1.0, 0.0),
        Filter::Outline { size, padding, color, outline_only } => canvas::outline(&src, size, padding, color, outline_only)?,
        Filter::Pixelate { size } => cells::pixelate(&src, size),
        Filter::Reflection { percentage, max_alpha, min_alpha, offset, skew, skew_size } => {
            canvas::reflection(&src, percentage, max_alpha, min_alpha, offset, skew, skew_size)?
        }
        Filter::RgbSplit { offset_red, offset_green, offset_blue } => shift::rgb_split(&src, [offset_red, offset_green, offset_blue]),
        Filter::Shadow { opacity, size, darkness, color, offset_x, offset_y, auto_resize } => {
            canvas::shadow(&src, opacity, size, darkness, color, (offset_x, offset_y), auto_resize)?
        }
        Filter::Sharpen => convolve::convolve(&src, &convolve::SHARPEN, 1.0, 0.0),
        Filter::Slice { min_height, max_height, min_shift, max_shift, seed } => {
            shift::slice(&src, min_height, max_height, min_shift, max_shift, seed)
        }
        Filter::Smooth => convolve::convolve(&src, &convolve::SMOOTH, 16.0, 0.0),
        Filter::TornEdge { depth, range, top, right, bottom, left, curved, seed } => {
            edge::torn(&src, depth, range, [top, right, bottom, left], curved, seed)
        }
        Filter::WaveEdge { depth, range, top, right, bottom, left } => edge::wave(&src, depth, range, [top, right, bottom, left]),
        Filter::Noise { amount, seed } => tone::noise(&src, amount, seed),
        Filter::Posterize { levels } => tone::posterize(&src, levels),
        Filter::Vignette { strength, radius, color } => tone::vignette(&src, strength, radius, color),
        Filter::Halftone { cell } => cells::halftone(&src, cell),
        Filter::Dither { levels } => tone::dither(&src, levels),
        Filter::MotionBlur { distance, angle } => blur::motion(&src, distance.min(MAX_MOTION_DISTANCE), angle),
        Filter::ZoomBlur { amount, center_x, center_y } => blur::zoom(&src, amount, center_x, center_y),
        Filter::Median { radius } => rank::median(&src, radius.min(MAX_WINDOW_RADIUS)),
        Filter::OilPaint { radius, levels } => rank::oil_paint(&src, radius.min(MAX_WINDOW_RADIUS), levels),
        // ---- X2 ----
        // ---- end X2 ----
        // ---- X3 ----
        // ---- end X3 ----
        // ---- X4 ----
        // ---- end X4 ----
    };
    Ok(out.into_bitmap())
}

/// Rgba8 pixels with their size. Kernels read one `Buf` and build another.
#[derive(Clone, Debug)]
pub(super) struct Buf {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Buf {
    fn from_bitmap(image: &Bitmap) -> Buf {
        Buf { w: image.width() as usize, h: image.height() as usize, px: image.pixels().to_vec() }
    }

    fn into_bitmap(self) -> Bitmap {
        let w = u32::try_from(self.w).expect("widths come from a bitmap or a canvas checked to fit u32");
        let h = u32::try_from(self.h).expect("heights come from a bitmap or a canvas checked to fit u32");
        Bitmap::new(w, h, PixelFormat::Rgba8, self.px.into()).unwrap_or_else(|e| unreachable!("pixel buffer built to size: {e:?}"))
    }
}

/// Premultiplied RGBA as 0..=1 floats: each colour channel is scaled by alpha.
fn premultiply(px: &[u8]) -> Vec<f32> {
    px.as_chunks::<4>().0.iter().flat_map(|p| premultiplied(p)).collect()
}

fn premultiplied(p: &[u8]) -> [f32; 4] {
    let a = f32::from(p[3]) / 255.0;
    [f32::from(p[0]) / 255.0 * a, f32::from(p[1]) / 255.0 * a, f32::from(p[2]) / 255.0 * a, a]
}

/// Straight RGBA8 from premultiplied floats. Pixels with no alpha come out as zeros.
fn unpremultiply(pm: &[f32]) -> Vec<u8> {
    pm.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = p[3].clamp(0.0, 1.0);
            if a <= 0.0 {
                return [0, 0, 0, 0];
            }
            let channel = |v: f32| to_u8(v / a * 255.0);
            [channel(p[0]), channel(p[1]), channel(p[2]), to_u8(a * 255.0)]
        })
        .collect()
}

/// Rounds to the nearest byte, clamping; NaN becomes 0.
fn to_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// Clamps to `0..=1`; NaN becomes 0.
fn unit(v: f32) -> f32 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

/// Clamps to `0..=max`; NaN becomes 0.
fn cap(v: f32, max: f32) -> f32 {
    if v.is_nan() { 0.0 } else { v.min(max) }
}

/// Index `offset - radius` steps from `v`, clamped to `0..len`. Callers pass `offset` in `0..=2 * radius`.
fn window_index(v: usize, offset: usize, radius: usize, len: usize) -> usize {
    (v + offset).saturating_sub(radius).min(len - 1)
}

/// Index `d` steps back from `v` (a positive `d` looks toward the start), clamped to `0..len`.
fn shifted(v: usize, d: i64, len: usize) -> usize {
    (v as i64 - d).clamp(0, len as i64 - 1) as usize
}

/// SplitMix64 finaliser: a bijective mix of 64 bits.
fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The value at `index` under `seed`. A pure function, so a parallel loop can call it from any thread.
fn hash(seed: u64, index: u64) -> u64 {
    mix64(mix64(seed).wrapping_add(index))
}

/// Sequential generator for the kernels that draw a few values per row or edge. The seed goes
/// through `mix64` first and the state is kept odd, so no seed leaves the xorshift stuck at zero.
#[derive(Debug)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(mix64(seed) | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in `lo..=hi`. Reversed bounds are swapped.
    fn range(&mut self, lo: u32, hi: u32) -> u32 {
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        lo + (self.next_u64() % (u64::from(hi - lo) + 1)) as u32
    }
}
