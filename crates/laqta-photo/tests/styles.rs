#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]
//! Layer styles (drop and inner shadow, outer glow, stroke, colour overlay), shapes, fills, frames,
//! and text with no fonts.

mod support;

use laqta_domain::color::Rgba;
use laqta_photo::{
    Command, Document, Fill, FrameLayer, LayerId, LayerKind, LayerStyle, PhotoEditor, Shadow, Shape, ShapeKind,
    StrokePosition, TextLayer,
};

use support::{px, solid};

const RED: Rgba = Rgba { r: 255, g: 0, b: 0, a: 255 };
const GREEN: Rgba = Rgba { r: 0, g: 255, b: 0, a: 255 };
const BLUE: Rgba = Rgba { r: 0, g: 0, b: 255, a: 255 };
const BLACK: Rgba = Rgba::BLACK;

/// A transparent `w` x `h` canvas with one layer of `kind` at `offset`, returned with its id.
fn layer(w: u32, h: u32, kind: LayerKind, offset: (i32, i32)) -> (PhotoEditor, LayerId) {
    let mut ed = PhotoEditor::new(Document::new(w, h).unwrap());
    ed.execute(Command::AddLayer { name: "l".into(), kind }).unwrap();
    let id = ed.document().active.unwrap();
    ed.execute(Command::SetOffset { layer: id, offset }).unwrap();
    (ed, id)
}

fn red_square(size: u32) -> LayerKind {
    LayerKind::Raster(solid(size, size, [255, 0, 0, 255]))
}

fn style(ed: &mut PhotoEditor, id: LayerId, styles: Vec<LayerStyle>) {
    ed.execute(Command::SetStyles { layer: id, styles }).unwrap();
}

#[test]
fn drop_shadow_falls_under_the_layer() {
    // Red 4x4 at (5, 5), shadow shifted by (2, 2): the shadow covers [7, 11) and the content [5, 9).
    let (mut ed, id) = layer(20, 20, red_square(4), (5, 5));
    style(&mut ed, id, vec![LayerStyle::DropShadow(Shadow { color: BLUE, opacity: 1.0, offset: (2, 2), blur: 0.0 })]);
    let out = ed.render();
    assert_eq!(px(&out, 6, 6), [255, 0, 0, 255], "the content is on top");
    assert_eq!(px(&out, 10, 10), [0, 0, 255, 255], "the shadow shows past the content");
    assert_eq!(px(&out, 11, 11)[3], 0, "and not beyond it");
}

#[test]
fn outer_stroke_surrounds_the_shape() {
    let (mut ed, id) = layer(20, 20, red_square(4), (8, 8));
    style(&mut ed, id, vec![LayerStyle::Stroke { color: GREEN, width: 2.0, position: StrokePosition::Outside }]);
    let out = ed.render();
    assert_eq!(px(&out, 6, 8), [0, 255, 0, 255], "two pixels outside the left edge");
    assert_eq!(px(&out, 7, 8), [0, 255, 0, 255]);
    assert_eq!(px(&out, 5, 8)[3], 0, "beyond the stroke");
    assert_eq!(px(&out, 9, 9), [255, 0, 0, 255], "the content keeps its colour");
}

#[test]
fn inside_stroke_is_on_the_edge() {
    let (mut ed, id) = layer(20, 20, red_square(4), (8, 8));
    style(&mut ed, id, vec![LayerStyle::Stroke { color: GREEN, width: 1.0, position: StrokePosition::Inside }]);
    let out = ed.render();
    assert_eq!(px(&out, 8, 8), [0, 255, 0, 255], "the corner is on the edge");
    assert_eq!(px(&out, 9, 9), [255, 0, 0, 255], "the centre is not");
}

#[test]
fn inner_shadow_darkens_the_top_left_inside_edge() {
    // Shifted by (2, 2), the shape leaves a gap at the top left, and the shadow falls into it.
    let (mut ed, id) = layer(12, 12, red_square(6), (3, 3));
    style(&mut ed, id, vec![LayerStyle::InnerShadow(Shadow { color: BLACK, opacity: 1.0, offset: (2, 2), blur: 0.0 })]);
    let out = ed.render();
    assert_eq!(px(&out, 3, 3), [0, 0, 0, 255], "top-left corner is in shadow");
    assert_eq!(px(&out, 8, 8), [255, 0, 0, 255], "bottom-right is not");
}

#[test]
fn centre_stroke_straddles_the_edge() {
    // Width 2 centred on the edge of a 4x4 square: one pixel out, one in, and the middle untouched.
    let (mut ed, id) = layer(20, 20, red_square(4), (8, 8));
    style(&mut ed, id, vec![LayerStyle::Stroke { color: GREEN, width: 2.0, position: StrokePosition::Center }]);
    let out = ed.render();
    assert_eq!(px(&out, 7, 8), [0, 255, 0, 255], "just outside the edge");
    assert_eq!(px(&out, 8, 8), [0, 255, 0, 255], "just inside the edge");
    assert_eq!(px(&out, 9, 9), [255, 0, 0, 255], "the middle keeps the content colour");
}

#[test]
fn outer_glow_spreads_past_the_edge() {
    let (mut ed, id) = layer(16, 16, red_square(4), (6, 6));
    style(&mut ed, id, vec![LayerStyle::OuterGlow { color: GREEN, opacity: 1.0, blur: 1.5 }]);
    let out = ed.render();
    assert!(px(&out, 5, 7)[3] > 0, "just outside the edge is lit");
    assert_eq!(px(&out, 5, 7)[1], 255, "in the glow colour");
    assert_eq!(px(&out, 0, 0)[3], 0, "far away is untouched");
}

#[test]
fn colour_overlay_replaces_the_colour_and_keeps_the_alpha() {
    let (mut ed, id) = layer(8, 8, red_square(4), (0, 0));
    style(&mut ed, id, vec![LayerStyle::ColorOverlay { color: BLUE, opacity: 1.0 }]);
    let out = ed.render();
    assert_eq!(px(&out, 1, 1), [0, 0, 255, 255]);
    assert_eq!(px(&out, 6, 6)[3], 0);
}

#[test]
fn a_rectangle_shape_fills_its_bounds() {
    let shape = Shape { kind: ShapeKind::Rect { x: 2.0, y: 2.0, width: 4.0, height: 4.0 }, fill: Some(RED), stroke: None };
    let (ed, _) = layer(10, 10, LayerKind::Shape(shape), (0, 0));
    let out = ed.render();
    assert_eq!(px(&out, 3, 3), [255, 0, 0, 255]);
    assert_eq!(px(&out, 0, 0)[3], 0);
    assert_eq!(px(&out, 6, 6)[3], 0);
}

#[test]
fn an_ellipse_shape_fills_its_middle() {
    let shape = Shape { kind: ShapeKind::Ellipse { x: 0.0, y: 0.0, width: 10.0, height: 10.0 }, fill: Some(BLUE), stroke: None };
    let (ed, _) = layer(10, 10, LayerKind::Shape(shape), (0, 0));
    let out = ed.render();
    assert_eq!(px(&out, 5, 5), [0, 0, 255, 255]);
    assert_eq!(px(&out, 0, 0)[3], 0);
}

#[test]
fn a_line_is_stroked_and_not_filled() {
    let shape = Shape {
        kind: ShapeKind::Line { from: (1.0, 4.0), to: (8.0, 4.0) },
        fill: Some(RED),
        stroke: Some((GREEN, 2.0)),
    };
    let (ed, _) = layer(10, 10, LayerKind::Shape(shape), (0, 0));
    let out = ed.render();
    assert_eq!(px(&out, 4, 4), [0, 255, 0, 255], "stroked");
    assert_eq!(px(&out, 4, 8)[3], 0);
}

#[test]
fn solid_and_gradient_fills_cover_the_canvas() {
    let (ed, _) = layer(8, 2, LayerKind::Fill(Fill::Solid(BLUE)), (0, 0));
    assert_eq!(px(&ed.render(), 7, 1), [0, 0, 255, 255]);

    let grad = Fill::LinearGradient { from: BLACK, to: Rgba::WHITE, angle: 0.0 };
    let (ed, _) = layer(8, 2, LayerKind::Fill(grad), (0, 0));
    let out = ed.render();
    let (left, right) = (px(&out, 0, 0)[0], px(&out, 7, 0)[0]);
    // The gradient spans the canvas, so the outer pixel centres sit a sixteenth in from each end.
    assert!(left <= 20 && right >= 235, "from black to white: {left} .. {right}");
}

#[test]
fn a_frame_is_a_border_with_an_open_middle() {
    let frame = FrameLayer { width: 2.0, color: RED, radius: 0.0 };
    let (ed, _) = layer(10, 10, LayerKind::Frame(frame), (0, 0));
    let out = ed.render();
    assert_eq!(px(&out, 0, 5), [255, 0, 0, 255], "on the border");
    assert_eq!(px(&out, 5, 5)[3], 0, "the middle is open");
}

#[test]
fn text_without_fonts_draws_nothing_and_does_not_panic() {
    let text = TextLayer {
        text: "hello".into(),
        font_family: "sans-serif".into(),
        size: 12.0,
        color: BLACK,
        bold: false,
        italic: false,
        x: 1.0,
        y: 1.0,
    };
    let (ed, _) = layer(10, 10, LayerKind::Text(text), (0, 0));
    assert!(ed.render().pixels().as_chunks::<4>().0.iter().all(|p| p[3] == 0));
}
