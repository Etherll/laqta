//! Undo by reversible changes (ADR 009). Each command becomes a list of small changes, and each change
//! knows how to apply itself and how to revert. Property edits store the layer before and after, and
//! count no pixels. Raster edits store pixels, and the bytes they hold count toward the cap, which
//! drops the oldest undo entries first.

use laqta_domain::Bitmap;

use crate::document::{Document, at_mut};
use crate::layer::{Layer, LayerKind};
use crate::mask::Mask;
use crate::surface::PixelRect;

/// One reversible edit. Paths are indices from the root (see `Document::path_of`).
#[derive(Clone, Debug)]
pub(crate) enum Change {
    /// Inserts `layer` at `path`. Reverting removes it.
    Insert { path: Vec<usize>, layer: Box<Layer> },
    /// Removes the layer at `path`. Reverting puts `layer` back.
    Remove { path: Vec<usize>, layer: Box<Layer> },
    /// Swaps the whole layer at `path`. Used for property edits and whole-layer pixel edits.
    Replace { path: Vec<usize>, before: Box<Layer>, after: Box<Layer> },
    /// Moves the layer at `path` to index `to` within its list.
    Move { path: Vec<usize>, to: usize },
    /// Replaces the pixels of a rectangle of the raster layer at `path`.
    Pixels { path: Vec<usize>, rect: PixelRect, before: Vec<u8>, after: Vec<u8> },
    Selection { before: Option<Mask>, after: Option<Mask> },
    /// The canvas size and every layer position and mask, as one state.
    Canvas { before: Box<CanvasState>, after: Box<CanvasState> },
}

#[derive(Clone, Debug)]
pub(crate) struct CanvasState {
    pub width: u32,
    pub height: u32,
    pub layers: Vec<Layer>,
    pub selection: Option<Mask>,
}

impl CanvasState {
    pub fn of(doc: &Document) -> CanvasState {
        CanvasState {
            width: doc.width,
            height: doc.height,
            layers: doc.layers.clone(),
            selection: doc.selection.clone(),
        }
    }
}

impl Change {
    pub(crate) fn apply(&self, doc: &mut Document) {
        self.run(doc, true);
    }

    pub(crate) fn revert(&self, doc: &mut Document) {
        self.run(doc, false);
    }

    fn run(&self, doc: &mut Document, forward: bool) {
        match self {
            Change::Insert { path, layer } => {
                if forward {
                    insert_at(doc, path, layer);
                } else {
                    remove_at(doc, path);
                }
            }
            Change::Remove { path, layer } => {
                if forward {
                    remove_at(doc, path);
                } else {
                    insert_at(doc, path, layer);
                }
            }
            Change::Replace { path, before, after } => {
                let next = if forward { after } else { before };
                if let Some(slot) = at_mut(&mut doc.layers, path) {
                    *slot = (**next).clone();
                }
            }
            Change::Move { path, to } => {
                let Some((&from, parent)) = path.split_last() else { return };
                let (from, to) = if forward { (from, *to) } else { (*to, from) };
                if let Some(list) = doc.list_mut(parent)
                    && from < list.len()
                {
                    let item = list.remove(from);
                    list.insert(to.min(list.len()), item);
                }
            }
            Change::Pixels { path, rect, before, after } => {
                let data = if forward { after } else { before };
                if let Some(layer) = at_mut(&mut doc.layers, path) {
                    let next = match &layer.kind {
                        LayerKind::Raster(b) => Some(write_rect(b, *rect, data)),
                        _ => None,
                    };
                    if let Some(bitmap) = next {
                        layer.kind = LayerKind::Raster(bitmap);
                    }
                }
            }
            Change::Selection { before, after } => {
                doc.selection = if forward { after.clone() } else { before.clone() };
            }
            Change::Canvas { before, after } => {
                let state = if forward { after } else { before };
                doc.width = state.width;
                doc.height = state.height;
                doc.layers = state.layers.clone();
                doc.selection = state.selection.clone();
            }
        }
    }

    /// The pixel and mask bytes this change holds, for the snapshot cap.
    pub(crate) fn bytes(&self) -> usize {
        match self {
            Change::Insert { layer, .. } | Change::Remove { layer, .. } => layer_bytes(layer),
            Change::Replace { before, after, .. } => {
                if same_pixels(&before.kind, &after.kind) {
                    if before.mask == after.mask {
                        0
                    } else {
                        mask_bytes(&before.mask) + mask_bytes(&after.mask)
                    }
                } else {
                    layer_bytes(before) + layer_bytes(after)
                }
            }
            Change::Move { .. } => 0,
            Change::Pixels { before, after, .. } => before.len() + after.len(),
            Change::Selection { before, after } => mask_option_bytes(before) + mask_option_bytes(after),
            // Canvas changes move layers but copy no pixels, so only their masks count.
            Change::Canvas { before, after } => {
                let masks = |s: &CanvasState| {
                    s.layers.iter().map(layer_mask_bytes).sum::<usize>() + mask_option_bytes(&s.selection)
                };
                masks(before) + masks(after)
            }
        }
    }
}

fn insert_at(doc: &mut Document, path: &[usize], layer: &Layer) {
    let Some((&index, parent)) = path.split_last() else { return };
    if let Some(list) = doc.list_mut(parent) {
        list.insert(index.min(list.len()), layer.clone());
    }
}

fn remove_at(doc: &mut Document, path: &[usize]) {
    let Some((&index, parent)) = path.split_last() else { return };
    if let Some(list) = doc.list_mut(parent)
        && index < list.len()
    {
        list.remove(index);
    }
}

/// A copy of `b` with the rectangle `rect` replaced by `data`.
fn write_rect(b: &Bitmap, rect: PixelRect, data: &[u8]) -> Bitmap {
    let mut s = crate::surface::Surface::from_bitmap(b);
    rect.write(&mut s, data);
    s.into_bitmap()
}

fn same_pixels(a: &LayerKind, b: &LayerKind) -> bool {
    match (a, b) {
        (LayerKind::Raster(x), LayerKind::Raster(y)) => std::ptr::eq(x.pixels().as_ptr(), y.pixels().as_ptr()),
        (LayerKind::Smart { source: x, .. }, LayerKind::Smart { source: y, .. }) => {
            std::ptr::eq(x.pixels().as_ptr(), y.pixels().as_ptr())
        }
        (LayerKind::Raster(_), _) | (_, LayerKind::Raster(_)) => false,
        (LayerKind::Smart { .. }, _) | (_, LayerKind::Smart { .. }) => false,
        _ => true,
    }
}

fn mask_bytes(m: &Option<Mask>) -> usize {
    mask_option_bytes(m)
}

fn mask_option_bytes(m: &Option<Mask>) -> usize {
    m.as_ref().map_or(0, |m| m.data().len())
}

fn layer_mask_bytes(l: &Layer) -> usize {
    let own = mask_option_bytes(&l.mask);
    match &l.kind {
        LayerKind::Group(children) => own + children.iter().map(layer_mask_bytes).sum::<usize>(),
        _ => own,
    }
}

/// The bytes a whole layer holds: its pixels (for groups, every child's) and its mask.
fn layer_bytes(l: &Layer) -> usize {
    let own = match &l.kind {
        LayerKind::Raster(b) => b.pixels().len(),
        LayerKind::Smart { source, .. } => source.pixels().len(),
        LayerKind::Group(children) => children.iter().map(layer_bytes).sum(),
        _ => 0,
    };
    own + mask_option_bytes(&l.mask)
}

/// One command's changes, applied in order and reverted in reverse.
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub changes: Vec<Change>,
}

impl Entry {
    fn bytes(&self) -> usize {
        self.changes.iter().map(Change::bytes).sum()
    }
}

/// The undo and redo stacks, capped by the bytes their entries hold.
#[derive(Debug)]
pub(crate) struct History {
    pub undo: Vec<Entry>,
    pub redo: Vec<Entry>,
    pub cap: usize,
}

impl History {
    pub fn new(cap: usize) -> History {
        History { undo: Vec::new(), redo: Vec::new(), cap }
    }

    /// Records a new command. A new command clears the redo stack.
    pub fn push(&mut self, entry: Entry) {
        self.redo.clear();
        self.undo.push(entry);
        self.enforce_cap();
    }

    /// Drops the oldest undo entries until the held bytes fit under the cap.
    fn enforce_cap(&mut self) {
        while self.bytes() > self.cap && !self.undo.is_empty() {
            self.undo.remove(0);
        }
    }

    pub fn bytes(&self) -> usize {
        self.undo.iter().chain(&self.redo).map(Entry::bytes).sum()
    }
}
