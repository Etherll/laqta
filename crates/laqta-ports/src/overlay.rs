//! The region overlay (ADR 012): a fullscreen surface that shows frozen frames, forwards input and
//! paints the Region session's state. The GPUI adapter lives in `laqta-ui`.

use std::sync::Arc;

use laqta_domain::Frame;
use laqta_domain::geometry::PhysRect;
use laqta_domain::region::{Handle, HoverTarget, RegionInput};

use crate::PortError;

/// Everything the overlay paints. Coordinates are physical virtual-desktop pixels.
#[derive(Clone, Debug, PartialEq, Default)]
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
}

pub trait ToolHost: Send + Sync {
    fn open(&self, window: ToolWindow) -> Result<(), PortError>;
}
