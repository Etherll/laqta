//! The photo editor window. It forwards input to `laqta-photo` and paints its render;
//! the engine runs on the compute pool, since adjustment layers and effects call imaging kernels.
//! Inpainting runs the `Inpainter` port and hands the result to the engine as a command, falling
//! back to the engine's classic fill when no model is available.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui::{
    App, AppContext as _, Context, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, Pixels, Render, RenderImage, SharedString, StatefulInteractiveElement as _, Styled as _, Window, WindowOptions,
    div, img, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::{ActiveTheme as _, Icon, Root, Sizable as _};
use laqta_domain::color::Rgba;
use laqta_domain::{Bitmap, PixelFormat};
use laqta_photo::{
    BlendMode, Brush, BrushTool, Command, Direction, Fill, FrameLayer, Layer, LayerId, LayerKind, LayerStyle, PhotoEditor, SelectOp,
    SelectShape, Shadow, Shape, ShapeKind, StrokePosition, TextLayer, inpaint_fallback,
};
use smallvec::SmallVec;

use crate::assets::IconName;
use crate::presenters::photo_interaction::PhotoInteraction;
use crate::presenters::render_gate::ComputeFn;
use crate::runtime::UiState;
use crate::widgets::{self, Pill};

const TOOLS_W: f32 = 56.0;
const PANEL_W: f32 = 300.0;
const TOP_H: f32 = 52.0;
/// The footer under the canvas: the status line and the save buttons.
const FOOT_H: f32 = 60.0;

/// Where the canvas area starts: the title bar (when the window draws its own) and the top bar.
fn head(window: &Window) -> f32 {
    crate::chrome::title_bar_height(window) + TOP_H
}
const PAD: f32 = 12.0;

/// A blend mode's name as image editors show it.
fn blend_name(b: BlendMode) -> &'static str {
    match b {
        BlendMode::Normal => "Normal",
        BlendMode::Multiply => "Multiply",
        BlendMode::Screen => "Screen",
        BlendMode::Overlay => "Overlay",
        BlendMode::Darken => "Darken",
        BlendMode::Lighten => "Lighten",
        BlendMode::ColorDodge => "Color dodge",
        BlendMode::ColorBurn => "Color burn",
        BlendMode::HardLight => "Hard light",
        BlendMode::SoftLight => "Soft light",
        BlendMode::Difference => "Difference",
        BlendMode::Exclusion => "Exclusion",
        BlendMode::Hue => "Hue",
        BlendMode::Saturation => "Saturation",
        BlendMode::Color => "Color",
        BlendMode::Luminosity => "Luminosity",
        #[allow(unreachable_patterns)]
        _ => "Blend",
    }
}

const BLENDS: [BlendMode; 16] = [
    BlendMode::Normal,
    BlendMode::Multiply,
    BlendMode::Screen,
    BlendMode::Overlay,
    BlendMode::Darken,
    BlendMode::Lighten,
    BlendMode::ColorDodge,
    BlendMode::ColorBurn,
    BlendMode::HardLight,
    BlendMode::SoftLight,
    BlendMode::Difference,
    BlendMode::Exclusion,
    BlendMode::Hue,
    BlendMode::Saturation,
    BlendMode::Color,
    BlendMode::Luminosity,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Brush,
    Eraser,
    /// Alt-click sets the source; then paint.
    Clone,
    Heal,
    Bucket,
    SelectRect,
    SelectEllipse,
    Lasso,
    Wand,
}

/// The tool rail, in groups: painting, retouching, selecting. Each tool has an icon and a name for
/// its tooltip.
const TOOLS: [&[(Tool, IconName, &str)]; 3] = [
    &[
        (Tool::Brush, IconName::PenLine, "Brush"),
        (Tool::Eraser, IconName::Eraser, "Eraser"),
        (Tool::Bucket, IconName::PaintBucket, "Fill bucket"),
    ],
    &[
        (Tool::Clone, IconName::Stamp, "Clone stamp · Alt-click sets the source"),
        (Tool::Heal, IconName::Sparkles, "Healing · Alt-click sets the source"),
    ],
    &[
        (Tool::SelectRect, IconName::SquareDashed, "Rectangle select"),
        (Tool::SelectEllipse, IconName::Circle, "Ellipse select"),
        (Tool::Lasso, IconName::SquareDashedMousePointer, "Lasso"),
        (Tool::Wand, IconName::WandSparkles, "Magic wand"),
    ],
];

pub(crate) fn open(image: Option<Bitmap>, path: Option<PathBuf>, cx: &mut App) {
    if image.is_none()
        && let Some(p) = path.clone()
    {
        // Read and decode the file off the GPUI thread; a file that doesn't decode opens a blank canvas.
        let app = cx.global::<UiState>().services.app.clone();
        cx.spawn(async move |cx| {
            let decoded = app.decode_file(p.clone()).await;
            if let Err(e) = &decoded {
                tracing::warn!(path = %p.display(), "photo editor: cannot open the image: {e}");
            }
            cx.update(|cx| open(Some(decoded.unwrap_or_else(|_| blank(1280, 800))), Some(p), cx));
        })
        .detach();
        return;
    }
    let image = image.unwrap_or_else(|| blank(1280, 800));
    let options = WindowOptions {
        window_bounds: Some(crate::chrome::centered(1280., 820., cx)),
        titlebar: Some(crate::chrome::titlebar("Photo editor")),
        window_decorations: crate::chrome::decorations(),
        app_owns_titlebar_drag: true,
        app_id: Some(crate::APP_ID.into()),
        focus: true,
        ..Default::default()
    };
    if let Err(e) = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| PhotoWindow::new(image, path, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        tracing::error!("photo editor failed: {e:#}");
    }
}

fn blank(w: u32, h: u32) -> Bitmap {
    let px: Vec<u8> = std::iter::repeat_n([255u8, 255, 255, 255], (w * h) as usize).flatten().collect();
    Bitmap::new(w, h, PixelFormat::Rgba8, px.into()).unwrap_or_else(|_| unreachable!("sized buffer"))
}

fn to_render_image(b: &Bitmap) -> Option<Arc<RenderImage>> {
    let mut bytes = b.pixels().to_vec();
    if b.format() == PixelFormat::Rgba8 {
        for p in bytes.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
        }
    }
    let buffer = image::RgbaImage::from_raw(b.width(), b.height(), bytes)?;
    Some(Arc::new(RenderImage::new(SmallVec::<[image::Frame; 1]>::from_elem(image::Frame::new(buffer), 1))))
}

fn parse_hex(s: &str) -> Option<Rgba> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Rgba::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// One queued engine edit: runs under the engine lock on the compute pool.
type Edit = Box<dyn FnOnce(&mut PhotoEditor) -> Result<(), String> + Send>;

struct PhotoWindow {
    /// Commands, view cache and render gating (GPUI-free, tested in the presenter).
    interaction: PhotoInteraction,
    compute: ComputeFn,
    /// Edits waiting for the running one; they run in order, one at a time.
    queue: std::collections::VecDeque<Edit>,
    /// The render gate asked for a render while a command held the engine.
    render_deferred: bool,
    picture: Option<Arc<RenderImage>>,
    /// Where the canvas area was laid out last frame, in window coordinates (the same space as
    /// mouse events). The window can sit inside a frame inset, so this is measured, not derived.
    canvas_bounds: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<Pixels>>>>,
    size: (u32, u32),
    path: Option<PathBuf>,
    tool: Tool,
    /// The stroke or shape being drawn, in document pixels.
    drag: Vec<(f32, f32)>,
    clone_source: Option<(f32, f32)>,
    color: Entity<InputState>,
    brush_size: Entity<InputState>,
    opacity: Entity<InputState>,
    tolerance: Entity<InputState>,
    /// The text for a new text layer.
    text: Entity<InputState>,
    /// Which adjustment "+ Adjustment" adds (an index into `Adjust::defaults()`).
    adjustment: usize,
    flash: crate::flash::Flash,
    busy: bool,
    /// The document revision the file on disk holds.
    saved_revision: u64,
    /// The window closes once the save it asked for lands (the unsaved-edits prompt's Save).
    close_after_save: bool,
}

use gpui::Entity;

impl crate::flash::HasFlash for PhotoWindow {
    fn flash(&mut self) -> &mut crate::flash::Flash {
        &mut self.flash
    }
}

impl PhotoWindow {
    /// A passing note in the status line.
    fn say(&mut self, msg: String, cx: &mut Context<Self>) {
        crate::flash::Flash::show(self, msg, cx);
    }

    /// A failure in the status line; it stays until dismissed.
    fn fail(&mut self, msg: String, cx: &mut Context<Self>) {
        crate::flash::Flash::fail(self, msg, cx);
    }

    fn new(image: Bitmap, path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let services = cx.global::<UiState>().services.clone();
        // Scan the fonts now, off the UI thread, so the first text tool or emoji doesn't wait for it.
        let fonts = services.fonts.clone();
        cx.background_spawn(async move { fonts.warm() }).detach();
        let size = (image.width(), image.height());
        let editor = PhotoEditor::from_bitmap(image).with_fonts(services.fonts.clone());
        let compute = crate::editor_window::compute_fn(services.spawner.clone());
        let mut input = |v: &str, cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx).default_value(v.to_owned()));
        let mut this = PhotoWindow {
            interaction: PhotoInteraction::new(editor, compute.clone()),
            compute,
            queue: std::collections::VecDeque::new(),
            render_deferred: false,
            picture: None,
            canvas_bounds: Default::default(),
            size,
            path,
            tool: Tool::Brush,
            drag: Vec::new(),
            clone_source: None,
            color: input("#e11d48", cx),
            brush_size: input("16", cx),
            opacity: input("1.0", cx),
            tolerance: input("32", cx),
            text: input("Text", cx),
            adjustment: 0,
            flash: Default::default(),
            busy: false,
            saved_revision: 0,
            close_after_save: false,
        };
        crate::close_guard::install(Self::close_guard, window, cx);
        if this.interaction.initial() {
            this.start_render(cx);
        }
        this
    }

    fn editor(&self) -> Arc<Mutex<PhotoEditor>> {
        self.interaction.editor()
    }

    /// Reads the document without waiting: `None` while a command holds the engine.
    fn doc_read<R>(&self, f: impl FnOnce(&laqta_photo::Document) -> R) -> Option<R> {
        let editor = self.editor();
        let guard = editor.try_lock().ok()?;
        Some(f(guard.document()))
    }

    /// Queues an engine edit. Edits run one at a time on the compute pool; the UI thread never
    /// waits for them.
    fn edit(&mut self, f: impl FnOnce(&mut PhotoEditor) -> Result<(), String> + Send + 'static, cx: &mut Context<Self>) {
        self.queue.push_back(Box::new(f));
        self.next_edit(cx);
    }

    fn next_edit(&mut self, cx: &mut Context<Self>) {
        if !self.interaction.idle() {
            return;
        }
        let Some(f) = self.queue.pop_front() else { return };
        let run = self.interaction.command(f);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = run.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Some(Ok(())) => {}
                    Some(Err(e)) => this.fail(e, cx),
                    None => this.fail("The edit failed.".into(), cx),
                }
                let render = this.interaction.command_finished();
                if render || std::mem::take(&mut this.render_deferred) {
                    this.start_render(cx);
                }
                this.next_edit(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Renders a snapshot on the compute pool and shows it. Only starts between edits, because a
    /// snapshot needs the engine lock that a running edit holds.
    fn start_render(&mut self, cx: &mut Context<Self>) {
        if !self.interaction.idle() {
            self.render_deferred = true;
            return;
        }
        let render = self.interaction.render();
        let compute = self.compute.clone();
        cx.spawn(async move |this, cx| {
            let bitmap = render.await;
            let size = bitmap.as_ref().map(|b| (b.width(), b.height()));
            let picture = match bitmap {
                Some(b) => crate::presenters::render_gate::compute(&compute, move || to_render_image(&b)).await.flatten(),
                None => None,
            };
            let _ = this.update(cx, |this, cx| {
                if let (Some(p), Some(size)) = (picture, size) {
                    this.picture = Some(p);
                    this.size = size;
                }
                if this.interaction.render_finished() {
                    this.start_render(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn exec(&mut self, cmd: Command, cx: &mut Context<Self>) {
        self.edit(move |e| e.execute(cmd).map_err(|e| e.to_string()), cx);
    }

    fn active(&self) -> Option<LayerId> {
        self.doc_read(|d| d.active)
            .unwrap_or_else(|| self.interaction.view().active.and_then(|i| self.interaction.view().layers.get(i).map(|l| l.id)))
    }

    fn val(&self, input: &Entity<InputState>, cx: &App) -> String {
        input.read(cx).value().trim().to_string()
    }

    fn brush(&self, cx: &App) -> Brush {
        Brush {
            size: self.val(&self.brush_size, cx).parse().unwrap_or(16.0f32).clamp(1.0, 500.0),
            opacity: self.val(&self.opacity, cx).parse().unwrap_or(1.0f32).clamp(0.0, 1.0),
            color: parse_hex(&self.val(&self.color, cx)).unwrap_or(Rgba::BLACK),
            ..Brush::default()
        }
    }

    /// Window position → document pixels.
    fn to_doc(&self, pos: gpui::Point<Pixels>, window: &Window) -> (f32, f32) {
        let (ox, oy, z) = self.layout(window);
        ((f32::from(pos.x) - ox) / z, (f32::from(pos.y) - oy) / z)
    }

    fn layout(&self, window: &Window) -> (f32, f32, f32) {
        let (left, top, area_w, area_h) = match self.canvas_bounds.get() {
            Some(b) => (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)),
            None => {
                let vp = window.viewport_size();
                let head = head(window);
                (TOOLS_W, head, f32::from(vp.width) - TOOLS_W - PANEL_W, f32::from(vp.height) - head - FOOT_H)
            }
        };
        let avail_w = (area_w - 2.0 * PAD).max(1.0);
        let avail_h = (area_h - 2.0 * PAD).max(1.0);
        let (w, h) = (self.size.0.max(1) as f32, self.size.1.max(1) as f32);
        let z = (avail_w / w).min(avail_h / h).clamp(0.05, 1.0);
        // Centred in the canvas area.
        (left + PAD + (avail_w - w * z) / 2.0, top + PAD + (avail_h - h * z) / 2.0, z)
    }

    fn in_canvas(&self, pos: gpui::Point<Pixels>, window: &Window) -> bool {
        match self.canvas_bounds.get() {
            Some(b) => b.contains(&pos),
            None => {
                let (x, y) = (f32::from(pos.x), f32::from(pos.y));
                let vp = window.viewport_size();
                x > TOOLS_W && y > head(window) && x < f32::from(vp.width) - PANEL_W && y < f32::from(vp.height) - FOOT_H
            }
        }
    }

    fn finish_drag(&mut self, cx: &mut Context<Self>) {
        let path = std::mem::take(&mut self.drag);
        let Some(&first) = path.first() else { return };
        let last = *path.last().unwrap_or(&first);
        let layer = self.active();
        let brush = self.brush(cx);
        let tol: u8 = self.val(&self.tolerance, cx).parse().unwrap_or(32);
        let rect = |a: (f32, f32), b: (f32, f32)| (a.0.min(b.0), a.1.min(b.1), (a.0 - b.0).abs(), (a.1 - b.1).abs());
        let cmd = match self.tool {
            Tool::Brush | Tool::Eraser | Tool::Clone | Tool::Heal => {
                let Some(layer) = layer else {
                    self.say("Select a raster layer to paint on.".into(), cx);
                    return cx.notify();
                };
                let offset = self.clone_source.map(|s| ((first.0 - s.0) as i32, (first.1 - s.1) as i32)).unwrap_or((0, 0));
                let tool = match self.tool {
                    Tool::Brush => BrushTool::Paint,
                    Tool::Eraser => BrushTool::Erase,
                    Tool::Clone => BrushTool::Clone { offset },
                    _ => BrushTool::Heal { offset },
                };
                if matches!(self.tool, Tool::Clone | Tool::Heal) && self.clone_source.is_none() {
                    self.say("Alt-click to set the source first.".into(), cx);
                    return cx.notify();
                }
                Command::Paint { layer, path, brush, tool }
            }
            Tool::Bucket => {
                let Some(layer) = layer else { return };
                Command::FloodFill { layer, x: first.0 as i32, y: first.1 as i32, tolerance: tol, color: brush.color, contiguous: true }
            }
            Tool::SelectRect => {
                let (x, y, width, height) = rect(first, last);
                Command::Select { shape: SelectShape::Rect { x, y, width, height }, op: SelectOp::Replace }
            }
            Tool::SelectEllipse => {
                let (x, y, width, height) = rect(first, last);
                Command::Select { shape: SelectShape::Ellipse { x, y, width, height }, op: SelectOp::Replace }
            }
            Tool::Lasso => Command::Select { shape: SelectShape::Polygon(path), op: SelectOp::Replace },
            Tool::Wand => Command::Select {
                shape: SelectShape::MagicWand { x: first.0 as i32, y: first.1 as i32, tolerance: tol, contiguous: true },
                op: SelectOp::Replace,
            },
        };
        self.exec(cmd, cx);
    }

    /// Inpaints the selection on the active raster layer: the model through the port, else the
    /// classic fill.
    fn inpaint(&mut self, cx: &mut Context<Self>) {
        let Some(layer) = self.active() else { return };
        let (bitmap, mask) = {
            let editor = self.editor();
            let Ok(e) = editor.try_lock() else { return };
            let doc = e.document();
            let Some(sel) = doc.selection.clone() else {
                self.say("Select the area to fill first.".into(), cx);
                return cx.notify();
            };
            match doc.layer(layer).map(|l| &l.kind) {
                Some(LayerKind::Raster(b)) => (b.clone(), sel),
                _ => {
                    self.say("Inpainting works on a raster layer.".into(), cx);
                    return cx.notify();
                }
            }
        };
        if bitmap.width() != mask.width() || bitmap.height() != mask.height() {
            self.say("Inpainting needs a layer the size of the canvas.".into(), cx);
            return cx.notify();
        }
        let app = cx.global::<UiState>().services.app.clone();
        let compute = self.compute.clone();
        self.busy = true;
        self.say("Inpainting…".into(), cx);
        cx.notify();
        let task = cx.background_spawn(async move {
            match app.erase(bitmap.clone(), mask.data().to_vec()).await {
                Ok(b) => Ok((b, false)),
                Err(laqta_ports::AppError::Cancelled | laqta_ports::AppError::Unsupported(_)) => {
                    crate::presenters::render_gate::compute(&compute, move || inpaint_fallback(&bitmap, &mask))
                        .await
                        .map(|b| (b, true))
                        .ok_or_else(|| "the compute pool stopped".to_owned())
                }
                Err(e) => Err(e.to_string()),
            }
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match r {
                    Ok((bitmap, fallback)) => {
                        this.exec(Command::InpaintResult { layer, bitmap }, cx);
                        this.say(
                            if fallback { "Filled with the classic method (no model).".into() } else { "Filled with the model.".into() },
                            cx,
                        );
                    }
                    Err(e) => this.fail(crate::presenters::notice::failure_line("Couldn't fill the area", &e.to_string()), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The active layer, cloned.
    fn active_layer(&self) -> Option<Layer> {
        self.doc_read(|d| d.active.and_then(|id| d.layer(id)).cloned()).flatten()
    }

    /// A shape over the selection's bounds, or a centred one when nothing is selected.
    fn add_shape(&mut self, ellipse: bool, cx: &mut Context<Self>) {
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let bounds = self.doc_read(|d| d.selection.clone()).flatten().and_then(|m| {
            let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
            for y in 0..m.height() {
                for x in 0..m.width() {
                    if m.get(x as i32, y as i32) > 127 {
                        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                    }
                }
            }
            (x0 <= x1).then(|| (x0 as f32, y0 as f32, (x1 - x0 + 1) as f32, (y1 - y0 + 1) as f32))
        });
        let (x, y, width, height) = bounds.unwrap_or((w * 0.3, h * 0.3, w * 0.4, h * 0.4));
        let color = parse_hex(&self.val(&self.color, cx)).unwrap_or(Rgba::BLACK);
        let kind = if ellipse { ShapeKind::Ellipse { x, y, width, height } } else { ShapeKind::Rect { x, y, width, height } };
        self.add_layer(
            LayerKind::Shape(Shape { kind, fill: Some(color), stroke: None }),
            if ellipse { "Ellipse" } else { "Rectangle" },
            cx,
        );
    }

    fn toggle_style(&mut self, which: &str, cx: &mut Context<Self>) {
        let Some(layer) = self.active_layer() else { return };
        let color = parse_hex(&self.val(&self.color, cx)).unwrap_or(Rgba::BLACK);
        let mut styles = layer.styles.clone();
        let is = |s: &LayerStyle| match which {
            "shadow" => matches!(s, LayerStyle::DropShadow(_)),
            "glow" => matches!(s, LayerStyle::OuterGlow { .. }),
            _ => matches!(s, LayerStyle::Stroke { .. }),
        };
        if styles.iter().any(is) {
            styles.retain(|s| !is(s));
        } else {
            styles.push(match which {
                "shadow" => LayerStyle::DropShadow(Shadow { color: Rgba::BLACK, opacity: 0.6, offset: (6, 6), blur: 6.0 }),
                "glow" => LayerStyle::OuterGlow { color, opacity: 0.8, blur: 8.0 },
                _ => LayerStyle::Stroke { color, width: 4.0, position: StrokePosition::Outside },
            });
        }
        self.exec(Command::SetStyles { layer: layer.id, styles }, cx);
    }

    fn add_layer(&mut self, kind: LayerKind, name: &str, cx: &mut Context<Self>) {
        self.exec(Command::AddLayer { name: name.into(), kind }, cx);
    }

    fn close_guard() -> crate::close_guard::Guard<Self> {
        crate::close_guard::Guard {
            // Edits still queued or running count as unsaved too.
            dirty: |this| this.interaction.view().revision != this.saved_revision || !this.queue.is_empty() || !this.interaction.idle(),
            message: |_| "Save changes to this image?",
            detail: |_| "Your edits are lost if you don't save them.",
            keep: |_| "Save",
            on_keep: |this, window, cx| {
                this.close_after_save = true;
                this.save(false, window.window_handle(), cx);
            },
        }
    }

    fn save(&mut self, ask: bool, handle: gpui::AnyWindowHandle, cx: &mut Context<Self>) {
        let services = cx.global::<UiState>().services.clone();
        let format = services.app.settings().defaults.image.format;
        let opts = laqta_imaging::EncodeOptions { format, jpeg_quality: 90 };
        // The flatten takes its turn in the edit queue, so the file holds every edit made before Save.
        let (flat, flattened) = futures_channel::oneshot::channel();
        self.edit(
            move |editor| {
                let _ = flat.send((editor.flatten(), editor.revision()));
                Ok(())
            },
            cx,
        );
        // The revision the file will hold, read in the same queue turn as the flatten.
        let (rev_tx, saved) = futures_channel::oneshot::channel();
        let flattened: laqta_ports::BoxFuture<'static, Option<Bitmap>> = Box::pin(async move {
            let (bitmap, revision) = flattened.await.ok()?;
            let _ = rev_tx.send(revision);
            Some(bitmap)
        });
        let encoded = crate::presenters::output::encode(self.compute.clone(), services.codecs.clone(), flattened, opts);
        let dir = services.app.settings().screenshots_dir.clone();
        let current = self.path.clone();
        let ext = format.extension();
        let task = cx.background_spawn(async move {
            let bytes = encoded.await?;
            let target = match (ask, current) {
                (false, Some(p)) => Some(p),
                _ => rfd::AsyncFileDialog::new()
                    .set_directory(&dir)
                    .set_file_name(format!("Laqta_photo.{ext}"))
                    .save_file()
                    .await
                    .map(|f| f.path().to_path_buf()),
            };
            match target {
                Some(p) => crate::presenters::output::write_file(&p, &bytes).map(|()| Some(p)),
                None => Ok(None),
            }
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let revision = saved.await.ok();
            let _ = this.update(cx, |this, cx| {
                let close = std::mem::take(&mut this.close_after_save) && matches!(r, Ok(Some(_)));
                match r {
                    Ok(Some(p)) => {
                        if let Some(revision) = revision {
                            this.saved_revision = revision;
                        }
                        this.say(format!("Saved {}", p.display()), cx);
                        this.path = Some(p);
                    }
                    Ok(None) => {}
                    Err(e) => this.fail(crate::presenters::notice::failure_line("Couldn't save", &e.to_string()), cx),
                }
                // An edit made while the file was written is not in it, so it keeps the window open.
                if close && !(Self::close_guard().dirty)(this) {
                    crate::close_guard::close(handle, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn layers_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        // The cached view: a running edit holds the engine, and the panel must not wait for it.
        let view = self.interaction.view();
        let layers: Vec<_> = view.layers.iter().map(|l| (l.id, l.name.clone(), l.visible, l.opacity, l.blend, l.locked)).collect();
        let active = view.active.and_then(|i| view.layers.get(i).map(|l| l.id));
        let selection = self.doc_read(|d| d.selection.is_some()).unwrap_or(false);
        let mut list = div().flex().flex_col().gap(crate::theme::space::S1);
        let wash = crate::theme::extra(cx).wash;
        let entity = cx.entity().downgrade();
        for (id, name, visible, opacity, blend, locked) in layers.into_iter().rev() {
            let selected = active == Some(id);
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(crate::theme::space::S1)
                    .p(crate::theme::space::S1)
                    .rounded(crate::theme::radius::SM)
                    .when_selected(selected, cx)
                    .child(
                        // An icon and a tooltip, so the state never rests on a glyph alone.
                        Button::new(SharedString::from(format!("vis-{}", id.0)))
                            .icon(Icon::new(if visible { IconName::Eye } else { IconName::EyeOff }))
                            .tooltip(if visible { "Hide layer" } else { "Show layer" })
                            .small()
                            .ghost()
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.exec(Command::SetVisible { layer: id, visible: !visible }, cx)),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("layer-{}", id.0)))
                            .flex_1()
                            .min_w_0()
                            .h(px(30.))
                            .flex()
                            .items_center()
                            .gap(crate::theme::space::S2)
                            .px(crate::theme::space::S2)
                            .rounded(crate::theme::radius::SM)
                            .cursor_pointer()
                            .text_color(if selected { cx.theme().foreground } else { cx.theme().muted_foreground })
                            .hover(move |s| s.bg(wash))
                            // An icon and a tooltip, so the state never rests on a glyph alone.
                            .child(
                                Icon::new(if locked { IconName::Lock } else { IconName::Layers }).size(crate::theme::icon::SM).flex_none(),
                            )
                            .child(div().flex_1().min_w_0().truncate().font_weight(gpui::FontWeight::MEDIUM).child(name))
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(if locked {
                                    "Locked: it can't be moved or painted"
                                } else {
                                    "Select this layer"
                                })
                                .build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit(move |e| e.set_active(Some(id)).map_err(|e| e.to_string()), cx);
                            })),
                    )
                    .child({
                        // Every blend mode by name in a menu, the current one checked.
                        let entity = entity.clone();
                        Button::new(SharedString::from(format!("blend-{}", id.0)))
                            .label(blend_name(blend))
                            .tooltip("Blend mode")
                            .xsmall()
                            .ghost()
                            .dropdown_caret(true)
                            .dropdown_menu(move |mut menu, _, _| {
                                for b in BLENDS {
                                    let entity = entity.clone();
                                    menu = menu.item(PopupMenuItem::new(blend_name(b)).checked(b == blend).on_click(move |_, _, cx| {
                                        let _ = entity.update(cx, |this, cx| this.exec(Command::SetBlend { layer: id, blend: b }, cx));
                                    }));
                                }
                                menu
                            })
                    })
                    .child(
                        Button::new(SharedString::from(format!("op-{}", id.0)))
                            .label(format!("{:.0}%", opacity * 100.0))
                            .tooltip("Opacity: click for 25% less")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let next = if opacity <= 0.25 { 1.0 } else { opacity - 0.25 };
                                this.exec(Command::SetOpacity { layer: id, opacity: next }, cx);
                            })),
                    ),
            );
        }
        let has_layer: widgets::Enabled = if self.active().is_some() { Ok(()) } else { Err("Select a layer first".into()) };
        let has_selection: widgets::Enabled = if selection { Ok(()) } else { Err("Select an area first".into()) };
        // A layer action as an icon button, run on the active layer.
        let act = |id: &'static str,
                   glyph: IconName,
                   tip: &'static str,
                   f: fn(&mut PhotoWindow, LayerId, &mut Context<PhotoWindow>),
                   cx: &mut Context<Self>| {
            widgets::icon_button_with(
                id,
                glyph,
                tip,
                false,
                has_layer.clone(),
                cx.listener(move |this, _, _, cx| {
                    if let Some(l) = this.active() {
                        f(this, l, cx);
                    }
                }),
                cx,
            )
        };
        let weak = cx.entity().downgrade();
        // Runs `f` on this window from a menu item.
        let item = |label: String, f: MenuAction| {
            let weak = weak.clone();
            widgets::Choice::new(label, false, move |_, _, cx| {
                let f = f.clone();
                let _ = weak.update(cx, move |this, cx| f(this, cx));
            })
        };
        let (w, h) = self.size;
        let mut add = vec![
            item(
                "Empty layer".into(),
                std::rc::Rc::new(move |this, cx| {
                    let px = Bitmap::new(w, h, PixelFormat::Rgba8, vec![0u8; (w * h * 4) as usize].into()).unwrap_or_else(|_| blank(1, 1));
                    this.add_layer(LayerKind::Raster(px), "Layer", cx)
                }),
            ),
            item(
                "Color fill".into(),
                std::rc::Rc::new(|this, cx| {
                    let c = parse_hex(&this.val(&this.color, cx)).unwrap_or(Rgba::WHITE);
                    this.add_layer(LayerKind::Fill(Fill::Solid(c)), "Fill", cx)
                }),
            ),
            item(
                "Gradient".into(),
                std::rc::Rc::new(|this, cx| {
                    let c = parse_hex(&this.val(&this.color, cx)).unwrap_or(Rgba::BLACK);
                    this.add_layer(LayerKind::Fill(Fill::LinearGradient { from: c, to: Rgba { a: 0, ..c }, angle: 90.0 }), "Gradient", cx)
                }),
            ),
            item(
                "Text".into(),
                std::rc::Rc::new(|this, cx| {
                    let text = this.val(&this.text, cx);
                    let size: f32 = this.val(&this.brush_size, cx).parse().unwrap_or(32.0f32).max(8.0) * 2.0;
                    let color = parse_hex(&this.val(&this.color, cx)).unwrap_or(Rgba::BLACK);
                    let (x, y) = (this.size.0 as f32 * 0.1, this.size.1 as f32 * 0.1);
                    let layer = TextLayer { text, font_family: String::new(), size, color, bold: false, italic: false, x, y };
                    this.add_layer(LayerKind::Text(layer), "Text", cx)
                }),
            ),
            item("Rectangle".into(), std::rc::Rc::new(|this, cx| this.add_shape(false, cx))),
            item("Ellipse".into(), std::rc::Rc::new(|this, cx| this.add_shape(true, cx))),
            item(
                "Frame".into(),
                std::rc::Rc::new(|this, cx| {
                    let color = parse_hex(&this.val(&this.color, cx)).unwrap_or(Rgba::BLACK);
                    this.add_layer(LayerKind::Frame(FrameLayer { width: 16.0, color, radius: 0.0 }), "Frame", cx)
                }),
            ),
        ];
        // Every adjustment by name, so none hides behind a cycling button.
        add.push(item(String::new(), std::rc::Rc::new(|_, _| {})));
        for (i, a) in laqta_domain::effects::Adjust::defaults().into_iter().enumerate() {
            add.push(item(
                format!("{} adjustment", a.name()),
                std::rc::Rc::new(move |this, cx| {
                    this.adjustment = i;
                    let Some(a) = laqta_domain::effects::Adjust::defaults().get(this.adjustment).cloned() else { return };
                    let name = a.name().to_owned();
                    this.add_layer(LayerKind::Adjustment(laqta_domain::effects::Effect::Adjust(a)), &name, cx)
                }),
            ));
        }
        let on_layer = |label: &'static str, f: fn(&mut PhotoWindow, LayerId, &mut Context<PhotoWindow>)| {
            item(
                label.into(),
                std::rc::Rc::new(move |this, cx| {
                    if let Some(l) = this.active() {
                        f(this, l, cx);
                    } else {
                        this.say("Select a layer first.".into(), cx);
                    }
                }),
            )
        };
        let more = vec![
            on_layer("Group", group_active),
            on_layer("Make smart object", smart_active),
            on_layer("Toggle shadow", |t, _, cx| t.toggle_style("shadow", cx)),
            on_layer("Toggle glow", |t, _, cx| t.toggle_style("glow", cx)),
            on_layer("Toggle stroke", |t, _, cx| t.toggle_style("stroke", cx)),
            on_layer("Mask from selection", mask_from_selection),
        ];
        let refine = vec![
            item("Feather by 4 px".into(), std::rc::Rc::new(|this, cx| this.exec(Command::Feather { radius: 4.0 }, cx))),
            item("Grow by 4 px".into(), std::rc::Rc::new(|this, cx| this.exec(Command::Grow { radius: 4 }, cx))),
            item("Shrink by 4 px".into(), std::rc::Rc::new(|this, cx| this.exec(Command::Shrink { radius: 4 }, cx))),
        ];
        let heading = |label: &'static str| {
            div()
                .font_family(crate::theme::text::HEADING_FONT)
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_size(crate::theme::text::SECTION)
                .child(label)
        };
        div()
            .w(px(PANEL_W))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(crate::theme::space::S3)
            .p(crate::theme::space::S4)
            .bg(cx.theme().sidebar)
            .child(div().flex().items_center().justify_between().child(heading("Layers")).child(widgets::menu_button(
                "add-layer",
                "Add",
                add,
            )))
            .child(div().flex_1().min_h_0().overflow_hidden().child(list))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(act(
                        "up",
                        IconName::ArrowUp,
                        "Move up",
                        |t, l, cx| t.exec(Command::MoveLayer { layer: l, direction: Direction::Up }, cx),
                        cx,
                    ))
                    .child(act(
                        "down",
                        IconName::ArrowDown,
                        "Move down",
                        |t, l, cx| t.exec(Command::MoveLayer { layer: l, direction: Direction::Down }, cx),
                        cx,
                    ))
                    .child(act("dup", IconName::Copy, "Duplicate", |t, l, cx| t.exec(Command::DuplicateLayer { layer: l }, cx), cx))
                    .child(act("merge", IconName::Layers, "Merge down", |t, l, cx| t.exec(Command::MergeDown { layer: l }, cx), cx))
                    .child(act("del", IconName::Trash, "Delete layer", |t, l, cx| t.exec(Command::RemoveLayer { layer: l }, cx), cx))
                    .child(div().flex_1())
                    .child(widgets::menu_button_with("layer-more", "", IconName::Ellipsis, more)),
            )
            .child(div().h(px(1.)).bg(cx.theme().border))
            .child(div().flex().items_center().justify_between().child(heading("Selection")).child(widgets::menu_button_with(
                "refine",
                "",
                IconName::Ellipsis,
                refine,
            )))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(crate::theme::space::S2)
                    .child(widgets::pill(
                        "sel-all",
                        "All",
                        Pill::Default,
                        true,
                        cx.listener(|this, _, _, cx| this.exec(Command::Select { shape: SelectShape::All, op: SelectOp::Replace }, cx)),
                        cx,
                    ))
                    .child(widgets::pill_with(
                        "sel-none",
                        "None",
                        Pill::Default,
                        true,
                        has_selection.clone(),
                        cx.listener(|this, _, _, cx| this.exec(Command::Deselect, cx)),
                        cx,
                    ))
                    .child(widgets::pill_with(
                        "sel-inv",
                        "Invert",
                        Pill::Default,
                        true,
                        has_selection.clone(),
                        cx.listener(|this, _, _, cx| this.exec(Command::InvertSelection, cx)),
                        cx,
                    )),
            )
            .child(
                widgets::pill_with(
                    "inpaint",
                    "Fill selection with AI",
                    Pill::Primary,
                    false,
                    if self.busy { Err("Wait for this to finish".into()) } else { has_selection },
                    cx.listener(|this, _, _, cx| this.inpaint(cx)),
                    cx,
                )
                .w_full(),
            )
    }
}

/// What a menu item in the layers panel does to the window.
type MenuAction = std::rc::Rc<dyn Fn(&mut PhotoWindow, &mut Context<PhotoWindow>)>;

/// Wraps the active layer in a group.
fn group_active(t: &mut PhotoWindow, l: LayerId, cx: &mut Context<PhotoWindow>) {
    if let Some(layer) = t.active_layer() {
        t.exec(Command::AddLayer { name: "Group".into(), kind: LayerKind::Group(vec![layer]) }, cx);
        t.exec(Command::RemoveLayer { layer: l }, cx);
    }
}

/// Turns the active raster layer into a smart object, so effects can be re-applied to its source.
fn smart_active(t: &mut PhotoWindow, l: LayerId, cx: &mut Context<PhotoWindow>) {
    match t.active_layer().map(|x| x.kind) {
        Some(LayerKind::Raster(source)) => {
            t.exec(Command::SetKind { layer: l, kind: LayerKind::Smart { source, effects: Vec::new() } }, cx)
        }
        _ => {
            t.say("Only a raster layer becomes a smart object.".into(), cx);
            cx.notify();
        }
    }
}

fn mask_from_selection(t: &mut PhotoWindow, l: LayerId, cx: &mut Context<PhotoWindow>) {
    let sel = t.doc_read(|d| d.selection.clone()).flatten();
    t.exec(Command::SetMask { layer: l, mask: sel }, cx);
}

trait SelectedBg {
    fn when_selected(self, on: bool, cx: &App) -> Self;
}

impl SelectedBg for gpui::Div {
    fn when_selected(self, on: bool, cx: &App) -> Self {
        if on { self.bg(cx.theme().secondary) } else { self }
    }
}

impl Render for PhotoWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (ox, oy, z) = self.layout(window);
        let (w, h) = self.size;
        let t = cx.theme().clone();
        let x = crate::theme::extra(cx);

        // The tool rail: icon buttons in groups, the name in each tooltip.
        let mut tools = div()
            .w(px(TOOLS_W))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(crate::theme::space::S1)
            .py(crate::theme::space::S3)
            .bg(t.sidebar);
        for (gi, group) in TOOLS.iter().enumerate() {
            if gi > 0 {
                tools = tools.child(div().w(px(28.)).h(px(1.)).my(crate::theme::space::S1).bg(t.border));
            }
            for &(tool, glyph, label) in group.iter() {
                tools = tools.child(widgets::icon_button(
                    SharedString::from(format!("tool-{tool:?}")),
                    glyph,
                    label,
                    self.tool == tool,
                    cx.listener(move |this, _, _, cx| {
                        this.tool = tool;
                        cx.notify();
                    }),
                    cx,
                ));
            }
        }

        // The options bar: undo and redo, then the settings the tools read.
        let option = |label: &'static str, input: &Entity<InputState>, width: f32| {
            div()
                .flex()
                .items_center()
                .gap(crate::theme::space::S2)
                .child(div().text_size(crate::theme::text::SMALL).text_color(t.muted_foreground).child(label))
                .child(div().w(px(width)).child(Input::new(input).small()))
        };
        let swatch = parse_hex(&self.val(&self.color, cx))
            .map(|c| gpui::Hsla::from(gpui::Rgba { r: c.r as f32 / 255.0, g: c.g as f32 / 255.0, b: c.b as f32 / 255.0, a: 1.0 }));
        let top = div()
            .h(px(TOP_H))
            .flex()
            .items_center()
            .gap(crate::theme::space::S4)
            .px(crate::theme::space::S3)
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(widgets::icon_button(
                        "undo",
                        IconName::Undo2,
                        "Undo",
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.edit(
                                |e| {
                                    e.undo();
                                    Ok(())
                                },
                                cx,
                            );
                        }),
                        cx,
                    ))
                    .child(widgets::icon_button(
                        "redo",
                        IconName::Redo2,
                        "Redo",
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.edit(
                                |e| {
                                    e.redo();
                                    Ok(())
                                },
                                cx,
                            );
                        }),
                        cx,
                    )),
            )
            .child(widgets::divider_v(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(crate::theme::space::S2)
                    .child(
                        div()
                            .size(px(20.))
                            .flex_none()
                            .rounded(crate::theme::radius::PILL)
                            .border_1()
                            .border_color(x.ring)
                            .bg(swatch.unwrap_or(gpui::transparent_black())),
                    )
                    .child(option("Color", &self.color, 96.)),
            )
            .child(option("Size", &self.brush_size, 56.))
            .child(option("Opacity", &self.opacity, 56.))
            .child(option("Tolerance", &self.tolerance, 56.))
            .child(option("Text", &self.text, 140.));

        // The footer: size or the status line on the left, the save actions on the right.
        let status = self
            .flash
            .render(cx.listener(|this, _, _, cx| crate::flash::Flash::dismiss(this, cx)), cx)
            .unwrap_or_else(|| div().truncate().child(widgets::muted(format!("{w} × {h} px"), cx)));
        let footer = div()
            .h(px(FOOT_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(crate::theme::space::S2)
            .px(crate::theme::space::S4)
            .border_t_1()
            .border_color(t.border)
            .child(div().flex_1().min_w_0().child(status))
            .child(widgets::pill("close", "Close", Pill::Ghost, false, |_, w, cx| crate::close_guard::request_close(w, cx), cx))
            .child(widgets::pill(
                "saveas",
                "Save as…",
                Pill::Default,
                false,
                cx.listener(|this, _, w, cx| this.save(true, w.window_handle(), cx)),
                cx,
            ))
            .child(widgets::pill(
                "save",
                "Save",
                Pill::Primary,
                false,
                cx.listener(|this, _, w, cx| this.save(false, w.window_handle(), cx)),
                cx,
            ));

        // Records where the canvas lands, so the picture and the pointer share one origin. A
        // change paints again at once, so the first frame's guess never stays on screen.
        let cell = self.canvas_bounds.clone();
        let measure = gpui::canvas(
            move |bounds, window, _| {
                if cell.get() != Some(bounds) {
                    cell.set(Some(bounds));
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let (bx, by) = self.canvas_bounds.get().map(|b| (f32::from(b.origin.x), f32::from(b.origin.y))).unwrap_or((TOOLS_W, head(window)));
        let mut canvas = div().flex_1().relative().overflow_hidden().bg(x.stage).child(measure);
        if let Some(p) = &self.picture {
            canvas =
                canvas.child(img(p.clone()).absolute().left(px(ox - bx)).top(px(oy - by)).w(px(w as f32 * z)).h(px(h as f32 * z)).shadow(
                    vec![gpui::BoxShadow {
                        color: gpui::black().opacity(0.35),
                        offset: gpui::point(px(0.), px(4.)),
                        blur_radius: px(16.),
                        spread_radius: px(0.),
                        inset: false,
                    }],
                ));
        }
        crate::chrome::rounded(div(), window)
            .id("laqta-photo")
            .size_full()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.foreground)
            .font_family(crate::theme::text::UI_FONT)
            .text_size(crate::theme::text::LABEL)
            .child(crate::chrome::TitleBar::new().title("Photo editor").flat())
            .child(top)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(tools)
                    .child(div().flex_1().min_w_0().flex().flex_col().child(canvas).child(footer))
                    .child(self.layers_panel(cx)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    if !this.in_canvas(ev.position, window) {
                        return;
                    }
                    let p = this.to_doc(ev.position, window);
                    if ev.modifiers.alt && matches!(this.tool, Tool::Clone | Tool::Heal) {
                        this.clone_source = Some(p);
                        this.say("Source set. Paint to copy from it.".into(), cx);
                        return cx.notify();
                    }
                    this.drag = vec![p];
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, _| {
                if !this.drag.is_empty() {
                    let p = this.to_doc(ev.position, window);
                    this.drag.push(p);
                }
            }))
            .capture_any_mouse_up(cx.listener(|this, _ev: &MouseUpEvent, _, cx| {
                if !this.drag.is_empty() {
                    this.finish_drag(cx);
                }
            }))
    }
}
