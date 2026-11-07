//! Dispatcher routes for the M5 tools. Windowed tools open `ToolWindow::Tool`; capture-based tools
//! (screen colour picker, ruler, pin, OCR, QR scan) run a Region session first and show the result.

use std::sync::Arc;

use laqta_domain::Action;
use laqta_domain::workflow::TaskSettings;
use laqta_ports::{AppError, ClipboardContent, ToolKind, ToolWindow};

use crate::dispatcher::{App, Dispatched, Ports};
use crate::region::{self, Selection};

/// The windowed tool an action opens, if it is one.
pub(crate) fn tool_kind(action: Action) -> Option<ToolKind> {
    Some(match action {
        Action::ColorPicker => ToolKind::ColorPicker,
        Action::ImageViewer => ToolKind::ImageViewer,
        Action::ImageComparer => ToolKind::ImageComparer,
        Action::IconConverter => ToolKind::IconConverter,
        Action::ImageCombiner => ToolKind::ImageCombiner,
        Action::ImageSplitter => ToolKind::ImageSplitter,
        Action::ImageResizer => ToolKind::ImageResizer,
        Action::ImageConverter => ToolKind::ImageConverter,
        Action::ImageWatermark => ToolKind::ImageWatermark,
        Action::ImageThumbnailer => ToolKind::ImageThumbnailer,
        Action::VideoConverter => ToolKind::VideoConverter,
        Action::VideoTrimmer => ToolKind::VideoTrimmer,
        Action::VideoThumbnailer => ToolKind::VideoThumbnailer,
        Action::GifMaker => ToolKind::GifMaker,
        Action::GifTrimmer => ToolKind::GifTrimmer,
        Action::QrCode => ToolKind::QrCode,
        Action::HashCheck => ToolKind::HashCheck,
        Action::Metadata => ToolKind::Metadata,
        Action::StripMetadata => ToolKind::StripMetadata,
        Action::IndexFolder => ToolKind::IndexFolder,
        Action::ClipboardViewer => ToolKind::ClipboardViewer,
        Action::BorderlessWindow => ToolKind::BorderlessWindow,
        Action::MonitorTest => ToolKind::MonitorTest,
        Action::NetworkMonitor => ToolKind::NetworkMonitor,
        _ => return None,
    })
}

/// The pixel at the centre of a selection (a click selects one pixel).
fn centre_pixel(sel: &Selection) -> laqta_domain::color::Rgba {
    let img = &sel.image;
    let (x, y) = (img.width() / 2, img.height() / 2);
    let i = ((y * img.width() + x) * 4) as usize;
    let p = img.pixels();
    let (r, b) = match img.format() {
        laqta_domain::PixelFormat::Rgba8 => (p[i], p[i + 2]),
        laqta_domain::PixelFormat::Bgra8 => (p[i + 2], p[i]),
    };
    laqta_domain::color::Rgba { r, g: p[i + 1], b, a: p[i + 3] }
}

impl App {
    /// The ports, for the tool windows that call `laqta_app::tools` functions directly.
    pub fn ports(&self) -> &Ports {
        &self.ports
    }

    async fn select_on_screen(&self, ts: &TaskSettings) -> Result<Selection, AppError> {
        let frames = self.frames(ts).await?;
        region::select(&self.ports, frames, &ts.capture.region).await
    }

    fn show(&self, title: &str, text: &str) -> Result<Dispatched, AppError> {
        self.ports.tools.show_text(title, text).map_err(AppError::from)?;
        Ok(Dispatched::Done)
    }

    /// `None` when `action` is not a tool route.
    pub(crate) async fn tool_route(self: &Arc<Self>, action: Action, ts: &TaskSettings) -> Option<Result<Dispatched, AppError>> {
        if let Some(kind) = tool_kind(action) {
            return Some(self.ports.tools.open(ToolWindow::Tool(kind)).map(|()| Dispatched::Done).map_err(AppError::from));
        }
        let r = match action {
            Action::ScreenColorPicker => self.screen_color(ts).await,
            Action::Ruler => self.ruler(ts).await,
            Action::PinToScreen | Action::PinToScreenFromScreen => self.pin_from_screen(ts).await,
            Action::PinToScreenFromClipboard => self.pin_from_clipboard(),
            Action::PinToScreenFromFile => self.pin_from_file().await,
            Action::PinToScreenCloseAll => self.ports.tools.close_pins().map(|()| Dispatched::Done).map_err(AppError::from),
            Action::Ocr => self.ocr_region(ts).await,
            Action::QrCodeDecodeFromScreen => match crate::tools::qr_decode_screen(&self.ports).await {
                Ok(found) => self.show_codes(found),
                Err(e) => Err(e),
            },
            Action::QrCodeScanRegion => self.qr_region(ts).await,
            Action::ActiveWindowBorderless => crate::tools::toggle_borderless_active(&self.ports, &self.window_toggles).await.map(|_| Dispatched::Done),
            Action::ActiveWindowTopMost => crate::tools::toggle_topmost_active(&self.ports, &self.window_toggles).await.map(|_| Dispatched::Done),
            Action::InspectWindow => match crate::tools::inspect_active(&self.ports).await {
                Ok(props) => {
                    let text = props.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n");
                    self.show("Window properties", &text)
                }
                Err(e) => Err(e),
            },
            Action::MouseHighlighter => self.ports.tools.toggle_mouse_highlighter().map(|_| Dispatched::Done).map_err(AppError::from),
            _ => return None,
        };
        Some(r)
    }

    async fn screen_color(&self, ts: &TaskSettings) -> Result<Dispatched, AppError> {
        let sel = self.select_on_screen(ts).await?;
        let formats = crate::tools::ColorFormats::from_rgba(centre_pixel(&sel));
        let _ = self.ports.clipboard.set_text(&formats.hex);
        let text = [&formats.hex, &formats.rgb, &formats.hsl, &formats.hsv, &formats.cmyk, &formats.decimal].map(|s| s.as_str()).join("\n");
        self.show("Screen colour (hex copied)", &text)
    }

    async fn ruler(&self, ts: &TaskSettings) -> Result<Dispatched, AppError> {
        let sel = self.select_on_screen(ts).await?;
        let Some(r) = sel.rects.first().copied() else { return Ok(Dispatched::Done) };
        let m = crate::tools::measure((r.x, r.y), (r.x + r.width as i32, r.y + r.height as i32));
        let text = format!(
            "Position: {}, {}\nWidth: {} px\nHeight: {} px\nDiagonal: {:.1} px\nAngle: {:.1}°\nArea: {} px²\nPerimeter: {} px",
            r.x, r.y, m.width, m.height, m.diagonal, m.angle_deg, m.area, m.perimeter
        );
        self.show("Ruler", &text)
    }

    async fn pin_from_screen(&self, ts: &TaskSettings) -> Result<Dispatched, AppError> {
        let sel = self.select_on_screen(ts).await?;
        self.ports.tools.pin(sel.image).map_err(AppError::from)?;
        Ok(Dispatched::Done)
    }

    fn pin_from_clipboard(&self) -> Result<Dispatched, AppError> {
        match self.ports.clipboard.get().map_err(AppError::from)? {
            Some(ClipboardContent::Image(image)) => {
                self.ports.tools.pin(image).map_err(AppError::from)?;
                Ok(Dispatched::Done)
            }
            _ => Err(AppError::Unsupported("the clipboard has no image".into())),
        }
    }

    async fn pin_from_file(&self) -> Result<Dispatched, AppError> {
        let paths = match self.ports.prompt.pick_files(Action::PinToScreenFromFile.label()).await {
            Ok(p) => p,
            Err(laqta_ports::PortError::Cancelled) => return Ok(Dispatched::Done),
            Err(e) => return Err(AppError::from(e)),
        };
        for path in paths {
            let bytes = crate::files::read(self.ports.spawner.as_ref(), path).await.map_err(|e| AppError::Io(e.to_string()))?;
            let image = self.ports.codecs.decode(&bytes).map_err(|e| AppError::Io(e.to_string()))?;
            self.ports.tools.pin(image).map_err(AppError::from)?;
        }
        Ok(Dispatched::Done)
    }

    async fn ocr_region(&self, ts: &TaskSettings) -> Result<Dispatched, AppError> {
        let sel = self.select_on_screen(ts).await?;
        let result = crate::tools::ocr_image(&self.ports, sel.image, "").await?;
        let text = result.text();
        if !text.is_empty() {
            let _ = self.ports.clipboard.set_text(&text);
        }
        self.show("Recognised text (copied)", if text.is_empty() { "No text found." } else { &text })
    }

    async fn qr_region(&self, ts: &TaskSettings) -> Result<Dispatched, AppError> {
        let sel = self.select_on_screen(ts).await?;
        let found = crate::tools::qr_decode_image(&self.ports, sel.image).await?;
        self.show_codes(found)
    }

    fn show_codes(&self, found: Vec<String>) -> Result<Dispatched, AppError> {
        if found.is_empty() {
            return self.show("QR code", "No QR code or barcode found.");
        }
        let _ = self.ports.clipboard.set_text(&found.join("\n"));
        self.show("QR code (copied)", &found.join("\n\n"))
    }
}
