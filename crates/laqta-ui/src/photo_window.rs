//! The photo editor window (M7 D1–D4). It forwards input to `laqta-photo` and paints its render;
//! the engine runs on the compute pool, since adjustment layers and effects call imaging kernels.
//! Inpainting runs the `Inpainter` port and hands the result to the engine as a command, falling
//! back to the engine's classic fill when no model is available.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures_channel::oneshot;
use gpui::{
    App, AppContext as _, Context, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, Pixels, Render, RenderImage, SharedString, Styled as _, TitlebarOptions, Window, WindowBounds, WindowOptions, div,
    img, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::{ActiveTheme as _, Disableable as _, Root, Selectable as _, Sizable as _};
use laqta_domain::color::Rgba;
use laqta_domain::{Bitmap, PixelFormat};
use laqta_photo::{
    BlendMode, Brush, BrushTool, Command, Direction, Fill, LayerId, LayerKind, PhotoEditor, SelectOp, SelectShape, inpaint_fallback,
};
use smallvec::SmallVec;

use crate::runtime::UiState;
use crate::widgets;

const TOOLS_W: f32 = 120.0;
const PANEL_W: f32 = 260.0;
const TOP_H: f32 = 48.0;
const PAD: f32 = 12.0;

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

const TOOLS: [(Tool, &str); 9] = [
    (Tool::Brush, "Brush"),
    (Tool::Eraser, "Eraser"),
    (Tool::Clone, "Clone stamp"),
    (Tool::Heal, "Healing"),
    (Tool::Bucket, "Fill bucket"),
    (Tool::SelectRect, "Rect select"),
    (Tool::SelectEllipse, "Ellipse select"),
    (Tool::Lasso, "Lasso"),
    (Tool::Wand, "Magic wand"),
];

pub(crate) fn open(image: Option<Bitmap>, path: Option<PathBuf>, cx: &mut App) {
    let services = cx.global::<UiState>().services.clone();
    let image = match (image, &path) {
        (Some(b), _) => Some(b),
        (None, Some(p)) => std::fs::read(p).ok().and_then(|b| services.codecs.decode(&b).ok()),
        (None, None) => None,
    };
    let image = image.unwrap_or_else(|| blank(1280, 800));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(1280.), px(820.)), cx)),
        titlebar: Some(TitlebarOptions { title: Some("Photo editor".into()), ..Default::default() }),
        app_id: Some("laqta".into()),
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

/// Runs `f` on the compute pool and waits for it.
fn in_pool<R: Send + 'static>(spawner: &Arc<dyn laqta_ports::Spawner>, f: impl FnOnce() -> R + Send + 'static) -> Option<R> {
    let (tx, rx) = oneshot::channel();
    let fut = spawner.run_compute(Box::new(move || {
        let _ = tx.send(f());
    }));
    futures_lite::future::block_on(async move {
        fut.await;
        rx.await.ok()
    })
}

fn parse_hex(s: &str) -> Option<Rgba> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Rgba::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

struct PhotoWindow {
    editor: Arc<Mutex<PhotoEditor>>,
    spawner: Arc<dyn laqta_ports::Spawner>,
    picture: Option<Arc<RenderImage>>,
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
    message: Option<String>,
    busy: bool,
}

use gpui::Entity;

impl PhotoWindow {
    fn new(image: Bitmap, path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let services = cx.global::<UiState>().services.clone();
        let size = (image.width(), image.height());
        let editor = PhotoEditor::from_bitmap(image).with_fonts(services.fonts.clone());
        let mut input = |v: &str, cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx).default_value(v.to_owned()));
        let mut this = PhotoWindow {
            editor: Arc::new(Mutex::new(editor)),
            spawner: services.spawner.clone(),
            picture: None,
            size,
            path,
            tool: Tool::Brush,
            drag: Vec::new(),
            clone_source: None,
            color: input("#e11d48", cx),
            brush_size: input("16", cx),
            opacity: input("1.0", cx),
            tolerance: input("32", cx),
            message: None,
            busy: false,
        };
        this.refresh(cx);
        this
    }

    fn with<R: Send + 'static>(&self, f: impl FnOnce(&mut PhotoEditor) -> R + Send + 'static) -> Option<R> {
        let editor = self.editor.clone();
        in_pool(&self.spawner, move || {
            let mut e = editor.lock().unwrap_or_else(|p| p.into_inner());
            f(&mut e)
        })
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(b) = self.with(|e| e.render()) {
            self.size = (b.width(), b.height());
            self.picture = to_render_image(&b);
        }
        cx.notify();
    }

    fn exec(&mut self, cmd: Command, cx: &mut Context<Self>) {
        match self.with(move |e| e.execute(cmd)) {
            Some(Ok(())) => self.message = None,
            Some(Err(e)) => self.message = Some(e.to_string()),
            None => {}
        }
        self.refresh(cx);
    }

    fn active(&self) -> Option<LayerId> {
        self.editor.lock().ok().and_then(|e| e.document().active)
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
        let vp = window.viewport_size();
        let avail_w = (f32::from(vp.width) - TOOLS_W - PANEL_W - 2.0 * PAD).max(1.0);
        let avail_h = (f32::from(vp.height) - TOP_H - 2.0 * PAD).max(1.0);
        let (w, h) = (self.size.0.max(1) as f32, self.size.1.max(1) as f32);
        let z = (avail_w / w).min(avail_h / h).clamp(0.05, 1.0);
        (TOOLS_W + PAD, TOP_H + PAD, z)
    }

    fn in_canvas(&self, pos: gpui::Point<Pixels>, window: &Window) -> bool {
        let x = f32::from(pos.x);
        let y = f32::from(pos.y);
        x > TOOLS_W && y > TOP_H && x < f32::from(window.viewport_size().width) - PANEL_W
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
                    self.message = Some("Select a raster layer to paint on.".into());
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
                    self.message = Some("Alt-click to set the source first.".into());
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
            let Ok(e) = self.editor.lock() else { return };
            let doc = e.document();
            let Some(sel) = doc.selection.clone() else {
                self.message = Some("Select the area to fill first.".into());
                return cx.notify();
            };
            match doc.layer(layer).map(|l| &l.kind) {
                Some(LayerKind::Raster(b)) => (b.clone(), sel),
                _ => {
                    self.message = Some("Inpainting works on a raster layer.".into());
                    return cx.notify();
                }
            }
        };
        if bitmap.width() != mask.width() || bitmap.height() != mask.height() {
            self.message = Some("Inpainting needs a layer the size of the canvas.".into());
            return cx.notify();
        }
        let inpainter = cx.global::<UiState>().services.app.ports().inpainter.clone();
        let spawner = self.spawner.clone();
        self.busy = true;
        self.message = Some("Inpainting…".into());
        cx.notify();
        let task = cx.background_spawn(async move {
            match inpainter.inpaint(bitmap.clone(), mask.data().to_vec()).await {
                Ok(b) => Ok((b, false)),
                Err(laqta_ports::PortError::Cancelled | laqta_ports::PortError::Unsupported(_)) => {
                    in_pool(&spawner, move || inpaint_fallback(&bitmap, &mask))
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
                        this.message = Some(if fallback {
                            "Filled with the classic method (no model).".into()
                        } else {
                            "Filled with the model.".into()
                        });
                    }
                    Err(e) => this.message = Some(format!("Inpainting failed: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_layer(&mut self, kind: LayerKind, name: &str, cx: &mut Context<Self>) {
        self.exec(Command::AddLayer { name: name.into(), kind }, cx);
    }

    fn save(&mut self, ask: bool, cx: &mut Context<Self>) {
        let Some(img) = self.with(|e| e.flatten()) else { return };
        let services = cx.global::<UiState>().services.clone();
        let codecs = services.codecs.clone();
        let format = services.app.settings().defaults.image.format;
        let opts = laqta_imaging::EncodeOptions { format, jpeg_quality: 90 };
        let Some(Ok(bytes)) = in_pool(&self.spawner, move || codecs.encode(&img, &opts)) else {
            self.message = Some("Couldn't encode the image.".into());
            return cx.notify();
        };
        let dir = services.app.settings().screenshots_dir.clone();
        let current = self.path.clone();
        let ext = format.extension();
        let task = cx.background_spawn(async move {
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
                Some(p) => std::fs::write(&p, &bytes).map(|_| Some(p)).map_err(|e| e.to_string()),
                None => Ok(None),
            }
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(Some(p)) => {
                        this.message = Some(format!("Saved {}", p.display()));
                        this.path = Some(p);
                    }
                    Ok(None) => {}
                    Err(e) => this.message = Some(format!("Save failed: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn layers_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (layers, active, selection) = match self.editor.lock() {
            Ok(e) => {
                let d = e.document();
                (
                    d.layers.iter().map(|l| (l.id, l.name.clone(), l.visible, l.opacity, l.blend, l.locked)).collect::<Vec<_>>(),
                    d.active,
                    d.selection.is_some(),
                )
            }
            Err(_) => (Vec::new(), None, false),
        };
        let mut list = div().flex().flex_col().gap(px(4.));
        for (id, name, visible, opacity, blend, locked) in layers.into_iter().rev() {
            let selected = active == Some(id);
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .p(px(4.))
                    .rounded(px(4.))
                    .when_selected(selected, cx)
                    .child(
                        Button::new(SharedString::from(format!("vis-{}", id.0)))
                            .label(if visible { "👁" } else { "–" })
                            .xsmall()
                            .ghost()
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.exec(Command::SetVisible { layer: id, visible: !visible }, cx)),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("layer-{}", id.0)))
                            .label(format!("{name}{}", if locked { " 🔒" } else { "" }))
                            .small()
                            .ghost()
                            .selected(selected)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let _ = this.with(move |e| e.set_active(Some(id)));
                                this.refresh(cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new(SharedString::from(format!("blend-{}", id.0))).label(format!("{blend:?}")).xsmall().ghost().on_click(
                            cx.listener(move |this, _, _, cx| {
                                let i = BLENDS.iter().position(|b| *b == blend).unwrap_or(0);
                                this.exec(Command::SetBlend { layer: id, blend: BLENDS[(i + 1) % BLENDS.len()] }, cx);
                            }),
                        ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("op-{}", id.0)))
                            .label(format!("{:.0}%", opacity * 100.0))
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let next = if opacity <= 0.25 { 1.0 } else { opacity - 0.25 };
                                this.exec(Command::SetOpacity { layer: id, opacity: next }, cx);
                            })),
                    ),
            );
        }
        let act = |label: &'static str,
                   id: &'static str,
                   f: fn(&mut PhotoWindow, LayerId, &mut Context<PhotoWindow>),
                   this: &Self,
                   cx: &mut Context<Self>| {
            Button::new(id).label(label).xsmall().disabled(this.active().is_none()).on_click(cx.listener(move |this, _, _, cx| {
                if let Some(l) = this.active() {
                    f(this, l, cx);
                }
            }))
        };
        let (w, h) = self.size;
        div()
            .w(px(PANEL_W))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.))
            .border_l_1()
            .border_color(cx.theme().border)
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Layers"))
            .child(div().flex_1().overflow_hidden().child(list))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .child(Button::new("add-raster").label("+ Layer").xsmall().on_click(cx.listener(move |this, _, _, cx| {
                        this.add_layer(
                            LayerKind::Raster(
                                Bitmap::new(w, h, PixelFormat::Rgba8, vec![0u8; (w * h * 4) as usize].into())
                                    .unwrap_or_else(|_| blank(1, 1)),
                            ),
                            "Layer",
                            cx,
                        )
                    })))
                    .child(Button::new("add-fill").label("+ Fill").xsmall().on_click(cx.listener(|this, _, _, cx| {
                        let c = parse_hex(&this.val(&this.color, cx)).unwrap_or(Rgba::WHITE);
                        this.add_layer(LayerKind::Fill(Fill::Solid(c)), "Fill", cx)
                    })))
                    .child(Button::new("add-grad").label("+ Gradient").xsmall().on_click(cx.listener(|this, _, _, cx| {
                        let c = parse_hex(&this.val(&this.color, cx)).unwrap_or(Rgba::BLACK);
                        this.add_layer(
                            LayerKind::Fill(Fill::LinearGradient { from: c, to: Rgba { a: 0, ..c }, angle: 90.0 }),
                            "Gradient",
                            cx,
                        )
                    })))
                    .child(Button::new("add-adj").label("+ Adjustment").xsmall().on_click(cx.listener(|this, _, _, cx| {
                        let effect = laqta_domain::effects::Effect::Adjust(
                            laqta_domain::effects::Adjust::defaults().into_iter().next().unwrap_or(laqta_domain::effects::Adjust::Inverse),
                        );
                        this.add_layer(LayerKind::Adjustment(effect), "Adjustment", cx)
                    })))
                    .child(act("Duplicate", "dup", |t, l, cx| t.exec(Command::DuplicateLayer { layer: l }, cx), self, cx))
                    .child(act("Up", "up", |t, l, cx| t.exec(Command::MoveLayer { layer: l, direction: Direction::Up }, cx), self, cx))
                    .child(act(
                        "Down",
                        "down",
                        |t, l, cx| t.exec(Command::MoveLayer { layer: l, direction: Direction::Down }, cx),
                        self,
                        cx,
                    ))
                    .child(act("Merge down", "merge", |t, l, cx| t.exec(Command::MergeDown { layer: l }, cx), self, cx))
                    .child(act("Delete", "del", |t, l, cx| t.exec(Command::RemoveLayer { layer: l }, cx), self, cx)),
            )
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Selection"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .child(Button::new("sel-all").label("All").xsmall().on_click(
                        cx.listener(|this, _, _, cx| this.exec(Command::Select { shape: SelectShape::All, op: SelectOp::Replace }, cx)),
                    ))
                    .child(
                        Button::new("sel-none")
                            .label("None")
                            .xsmall()
                            .disabled(!selection)
                            .on_click(cx.listener(|this, _, _, cx| this.exec(Command::Deselect, cx))),
                    )
                    .child(
                        Button::new("sel-inv")
                            .label("Invert")
                            .xsmall()
                            .disabled(!selection)
                            .on_click(cx.listener(|this, _, _, cx| this.exec(Command::InvertSelection, cx))),
                    )
                    .child(
                        Button::new("sel-feather")
                            .label("Feather 4")
                            .xsmall()
                            .disabled(!selection)
                            .on_click(cx.listener(|this, _, _, cx| this.exec(Command::Feather { radius: 4.0 }, cx))),
                    )
                    .child(
                        Button::new("sel-grow")
                            .label("Grow 4")
                            .xsmall()
                            .disabled(!selection)
                            .on_click(cx.listener(|this, _, _, cx| this.exec(Command::Grow { radius: 4 }, cx))),
                    )
                    .child(
                        Button::new("sel-shrink")
                            .label("Shrink 4")
                            .xsmall()
                            .disabled(!selection)
                            .on_click(cx.listener(|this, _, _, cx| this.exec(Command::Shrink { radius: 4 }, cx))),
                    )
                    .child(
                        Button::new("inpaint")
                            .label("Inpaint selection")
                            .xsmall()
                            .primary()
                            .disabled(!selection || self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.inpaint(cx))),
                    ),
            )
    }
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
        let mut tools = div().w(px(TOOLS_W)).flex().flex_col().gap(px(4.)).p(px(8.)).border_r_1().border_color(cx.theme().border);
        for (t, label) in TOOLS {
            tools = tools.child(
                Button::new(SharedString::from(format!("tool-{t:?}"))).label(label).small().ghost().selected(self.tool == t).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.tool = t;
                        cx.notify();
                    }),
                ),
            );
        }
        let top = div()
            .h(px(TOP_H))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(Button::new("undo").label("Undo").small().on_click(cx.listener(|this, _, _, cx| {
                let _ = this.with(|e| e.undo());
                this.refresh(cx);
            })))
            .child(Button::new("redo").label("Redo").small().on_click(cx.listener(|this, _, _, cx| {
                let _ = this.with(|e| e.redo());
                this.refresh(cx);
            })))
            .child(widgets::muted("Colour", cx))
            .child(div().w(px(90.)).child(Input::new(&self.color).small()))
            .child(widgets::muted("Size", cx))
            .child(div().w(px(56.)).child(Input::new(&self.brush_size).small()))
            .child(widgets::muted("Opacity", cx))
            .child(div().w(px(56.)).child(Input::new(&self.opacity).small()))
            .child(widgets::muted("Tolerance", cx))
            .child(div().w(px(56.)).child(Input::new(&self.tolerance).small()))
            .child(div().flex_1().truncate().child(widgets::muted(self.message.clone().unwrap_or_else(|| format!("{w} × {h}")), cx)))
            .child(Button::new("save").label("Save").small().on_click(cx.listener(|this, _, _, cx| this.save(false, cx))))
            .child(Button::new("saveas").label("Save as…").small().on_click(cx.listener(|this, _, _, cx| this.save(true, cx))))
            .child(Button::new("close").label("Close").small().on_click(|_, w, _| w.remove_window()));
        let mut canvas = div().flex_1().relative().overflow_hidden().bg(cx.theme().secondary);
        if let Some(p) = &self.picture {
            canvas =
                canvas.child(img(p.clone()).absolute().left(px(ox - TOOLS_W)).top(px(oy - TOP_H)).w(px(w as f32 * z)).h(px(h as f32 * z)));
        }
        div()
            .id("laqta-photo")
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_sm()
            .child(top)
            .child(div().flex_1().flex().flex_row().child(tools).child(canvas).child(self.layers_panel(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    if !this.in_canvas(ev.position, window) {
                        return;
                    }
                    let p = this.to_doc(ev.position, window);
                    if ev.modifiers.alt && matches!(this.tool, Tool::Clone | Tool::Heal) {
                        this.clone_source = Some(p);
                        this.message = Some("Source set. Paint to copy from it.".into());
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
