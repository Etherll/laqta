//! The annotation editor window. It forwards input to the Editor engine, paints the engine's
//! rendered pixels and chrome, and computes nothing itself (ARCHITECTURE §5 rule). The engine runs
//! on the compute pool for every call, since some commands call imaging kernels.

use std::path::PathBuf;
use std::sync::Arc;

use futures_channel::oneshot;
use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, Bounds, ClipboardItem, Context, Div, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent, KeyUpEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render, RenderImage, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, WindowOptions, div, img, prelude::FluentBuilder as _, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::{ActiveTheme as _, Disableable as _, Icon, Root, Sizable as _};
use laqta_domain::color::Rgba;
use laqta_domain::{Bitmap, ImageFormat};
use laqta_editor::{Chrome, Command, Editor, EditorInput, EditorKey, EditorRequest, Mods, PointerButton, Tool};
use laqta_ports::PortError;

use crate::assets::IconName;
use crate::chrome;
use crate::presenters::editor_interaction::{Batch, EditorInteraction, Step, Submitted, Work};
use crate::presenters::editor_toolbar;
use crate::presenters::notice::{AiTool, Notice};
use crate::presenters::output;
use crate::presenters::render_gate::ComputeFn;
use crate::presenters::space_lend::SpaceLend;
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
    if image.is_none()
        && let Some(p) = path.clone()
    {
        // Read and decode the file off the GPUI thread, then open on the decoded image.
        let app = cx.global::<UiState>().services.app.clone();
        cx.spawn(async move |cx| {
            let decoded = app.decode_file(p.clone()).await;
            cx.update(|cx| match decoded {
                Ok(image) => open(Some(image), reply, Some(p), cx),
                Err(e) => {
                    tracing::warn!(path = %p.display(), "editor: cannot open the image: {e}");
                    if let Some(r) = reply {
                        let _ = r.send(Err(PortError::Failed("no image to edit".into())));
                    }
                }
            });
        })
        .detach();
        return;
    }
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
        window_bounds: Some(crate::chrome::centered(1180., 820., cx)),
        titlebar: Some(chrome::titlebar("Laqta editor")),
        window_decorations: chrome::decorations(),
        app_owns_titlebar_drag: true,
        app_id: Some(crate::APP_ID.into()),
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
pub(crate) async fn compute_picture(compute: &ComputeFn, bitmap: Bitmap) -> Option<Arc<RenderImage>> {
    crate::presenters::render_gate::compute(compute, move || to_render_image(&bitmap)).await.flatten()
}

use crate::gpu_image::render_image as to_render_image;

struct EditorWindow {
    /// Input, render gating and snapshot renders (GPUI-free, tested in the presenter).
    interaction: EditorInteraction,
    compute: ComputeFn,
    picture: Option<Arc<RenderImage>>,
    chrome: Chrome,
    reply: Option<EditReply>,
    path: Option<PathBuf>,
    focus: FocusHandle,
    message: Option<Notice>,
    zoom: f32,
    emoji_open: bool,
    /// The stage's bounds from the last frame; the canvas is laid out inside them.
    stage: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The message the dismiss timer is running for.
    toast_for: Option<Notice>,
    /// A smart tool at work ("Removing the background…"); others wait for it.
    busy: Option<&'static str>,
    /// Outputs asked for while commands were still running or queued; they run when the queue drains,
    /// so they see every edit made before them.
    deferred: Vec<Deferred>,
    /// The window closes once the save it asked for lands (the unsaved-edits prompt's Save).
    close_after_save: bool,
    /// The last save failed or was cancelled after the editor was marked saved.
    save_missed: bool,
    /// The pointer over the canvas, in image pixels, for its cursor shape.
    pointer_at: Option<(f32, f32)>,
    /// The box of the annotation the Select tool hovers, outlined so the next click's target shows.
    hover: Option<laqta_editor::RectF>,
    /// A button is down on the canvas.
    pressed: bool,
    /// The button that made `pressed`: another button's press and release during the drag are
    /// ignored, so they can't end it.
    press_button: Option<MouseButton>,
    /// Holding Space lends Select while a drawing tool is out.
    space: SpaceLend<Tool>,
    /// The handle reach last given to the engine, in image pixels (`sync_reach`).
    reach: f32,
    /// The failure on show had its detail copied: its button says so.
    detail_copied: bool,
}

/// An output that reads the image, so it waits for queued edits.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Deferred {
    Finish,
    Save { ask: bool },
    Copy,
    Upload,
    Smart(Smart),
}

/// The editor's AI tools, run on the whole current image.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Smart {
    CopyText,
    RemoveBackground,
    /// Fill the box (image pixels) with what the model thinks is behind it.
    Erase(laqta_domain::geometry::PhysRect),
    Describe,
}

impl Smart {
    /// The tool, by what its notices say.
    fn tool(self) -> AiTool {
        match self {
            Smart::CopyText => AiTool::CopyText,
            Smart::RemoveBackground => AiTool::RemoveBackground,
            Smart::Erase(_) => AiTool::Erase,
            Smart::Describe => AiTool::Describe,
        }
    }
}

/// What a smart tool hands back to the window.
enum SmartResult {
    Text { image: Bitmap, text: String },
    Description { image: Bitmap, text: String },
    Image(Bitmap),
}

/// A mask the size of the image, set inside `rect`: one byte per pixel, row-major.
fn rect_mask(width: u32, height: u32, rect: laqta_domain::geometry::PhysRect) -> Vec<u8> {
    let mut mask = vec![0u8; width as usize * height as usize];
    let x0 = rect.x.clamp(0, width as i32) as usize;
    let y0 = rect.y.clamp(0, height as i32) as usize;
    let x1 = (rect.x + rect.width as i32).clamp(0, width as i32) as usize;
    let y1 = (rect.y + rect.height as i32).clamp(0, height as i32) as usize;
    for row in mask.chunks_exact_mut(width as usize).take(y1).skip(y0) {
        row[x0..x1].fill(255);
    }
    mask
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

impl EditorWindow {
    fn new(image: Bitmap, reply: Option<EditReply>, path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let services = cx.global::<UiState>().services.clone();
        // Scan the fonts now, off the UI thread, so the first text tool or emoji doesn't wait for it.
        let fonts = services.fonts.clone();
        cx.background_spawn(async move { fonts.warm() }).detach();
        let editor = Editor::new(image, services.fonts.clone());
        let chrome = editor.chrome();
        let compute = compute_fn(services.spawner.clone());
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut this = EditorWindow {
            interaction: EditorInteraction::new(editor, compute.clone()),
            compute,
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
            busy: None,
            deferred: Vec::new(),
            close_after_save: false,
            save_missed: false,
            pointer_at: None,
            hover: None,
            pressed: false,
            press_button: None,
            space: SpaceLend::new(Tool::Select),
            reach: 0.0,
            detail_copied: false,
        };
        // Space let go in another window never comes back as a key-up here: losing focus ends the
        // lend and any press.
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                // A drag whose release went to another window ends here, so the engine doesn't
                // keep moving the mark (or lose its undo step) when the pointer comes back.
                if this.pressed
                    && let Some((x, y)) = this.pointer_at
                {
                    let up = EditorInput::PointerUp { x, y, button: PointerButton::Left, mods: Mods::default() };
                    this.input(up, window, cx);
                }
                this.pressed = false;
                this.press_button = None;
                if this.space.is_lent() {
                    this.return_select(window, cx);
                }
                this.space.picked();
            }
        })
        .detach();
        crate::close_guard::install(Self::close_guard, window, cx);
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
        // The marks may have moved under a still pointer (undo, a nudge, delete).
        if let Some((x, y)) = self.pointer_at {
            self.track_hover(x, y);
        }
        if step.render_now {
            self.start_render(cx);
        }
        if let Some(r) = step.request {
            self.request(r, None, window, cx);
        }
        cx.notify();
    }

    /// Pointer, key and text input. The engine runs inline (state math only) unless a command is
    /// running, in which case the input queues behind it. Rendering is async.
    fn input(&mut self, input: EditorInput, window: &mut Window, cx: &mut Context<Self>) {
        // A smart tool replaces the whole image when it lands; edits made meanwhile would be lost.
        if self.busy.is_some() {
            return;
        }
        self.submit(Work::Input(input), window, cx);
    }

    /// Commands run on the compute pool, in order, and the window never waits for them.
    fn command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        // As for input: a smart tool is about to replace the image (it clears `busy` before it does).
        if self.busy.is_some() {
            return;
        }
        self.submit(Work::Command(command), window, cx);
    }

    fn submit(&mut self, work: Work, window: &mut Window, cx: &mut Context<Self>) {
        match self.interaction.submit(work) {
            Submitted::Step(step) => self.apply(*step, window, cx),
            Submitted::Started(batch) => self.run_batch(batch, window, cx),
            Submitted::Queued => {}
        }
    }

    fn run_batch(&mut self, batch: Batch, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let outputs = batch.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let done = this.interaction.batch_finished(outputs);
                match done.step {
                    Some(step) => this.apply(step, window, cx),
                    None if done.failed => this.message = Some(Notice::error("The command failed.")),
                    None => {}
                }
                for (request, snapshot) in done.requests {
                    this.request(request, snapshot, window, cx);
                }
                match done.next {
                    Some(next) => this.run_batch(next, window, cx),
                    None => {
                        for output in std::mem::take(&mut this.deferred) {
                            match output {
                                Deferred::Finish => this.finish(window, cx),
                                Deferred::Save { ask } => this.save(ask, window, cx),
                                Deferred::Copy => this.copy(window, cx),
                                Deferred::Upload => this.upload(window, cx),
                                Deferred::Smart(op) => this.smart(op, window, cx),
                            }
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// True when `output` has to wait: commands are still running or queued, and the output must
    /// include them. It is kept and runs when they end.
    fn defer(&mut self, output: Deferred) -> bool {
        if self.interaction.idle() {
            return false;
        }
        // One smart tool at a time, as when one is already at work: a second one is not kept.
        let second_smart = matches!(output, Deferred::Smart(_)) && self.deferred.iter().any(|d| matches!(d, Deferred::Smart(_)));
        if !second_smart && !self.deferred.contains(&output) {
            self.deferred.push(output);
        }
        true
    }

    /// The image encoded off the UI thread.
    fn encoded(
        &mut self,
        format: ImageFormat,
        image: laqta_ports::BoxFuture<'static, Option<Bitmap>>,
        cx: &mut Context<Self>,
    ) -> impl Future<Output = Result<Vec<u8>, String>> + Send + 'static {
        let codecs = cx.global::<UiState>().services.codecs.clone();
        let opts = laqta_imaging::EncodeOptions { format, jpeg_quality: 90 };
        output::encode(self.compute.clone(), codecs, image, opts)
    }

    /// The image as it is now: later edits wait behind the export, so they are never in it.
    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) -> laqta_ports::BoxFuture<'static, Option<Bitmap>> {
        let (image, submitted) = self.interaction.export();
        if let Submitted::Started(batch) = submitted {
            self.run_batch(batch, window, cx);
        }
        image
    }

    /// Acts on a request from the engine. `snapshot` is the image as it was when the request was
    /// made, when later work may already have changed the engine.
    fn request(&mut self, r: EditorRequest, snapshot: Option<Bitmap>, window: &mut Window, cx: &mut Context<Self>) {
        match r {
            EditorRequest::Save => self.save_from(false, snapshot, window, cx),
            EditorRequest::SaveAs => self.save_from(true, snapshot, window, cx),
            EditorRequest::Continue => self.finish_from(snapshot, window, cx),
            EditorRequest::EraseObject { rect } => self.smart_from(Smart::Erase(rect), snapshot, window, cx),
        }
    }

    /// `snapshot` as a ready image, or an export queued now.
    fn image(
        &mut self,
        snapshot: Option<Bitmap>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> laqta_ports::BoxFuture<'static, Option<Bitmap>> {
        match snapshot {
            Some(image) => Box::pin(async move { Some(image) }),
            None => self.export(window, cx),
        }
    }

    /// Hands the finished image to whoever opened the editor, then closes the window. The export
    /// runs on the compute pool; input is ignored while it does.
    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_from(None, window, cx);
    }

    fn finish_from(&mut self, snapshot: Option<Bitmap>, window: &mut Window, cx: &mut Context<Self>) {
        // A smart tool is still working on the image: Done waits for the user to press it again.
        if self.busy.is_some() {
            return;
        }
        // Commands still running or queued belong in the result: finish when they end.
        if snapshot.is_none() && self.defer(Deferred::Finish) {
            return;
        }
        let Some(reply) = self.reply.take() else {
            window.remove_window();
            return;
        };
        self.busy = Some("Finishing…");
        cx.notify();
        let export = self.image(snapshot, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let image = export.await.ok_or_else(|| PortError::Failed("export failed".into()));
            let _ = reply.send(image);
            let _ = this.update_in(cx, |_, window, _| window.remove_window());
        })
        .detach();
    }

    fn close_guard() -> crate::close_guard::Guard<Self> {
        crate::close_guard::Guard {
            dirty: |this: &Self| this.chrome.modified || this.save_missed,
            message: |this| if this.reply.is_some() { "Use your edits?" } else { "Save changes to this image?" },
            detail: |this| {
                if this.reply.is_some() {
                    "Closing without them continues with the original capture."
                } else {
                    "Your edits are lost if you don't save them."
                }
            },
            keep: |this| if this.reply.is_some() { "Use edits" } else { "Save" },
            on_keep: |this, window, cx| {
                if this.reply.is_some() {
                    this.finish(window, cx);
                } else {
                    this.close_after_save = true;
                    this.save(false, window, cx);
                }
            },
        }
    }

    fn save(&mut self, ask: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.save_from(ask, None, window, cx);
    }

    fn save_from(&mut self, ask: bool, snapshot: Option<Bitmap>, window: &mut Window, cx: &mut Context<Self>) {
        if snapshot.is_none() && self.defer(Deferred::Save { ask }) {
            return;
        }
        // Saved from here on; a failed or cancelled save sets `save_missed`. Nothing is running
        // (`defer` said so), so the engine's lock is free. An after-capture edit stays unsaved: the
        // task still waits for the edits, so closing must still ask whether to use them.
        if self.reply.is_none() {
            if let Ok(mut editor) = self.interaction.editor().lock() {
                editor.mark_saved();
            }
            self.chrome.modified = false;
        }
        let handle = window.window_handle();
        let settings = cx.global::<UiState>().services.app.settings();
        let format = settings.defaults.image.format;
        let ext = format.extension();
        let image = self.image(snapshot, window, cx);
        let encoded = self.encoded(format, image, cx);
        let default_dir = settings.screenshots_dir.clone();
        let current = self.path.clone();
        let task = cx.background_spawn(async move {
            let bytes = encoded.await?;
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
                Some(p) => output::write_file(&p, &bytes).map(|()| Some(p)),
                None => Ok(None),
            }
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                this.save_missed = !matches!(r, Ok(Some(_)));
                // An edit made while the file was written is not in it, so it keeps the window open.
                if std::mem::take(&mut this.close_after_save) && !(Self::close_guard().dirty)(this) {
                    crate::close_guard::close(handle, cx);
                }
                this.message = Some(match r {
                    Ok(Some(p)) => {
                        let m = Notice::info(format!("Saved {}", p.display()));
                        this.path = Some(p);
                        m
                    }
                    Ok(None) => return,
                    Err(e) => Notice::failure_text("Couldn't save the image", &e.to_string()),
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.defer(Deferred::Copy) {
            return;
        }
        let export = self.export(window, cx);
        let app = cx.global::<UiState>().services.app.clone();
        cx.spawn(async move |this, cx| {
            let copied = match export.await {
                Some(image) => app.copy_image(image).await.map_err(|error| Notice::failure("Couldn't copy the image", &error)),
                None => Err(Notice::error("Couldn't copy the image")),
            };
            let _ = this.update(cx, |this, cx| {
                this.message = Some(copied.map_or_else(|e| e, |()| Notice::info("Copied to the clipboard.")));
                cx.notify();
            });
        })
        .detach();
    }

    /// Saves a temporary PNG and hands it to Task as a file upload.
    fn upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.defer(Deferred::Upload) {
            return;
        }
        let image = self.export(window, cx);
        let encoded = self.encoded(ImageFormat::Png, image, cx);
        let services = cx.global::<UiState>().services.clone();
        let dir = services.data_dir.join("edits");
        let stem = format!("Laqta_edit_{}", std::process::id());
        let written = cx.background_spawn(async move {
            let bytes = encoded.await?;
            output::write_temp(&dir, &stem, "png", &bytes, std::time::SystemTime::now())
        });
        cx.spawn(async move |this, cx| {
            // Submitted here, not in the view update: the upload goes ahead even if the editor was
            // closed while the image was being written.
            let message = match written.await {
                Ok(path) => Notice::info(format!("Uploading ({} task).", services.app.upload_files(vec![path], None).len())),
                Err(e) => Notice::failure_text("Couldn't upload the image", &e),
            };
            let _ = this.update(cx, |this, cx| {
                this.message = Some(message);
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs a smart tool on the current image: export on the compute pool, the model through
    /// `App`, then the result back into the editor (an undoable step) or a text window.
    fn smart(&mut self, op: Smart, window: &mut Window, cx: &mut Context<Self>) {
        self.smart_from(op, None, window, cx);
    }

    fn smart_from(&mut self, op: Smart, snapshot: Option<Bitmap>, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() || (snapshot.is_none() && self.defer(Deferred::Smart(op))) {
            return;
        }
        self.busy = Some(op.tool().busy());
        cx.notify();
        let app = cx.global::<UiState>().services.app.clone();
        let prompt = app.settings().cloud.analyzer.prompt.clone();
        let export = self.image(snapshot, window, cx);
        let work = cx.background_spawn(async move {
            let image = export.await.ok_or_else(|| laqta_ports::AppError::Io("couldn't read the image".into()))?;
            Ok::<_, laqta_ports::AppError>(match op {
                Smart::CopyText => {
                    let text = app.recognize_text(image.clone()).await?.text();
                    SmartResult::Text { image, text }
                }
                Smart::RemoveBackground => SmartResult::Image(app.remove_background(image).await?),
                Smart::Erase(rect) => {
                    let mask = rect_mask(image.width(), image.height(), rect);
                    SmartResult::Image(app.erase(image, mask).await?)
                }
                Smart::Describe => {
                    let text = app.analyze(image.clone(), prompt).await?;
                    SmartResult::Description { image, text }
                }
            })
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(SmartResult::Image(image)) => this.command(Command::ReplaceImage(image), window, cx),
                    Ok(SmartResult::Text { image, text }) => {
                        let copied = !text.trim().is_empty();
                        if copied {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                        }
                        let title = "Text capture".into();
                        crate::text_window::open(crate::text_window::TextResult { title, image: Some(image), text, copied }, cx);
                    }
                    Ok(SmartResult::Description { image, text }) => {
                        let title = "Image description".into();
                        crate::text_window::open(crate::text_window::TextResult { title, image: Some(image), text, copied: false }, cx);
                    }
                    Err(e) => this.message = Some(Notice::failure(op.tool().failed(), &e)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn pick_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let codecs = cx.global::<UiState>().services.codecs.clone();
        let task = cx.background_spawn(async move {
            let f = rfd::AsyncFileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp"])
                .pick_file()
                .await?;
            let bytes = output::read_file(f.path()).ok()?;
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
        let rendered = crate::presenters::render_gate::compute(&self.compute, move || fonts.render_text(&text, &font));
        self.emoji_open = false;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let bmp = rendered.await.flatten();
            let _ = this.update_in(cx, |this, window, cx| {
                match bmp {
                    Some(b) => {
                        this.command(Command::SetToolImage(b), window, cx);
                        this.command(Command::SetTool(Tool::Emoji), window, cx);
                    }
                    None => this.message = Some(Notice::error("No emoji font is installed.")),
                }
                cx.notify();
            });
        })
        .detach();
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

    /// Switches tools on the UI thread when the engine is free: `SetTool` only changes engine state,
    /// so it needs no pool job. A busy engine queues it instead, so a tool key never blocks the window.
    fn select_tool(&mut self, t: Tool, window: &mut Window, cx: &mut Context<Self>) {
        // A tool picked while Space is held is the one to keep.
        self.space.picked();
        self.set_tool(t, window, cx);
    }

    /// Switches tools without touching what Space lent (`select_tool` is the user's pick).
    fn set_tool(&mut self, t: Tool, window: &mut Window, cx: &mut Context<Self>) {
        self.emoji_open = false;
        self.hover = None;
        if self.busy.is_some() {
            return cx.notify();
        }
        match self.interaction.submit_light(Command::SetTool(t)) {
            Submitted::Step(step) => self.apply(*step, window, cx),
            Submitted::Started(batch) => self.run_batch(batch, window, cx),
            Submitted::Queued => {}
        }
        cx.notify();
    }

    /// Holds Space: Select stands in for the drawing tool until Space comes up.
    fn borrow_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Only when the switch lands at once: mid-press it would end the drag as the wrong tool,
        // and a busy or queued engine would leave `chrome.tool` behind the lend.
        if self.pressed || self.busy.is_some() || !self.interaction.idle() {
            return;
        }
        if let Some(t) = self.space.space_down(self.chrome.tool) {
            self.set_tool(t, window, cx);
        }
    }

    /// Space came up: the drawing tool comes back, once any drag under way has ended.
    fn return_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.space.space_up(self.pressed, self.chrome.tool) {
            self.set_tool(t, window, cx);
        }
    }

    /// The hover outline at the image point (`x`, `y`): only under Select with no press under way.
    /// Returns whether it changed.
    fn track_hover(&mut self, x: f32, y: f32) -> bool {
        let next = if self.chrome.tool == Tool::Select && !self.pressed {
            // A hover move can't wait on a render; it keeps the old outline until the lock frees.
            match self.interaction.editor().try_lock() {
                Ok(e) => e.hover_at(x, y),
                Err(_) => return false,
            }
        } else {
            None
        };
        std::mem::replace(&mut self.hover, next) != next
    }

    /// The cursor over the canvas at the last pointer position.
    fn canvas_cursor(&self, z: f32) -> gpui::CursorStyle {
        use crate::presenters::mark_pointer::{GRAB, canvas_pointer, handle_under};
        let Some((x, y)) = self.pointer_at else { return gpui::CursorStyle::Arrow };
        let grab = handle_under(&self.chrome.handles, (x, y), GRAB / z);
        let in_picked = self.chrome.selection.is_some_and(|r| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h);
        match canvas_pointer(self.chrome.tool, grab, self.hover.is_some(), in_picked) {
            Some(p) => crate::mark_chrome::pointer_style(p, self.pressed),
            None => gpui::CursorStyle::Arrow,
        }
    }

    /// Gives the engine the handle reach for zoom `z`, so a press takes a handle over the area its
    /// resize cursor shows. Never waits on the engine: a busy one gets it on a later move or press.
    fn sync_reach(&mut self, z: f32) {
        let reach = crate::presenters::mark_pointer::GRAB / z;
        if reach == self.reach {
            return;
        }
        if let Ok(mut e) = self.interaction.editor().try_lock() {
            e.set_handle_reach(reach);
            self.reach = reach;
        }
    }

    /// Clears a message a few seconds after it appears.
    fn dismiss_later(&mut self, cx: &mut Context<Self>) {
        if self.message == self.toast_for {
            return;
        }
        self.detail_copied = false;
        self.toast_for = self.message.clone();
        // A failure stays until dismissed, so its reason can be read and its detail copied.
        let Some(shown) = self.message.clone().filter(|m| !m.error) else { return };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(3200)).await;
            let _ = this.update(cx, |this, cx| {
                if this.message.as_ref() == Some(&shown) {
                    this.message = None;
                    this.toast_for = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

pub(crate) fn mods(m: &gpui::Modifiers) -> Mods {
    Mods { shift: m.shift, ctrl: m.control || m.platform, alt: m.alt }
}

pub(crate) fn button(b: MouseButton) -> Option<PointerButton> {
    match b {
        MouseButton::Left => Some(PointerButton::Left),
        MouseButton::Right => Some(PointerButton::Right),
        MouseButton::Middle => Some(PointerButton::Middle),
        _ => None,
    }
}

pub(crate) fn editor_key(key: &str) -> Option<EditorKey> {
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
        Tool::ObjectEraser => IconName::WandSparkles,
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
            format!("Undo  ·  {}", crate::presenters::hotkeys::shortcut_text("Ctrl+Z", crate::host_os())),
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
            format!("Redo  ·  {}", crate::presenters::hotkeys::shortcut_text("Ctrl+Y", crate::host_os())),
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
        // The marker draws with its own colour and width, not the border's.
        let marker = chrome.tool == Tool::Highlight;
        if controls.color {
            let mut track =
                div().flex().flex_none().items_center().gap(px(1.)).p(px(3.)).rounded(radius::PILL).bg(cx.theme().foreground.opacity(0.06));
            let current = if marker { chrome.style.highlight_color } else { chrome.style.border_color };
            for (i, c) in PALETTE.into_iter().enumerate() {
                track = track.child(widgets::swatch(
                    ("col", i),
                    rgba(c),
                    current == c,
                    cx.listener(move |this, _, window, cx| {
                        this.set_style(|s| if marker { s.highlight_color = c } else { s.border_color = c }, window, cx)
                    }),
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
                format!("{:.0} px", if marker { chrome.style.highlight_size } else { chrome.style.border_size }),
                cx.listener(move |this, _, w, cx| {
                    this.set_style(
                        |s| {
                            if marker {
                                s.highlight_size = (s.highlight_size - 2.0).max(4.0)
                            } else {
                                s.border_size = (s.border_size - 1.0).max(1.0)
                            }
                        },
                        w,
                        cx,
                    )
                }),
                cx.listener(move |this, _, w, cx| {
                    this.set_style(
                        |s| {
                            if marker {
                                s.highlight_size = (s.highlight_size + 2.0).min(60.0)
                            } else {
                                s.border_size = (s.border_size + 1.0).min(40.0)
                            }
                        },
                        w,
                        cx,
                    )
                }),
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
            .child(self.smart_menu(cx))
            .child(self.image_menu(cx))
    }

    /// The AI tools: read the text, cut out the subject, erase an object, describe the image.
    fn smart_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let entity = cx.entity().downgrade();
        let busy = self.busy.is_some();
        Button::new("smart-menu")
            .label("AI tools")
            .icon(Icon::new(IconName::Sparkles))
            .ghost()
            .dropdown_caret(true)
            .disabled(busy)
            .dropdown_menu(move |menu, _, _| {
                let run = |op: Smart| {
                    let entity = entity.clone();
                    move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                        let _ = entity.update(cx, |this, cx| this.smart(op, window, cx));
                    }
                };
                let erase = {
                    let entity = entity.clone();
                    move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                        let _ = entity.update(cx, |this, cx| this.pick_tool(Tool::ObjectEraser, window, cx));
                    }
                };
                menu.item(PopupMenuItem::new("Copy text").icon(Icon::new(IconName::ScanText)).on_click(run(Smart::CopyText)))
                    .item(
                        PopupMenuItem::new("Remove background")
                            .icon(Icon::new(IconName::WandSparkles))
                            .on_click(run(Smart::RemoveBackground)),
                    )
                    .item(PopupMenuItem::new("Erase an object").icon(Icon::new(IconName::Eraser)).on_click(erase))
                    .separator()
                    .item(PopupMenuItem::new("Describe the image").icon(Icon::new(IconName::Eye)).on_click(run(Smart::Describe)))
            })
    }

    /// Rotate, flip, auto crop and the effect presets: things done to the whole image.
    fn image_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let presets = cx.global::<UiState>().services.app.settings().effect_presets;
        let entity = cx.entity().downgrade();
        Button::new("image-menu").label("Image").icon(Icon::new(IconName::Image)).ghost().dropdown_caret(true).dropdown_menu(
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
            .child(widgets::pill(
                "copy",
                "Copy",
                widgets::Pill::Default,
                false,
                cx.listener(|this, _, window, cx| this.copy(window, cx)),
                cx,
            ))
            .child(widgets::pill(
                "upload",
                "Upload",
                widgets::Pill::Default,
                false,
                cx.listener(|this, _, window, cx| this.upload(window, cx)),
                cx,
            ))
            .child(widgets::pill(
                "saveas",
                "Save as…",
                widgets::Pill::Default,
                false,
                cx.listener(|this, _, window, cx| this.save(true, window, cx)),
                cx,
            ));
        actions = if continuing {
            actions
                .child(widgets::pill(
                    "save",
                    "Save",
                    widgets::Pill::Default,
                    false,
                    cx.listener(|this, _, window, cx| this.save(false, window, cx)),
                    cx,
                ))
                .child(widgets::pill(
                    "continue",
                    "Done",
                    widgets::Pill::Primary,
                    false,
                    cx.listener(|this, _, w, cx| this.finish(w, cx)),
                    cx,
                ))
        } else {
            actions.child(widgets::pill(
                "save",
                "Save",
                widgets::Pill::Primary,
                false,
                cx.listener(|this, _, window, cx| this.save(false, window, cx)),
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
            .when_some(self.busy, |d, busy| d.child(widgets::badge(busy, widgets::Tone::Busy, cx)))
            .child(actions)
    }

    /// The toast over the stage. Information is one line that fades; a failure is a card with its
    /// reason, wrapped, a Copy details button when the raw error says more, and a close button.
    fn toast(&self, m: Notice, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = cx.theme().clone();
        if !m.error {
            return widgets::floating(cx)
                .px(space::S4)
                .py(space::S2)
                .gap(space::S2)
                .text_size(text::LABEL)
                .child(widgets::icon(IconName::Info, t.muted_foreground))
                .child(m.title)
                .into_any_element();
        }
        let danger = t.danger;
        let mut body = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(div().text_size(text::LABEL).font_weight(gpui::FontWeight::MEDIUM).child(m.title.clone()));
        if let Some(reason) = m.reason.clone() {
            body = body.child(div().text_size(text::SMALL).text_color(t.muted_foreground).child(reason));
        }
        if let Some(detail) = m.detail.clone() {
            // The card stays: copying is not dismissing, and the reason may still be needed.
            let label = if self.detail_copied { "Copied" } else { "Copy details" };
            body = body.child(div().pt(px(4.)).child(Button::new("toast-copy").label(label).xsmall().ghost().on_click(cx.listener(
                move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(detail.clone()));
                    this.detail_copied = true;
                    cx.notify();
                },
            ))));
        }
        stop_canvas(div())
            .id("toast")
            .w(px(420.))
            .max_w(gpui::relative(0.9))
            .flex()
            .items_start()
            .gap(space::S2)
            .px(space::S3)
            .py(space::S2)
            .rounded(radius::LG)
            .bg(t.popover)
            .border_1()
            .border_color(t.border)
            .shadow_md()
            .child(div().pt(px(1.)).child(widgets::icon(IconName::TriangleAlert, danger)))
            .child(body)
            .child(widgets::icon_button(
                "toast-close",
                IconName::X,
                "Dismiss",
                false,
                cx.listener(|this, _, _, cx| {
                    this.message = None;
                    // The same failure again later is a new card, with its own Copy details.
                    this.toast_for = None;
                    cx.notify();
                }),
                cx,
            ))
            .into_any_element()
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
        let sel = theme::extra(cx).info;
        if let Some(r) = chrome.selection {
            canvas = canvas.child(abs(r.x * z, r.y * z, r.w * z, r.h * z).border_1().border_color(sel));
        }
        if let Some(r) = chrome.pending {
            canvas =
                canvas.child(abs(r.x * z, r.y * z, r.w * z, r.h * z).border_1().border_color(sel).bg(theme::extra(cx).info.opacity(0.16)));
        }
        if let Some(r) = self.hover {
            canvas =
                canvas.child(abs(r.x * z - 2.0, r.y * z - 2.0, r.w * z + 4.0, r.h * z + 4.0).border_1().border_color(sel.opacity(0.7)));
        }
        for (_, p) in &chrome.handles {
            canvas = canvas.child(crate::mark_chrome::mark_handle(p.x * z, p.y * z, sel));
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
                    .text_size(text::SMALL)
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
            .cursor(self.canvas_cursor(z))
            .on_any_mouse_down(cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                if let Some(b) = button(ev.button) {
                    // Another button mid-drag is ignored; the same one again means its release was lost.
                    if this.press_button.is_some_and(|p| p != ev.button) {
                        return;
                    }
                    window.focus(&this.focus, cx);
                    this.pressed = true;
                    this.press_button = Some(ev.button);
                    this.hover = None;
                    let (_, _, z) = this.layout(window);
                    this.sync_reach(z);
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
            stage = stage.child(div().absolute().top(space::S4).left_0().right_0().flex().justify_center().child(self.toast(m, cx)));
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
                // Redraw only when the outline or the cursor shape changes, not on every move.
                let (_, _, z) = this.layout(window);
                this.sync_reach(z);
                let before = this.canvas_cursor(z);
                this.pointer_at = Some((x, y));
                if this.track_hover(x, y) || this.canvas_cursor(z) != before {
                    cx.notify();
                }
                this.input(EditorInput::PointerMove { x, y, mods: mods(&ev.modifiers) }, window, cx);
            }))
            .capture_any_mouse_up(cx.listener(|this, ev: &MouseUpEvent, window, cx| {
                if this.press_button.is_some_and(|p| p != ev.button) {
                    return;
                }
                this.press_button = None;
                if let Some(b) = button(ev.button) {
                    let (x, y) = this.to_image(ev.position, window);
                    this.input(EditorInput::PointerUp { x, y, button: b, mods: mods(&ev.modifiers) }, window, cx);
                }
                this.pressed = false;
                // Space came up during the drag: the drawing tool comes back now it is over.
                if let Some(t) = this.space.released(this.chrome.tool) {
                    this.set_tool(t, window, cx);
                }
            }))
            .on_key_up(cx.listener(|this, ev: &KeyUpEvent, window, cx| {
                if ev.keystroke.key == "space" && this.space.is_lent() {
                    this.return_select(window, cx);
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
                let key = ev.keystroke.key.as_str();
                // Mid-drag only Esc acts (it drops the stroke or puts the mark back): a tool key
                // would end the drag as another tool and lose what it was drawing.
                if this.pressed && !this.chrome.editing_text {
                    if key == "escape" {
                        this.input(EditorInput::Key { key: EditorKey::Escape, mods: m }, window, cx);
                    }
                    return;
                }
                // Holding Space lends Select while a drawing tool is out (repeats keep it lent).
                if key == "space" && !this.chrome.editing_text && !m.ctrl {
                    this.borrow_select(window, cx);
                    return;
                }
                // Single-key tool shortcuts, when no text is being typed.
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

#[cfg(test)]
mod tests {
    use laqta_domain::geometry::PhysRect;

    use super::rect_mask;

    #[test]
    fn the_erase_mask_covers_the_box_clamped_to_the_image() {
        let mask = rect_mask(4, 3, PhysRect { x: 1, y: 1, width: 10, height: 1 });
        assert_eq!(mask, vec![0, 0, 0, 0, 0, 255, 255, 255, 0, 0, 0, 0]);
        assert!(rect_mask(4, 3, PhysRect { x: -5, y: 9, width: 2, height: 2 }).iter().all(|b| *b == 0));
    }
}
