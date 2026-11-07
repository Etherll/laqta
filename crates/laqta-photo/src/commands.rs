//! Commands: each one is planned into reversible changes against the current document, then applied
//! and recorded for undo. A command that changes nothing (a move past the end, a wand click outside
//! the canvas) records nothing.

use laqta_domain::Bitmap;
use laqta_domain::color::Rgba;
use laqta_domain::effects::Effect;
use laqta_imaging::effects::apply;

use crate::PhotoEditor;
use crate::PhotoError;
use crate::blend::BlendMode;
use crate::brush::{self, Brush, BrushTool};
use crate::document::renumber;
use crate::history::{CanvasState, Change};
use crate::layer::{Layer, LayerId, LayerKind, normalise};
use crate::mask::Mask;
use crate::render::{render_list, with_context};
use crate::selection::{self, SelectOp, SelectShape};
use crate::style::LayerStyle;
use crate::surface::{PixelRect, Surface};

/// Which way [`Command::MoveLayer`] moves a layer in its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Towards the top of the list (higher index).
    Up,
    /// Towards the bottom of the list (lower index).
    Down,
}

/// An edit to the document. Each variant is one undo step, except commands that change nothing.
#[derive(Clone, Debug)]
pub enum Command {
    /// Adds a layer on top of the root list. A group's children get fresh ids.
    AddLayer { name: String, kind: LayerKind },
    RemoveLayer { layer: LayerId },
    /// Copies a layer just above itself.
    DuplicateLayer { layer: LayerId },
    MoveLayer { layer: LayerId, direction: Direction },
    /// Merges a layer into the one directly below it. The result is a raster layer that keeps the
    /// lower layer's id and name.
    MergeDown { layer: LayerId },
    RenameLayer { layer: LayerId, name: String },
    SetOpacity { layer: LayerId, opacity: f32 },
    SetBlend { layer: LayerId, blend: BlendMode },
    SetVisible { layer: LayerId, visible: bool },
    SetLocked { layer: LayerId, locked: bool },
    SetOffset { layer: LayerId, offset: (i32, i32) },
    SetStyles { layer: LayerId, styles: Vec<LayerStyle> },
    /// Replaces the layer's content kind, for example after editing a shape or an effect.
    SetKind { layer: LayerId, kind: LayerKind },
    SetMask { layer: LayerId, mask: Option<Mask> },
    /// Resizes the canvas. Layers and masks move by `offset`, so `(dx, dy)` shifts the content.
    ResizeCanvas { width: u32, height: u32, offset: (i32, i32) },
    /// Keeps the rectangle at `(x, y)` with the given size. Equivalent to a resize with the
    /// opposite offset.
    Crop { x: i32, y: i32, width: u32, height: u32 },
    Select { shape: SelectShape, op: SelectOp },
    Deselect,
    InvertSelection,
    /// Gaussian feather of the selection, with sigma `radius` pixels.
    Feather { radius: f32 },
    /// Grows the selection by `radius` pixels (square structuring element).
    Grow { radius: u32 },
    /// Shrinks the selection by `radius` pixels (square structuring element).
    Shrink { radius: u32 },
    /// A stroke along `path`, in the layer's own pixel coordinates.
    Paint { layer: LayerId, path: Vec<(f32, f32)>, brush: Brush, tool: BrushTool },
    /// Fills the region matching the pixel at `(x, y)`, in the layer's own coordinates.
    FloodFill { layer: LayerId, x: i32, y: i32, tolerance: u8, color: Rgba, contiguous: bool },
    /// Runs an effect over a raster layer's pixels and keeps the result.
    ApplyEffect { layer: LayerId, effect: Effect },
    /// Replaces a raster layer's pixels with an inpainting result of the same size.
    InpaintResult { layer: LayerId, bitmap: Bitmap },
}

/// A command's changes, and the layer it makes active, if any.
type Plan = (Vec<Change>, Option<LayerId>);

/// A raster layer's path, its pixels, and its document origin.
type RasterTarget = (Vec<usize>, Surface, (i32, i32));

impl PhotoEditor {
    pub(crate) fn plan(&mut self, cmd: Command) -> Result<Plan, PhotoError> {
        match cmd {
            Command::AddLayer { name, kind } => {
                let id = self.alloc_id();
                let mut layer = Layer::new(id, name, kind);
                if let LayerKind::Group(children) = &mut layer.kind {
                    for child in children {
                        renumber(child, &mut self.next_id);
                    }
                }
                let index = self.doc.layers.len();
                Ok((vec![Change::Insert { path: vec![index], layer: Box::new(layer) }], Some(id)))
            }
            Command::RemoveLayer { layer } => {
                let (path, l) = self.find_unlocked(layer)?;
                Ok((vec![Change::Remove { path, layer: Box::new(l) }], None))
            }
            Command::DuplicateLayer { layer } => {
                let (path, mut copy) = self.find(layer)?;
                let index = *path.last().ok_or(PhotoError::NoSuchLayer(layer))?;
                copy.name = format!("{} copy", copy.name);
                renumber(&mut copy, &mut self.next_id);
                let new_id = copy.id;
                let mut new_path = path;
                if let Some(last) = new_path.last_mut() {
                    *last = index + 1;
                }
                Ok((vec![Change::Insert { path: new_path, layer: Box::new(copy) }], Some(new_id)))
            }
            Command::MoveLayer { layer, direction } => {
                let (path, _) = self.find_unlocked(layer)?;
                let (&index, parent) = path.split_last().ok_or(PhotoError::NoSuchLayer(layer))?;
                let len = self.doc.list(parent).map_or(0, <[Layer]>::len);
                let target = match direction {
                    Direction::Up if index + 1 < len => Some(index + 1),
                    Direction::Down if index > 0 => Some(index - 1),
                    _ => None,
                };
                let changes = target.map(|to| Change::Move { path, to }).into_iter().collect();
                Ok((changes, None))
            }
            Command::MergeDown { layer } => self.plan_merge(layer),
            Command::RenameLayer { layer, name } => self.edit(layer, true, |l| l.name = name),
            Command::SetOpacity { layer, opacity } => {
                if !opacity.is_finite() {
                    return Err(PhotoError::Invalid("opacity must be a finite number"));
                }
                self.edit(layer, true, |l| l.opacity = opacity.clamp(0.0, 1.0))
            }
            Command::SetBlend { layer, blend } => self.edit(layer, true, |l| l.blend = blend),
            Command::SetVisible { layer, visible } => self.edit(layer, false, |l| l.visible = visible),
            Command::SetLocked { layer, locked } => self.edit(layer, false, |l| l.locked = locked),
            Command::SetOffset { layer, offset } => self.edit(layer, true, |l| l.offset = offset),
            Command::SetStyles { layer, styles } => self.edit(layer, true, |l| l.styles = styles),
            Command::SetKind { layer, kind } => {
                let kind = normalise(kind);
                self.edit(layer, true, |l| l.kind = kind)
            }
            Command::SetMask { layer, mask } => {
                if mask.as_ref().is_some_and(|m| m.width() != self.doc.width || m.height() != self.doc.height) {
                    return Err(PhotoError::Invalid("layer mask must be document-sized"));
                }
                self.edit(layer, true, |l| l.mask = mask)
            }
            Command::ResizeCanvas { width, height, offset } => self.plan_resize(width, height, offset),
            Command::Crop { x, y, width, height } => self.plan_resize(width, height, (-x, -y)),
            Command::Select { shape, op } => self.plan_select(&shape, op),
            Command::Deselect => {
                let before = self.doc.selection.clone();
                if before.is_none() {
                    return Ok((Vec::new(), None));
                }
                Ok((vec![Change::Selection { before, after: None }], None))
            }
            Command::InvertSelection => {
                let after = selection::invert(self.doc.selection.as_ref(), self.doc.width, self.doc.height);
                Ok((vec![Change::Selection { before: self.doc.selection.clone(), after: Some(after) }], None))
            }
            Command::Feather { radius } => self.map_selection(|m| (radius > 0.0).then(|| m.blur(radius))),
            Command::Grow { radius } => self.map_selection(|m| (radius > 0).then(|| selection::grow(m, radius))),
            Command::Shrink { radius } => self.map_selection(|m| (radius > 0).then(|| selection::shrink(m, radius))),
            Command::Paint { layer, path, brush, tool } => self.plan_paint(layer, &path, &brush, tool),
            Command::FloodFill { layer, x, y, tolerance, color, contiguous } => {
                self.plan_fill(layer, (x, y), tolerance, color, contiguous)
            }
            Command::ApplyEffect { layer, effect } => self.plan_effect(layer, &effect),
            Command::InpaintResult { layer, bitmap } => self.plan_inpaint(layer, bitmap),
        }
    }

    pub(crate) fn alloc_id(&mut self) -> LayerId {
        let id = LayerId(self.next_id);
        self.next_id += 1;
        id
    }

    /// The layer and its path, or `NoSuchLayer`.
    fn find(&self, layer: LayerId) -> Result<(Vec<usize>, Layer), PhotoError> {
        let path = self.doc.path_of(layer).ok_or(PhotoError::NoSuchLayer(layer))?;
        let l = self.doc.at(&path).ok_or(PhotoError::NoSuchLayer(layer))?.clone();
        Ok((path, l))
    }

    /// Like [`find`](Self::find), but refuses a locked layer.
    fn find_unlocked(&self, layer: LayerId) -> Result<(Vec<usize>, Layer), PhotoError> {
        let (path, l) = self.find(layer)?;
        if l.locked {
            return Err(PhotoError::Locked(layer));
        }
        Ok((path, l))
    }

    /// A whole-layer property edit. `guarded` edits refuse a locked layer.
    fn edit(&self, layer: LayerId, guarded: bool, f: impl FnOnce(&mut Layer)) -> Result<Plan, PhotoError> {
        let (path, before) = if guarded { self.find_unlocked(layer)? } else { self.find(layer)? };
        let mut after = before.clone();
        f(&mut after);
        Ok((vec![Change::Replace { path, before: Box::new(before), after: Box::new(after) }], None))
    }

    fn map_selection(&self, f: impl FnOnce(&Mask) -> Option<Mask>) -> Result<Plan, PhotoError> {
        let Some(current) = self.doc.selection.as_ref() else { return Ok((Vec::new(), None)) };
        let Some(after) = f(current) else { return Ok((Vec::new(), None)) };
        Ok((vec![Change::Selection { before: self.doc.selection.clone(), after: Some(after) }], None))
    }

    fn plan_merge(&self, layer: LayerId) -> Result<Plan, PhotoError> {
        let (path, upper) = self.find_unlocked(layer)?;
        let index = *path.last().ok_or(PhotoError::NoSuchLayer(layer))?;
        if index == 0 {
            return Err(PhotoError::Invalid("no layer below to merge into"));
        }
        let mut lower_path = path.clone();
        if let Some(last) = lower_path.last_mut() {
            *last = index - 1;
        }
        let lower = self.doc.at(&lower_path).cloned().ok_or(PhotoError::NoSuchLayer(layer))?;
        if lower.locked {
            return Err(PhotoError::Locked(lower.id));
        }
        // The pair composites onto a transparent canvas, then the result becomes one raster layer.
        let mut canvas = Surface::new(self.doc.width, self.doc.height);
        render_list(&mut canvas, &[lower.clone(), upper.clone()], (0, 0), &self.fonts);
        let merged = Layer {
            id: lower.id,
            name: lower.name.clone(),
            kind: LayerKind::Raster(canvas.into_bitmap()),
            visible: lower.visible,
            opacity: 1.0,
            blend: BlendMode::Normal,
            mask: None,
            styles: Vec::new(),
            locked: false,
            offset: (0, 0),
        };
        let changes = vec![
            Change::Replace { path: lower_path, before: Box::new(lower), after: Box::new(merged) },
            Change::Remove { path, layer: Box::new(upper) },
        ];
        Ok((changes, None))
    }

    fn plan_resize(&self, width: u32, height: u32, offset: (i32, i32)) -> Result<Plan, PhotoError> {
        if width == 0 || height == 0 {
            return Err(PhotoError::InvalidSize { width, height });
        }
        let before = CanvasState::of(&self.doc);
        let mut after = before.clone();
        after.width = width;
        after.height = height;
        for layer in &mut after.layers {
            layer.offset.0 += offset.0;
            layer.offset.1 += offset.1;
        }
        remap_layer_masks(&mut after.layers, width, height, offset);
        after.selection = before.selection.as_ref().map(|m| m.resized(width, height, offset.0, offset.1, 0));
        Ok((vec![Change::Canvas { before: Box::new(before), after: Box::new(after) }], None))
    }

    fn plan_select(&self, shape: &SelectShape, op: SelectOp) -> Result<Plan, PhotoError> {
        let (w, h) = (self.doc.width, self.doc.height);
        let mask = match shape {
            SelectShape::MagicWand { x, y, tolerance, contiguous } => {
                let composite = self.composite();
                match selection::region(&composite, *x, *y, *tolerance, *contiguous) {
                    Some(data) => Mask::from_vec(w, h, data)?,
                    None => return Ok((Vec::new(), None)),
                }
            }
            other => match selection::geometry_mask(other, w, h) {
                Some(m) => m,
                None => return Ok((Vec::new(), None)),
            },
        };
        let after = selection::combine(self.doc.selection.as_ref(), &mask, op);
        Ok((vec![Change::Selection { before: self.doc.selection.clone(), after: Some(after) }], None))
    }

    /// The raster layer's path, its pixels and its document origin, for a pixel edit.
    fn raster_target(&self, layer: LayerId) -> Result<RasterTarget, PhotoError> {
        let (path, l) = self.find_unlocked(layer)?;
        let LayerKind::Raster(b) = &l.kind else {
            return Err(PhotoError::WrongKind { id: layer, expected: "a raster layer" });
        };
        let surface = Surface::from_bitmap(b);
        let origin = self.doc.origin_of(&path);
        Ok((path, surface, origin))
    }

    fn plan_paint(&self, layer: LayerId, path_pts: &[(f32, f32)], brush: &Brush, tool: BrushTool) -> Result<Plan, PhotoError> {
        let (path, before, origin) = self.raster_target(layer)?;
        let mut after = before.clone();
        let Some(rect) = brush::stroke(&mut after, path_pts, brush, tool, self.doc.selection.as_ref(), origin) else {
            return Ok((Vec::new(), None));
        };
        Ok((vec![pixel_change(path, rect, &before, &after)], None))
    }

    fn plan_fill(
        &self,
        layer: LayerId,
        seed: (i32, i32),
        tolerance: u8,
        color: Rgba,
        contiguous: bool,
    ) -> Result<Plan, PhotoError> {
        let (path, before, origin) = self.raster_target(layer)?;
        let mut after = before.clone();
        let Some(rect) = brush::bucket(&mut after, seed, tolerance, color, contiguous, self.doc.selection.as_ref(), origin)
        else {
            return Ok((Vec::new(), None));
        };
        Ok((vec![pixel_change(path, rect, &before, &after)], None))
    }

    fn plan_effect(&self, layer: LayerId, effect: &Effect) -> Result<Plan, PhotoError> {
        let (path, l) = self.find_unlocked(layer)?;
        let LayerKind::Raster(b) = &l.kind else {
            return Err(PhotoError::WrongKind { id: layer, expected: "a raster layer" });
        };
        let out = with_context(&self.fonts, |cx| apply(b, effect, cx))
            .map_err(|e| PhotoError::Effect(e.to_string()))?;
        let mut after = l.clone();
        after.kind = LayerKind::Raster(out);
        Ok((vec![Change::Replace { path, before: Box::new(l), after: Box::new(after) }], None))
    }

    fn plan_inpaint(&self, layer: LayerId, bitmap: Bitmap) -> Result<Plan, PhotoError> {
        let (path, l) = self.find_unlocked(layer)?;
        let LayerKind::Raster(current) = &l.kind else {
            return Err(PhotoError::WrongKind { id: layer, expected: "a raster layer" });
        };
        if current.width() != bitmap.width() || current.height() != bitmap.height() {
            return Err(PhotoError::Invalid("inpainting result must match the layer size"));
        }
        let mut after = l.clone();
        after.kind = normalise(LayerKind::Raster(bitmap));
        Ok((vec![Change::Replace { path, before: Box::new(l), after: Box::new(after) }], None))
    }

    /// The composite of the whole document, as a surface.
    pub(crate) fn composite(&self) -> Surface {
        let mut canvas = Surface::new(self.doc.width, self.doc.height);
        render_list(&mut canvas, &self.doc.layers, (0, 0), &self.fonts);
        canvas
    }
}

/// The change that rewrites `rect` of a raster layer from `before` to `after`.
fn pixel_change(path: Vec<usize>, rect: PixelRect, before: &Surface, after: &Surface) -> Change {
    Change::Pixels { path, rect, before: rect.extract(before), after: rect.extract(after) }
}

/// Moves every layer mask (including masks of nested layers) with the canvas. New area is visible.
fn remap_layer_masks(layers: &mut [Layer], width: u32, height: u32, offset: (i32, i32)) {
    for layer in layers {
        layer.mask = layer.mask.as_ref().map(|m| m.resized(width, height, offset.0, offset.1, 255));
        if let LayerKind::Group(children) = &mut layer.kind {
            remap_layer_masks(children, width, height, offset);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, max_id};

    #[test]
    fn ids_are_found_in_groups() {
        let mut doc = Document::new(4, 4).expect("valid size");
        let child = Layer::new(LayerId(2), "b", LayerKind::Fill(crate::layer::Fill::Solid(Rgba::BLACK)));
        doc.layers.push(Layer::new(LayerId(1), "a", LayerKind::Group(vec![child])));
        assert_eq!(doc.path_of(LayerId(2)), Some(vec![0, 0]));
        assert_eq!(max_id(&doc.layers), 2);
    }
}
