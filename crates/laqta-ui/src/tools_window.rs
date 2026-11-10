//! The tools window: one panel per `ToolKind`. Each panel picks its inputs (files, a folder,
//! a few fields, a choice), runs the matching `laqta_app::tools` function off the UI thread, and
//! shows the outcome as text or an image.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Render, RenderImage,
    SharedString, Styled as _, Window, WindowOptions, div, img, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme as _, Disableable as _, Root, Selectable as _, Sizable as _};
use laqta_app as t;
use laqta_domain::{Bitmap, ImageFormat, PixelFormat};
use laqta_ports::{ClipboardContent, ToolKind};

use crate::runtime::UiState;
use crate::widgets;

pub(crate) fn title(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::ColorPicker => "Color picker",
        ToolKind::ImageViewer => "Image viewer",
        ToolKind::ImageComparer => "Image comparer",
        ToolKind::IconConverter => "Icon converter",
        ToolKind::ImageCombiner => "Image combiner",
        ToolKind::ImageSplitter => "Image splitter",
        ToolKind::ImageResizer => "Image resizer",
        ToolKind::ImageConverter => "Image converter",
        ToolKind::ImageWatermark => "Image watermark",
        ToolKind::ImageThumbnailer => "Image thumbnailer",
        ToolKind::VideoConverter => "Video converter",
        ToolKind::VideoTrimmer => "Video trimmer",
        ToolKind::VideoThumbnailer => "Video thumbnailer",
        ToolKind::GifMaker => "Animated GIF maker",
        ToolKind::GifTrimmer => "Animated GIF trimmer",
        ToolKind::QrCode => "QR code",
        ToolKind::HashCheck => "Hash checker",
        ToolKind::Metadata => "Metadata viewer",
        ToolKind::StripMetadata => "Metadata stripper",
        ToolKind::IndexFolder => "Directory indexer",
        ToolKind::ClipboardViewer => "Clipboard viewer",
        ToolKind::BorderlessWindow => "Borderless window",
        ToolKind::MonitorTest => "Monitor test",
        ToolKind::NetworkMonitor => "Network monitor",
    }
}

/// What a panel asks for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pick {
    Nothing,
    OneFile,
    Files,
    Folder,
}

struct Spec {
    pick: Pick,
    /// A second file (the comparer's other image, the watermark image).
    second: Option<&'static str>,
    /// Up to three text fields: (label, default).
    fields: &'static [(&'static str, &'static str)],
    choices: &'static [&'static str],
    run: Option<&'static str>,
    help: &'static str,
}

fn spec(kind: ToolKind) -> Spec {
    let s = |pick, second, fields, choices, run, help| Spec { pick, second, fields, choices, run, help };
    match kind {
        ToolKind::ColorPicker => s(
            Pick::Nothing,
            None,
            &[("Color (hex, rgb(), hsl(), name)", "#3b82f6")],
            &[],
            Some("Show"),
            "Type a color, or pick one from the screen.",
        ),
        ToolKind::ImageViewer => s(Pick::OneFile, None, &[], &[], None, "Open an image to view it."),
        ToolKind::ImageComparer => {
            s(Pick::OneFile, Some("Second image"), &[("Tolerance (0–255)", "0")], &[], Some("Compare"), "Changed pixels show in red.")
        }
        ToolKind::IconConverter => {
            s(Pick::OneFile, None, &[("Sizes", "16,24,32,48,64,128,256")], &[], Some("Make .ico"), "Writes <name>.ico next to the image.")
        }
        ToolKind::ImageCombiner => s(
            Pick::Files,
            None,
            &[("Spacing (px)", "0")],
            &["Vertical", "Horizontal"],
            Some("Combine"),
            "Joins the images in order into combined.png next to the first.",
        ),
        ToolKind::ImageSplitter => {
            s(Pick::OneFile, None, &[("Rows", "2"), ("Columns", "2")], &[], Some("Split"), "Writes the pieces next to the image.")
        }
        ToolKind::ImageResizer => s(
            Pick::Files,
            None,
            &[("Width", "1280"), ("Height", "720")],
            &["Keep aspect ratio", "Stretch"],
            Some("Resize"),
            "Writes resized copies next to the originals.",
        ),
        ToolKind::ImageConverter => s(
            Pick::Files,
            None,
            &[("Quality (JPEG/WebP)", "90")],
            &["PNG", "JPEG", "WebP", "GIF", "BMP", "TIFF"],
            Some("Convert"),
            "Writes converted copies next to the originals.",
        ),
        ToolKind::ImageWatermark => s(
            Pick::Files,
            Some("Watermark image"),
            &[("Opacity (0–1)", "0.5")],
            &["Bottom right", "Bottom left", "Top right", "Top left", "Centre"],
            Some("Apply"),
            "Writes watermarked copies next to the originals.",
        ),
        ToolKind::ImageThumbnailer => s(
            Pick::Files,
            None,
            &[("Max width", "320"), ("Max height", "240")],
            &[],
            Some("Make thumbnails"),
            "Writes thumbnails next to the originals.",
        ),
        ToolKind::VideoConverter => s(
            Pick::OneFile,
            None,
            &[("Output extension", "mp4"), ("Width (empty = same)", "")],
            &["H.264", "H.265", "VP9", "AV1"],
            Some("Convert"),
            "Uses FFmpeg; writes next to the input.",
        ),
        ToolKind::VideoTrimmer | ToolKind::GifTrimmer => s(
            Pick::OneFile,
            None,
            &[("Start (s)", "0"), ("Duration (s)", "10")],
            &[],
            Some("Trim"),
            "Writes <name>-trimmed next to the input.",
        ),
        ToolKind::VideoThumbnailer => {
            s(Pick::OneFile, None, &[("How many", "6")], &[], Some("Make thumbnails"), "Evenly spaced frames, written next to the video.")
        }
        ToolKind::GifMaker => s(
            Pick::OneFile,
            None,
            &[("Frame rate", "15"), ("Width", "640")],
            &[],
            Some("Make GIF"),
            "Turns a video into an animated GIF (two-pass palette).",
        ),
        ToolKind::QrCode => s(
            Pick::Nothing,
            None,
            &[("Text", "https://")],
            &["Generate", "Decode a file"],
            Some("Go"),
            "Generate a code from text, or decode one from an image file.",
        ),
        ToolKind::HashCheck => s(
            Pick::OneFile,
            None,
            &[("Expected hash (optional)", "")],
            &[],
            Some("Hash"),
            "CRC32, MD5, SHA-1/256/384/512, BLAKE3, XXH64, XXH3.",
        ),
        ToolKind::Metadata => s(Pick::OneFile, None, &[], &[], Some("Read"), "EXIF for images; container tags for video and audio."),
        ToolKind::StripMetadata => s(Pick::Files, None, &[], &[], Some("Strip"), "Writes copies without metadata next to the originals."),
        ToolKind::IndexFolder => s(
            Pick::Folder,
            None,
            &[("Depth (empty = all)", "")],
            &["Text", "HTML", "JSON", "XML"],
            Some("Index"),
            "Lists the folder's tree; copy or save the result.",
        ),
        ToolKind::ClipboardViewer => s(Pick::Nothing, None, &[], &[], Some("Refresh"), "What the clipboard holds now."),
        ToolKind::BorderlessWindow => s(
            Pick::Nothing,
            None,
            &[("Window title contains", "")],
            &["Borderless", "Restore border"],
            Some("Apply"),
            "X11 and Windows; Wayland compositors don't allow it.",
        ),
        ToolKind::MonitorTest => s(
            Pick::Nothing,
            None,
            &[],
            &[],
            None,
            "Click the panel to step through test colors; maximize the window for a full-screen test.",
        ),
        ToolKind::NetworkMonitor => {
            s(Pick::Nothing, None, &[], &[], None, "Traffic per interface, sampled once a second while this window is open.")
        }
    }
}

use crate::gpu_image::render_image;

fn decode_file(path: &Path, codecs: &laqta_imaging::Codecs) -> Result<Bitmap, String> {
    let bytes = crate::presenters::output::read_file(path)?;
    codecs.decode(&bytes).map_err(|e| e.to_string())
}

fn list(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return "Nothing was written.".into();
    }
    format!("Wrote {} file(s):\n{}", paths.len(), paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n"))
}

fn dir_of(path: &Path) -> PathBuf {
    path.parent().map(Path::to_path_buf).unwrap_or_else(|| ".".into())
}

fn with_suffix(path: &Path, suffix: &str, ext: Option<&str>) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "output".into());
    let ext = ext.map(str::to_owned).or_else(|| path.extension().map(|e| e.to_string_lossy().into_owned())).unwrap_or_default();
    dir_of(path).join(if ext.is_empty() { format!("{stem}{suffix}") } else { format!("{stem}{suffix}.{ext}") })
}

/// The monitor test's colours, in order.
const TEST_COLOURS: [(u32, &str); 8] = [
    (0x000000, "Black"),
    (0xffffff, "White"),
    (0xff0000, "Red"),
    (0x00ff00, "Green"),
    (0x0000ff, "Blue"),
    (0x808080, "Grey 50%"),
    (0xffff00, "Yellow"),
    (0x00ffff, "Cyan"),
];

pub(crate) fn open(kind: ToolKind, cx: &mut App) {
    let big = matches!(kind, ToolKind::ImageViewer | ToolKind::ImageComparer | ToolKind::MonitorTest | ToolKind::IndexFolder);
    let options = WindowOptions {
        window_bounds: Some(crate::chrome::centered(if big { 960. } else { 680. }, if big { 720. } else { 560. }, cx)),
        titlebar: Some(crate::chrome::titlebar(title(kind))),
        window_decorations: crate::chrome::decorations(),
        app_owns_titlebar_drag: true,
        app_id: Some(crate::APP_ID.into()),
        focus: true,
        ..Default::default()
    };
    if let Err(e) = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| ToolsWindow::new(kind, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        tracing::error!("tool window failed: {e:#}");
    }
}

struct ToolsWindow {
    kind: ToolKind,
    files: Vec<PathBuf>,
    second: Option<PathBuf>,
    fields: Vec<Entity<InputState>>,
    choice: usize,
    output: String,
    image: Option<Arc<RenderImage>>,
    busy: bool,
    colour_step: usize,
    /// Network monitor rows: (interface, received/s, sent/s, total received, total sent).
    net: Vec<(String, u64, u64, u64, u64)>,
}

impl ToolsWindow {
    fn new(kind: ToolKind, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let fields =
            spec(kind).fields.iter().map(|(_, default)| cx.new(|cx| InputState::new(window, cx).default_value(*default))).collect();
        let mut this = ToolsWindow {
            kind,
            files: Vec::new(),
            second: None,
            fields,
            choice: 0,
            output: String::new(),
            image: None,
            busy: false,
            colour_step: 0,
            net: Vec::new(),
        };
        match kind {
            ToolKind::NetworkMonitor => this.start_network(cx),
            ToolKind::ClipboardViewer => this.run(cx),
            _ => {}
        }
        this
    }

    fn field(&self, i: usize, cx: &App) -> String {
        self.fields.get(i).map(|f| f.read(cx).value().trim().to_string()).unwrap_or_default()
    }

    fn num<T: std::str::FromStr>(&self, i: usize, what: &str, cx: &App) -> Result<T, String> {
        self.field(i, cx).parse::<T>().map_err(|_| format!("{what} must be a number"))
    }

    fn pick(&mut self, second: bool, cx: &mut Context<Self>) {
        let mode = if second { Pick::OneFile } else { spec(self.kind).pick };
        let task = cx.background_spawn(async move {
            let d = rfd::AsyncFileDialog::new();
            match mode {
                Pick::Files => d.pick_files().await.map(|f| f.iter().map(|h| h.path().to_path_buf()).collect()),
                Pick::Folder => d.pick_folder().await.map(|f| vec![f.path().to_path_buf()]),
                _ => d.pick_file().await.map(|f| vec![f.path().to_path_buf()]),
            }
        });
        cx.spawn(async move |this, cx| {
            if let Some(paths) = task.await {
                let _ = this.update(cx, |this, cx| {
                    if second {
                        this.second = paths.into_iter().next();
                    } else {
                        this.files = paths;
                        if this.kind == ToolKind::ImageViewer {
                            this.run(cx);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn start_network(&mut self, cx: &mut Context<Self>) {
        // Reading the counters walks /proc or the OS tables, so it runs on a background thread.
        let first = cx.background_spawn(async move { sysinfo::Networks::new_with_refreshed_list() });
        cx.spawn(async move |this, cx| {
            let Ok(mut networks) = first.await else {
                let _ = this.update(cx, |this, cx| {
                    this.net = vec![("Network counters are not available here".into(), 0, 0, 0, 0)];
                    cx.notify();
                });
                return;
            };
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                let sampled = cx.background_spawn(async move {
                    networks.refresh(true);
                    let mut rows: Vec<_> = networks
                        .iter()
                        .map(|(name, d)| {
                            (name.to_string_lossy().into_owned(), d.received(), d.transmitted(), d.total_received(), d.total_transmitted())
                        })
                        .collect();
                    rows.sort_by(|a, b| a.0.cmp(&b.0));
                    (networks, rows)
                });
                let rows;
                (networks, rows) = sampled.await;
                // The window closed: stop sampling.
                if this
                    .update(cx, |this, cx| {
                        this.net = rows;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn finish(&mut self, r: Result<(String, Option<Bitmap>), String>, cx: &mut Context<Self>) {
        self.busy = false;
        match r {
            Ok((text, image)) => {
                self.output = text;
                self.image = image.as_ref().and_then(render_image);
            }
            Err(e) => {
                self.output = format!("Failed: {e}");
                self.image = None;
            }
        }
        cx.notify();
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        let services = cx.global::<UiState>().services.clone();
        let app = services.app.clone();
        let codecs = services.codecs.clone();
        let kind = self.kind;
        let files = self.files.clone();
        let second = self.second.clone();
        let choice = self.choice;
        let f: Vec<String> = (0..self.fields.len()).map(|i| self.field(i, cx)).collect();
        let first = files.first().cloned();
        let need = |p: Option<PathBuf>, what: &str| p.ok_or_else(|| format!("choose {what} first"));

        // Fields parsed up front so a typo is reported before any work starts.
        let parsed: Result<Job, String> = (|| {
            Ok(match kind {
                ToolKind::ColorPicker => Job::Colour(f[0].clone()),
                ToolKind::ImageViewer => Job::View(need(first, "an image")?),
                ToolKind::ImageComparer => {
                    Job::Compare(need(first, "an image")?, need(second, "the second image")?, self.num(0, "Tolerance", cx)?)
                }
                ToolKind::IconConverter => {
                    let sizes: Vec<u32> = f[0].split(',').filter_map(|s| s.trim().parse().ok()).collect();
                    Job::Icon(need(first, "an image")?, sizes)
                }
                ToolKind::ImageCombiner => {
                    if files.len() < 2 {
                        return Err("choose two or more images".into());
                    }
                    Job::Combine(files.clone(), choice == 1, self.num(0, "Spacing", cx)?)
                }
                ToolKind::ImageSplitter => Job::Split(need(first, "an image")?, self.num(0, "Rows", cx)?, self.num(1, "Columns", cx)?),
                ToolKind::ImageResizer => Job::Resize(files.clone(), self.num(0, "Width", cx)?, self.num(1, "Height", cx)?, choice == 0),
                ToolKind::ImageConverter => {
                    let format =
                        [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP, ImageFormat::Gif, ImageFormat::Bmp, ImageFormat::Tiff]
                            [choice];
                    Job::Convert(files.clone(), format, self.num(0, "Quality", cx)?)
                }
                ToolKind::ImageWatermark => {
                    use laqta_domain::effects::draw::Placement as P;
                    let placement = [P::BottomRight, P::BottomLeft, P::TopRight, P::TopLeft, P::MiddleCenter][choice];
                    Job::Watermark(files.clone(), need(second, "the watermark image")?, placement, self.num(0, "Opacity", cx)?)
                }
                ToolKind::ImageThumbnailer => Job::Thumbs(files.clone(), self.num(0, "Max width", cx)?, self.num(1, "Max height", cx)?),
                ToolKind::VideoConverter => {
                    use laqta_domain::record::VideoCodec as C;
                    let codec = [C::X264, C::X265, C::Vp9, C::Av1][choice];
                    let width = if f[1].is_empty() { None } else { Some(self.num(1, "Width", cx)?) };
                    let ext = if f[0].is_empty() { "mp4".to_string() } else { f[0].trim_start_matches('.').to_string() };
                    Job::Video(need(first, "a video")?, ext, codec, width)
                }
                ToolKind::VideoTrimmer | ToolKind::GifTrimmer => {
                    Job::Trim(need(first, "a video")?, self.num(0, "Start", cx)?, self.num(1, "Duration", cx)?)
                }
                ToolKind::VideoThumbnailer => Job::VideoThumbs(need(first, "a video")?, self.num(0, "How many", cx)?),
                ToolKind::GifMaker => Job::Gif(need(first, "a video")?, self.num(0, "Frame rate", cx)?, self.num(1, "Width", cx)?),
                ToolKind::QrCode if choice == 1 => Job::QrDecode,
                ToolKind::QrCode => Job::QrMake(f[0].clone()),
                ToolKind::HashCheck => Job::Hash(need(first, "a file")?, f[0].clone()),
                ToolKind::Metadata => Job::Meta(need(first, "a file")?),
                ToolKind::StripMetadata => Job::Strip(files.clone()),
                ToolKind::IndexFolder => {
                    let depth = if f[0].is_empty() { None } else { Some(self.num(0, "Depth", cx)?) };
                    Job::Index(need(first, "a folder")?, choice, depth)
                }
                ToolKind::ClipboardViewer => Job::Clipboard,
                ToolKind::BorderlessWindow => Job::Borderless(f[0].clone(), choice == 0),
                ToolKind::MonitorTest | ToolKind::NetworkMonitor => return Err("nothing to run".into()),
            })
        })();
        let job = match parsed {
            Ok(j) => j,
            Err(e) => return self.finish(Err(e), cx),
        };
        if let Job::QrDecode = job {
            // Decoding picks its file first.
            let task = cx.background_spawn(async move {
                let Some(file) = rfd::AsyncFileDialog::new().pick_file().await else { return Ok(None) };
                match app.run_tool(t::ToolJob::QrDecodeFile(file.path().to_path_buf())).await {
                    Ok(t::ToolOutput::Lines(codes)) => Ok(Some(codes)),
                    Ok(_) => Ok(Some(Vec::new())),
                    Err(e) => Err(e.to_string()),
                }
            });
            self.busy = true;
            cx.spawn(async move |this, cx| {
                let r = task.await;
                let _ = this.update(cx, |this, cx| match r {
                    Ok(None) => this.finish(Ok((String::new(), None)), cx),
                    Ok(Some(codes)) if codes.is_empty() => this.finish(Ok(("No code found.".into(), None)), cx),
                    Ok(Some(codes)) => this.finish(Ok((codes.join("\n\n"), None)), cx),
                    Err(e) => this.finish(Err(e), cx),
                });
            })
            .detach();
            cx.notify();
            return;
        }
        self.busy = true;
        self.output = "Working…".into();
        cx.notify();
        let task = cx.background_spawn(async move { job.run(&app, &codecs).await });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| this.finish(r, cx));
        })
        .detach();
    }
}

enum Job {
    Colour(String),
    View(PathBuf),
    Compare(PathBuf, PathBuf, u8),
    Icon(PathBuf, Vec<u32>),
    Combine(Vec<PathBuf>, bool, u32),
    Split(PathBuf, u32, u32),
    Resize(Vec<PathBuf>, u32, u32, bool),
    Convert(Vec<PathBuf>, ImageFormat, u8),
    Watermark(Vec<PathBuf>, PathBuf, laqta_domain::effects::draw::Placement, f32),
    Thumbs(Vec<PathBuf>, u32, u32),
    Video(PathBuf, String, laqta_domain::record::VideoCodec, Option<u32>),
    Trim(PathBuf, f64, f64),
    VideoThumbs(PathBuf, u32),
    Gif(PathBuf, u32, u32),
    QrMake(String),
    QrDecode,
    Hash(PathBuf, String),
    Meta(PathBuf),
    Strip(Vec<PathBuf>),
    Index(PathBuf, usize, Option<u32>),
    Clipboard,
    Borderless(String, bool),
}

impl Job {
    async fn run(self, app: &laqta_app::App, codecs: &laqta_imaging::Codecs) -> Result<(String, Option<Bitmap>), String> {
        use t::{ToolJob as J, ToolOutput as O};
        let text = |s: String| Ok((s, None));
        // One tool run through the app. `$out` names the output shape this job produces.
        macro_rules! run {
            ($job:expr, $out:path) => {
                match app.run_tool($job).await.map_err(|e| e.to_string())? {
                    $out(v) => v,
                    other => return Err(format!("unexpected tool output: {other:?}")),
                }
            };
        }
        let done = |out: O| matches!(out, O::Done).then_some(()).ok_or("unexpected tool output");
        match self {
            Job::Colour(s) => {
                let c = t::parse_color(&s).ok_or("not a color")?;
                let f = t::ColorFormats::from_rgba(c);
                let px: Vec<u8> = std::iter::repeat_n([c.r, c.g, c.b, 255], 96 * 48).flatten().collect();
                let swatch = Bitmap::new(96, 48, PixelFormat::Rgba8, px.into()).ok();
                Ok(([f.hex, f.rgb, f.rgba, f.hsl, f.hsv, f.cmyk, f.decimal, f.linear].join("\n"), swatch))
            }
            Job::View(p) => {
                let b = decode_file(&p, codecs)?;
                Ok((format!("{} — {}×{}", p.display(), b.width(), b.height()), Some(b)))
            }
            Job::Compare(a, b, tol) => {
                let d = run!(J::Compare { a, b, tolerance: tol }, O::Diff);
                let pct = if d.total == 0 { 0.0 } else { d.changed as f64 * 100.0 / d.total as f64 };
                Ok((
                    format!("{} of {} pixels differ ({pct:.2}%); largest channel difference {}.", d.changed, d.total, d.max_delta),
                    Some(d.image),
                ))
            }
            Job::Icon(p, sizes) => {
                let out = with_suffix(&p, "", Some("ico"));
                let out = run!(J::Icon { path: p, sizes, out }, O::Path);
                text(format!("Wrote {}", out.display()))
            }
            Job::Combine(paths, horizontal, spacing) => {
                use laqta_imaging::tools::combine::{Align, Orientation};
                let o = if horizontal { Orientation::Horizontal } else { Orientation::Vertical };
                let out = dir_of(&paths[0]).join("combined.png");
                let out = run!(J::Combine { paths, orientation: o, align: Align::Start, spacing, out }, O::Path);
                let img = decode_file(&out, codecs).ok();
                Ok((format!("Wrote {}", out.display()), img))
            }
            Job::Split(p, rows, cols) => {
                let out_dir = dir_of(&p);
                text(list(&run!(J::Split { path: p, rows, cols, out_dir }, O::Paths)))
            }
            Job::Resize(paths, w, h, keep) => {
                let dir = dir_of(paths.first().ok_or("choose images first")?);
                text(list(&run!(J::Resize { paths, width: w, height: h, keep_aspect: keep, out_dir: dir }, O::Paths)))
            }
            Job::Convert(paths, format, q) => {
                let dir = dir_of(paths.first().ok_or("choose images first")?);
                text(list(&run!(J::Convert { paths, format, quality: q, out_dir: dir }, O::Paths)))
            }
            Job::Watermark(paths, mark, placement, opacity) => {
                let dir = dir_of(paths.first().ok_or("choose images first")?);
                text(list(&run!(J::Watermark { paths, mark, placement, opacity, out_dir: dir }, O::Paths)))
            }
            Job::Thumbs(paths, w, h) => {
                let dir = dir_of(paths.first().ok_or("choose images first")?);
                text(list(&run!(J::Thumbnails { paths, max_width: w, max_height: h, out_dir: dir }, O::Paths)))
            }
            Job::Video(p, ext, codec, width) => {
                let out = with_suffix(&p, "-converted", Some(&ext));
                let spec = laqta_ports::TranscodeSpec { codec: Some(codec), width, ..Default::default() };
                done(app.run_tool(J::ConvertVideo { input: p, output: out.clone(), spec }).await.map_err(|e| e.to_string())?)?;
                text(format!("Wrote {}", out.display()))
            }
            Job::Trim(p, start, dur) => {
                let out = with_suffix(&p, "-trimmed", None);
                done(
                    app.run_tool(J::TrimVideo { input: p, output: out.clone(), start_secs: start, duration_secs: dur })
                        .await
                        .map_err(|e| e.to_string())?,
                )?;
                text(format!("Wrote {}", out.display()))
            }
            Job::VideoThumbs(p, n) => {
                let out_dir = dir_of(&p);
                text(list(&run!(J::VideoThumbnails { input: p, out_dir, count: n }, O::Paths)))
            }
            Job::Gif(p, fps, w) => {
                let out = with_suffix(&p, "", Some("gif"));
                done(app.run_tool(J::Gif { input: p, output: out.clone(), fps, width: w }).await.map_err(|e| e.to_string())?)?;
                text(format!("Wrote {}", out.display()))
            }
            Job::QrMake(s) => {
                let b = t::qr_image(&s, 8).map_err(|e| e.to_string())?;
                Ok((s, Some(b)))
            }
            Job::QrDecode => text(String::new()),
            Job::Hash(p, expected) => {
                let rows = run!(J::Hash { path: p, kinds: t::HashKind::ALL.to_vec() }, O::Hashes);
                let mut out: Vec<String> = rows.iter().map(|(k, v)| format!("{k:?}: {v}")).collect();
                if !expected.is_empty() {
                    let hit = rows.iter().find(|(_, v)| t::compare_hash(&expected, v));
                    out.insert(
                        0,
                        match hit {
                            Some((k, _)) => format!("Match ({k:?})\n"),
                            None => "No match\n".into(),
                        },
                    );
                }
                text(out.join("\n"))
            }
            Job::Meta(p) => {
                let rows = run!(J::Metadata(p), O::Pairs);
                text(if rows.is_empty() {
                    "No metadata.".into()
                } else {
                    rows.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n")
                })
            }
            Job::Strip(paths) => {
                let dir = dir_of(paths.first().ok_or("choose files first")?);
                text(list(&run!(J::StripMetadata { paths, out_dir: dir }, O::Paths)))
            }
            Job::Index(dir, choice, depth) => {
                let format = [t::IndexFormat::Text, t::IndexFormat::Html, t::IndexFormat::Json, t::IndexFormat::Xml][choice];
                let options = t::IndexOptions { max_depth: depth, ..Default::default() };
                text(run!(J::Index { dir, format, options }, O::Text))
            }
            Job::Clipboard => match run!(J::Clipboard, O::Clipboard) {
                None => text("The clipboard is empty.".into()),
                Some(ClipboardContent::Text(s)) => text(format!("Text ({} characters):\n\n{s}", s.chars().count())),
                Some(ClipboardContent::Image(b)) => Ok((format!("Image {}×{}", b.width(), b.height()), Some(b))),
                Some(ClipboardContent::Files(f)) => {
                    text(format!("{} file(s):\n{}", f.len(), f.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n")))
                }
            },
            Job::Borderless(title, on) => {
                let title = run!(J::Borderless { title, on }, O::Window).ok_or("no window has that title")?;
                text(format!("{} \"{}\".", if on { "Removed the border of" } else { "Restored the border of" }, title))
            }
        }
    }
}

fn rate(bytes: u64) -> String {
    format!("{}/s", crate::presenters::history::format_size(bytes))
}

impl Render for ToolsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sp = spec(self.kind);
        let mut body = div()
            .flex()
            .flex_col()
            .gap(crate::theme::space::S3)
            .px(crate::theme::space::S8)
            .pt(crate::theme::space::S6)
            .pb(crate::theme::space::S8)
            .child(div().pb(crate::theme::space::S2).child(widgets::page_title(title(self.kind), sp.help, cx)));

        if self.kind == ToolKind::MonitorTest {
            let (colour, name) = TEST_COLOURS[self.colour_step % TEST_COLOURS.len()];
            return crate::chrome::rounded(widgets::page(cx), window)
                .child(crate::chrome::TitleBar::new().title(title(self.kind)).flat())
                .child(
                    div()
                        .id("monitor-test")
                        .flex_1()
                        .flex()
                        .items_end()
                        .justify_center()
                        .p(px(12.))
                        .bg(gpui::rgb(colour))
                        .text_color(if colour == 0xffffff || colour == 0xffff00 || colour == 0x00ffff {
                            gpui::black()
                        } else {
                            gpui::white()
                        })
                        .child(format!("{name} — click for the next color"))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.colour_step += 1;
                                cx.notify();
                            }),
                        ),
                );
        }
        if self.kind == ToolKind::NetworkMonitor {
            let mut table = div().flex().flex_col().gap(px(4.)).text_sm();
            table = table.child(
                div()
                    .flex()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(div().w(px(160.)).child("Interface"))
                    .child(div().w(px(110.)).child("Down"))
                    .child(div().w(px(110.)).child("Up"))
                    .child(div().w(px(110.)).child("Total down"))
                    .child(div().w(px(110.)).child("Total up")),
            );
            if self.net.is_empty() {
                table = table.child(widgets::muted("Sampling…", cx));
            }
            for (name, rx, tx, trx, ttx) in &self.net {
                table = table.child(
                    div()
                        .flex()
                        .child(div().w(px(160.)).truncate().child(name.clone()))
                        .child(div().w(px(110.)).child(rate(*rx)))
                        .child(div().w(px(110.)).child(rate(*tx)))
                        .child(div().w(px(110.)).child(crate::presenters::history::format_size(*trx)))
                        .child(div().w(px(110.)).child(crate::presenters::history::format_size(*ttx))),
                );
            }
            return crate::chrome::rounded(widgets::page(cx), window)
                .child(crate::chrome::TitleBar::new().title(title(self.kind)).flat())
                .child(body.child(table));
        }

        if sp.pick != Pick::Nothing {
            let label = match sp.pick {
                Pick::Files => "Choose files…",
                Pick::Folder => "Choose folder…",
                _ => "Choose file…",
            };
            let chosen = match self.files.len() {
                0 => "Nothing chosen".to_string(),
                1 => self.files[0].display().to_string(),
                n => format!("{n} files"),
            };
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(Button::new("pick").label(label).on_click(cx.listener(|this, _, _, cx| this.pick(false, cx))))
                    .child(div().flex_1().truncate().child(chosen)),
            );
        }
        if let Some(what) = sp.second {
            let chosen = self.second.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "Nothing chosen".into());
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(Button::new("pick2").label(format!("{what}…")).on_click(cx.listener(|this, _, _, cx| this.pick(true, cx))))
                    .child(div().flex_1().truncate().child(chosen)),
            );
        }
        for (i, (label, _)) in sp.fields.iter().enumerate() {
            body = body.child(widgets::row(*label, Input::new(&self.fields[i]), cx));
        }
        if !sp.choices.is_empty() {
            let mut row = div().flex().flex_wrap().gap(px(4.));
            for (i, c) in sp.choices.iter().enumerate() {
                row = row.child(
                    Button::new(SharedString::from(format!("choice-{i}"))).label(*c).small().selected(self.choice == i).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.choice = i;
                            cx.notify();
                        }),
                    ),
                );
            }
            body = body.child(row);
        }
        let mut actions = div().flex().gap(px(8.));
        if let Some(run) = sp.run {
            actions = actions
                .child(Button::new("run").label(run).primary().disabled(self.busy).on_click(cx.listener(|this, _, _, cx| this.run(cx))));
        }
        if self.kind == ToolKind::ColorPicker {
            actions = actions.child(Button::new("from-screen").label("Pick from screen").on_click(cx.listener(|_, _, _, cx| {
                let dispatch = cx.global::<UiState>().services.dispatch.clone();
                dispatch(laqta_domain::Action::ScreenColorPicker);
            })));
        }
        if !self.output.is_empty() {
            let out = self.output.clone();
            actions = actions.child(Button::new("copy").label("Copy result").ghost().on_click(move |_, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(out.clone()));
            }));
            if self.kind == ToolKind::IndexFolder {
                let out = self.output.clone();
                let ext = ["txt", "html", "json", "xml"][self.choice];
                actions = actions.child(Button::new("save").label("Save…").ghost().on_click(move |_, _, cx| {
                    let out = out.clone();
                    cx.background_spawn(async move {
                        if let Some(f) = rfd::AsyncFileDialog::new().set_file_name(format!("index.{ext}")).save_file().await
                            && let Err(e) = crate::presenters::output::write_file(f.path(), out.as_bytes())
                        {
                            tracing::warn!("index not saved: {e}");
                        }
                    })
                    .detach();
                }));
            }
        }
        body = body.child(actions);
        if let Some(image) = &self.image {
            body = body.child(
                div()
                    .flex()
                    .justify_center()
                    .p(px(12.))
                    .bg(cx.theme().secondary)
                    .rounded(crate::theme::radius::CARD)
                    .child(img(image.clone()).max_w_full().max_h(px(420.)).rounded(crate::theme::radius::MD)),
            );
        }
        if !self.output.is_empty() {
            body = body.child(
                div()
                    .p(px(12.))
                    .rounded(crate::theme::radius::LG)
                    .bg(cx.theme().secondary)
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_sm()
                    .whitespace_normal()
                    .child(self.output.clone()),
            );
        }
        crate::chrome::rounded(widgets::page(cx), window)
            .child(crate::chrome::TitleBar::new().title(title(self.kind)).flat())
            .child(div().flex_1().overflow_y_scrollbar().child(body))
    }
}
