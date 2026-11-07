//! Compositing the layer stack into a canvas. Index 0 of a layer list is the bottom. Patches are
//! clipped to the canvas, so a small layer costs only its own area.

use laqta_domain::Bitmap;
use laqta_domain::color::Rgba;
use laqta_domain::effects::{Effect, TextSpec};
use laqta_editor::Fonts;
use laqta_imaging::effects::{EffectContext, apply};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::blend::{self, BlendMode};
use crate::layer::{Fill, FrameLayer, Layer, LayerKind, Shape, ShapeKind};
use crate::mask::Mask;
use crate::style;
use crate::surface::{Surface, to_f32, to_u8};
use crate::text;

/// Straight RGBA pixels at a position in canvas coordinates.
#[derive(Clone, Debug)]
pub(crate) struct Patch {
    pub x: i32,
    pub y: i32,
    pub s: Surface,
}

/// Source-over `src` onto `canvas` at `(x, y)`, scaled by `opacity` and the mask.
pub(crate) fn composite_at(canvas: &mut Surface, x: i32, y: i32, src: &Surface, blend_mode: BlendMode, opacity: f32, mask: Option<&Mask>) {
    let (cw, ch) = (canvas.w as i32, canvas.h as i32);
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + src.w as i32).min(cw);
    let y1 = (y + src.h as i32).min(ch);
    let k = opacity.clamp(0.0, 1.0);
    for cy in y0..y1 {
        for cx in x0..x1 {
            let si = ((cy - y) as usize * src.w as usize + (cx - x) as usize) * 4;
            let sp = &src.px[si..si + 4];
            let coverage = mask.map_or(1.0, |m| m.coverage(cx, cy));
            let sa = f32::from(sp[3]) / 255.0 * k * coverage;
            if sa <= 0.0 {
                continue;
            }
            let ci = (cy as usize * canvas.w as usize + cx as usize) * 4;
            let cs = to_f32(sp);
            let out = blend::composite(blend_mode, to_f32(&canvas.px[ci..ci + 4]), [cs[0], cs[1], cs[2], sa]);
            to_u8(out, &mut canvas.px[ci..ci + 4]);
        }
    }
}

/// Runs `f` with an effect context that renders text through `fonts` and loads no assets.
pub(crate) fn with_context<T>(fonts: &Fonts, f: impl FnOnce(&EffectContext<'_>) -> T) -> T {
    let text_fn = |spec: &TextSpec| text::render_spec(fonts, spec);
    let cx = EffectContext { asset: &|_| None, text: &text_fn };
    f(&cx)
}

/// Composites `layers` onto `canvas`, placed at `origin`. Visible layers only.
pub(crate) fn render_list(canvas: &mut Surface, layers: &[Layer], origin: (i32, i32), fonts: &Fonts) {
    let (w, h) = (canvas.w, canvas.h);
    for layer in layers.iter().filter(|l| l.visible) {
        let o = (origin.0 + layer.offset.0, origin.1 + layer.offset.1);
        match &layer.kind {
            LayerKind::Adjustment(effect) => {
                let input = canvas.to_bitmap();
                if let Some(out) = adjust(&input, effect, fonts) {
                    let fitted = fit(&out, w, h);
                    composite_at(canvas, 0, 0, &fitted, layer.blend, layer.opacity, layer.mask.as_ref());
                }
            }
            LayerKind::Group(children) => {
                let mut sub = Surface::new(w, h);
                render_list(&mut sub, children, o, fonts);
                composite_at(canvas, 0, 0, &sub, layer.blend, layer.opacity, layer.mask.as_ref());
            }
            _ => draw_leaf(canvas, layer, o, fonts),
        }
    }
}

/// Applies an adjustment's effect to the canvas so far. A failing effect leaves the canvas as it is.
fn adjust(input: &Bitmap, effect: &Effect, fonts: &Fonts) -> Option<Bitmap> {
    with_context(fonts, |cx| apply(input, effect, cx).ok())
}

/// A pixel copy of `out` in a `w` x `h` surface, cropped or padded with transparency. An effect may
/// change the image size (resize, rotate), and the canvas keeps its own size.
fn fit(out: &Bitmap, w: u32, h: u32) -> Surface {
    let src = Surface::from_bitmap(out);
    if src.w == w && src.h == h {
        return src;
    }
    let mut dst = Surface::new(w, h);
    let rows = h.min(src.h) as usize;
    let cols = w.min(src.w) as usize * 4;
    for y in 0..rows {
        let d = y * w as usize * 4;
        let s = y * src.w as usize * 4;
        dst.px[d..d + cols].copy_from_slice(&src.px[s..s + cols]);
    }
    dst
}

/// A layer with pixels of its own: raster, shape, text, fill, frame or smart object. Its styles
/// make it an isolated group first.
fn draw_leaf(canvas: &mut Surface, layer: &Layer, o: (i32, i32), fonts: &Fonts) {
    let Some(mut content) = content(&layer.kind, o, fonts, canvas.w, canvas.h) else { return };
    if layer.styles.is_empty() {
        composite_at(canvas, content.x, content.y, &content.s, layer.blend, layer.opacity, layer.mask.as_ref());
        return;
    }
    for st in &layer.styles {
        style::tint(&mut content.s, st);
    }
    let mut sub = Surface::new(canvas.w, canvas.h);
    for st in layer.styles.iter().filter(|s| style::is_under(s)) {
        if let Some(p) = style::under(st, &content) {
            composite_at(&mut sub, p.x, p.y, &p.s, BlendMode::Normal, 1.0, None);
        }
    }
    composite_at(&mut sub, content.x, content.y, &content.s, BlendMode::Normal, 1.0, None);
    for st in layer.styles.iter().filter(|s| !style::is_under(s)) {
        if let Some(p) = style::over(st, &content) {
            composite_at(&mut sub, p.x, p.y, &p.s, BlendMode::Normal, 1.0, None);
        }
    }
    composite_at(canvas, 0, 0, &sub, layer.blend, layer.opacity, layer.mask.as_ref());
}

/// The layer's own pixels at its placement `o`, or `None` when it draws nothing.
fn content(kind: &LayerKind, o: (i32, i32), fonts: &Fonts, w: u32, h: u32) -> Option<Patch> {
    match kind {
        LayerKind::Raster(b) => Some(Patch { x: o.0, y: o.1, s: Surface::from_bitmap(b) }),
        LayerKind::Smart { source, effects } => smart(source, effects, fonts).map(|s| Patch { x: o.0, y: o.1, s }),
        LayerKind::Shape(shape) => shape_patch(shape).map(|p| Patch { x: o.0 + p.x, y: o.1 + p.y, s: p.s }),
        LayerKind::Text(t) => text::render(fonts, t).map(|s| Patch { x: o.0 + t.x.round() as i32, y: o.1 + t.y.round() as i32, s }),
        LayerKind::Fill(f) => Some(Patch { x: o.0, y: o.1, s: fill_surface(f, w, h) }),
        LayerKind::Frame(f) => frame_surface(f, w, h).map(|s| Patch { x: o.0, y: o.1, s }),
        LayerKind::Adjustment(_) | LayerKind::Group(_) => None,
    }
}

/// The source with its enabled effects applied in order. A failing effect hides the layer.
fn smart(source: &Bitmap, effects: &[laqta_domain::effects::EffectItem], fonts: &Fonts) -> Option<Surface> {
    with_context(fonts, |cx| {
        let mut cur = source.clone();
        for item in effects.iter().filter(|i| i.enabled) {
            cur = apply(&cur, &item.effect, cx).ok()?;
        }
        Some(Surface::from_bitmap(&cur))
    })
}

fn lerp_rgba(a: Rgba, b: Rgba, t: f32) -> [u8; 4] {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round().clamp(0.0, 255.0) as u8;
    [mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b), mix(a.a, b.a)]
}

/// A full-canvas fill. Gradients are evaluated per pixel at the pixel centre.
fn fill_surface(fill: &Fill, w: u32, h: u32) -> Surface {
    let mut s = Surface::new(w, h);
    let (wf, hf) = (w as f32, h as f32);
    for y in 0..h {
        for x in 0..w {
            let (px, py) = (x as f32 + 0.5 - wf / 2.0, y as f32 + 0.5 - hf / 2.0);
            let c = match *fill {
                Fill::Solid(c) => [c.r, c.g, c.b, c.a],
                Fill::LinearGradient { from, to, angle } => {
                    let th = angle.to_radians();
                    let (dx, dy) = (th.cos(), th.sin());
                    let half = (wf * dx.abs() + hf * dy.abs()) / 2.0;
                    let t = if half > 0.0 { (px * dx + py * dy) / (2.0 * half) + 0.5 } else { 0.0 };
                    lerp_rgba(from, to, t)
                }
                Fill::RadialGradient { inner, outer } => {
                    let radius = (wf * wf + hf * hf).sqrt() / 2.0;
                    let t = if radius > 0.0 { (px * px + py * py).sqrt() / radius } else { 0.0 };
                    lerp_rgba(inner, outer, t)
                }
            };
            let i = (y as usize * w as usize + x as usize) * 4;
            s.px[i..i + 4].copy_from_slice(&c);
        }
    }
    s
}

/// Converts a premultiplied `tiny-skia` pixmap to straight RGBA.
fn unpremultiply(pm: &Pixmap) -> Surface {
    let mut s = Surface::new(pm.width(), pm.height());
    for (d, p) in s.px.as_chunks_mut::<4>().0.iter_mut().zip(pm.data().as_chunks::<4>().0) {
        let a = u32::from(p[3]);
        if a == 0 {
            continue;
        }
        for (dst, src) in d.iter_mut().take(3).zip(p) {
            *dst = ((u32::from(*src) * 255 + a / 2) / a).min(255) as u8;
        }
        d[3] = p[3];
    }
    s
}

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c.r, c.g, c.b, c.a);
    p
}

/// The shape's outline in layer-local coordinates. `None` for degenerate geometry.
fn outline(kind: &ShapeKind) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    match kind {
        ShapeKind::Rect { x, y, width, height } => {
            let r = tiny_skia::Rect::from_ltrb(*x, *y, x + width, y + height)?;
            pb.push_rect(r);
        }
        ShapeKind::Ellipse { x, y, width, height } => {
            let r = tiny_skia::Rect::from_ltrb(*x, *y, x + width, y + height)?;
            return PathBuilder::from_oval(r);
        }
        ShapeKind::Line { from, to } => {
            pb.move_to(from.0, from.1);
            pb.line_to(to.0, to.1);
        }
        ShapeKind::Polygon(points) => {
            let (first, rest) = points.split_first()?;
            pb.move_to(first.0, first.1);
            for p in rest {
                pb.line_to(p.0, p.1);
            }
            pb.close();
        }
    }
    pb.finish()
}

/// Rasterises a shape. The returned patch is positioned in layer-local coordinates.
fn shape_patch(shape: &Shape) -> Option<Patch> {
    let path = outline(&shape.kind)?;
    let bounds = path.bounds();
    let stroke_w = shape.stroke.map_or(0.0, |(_, w)| w);
    let half = stroke_w / 2.0 + 1.0;
    let ox = (bounds.left() - half).floor();
    let oy = (bounds.top() - half).floor();
    let w = (bounds.right() + half - ox).ceil().max(1.0) as u32;
    let h = (bounds.bottom() + half - oy).ceil().max(1.0) as u32;
    let mut pm = Pixmap::new(w, h)?;
    let t = Transform::from_translate(-ox, -oy);
    if let Some(fill) = shape.fill
        && !matches!(shape.kind, ShapeKind::Line { .. })
    {
        pm.fill_path(&path, &paint(fill), FillRule::Winding, t, None);
    }
    if let Some((color, width)) = shape.stroke {
        let stroke = Stroke { width, ..Stroke::default() };
        pm.stroke_path(&path, &paint(color), &stroke, t, None);
    }
    Some(Patch { x: ox as i32, y: oy as i32, s: unpremultiply(&pm) })
}

/// Appends a rounded rectangle to `pb`, with corners drawn as cubic curves.
fn push_rounded_rect(pb: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, radius: f32) {
    let r = radius.clamp(0.0, w.min(h).max(0.0) / 2.0);
    let k = 0.552_284_8 * r;
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
}

/// A border around the canvas: the outer rounded rectangle minus the inner one.
fn frame_surface(frame: &FrameLayer, w: u32, h: u32) -> Option<Surface> {
    let (wf, hf) = (w as f32, h as f32);
    let bw = frame.width.max(0.0);
    let mut pb = PathBuilder::new();
    push_rounded_rect(&mut pb, 0.0, 0.0, wf, hf, frame.radius);
    let (iw, ih) = (wf - 2.0 * bw, hf - 2.0 * bw);
    if iw > 0.0 && ih > 0.0 {
        push_rounded_rect(&mut pb, bw, bw, iw, ih, frame.radius - bw);
    }
    let path = pb.finish()?;
    let mut pm = Pixmap::new(w, h)?;
    pm.fill_path(&path, &paint(frame.color), FillRule::EvenOdd, Transform::identity(), None);
    Some(unpremultiply(&pm))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_crops_and_pads() {
        let big = Surface::new(4, 4).into_bitmap();
        assert_eq!(fit(&big, 2, 3).px.len(), 2 * 3 * 4);
        let small = Surface::new(2, 2).into_bitmap();
        assert_eq!(fit(&small, 3, 3).w, 3);
    }
}
