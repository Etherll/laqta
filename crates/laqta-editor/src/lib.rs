//! The Editor engine (ADR 013): one deep module behind every annotation surface.
//!
//! Input events and commands go in; dirty rects, rendered pixels and a chrome description come
//! out. The document (base image plus annotations), the 22 tools, undo and redo, the
//! per-annotation effect cache and the `tiny-skia` renderer are all private. Export is the same
//! renderer at full resolution (ADR 003). Fonts are injected (ADR 024).
//!
//! `execute(ResizeImage | ApplyEffect)` and `render` of blur-heavy documents use rayon kernels:
//! call them inside the compute pool (`Spawner::run_compute`).
#![forbid(unsafe_code)]

mod fonts;
mod fx;
mod geom;
mod history;
mod model;
mod render;

use std::sync::{Arc, Mutex};

use laqta_domain::Bitmap;
use laqta_domain::color::Rgba;
use laqta_domain::effects::{Preset, TextSpec};
use laqta_domain::geometry::PhysRect;

pub use fonts::Fonts;
pub use geom::{PointF, RectF};
pub use history::DEFAULT_SNAPSHOT_CAP;
pub use model::{Annotation, AnnotationKind, FontStyle, Handle, Picture, Style, TextAlign, Tool};

use fx::Img;
use history::{Entry, History, Snapshot};
use render::EffectCache;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorKey {
    /// A letter or digit key (lowercase), for shortcuts.
    Char(char),
    Delete,
    Backspace,
    Enter,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditorInput {
    PointerDown {
        x: f32,
        y: f32,
        button: PointerButton,
        mods: Mods,
    },
    PointerMove {
        x: f32,
        y: f32,
        mods: Mods,
    },
    PointerUp {
        x: f32,
        y: f32,
        button: PointerButton,
        mods: Mods,
    },
    Key {
        key: EditorKey,
        mods: Mods,
    },
    /// Typed characters (the text tools).
    Text(String),
}

#[derive(Clone, Debug)]
pub enum Command {
    SetTool(Tool),
    SetStyle(Style),
    /// The picture the Image, Emoji and Cursor tools place on click.
    SetToolImage(Bitmap),
    Undo,
    Redo,
    Delete,
    Duplicate,
    BringToFront,
    SendToBack,
    MoveUp,
    MoveDown,
    SelectAll,
    Deselect,
    Nudge {
        dx: f32,
        dy: f32,
    },
    Crop(PhysRect),
    /// Removes columns (`vertical`) or rows `from..to` and joins the rest.
    CutOut {
        vertical: bool,
        from: u32,
        to: u32,
    },
    ResizeImage {
        width: u32,
        height: u32,
    },
    /// Adds (or with negative values trims) margins around the canvas.
    ResizeCanvas {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        color: Rgba,
    },
    Rotate90 {
        clockwise: bool,
    },
    Flip {
        horizontal: bool,
    },
    AutoCrop,
    ApplyEffect(Preset),
    InsertImage {
        bitmap: Bitmap,
        x: f32,
        y: f32,
    },
    /// Pastes a picture centred on the canvas.
    Paste(Bitmap),
    /// Swaps the base image for `bitmap` and sizes the canvas to it. The annotations are
    /// flattened away like any destructive operation; one undo restores the old image and them.
    /// Used when the host edited the pixels elsewhere (AI erase, background removal).
    ReplaceImage(Bitmap),
}

/// Something the UI should do (save, copy, close) after a keyboard shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorRequest {
    Save,
    SaveAs,
    Continue,
    /// The Object eraser tool was released over `rect` (image pixels, clamped to the canvas,
    /// never empty). The host builds the mask, runs the erase and answers with
    /// `Command::ReplaceImage`.
    EraseObject {
        rect: PhysRect,
    },
}

/// What the UI paints around the image. Data only.
#[derive(Clone, Debug, PartialEq)]
pub struct Chrome {
    pub tool: Tool,
    pub style: Style,
    pub canvas: (u32, u32),
    /// Bounds of the selected annotation, or the box being dragged with the Object eraser.
    pub selection: Option<RectF>,
    pub handles: Vec<(Handle, PointF)>,
    /// A crop or cut-out rect waiting for Enter.
    pub pending: Option<RectF>,
    /// Where the text caret is, while typing.
    pub caret: Option<PointF>,
    pub editing_text: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub modified: bool,
    pub annotation_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditorOutput {
    /// Canvas regions to repaint (image pixels). Empty when only the chrome changed.
    pub dirty: Vec<PhysRect>,
    pub chrome: Chrome,
    pub request: Option<EditorRequest>,
}

#[derive(Clone, Debug)]
enum Gesture {
    Idle,
    Drawing { start: PointF, current: PointF, shift: bool },
    Freehand { points: Vec<PointF> },
    Moving { id: u64, last: PointF, before: Box<Annotation> },
    Resizing { id: u64, handle: Handle, before: Box<Annotation> },
}

const HANDLE_RADIUS: f32 = 6.0;

/// Everything one render needs, copied out of the engine so it can run on another thread
/// with no engine borrow or lock held. Made by `Editor::render_job`.
pub struct RenderJob {
    base: Arc<Img>,
    annotations: Vec<Annotation>,
    draft: Option<Annotation>,
    view: PhysRect,
    fonts: Fonts,
    cache: Arc<Mutex<EffectCache>>,
    generation: u64,
}

const _: fn() = || {
    fn assert_send<T: Send + 'static>() {}
    assert_send::<RenderJob>();
};

impl RenderJob {
    /// Renders the job's view: the annotations, then the draft on top.
    pub fn run(self) -> Bitmap {
        let mut list: Vec<&Annotation> = self.annotations.iter().collect();
        if let Some(d) = &self.draft {
            list.push(d);
        }
        let mut cache = match self.cache.lock() {
            Ok(c) => c,
            Err(p) => p.into_inner(),
        };
        render::render(&self.base, &list, self.view, &self.fonts, &mut cache, self.generation)
    }
}

impl std::fmt::Debug for RenderJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderJob")
            .field("canvas", &(self.base.w, self.base.h))
            .field("view", &self.view)
            .field("annotations", &self.annotations.len())
            .finish_non_exhaustive()
    }
}

pub struct Editor {
    base: Arc<Img>,
    generation: u64,
    annotations: Vec<Annotation>,
    fonts: Fonts,
    style: Style,
    tool: Tool,
    tool_image: Option<Bitmap>,
    selected: Option<u64>,
    gesture: Gesture,
    /// The annotation being typed into, with its state when typing began.
    editing: Option<(u64, Box<Annotation>)>,
    pending: Option<(Tool, RectF)>,
    next_id: u64,
    step_next: i32,
    history: History,
    clipboard: Option<Annotation>,
    modified: bool,
    cache: Arc<Mutex<EffectCache>>,
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editor")
            .field("canvas", &(self.base.w, self.base.h))
            .field("annotations", &self.annotations.len())
            .finish_non_exhaustive()
    }
}

impl Editor {
    pub fn new(base: Bitmap, fonts: Fonts) -> Editor {
        Editor {
            base: Arc::new(Img::from_bitmap(&base)),
            generation: 0,
            annotations: Vec::new(),
            fonts,
            style: Style::default(),
            tool: Tool::Rectangle,
            tool_image: None,
            selected: None,
            gesture: Gesture::Idle,
            editing: None,
            pending: None,
            next_id: 1,
            step_next: 1,
            history: History::new(),
            clipboard: None,
            modified: false,
            cache: Arc::new(Mutex::new(EffectCache::default())),
        }
    }

    pub fn canvas_size(&self) -> (u32, u32) {
        (self.base.w, self.base.h)
    }

    pub fn annotations(&self) -> &[Annotation] {
        &self.annotations
    }

    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    pub fn is_modified(&self) -> bool {
        self.modified
    }

    /// Changes the undo snapshot cap (bytes); drops old entries if needed.
    pub fn set_snapshot_cap(&mut self, bytes: usize) {
        self.history.cap = bytes;
        self.history.enforce_cap();
    }

    pub fn snapshot_bytes(&self) -> usize {
        self.history.snapshot_bytes()
    }

    #[doc(hidden)]
    pub fn effect_cache_misses(&self) -> u64 {
        self.cache.lock().map(|c| c.misses).unwrap_or(0)
    }

    fn canvas(&self) -> PhysRect {
        PhysRect { x: 0, y: 0, width: self.base.w, height: self.base.h }
    }

    /// A snapshot of the full canvas as `render` draws it, including the shape being drawn.
    /// Run it on any thread; the engine's lock is not held while it runs.
    pub fn render_job(&self) -> RenderJob {
        self.job(self.canvas())
    }

    /// Copies what a render of `view` needs out of the engine.
    fn job(&self, view: PhysRect) -> RenderJob {
        RenderJob {
            base: Arc::clone(&self.base),
            annotations: self.annotations.clone(),
            draft: self.draft(),
            view,
            fonts: self.fonts.clone(),
            cache: Arc::clone(&self.cache),
            generation: self.generation,
        }
    }

    /// Renders the canvas region `view`, including the shape being drawn.
    pub fn render(&self, view: PhysRect) -> Bitmap {
        self.job(view).run()
    }

    /// The full canvas through the same renderer, without the in-progress shape.
    pub fn export(&self) -> Bitmap {
        let list: Vec<&Annotation> = self.annotations.iter().collect();
        let mut cache = match self.cache.lock() {
            Ok(c) => c,
            Err(p) => p.into_inner(),
        };
        render::render(&self.base, &list, self.canvas(), &self.fonts, &mut cache, self.generation)
    }

    pub fn chrome(&self) -> Chrome {
        let sel = self.selected.and_then(|id| self.find(id));
        let caret = self.editing.as_ref().and_then(|(id, _)| self.find(*id)).and_then(|a| {
            let r = a.kind.rect()?.normalized();
            let pad = a.style.font.padding;
            let text = match &a.kind {
                AnnotationKind::Text { text, .. } | AnnotationKind::SpeechBalloon { text, .. } => text.clone(),
                _ => String::new(),
            };
            let (_, w, h) = self.fonts.layout(&text, &a.style.font, Some((r.w - 2.0 * pad).max(1.0)));
            let line = a.style.font.size * 1.25;
            Some(PointF::new(r.x + pad + w, r.y + pad + (h - line).max(0.0)))
        });
        Chrome {
            tool: self.tool,
            style: self.style.clone(),
            canvas: (self.base.w, self.base.h),
            selection: sel.map(|a| a.kind.bounds()).or_else(|| self.rubber_band()),
            handles: sel.map(|a| a.kind.handles()).unwrap_or_default(),
            pending: self.pending.map(|(_, r)| r),
            caret,
            editing_text: self.editing.is_some(),
            can_undo: self.can_undo(),
            can_redo: self.can_redo(),
            modified: self.modified,
            annotation_count: self.annotations.len(),
        }
    }

    fn find(&self, id: u64) -> Option<&Annotation> {
        self.annotations.iter().find(|a| a.id == id)
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.annotations.iter().position(|a| a.id == id)
    }

    fn new_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // ---------------------------------------------------------------- dirty tracking

    fn state(&self) -> (Vec<Annotation>, Option<Annotation>, u64) {
        (self.annotations.clone(), self.draft(), self.generation)
    }

    fn output(&self, before: (Vec<Annotation>, Option<Annotation>, u64), request: Option<EditorRequest>) -> EditorOutput {
        let (before_list, before_draft, before_gen) = before;
        let canvas = self.canvas();
        let full = || vec![canvas];
        let dirty = if before_gen != self.generation {
            full()
        } else {
            let mut rects: Vec<RectF> = Vec::new();
            let mut spot = false;
            let order_changed = before_list.iter().map(|a| a.id).ne(self.annotations.iter().map(|a| a.id));
            for a in &before_list {
                let now = self.find(a.id);
                if now != Some(a) || order_changed {
                    spot |= matches!(a.kind, AnnotationKind::Spotlight { .. });
                    rects.push(a.paint_bounds());
                }
            }
            for a in &self.annotations {
                let then = before_list.iter().find(|b| b.id == a.id);
                if then != Some(a) || order_changed {
                    spot |= matches!(a.kind, AnnotationKind::Spotlight { .. });
                    rects.push(a.paint_bounds());
                }
            }
            let draft = self.draft();
            if before_draft != draft {
                for d in before_draft.iter().chain(draft.iter()) {
                    spot |= matches!(d.kind, AnnotationKind::Spotlight { .. });
                    rects.push(d.paint_bounds());
                }
            }
            if spot { full() } else { rects.into_iter().filter_map(|r| geom::intersect(r.to_phys(), canvas)).collect() }
        };
        EditorOutput { dirty, chrome: self.chrome(), request }
    }

    // ---------------------------------------------------------------- commands

    pub fn execute(&mut self, command: Command) -> EditorOutput {
        let before = self.state();
        self.apply_command(command);
        self.output(before, None)
    }

    fn apply_command(&mut self, command: Command) {
        match command {
            Command::SetTool(t) => {
                self.finish_editing();
                self.tool = t;
                self.pending = None;
                if t != Tool::Select {
                    self.selected = None;
                }
            }
            Command::SetStyle(s) => {
                // Applies to new annotations and to the selected one.
                if let Some(id) = self.selected {
                    let s2 = s.clone();
                    self.modify(id, move |a| a.style = s2);
                }
                self.style = s;
            }
            Command::SetToolImage(b) => self.tool_image = Some(b),
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::Delete => self.delete_selected(),
            Command::Duplicate => self.duplicate_selected(10.0),
            Command::BringToFront => self.reorder(|v, i| {
                let a = v.remove(i);
                v.push(a);
            }),
            Command::SendToBack => self.reorder(|v, i| {
                let a = v.remove(i);
                v.insert(0, a);
            }),
            Command::MoveUp => self.reorder(|v, i| {
                if i + 1 < v.len() {
                    v.swap(i, i + 1);
                }
            }),
            Command::MoveDown => self.reorder(|v, i| {
                if i > 0 {
                    v.swap(i, i - 1);
                }
            }),
            Command::SelectAll => self.selected = self.annotations.last().map(|a| a.id),
            Command::Deselect => {
                self.finish_editing();
                self.selected = None;
                self.pending = None;
            }
            Command::Nudge { dx, dy } => {
                if let Some(id) = self.selected {
                    self.modify(id, |a| a.kind.translate(dx, dy));
                }
            }
            Command::Crop(r) => {
                if let Some(r) = geom::intersect(r, self.canvas()) {
                    self.destructive(|img| img.crop(r));
                }
            }
            Command::CutOut { vertical, from, to } => self.destructive(|img| fx::cut_out(img, vertical, from, to)),
            Command::ResizeImage { width, height } => {
                if width > 0 && height > 0 {
                    self.destructive(|img| Img::from_bitmap(&laqta_imaging::resize(&img.to_bitmap(), width, height)));
                }
            }
            Command::ResizeCanvas { left, top, right, bottom, color } => {
                let w = self.base.w as i32 + left + right;
                let h = self.base.h as i32 + top + bottom;
                if w > 0 && h > 0 {
                    self.destructive(|img| {
                        let mut out = fx::solid(w as u32, h as u32, color);
                        for y in 0..img.h as i32 {
                            for x in 0..img.w as i32 {
                                let (nx, ny) = (x + left, y + top);
                                if nx >= 0 && ny >= 0 && nx < w && ny < h {
                                    out.set(nx as u32, ny as u32, img.get(x, y));
                                }
                            }
                        }
                        out
                    });
                }
            }
            Command::Rotate90 { clockwise } => self.destructive(|img| fx::rotate90(img, clockwise)),
            Command::Flip { horizontal } => self.destructive(|img| fx::flip(img, horizontal)),
            Command::AutoCrop => self.destructive(|img| img.crop(fx::auto_crop_rect(img))),
            Command::ApplyEffect(preset) => {
                let fonts = self.fonts.clone();
                let text = move |spec: &TextSpec| {
                    let font = FontStyle {
                        family: spec.font_family.clone(),
                        size: spec.size,
                        bold: spec.bold,
                        italic: spec.italic,
                        color: spec.color,
                        padding: 0.0,
                        ..FontStyle::default()
                    };
                    fonts.render_spans_bitmap(&spec.text, &font)
                };
                let cx = laqta_imaging::effects::EffectContext { asset: &|_| None, text: &text };
                self.destructive(|img| match laqta_imaging::effects::apply_preset(&img.to_bitmap(), &preset, &cx) {
                    Ok(b) => Img::from_bitmap(&b),
                    Err(_) => img.clone(),
                });
            }
            Command::InsertImage { bitmap, x, y } => {
                let rect = RectF::new(x, y, bitmap.width() as f32, bitmap.height() as f32);
                self.add(AnnotationKind::Image { rect, bitmap: Picture(bitmap) });
            }
            Command::Paste(bitmap) => {
                let (w, h) = (bitmap.width() as f32, bitmap.height() as f32);
                let rect = RectF::new((self.base.w as f32 - w) / 2.0, (self.base.h as f32 - h) / 2.0, w, h);
                self.add(AnnotationKind::Image { rect, bitmap: Picture(bitmap) });
            }
            Command::ReplaceImage(bitmap) => {
                self.finish_editing();
                self.commit_base(Img::from_bitmap(&bitmap));
            }
        }
    }

    fn add(&mut self, kind: AnnotationKind) -> u64 {
        let id = self.new_id();
        let annotation = Annotation { id, kind, style: self.style.clone() };
        self.annotations.push(annotation.clone());
        self.history.push(Entry::Add { index: self.annotations.len() - 1, annotation });
        self.selected = Some(id);
        self.modified = true;
        id
    }

    /// Changes one annotation and records the change for undo.
    fn modify(&mut self, id: u64, f: impl FnOnce(&mut Annotation)) {
        let Some(i) = self.index_of(id) else { return };
        let before = self.annotations[i].clone();
        f(&mut self.annotations[i]);
        let after = self.annotations[i].clone();
        if before != after {
            self.history.push(Entry::Modify { before, after });
            self.modified = true;
        }
    }

    fn reorder(&mut self, f: impl FnOnce(&mut Vec<Annotation>, usize)) {
        let Some(i) = self.selected.and_then(|id| self.index_of(id)) else { return };
        let before: Vec<u64> = self.annotations.iter().map(|a| a.id).collect();
        f(&mut self.annotations, i);
        let after: Vec<u64> = self.annotations.iter().map(|a| a.id).collect();
        if before != after {
            self.history.push(Entry::Reorder { before, after });
            self.modified = true;
        }
    }

    fn delete_selected(&mut self) {
        let Some(id) = self.selected.take() else { return };
        if self.editing.as_ref().is_some_and(|(e, _)| *e == id) {
            self.editing = None;
        }
        if let Some(index) = self.index_of(id) {
            let annotation = self.annotations.remove(index);
            self.history.push(Entry::Remove { index, annotation });
            self.modified = true;
        }
    }

    fn duplicate_selected(&mut self, offset: f32) {
        let Some(a) = self.selected.and_then(|id| self.find(id)).cloned() else { return };
        let mut kind = a.kind;
        kind.translate(offset, offset);
        let style = self.style.clone();
        self.style = a.style;
        self.add(kind);
        self.style = style;
    }

    /// Flattens the annotations into the base, then applies a whole-image operation.
    fn destructive(&mut self, op: impl FnOnce(&Img) -> Img) {
        self.finish_editing();
        let flat = Img::from_bitmap(&self.export());
        self.commit_base(op(&flat));
    }

    /// Makes `result` the base, drops the annotations and records one undo step holding the old
    /// base and annotations. An empty image changes nothing.
    fn commit_base(&mut self, result: Img) {
        if result.w == 0 || result.h == 0 {
            return;
        }
        let before = Snapshot { base: Img::clone(&self.base), annotations: self.annotations.clone() };
        self.base = Arc::new(result);
        self.annotations.clear();
        self.selected = None;
        self.pending = None;
        self.generation += 1;
        self.reset_cache();
        let after = Snapshot { base: Img::clone(&self.base), annotations: Vec::new() };
        self.history.push(Entry::Destructive { before: Box::new(before), after: Box::new(after) });
        self.modified = true;
    }

    fn restore(&mut self, s: &Snapshot) {
        self.base = Arc::new(s.base.clone());
        self.annotations = s.annotations.clone();
        self.generation += 1;
        self.reset_cache();
    }

    fn undo(&mut self) {
        self.finish_editing();
        let Some(e) = self.history.undo.pop() else { return };
        match &e {
            Entry::Add { annotation, .. } => {
                if let Some(i) = self.index_of(annotation.id) {
                    self.annotations.remove(i);
                }
            }
            Entry::Remove { index, annotation } => self.annotations.insert((*index).min(self.annotations.len()), annotation.clone()),
            Entry::Modify { before, .. } => {
                if let Some(i) = self.index_of(before.id) {
                    self.annotations[i] = before.clone();
                }
            }
            Entry::Reorder { before, .. } => self.order_by(before),
            Entry::Destructive { before, .. } => self.restore(before),
        }
        self.history.redo.push(e);
        self.selected = self.selected.filter(|id| self.find(*id).is_some());
        self.modified = true;
        self.cleanup_cache();
    }

    fn redo(&mut self) {
        let Some(e) = self.history.redo.pop() else { return };
        match &e {
            Entry::Add { index, annotation } => self.annotations.insert((*index).min(self.annotations.len()), annotation.clone()),
            Entry::Remove { annotation, .. } => {
                if let Some(i) = self.index_of(annotation.id) {
                    self.annotations.remove(i);
                }
            }
            Entry::Modify { after, .. } => {
                if let Some(i) = self.index_of(after.id) {
                    self.annotations[i] = after.clone();
                }
            }
            Entry::Reorder { after, .. } => self.order_by(after),
            Entry::Destructive { after, .. } => self.restore(after),
        }
        self.history.undo.push(e);
        self.selected = self.selected.filter(|id| self.find(*id).is_some());
        self.modified = true;
        self.cleanup_cache();
    }

    fn order_by(&mut self, ids: &[u64]) {
        self.annotations.sort_by_key(|a| ids.iter().position(|i| *i == a.id).unwrap_or(usize::MAX));
    }

    /// Pruning is housekeeping: skip it while a render job holds the cache, so input never waits.
    fn cleanup_cache(&self) {
        let live: Vec<u64> = self.annotations.iter().map(|a| a.id).collect();
        if let Ok(mut c) = self.cache.try_lock() {
            c.retain(&live);
        }
    }

    /// Drops every cached effect after the base changes. A render job may hold the cache for its
    /// whole run; then the editor takes a fresh cache and the job keeps the old one.
    fn reset_cache(&mut self) {
        let busy = match self.cache.try_lock() {
            Ok(mut c) => {
                c.clear();
                false
            }
            Err(_) => true,
        };
        if busy {
            self.cache = Arc::new(Mutex::new(EffectCache::default()));
        }
    }

    // ---------------------------------------------------------------- input

    pub fn handle(&mut self, input: EditorInput) -> EditorOutput {
        let before = self.state();
        let request = match input {
            EditorInput::PointerDown { x, y, button, mods } => {
                self.pointer_down(PointF::new(x, y), button, mods);
                None
            }
            EditorInput::PointerMove { x, y, mods } => {
                self.pointer_move(PointF::new(x, y), mods);
                None
            }
            EditorInput::PointerUp { x, y, button, mods } => self.pointer_up(PointF::new(x, y), button, mods),
            EditorInput::Key { key, mods } => self.key(key, mods),
            EditorInput::Text(s) => {
                self.type_text(&s);
                None
            }
        };
        self.output(before, request)
    }

    fn hit_handle(&self, p: PointF) -> Option<(u64, Handle)> {
        let a = self.find(self.selected?)?;
        a.kind
            .handles()
            .into_iter()
            .filter(|(_, hp)| hp.distance(p) <= HANDLE_RADIUS)
            .min_by(|x, y| x.1.distance(p).total_cmp(&y.1.distance(p)))
            .map(|(h, _)| (a.id, h))
    }

    fn hit(&self, p: PointF) -> Option<u64> {
        self.annotations.iter().rev().find(|a| a.kind.hit(p, (a.style.border_size / 2.0 + 3.0).max(4.0))).map(|a| a.id)
    }

    fn pointer_down(&mut self, p: PointF, button: PointerButton, mods: Mods) {
        if button != PointerButton::Left {
            // Right/middle cancels a gesture or deselects.
            self.gesture = Gesture::Idle;
            self.pending = None;
            self.finish_editing();
            self.selected = None;
            return;
        }
        if let Some((id, _)) = &self.editing {
            let inside = self.find(*id).is_some_and(|a| a.kind.bounds().contains(p, 2.0));
            if inside {
                return;
            }
            self.finish_editing();
        }
        // Handles of the selection win with any tool.
        if let Some((id, handle)) = self.hit_handle(p)
            && let Some(a) = self.find(id).cloned()
        {
            self.gesture = Gesture::Resizing { id, handle, before: Box::new(a) };
            return;
        }
        match self.tool {
            Tool::Select => match self.hit(p) {
                Some(id) => {
                    self.selected = Some(id);
                    if let Some(a) = self.find(id).cloned() {
                        self.gesture = Gesture::Moving { id, last: p, before: Box::new(a) };
                    }
                }
                None => self.selected = None,
            },
            Tool::Freehand => self.gesture = Gesture::Freehand { points: vec![p] },
            Tool::Step => {
                let n = self.step_next;
                self.step_next += self.style.step_increment;
                let radius = (self.style.font.size * 0.8).max(10.0);
                self.add(AnnotationKind::Step { center: p, number: n, radius });
            }
            Tool::Image | Tool::Emoji | Tool::Cursor => {
                let Some(b) = self.tool_image.clone() else { return };
                let (w, h) = (b.width() as f32, b.height() as f32);
                let kind = match self.tool {
                    Tool::Cursor => AnnotationKind::Cursor { pos: p, bitmap: Picture(b) },
                    Tool::Emoji => {
                        let s = 64.0 / w.max(h).max(1.0);
                        AnnotationKind::Emoji { rect: RectF::new(p.x - w * s / 2.0, p.y - h * s / 2.0, w * s, h * s), bitmap: Picture(b) }
                    }
                    _ => AnnotationKind::Image { rect: RectF::new(p.x, p.y, w, h), bitmap: Picture(b) },
                };
                self.add(kind);
            }
            _ => {
                self.selected = None;
                self.gesture = Gesture::Drawing { start: p, current: p, shift: mods.shift };
            }
        }
    }

    fn pointer_move(&mut self, p: PointF, mods: Mods) {
        match &mut self.gesture {
            Gesture::Drawing { current, shift, .. } => {
                *current = p;
                *shift = mods.shift;
            }
            Gesture::Freehand { points } => {
                if points.last().is_none_or(|l| l.distance(p) >= 1.0) {
                    points.push(p);
                }
            }
            Gesture::Moving { id, last, .. } => {
                let (dx, dy) = (p.x - last.x, p.y - last.y);
                *last = p;
                let id = *id;
                if let Some(i) = self.index_of(id) {
                    self.annotations[i].kind.translate(dx, dy);
                }
            }
            Gesture::Resizing { id, handle, .. } => {
                let (id, handle) = (*id, *handle);
                if let Some(i) = self.index_of(id) {
                    self.annotations[i].kind.drag_handle(handle, p);
                }
            }
            Gesture::Idle => {}
        }
    }

    fn pointer_up(&mut self, p: PointF, button: PointerButton, mods: Mods) -> Option<EditorRequest> {
        if button != PointerButton::Left {
            return None;
        }
        self.pointer_move(p, mods);
        match std::mem::replace(&mut self.gesture, Gesture::Idle) {
            Gesture::Drawing { start, current, shift } => return self.finish_drawing(start, current, shift),
            Gesture::Freehand { points } => {
                self.add(AnnotationKind::Freehand { points });
            }
            Gesture::Moving { id, before, .. } | Gesture::Resizing { id, before, .. } => {
                if let Some(i) = self.index_of(id) {
                    self.annotations[i].kind.normalize();
                    let after = self.annotations[i].clone();
                    if after != *before {
                        self.history.push(Entry::Modify { before: *before, after });
                        self.modified = true;
                    }
                }
            }
            Gesture::Idle => {}
        }
        None
    }

    /// End point adjusted for Shift (squares and 45° lines).
    fn constrained(&self, start: PointF, current: PointF, shift: bool) -> PointF {
        if !shift {
            return current;
        }
        match self.tool {
            Tool::Line | Tool::Arrow => geom::snap_45(start, current),
            _ => geom::square(start, current),
        }
    }

    fn kind_for(&self, start: PointF, end: PointF) -> Option<AnnotationKind> {
        let r = RectF::from_points(start, end);
        Some(match self.tool {
            Tool::Rectangle => AnnotationKind::Rectangle { rect: r },
            Tool::Ellipse => AnnotationKind::Ellipse { rect: r },
            Tool::Line => AnnotationKind::Line { start, end, curve: None },
            Tool::Arrow => AnnotationKind::Arrow { start, end, curve: None },
            Tool::Text => AnnotationKind::Text { rect: r, text: String::new() },
            Tool::SpeechBalloon => {
                AnnotationKind::SpeechBalloon { rect: r, tail: PointF::new(r.x + r.w * 0.3, r.bottom() + 30.0), text: String::new() }
            }
            Tool::Highlight => AnnotationKind::Highlight { rect: r },
            Tool::SmartEraser => {
                let c = self.base.get(r.x as i32, r.y as i32);
                AnnotationKind::SmartEraser { rect: r, color: Rgba { r: c[0], g: c[1], b: c[2], a: c[3] } }
            }
            Tool::Blur => AnnotationKind::Blur { rect: r },
            Tool::Pixelate => AnnotationKind::Pixelate { rect: r },
            Tool::Magnify => AnnotationKind::Magnify { rect: r },
            Tool::Spotlight => AnnotationKind::Spotlight { rect: r },
            Tool::Clone => AnnotationKind::CloneStamp { src: r, dst: PointF::new(r.right() + 10.0, r.y) },
            _ => return None,
        })
    }

    /// The shape being drawn right now, for the live view.
    fn draft(&self) -> Option<Annotation> {
        match &self.gesture {
            Gesture::Drawing { start, current, shift } => {
                let end = self.constrained(*start, *current, *shift);
                if matches!(self.tool, Tool::Crop | Tool::CutOut) {
                    return None;
                }
                let kind = self.kind_for(*start, end)?;
                Some(Annotation { id: 0, kind, style: self.style.clone() })
            }
            Gesture::Freehand { points } => {
                Some(Annotation { id: 0, kind: AnnotationKind::Freehand { points: points.clone() }, style: self.style.clone() })
            }
            _ => None,
        }
    }

    /// The box being dragged with the Object eraser. It is chrome only: the tool never draws a
    /// draft or leaves an annotation.
    fn rubber_band(&self) -> Option<RectF> {
        match (&self.gesture, self.tool) {
            (Gesture::Drawing { start, current, shift }, Tool::ObjectEraser) => {
                Some(RectF::from_points(*start, self.constrained(*start, *current, *shift)))
            }
            _ => None,
        }
    }

    fn finish_drawing(&mut self, start: PointF, current: PointF, shift: bool) -> Option<EditorRequest> {
        let end = self.constrained(start, current, shift);
        let r = RectF::from_points(start, end);
        let tiny = r.w < 2.0 && r.h < 2.0;
        if self.tool == Tool::ObjectEraser {
            if tiny {
                return None;
            }
            return geom::intersect(r.to_phys(), self.canvas()).map(|rect| EditorRequest::EraseObject { rect });
        }
        match self.tool {
            Tool::Crop | Tool::CutOut => {
                if !tiny {
                    self.pending = Some((self.tool, r));
                }
            }
            Tool::Text | Tool::SpeechBalloon => {
                let end = if r.w < 8.0 || r.h < 8.0 { start.offset(200.0, 60.0) } else { end };
                if let Some(kind) = self.kind_for(start, end) {
                    let id = self.add(kind);
                    if let Some(a) = self.find(id).cloned() {
                        self.editing = Some((id, Box::new(a)));
                    }
                }
            }
            _ if tiny => {}
            _ => {
                if let Some(kind) = self.kind_for(start, end) {
                    self.add(kind);
                }
            }
        }
        None
    }

    fn apply_pending(&mut self) {
        let Some((tool, r)) = self.pending.take() else { return };
        let p = r.to_phys();
        match tool {
            Tool::Crop => self.apply_command(Command::Crop(p)),
            Tool::CutOut => {
                let vertical = r.w >= r.h;
                let (from, to) =
                    if vertical { (p.x.max(0) as u32, p.right().max(0) as u32) } else { (p.y.max(0) as u32, p.bottom().max(0) as u32) };
                self.apply_command(Command::CutOut { vertical, from, to });
            }
            _ => {}
        }
    }

    fn type_text(&mut self, s: &str) {
        let Some((id, _)) = &self.editing else { return };
        let Some(i) = self.index_of(*id) else { return };
        if let Some(t) = self.annotations[i].kind.text_mut() {
            t.extend(s.chars().filter(|c| !c.is_control()));
        }
    }

    fn finish_editing(&mut self) {
        let Some((id, before)) = self.editing.take() else { return };
        let Some(a) = self.find(id).cloned() else { return };
        if a != *before {
            // The Add entry for a fresh box already holds the empty text; record the typing.
            self.history.push(Entry::Modify { before: *before, after: a });
            self.modified = true;
        }
    }

    /// The keyboard map.
    fn key(&mut self, key: EditorKey, m: Mods) -> Option<EditorRequest> {
        if let Some((id, _)) = &self.editing {
            let id = *id;
            match key {
                EditorKey::Escape => self.finish_editing(),
                EditorKey::Backspace => {
                    if let Some(i) = self.index_of(id)
                        && let Some(t) = self.annotations[i].kind.text_mut()
                    {
                        t.pop();
                    }
                }
                EditorKey::Enter if m.ctrl => self.finish_editing(),
                EditorKey::Enter => self.type_newline(id),
                _ => {}
            }
            return None;
        }
        let step = if m.shift { 10.0 } else { 1.0 };
        match key {
            EditorKey::Char('z') if m.ctrl && m.shift => self.redo(),
            EditorKey::Char('z') if m.ctrl => self.undo(),
            EditorKey::Char('y') if m.ctrl => self.redo(),
            EditorKey::Char('c') if m.ctrl => self.clipboard = self.selected.and_then(|id| self.find(id)).cloned(),
            EditorKey::Char('x') if m.ctrl => {
                self.clipboard = self.selected.and_then(|id| self.find(id)).cloned();
                self.delete_selected();
            }
            EditorKey::Char('v') if m.ctrl => {
                if let Some(a) = self.clipboard.clone() {
                    let mut kind = a.kind;
                    kind.translate(10.0, 10.0);
                    let style = std::mem::replace(&mut self.style, a.style);
                    self.add(kind);
                    self.style = style;
                }
            }
            EditorKey::Char('d') if m.ctrl => self.duplicate_selected(10.0),
            EditorKey::Char('a') if m.ctrl => self.apply_command(Command::SelectAll),
            EditorKey::Char('s') if m.ctrl && m.shift => return Some(EditorRequest::SaveAs),
            EditorKey::Char('s') if m.ctrl => return Some(EditorRequest::Save),
            EditorKey::Char(c) if !m.ctrl && !m.alt => {
                if let Some(t) = Tool::from_key(c) {
                    self.apply_command(Command::SetTool(t));
                }
            }
            EditorKey::Delete => self.delete_selected(),
            EditorKey::Escape => {
                if self.pending.is_some() || !matches!(self.gesture, Gesture::Idle) {
                    self.pending = None;
                    self.gesture = Gesture::Idle;
                } else {
                    self.selected = None;
                }
            }
            EditorKey::Enter => {
                if self.pending.is_some() {
                    self.apply_pending();
                } else {
                    return Some(EditorRequest::Continue);
                }
            }
            EditorKey::Left | EditorKey::Right | EditorKey::Up | EditorKey::Down => {
                let id = self.selected?;
                let (dx, dy) = match key {
                    EditorKey::Left => (-step, 0.0),
                    EditorKey::Right => (step, 0.0),
                    EditorKey::Up => (0.0, -step),
                    _ => (0.0, step),
                };
                if m.ctrl {
                    self.modify(id, |a| {
                        if let Some(r) = a.kind.rect() {
                            a.kind.set_rect(RectF { w: (r.w + dx).max(1.0), h: (r.h + dy).max(1.0), ..r });
                        }
                    });
                } else {
                    self.modify(id, |a| a.kind.translate(dx, dy));
                }
            }
            EditorKey::Home => self.apply_command(Command::BringToFront),
            EditorKey::End => self.apply_command(Command::SendToBack),
            EditorKey::PageUp => self.apply_command(Command::MoveUp),
            EditorKey::PageDown => self.apply_command(Command::MoveDown),
            EditorKey::Tab => {
                // Cycles the selection through the annotations.
                let ids: Vec<u64> = self.annotations.iter().map(|a| a.id).collect();
                if !ids.is_empty() {
                    let next = match self.selected.and_then(|s| ids.iter().position(|i| *i == s)) {
                        Some(i) => ids[(i + 1) % ids.len()],
                        None => ids[0],
                    };
                    self.selected = Some(next);
                }
            }
            _ => {}
        }
        None
    }

    fn type_newline(&mut self, id: u64) {
        if let Some(i) = self.index_of(id)
            && let Some(t) = self.annotations[i].kind.text_mut()
        {
            t.push('\n');
        }
    }
}
