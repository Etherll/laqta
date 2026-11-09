#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]
//! The render job and the revision counter. A job is a snapshot: it renders the document as it was
//! when taken, and its pixels match `render()` at the same revision.

mod support;

use laqta_domain::color::Rgba;
use laqta_photo::{Command, Document, Fill, LayerId, LayerKind, PhotoEditor, PhotoError};

use support::{in_pool, px, solid};

const W: u32 = 64;
const H: u32 = 48;

fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    Rgba { r, g, b, a: 255 }
}

fn editor() -> PhotoEditor {
    PhotoEditor::new(Document::new(W, H).unwrap())
}

/// A paper fill with a red square over it. Returns the editor and the square's id.
fn scene() -> (PhotoEditor, LayerId) {
    let mut ed = editor();
    ed.execute(Command::AddLayer { name: "paper".into(), kind: LayerKind::Fill(Fill::Solid(rgb(240, 240, 240))) }).unwrap();
    ed.execute(Command::AddLayer { name: "square".into(), kind: LayerKind::Raster(solid(16, 16, [200, 30, 30, 255])) }).unwrap();
    let square = ed.document().active.unwrap();
    ed.execute(Command::SetOffset { layer: square, offset: (8, 8) }).unwrap();
    (ed, square)
}

#[test]
fn job_run_matches_render() {
    let (job, direct) = in_pool(|| {
        let (ed, _) = scene();
        (ed.render_job().run(), ed.render())
    });
    assert_eq!(job.pixels(), direct.pixels());
    assert_eq!(px(&job, 0, 0), [240, 240, 240, 255]);
    assert_eq!(px(&job, 10, 10), [200, 30, 30, 255]);
}

#[test]
fn job_taken_before_a_change_renders_the_old_state() {
    in_pool(|| {
        let (mut ed, square) = scene();
        let before = ed.render();
        let job = ed.render_job();
        ed.execute(Command::RemoveLayer { layer: square }).unwrap();
        assert_eq!(job.run().pixels(), before.pixels());
        assert_ne!(ed.render().pixels(), before.pixels());
    });
}

#[test]
fn revision_rises_on_execute_undo_and_redo() {
    let mut ed = editor();
    let r0 = ed.revision();
    ed.execute(Command::AddLayer { name: "a".into(), kind: LayerKind::Raster(solid(4, 4, [0, 0, 0, 255])) }).unwrap();
    let r1 = ed.revision();
    assert!(r1 > r0);
    assert!(ed.undo());
    let r2 = ed.revision();
    assert!(r2 > r1);
    assert!(ed.redo());
    assert!(ed.revision() > r2);
}

#[test]
fn a_failed_execute_leaves_the_revision_and_the_document() {
    let (mut ed, _) = scene();
    let r0 = ed.revision();
    let layers = ed.document().layers.len();
    let err = ed.execute(Command::RemoveLayer { layer: LayerId(999) }).unwrap_err();
    assert!(matches!(err, PhotoError::NoSuchLayer(LayerId(999))));
    assert_eq!(ed.revision(), r0);
    assert_eq!(ed.document().layers.len(), layers);
}

#[test]
fn set_active_counts_as_a_change_and_rejects_an_unknown_layer() {
    let (mut ed, square) = scene();
    let r0 = ed.revision();
    let err = ed.set_active(Some(LayerId(999))).unwrap_err();
    assert!(matches!(err, PhotoError::NoSuchLayer(LayerId(999))));
    assert_eq!(ed.revision(), r0);
    assert_eq!(ed.document().active, Some(square));
    ed.set_active(None).unwrap();
    assert!(ed.revision() > r0);
    assert_eq!(ed.document().active, None);
}

#[test]
fn undo_and_redo_with_nothing_to_do_keep_the_revision() {
    let mut ed = editor();
    let r0 = ed.revision();
    assert!(!ed.undo());
    assert!(!ed.redo());
    assert_eq!(ed.revision(), r0);
}
