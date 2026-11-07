//! The annotation editor window (M3). It forwards input to the Editor engine, paints the engine's
//! rendered pixels and chrome, and computes nothing itself (ARCHITECTURE §5 rule). The engine runs
//! on the compute pool for every call, since some commands call imaging kernels.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures_channel::oneshot;
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Div, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render, RenderImage, SharedString, StatefulInteractiveElement as _, Styled as _,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, img, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme as _, Disableable as _, Root, Selectable as _, Sizable as _};
use laqta_domain::color::Rgba;
use laqta_domain::{Bitmap, ImageFormat, PixelFormat};
use laqta_editor::{Chrome, Command, Editor, EditorInput, EditorKey, EditorRequest, Mods, PointerButton, Tool};
use laqta_ports::PortError;
use smallvec::SmallVec;

use crate::runtime::UiState;

pub(crate) type EditReply = oneshot::Sender<Result<Bitmap, PortError>>;

const TOOLBAR_H: f32 = 84.0;
const FOOTER_H: f32 = 52.0;
const PAD: f32 = 12.0;

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
        titlebar: Some(TitlebarOptions { title: Some("Laqta editor".into()), ..Default::default() }),
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
    editor: Arc<Mutex<Editor>>,
    spawner: Arc<dyn laqta_ports::Spawner>,
    picture: Option<Arc<RenderImage>>,
    chrome: Chrome,
    reply: Option<EditReply>,
    path: Option<PathBuf>,
    focus: FocusHandle,
    message: Option<String>,
    zoom: f32,
    emoji_open: bool,
    customising: bool,
}

/// Runs `f` on the compute pool and waits for it (editor operations take milliseconds).
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
        let editor = Arc::new(Mutex::new(Editor::new(image, services.fonts.clone())));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let chrome = editor.lock().map(|e| e.chrome()).unwrap_or_else(|p| p.into_inner().chrome());
        let mut this = EditorWindow {
            editor,
            spawner: services.spawner.clone(),
            picture: None,
            chrome,
            reply,
            path,
            focus,
            message: None,
            zoom: 1.0,
            emoji_open: false,
            customising: false,
        };
        this.refresh();
        this
    }

    /// Re-renders the whole canvas and the chrome.
    fn refresh(&mut self) {
        let ed = self.editor.clone();
        if let Some((bmp, chrome)) = in_pool(&self.spawner, move || {
            let e = ed.lock().unwrap_or_else(|p| p.into_inner());
            (e.export(), e.chrome())
        }) {
            self.picture = to_render_image(&bmp);
            self.chrome = chrome;
        }
    }

    fn run(&mut self, f: impl FnOnce(&mut Editor) -> Option<EditorRequest> + Send + 'static, window: &mut Window, cx: &mut Context<Self>) {
        let ed = self.editor.clone();
        let request = in_pool(&self.spawner, move || {
            let mut e = ed.lock().unwrap_or_else(|p| p.into_inner());
            f(&mut e)
        })
        .flatten();
        self.refresh();
        if let Some(r) = request {
            self.request(r, window, cx);
        }
        cx.notify();
    }

    fn input(&mut self, input: EditorInput, window: &mut Window, cx: &mut Context<Self>) {
        self.run(move |e| e.handle(input).request, window, cx);
    }

    fn command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        self.run(move |e| e.execute(command).request, window, cx);
    }

    fn export(&self) -> Option<Bitmap> {
        let ed = self.editor.clone();
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
                    rfd::AsyncFileDialog::new().set_directory(&default_dir).set_file_name(name).save_file().await.map(|f| f.path().to_path_buf())
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
            let f = rfd::AsyncFileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp"]).pick_file().await?;
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

    /// Window position → image pixels.
    fn to_image(&self, pos: gpui::Point<Pixels>, window: &Window) -> (f32, f32) {
        let (ox, oy, z) = self.layout(window);
        ((f32::from(pos.x) - ox) / z, (f32::from(pos.y) - oy) / z)
    }

    /// The canvas origin in the window and the zoom that fits it.
    fn layout(&self, window: &Window) -> (f32, f32, f32) {
        let vp = window.viewport_size();
        let (w, h) = (f32::from(vp.width), f32::from(vp.height));
        let (cw, ch) = (self.chrome.canvas.0.max(1) as f32, self.chrome.canvas.1.max(1) as f32);
        let avail_w = (w - 2.0 * PAD).max(1.0);
        let avail_h = (h - TOOLBAR_H - FOOTER_H - 2.0 * PAD).max(1.0);
        let fit = (avail_w / cw).min(avail_h / ch).min(1.0);
        let z = (fit * self.zoom).max(0.05);
        let ox = PAD + (avail_w - cw * z).max(0.0) / 2.0;
        let oy = TOOLBAR_H + PAD + (avail_h - ch * z).max(0.0) / 2.0;
        (ox, oy, z)
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

fn tool_label(t: Tool) -> &'static str {
    match t {
        Tool::Select => "Select",
        Tool::Rectangle => "Rect",
        Tool::Ellipse => "Ellipse",
        Tool::Line => "Line",
        Tool::Arrow => "Arrow",
        Tool::Freehand => "Pen",
        Tool::Text => "Text",
        Tool::SpeechBalloon => "Balloon",
        Tool::Step => "Step",
        Tool::Image => "Image",
        Tool::Clone => "Clone",
        Tool::Emoji => "Emoji",
        Tool::Cursor => "Cursor",
        Tool::Highlight => "Highlight",
        Tool::SmartEraser => "Eraser",
        Tool::Blur => "Blur",
        Tool::Pixelate => "Pixelate",
        Tool::Magnify => "Magnify",
        Tool::Spotlight => "Spotlight",
        Tool::Crop => "Crop",
        Tool::CutOut => "Cut out",
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

impl Render for EditorWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (ox, oy, z) = self.layout(window);
        let (cw, ch) = (self.chrome.canvas.0 as f32 * z, self.chrome.canvas.1 as f32 * z);
        let chrome = self.chrome.clone();

        // Tools (the user can hide some; the gear toggles customisation).
        let hidden = cx.global::<UiState>().services.app.settings().editor_hidden_tools;
        let customising = self.customising;
        let mut tools = div().flex().flex_row().flex_wrap().gap(px(2.));
        for (i, t) in Tool::ALL.into_iter().enumerate() {
            let is_hidden = hidden.iter().any(|h| h == t.name());
            if customising {
                tools = tools.child(Button::new(SharedString::from(format!("vis-{i}"))).label(tool_label(t)).small().selected(!is_hidden).on_click(cx.listener(
                    move |_, _, _, cx| {
                        let services = cx.global::<UiState>().services.clone();
                        let mut s = services.app.settings();
                        if is_hidden {
                            s.editor_hidden_tools.retain(|h| h != t.name());
                        } else {
                            s.editor_hidden_tools.push(t.name().to_string());
                        }
                        let _ = (services.save_settings)(s);
                        cx.notify();
                    },
                )));
                continue;
            }
            if is_hidden {
                continue;
            }
            tools = tools.child(Button::new(SharedString::from(format!("tool-{i}"))).label(tool_label(t)).small().selected(chrome.tool == t).on_click(cx.listener(
                move |this, _, window, cx| match t {
                    Tool::Image => this.pick_image(window, cx),
                    Tool::Emoji => {
                        this.emoji_open = !this.emoji_open;
                        cx.notify();
                    }
                    _ => this.command(Command::SetTool(t), window, cx),
                },
            )));
        }
        // Style: colour, size, fill.
        let mut colors = div().flex().flex_row().gap(px(3.)).items_center();
        for (i, c) in PALETTE.into_iter().enumerate() {
            let selected = chrome.style.border_color == c;
            colors = colors.child(
                div()
                    .id(("col", i))
                    .w(px(18.))
                    .h(px(18.))
                    .rounded(px(9.))
                    .bg(rgba(c))
                    .border_2()
                    .border_color(if selected { cx.theme().foreground } else { cx.theme().border })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| this.set_style(|s| s.border_color = c, window, cx))),
            );
        }
        let size_label = format!("{:.0}px", chrome.style.border_size);
        let fill_on = chrome.style.fill_color.a > 0;
        let style_bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .child(colors)
            .child(Button::new("thinner").label("−").small().ghost().on_click(cx.listener(|this, _, w, cx| {
                this.set_style(|s| s.border_size = (s.border_size - 1.0).max(1.0), w, cx)
            })))
            .child(div().text_xs().child(size_label))
            .child(Button::new("thicker").label("+").small().ghost().on_click(cx.listener(|this, _, w, cx| {
                this.set_style(|s| s.border_size = (s.border_size + 1.0).min(40.0), w, cx)
            })))
            .child(Button::new("fill").label("Fill").small().selected(fill_on).on_click(cx.listener(|this, _, w, cx| {
                this.set_style(
                    |s| {
                        s.fill_color = if s.fill_color.a > 0 { Rgba::TRANSPARENT } else { Rgba { a: 96, ..s.border_color } };
                    },
                    w,
                    cx,
                )
            })))
            .child(Button::new("shadow").label("Shadow").small().selected(chrome.style.shadow).on_click(cx.listener(|this, _, w, cx| {
                this.set_style(|s| s.shadow = !s.shadow, w, cx)
            })))
            .child(Button::new("font-").label("A−").small().ghost().on_click(cx.listener(|this, _, w, cx| {
                this.set_style(|s| s.font.size = (s.font.size - 2.0).max(8.0), w, cx)
            })))
            .child(Button::new("font+").label("A+").small().ghost().on_click(cx.listener(|this, _, w, cx| {
                this.set_style(|s| s.font.size = (s.font.size + 2.0).min(200.0), w, cx)
            })))
            .child(div().w(px(12.)))
            .child(Button::new("undo").label("Undo").small().disabled(!chrome.can_undo).on_click(cx.listener(|this, _, w, cx| this.command(Command::Undo, w, cx))))
            .child(Button::new("redo").label("Redo").small().disabled(!chrome.can_redo).on_click(cx.listener(|this, _, w, cx| this.command(Command::Redo, w, cx))))
            .child(Button::new("rotl").label("⟲").small().ghost().on_click(cx.listener(|this, _, w, cx| this.command(Command::Rotate90 { clockwise: false }, w, cx))))
            .child(Button::new("rotr").label("⟳").small().ghost().on_click(cx.listener(|this, _, w, cx| this.command(Command::Rotate90 { clockwise: true }, w, cx))))
            .child(Button::new("flip").label("Flip").small().ghost().on_click(cx.listener(|this, _, w, cx| this.command(Command::Flip { horizontal: true }, w, cx))))
            .child(Button::new("autocrop").label("Auto crop").small().ghost().on_click(cx.listener(|this, _, w, cx| this.command(Command::AutoCrop, w, cx))))
            .child(self.effects_menu(cx))
            .child(div().flex_1())
            .child(Button::new("customise").label(if self.customising { "Done" } else { "Customise" }).small().ghost().on_click(cx.listener(|this, _, _, cx| {
                this.customising = !this.customising;
                cx.notify();
            })))
            .child(Button::new("zoom-").label("−").small().ghost().on_click(cx.listener(|this, _, _, cx| {
                this.zoom = (this.zoom / 1.25).max(0.1);
                cx.notify();
            })))
            .child(div().text_xs().child(format!("{:.0}%", z * 100.0)))
            .child(Button::new("zoom+").label("+").small().ghost().on_click(cx.listener(|this, _, _, cx| {
                this.zoom = (this.zoom * 1.25).min(8.0);
                cx.notify();
            })));

        let mut toolbar = div().flex().flex_col().gap(px(4.)).p(px(6.)).h(px(TOOLBAR_H)).border_b_1().border_color(cx.theme().border).child(tools).child(style_bar);
        if self.emoji_open {
            let mut row = div().flex().flex_row().gap(px(4.));
            for (i, e) in EMOJI.into_iter().enumerate() {
                row = row.child(Button::new(SharedString::from(format!("emoji-{i}"))).label(e).small().ghost().on_click(cx.listener(move |this, _, w, cx| this.emoji(e, w, cx))));
            }
            toolbar = toolbar.child(row);
        }

        // Canvas with chrome.
        let mut canvas = div().absolute().left(px(ox)).top(px(oy)).w(px(cw)).h(px(ch)).bg(cx.theme().secondary);
        if let Some(p) = &self.picture {
            canvas = canvas.child(img(p.clone()).absolute().left_0().top_0().w(px(cw)).h(px(ch)));
        }
        let sel = rgba(Rgba::rgb(0x3b, 0x82, 0xf6));
        if let Some(r) = chrome.selection {
            canvas = canvas.child(abs(r.x * z, r.y * z, r.w * z, r.h * z).border_1().border_color(sel));
        }
        if let Some(r) = chrome.pending {
            canvas = canvas.child(abs(r.x * z, r.y * z, r.w * z, r.h * z).border_1().border_color(sel).bg(rgba(Rgba { a: 40, ..Rgba::rgb(0x3b, 0x82, 0xf6) })));
        }
        for (_, p) in &chrome.handles {
            canvas = canvas.child(abs(p.x * z - 4.0, p.y * z - 4.0, 8.0, 8.0).bg(gpui::white()).border_1().border_color(sel));
        }
        if let Some(c) = chrome.caret {
            canvas = canvas.child(abs(c.x * z, c.y * z, 2.0, chrome.style.font.size * z).bg(cx.theme().foreground));
        }

        let mut footer = div()
            .absolute()
            .left_0()
            .bottom_0()
            .w_full()
            .h(px(FOOTER_H))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background);
        footer = footer.child(div().text_xs().text_color(cx.theme().muted_foreground).child(
            self.message.clone().unwrap_or_else(|| format!("{}×{} · {} annotations", chrome.canvas.0, chrome.canvas.1, chrome.annotation_count)),
        ));
        footer = footer
            .child(div().flex_1())
            .child(Button::new("copy").label("Copy").on_click(cx.listener(|this, _, _, cx| this.copy(cx))))
            .child(Button::new("upload").label("Upload").on_click(cx.listener(|this, _, _, cx| this.upload(cx))))
            .child(Button::new("save").label("Save").on_click(cx.listener(|this, _, _, cx| this.save(false, cx))))
            .child(Button::new("saveas").label("Save as…").on_click(cx.listener(|this, _, _, cx| this.save(true, cx))));
        if self.reply.is_some() {
            footer = footer.child(Button::new("continue").label("Continue").primary().on_click(cx.listener(|this, _, w, _| this.finish(w))));
        } else {
            footer = footer.child(Button::new("close").label("Close").on_click(|_, w, _| w.remove_window()));
        }

        div()
            .id("laqta-editor")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_sm()
            .child(toolbar)
            .child(canvas)
            .child(footer)
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                let (x, y) = this.to_image(ev.position, window);
                this.input(EditorInput::PointerMove { x, y, mods: mods(&ev.modifiers) }, window, cx);
            }))
            .on_any_mouse_down(cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                let y_win = f32::from(ev.position.y);
                if y_win < TOOLBAR_H || y_win > f32::from(window.viewport_size().height) - FOOTER_H {
                    return;
                }
                if let Some(b) = button(ev.button) {
                    window.focus(&this.focus, cx);
                    let (x, y) = this.to_image(ev.position, window);
                    this.input(EditorInput::PointerDown { x, y, button: b, mods: mods(&ev.modifiers) }, window, cx);
                }
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
                if let Some(key) = editor_key(ev.keystroke.key.as_str()) {
                    this.input(EditorInput::Key { key, mods: m }, window, cx);
                }
            }))
    }
}

impl EditorWindow {
    fn effects_menu(&self, cx: &mut Context<Self>) -> Div {
        let presets = cx.global::<UiState>().services.app.settings().effect_presets;
        let mut row = div().flex().flex_row().gap(px(2.));
        for (i, p) in presets.into_iter().enumerate().take(6) {
            let name = if p.name.is_empty() { format!("Preset {}", i + 1) } else { p.name.clone() };
            row = row.child(Button::new(SharedString::from(format!("fx-{i}"))).label(name).small().ghost().on_click(cx.listener(move |this, _, w, cx| {
                this.command(Command::ApplyEffect(p.clone()), w, cx)
            })));
        }
        row
    }
}
