//! The photo editor engine (ADR 027): layered documents with the 16 blend modes, masks,
//! selections, brushes, retouching, layer styles, adjustment and smart layers, and the inpainting
//! fallback. The pure core: no I/O, threads, environment or clock reads.
//!
//! Adjustment and smart layers, `ApplyEffect`, and the commands that render (a magic wand, a merge
//! down) run `laqta-imaging` kernels, which must run inside the compute pool (`Spawner::run_compute`).
//! Calling [`PhotoEditor::render`] on a document that has adjustment or smart layers needs the pool too.
#![forbid(unsafe_code)]

mod blend;
mod brush;
mod commands;
mod document;
mod history;
mod inpaint;
mod layer;
mod mask;
mod render;
mod selection;
mod style;
mod surface;
mod text;

use laqta_domain::Bitmap;
use laqta_editor::{DEFAULT_SNAPSHOT_CAP, Fonts};

pub use blend::BlendMode;
pub use brush::{Brush, BrushTool};
pub use commands::{Command, Direction};
pub use document::Document;
pub use inpaint::inpaint_fallback;
pub use layer::{Fill, FrameLayer, Layer, LayerId, LayerKind, Shape, ShapeKind, TextLayer};
pub use mask::Mask;
pub use selection::{SelectOp, SelectShape};
pub use style::{LayerStyle, Shadow, StrokePosition};

use crate::document::max_id;
use crate::history::{Entry, History};
use crate::render::render_list;
use crate::surface::Surface;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PhotoError {
    #[error("no layer {0:?}")]
    NoSuchLayer(LayerId),
    #[error("layer {0:?} is locked")]
    Locked(LayerId),
    #[error("layer {id:?} is not {expected}")]
    WrongKind { id: LayerId, expected: &'static str },
    #[error("invalid canvas size {width}x{height}")]
    InvalidSize { width: u32, height: u32 },
    #[error("{0}")]
    Invalid(&'static str),
    #[error("effect failed: {0}")]
    Effect(String),
}

/// The editor: a document, its undo history, and the fonts that text layers use.
#[derive(Debug)]
pub struct PhotoEditor {
    doc: Document,
    history: History,
    fonts: Fonts,
    next_id: u64,
    revision: u64,
}

impl PhotoEditor {
    /// An editor over `doc`. Fonts start empty, so text draws nothing until [`with_fonts`](Self::with_fonts).
    pub fn new(doc: Document) -> PhotoEditor {
        let next_id = max_id(&doc.layers) + 1;
        PhotoEditor { doc, history: History::new(DEFAULT_SNAPSHOT_CAP), fonts: Fonts::empty(), next_id, revision: 0 }
    }

    /// A one-layer document: the bitmap as a raster layer named "Background", at the bitmap's size.
    pub fn from_bitmap(bitmap: Bitmap) -> PhotoEditor {
        let id = LayerId(1);
        let mut doc = Document { width: bitmap.width(), height: bitmap.height(), layers: Vec::new(), selection: None, active: None };
        doc.layers.push(Layer::new(id, "Background", LayerKind::Raster(bitmap)));
        doc.active = Some(id);
        PhotoEditor { doc, history: History::new(DEFAULT_SNAPSHOT_CAP), fonts: Fonts::empty(), next_id: 2, revision: 0 }
    }

    /// Sets the fonts that text layers and text effects render with.
    pub fn with_fonts(mut self, fonts: Fonts) -> PhotoEditor {
        self.fonts = fonts;
        self
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// Sets the active layer. It is not an undo step, but it still bumps the [`revision`](Self::revision).
    pub fn set_active(&mut self, layer: Option<LayerId>) -> Result<(), PhotoError> {
        if let Some(id) = layer
            && self.doc.layer(id).is_none()
        {
            return Err(PhotoError::NoSuchLayer(id));
        }
        self.doc.active = layer;
        self.revision += 1;
        Ok(())
    }

    /// Applies a command and records it for undo. A failed command changes nothing.
    pub fn execute(&mut self, cmd: Command) -> Result<(), PhotoError> {
        let (changes, active) = self.plan(cmd)?;
        if changes.is_empty() {
            return Ok(());
        }
        for change in &changes {
            change.apply(&mut self.doc);
        }
        if let Some(id) = active {
            self.doc.active = Some(id);
        }
        self.fix_active();
        self.history.push(Entry { changes });
        self.revision += 1;
        Ok(())
    }

    /// Reverts the last command. Returns `false` when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(entry) = self.history.undo.pop() else { return false };
        for change in entry.changes.iter().rev() {
            change.revert(&mut self.doc);
        }
        self.history.redo.push(entry);
        self.fix_active();
        self.revision += 1;
        true
    }

    /// Re-applies the last undone command. Returns `false` when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(entry) = self.history.redo.pop() else { return false };
        for change in &entry.changes {
            change.apply(&mut self.doc);
        }
        self.history.undo.push(entry);
        self.fix_active();
        self.revision += 1;
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    /// The most bytes the undo and redo entries may hold (pixels and masks). Default: the editor's
    /// snapshot cap. The oldest undo entries go first when a new one exceeds it.
    pub fn set_snapshot_cap(&mut self, bytes: usize) {
        self.history.cap = bytes;
    }

    /// The bytes the undo and redo entries hold now.
    pub fn snapshot_bytes(&self) -> usize {
        self.history.bytes()
    }

    /// Counts the changes to the document: a successful [`execute`](Self::execute), an
    /// [`undo`](Self::undo) or [`redo`](Self::redo) that changed something, and a successful
    /// [`set_active`](Self::set_active). A render or view taken at one revision still matches the
    /// document while the revision is unchanged.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// A copy of the document and fonts to render later, possibly on another thread. Its
    /// [`run`](PhotoRenderJob::run) gives the same pixels as [`render`](Self::render) at the revision
    /// it was taken.
    pub fn render_job(&self) -> PhotoRenderJob {
        PhotoRenderJob { doc: self.doc.clone(), fonts: self.fonts.clone() }
    }

    /// The composite of the visible layers, straight RGBA8 at the canvas size. Transparent where no
    /// layer is.
    pub fn render(&self) -> Bitmap {
        self.render_job().run()
    }

    /// The image to export: the same pixels as [`render`](Self::render). The layer stack is unchanged;
    /// to merge layers, use [`Command::MergeDown`].
    pub fn flatten(&self) -> Bitmap {
        self.render()
    }

    /// Clears the active layer if it no longer exists (after an undo or a removal).
    fn fix_active(&mut self) {
        if let Some(id) = self.doc.active
            && self.doc.layer(id).is_none()
        {
            self.doc.active = None;
        }
    }
}

/// A snapshot of a document and its fonts, from [`PhotoEditor::render_job`]. It owns its copy, so
/// [`run`](Self::run) can happen on the compute pool while the editor keeps changing.
#[derive(Clone, Debug)]
pub struct PhotoRenderJob {
    doc: Document,
    fonts: Fonts,
}

impl PhotoRenderJob {
    /// Composites the snapshot. Adjustment and smart layers run imaging kernels, so this needs the
    /// compute pool, as [`PhotoEditor::render`] does.
    pub fn run(self) -> Bitmap {
        let mut canvas = Surface::new(self.doc.width, self.doc.height);
        render_list(&mut canvas, &self.doc.layers, (0, 0), &self.fonts);
        canvas.into_bitmap()
    }
}

// A render job must be able to move to the compute pool.
const _: fn() = || {
    fn assert_send<T: Send + 'static>() {}
    assert_send::<PhotoRenderJob>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::Surface;

    #[test]
    fn from_bitmap_makes_one_background_layer() {
        let editor = PhotoEditor::from_bitmap(Surface::new(3, 2).into_bitmap());
        assert_eq!(editor.document().layers.len(), 1);
        assert_eq!((editor.document().width, editor.document().height), (3, 2));
        assert_eq!(editor.document().active, Some(LayerId(1)));
    }
}
