//! Layer styles. Shadows and glows draw under the layer's content, strokes and inner shadows over it,
//! and a colour overlay tints the content. A styled layer composites as an isolated group, so its
//! blend mode, opacity and mask apply to the styles too.

use laqta_domain::color::Rgba;

use crate::mask::Mask;
use crate::render::Patch;
use crate::surface::Surface;

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Shadow {
    pub color: Rgba,
    /// 0..=1.
    pub opacity: f32,
    /// Shift in pixels, `(dx, dy)`.
    pub offset: (i32, i32),
    /// Gaussian sigma in pixels. 0 gives a hard edge.
    pub blur: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StrokePosition {
    /// Outside the shape's outline.
    Outside,
    /// Inside the shape's outline.
    Inside,
    /// Centred on the outline.
    Center,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LayerStyle {
    DropShadow(Shadow),
    InnerShadow(Shadow),
    /// A blurred halo outside the shape. The shadow's offset is ignored.
    OuterGlow { color: Rgba, opacity: f32, blur: f32 },
    /// A ring of `width` pixels. The ring follows the 50% alpha contour, so its edge is hard.
    Stroke { color: Rgba, width: f32, position: StrokePosition },
    /// Replaces the content colour with `color` at `opacity`, keeping its alpha.
    ColorOverlay { color: Rgba, opacity: f32 },
}

/// Styles drawn beneath the content.
pub(crate) fn is_under(style: &LayerStyle) -> bool {
    matches!(style, LayerStyle::DropShadow(_) | LayerStyle::OuterGlow { .. })
}

/// The layer below-content patch for a style, or `None` when the style draws nothing.
pub(crate) fn under(style: &LayerStyle, content: &Patch) -> Option<Patch> {
    match *style {
        LayerStyle::DropShadow(s) => shadow(content, s),
        LayerStyle::OuterGlow { color, opacity, blur } => {
            shadow(content, Shadow { color, opacity, offset: (0, 0), blur })
        }
        _ => None,
    }
}

/// The over-content patch for a style, or `None`.
pub(crate) fn over(style: &LayerStyle, content: &Patch) -> Option<Patch> {
    match *style {
        LayerStyle::InnerShadow(s) => inner_shadow(content, s),
        LayerStyle::Stroke { color, width, position } => stroke(content, color, width, position),
        _ => None,
    }
}

/// Applies a colour overlay to the content in place.
pub(crate) fn tint(content: &mut Surface, style: &LayerStyle) {
    let LayerStyle::ColorOverlay { color, opacity } = *style else { return };
    let k = opacity.clamp(0.0, 1.0);
    let target = [f32::from(color.r), f32::from(color.g), f32::from(color.b)];
    for p in content.px.as_chunks_mut::<4>().0.iter_mut().filter(|p| p[3] > 0) {
        for (c, t) in p.iter_mut().take(3).zip(target) {
            let v = f32::from(*c) + (t - f32::from(*c)) * k;
            *c = v.round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// The content's alpha as a mask of the content's size.
fn alpha_mask(s: &Surface) -> Mask {
    let data: Vec<u8> = s.px.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
    // The length is `w * h` by construction.
    Mask::from_vec(s.w, s.h, data).expect("alpha has one byte per pixel")
}

/// Writes `coverage` (0..=255 per pixel of `mask`) as colour `color` at `opacity`.
fn paint_coverage(mask: &Mask, color: Rgba, opacity: f32) -> Surface {
    let mut out = Surface::new(mask.width(), mask.height());
    let k = opacity.clamp(0.0, 1.0) * f32::from(color.a) / 255.0;
    for (p, &cov) in out.px.as_chunks_mut::<4>().0.iter_mut().zip(mask.data()) {
        let a = f32::from(cov) / 255.0 * k;
        p.copy_from_slice(&[color.r, color.g, color.b, (a * 255.0).round() as u8]);
    }
    out
}

/// A copy of `alpha` (of `src`'s size) moved by `(dx, dy)` into a buffer with `margin` on each side.
fn padded_alpha(alpha: &Mask, margin: i32, dx: i32, dy: i32) -> Mask {
    let (w, h) = (alpha.width() as i32 + 2 * margin, alpha.height() as i32 + 2 * margin);
    let mut data = vec![0u8; (w * h) as usize];
    for y in 0..alpha.height() as i32 {
        for x in 0..alpha.width() as i32 {
            let (tx, ty) = (x + margin + dx, y + margin + dy);
            data[(ty * w + tx) as usize] = alpha.get(x, y);
        }
    }
    // The length is `w * h` by construction.
    Mask::from_vec(w as u32, h as u32, data).expect("padded alpha matches its size")
}

fn shadow(content: &Patch, s: Shadow) -> Option<Patch> {
    let alpha = alpha_mask(&content.s);
    let margin = (3.0 * s.blur).ceil() as i32 + s.offset.0.abs() + s.offset.1.abs();
    let moved = padded_alpha(&alpha, margin, s.offset.0, s.offset.1).blur(s.blur);
    Some(Patch {
        x: content.x - margin,
        y: content.y - margin,
        s: paint_coverage(&moved, s.color, s.opacity),
    })
}

fn inner_shadow(content: &Patch, s: Shadow) -> Option<Patch> {
    let (w, h) = (content.s.w as i32, content.s.h as i32);
    let alpha = alpha_mask(&content.s);
    // The inverse of the content shifted by the offset: where the shape is not, the shadow falls.
    let mut inverse = vec![255u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let v = alpha.get(x - s.offset.0, y - s.offset.1);
            inverse[(y * w + x) as usize] = 255 - v;
        }
    }
    let shade = Mask::from_vec(w as u32, h as u32, inverse).ok()?.blur(s.blur);
    // Only inside the shape: the shadow is multiplied by the content's own alpha.
    let k = s.opacity.clamp(0.0, 1.0) * f32::from(s.color.a) / 255.0;
    let mut out = Surface::new(content.s.w, content.s.h);
    for (i, p) in out.px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let inside = f32::from(alpha.data()[i]) / 255.0;
        let a = f32::from(shade.data()[i]) / 255.0 * inside * k;
        p.copy_from_slice(&[s.color.r, s.color.g, s.color.b, (a * 255.0).round() as u8]);
    }
    Some(Patch { x: content.x, y: content.y, s: out })
}

fn stroke(content: &Patch, color: Rgba, width: f32, position: StrokePosition) -> Option<Patch> {
    let r = match position {
        StrokePosition::Outside | StrokePosition::Inside => width,
        StrokePosition::Center => width / 2.0,
    }
    .round()
    .max(0.0) as u32;
    let margin = if position == StrokePosition::Inside { 0 } else { r as i32 };
    let shape = alpha_mask(&content.s);
    let binary = {
        let data: Vec<u8> = shape.data().iter().map(|&a| if a >= 128 { 255 } else { 0 }).collect();
        Mask::from_vec(shape.width(), shape.height(), data).ok()?
    };
    let padded = padded_alpha(&binary, margin, 0, 0);
    let ring = match position {
        StrokePosition::Outside => and_not(&padded.dilate(r), &padded),
        StrokePosition::Inside => and_not(&padded, &padded.erode(r)),
        StrokePosition::Center => and_not(&padded.dilate(r), &padded.erode(r)),
    };
    Some(Patch { x: content.x - margin, y: content.y - margin, s: paint_coverage(&ring, color, 1.0) })
}

/// `a` with the pixels of `b` removed.
fn and_not(a: &Mask, b: &Mask) -> Mask {
    let data = a.data().iter().zip(b.data()).map(|(&x, &y)| if y > 0 { 0 } else { x }).collect();
    Mask::from_vec(a.width(), a.height(), data).unwrap_or_else(|_| Mask::filled(a.width(), a.height(), 0))
}
