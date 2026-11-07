//! Drawing kernels (the 8 classic drawing effects). This module also holds the raster helpers that
//! `manip.rs` shares: a straight-alpha RGBA8 buffer, source-over, bilinear sampling and rotation.
//!
//! Every kernel works on `Buf` and returns Rgba8. `apply` runs on the compute pool (`ops::resize`
//! needs it).

use laqta_domain::color::Rgba;
use laqta_domain::effects::TextSpec;
use laqta_domain::effects::draw::{BorderKind, Draw, Placement, SizeMode};
use laqta_domain::{Bitmap, PixelFormat};

use super::draw_m7;
use super::{EffectContext, EffectError};

/// Largest working buffer, in pixels (1 GiB of RGBA8).
const MAX_PIXELS: u64 = 1 << 28;

/// Slack for float error when a rounded-up size should be an exact integer.
const EPS: f64 = 1e-9;

pub(super) fn apply(image: &Bitmap, effect: &Draw, cx: &EffectContext<'_>) -> Result<Bitmap, EffectError> {
    debug_assert!(rayon::current_thread_index().is_some(), "effects must run inside the compute pool");
    let base = Buf::from_bitmap(image);
    let out = match effect {
        Draw::Background { color, gradient } => background(&base, *color, *gradient),
        Draw::BackgroundImage { path, center, tile } => background_image(&base, cx, path, *center, *tile)?,
        Draw::Border { kind, size, color, dash } => border(&base, *kind, *size, *color, *dash)?,
        Draw::Checkerboard { size, color1, color2 } => checkerboard(&base, *size, *color1, *color2),
        Draw::Image { path, placement, offset, size_mode, opacity } => {
            overlay(&base, cx, path, Place { anchor: *placement, offset: *offset }, *size_mode, *opacity)?
        }
        Draw::Particles { path, count, min_size, max_size, random_angle, seed } => {
            particles(&base, cx, path, *count, (*min_size, *max_size), *random_angle, *seed)?
        }
        Draw::Text { text, placement, offset, background, padding, corner_radius } => {
            text_block(&base, cx, text, Place { anchor: *placement, offset: *offset }, *background, *padding, *corner_radius)?
        }
        Draw::TextEx { text, angle, placement, offset, outline, shadow } => {
            text_ex(&base, cx, text, *angle, Place { anchor: *placement, offset: *offset }, *outline, *shadow)?
        }
        Draw::Confetti { count, min_size, max_size, seed } => draw_m7::confetti(&base, *count, (*min_size, *max_size), *seed),
        Draw::DrawLine { from, to, width, color } => draw_m7::line(&base, *from, *to, *width, rgba(*color)),
        Draw::DrawShape { shape, x, y, width, height, fill, stroke, stroke_width } => {
            draw_m7::shape(&base, *shape, (*x, *y, *width, *height), (*fill).map(rgba), rgba(*stroke), *stroke_width)
        }
        Draw::FakeCursor { x, y, size, shadow } => draw_m7::cursor(&base, (*x, *y), *size, *shadow),
        Draw::GradientOverlay { start, end, angle, opacity } => draw_m7::gradient_overlay(&base, *start, *end, *angle, *opacity),
        Draw::Snowflakes { count, min_size, max_size, seed } => draw_m7::snowflakes(&base, *count, (*min_size, *max_size), *seed),
    };
    out.into_bitmap()
}

/// Fills behind the image, with an optional linear gradient.
fn background(base: &Buf, color: Rgba, gradient: Option<(Rgba, Rgba, f32)>) -> Buf {
    let (w, h) = (base.w, base.h);
    let mut canvas = Buf::filled(w, h, rgba(color));
    if let Some((from, to, angle)) = gradient {
        let (dy, dx) = angle.to_radians().sin_cos();
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        // Half the image's extent along the gradient direction maps to 0 and 1.
        let half = (w as f32 * dx.abs() + h as f32 * dy.abs()) / 2.0;
        for y in 0..h {
            for x in 0..w {
                let along = (x as f32 + 0.5 - cx) * dx + (y as f32 + 0.5 - cy) * dy;
                let t = if half > 0.0 { (along / half + 1.0) / 2.0 } else { 0.5 };
                canvas.set(x, y, lerp(from, to, t.clamp(0.0, 1.0)));
            }
        }
    }
    canvas.blend(base, 0, 0, 1.0);
    canvas
}

/// An asset behind the image: tiled from the top-left, centred at its own size, or stretched.
fn background_image(base: &Buf, cx: &EffectContext<'_>, path: &str, center: bool, tile: bool) -> Result<Buf, EffectError> {
    let img = load_asset(cx, path)?;
    let (w, h) = (base.w, base.h);
    let mut canvas = Buf::new(w, h);
    if tile {
        if img.w > 0 && img.h > 0 {
            for y in 0..h {
                for x in 0..w {
                    canvas.set(x, y, img.get(x % img.w, y % img.h));
                }
            }
        }
    } else if center {
        canvas.blend(&img, (w as i64 - img.w as i64) / 2, (h as i64 - img.h as i64) / 2, 1.0);
    } else {
        let stretched = resize_buf(&img, w, h)?;
        canvas.blend(&stretched, 0, 0, 1.0);
    }
    canvas.blend(base, 0, 0, 1.0);
    Ok(canvas)
}

/// A frame drawn over the image's edge (inside) or added around it (outside).
fn border(base: &Buf, kind: BorderKind, size: u32, color: Rgba, dash: bool) -> Result<Buf, EffectError> {
    let s = size as usize;
    if s == 0 {
        return Ok(base.clone());
    }
    match kind {
        BorderKind::Outside => {
            let (w, h) = (base.w + 2 * s, base.h + 2 * s);
            ensure_size(w, h)?;
            let mut outer = Buf::new(w, h);
            paint_ring(&mut outer, s, rgba(color), dash);
            outer.blend(base, s as i64, s as i64, 1.0);
            Ok(outer)
        }
        BorderKind::Inside => {
            let mut canvas = base.clone();
            paint_ring(&mut canvas, s, rgba(color), dash);
            Ok(canvas)
        }
    }
}

/// Paints the outer `s` pixels of `canvas`. A dashed ring is 3 units on and 1 off along each edge.
/// Top and bottom edges run along x, and the sides along y.
fn paint_ring(canvas: &mut Buf, s: usize, color: [u8; 4], dash: bool) {
    let (w, h) = (canvas.w, canvas.h);
    for y in 0..h {
        for x in 0..w {
            let on_ring = x < s || y < s || x + s >= w || y + s >= h;
            if !on_ring {
                continue;
            }
            let along = if y < s || y + s >= h { x } else { y };
            if dash && along % (4 * s) >= 3 * s {
                continue;
            }
            canvas.blend_px(x, y, color, 1.0);
        }
    }
}

/// Squares in two colours, behind the image.
fn checkerboard(base: &Buf, size: u32, color1: Rgba, color2: Rgba) -> Buf {
    let s = size.max(1) as usize;
    let (a, b) = (rgba(color1), rgba(color2));
    let mut canvas = Buf::new(base.w, base.h);
    for y in 0..base.h {
        for x in 0..base.w {
            let even = ((x / s) + (y / s)).is_multiple_of(2);
            canvas.set(x, y, if even { a } else { b });
        }
    }
    canvas.blend(base, 0, 0, 1.0);
    canvas
}

/// An asset resized, placed and blended over the image.
fn overlay(base: &Buf, cx: &EffectContext<'_>, path: &str, place: Place, size_mode: SizeMode, opacity: f32) -> Result<Buf, EffectError> {
    let img = load_asset(cx, path)?;
    let (w, h) = match size_mode {
        SizeMode::DontResize => (img.w, img.h),
        SizeMode::Absolute(w, h) => (if w == 0 { img.w } else { w as usize }, if h == 0 { img.h } else { h as usize }),
        SizeMode::Percentage(p) => (percent(img.w, p), percent(img.h, p)),
    };
    let img = resize_buf(&img, w, h)?;
    let (x, y) = place.position((base.w, base.h), (img.w, img.h));
    let mut canvas = base.clone();
    canvas.blend(&img, x, y, opacity.clamp(0.0, 1.0));
    Ok(canvas)
}

/// Scales a side by a percentage, never below 1 pixel.
fn percent(len: usize, pct: f32) -> usize {
    ((len as f32) * pct / 100.0).round().max(1.0) as usize
}

/// `count` copies of an asset at seeded random positions, sizes and (optionally) angles.
fn particles(
    base: &Buf,
    cx: &EffectContext<'_>,
    path: &str,
    count: u32,
    sizes: (u32, u32),
    random_angle: bool,
    seed: u64,
) -> Result<Buf, EffectError> {
    let img = load_asset(cx, path)?;
    let mut canvas = base.clone();
    if count == 0 || img.w == 0 || img.h == 0 || canvas.w == 0 || canvas.h == 0 {
        return Ok(canvas);
    }
    let lo = u64::from(sizes.0.min(sizes.1).max(1));
    let hi = u64::from(sizes.0.max(sizes.1).max(1));
    let (iw, ih) = (img.w as u64, img.h as u64);
    let mut rng = SplitMix64(seed);
    for _ in 0..count {
        let width = lo + rng.below(hi - lo + 1);
        let height = ((width * ih + iw / 2) / iw).max(1);
        let mut piece = resize_buf(&img, width as usize, height as usize)?;
        if random_angle {
            piece = rotate_buf(&piece, rng.below(360) as f32, true, [0; 4])?;
        }
        let x = rng.below(canvas.w as u64) as i64;
        let y = rng.below(canvas.h as u64) as i64;
        canvas.blend(&piece, x, y, 1.0);
    }
    Ok(canvas)
}

/// Text on an optional rounded background, placed over the image.
fn text_block(
    base: &Buf,
    cx: &EffectContext<'_>,
    spec: &TextSpec,
    place: Place,
    background: Option<Rgba>,
    padding: u32,
    corner_radius: u32,
) -> Result<Buf, EffectError> {
    let glyphs = render_text(cx, spec)?;
    let pad = padding as usize;
    let (w, h) = (glyphs.w + 2 * pad, glyphs.h + 2 * pad);
    ensure_size(w, h)?;
    let mut block = Buf::new(w, h);
    if let Some(color) = background {
        fill_rounded(&mut block, color, corner_radius as f32);
    }
    block.blend(&glyphs, pad as i64, pad as i64, 1.0);
    let (x, y) = place.position((base.w, base.h), (w, h));
    let mut canvas = base.clone();
    canvas.blend(&block, x, y, 1.0);
    Ok(canvas)
}

/// Text turned by `angle`, with an outline and a shadow, placed over the image.
fn text_ex(
    base: &Buf,
    cx: &EffectContext<'_>,
    spec: &TextSpec,
    angle: f32,
    place: Place,
    outline: Option<(Rgba, f32)>,
    shadow: Option<(Rgba, i32, i32)>,
) -> Result<Buf, EffectError> {
    let glyphs = render_text(cx, spec)?;
    let outlined = match outline {
        Some((color, width)) => with_outline(&glyphs, color, width)?,
        None => glyphs,
    };
    let mut block = rotate_buf(&outlined, angle, true, [0; 4])?;
    if let Some((color, dx, dy)) = shadow {
        block = with_shadow(&block, color, dx, dy)?;
    }
    let (x, y) = place.position((base.w, base.h), (block.w, block.h));
    let mut canvas = base.clone();
    canvas.blend(&block, x, y, 1.0);
    Ok(canvas)
}

/// Draws the text with a dilated copy in `color` behind it. The dilation is a disc of `width` pixels.
fn with_outline(glyphs: &Buf, color: Rgba, width: f32) -> Result<Buf, EffectError> {
    let width = width.max(0.0);
    let r = width.ceil() as usize;
    let (w, h) = (glyphs.w + 2 * r, glyphs.h + 2 * r);
    ensure_size(w, h)?;
    let ri = r as i64;
    let reach = width * width;
    let mut alpha = vec![0u8; w * h];
    for y in 0..glyphs.h {
        for x in 0..glyphs.w {
            let a = glyphs.get(x, y)[3];
            if a == 0 {
                continue;
            }
            for dy in -ri..=ri {
                for dx in -ri..=ri {
                    if ((dx * dx + dy * dy) as f32) > reach {
                        continue;
                    }
                    let (px, py) = (x as i64 + r as i64 + dx, y as i64 + r as i64 + dy);
                    if (0..w as i64).contains(&px) && (0..h as i64).contains(&py) {
                        let i = py as usize * w + px as usize;
                        alpha[i] = alpha[i].max(a);
                    }
                }
            }
        }
    }
    let mut out = Buf::new(w, h);
    let outline_rgba = rgba(color);
    for (i, &a) in alpha.iter().enumerate() {
        if a > 0 {
            out.blend_px(i % w, i / w, outline_rgba, f32::from(a) / 255.0);
        }
    }
    out.blend(glyphs, r as i64, r as i64, 1.0);
    Ok(out)
}

/// A copy of `block` with its alpha tinted by `color`, offset by (`dx`, `dy`), with the block over it.
/// The canvas grows to hold the shadow.
fn with_shadow(block: &Buf, color: Rgba, dx: i32, dy: i32) -> Result<Buf, EffectError> {
    let left = usize::try_from((-i64::from(dx)).max(0)).unwrap_or(0);
    let top = usize::try_from((-i64::from(dy)).max(0)).unwrap_or(0);
    let right = usize::try_from(i64::from(dx).max(0)).unwrap_or(0);
    let bottom = usize::try_from(i64::from(dy).max(0)).unwrap_or(0);
    let (w, h) = (block.w + left + right, block.h + top + bottom);
    ensure_size(w, h)?;
    let mut shade = Buf::new(block.w, block.h);
    for y in 0..block.h {
        for x in 0..block.w {
            let a = block.get(x, y)[3];
            if a > 0 {
                shade.set(x, y, [color.r, color.g, color.b, scale_alpha(a, color.a)]);
            }
        }
    }
    let mut out = Buf::new(w, h);
    out.blend(&shade, left as i64 + i64::from(dx), top as i64 + i64::from(dy), 1.0);
    out.blend(block, left as i64, top as i64, 1.0);
    Ok(out)
}

/// Fills a rounded rectangle covering all of `block`, anti-aliased at the corners.
fn fill_rounded(block: &mut Buf, color: Rgba, radius: f32) {
    let (w, h) = (block.w as f32, block.h as f32);
    let src = rgba(color);
    for y in 0..block.h {
        for x in 0..block.w {
            let cov = rounded_coverage(x as f32 + 0.5, y as f32 + 0.5, w, h, radius);
            block.blend_px(x, y, src, cov);
        }
    }
}

fn render_text(cx: &EffectContext<'_>, spec: &TextSpec) -> Result<Buf, EffectError> {
    (cx.text)(spec).map(|bitmap| Buf::from_bitmap(&bitmap)).ok_or(EffectError::NoText)
}

fn load_asset(cx: &EffectContext<'_>, path: &str) -> Result<Buf, EffectError> {
    (cx.asset)(path).map(|bitmap| Buf::from_bitmap(&bitmap)).ok_or_else(|| EffectError::MissingAsset(path.to_owned()))
}

/// Where an item of a given size goes in a canvas, for an anchor and an offset.
#[derive(Clone, Copy)]
struct Place {
    anchor: Placement,
    offset: (i32, i32),
}

impl Place {
    /// Top-left corner of an `item` (width, height) inside a `canvas` (width, height).
    fn position(self, canvas: (usize, usize), item: (usize, usize)) -> (i64, i64) {
        let (cw, ch) = (canvas.0 as i64, canvas.1 as i64);
        let (iw, ih) = (item.0 as i64, item.1 as i64);
        let (x, y) = match self.anchor {
            Placement::TopLeft => (0, 0),
            Placement::TopCenter => ((cw - iw) / 2, 0),
            Placement::TopRight => (cw - iw, 0),
            Placement::MiddleLeft => (0, (ch - ih) / 2),
            Placement::MiddleCenter => ((cw - iw) / 2, (ch - ih) / 2),
            Placement::MiddleRight => (cw - iw, (ch - ih) / 2),
            Placement::BottomLeft => (0, ch - ih),
            Placement::BottomCenter => ((cw - iw) / 2, ch - ih),
            Placement::BottomRight => (cw - iw, ch - ih),
        };
        (x + i64::from(self.offset.0), y + i64::from(self.offset.1))
    }
}

/// Straight-alpha RGBA8 pixels, row-major. The working format of every kernel.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Buf {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Buf {
    /// Fully transparent.
    pub(super) fn new(w: usize, h: usize) -> Buf {
        Buf { w, h, px: vec![0; w * h * 4] }
    }

    pub(super) fn filled(w: usize, h: usize, px: [u8; 4]) -> Buf {
        Buf { w, h, px: std::iter::repeat_n(px, w * h).flatten().collect() }
    }

    /// Copies an Rgba8 or Bgra8 bitmap into Rgba8.
    pub(super) fn from_bitmap(bitmap: &Bitmap) -> Buf {
        let mut px = bitmap.pixels().to_vec();
        if bitmap.format() == PixelFormat::Bgra8 {
            for p in px.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
            }
        }
        Buf { w: bitmap.width() as usize, h: bitmap.height() as usize, px }
    }

    pub(super) fn into_bitmap(self) -> Result<Bitmap, EffectError> {
        let (w, h) = (to_u32(self.w)?, to_u32(self.h)?);
        Bitmap::new(w, h, PixelFormat::Rgba8, self.px.into()).map_err(|e| EffectError::Invalid(e.to_string()))
    }

    pub(super) fn get(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.w + x) * 4;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }

    pub(super) fn set(&mut self, x: usize, y: usize, p: [u8; 4]) {
        let i = (y * self.w + x) * 4;
        self.px[i..i + 4].copy_from_slice(&p);
    }

    /// Source-over of one pixel. `opacity` scales the source alpha.
    pub(super) fn blend_px(&mut self, x: usize, y: usize, src: [u8; 4], opacity: f32) {
        let alpha = f32::from(src[3]) / 255.0 * opacity;
        if alpha <= 0.0 {
            return;
        }
        let i = (y * self.w + x) * 4;
        let out = over(self.get(x, y), src, alpha);
        self.px[i..i + 4].copy_from_slice(&out);
    }

    /// Source-over of `src` with its top-left corner at (`x0`, `y0`), clipped to this buffer.
    pub(super) fn blend(&mut self, src: &Buf, x0: i64, y0: i64, opacity: f32) {
        for sy in 0..src.h {
            let dy = y0 + sy as i64;
            if !(0..self.h as i64).contains(&dy) {
                continue;
            }
            for sx in 0..src.w {
                let dx = x0 + sx as i64;
                if (0..self.w as i64).contains(&dx) {
                    self.blend_px(dx as usize, dy as usize, src.get(sx, sy), opacity);
                }
            }
        }
    }

    /// The `w` x `h` rectangle at (`x`, `y`). The caller keeps it inside the buffer.
    pub(super) fn sub(&self, x: usize, y: usize, w: usize, h: usize) -> Buf {
        let mut px = Vec::with_capacity(w * h * 4);
        for row in y..y + h {
            let start = (row * self.w + x) * 4;
            px.extend_from_slice(&self.px[start..start + w * 4]);
        }
        Buf { w, h, px }
    }

    /// Bilinear sample at continuous coordinates, where pixel `i` covers `[i, i + 1)`. Interpolates
    /// premultiplied, so semi-transparent edges don't darken. Outside the buffer counts as transparent.
    pub(super) fn sample(&self, x: f64, y: f64) -> [u8; 4] {
        let (fx, fy) = (x - 0.5, y - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let (ix, iy) = (x0 as i64, y0 as i64);
        let taps: [(i64, i64, f64); 4] =
            [(0, 0, (1.0 - tx) * (1.0 - ty)), (1, 0, tx * (1.0 - ty)), (0, 1, (1.0 - tx) * ty), (1, 1, tx * ty)];
        let mut acc = [0.0f64; 4];
        for (dx, dy, weight) in taps {
            let (px, py) = (ix + dx, iy + dy);
            if weight <= 0.0 || !(0..self.w as i64).contains(&px) || !(0..self.h as i64).contains(&py) {
                continue;
            }
            let p = self.get(px as usize, py as usize);
            let a = f64::from(p[3]) / 255.0 * weight;
            acc[0] += f64::from(p[0]) / 255.0 * a;
            acc[1] += f64::from(p[1]) / 255.0 * a;
            acc[2] += f64::from(p[2]) / 255.0 * a;
            acc[3] += a;
        }
        if acc[3] <= 0.0 {
            return [0; 4];
        }
        let inv = 1.0 / acc[3];
        [
            to_u8((acc[0] * inv * 255.0) as f32),
            to_u8((acc[1] * inv * 255.0) as f32),
            to_u8((acc[2] * inv * 255.0) as f32),
            to_u8((acc[3] * 255.0) as f32),
        ]
    }

    /// The pixel containing continuous coordinates (`x`, `y`). Outside the buffer counts as transparent.
    pub(super) fn nearest(&self, x: f64, y: f64) -> [u8; 4] {
        let (ix, iy) = (x.floor() as i64, y.floor() as i64);
        if (0..self.w as i64).contains(&ix) && (0..self.h as i64).contains(&iy) { self.get(ix as usize, iy as usize) } else { [0; 4] }
    }
}

/// Resamples with Lanczos3 (`ops::resize`), with each side at least 1 pixel. The same size is a clone.
pub(super) fn resize_buf(src: &Buf, w: usize, h: usize) -> Result<Buf, EffectError> {
    let (w, h) = (w.max(1), h.max(1));
    if (w, h) == (src.w, src.h) {
        return Ok(src.clone());
    }
    ensure_size(w, h)?;
    let scaled = crate::ops::resize(&src.clone().into_bitmap()?, to_u32(w)?, to_u32(h)?);
    Ok(Buf::from_bitmap(&scaled))
}

/// Rotates clockwise by `angle` degrees about the centre (y points down). Multiples of 90 are exact.
/// With `grow` the canvas becomes the rotated bounding box, otherwise it keeps its size. Uncovered
/// pixels take `fill`.
pub(super) fn rotate_buf(src: &Buf, angle: f32, grow: bool, fill: [u8; 4]) -> Result<Buf, EffectError> {
    let deg = f64::from(angle).rem_euclid(360.0);
    let (cos, sin) = cos_sin(deg);
    // Quarter turns map pixel grid onto pixel grid, but an odd size difference can put a sample on a
    // pixel edge. Nearest-neighbour keeps those exact too, where bilinear would average two pixels.
    let quarter = is_quarter_turn(deg);
    let (sw, sh) = (src.w as f64, src.h as f64);
    let (dw, dh) = if grow {
        let w = (sw * cos.abs() + sh * sin.abs() - EPS).ceil().max(0.0) as usize;
        let h = (sw * sin.abs() + sh * cos.abs() - EPS).ceil().max(0.0) as usize;
        (w, h)
    } else {
        (src.w, src.h)
    };
    ensure_size(dw, dh)?;
    let mut out = Buf::filled(dw, dh, fill);
    for y in 0..dh {
        for x in 0..dw {
            // Output pixel centre, relative to the output centre, mapped back by the inverse rotation.
            let px = x as f64 + 0.5 - dw as f64 / 2.0;
            let py = y as f64 + 0.5 - dh as f64 / 2.0;
            let sx = px * cos + py * sin + sw / 2.0;
            let sy = -px * sin + py * cos + sh / 2.0;
            let p = if quarter { src.nearest(sx, sy) } else { src.sample(sx, sy) };
            out.blend_px(x, y, p, 1.0);
        }
    }
    Ok(out)
}

/// Whether a clockwise angle in degrees is a multiple of 90.
fn is_quarter_turn(deg: f64) -> bool {
    (deg - (deg / 90.0).round() * 90.0).abs() < EPS
}

/// Cosine and sine of a clockwise angle in degrees. Multiples of 90 are exact.
fn cos_sin(deg: f64) -> (f64, f64) {
    if is_quarter_turn(deg) {
        let quarter = (deg / 90.0).round();
        return match (quarter as i64).rem_euclid(4) {
            0 => (1.0, 0.0),
            1 => (0.0, 1.0),
            2 => (-1.0, 0.0),
            _ => (0.0, -1.0),
        };
    }
    let rad = deg.to_radians();
    (rad.cos(), rad.sin())
}

/// Coverage (0..=1) of the pixel centre (`x`, `y`) inside a `w` x `h` rectangle with corners of
/// `radius` pixels. The edge is anti-aliased over one pixel.
pub(super) fn rounded_coverage(x: f32, y: f32, w: f32, h: f32, radius: f32) -> f32 {
    let r = radius.clamp(0.0, w.min(h) / 2.0);
    let qx = (x - w / 2.0).abs() - (w / 2.0 - r);
    let qy = (y - h / 2.0).abs() - (h / 2.0 - r);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    let inside = qx.max(qy).min(0.0);
    let dist = outside + inside - r;
    (0.5 - dist).clamp(0.0, 1.0)
}

/// Errors when a `w` x `h` buffer would be too large to build.
pub(super) fn ensure_size(w: usize, h: usize) -> Result<(), EffectError> {
    let (w, h) = (w as u64, h as u64);
    if w > u64::from(u32::MAX) || h > u64::from(u32::MAX) || w * h > MAX_PIXELS {
        return Err(EffectError::Invalid(format!("{w}x{h} image is too large")));
    }
    Ok(())
}

pub(super) fn rgba(c: Rgba) -> [u8; 4] {
    [c.r, c.g, c.b, c.a]
}

fn to_u32(v: usize) -> Result<u32, EffectError> {
    u32::try_from(v).map_err(|_| EffectError::Invalid(format!("{v} is too large")))
}

pub(super) fn to_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// Source-over on straight alpha. `src_alpha` is the source's effective alpha, already scaled by
/// opacity; the colour channels of `src` are used as they are.
fn over(dst: [u8; 4], src: [u8; 4], src_alpha: f32) -> [u8; 4] {
    let dst_alpha = f32::from(dst[3]) / 255.0;
    let out_alpha = src_alpha + dst_alpha * (1.0 - src_alpha);
    if out_alpha <= 0.0 {
        return [0; 4];
    }
    let mix = |s: u8, d: u8| to_u8((f32::from(s) * src_alpha + f32::from(d) * dst_alpha * (1.0 - src_alpha)) / out_alpha);
    [mix(src[0], dst[0]), mix(src[1], dst[1]), mix(src[2], dst[2]), to_u8(out_alpha * 255.0)]
}

pub(super) fn lerp(a: Rgba, b: Rgba, t: f32) -> [u8; 4] {
    let mix = |x: u8, y: u8| to_u8(f32::from(x) + (f32::from(y) - f32::from(x)) * t);
    [mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b), mix(a.a, b.a)]
}

/// Alpha `a` scaled by the colour's alpha `b`, rounded.
fn scale_alpha(a: u8, b: u8) -> u8 {
    ((u32::from(a) * u32::from(b) + 127) / 255) as u8
}

/// SplitMix64. A fixed generator, so one seed gives the same layout on every platform.
pub(super) struct SplitMix64(pub(super) u64);

impl SplitMix64 {
    pub(super) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..n`, or 0 when `n` is 0.
    pub(super) fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }
}

/// A uniform value in 0..1 from a seed and two integers. It is a pure function of its inputs, so
/// per-pixel noise doesn't depend on the order pixels are visited in.
pub(super) fn hash_unit(seed: u64, a: u64, b: u64) -> f64 {
    let mut rng = SplitMix64(seed ^ a.wrapping_mul(0xD6E8_FEB8_6659_FD93) ^ b.rotate_left(29).wrapping_mul(0xA076_1D64_78BD_642F));
    (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn over_opaque_source_replaces_and_transparent_keeps() {
        assert_eq!(over([10, 20, 30, 255], [200, 100, 50, 255], 1.0), [200, 100, 50, 255]);
        assert_eq!(over([10, 20, 30, 255], [200, 100, 50, 0], 0.0), [10, 20, 30, 255]);
        assert_eq!(over([0, 0, 0, 0], [7, 8, 9, 0], 0.0), [0, 0, 0, 0]);
    }

    #[test]
    fn cos_sin_is_exact_at_quarter_turns() {
        assert_eq!(cos_sin(0.0), (1.0, 0.0));
        assert_eq!(cos_sin(90.0), (0.0, 1.0));
        assert_eq!(cos_sin(180.0), (-1.0, 0.0));
        assert_eq!(cos_sin(270.0), (0.0, -1.0));
    }

    #[test]
    fn rounded_coverage_is_one_inside_and_zero_in_the_corner() {
        assert_eq!(rounded_coverage(10.5, 8.5, 20.0, 16.0, 5.0), 1.0);
        assert_eq!(rounded_coverage(0.5, 0.5, 20.0, 16.0, 5.0), 0.0);
        assert_eq!(rounded_coverage(0.5, 0.5, 20.0, 16.0, 0.0), 1.0);
    }

    #[test]
    fn splitmix_is_deterministic() {
        let (mut a, mut b) = (SplitMix64(42), SplitMix64(42));
        let first: Vec<u64> = (0..4).map(|_| a.next_u64()).collect();
        let again: Vec<u64> = (0..4).map(|_| b.next_u64()).collect();
        assert_eq!(first, again);
        assert_eq!(SplitMix64(1).below(0), 0);
    }
}
