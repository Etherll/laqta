//! The region overlay (ADR 012): a fullscreen surface that shows frozen frames, forwards input and
//! paints the Region session's state. The GPUI adapter lives in `laqta-ui`.

use std::sync::Arc;

use laqta_domain::geometry::PhysRect;
use laqta_domain::region::{Handle, HoverTarget, RegionInput};
use laqta_domain::{Bitmap, Frame};

use crate::PortError;

/// Everything the overlay paints. Coordinates are physical virtual-desktop pixels.
#[derive(Clone, Debug, Default)]
pub struct RegionView {
    /// Union of all monitor rects; overlay pixel (0,0) is `bounds.(x, y)`.
    pub bounds: PhysRect,
    pub regions: Vec<PhysRect>,
    pub active: Option<usize>,
    pub drawing: Option<PhysRect>,
    pub hovered: Option<HoverTarget>,
    pub handles: Vec<(Handle, PhysRect)>,
    pub cursor: (i32, i32),
    pub magnifier: Option<PhysRect>,
    pub info_text: Option<String>,
    pub show_crosshair: bool,
    /// 0 = no dimming, 255 = black outside the selection.
    pub dim_alpha: u8,
    /// Annotate mode is on: the toolbar's Annotate button shows as pressed.
    pub annotate: bool,
    /// The region the session holds as the adjustable selection: the active region once the gesture
    /// that made it has ended, for a session that does not confirm on select. `None` while the
    /// user draws, moves or resizes, and in every other kind of session.
    pub selected: Option<PhysRect>,
    /// The desktop's pixels under `selected`, in physical pixels. It rides on the one view that
    /// follows a change of `selected`; later views leave it `None`, and the overlay keeps the image
    /// until `selected` changes again.
    pub selection_image: Option<Arc<Bitmap>>,
    /// The capture tools (toolbar and info badge) are shown; Space toggles them.
    pub tools_visible: bool,
    /// The selection is frozen because annotations exist on it.
    pub locked: bool,
    /// Show the mode bar at the top of the screen (`RegionCaptureOptions::show_mode_bar`).
    pub mode_bar: bool,
    /// The session picks one pixel (the screen colour picker): a click picks, nothing is drawn.
    pub pick_point: bool,
}

impl PartialEq for RegionView {
    /// `Bitmap` has no equality of its own, so the selection images compare by identity: the same
    /// crop is the same image, and two crops of equal pixels are not worth a pixel scan.
    fn eq(&self, other: &Self) -> bool {
        // Destructured so that a new field cannot be left out of the comparison.
        let Self {
            bounds,
            regions,
            active,
            drawing,
            hovered,
            handles,
            cursor,
            magnifier,
            info_text,
            show_crosshair,
            dim_alpha,
            annotate,
            selected,
            selection_image,
            tools_visible,
            locked,
            mode_bar,
            pick_point,
        } = self;
        let same_image = match (selection_image, &other.selection_image) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        same_image
            && *bounds == other.bounds
            && *regions == other.regions
            && *active == other.active
            && *drawing == other.drawing
            && *hovered == other.hovered
            && *handles == other.handles
            && *cursor == other.cursor
            && *magnifier == other.magnifier
            && *info_text == other.info_text
            && *show_crosshair == other.show_crosshair
            && *dim_alpha == other.dim_alpha
            && *annotate == other.annotate
            && *selected == other.selected
            && *tools_visible == other.tools_visible
            && *locked == other.locked
            && *mode_bar == other.mode_bar
            && *pick_point == other.pick_point
    }
}

pub type InputCallback = Arc<dyn Fn(RegionInput) + Send + Sync>;

pub trait Overlay: Send + Sync {
    /// Opens the overlay over `frames` and starts forwarding input to `on_input`.
    fn show(&self, frames: Vec<Frame>, view: RegionView, on_input: InputCallback) -> Result<(), PortError>;
    /// Repaints with a new view.
    fn update(&self, view: RegionView);
    /// Closes the overlay. Input stops.
    fn close(&self);
}

/// Windows and UI elements the overlay can snap to, front to back.
pub trait HoverSource: Send + Sync {
    fn targets(&self) -> crate::BoxFuture<'_, Result<Vec<HoverTarget>, PortError>>;
    /// The focused top-level window, if the platform can tell.
    fn active_window(&self) -> crate::BoxFuture<'_, Result<Option<HoverTarget>, PortError>>;
}

/// A window the dispatcher can open (ARCHITECTURE §2.2, windowed tools). Tools join in M5.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolWindow {
    Main,
    Settings,
    History,
    ImageHistory,
    /// The tray menu as a popup (`Action::ToggleTrayMenu`).
    TrayMenu,
    /// The floating actions toolbar (`Action::ToggleActionsToolbar`).
    ActionsToolbar,
    /// The effects window with live preview (`Action::ImageEffects`), optionally on a file.
    Effects(Option<std::path::PathBuf>),
    /// The image editor on a file (`Action::ImageEditor` with no capture).
    Editor(Option<std::path::PathBuf>),
    /// A windowed tool (M5).
    Tool(ToolKind),
    /// The photo editor on a file, or empty (M7).
    PhotoEditor(Option<std::path::PathBuf>),
}

/// The windowed tools (M5 D1). Each opens its own panel in the tools window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolKind {
    ColorPicker,
    ImageViewer,
    ImageComparer,
    IconConverter,
    ImageCombiner,
    ImageSplitter,
    ImageResizer,
    ImageConverter,
    ImageWatermark,
    ImageThumbnailer,
    VideoConverter,
    VideoTrimmer,
    VideoThumbnailer,
    GifMaker,
    GifTrimmer,
    QrCode,
    HashCheck,
    Metadata,
    StripMetadata,
    IndexFolder,
    ClipboardViewer,
    BorderlessWindow,
    MonitorTest,
    NetworkMonitor,
}

pub trait ToolHost: Send + Sync {
    fn open(&self, window: ToolWindow) -> Result<(), PortError>;

    /// Pins an image to the screen: a borderless, always-on-top window (M5).
    fn pin(&self, image: laqta_domain::Bitmap) -> Result<(), PortError> {
        let _ = image;
        Err(PortError::Unsupported("pin to screen".into()))
    }

    /// Opens the photo editor on an image (`OPEN_IN_PHOTO_EDITOR`). The task does not wait for it.
    fn photo_editor(&self, image: laqta_domain::Bitmap) -> Result<(), PortError> {
        let _ = image;
        Err(PortError::Unsupported("the photo editor".into()))
    }

    /// Closes every pinned image.
    fn close_pins(&self) -> Result<(), PortError> {
        Ok(())
    }

    /// Shows a colour picked from the screen in the colour picker.
    fn show_color(&self, color: laqta_domain::color::Rgba) -> Result<(), PortError> {
        let _ = color;
        Err(PortError::Unsupported("the colour picker".into()))
    }

    /// Shows a tool's text result (OCR, QR decode, ruler, inspector) with a copy button.
    fn show_text(&self, title: &str, text: &str) -> Result<(), PortError> {
        let _ = (title, text);
        Err(PortError::Unsupported("text results".into()))
    }

    /// Shows an OCR result beside the image it was read from: the text window for text capture. A host
    /// without a window of its own shows the recognised text with `show_text`.
    fn show_ocr(&self, image: laqta_domain::Bitmap, result: crate::OcrResult) -> Result<(), PortError> {
        let _ = image;
        let text = result.text();
        self.show_text("Recognised text", if text.is_empty() { "No text found." } else { &text })
    }
}

/// The annotation editor window (`AfterCaptureTasks::ANNOTATE_IMAGE`): returns the edited image when
/// the user continues, `Cancelled` when they close it.
pub trait EditorHost: Send + Sync {
    fn edit(&self, image: laqta_domain::Bitmap) -> crate::BoxFuture<'_, Result<laqta_domain::Bitmap, PortError>>;
}
