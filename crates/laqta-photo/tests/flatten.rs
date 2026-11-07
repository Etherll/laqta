#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]
//! Flatten snapshots of composed documents: a blend stack, styled layers, a painted selection with an
//! adjustment layer, and a smart object in a group. The scenes build inside the compute pool, since
//! the adjustment and smart layers run imaging kernels. Each PNG is written on the first run and
//! compared after; `LAQTA_BLESS=1` rewrites them.

mod support;

use laqta_domain::Bitmap;
use laqta_domain::color::Rgba;
use laqta_domain::effects::{Adjust, Effect, EffectItem};
use laqta_photo::{
    BlendMode, Brush, BrushTool, Command, Document, Fill, FrameLayer, Layer, LayerId, LayerKind, LayerStyle, Mask, PhotoEditor, SelectOp,
    SelectShape, Shadow, Shape, ShapeKind, StrokePosition,
};

use support::{from_fn, in_pool, snapshot, solid};

const W: u32 = 64;
const H: u32 = 48;

fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    Rgba { r, g, b, a: 255 }
}

fn editor() -> PhotoEditor {
    PhotoEditor::new(Document::new(W, H).unwrap())
}

/// Adds a layer on top, moves it to `offset`, and returns its id.
fn add(ed: &mut PhotoEditor, name: &str, kind: LayerKind, offset: (i32, i32)) -> LayerId {
    ed.execute(Command::AddLayer { name: name.into(), kind }).unwrap();
    let id = ed.document().active.unwrap();
    ed.execute(Command::SetOffset { layer: id, offset }).unwrap();
    id
}

fn ellipse(w: f32, h: f32, fill: Rgba) -> LayerKind {
    LayerKind::Shape(Shape { kind: ShapeKind::Ellipse { x: 0.0, y: 0.0, width: w, height: h }, fill: Some(fill), stroke: None })
}

fn rect(w: f32, h: f32, fill: Rgba) -> LayerKind {
    LayerKind::Shape(Shape { kind: ShapeKind::Rect { x: 0.0, y: 0.0, width: w, height: h }, fill: Some(fill), stroke: None })
}

fn blend_stack() -> Bitmap {
    let mut ed = editor();
    add(&mut ed, "gradient", LayerKind::Fill(Fill::LinearGradient { from: rgb(30, 60, 120), to: rgb(240, 200, 80), angle: 45.0 }), (0, 0));
    let checker = from_fn(40, 30, |x, y| if (x / 5 + y / 5) % 2 == 0 { [220, 40, 40, 255] } else { [250, 250, 250, 255] });
    add(&mut ed, "checker", LayerKind::Raster(checker), (10, 8));
    let sun = add(&mut ed, "sun", ellipse(30.0, 22.0, rgb(255, 160, 0)), (28, 12));
    ed.execute(Command::SetBlend { layer: sun, blend: BlendMode::Screen }).unwrap();
    ed.execute(Command::SetOpacity { layer: sun, opacity: 0.8 }).unwrap();
    let band = add(&mut ed, "band", rect(24.0, 40.0, rgb(40, 90, 255)), (46, 4));
    ed.execute(Command::SetBlend { layer: band, blend: BlendMode::HardLight }).unwrap();
    // A group that composites in isolation, with its left half hidden by a mask.
    let dot = Layer::new(LayerId(0), "dot", ellipse(20.0, 20.0, rgb(255, 0, 200)));
    ed.execute(Command::AddLayer { name: "group".into(), kind: LayerKind::Group(vec![dot]) }).unwrap();
    let group = ed.document().active.unwrap();
    ed.execute(Command::SetOffset { layer: group, offset: (4, 22) }).unwrap();
    ed.execute(Command::SetBlend { layer: group, blend: BlendMode::Difference }).unwrap();
    ed.execute(Command::SetOpacity { layer: group, opacity: 0.7 }).unwrap();
    let mask = Mask::from_vec(W, H, (0..W * H).map(|i| if i % W < 32 { 0 } else { 255 }).collect()).unwrap();
    ed.execute(Command::SetMask { layer: group, mask: Some(mask) }).unwrap();
    ed.flatten()
}

fn styled_layers() -> Bitmap {
    let mut ed = editor();
    add(&mut ed, "paper", LayerKind::Fill(Fill::Solid(rgb(240, 240, 240))), (0, 0));
    let square = add(&mut ed, "square", LayerKind::Raster(solid(16, 16, [200, 30, 30, 255])), (8, 8));
    ed.execute(Command::SetStyles {
        layer: square,
        styles: vec![
            LayerStyle::DropShadow(Shadow { color: rgb(0, 0, 0), opacity: 0.6, offset: (3, 3), blur: 2.0 }),
            LayerStyle::Stroke { color: rgb(20, 160, 60), width: 2.0, position: StrokePosition::Outside },
        ],
    })
    .unwrap();
    let disc = add(&mut ed, "disc", ellipse(20.0, 20.0, rgb(40, 90, 220)), (30, 8));
    ed.execute(Command::SetStyles {
        layer: disc,
        styles: vec![
            LayerStyle::OuterGlow { color: rgb(255, 220, 40), opacity: 0.9, blur: 2.5 },
            LayerStyle::InnerShadow(Shadow { color: rgb(0, 0, 0), opacity: 0.5, offset: (2, 2), blur: 1.0 }),
        ],
    })
    .unwrap();
    add(&mut ed, "frame", LayerKind::Frame(FrameLayer { width: 3.0, color: rgb(40, 40, 40), radius: 6.0 }), (0, 0));
    ed.flatten()
}

fn painted_and_adjusted() -> Bitmap {
    let mut ed = editor();
    let bg = add(
        &mut ed,
        "tiles",
        LayerKind::Raster(from_fn(W, H, |x, y| {
            let v = if (x / 8 + y / 8) % 2 == 0 { 200 } else { 120 };
            [v, v, v, 255]
        })),
        (0, 0),
    );
    ed.execute(Command::Select { shape: SelectShape::Rect { x: 0.0, y: 0.0, width: 32.0, height: 48.0 }, op: SelectOp::Replace }).unwrap();
    let brush = Brush { size: 9.0, hardness: 0.6, opacity: 0.8, spacing: 0.25, color: rgb(255, 80, 0) };
    ed.execute(Command::Paint { layer: bg, path: vec![(4.0, 4.0), (28.0, 44.0), (60.0, 10.0)], brush, tool: BrushTool::Paint }).unwrap();
    ed.execute(Command::Deselect).unwrap();
    ed.execute(Command::FloodFill { layer: bg, x: 40, y: 40, tolerance: 30, color: rgb(0, 120, 90), contiguous: true }).unwrap();
    let paint = add(&mut ed, "ink", LayerKind::Raster(solid(W, H, [0, 0, 0, 0])), (0, 0));
    ed.execute(Command::Paint {
        layer: paint,
        path: vec![(50.0, 6.0), (56.0, 40.0)],
        brush: Brush { size: 5.0, hardness: 1.0, opacity: 1.0, spacing: 0.25, color: rgb(20, 20, 160) },
        tool: BrushTool::Paint,
    })
    .unwrap();
    ed.execute(Command::ApplyEffect { layer: paint, effect: Effect::Adjust(Adjust::Inverse) }).unwrap();
    // An adjustment that inverts everything below it, faded by a vertical mask.
    ed.execute(Command::AddLayer { name: "invert".into(), kind: LayerKind::Adjustment(Effect::Adjust(Adjust::Inverse)) }).unwrap();
    let adj = ed.document().active.unwrap();
    let fade = Mask::from_vec(W, H, (0..W * H).map(|i| ((i / W) * 255 / (H - 1)) as u8).collect()).unwrap();
    ed.execute(Command::SetMask { layer: adj, mask: Some(fade) }).unwrap();
    ed.execute(Command::SetOpacity { layer: adj, opacity: 0.9 }).unwrap();
    ed.flatten()
}

fn smart_in_group() -> Bitmap {
    let mut ed = editor();
    add(&mut ed, "floor", LayerKind::Fill(Fill::Solid(rgb(200, 180, 140))), (0, 0));
    let source = from_fn(40, 30, |x, y| [(x * 6) as u8, (y * 8) as u8, 160, 255]);
    let smart = Layer::new(
        LayerId(0),
        "smart",
        LayerKind::Smart {
            source,
            effects: vec![EffectItem { effect: Effect::Adjust(Adjust::Grayscale { value: 100.0 }), enabled: true }],
        },
    );
    ed.execute(Command::AddLayer { name: "group".into(), kind: LayerKind::Group(vec![smart]) }).unwrap();
    let group = ed.document().active.unwrap();
    ed.execute(Command::SetOffset { layer: group, offset: (12, 9) }).unwrap();
    ed.execute(Command::SetBlend { layer: group, blend: BlendMode::Multiply }).unwrap();
    ed.flatten()
}

#[test]
fn blend_stack_matches_its_snapshot() {
    snapshot("blend_stack", &in_pool(blend_stack));
}

#[test]
fn styled_layers_match_their_snapshot() {
    snapshot("styled_layers", &in_pool(styled_layers));
}

#[test]
fn painted_selection_with_adjustment_matches_its_snapshot() {
    snapshot("painted_adjusted", &in_pool(painted_and_adjusted));
}

#[test]
fn smart_object_in_a_group_matches_its_snapshot() {
    snapshot("smart_group", &in_pool(smart_in_group));
}

#[test]
fn flatten_is_the_render() {
    let (a, b) = in_pool(|| {
        let ed = {
            let mut ed = editor();
            add(&mut ed, "paper", LayerKind::Fill(Fill::Solid(rgb(10, 20, 30))), (0, 0));
            ed
        };
        (ed.render(), ed.flatten())
    });
    assert_eq!(a.pixels(), b.pixels());
}
