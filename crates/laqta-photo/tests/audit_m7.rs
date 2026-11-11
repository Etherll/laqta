#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]
//! Each `audit_bug_*` test asserts the correct result and fails while the bug it names is present.

mod support;

use laqta_domain::color::Rgba;
use laqta_domain::effects::{Adjust, Effect};
use laqta_photo::{BlendMode, Command, Document, Fill, LayerId, LayerKind, PhotoEditor};

use support::{in_pool, px, solid};

const W: u32 = 8;
const H: u32 = 8;

fn add(ed: &mut PhotoEditor, name: &str, kind: LayerKind) -> LayerId {
    ed.execute(Command::AddLayer { name: name.into(), kind }).unwrap();
    ed.document().active.unwrap()
}

/// Merging a layer down into a lower layer whose blend mode is not Normal: the merged layer is reset
/// to Normal, so the picture changes (Photoshop keeps the lower layer's mode).
#[test]
fn audit_bug_merge_down_drops_the_lower_layers_blend_mode() {
    let mut ed = PhotoEditor::new(Document::new(W, H).unwrap());
    add(&mut ed, "red", LayerKind::Fill(Fill::Solid(Rgba { r: 255, g: 0, b: 0, a: 255 })));
    let lower = add(&mut ed, "gray", LayerKind::Raster(solid(W, H, [128, 128, 128, 255])));
    ed.execute(Command::SetBlend { layer: lower, blend: BlendMode::Multiply }).unwrap();
    let upper = add(&mut ed, "empty", LayerKind::Raster(solid(W, H, [0, 0, 0, 0])));
    let before = ed.render();
    ed.execute(Command::MergeDown { layer: upper }).unwrap();
    let after = ed.render();
    assert_eq!(px(&before, 0, 0), px(&after, 0, 0), "merging an empty layer down must not change the picture");
}

/// Merging a raster layer down into an adjustment layer: the adjustment, which applied to every
/// layer beneath it, becomes a plain raster of the upper layer, so the layers below lose it.
#[test]
fn audit_bug_merge_down_into_an_adjustment_layer_drops_the_adjustment() {
    in_pool(|| {
        let mut ed = PhotoEditor::new(Document::new(W, H).unwrap());
        add(&mut ed, "red", LayerKind::Fill(Fill::Solid(Rgba { r: 255, g: 0, b: 0, a: 255 })));
        add(&mut ed, "invert", LayerKind::Adjustment(Effect::Adjust(Adjust::Inverse)));
        let upper = add(&mut ed, "empty", LayerKind::Raster(solid(W, H, [0, 0, 0, 0])));
        let before = ed.render();
        ed.execute(Command::MergeDown { layer: upper }).unwrap();
        let after = ed.render();
        assert_eq!(px(&before, 0, 0), px(&after, 0, 0), "merging an empty layer down must not change the picture");
    });
}
