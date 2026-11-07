//! Capture modes and their options (docs/plan/M1.md §4.1).

use crate::geometry::{MonitorId, PhysRect};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum CaptureMode {
    Fullscreen,
    Monitor(MonitorId),
    ActiveMonitor,
    ActiveWindow,
    /// A window whose title contains this text.
    Window(String),
    Region,
    CustomRegion(PhysRect),
    LastRegion,
}

/// Serde adapter for `MonitorId`, which has no serde derives in `geometry.rs`. It is stored as its string.

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CaptureOptions {
    pub delay_ms: u32,
    pub show_cursor: bool,
    pub transparent_window: bool,
    pub hdr_tonemap: bool,
    pub region: RegionCaptureOptions,
    /// For `Action::CustomRegion` (physical desktop pixels).
    pub custom_region: Option<PhysRect>,
    /// For `Action::CustomWindow`: the first window whose title contains this text.
    pub custom_window_title: String,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        CaptureOptions {
            delay_ms: 0,
            show_cursor: false,
            transparent_window: false,
            hdr_tonemap: true,
            region: RegionCaptureOptions::default(),
            custom_region: None,
            custom_window_title: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RegionCaptureOptions {
    pub multi_region: bool,
    pub snap_to_windows: bool,
    pub show_magnifier: bool,
    pub magnifier_pixels: u32,
    pub show_crosshair: bool,
    pub show_info: bool,
    /// Template with `$x $y $r $g $b $hex $HEX $n`.
    pub info_text: String,
    pub dim_alpha: u8,
    pub min_size: u32,
}

impl Default for RegionCaptureOptions {
    fn default() -> Self {
        RegionCaptureOptions {
            multi_region: false,
            snap_to_windows: true,
            show_magnifier: true,
            magnifier_pixels: 15,
            show_crosshair: false,
            show_info: true,
            info_text: "X: $x Y: $y$nR: $r G: $g B: $b$nHex: $hex".to_string(),
            dim_alpha: 100,
            min_size: 5,
        }
    }
}
