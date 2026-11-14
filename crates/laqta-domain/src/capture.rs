//! Capture modes and their options.

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
    /// Keep Laqta's own windows in captures: they stay up while a capture starts, and the region
    /// overlay can snap to them. Off, Laqta hides itself and is left out.
    pub include_own_windows: bool,
    pub region: RegionCaptureOptions,
    /// For `Action::CustomRegion` (physical desktop pixels).
    pub custom_region: Option<PhysRect>,
    /// For `Action::CustomWindow`: the first window whose title contains this text.
    pub custom_window_title: String,
    pub scrolling: ScrollingOptions,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        CaptureOptions {
            delay_ms: 0,
            show_cursor: false,
            transparent_window: false,
            hdr_tonemap: true,
            include_own_windows: false,
            region: RegionCaptureOptions::default(),
            custom_region: None,
            custom_window_title: String::new(),
            scrolling: ScrollingOptions::default(),
        }
    }
}

/// How scrolling capture scrolls the window under the region.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ScrollMethod {
    #[default]
    MouseWheel,
    DownArrow,
    PageDown,
    /// Windows `WM_VSCROLL` to the window under the region.
    ScrollMessage,
}

/// Options for scrolling capture.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ScrollingOptions {
    pub method: ScrollMethod,
    /// Wait before the first frame, so the user can let go of the mouse.
    pub start_delay_ms: u32,
    /// Wait after each scroll before the next frame.
    pub scroll_delay_ms: u32,
    /// Stop after this many frames even if the page keeps scrolling.
    pub max_frames: u32,
    /// Wheel notches or key presses per step.
    pub scroll_amount: u32,
    /// Rows at the top and bottom of the region that never move (sticky headers, status bars).
    pub ignore_top: u32,
    pub ignore_bottom: u32,
}

impl Default for ScrollingOptions {
    fn default() -> Self {
        ScrollingOptions {
            method: ScrollMethod::MouseWheel,
            start_delay_ms: 300,
            scroll_delay_ms: 300,
            max_frames: 50,
            scroll_amount: 2,
            ignore_top: 0,
            ignore_bottom: 0,
        }
    }
}

/// What a right or middle click does while the region overlay is idle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum RegionMouseAction {
    /// Nothing happens.
    None,
    /// Ends the session without a capture.
    CancelCapture,
    /// Removes the region under the cursor in multi-region mode, else cancels.
    #[default]
    RemoveShapeOrCancel,
    /// Removes the region under the cursor and keeps the session open.
    RemoveShape,
    /// Captures the whole desktop.
    CaptureFullscreen,
    /// Captures the monitor under the cursor.
    CaptureActiveMonitor,
    /// Confirms the selection, as Enter does.
    ConfirmSelection,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RegionCaptureOptions {
    pub multi_region: bool,
    /// Keep a drawn or clicked region as an adjustable selection with the capture tools beside it,
    /// instead of capturing as soon as the mouse is released.
    pub show_tools_after_select: bool,
    /// The bar at the top of the selection screen: screenshot or video, region, full screen or
    /// monitor, edit after capture, colour picker, text capture and close.
    pub show_mode_bar: bool,
    pub snap_to_windows: bool,
    /// Also snap to the regions inside a window (sidebars, panels, text boxes), found from the
    /// screenshot's pixels and merged with the accessibility tree.
    pub detect_regions: bool,
    pub show_magnifier: bool,
    pub magnifier_pixels: u32,
    pub show_crosshair: bool,
    pub show_info: bool,
    /// Template with `$x $y $r $g $b $hex $HEX $n`.
    pub info_text: String,
    pub dim_alpha: u8,
    pub min_size: u32,
    /// Right-click action while the overlay is idle.
    pub right_click: RegionMouseAction,
    /// Middle-click action while the overlay is idle.
    pub middle_click: RegionMouseAction,
}

/// The colour and size last picked for one capture-toolbar drawing tool, kept between runs.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MarkStyle {
    /// The editor tool's name (`Tool::name`), e.g. "Highlight".
    pub tool: String,
    pub color: crate::color::Rgba,
    /// 0 small, 1 medium, 2 large.
    pub size: u8,
}

impl Default for RegionCaptureOptions {
    fn default() -> Self {
        RegionCaptureOptions {
            multi_region: false,
            show_tools_after_select: true,
            show_mode_bar: true,
            snap_to_windows: true,
            detect_regions: true,
            show_magnifier: true,
            magnifier_pixels: 15,
            show_crosshair: false,
            show_info: true,
            info_text: "X: $x Y: $y$nR: $r G: $g B: $b$nHex: $hex".to_string(),
            dim_alpha: 100,
            min_size: 5,
            right_click: RegionMouseAction::RemoveShapeOrCancel,
            middle_click: RegionMouseAction::CancelCapture,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_json_without_mouse_actions_gets_the_shipped_defaults() {
        let options: RegionCaptureOptions = serde_json::from_str(r#"{"min_size": 7}"#).expect("old settings still load");
        assert_eq!(options.min_size, 7);
        assert_eq!(options.right_click, RegionMouseAction::RemoveShapeOrCancel);
        assert_eq!(options.middle_click, RegionMouseAction::CancelCapture);
    }

    #[test]
    fn tools_after_select_defaults_on_and_round_trips() {
        let old: RegionCaptureOptions = serde_json::from_str(r#"{"min_size": 7}"#).expect("old settings still load");
        assert!(old.show_tools_after_select, "settings saved before the option existed get the tools");
        assert!(old.show_mode_bar, "and the mode bar");

        let off: RegionCaptureOptions = serde_json::from_str(r#"{"show_tools_after_select": false}"#).expect("explicit value loads");
        assert!(!off.show_tools_after_select);

        let json = serde_json::to_string(&off).expect("serialises");
        let back: RegionCaptureOptions = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back, off);
    }

    #[test]
    fn including_own_windows_is_off_by_default_and_round_trips() {
        assert!(!CaptureOptions::default().include_own_windows);

        let old: CaptureOptions = serde_json::from_str(r#"{"delay_ms": 250}"#).expect("old settings still load");
        assert_eq!(old.delay_ms, 250);
        assert!(!old.include_own_windows, "settings saved before the option existed leave Laqta out");

        let on = CaptureOptions { include_own_windows: true, ..CaptureOptions::default() };
        let json = serde_json::to_string(&on).expect("serialises");
        let back: CaptureOptions = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back, on);
        assert!(back.include_own_windows);
    }

    #[test]
    fn mouse_actions_round_trip() {
        let options = RegionCaptureOptions {
            right_click: RegionMouseAction::CaptureFullscreen,
            middle_click: RegionMouseAction::ConfirmSelection,
            ..RegionCaptureOptions::default()
        };
        let json = serde_json::to_string(&options).expect("serialises");
        let back: RegionCaptureOptions = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back, options);
    }
}
