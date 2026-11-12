//! The one renderer (ADR 003): base image, then annotations in z-order, on `tiny-skia`. The view
//! and the export both come from here.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use laqta_domain::Bitmap;
use laqta_domain::color::Rgba;
use laqta_domain::geometry::PhysRect;
use tiny_skia::{
    FillRule, FilterQuality, IntSize, LineCap, LineJoin, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform,
};

use crate::fonts::{Fonts, Span};
use crate::fx::{self, Img};
use crate::geom::{PointF, RectF};
use crate::model::{Annotation, AnnotationKind, Style};

/// Rendered pixels of one effect annotation, keyed by everything they depend on.
pub(crate) struct CachedEffect {
    key: u64,
    x: i32,
    y: i32,
    pixmap: Pixmap,
}

/// The per-annotation effect cache (ADR 013), plus counters for tests.
#[derive(Default)]
pub(crate) struct EffectCache {
    entries: HashMap<u64, CachedEffect>,
    blurred_base: Option<(u64, Pixmap)>,
    pub misses: u64,
}

impl EffectCache {
    pub fn retain(&mut self, live: &[u64]) {
        self.entries.retain(|id, _| live.contains(id));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.blurred_base = None;
    }
}

fn color(c: Rgba) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c.r, c.g, c.b, c.a)
}

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color(c));
    p.anti_alias = true;
    p
}

/// Straight RGBA → premultiplied pixmap.
pub(crate) fn img_to_pixmap(img: &Img) -> Option<Pixmap> {
    let mut data = img.px.clone();
    for p in data.as_chunks_mut::<4>().0 {
        let a = p[3] as u16;
        p[0] = ((p[0] as u16 * a + 127) / 255) as u8;
        p[1] = ((p[1] as u16 * a + 127) / 255) as u8;
        p[2] = ((p[2] as u16 * a + 127) / 255) as u8;
    }
    Pixmap::from_vec(data, IntSize::from_wh(img.w.max(1), img.h.max(1))?)
}

/// Premultiplied pixmap → straight RGBA bitmap.
pub(crate) fn pixmap_to_bitmap(p: &Pixmap) -> Bitmap {
    let mut data = p.data().to_vec();
    for px in data.as_chunks_mut::<4>().0 {
        let a = px[3] as u16;
        if a > 0 && a < 255 {
            px[0] = ((px[0] as u16 * 255 + a / 2) / a).min(255) as u8;
            px[1] = ((px[1] as u16 * 255 + a / 2) / a).min(255) as u8;
            px[2] = ((px[2] as u16 * 255 + a / 2) / a).min(255) as u8;
        }
    }
    Bitmap::new(p.width(), p.height(), laqta_domain::PixelFormat::Rgba8, data.into()).unwrap_or_else(|_| fx::empty_bitmap())
}

/// Source-over of glyph spans onto a straight-alpha buffer at (ox, oy).
pub(crate) fn blend_spans(px: &mut [u8], w: u32, h: u32, ox: i32, oy: i32, spans: &[Span]) {
    for &(sx, sy, sw, sh, c) in spans {
        for y in sy..sy + sh as i32 {
            for x in sx..sx + sw as i32 {
                let (x, y) = (x + ox, y + oy);
                if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                    continue;
                }
                let i = ((y as u32 * w + x as u32) * 4) as usize;
                let sa = c.a as f32 / 255.0;
                let da = px[i + 3] as f32 / 255.0;
                let oa = sa + da * (1.0 - sa);
                for (k, sc) in [c.r, c.g, c.b].into_iter().enumerate() {
                    let v = if oa > 0.0 { (sc as f32 * sa + px[i + k] as f32 * da * (1.0 - sa)) / oa } else { 0.0 };
                    px[i + k] = v.round() as u8;
                }
                px[i + 3] = (oa * 255.0).round() as u8;
            }
        }
    }
}

fn rect(r: RectF) -> Option<Rect> {
    let r = r.normalized();
    Rect::from_xywh(r.x, r.y, r.w.max(0.5), r.h.max(0.5))
}

fn rounded_rect_path(r: RectF, radius: f32) -> Option<tiny_skia::Path> {
    let r = r.normalized();
    let rad = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    if rad <= 0.0 {
        return Some(PathBuilder::from_rect(rect(r)?));
    }
    let mut pb = PathBuilder::new();
    pb.move_to(r.x + rad, r.y);
    pb.line_to(r.right() - rad, r.y);
    pb.quad_to(r.right(), r.y, r.right(), r.y + rad);
    pb.line_to(r.right(), r.bottom() - rad);
    pb.quad_to(r.right(), r.bottom(), r.right() - rad, r.bottom());
    pb.line_to(r.x + rad, r.bottom());
    pb.quad_to(r.x, r.bottom(), r.x, r.bottom() - rad);
    pb.line_to(r.x, r.y + rad);
    pb.quad_to(r.x, r.y, r.x + rad, r.y);
    pb.close();
    pb.finish()
}

fn oval_path(r: RectF) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    pb.push_oval(rect(r)?);
    pb.finish()
}

fn stroke(width: f32) -> Stroke {
    Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() }
}

/// Draws a filled+stroked shape with an optional shadow.
fn shape(pm: &mut Pixmap, path: &tiny_skia::Path, style: &Style, t: Transform) {
    if style.shadow {
        let st = t.pre_translate(style.shadow_offset.0, style.shadow_offset.1);
        if style.fill_color.a > 0 {
            pm.fill_path(path, &paint(style.shadow_color), FillRule::Winding, st, None);
        }
        if style.border_size > 0.0 {
            pm.stroke_path(path, &paint(style.shadow_color), &stroke(style.border_size), st, None);
        }
    }
    if style.fill_color.a > 0 {
        pm.fill_path(path, &paint(style.fill_color), FillRule::Winding, t, None);
    }
    if style.border_size > 0.0 && style.border_color.a > 0 {
        pm.stroke_path(path, &paint(style.border_color), &stroke(style.border_size), t, None);
    }
}

/// A path through `points`; a single point becomes a dot.
fn polyline(points: &[PointF]) -> Option<tiny_skia::Path> {
    let (first, rest) = points.split_first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.x, first.y);
    if rest.is_empty() {
        pb.line_to(first.x + 0.01, first.y);
    }
    for p in rest {
        pb.line_to(p.x, p.y);
    }
    pb.finish()
}

/// How opaque the Highlight marker is.
const MARKER_ALPHA: u8 = 115;

fn line_path(start: PointF, end: PointF, curve: Option<PointF>) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(start.x, start.y);
    match curve {
        Some(c) => pb.quad_to(c.x, c.y, end.x, end.y),
        None => pb.line_to(end.x, end.y),
    }
    pb.finish()
}

fn arrow_head(tip: PointF, from: PointF, width: f32) -> Option<tiny_skia::Path> {
    let (dx, dy) = (tip.x - from.x, tip.y - from.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.001 {
        return None;
    }
    let (ux, uy) = (dx / len, dy / len);
    let l = width * 1.2;
    let base = PointF::new(tip.x - ux * l, tip.y - uy * l);
    let (px, py) = (-uy * width / 2.0, ux * width / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(tip.x, tip.y);
    pb.line_to(base.x + px, base.y + py);
    pb.line_to(base.x - px, base.y - py);
    pb.close();
    pb.finish()
}

fn draw_text(pm: &mut Pixmap, fonts: &Fonts, text: &str, style: &Style, area: RectF, t: Transform) {
    let pad = style.font.padding;
    let (spans, _, _) = fonts.layout(text, &style.font, Some((area.w - 2.0 * pad).max(1.0)));
    let ox = area.x + pad;
    let oy = area.y + pad;
    for (sx, sy, sw, sh, c) in spans {
        if let Some(r) = Rect::from_xywh(ox + sx as f32, oy + sy as f32, sw as f32, sh as f32) {
            let mut p = paint(c);
            p.anti_alias = false;
            pm.fill_rect(r, &p, t, None);
        }
    }
}

fn draw_bitmap(pm: &mut Pixmap, b: &Bitmap, dst: RectF, t: Transform) {
    let Some(src) = img_to_pixmap(&Img::from_bitmap(b)) else { return };
    let dst = dst.normalized();
    let sx = if b.width() > 0 { dst.w / b.width() as f32 } else { 1.0 };
    let sy = if b.height() > 0 { dst.h / b.height() as f32 } else { 1.0 };
    let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
    pm.draw_pixmap(0, 0, src.as_ref(), &paint, t.pre_translate(dst.x, dst.y).pre_scale(sx, sy), None);
}

fn key_of(a: &Annotation, base_generation: u64) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}|{:?}", a.kind, a.style).hash(&mut h);
    base_generation.hash(&mut h);
    h.finish()
}

/// The pixels of an effect annotation, computed from the base under it.
fn effect_pixels(a: &Annotation, base: &Img) -> Option<(i32, i32, Pixmap)> {
    let canvas = PhysRect { x: 0, y: 0, width: base.w, height: base.h };
    let r = a.kind.bounds().to_phys();
    let area = crate::geom::intersect(r, canvas)?;
    let img = match &a.kind {
        AnnotationKind::Blur { .. } => {
            let m = a.style.blur_radius as i32 * 2;
            let outer = PhysRect { x: area.x - m, y: area.y - m, width: area.width + 2 * m as u32, height: area.height + 2 * m as u32 };
            let blurred = fx::blur(&base.crop(outer), a.style.blur_radius);
            blurred.crop(PhysRect { x: m, y: m, width: area.width, height: area.height })
        }
        AnnotationKind::Pixelate { .. } => fx::pixelate(&base.crop(area), a.style.pixel_size),
        AnnotationKind::SmartEraser { color, .. } => fx::solid(area.width, area.height, *color),
        AnnotationKind::Magnify { rect } => {
            let r = rect.normalized();
            let zoom = a.style.magnify_zoom.max(1.0);
            let (sw, sh) = ((r.w / zoom).max(1.0), (r.h / zoom).max(1.0));
            let c = r.center();
            let src = RectF::new(c.x - sw / 2.0, c.y - sh / 2.0, sw, sh).to_phys();
            let full = fx::scale_nearest(&base.crop(src), r.w.round().max(1.0) as u32, r.h.round().max(1.0) as u32);
            full.crop(PhysRect { x: area.x - r.x.round() as i32, y: area.y - r.y.round() as i32, width: area.width, height: area.height })
        }
        AnnotationKind::CloneStamp { src, dst } => {
            let s = src.to_phys();
            let full = base.crop(s);
            let (dx, dy) = (dst.x.round() as i32, dst.y.round() as i32);
            full.crop(PhysRect { x: area.x - dx, y: area.y - dy, width: area.width, height: area.height })
        }
        _ => return None,
    };
    Some((area.x, area.y, img_to_pixmap(&img)?))
}

/// Renders the canvas region `view` (image pixels).
pub(crate) fn render(
    base: &Img,
    annotations: &[&Annotation],
    view: PhysRect,
    fonts: &Fonts,
    cache: &mut EffectCache,
    base_generation: u64,
) -> Bitmap {
    let (w, h) = (view.width.max(1), view.height.max(1));
    let Some(mut pm) = Pixmap::new(w, h) else { return fx::empty_bitmap() };
    if let Some(bp) = img_to_pixmap(&base.crop(view)) {
        pm.draw_pixmap(0, 0, bp.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    let t = Transform::from_translate(-view.x as f32, -view.y as f32);
    for a in annotations {
        draw_annotation(&mut pm, a, base, fonts, cache, base_generation, t);
    }
    pixmap_to_bitmap(&pm)
}

fn draw_annotation(pm: &mut Pixmap, a: &Annotation, base: &Img, fonts: &Fonts, cache: &mut EffectCache, generation: u64, t: Transform) {
    let s = &a.style;
    match &a.kind {
        kind if kind.is_effect() => {
            let key = key_of(a, generation);
            let fresh = cache.entries.get(&a.id).is_none_or(|e| e.key != key);
            if fresh {
                cache.misses += 1;
                match effect_pixels(a, base) {
                    Some((x, y, pixmap)) => {
                        cache.entries.insert(a.id, CachedEffect { key, x, y, pixmap });
                    }
                    None => {
                        cache.entries.remove(&a.id);
                    }
                }
            }
            if let Some(e) = cache.entries.get(&a.id) {
                pm.draw_pixmap(0, 0, e.pixmap.as_ref(), &PixmapPaint::default(), t.pre_translate(e.x as f32, e.y as f32), None);
            }
        }
        AnnotationKind::Rectangle { rect } => {
            if let Some(p) = rounded_rect_path(*rect, s.corner_radius) {
                shape(pm, &p, s, t);
            }
        }
        AnnotationKind::Ellipse { rect } => {
            if let Some(p) = oval_path(*rect) {
                shape(pm, &p, s, t);
            }
        }
        AnnotationKind::Line { start, end, curve } => {
            if let Some(p) = line_path(*start, *end, *curve) {
                let mut ls = s.clone();
                ls.fill_color = Rgba::TRANSPARENT;
                shape(pm, &p, &ls, t);
            }
        }
        AnnotationKind::Arrow { start, end, curve } => {
            let width = s.arrow_head_width.max(s.border_size * 3.0);
            if let Some(p) = line_path(*start, *end, *curve) {
                let mut ls = s.clone();
                ls.fill_color = Rgba::TRANSPARENT;
                shape(pm, &p, &ls, t);
            }
            let mut heads = vec![(*end, curve.unwrap_or(*start))];
            if s.arrow_double {
                heads.push((*start, curve.unwrap_or(*end)));
            }
            for (tip, from) in heads {
                if let Some(h) = arrow_head(tip, from, width) {
                    pm.fill_path(&h, &paint(s.border_color), FillRule::Winding, t, None);
                }
            }
        }
        AnnotationKind::Freehand { points } => {
            if let Some(path) = polyline(points) {
                pm.stroke_path(&path, &paint(s.border_color), &stroke(s.border_size.max(1.0)), t, None);
            }
        }
        // See-through, so the text under it stays readable on light and dark backgrounds alike
        // (multiplying would vanish on dark ones). One path, so it never darkens where it crosses
        // itself.
        AnnotationKind::Highlight { points } => {
            if let Some(path) = polyline(points) {
                let ink = Rgba { a: MARKER_ALPHA, ..s.highlight_color };
                pm.stroke_path(&path, &paint(ink), &stroke(s.highlight_size.max(1.0)), t, None);
            }
        }
        AnnotationKind::Text { rect, text } => {
            if let Some(p) = rounded_rect_path(*rect, s.corner_radius) {
                shape(pm, &p, s, t);
            }
            draw_text(pm, fonts, text, s, rect.normalized(), t);
        }
        AnnotationKind::SpeechBalloon { rect, tail, text } => {
            let r = rect.normalized();
            let mut pb = PathBuilder::new();
            if let Some(body) = rounded_rect_path(r, s.corner_radius.max(8.0)) {
                pb.push_path(&body);
            }
            let c = r.center();
            let half = (r.w.min(r.h) / 6.0).max(4.0);
            let (bx, by) = if (tail.y - c.y).abs() * r.w > (tail.x - c.x).abs() * r.h {
                (c.x, if tail.y > c.y { r.bottom() - 1.0 } else { r.y + 1.0 })
            } else {
                (if tail.x > c.x { r.right() - 1.0 } else { r.x + 1.0 }, c.y)
            };
            let vertical = bx == c.x;
            pb.move_to(if vertical { bx - half } else { bx }, if vertical { by } else { by - half });
            pb.line_to(tail.x, tail.y);
            pb.line_to(if vertical { bx + half } else { bx }, if vertical { by } else { by + half });
            pb.close();
            if let Some(p) = pb.finish() {
                let mut fill = s.clone();
                if fill.fill_color.a == 0 {
                    fill.fill_color = Rgba::WHITE;
                }
                shape(pm, &p, &fill, t);
            }
            draw_text(pm, fonts, text, s, r, t);
        }
        AnnotationKind::Step { center, number, radius } => {
            if let Some(p) = PathBuilder::from_circle(center.x, center.y, radius.max(1.0)) {
                let mut st = s.clone();
                if st.fill_color.a == 0 {
                    st.fill_color = s.border_color;
                }
                st.border_size = 0.0;
                shape(pm, &p, &st, t);
            }
            let mut font = s.font.clone();
            font.size = radius * 1.1;
            font.padding = 0.0;
            let label = number.to_string();
            let (spans, tw, th) = fonts.layout(&label, &font, None);
            let (ox, oy) = (center.x - tw / 2.0, center.y - th / 2.0);
            for (sx, sy, sw, sh, c) in spans {
                if let Some(r) = Rect::from_xywh(ox + sx as f32, oy + sy as f32, sw as f32, sh as f32) {
                    pm.fill_rect(r, &paint(c), t, None);
                }
            }
        }
        AnnotationKind::Image { rect, bitmap } | AnnotationKind::Emoji { rect, bitmap } => draw_bitmap(pm, &bitmap.0, *rect, t),
        AnnotationKind::Cursor { pos, bitmap } => {
            draw_bitmap(pm, &bitmap.0, RectF::new(pos.x, pos.y, bitmap.0.width() as f32, bitmap.0.height() as f32), t);
        }
        AnnotationKind::Spotlight { rect } => spotlight(pm, a, *rect, base, cache, generation, t),
        _ => {}
    }
}

/// Dims (and optionally blurs) everything outside the spot.
fn spotlight(pm: &mut Pixmap, a: &Annotation, spot: RectF, base: &Img, cache: &mut EffectCache, generation: u64, t: Transform) {
    let s = &a.style;
    let full = RectF::new(0.0, 0.0, base.w as f32, base.h as f32);
    let mut pb = PathBuilder::new();
    if let Some(r) = rect(full) {
        pb.push_rect(r);
    }
    let spot_path = if s.spotlight_ellipse { oval_path(spot) } else { rounded_rect_path(spot, s.corner_radius) };
    if let Some(sp) = &spot_path {
        pb.push_path(sp);
    }
    let Some(outside) = pb.finish() else { return };
    if s.spotlight_blur > 0 {
        let key = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (generation, s.spotlight_blur).hash(&mut h);
            h.finish()
        };
        if cache.blurred_base.as_ref().is_none_or(|(k, _)| *k != key) {
            cache.misses += 1;
            cache.blurred_base = img_to_pixmap(&fx::blur(base, s.spotlight_blur)).map(|p| (key, p));
        }
        if let (Some((_, blurred)), Some(mut mask)) = (&cache.blurred_base, Mask::new(pm.width(), pm.height())) {
            mask.fill_path(&outside, FillRule::EvenOdd, true, t);
            pm.draw_pixmap(0, 0, blurred.as_ref(), &PixmapPaint::default(), t, Some(&mask));
        }
    }
    pm.fill_path(&outside, &paint(Rgba { r: 0, g: 0, b: 0, a: s.spotlight_dim }), FillRule::EvenOdd, t, None);
}
