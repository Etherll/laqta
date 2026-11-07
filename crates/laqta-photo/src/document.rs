//! The document: canvas size, the layer tree, the selection and the active layer.

use crate::PhotoError;
use crate::layer::{Layer, LayerId, LayerKind};
use crate::mask::Mask;

#[derive(Clone, Debug)]
pub struct Document {
    pub width: u32,
    pub height: u32,
    /// Bottom first: index 0 is drawn underneath the others.
    pub layers: Vec<Layer>,
    /// `None` means no selection, so edits are unrestricted.
    pub selection: Option<Mask>,
    pub active: Option<LayerId>,
}

impl Document {
    /// An empty canvas. Both sides must be at least one pixel.
    pub fn new(width: u32, height: u32) -> Result<Document, PhotoError> {
        if width == 0 || height == 0 {
            return Err(PhotoError::InvalidSize { width, height });
        }
        Ok(Document { width, height, layers: Vec::new(), selection: None, active: None })
    }

    /// The layer with `id`, anywhere in the tree.
    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.path_of(id).and_then(|p| at(&self.layers, &p))
    }

    /// Indices from the root to the layer with `id`. A child of a group at index `i`, inside a root
    /// layer at index `r`, has the path `[r, i]`.
    pub(crate) fn path_of(&self, id: LayerId) -> Option<Vec<usize>> {
        find(&self.layers, id)
    }

    pub(crate) fn at(&self, path: &[usize]) -> Option<&Layer> {
        at(&self.layers, path)
    }

    /// The sibling list that holds the layer at `parent` (empty for the root).
    pub(crate) fn list(&self, parent: &[usize]) -> Option<&[Layer]> {
        list(&self.layers, parent)
    }

    pub(crate) fn list_mut(&mut self, parent: &[usize]) -> Option<&mut Vec<Layer>> {
        list_mut(&mut self.layers, parent)
    }

    /// The sum of the offsets of every layer on `path`, which places its content on the canvas.
    pub(crate) fn origin_of(&self, path: &[usize]) -> (i32, i32) {
        let mut origin = (0, 0);
        let mut layers: &[Layer] = &self.layers;
        for &i in path {
            let Some(layer) = layers.get(i) else { break };
            origin.0 += layer.offset.0;
            origin.1 += layer.offset.1;
            layers = match &layer.kind {
                LayerKind::Group(children) => children,
                _ => &[],
            };
        }
        origin
    }
}

fn find(layers: &[Layer], id: LayerId) -> Option<Vec<usize>> {
    for (i, layer) in layers.iter().enumerate() {
        if layer.id == id {
            return Some(vec![i]);
        }
        if let LayerKind::Group(children) = &layer.kind
            && let Some(mut rest) = find(children, id)
        {
            rest.insert(0, i);
            return Some(rest);
        }
    }
    None
}

pub(crate) fn at<'a>(layers: &'a [Layer], path: &[usize]) -> Option<&'a Layer> {
    let (first, rest) = path.split_first()?;
    let layer = layers.get(*first)?;
    if rest.is_empty() {
        return Some(layer);
    }
    match &layer.kind {
        LayerKind::Group(children) => at(children, rest),
        _ => None,
    }
}

pub(crate) fn at_mut<'a>(layers: &'a mut [Layer], path: &[usize]) -> Option<&'a mut Layer> {
    let (first, rest) = path.split_first()?;
    let layer = layers.get_mut(*first)?;
    if rest.is_empty() {
        return Some(layer);
    }
    match &mut layer.kind {
        LayerKind::Group(children) => at_mut(children, rest),
        _ => None,
    }
}

fn list<'a>(layers: &'a [Layer], parent: &[usize]) -> Option<&'a [Layer]> {
    match parent.split_first() {
        None => Some(layers),
        Some((first, rest)) => match &layers.get(*first)?.kind {
            LayerKind::Group(children) => list(children, rest),
            _ => None,
        },
    }
}

fn list_mut<'a>(layers: &'a mut Vec<Layer>, parent: &[usize]) -> Option<&'a mut Vec<Layer>> {
    match parent.split_first() {
        None => Some(layers),
        Some((first, rest)) => match &mut layers.get_mut(*first)?.kind {
            LayerKind::Group(children) => list_mut(children, rest),
            _ => None,
        },
    }
}

/// Gives every layer in the subtree a fresh id from `next`.
pub(crate) fn renumber(layer: &mut Layer, next: &mut u64) {
    layer.id = LayerId(*next);
    *next += 1;
    if let LayerKind::Group(children) = &mut layer.kind {
        for child in children {
            renumber(child, next);
        }
    }
}

/// The largest id in the subtree, or 0.
pub(crate) fn max_id(layers: &[Layer]) -> u64 {
    layers
        .iter()
        .map(|l| match &l.kind {
            LayerKind::Group(children) => l.id.0.max(max_id(children)),
            _ => l.id.0,
        })
        .max()
        .unwrap_or(0)
}
