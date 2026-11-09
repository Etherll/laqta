//! The annotation editor window (M3). It forwards input to the Editor engine, paints the engine's
//! rendered pixels and chrome, and computes nothing itself (ARCHITECTURE §5 rule). The engine runs
//! on the compute pool for every call, since some commands call imaging kernels.

use std::path::PathBuf;
use std::sync::Arc;

use futures_channel::oneshot;
use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, Bounds, ClipboardItem, Context, Div, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render, RenderImage, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, WindowBounds, WindowOptions, div, img, prelude::FluentBuilder as _, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::{ActiveTheme as _, Icon, Root};
use laqta_domain::color::Rgba;
use laqta_domain::{Bitmap, ImageFormat, PixelFormat};
use laqta_editor::{Chrome, Command, Editor, EditorInput, EditorKey, EditorRequest, Mods, PointerButton, Tool};
use laqta_ports::PortError;
use smallvec::SmallVec;

use crate::assets::IconName;
use crate::chrome;
use crate::presenters::editor_interaction::{EditorInteraction, Step};
use crate::presenters::editor_toolbar;
use crate::presenters::render_gate::ComputeFn;
use crate::runtime::UiState;
use crate::theme::{self, radius, space, text};
use crate::widgets;

pub(crate) type EditReply = oneshot::Sender<Result<Bitmap, PortError>>;

/// The tool rail: two columns of tool buttons.
const RAIL_W: f32 = 88.0;
/// The properties bar over the stage.
const TOPBAR_H: f32 = 52.0;
/// The action bar under the stage.
const FOOTER_H: f32 = 60.0;
/// Space between the stage edge and the window's right edge.
const GUTTER: f32 = 10.0;
/// Space around the image inside the stage. The bottom keeps the zoom controls off the image.
const PAD: f32 = 28.0;
const PAD_BOTTOM: f32 = 76.0;

const PALETTE: [Rgba; 10] = [
    Rgba::rgb(0xef, 0x44, 0x44),
    Rgba::rgb(0xf9, 0x73, 0x16),
    Rgba::rgb(0xea, 0xb3, 0x08),
    Rgba::rgb(0x22, 0xc5, 0x5e),
    Rgba::rgb(0x06, 0xb6, 0xd4),
    Rgba::rgb(0x3b, 0x82, 0xf6),
    Rgba::rgb(0xa8, 0x55, 0xf7),
    Rgba::rgb(0xec, 0x48, 0x99),
    Rgba::WHITE,
    Rgba::BLACK,
];

const EMOJI: [&str; 12] = ["😀", "😂", "😍", "👍", "👎", "🔥", "✅", "❌", "⚠️", "❤️", "👉", "💡"];

/// Opens the editor on `image` (a capture) or on a file. `reply` receives the result for Task's
/// `ANNOTATE_IMAGE`; without it the window is standalone (Save/Copy/Upload).
pub(crate) fn open(image: Option<Bitmap>, reply: Option<EditReply>, path: Option<PathBuf>, cx: &mut App) {
    let services = cx.global::<UiState>().services.clone();
    let image = match (image, &path) {
        (Some(i), _) => Some(i),
        (None, Some(p)) => std::fs::read(p).ok().and_then(|b| services.codecs.decode(&b).ok()),
        (None, None) => None,
    };
    let Some(image) = image else {
        if let Some(r) = reply {
            let _ = r.send(Err(PortError::Failed("no image to edit".into())));
        }
        if path.is_none() {
            // No image: ask for one.
            let pick = cx.background_spawn(async move {
                rfd::AsyncFileDialog::new()
                    .add_filter("Images", &["png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp"])
                    .pick_file()
                    .await
                    .map(|f| f.path().to_path_buf())
            });
            cx.spawn(async move |cx| {
                if let Some(file) = pick.await {
                    cx.update(|cx| open(None, None, Some(file), cx));
                }
            })
            .detach();
        }
        return;
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(1180.), px(820.)), cx)),
        titlebar: Some(chrome::titlebar("Laqta editor")),
        window_decorations: chrome::decorations(),
        app_owns_titlebar_drag: true,
        app_id: Some("laqta".into()),
        focus: true,
        window_min_size: Some(size(px(720.), px(480.))),
        ..Default::default()
    };
    let mut reply = reply;
    let result = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| EditorWindow::new(image, reply.take(), path, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    });
    if let Err(e) = result {
        tracing::error!("editor window failed: {e:#}");
        if let Some(r) = reply {
            let _ = r.send(Err(PortError::Failed(format!("editor window: {e:#}"))));
        }
    }
}

pub(crate) fn open_file(path: Option<PathBuf>, cx: &mut App) {
    open(None, None, path, cx);
}

/// Converts a rendered canvas for GPUI on the compute pool (a 4K copy and channel swap).
async fn compute_picture(compute: &ComputeFn, bitmap: Bitmap) -> Option<Arc<RenderImage>> {
    crate::presenters::render_gate::compute(compute, move || to_render_image(&bitmap)).await.flatten()
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

struct EditorWindow {
    /// Input, render gating and snapshot renders (GPUI-free, tested in the presenter).
    interaction: EditorInteraction,
    compute: ComputeFn,
    spawner: Arc<dyn laqta_ports::Spawner>,
    picture: Option<Arc<RenderImage>>,
    chrome: Chrome,
    reply: Option<EditReply>,
    path: Option<PathBuf>,
    focus: FocusHandle,
    message: Option<String>,
    zoom: f32,
    emoji_open: bool,
    /// The stage's bounds from the last frame; the canvas is laid out inside them.
    stage: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The message the dismiss timer is running for.
    toast_for: Option<String>,
}

/// The compute pool as the presenters take it. A panic drops the work's result sender, so the
/// presenter's future resolves to `None` (ADR 020).
pub(crate) fn compute_fn(spawner: Arc<dyn laqta_ports::Spawner>) -> ComputeFn {
    Arc::new(move |work: Box<dyn FnOnce() + Send>| {
        spawner.run_compute(Box::new(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
        }))
    })
}

/// Runs `f` on the compute pool and waits for it. Only for commands and export paths until R1 A2;
/// pointer and key input never use it (`tests/view_guards.rs`).
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

impl EditorWindow {
    fn new(image: Bitmap, reply: Option<EditReply>, path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let services = cx.global::<UiState>().services.clone();
        let editor = Editor::new(image, services.fonts.clone());
        let chrome = editor.chrome();
        let compute = compute_fn(services.spawner.clone());
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut this = EditorWindow {
            interaction: EditorInteraction::new(editor, compute.clone()),
            compute,
            spawner: services.spawner.clone(),
            picture: None,
            chrome,
            reply,
            path,
            focus,
            message: None,
            zoom: 1.0,
            emoji_open: false,
            stage: Rc::new(Cell::new(None)),
            toast_for: None,
        };
        if this.interaction.initial() {
            this.start_render(cx);
        }
        this
    }

    /// Renders a snapshot of the canvas on the compute pool and shows it, without blocking the UI
    /// thread. While a render runs, further requests collapse into one more (`RenderGate`).
    fn start_render(&mut self, cx: &mut Context<Self>) {
        let render = self.interaction.render();
        let compute = self.compute.clone();
        cx.spawn(async move |this, cx| {
            let bitmap = render.await;
            let picture = match bitmap {
                Some(b) => compute_picture(&compute, b).await,
                None => None,
            };
            let _ = this.update(cx, |this, cx| {
                if picture.is_some() {
                    this.picture = picture;
                }
                if this.interaction.render_finished() {
                    this.start_render(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome = step.chrome;
        if step.render_now {
            self.start_render(cx);
        }
        if let Some(r) = step.request {
            self.request(r, window, cx);
        }
        cx.notify();
    }

    /// Pointer, key and text input: the engine runs inline (state math only); rendering is async.
    fn input(&mut self, input: EditorInput, window: &mut Window, cx: &mut Context<Self>) {
        let step = self.interaction.input(input);
        self.apply(step, window, cx);
    }

    /// Commands still run on the pool and wait for it, until R1 A2 moves them behind the facade.
    fn command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        let ed = self.interaction.editor();
        let out = in_pool(&self.spawner, move || ed.lock().unwrap_or_else(|p| p.into_inner()).execute(command));
        match out {
            Some(out) => {
                let step = self.interaction.after_output(out);
                self.apply(step, window, cx);
            }
            None => self.message = Some("The command failed.".into()),
        }
    }

    fn export(&self) -> Option<Bitmap> {
        let ed = self.interaction.editor();
        in_pool(&self.spawner, move || ed.lock().unwrap_or_else(|p| p.into_inner()).export())
    }

    fn encode(&self, cx: &App) -> Option<(Vec<u8>, &'static str)> {
        let img = self.export()?;
        let codecs = cx.global::<UiState>().services.codecs.clone();
        let format = cx.global::<UiState>().services.app.settings().defaults.image.format;
        let opts = laqta_imaging::EncodeOptions { format, jpeg_quality: 90 };
        let bytes = in_pool(&self.spawner, move || codecs.encode(&img, &opts))?.ok()?;
        Some((bytes, format.extension()))
    }

    fn request(&mut self, r: EditorRequest, window: &mut Window, cx: &mut Context<Self>) {
        match r {
            EditorRequest::Save => self.save(false, cx),
            EditorRequest::SaveAs => self.save(true, cx),
            EditorRequest::Continue => self.finish(window),
        }
    }

    fn finish(&mut self, window: &mut Window) {
        if let Some(r) = self.reply.take() {
            match self.export() {
                Some(img) => {
                    let _ = r.send(Ok(img));
                }
                None => {
                    let _ = r.send(Err(PortError::Failed("export failed".into())));
                }
            }
        }
        window.remove_window();
    }

    fn save(&mut self, ask: bool, cx: &mut Context<Self>) {
        let Some((bytes, ext)) = self.encode(cx) else {
            self.message = Some("Couldn't encode the image.".into());
            return;
        };
        let settings = cx.global::<UiState>().services.app.settings();
        let default_dir = settings.screenshots_dir.clone();
        let current = self.path.clone();
        let task = cx.background_spawn(async move {
            let target = match (ask, current) {
                (false, Some(p)) => Some(p),
                _ => {
                    let name = format!("Laqta_edit.{ext}");
                    rfd::AsyncFileDialog::new()
                        .set_directory(&default_dir)
                        .set_file_name(name)
                        .save_file()
                        .await
                        .map(|f| f.path().to_path_buf())
                }
            };
            match target {
                Some(p) => std::fs::write(&p, &bytes).map(|_| Some(p)).map_err(|e| e.to_string()),
                None => Ok(None),
            }
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                this.message = Some(match r {
                    Ok(Some(p)) => {
                        let m = format!("Saved {}", p.display());
                        this.path = Some(p);
                        m
                    }
                    Ok(None) => return,
                    Err(e) => format!("Save failed: {e}"),
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn png(&self, cx: &App) -> Option<Vec<u8>> {
        let img = self.export()?;
        let codecs = cx.global::<UiState>().services.codecs.clone();
        let opts = laqta_imaging::EncodeOptions { format: ImageFormat::Png, jpeg_quality: 90 };
        in_pool(&self.spawner, move || codecs.encode(&img, &opts)).and_then(Result::ok)
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        self.message = Some(match self.png(cx) {
            Some(bytes) => {
                cx.write_to_clipboard(ClipboardItem::new_image(&gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes)));
                "Copied to the clipboard.".into()
            }
            None => "Copy failed.".into(),
        });
        cx.notify();
    }

    /// Saves a temporary PNG and hands it to Task as a file upload.
    fn upload(&mut self, cx: &mut Context<Self>) {
        let Some(bytes) = self.png(cx) else { return };
        let services = cx.global::<UiState>().services.clone();
        let dir = services.data_dir.join("edits");
        let path = dir.join(format!("Laqta_edit_{}.png", std::process::id()));
        let written = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, bytes));
        self.message = Some(match written {
            Ok(()) => {
                let ids = services.app.upload_files(vec![path], None);
                format!("Uploading ({} task).", ids.len())
            }
            Err(e) => format!("Upload failed: {e}"),
        });
        cx.notify();
    }

    fn pick_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let codecs = cx.global::<UiState>().services.codecs.clone();
        let task = cx.background_spawn(async move {
            let f = rfd::AsyncFileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp"])
                .pick_file()
                .await?;
            let bytes = std::fs::read(f.path()).ok()?;
            codecs.decode(&bytes).ok()
        });
        let _ = window;
        cx.spawn_in(window, async move |this, cx| {
            if let Some(bmp) = task.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.command(Command::SetToolImage(bmp), window, cx);
                    this.command(Command::SetTool(Tool::Image), window, cx);
                });
            }
        })
        .detach();
    }

    fn emoji(&mut self, e: &str, window: &mut Window, cx: &mut Context<Self>) {
        let fonts = cx.global::<UiState>().services.fonts.clone();
        let font = laqta_editor::FontStyle { size: 64.0, ..laqta_editor::FontStyle::default() };
        let text = e.to_string();
        let bmp = in_pool(&self.spawner, move || fonts.render_text(&text, &font)).flatten();
        match bmp {
            Some(b) => {
                self.command(Command::SetToolImage(b), window, cx);
                self.command(Command::SetTool(Tool::Emoji), window, cx);
            }
            None => self.message = Some("No emoji font is installed.".into()),
        }
        self.emoji_open = false;
        cx.notify();
    }

    fn set_style(&mut self, f: impl FnOnce(&mut laqta_editor::Style), window: &mut Window, cx: &mut Context<Self>) {
        let mut style = self.chrome.style.clone();
        f(&mut style);
        self.command(Command::SetStyle(style), window, cx);
    }

    /// The stage's origin in the window and its size. Before the first frame, an estimate.
    fn stage_rect(&self, window: &Window) -> (f32, f32, f32, f32) {
        if let Some(b) = self.stage.get() {
            return (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
        }
        let vp = window.viewport_size();
        let top = f32::from(chrome::TITLE_BAR_H) + TOPBAR_H;
        (RAIL_W, top, f32::from(vp.width) - RAIL_W - GUTTER, f32::from(vp.height) - top - FOOTER_H)
    }

    /// Window position → image pixels.
    fn to_image(&self, pos: gpui::Point<Pixels>, window: &Window) -> (f32, f32) {
        let (sx, sy, _, _) = self.stage_rect(window);
        let (ox, oy, z) = self.layout(window);
        ((f32::from(pos.x) - sx - ox) / z, (f32::from(pos.y) - sy - oy) / z)
    }

    /// The canvas origin inside the stage and the zoom: fit to the stage, times the user's zoom.
    fn layout(&self, window: &Window) -> (f32, f32, f32) {
        let (_, _, w, h) = self.stage_rect(window);
        let (cw, ch) = (self.chrome.canvas.0.max(1) as f32, self.chrome.canvas.1.max(1) as f32);
        let avail_w = (w - 2.0 * PAD).max(1.0);
        let avail_h = (h - PAD - PAD_BOTTOM).max(1.0);
        let fit = (avail_w / cw).min(avail_h / ch).min(1.0);
        let z = (fit * self.zoom).max(0.05);
        let ox = PAD + (avail_w - cw * z).max(0.0) / 2.0;
        let oy = PAD + (avail_h - ch * z).max(0.0) / 2.0;
        (ox, oy, z)
    }

    fn pick_tool(&mut self, t: Tool, window: &mut Window, cx: &mut Context<Self>) {
        match t {
            Tool::Image => self.pick_image(window, cx),
            Tool::Emoji => {
                self.emoji_open = !self.emoji_open;
                cx.notify();
            }
            _ => self.select_tool(t, window, cx),
        }
    }

    /// Switches tools on the UI thread: `SetTool` only changes engine state, so it needs neither
    /// the compute pool nor a wait, and a tool key never blocks the window.
    fn select_tool(&mut self, t: Tool, window: &mut Window, cx: &mut Context<Self>) {
        self.emoji_open = false;
        let ed = self.interaction.editor();
        let out = ed.lock().unwrap_or_else(|p| p.into_inner()).execute(Command::SetTool(t));
        let step = self.interaction.after_output(out);
        self.apply(step, window, cx);
    }

    /// Clears a message a few seconds after it appears.
    fn dismiss_later(&mut self, cx: &mut Context<Self>) {
        if self.message == self.toast_for {
            return;
        }
        self.toast_for = self.message.clone();
        let Some(shown) = self.message.clone() else { return };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(3200)).await;
            let _ = this.update(cx, |this, cx| {
                if this.message.as_deref() == Some(shown.as_str()) {
                    this.message = None;
                    this.toast_for = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

fn mods(m: &gpui::Modifiers) -> Mods {
    Mods { shift: m.shift, ctrl: m.control || m.platform, alt: m.alt }
}

fn button(b: MouseButton) -> Option<PointerButton> {
    match b {
        MouseButton::Left => Some(PointerButton::Left),
        MouseButton::Right => Some(PointerButton::Right),
        MouseButton::Middle => Some(PointerButton::Middle),
        _ => None,
    }
}

fn editor_key(key: &str) -> Option<EditorKey> {
    Some(match key {
        "delete" => EditorKey::Delete,
        "backspace" => EditorKey::Backspace,
        "enter" => EditorKey::Enter,
        "escape" => EditorKey::Escape,
        "left" => EditorKey::Left,
        "right" => EditorKey::Right,
        "up" => EditorKey::Up,
        "down" => EditorKey::Down,
        "home" => EditorKey::Home,
        "end" => EditorKey::End,
        "pageup" => EditorKey::PageUp,
        "pagedown" => EditorKey::PageDown,
        "tab" => EditorKey::Tab,
        k if k.chars().count() == 1 => EditorKey::Char(k.chars().next().unwrap_or(' ').to_ascii_lowercase()),
        _ => return None,
    })
}

fn tool_icon(t: Tool) -> IconName {
    match t {
        Tool::Select => IconName::MousePointer2,
        Tool::Rectangle => IconName::Square,
        Tool::Ellipse => IconName::Circle,
        Tool::Line => IconName::Slash,
        Tool::Arrow => IconName::MoveUpRight,
        Tool::Freehand => IconName::PenLine,
        Tool::Text => IconName::Type,
        Tool::SpeechBalloon => IconName::MessageSquare,
        Tool::Step => IconName::ListOrdered,
        Tool::Image => IconName::ImagePlus,
        Tool::Clone => IconName::Stamp,
        Tool::Emoji => IconName::FaceSlightlySmiling,
        Tool::Cursor => IconName::MousePointer,
        Tool::Highlight => IconName::Highlighter,
        Tool::SmartEraser => IconName::Eraser,
        Tool::Blur => IconName::Droplet,
        Tool::Pixelate => IconName::Grid3x3,
        Tool::Magnify => IconName::ZoomIn,
        Tool::Spotlight => IconName::Flashlight,
        Tool::Crop => IconName::Crop,
        Tool::CutOut => IconName::Scissors,
    }
}

/// "Rectangle  R" for a tooltip.
fn tool_tip(t: Tool) -> String {
    let name = editor_toolbar::label(t);
    match editor_toolbar::shortcut(t) {
        Some(k) => format!("{name}  ·  {}", k.to_ascii_uppercase()),
        None => name.to_string(),
    }
}

fn rgba(c: Rgba) -> gpui::Hsla {
    let mut h: gpui::Hsla = gpui::rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32).into();
    h.a = c.a as f32 / 255.0;
    h
}

fn abs(x: f32, y: f32, w: f32, h: f32) -> Div {
    div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h))
}

/// Keeps a click on a floating control from reaching the canvas under it.
fn stop_canvas(el: Div) -> Div {
    el.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

impl EditorWindow {
    fn rail(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let hidden = cx.global::<UiState>().services.app.settings().editor_hidden_tools;
        let active = self.chrome.tool;
        let emoji_open = self.emoji_open;
        let mut rail = div()
            .id("tool-rail")
            .w(px(RAIL_W))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .pt(px((TOPBAR_H - 34.0) / 2.0))
            .pb(space::S3)
            .overflow_y_scroll();
        for (gi, group) in editor_toolbar::groups(&hidden).into_iter().enumerate() {
            if gi > 0 {
                rail = rail.child(widgets::divider_h(cx).w(px(56.)));
            }
            let mut grid = div().w(px(72.)).flex().flex_wrap().gap(px(4.));
            for t in group.tools {
                let on = t == active || (t == Tool::Emoji && emoji_open);
                grid = grid.child(widgets::icon_button(
                    SharedString::from(format!("tool-{}", t.name())),
                    tool_icon(t),
                    tool_tip(t),
                    on,
                    cx.listener(move |this, _, window, cx| this.pick_tool(t, window, cx)),
                    cx,
                ));
            }
            rail = rail.child(grid);
        }
        let entity = cx.entity().downgrade();
        let customise =
            Button::new("customise-tools").icon(Icon::new(IconName::Settings2)).ghost().tooltip("Show or hide tools").dropdown_menu(
                move |mut menu, _, _| {
                    menu = menu.item(PopupMenuItem::label("Show on the rail"));
                    for t in editor_toolbar::hideable() {
                        let shown = !hidden.iter().any(|h| h == t.name());
                        let entity = entity.clone();
                        menu = menu.item(PopupMenuItem::new(editor_toolbar::label(t)).checked(shown).on_click(move |_, _, cx| {
                            let services = cx.global::<UiState>().services.clone();
                            let mut s = services.app.settings();
                            if shown {
                                s.editor_hidden_tools.push(t.name().to_string());
                            } else {
                                s.editor_hidden_tools.retain(|h| h != t.name());
                            }
                            let _ = (services.save_settings)(s);
                            let _ = entity.update(cx, |_, cx| cx.notify());
                        }));
                    }
                    menu
                },
            );
        rail.child(div().flex_1().min_h(space::S3)).child(customise)
    }

    fn topbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let chrome = &self.chrome;
        let controls = editor_toolbar::style_controls(chrome.tool);
        let undo = widgets::icon_button(
            "undo",
            IconName::Undo2,
            "Undo  ·  Ctrl+Z",
            false,
            cx.listener(|this, _, w, cx| {
                if this.chrome.can_undo {
                    this.command(Command::Undo, w, cx)
                }
            }),
            cx,
        )
        .when(!chrome.can_undo, |d| d.opacity(0.35));
        let redo = widgets::icon_button(
            "redo",
            IconName::Redo2,
            "Redo  ·  Ctrl+Y",
            false,
            cx.listener(|this, _, w, cx| {
                if this.chrome.can_redo {
                    this.command(Command::Redo, w, cx)
                }
            }),
            cx,
        )
        .when(!chrome.can_redo, |d| d.opacity(0.35));

        let mut props = div().flex_1().min_w_0().h_full().flex().items_center().gap(space::S2).overflow_hidden().child(
            div()
                .flex_none()
                .pr(space::S1)
                .text_size(text::LABEL)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(editor_toolbar::label(chrome.tool)),
        );
        if controls.color {
            let mut track =
                div().flex().flex_none().items_center().gap(px(1.)).p(px(3.)).rounded(radius::PILL).bg(cx.theme().foreground.opacity(0.06));
            for (i, c) in PALETTE.into_iter().enumerate() {
                track = track.child(widgets::swatch(
                    ("col", i),
                    rgba(c),
                    chrome.style.border_color == c,
                    cx.listener(move |this, _, window, cx| this.set_style(|s| s.border_color = c, window, cx)),
                    cx,
                ));
            }
            props = props.child(track).child(widgets::divider_v(cx));
        }
        if controls.stroke {
            props = props.child(widgets::stepper(
                "stroke",
                "Width",
                "Line width",
                format!("{:.0} px", chrome.style.border_size),
                cx.listener(|this, _, w, cx| this.set_style(|s| s.border_size = (s.border_size - 1.0).max(1.0), w, cx)),
                cx.listener(|this, _, w, cx| this.set_style(|s| s.border_size = (s.border_size + 1.0).min(40.0), w, cx)),
                cx,
            ));
        }
        if controls.font {
            props = props.child(widgets::stepper(
                "font",
                "Size",
                "Text size",
                format!("{:.0}", chrome.style.font.size),
                cx.listener(|this, _, w, cx| this.set_style(|s| s.font.size = (s.font.size - 2.0).max(8.0), w, cx)),
                cx.listener(|this, _, w, cx| this.set_style(|s| s.font.size = (s.font.size + 2.0).min(200.0), w, cx)),
                cx,
            ));
        }
        if controls.fill || controls.shadow {
            props = props.child(widgets::divider_v(cx));
        }
        if controls.fill {
            let fill_on = chrome.style.fill_color.a > 0;
            props = props.child(widgets::toggle_chip(
                "fill",
                IconName::PaintBucket,
                "Fill",
                fill_on,
                cx.listener(|this, _, w, cx| {
                    this.set_style(
                        |s| s.fill_color = if s.fill_color.a > 0 { Rgba::TRANSPARENT } else { Rgba { a: 96, ..s.border_color } },
                        w,
                        cx,
                    )
                }),
                cx,
            ));
        }
        if controls.shadow {
            let on = chrome.style.shadow;
            props = props.child(widgets::toggle_chip(
                "shadow",
                IconName::Blend,
                "Shadow",
                on,
                cx.listener(|this, _, w, cx| this.set_style(|s| s.shadow = !s.shadow, w, cx)),
                cx,
            ));
        }
        if controls == editor_toolbar::StyleControls::default() {
            props = props.child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .text_size(text::LABEL)
                    .text_color(cx.theme().muted_foreground)
                    .child(div().truncate().child(editor_toolbar::hint(chrome.tool))),
            );
        }

        div()
            .h(px(TOPBAR_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(space::S1)
            .pr(px(GUTTER))
            .child(undo)
            .child(redo)
            .child(widgets::divider_v(cx).mx(space::S2))
            .child(props)
            .child(self.image_menu(cx))
    }

    /// Rotate, flip, auto crop and the effect presets: things done to the whole image.
    fn image_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let presets = cx.global::<UiState>().services.app.settings().effect_presets;
        let entity = cx.entity().downgrade();
        Button::new("image-menu").label("Image").icon(Icon::new(IconName::WandSparkles)).ghost().dropdown_caret(true).dropdown_menu(
            move |mut menu, _, _| {
                let run = |c: Command| {
                    let entity = entity.clone();
                    move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                        let c = c.clone();
                        let _ = entity.update(cx, |this, cx| this.command(c, window, cx));
                    }
                };
                menu = menu
                    .item(
                        PopupMenuItem::new("Rotate left")
                            .icon(Icon::new(IconName::RotateCcw))
                            .on_click(run(Command::Rotate90 { clockwise: false })),
                    )
                    .item(
                        PopupMenuItem::new("Rotate right")
                            .icon(Icon::new(IconName::RotateCw))
                            .on_click(run(Command::Rotate90 { clockwise: true })),
                    )
                    .item(
                        PopupMenuItem::new("Flip horizontally")
                            .icon(Icon::new(IconName::FlipHorizontal2))
                            .on_click(run(Command::Flip { horizontal: true })),
                    )
                    .item(
                        PopupMenuItem::new("Flip vertically")
                            .icon(Icon::new(IconName::FlipVertical2))
                            .on_click(run(Command::Flip { horizontal: false })),
                    )
                    .item(PopupMenuItem::new("Auto crop").icon(Icon::new(IconName::Scan)).on_click(run(Command::AutoCrop)))
                    .separator()
                    .item(PopupMenuItem::label("Apply an effect preset"));
                if presets.is_empty() {
                    menu = menu.item(PopupMenuItem::new("No presets yet").disabled(true));
                }
                for (i, p) in presets.iter().enumerate() {
                    let name = if p.name.is_empty() { format!("Preset {}", i + 1) } else { p.name.clone() };
                    menu = menu
                        .item(PopupMenuItem::new(name).icon(Icon::new(IconName::Sparkles)).on_click(run(Command::ApplyEffect(p.clone()))));
                }
                menu
            },
        )
    }

    fn footer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let chrome = &self.chrome;
        let t = cx.theme();
        let n = chrome.annotation_count;
        let status =
            format!("{} × {} px  ·  {} {}", chrome.canvas.0, chrome.canvas.1, n, if n == 1 { "annotation" } else { "annotations" });
        let continuing = self.reply.is_some();
        let mut actions = div()
            .flex()
            .items_center()
            .gap(space::S2)
            .child(widgets::pill("copy", "Copy", widgets::Pill::Default, false, cx.listener(|this, _, _, cx| this.copy(cx)), cx))
            .child(widgets::pill("upload", "Upload", widgets::Pill::Default, false, cx.listener(|this, _, _, cx| this.upload(cx)), cx))
            .child(widgets::pill("saveas", "Save as…", widgets::Pill::Default, false, cx.listener(|this, _, _, cx| this.save(true, cx)), cx));
        actions = if continuing {
            actions
                .child(widgets::pill("save", "Save", widgets::Pill::Default, false, cx.listener(|this, _, _, cx| this.save(false, cx)), cx))
                .child(widgets::pill("continue", "Done", widgets::Pill::Primary, false, cx.listener(|this, _, w, _| this.finish(w)), cx))
        } else {
            actions.child(widgets::pill(
                "save",
                "Save",
                widgets::Pill::Primary,
                false,
                cx.listener(|this, _, _, cx| this.save(false, cx)),
                cx,
            ))
        };
        div()
            .h(px(FOOTER_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(space::S3)
            .pr(px(GUTTER + 6.0))
            .pl(space::S1)
            .child(div().flex_1().min_w_0().truncate().text_size(text::LABEL).text_color(t.muted_foreground).child(status))
            .child(actions)
    }

    fn stage(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (ox, oy, z) = self.layout(window);
        let (cw, ch) = (self.chrome.canvas.0 as f32 * z, self.chrome.canvas.1 as f32 * z);
        let chrome = &self.chrome;
        let t = cx.theme().clone();
        let x = theme::extra(cx);

        // Records the stage's bounds for the next frame's layout and pointer mapping.
        let cell = self.stage.clone();
        let tracker = gpui::canvas(
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

        let mut canvas = div().absolute().left(px(ox)).top(px(oy)).w(px(cw)).h(px(ch)).bg(t.secondary).shadow(vec![gpui::BoxShadow {
            color: gpui::black().opacity(if t.is_dark() { 0.55 } else { 0.18 }),
            offset: gpui::point(px(0.), px(12.)),
            blur_radius: px(40.),
            spread_radius: px(0.),
            inset: false,
        }]);
        if let Some(p) = &self.picture {
            canvas = canvas.child(img(p.clone()).absolute().left_0().top_0().w(px(cw)).h(px(ch)));
        }
        let sel = rgba(Rgba::rgb(0x3b, 0x82, 0xf6));
        if let Some(r) = chrome.selection {
            canvas = canvas.child(abs(r.x * z, r.y * z, r.w * z, r.h * z).border_1().border_color(sel));
        }
        if let Some(r) = chrome.pending {
            canvas = canvas.child(
                abs(r.x * z, r.y * z, r.w * z, r.h * z)
                    .border_1()
                    .border_color(sel)
                    .bg(rgba(Rgba { a: 40, ..Rgba::rgb(0x3b, 0x82, 0xf6) })),
            );
        }
        for (_, p) in &chrome.handles {
            canvas =
                canvas.child(abs(p.x * z - 4.5, p.y * z - 4.5, 9.0, 9.0).rounded(px(2.)).bg(gpui::white()).border_1().border_color(sel));
        }
        if let Some(c) = chrome.caret {
            canvas = canvas.child(abs(c.x * z, c.y * z, 2.0, chrome.style.font.size * z).bg(t.foreground));
        }

        let zoom = stop_canvas(widgets::floating(cx))
            .absolute()
            .right(space::S4)
            .bottom(space::S4)
            .child(widgets::icon_button(
                "zoom-out",
                IconName::ZoomOut,
                "Zoom the view out",
                false,
                cx.listener(|this, _, _, cx| {
                    this.zoom = (this.zoom / 1.25).max(0.1);
                    cx.notify();
                }),
                cx,
            ))
            .child(
                div()
                    .min_w(px(46.))
                    .text_center()
                    .text_size(px(12.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(format!("{:.0}%", z * 100.0)),
            )
            .child(widgets::icon_button(
                "zoom-in",
                IconName::ZoomIn,
                "Zoom the view in",
                false,
                cx.listener(|this, _, _, cx| {
                    this.zoom = (this.zoom * 1.25).min(8.0);
                    cx.notify();
                }),
                cx,
            ))
            .child(widgets::divider_v(cx))
            .child(widgets::icon_button(
                "zoom-fit",
                IconName::Maximize,
                "Fit to window",
                self.zoom == 1.0,
                cx.listener(|this, _, _, cx| {
                    this.zoom = 1.0;
                    cx.notify();
                }),
                cx,
            ));

        let mut stage = div()
            .id("stage")
            .relative()
            .flex_1()
            .min_h_0()
            .mr(px(GUTTER))
            .rounded(radius::LG)
            .bg(x.stage)
            .overflow_hidden()
            .child(tracker)
            .child(canvas)
            .child(zoom)
            .on_any_mouse_down(cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                if let Some(b) = button(ev.button) {
                    window.focus(&this.focus, cx);
                    let (x, y) = this.to_image(ev.position, window);
                    this.input(EditorInput::PointerDown { x, y, button: b, mods: mods(&ev.modifiers) }, window, cx);
                }
            }));

        if self.emoji_open {
            let mut grid = div().w(px(6. * 40.)).flex().flex_wrap().gap(px(4.));
            for (i, e) in EMOJI.into_iter().enumerate() {
                let wash = x.wash;
                grid = grid.child(
                    div()
                        .id(("emoji", i))
                        .size(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius::MD)
                        .text_size(px(20.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(wash))
                        .child(e)
                        .on_click(cx.listener(move |this, _, w, cx| this.emoji(e, w, cx))),
                );
            }
            let panel = stop_canvas(div())
                .absolute()
                .left(space::S4)
                .top(space::S4)
                .p(space::S3)
                .flex()
                .flex_col()
                .gap(space::S2)
                .rounded(radius::XL)
                .bg(t.sidebar)
                .border_1()
                .border_color(t.foreground.opacity(0.1))
                .child(
                    div()
                        .text_size(text::CAPTION)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(t.muted_foreground)
                        .child("Pick an emoji"),
                )
                .child(grid);
            stage = stage.child(panel);
        }

        if let Some(m) = self.message.clone() {
            self.dismiss_later(cx);
            stage = stage.child(
                div().absolute().top(space::S4).left_0().right_0().flex().justify_center().child(
                    widgets::floating(cx)
                        .px(space::S4)
                        .py(space::S2)
                        .gap(space::S2)
                        .text_size(text::LABEL)
                        .child(widgets::icon(IconName::Info, t.muted_foreground))
                        .child(m),
                ),
            );
        }
        stage
    }
}

impl Render for EditorWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let title = match &self.path {
            Some(p) => p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Editor".into()),
            None => "Editor".to_string(),
        };
        let rail = self.rail(cx);
        let topbar = self.topbar(cx);
        let stage = self.stage(window, cx);
        let footer = self.footer(cx);

        chrome::rounded(div(), window)
            .id("laqta-editor")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(t.sidebar)
            .text_color(t.foreground)
            .font_family(text::UI_FONT)
            .text_size(text::BODY)
            .child(chrome::TitleBar::new().title(title))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(rail)
                    .child(div().flex_1().min_w_0().flex().flex_col().child(topbar).child(stage).child(footer)),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                let (x, y) = this.to_image(ev.position, window);
                this.input(EditorInput::PointerMove { x, y, mods: mods(&ev.modifiers) }, window, cx);
            }))
            .capture_any_mouse_up(cx.listener(|this, ev: &MouseUpEvent, window, cx| {
                if let Some(b) = button(ev.button) {
                    let (x, y) = this.to_image(ev.position, window);
                    this.input(EditorInput::PointerUp { x, y, button: b, mods: mods(&ev.modifiers) }, window, cx);
                }
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                let m = mods(&ev.keystroke.modifiers);
                if this.chrome.editing_text
                    && !m.ctrl
                    && let Some(ch) = ev.keystroke.key_char.clone().filter(|c| !c.is_empty() && !c.chars().any(char::is_control))
                {
                    this.input(EditorInput::Text(ch), window, cx);
                    return;
                }
                // Single-key tool shortcuts, when no text is being typed.
                let key = ev.keystroke.key.as_str();
                if !this.chrome.editing_text
                    && !m.ctrl
                    && !m.alt
                    && !m.shift
                    && key.chars().count() == 1
                    && let Some(t) = key.chars().next().and_then(editor_toolbar::tool_for_key)
                {
                    this.select_tool(t, window, cx);
                    return;
                }
                if let Some(key) = editor_key(key) {
                    this.input(EditorInput::Key { key, mods: m }, window, cx);
                }
            }))
    }
}
