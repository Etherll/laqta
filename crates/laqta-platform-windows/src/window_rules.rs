//! Pure rules for hover targets: which windows a region selection may snap to, their rects, and
//! the names the naming tokens see.

use laqta_domain::PhysRect;

/// The snap filter: visible, not minimized, not cloaked, and a tool window only when it
/// is also an app window.
pub(crate) fn is_snap_candidate(visible: bool, iconic: bool, cloaked: bool, tool_window: bool, app_window: bool) -> bool {
    visible && !iconic && !cloaked && (!tool_window || app_window)
}

/// A window rect from its edges (`right` and `bottom` exclusive), if it is larger than 1x1 pixel.
pub(crate) fn window_rect(left: i32, top: i32, right: i32, bottom: i32) -> Option<PhysRect> {
    let width = u32::try_from(i64::from(right) - i64::from(left)).ok().filter(|&w| w > 1)?;
    let height = u32::try_from(i64::from(bottom) - i64::from(top)).ok().filter(|&h| h > 1)?;
    Some(PhysRect { x: left, y: top, width, height })
}

/// The file name of an executable path: `C:\Apps\tool.exe` gives `tool.exe`.
pub(crate) fn exe_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_filter_truth_table() {
        // (visible, iconic, cloaked, tool, app) -> candidate
        assert!(is_snap_candidate(true, false, false, false, false));
        assert!(!is_snap_candidate(false, false, false, false, false), "hidden");
        assert!(!is_snap_candidate(true, true, false, false, false), "minimized");
        assert!(!is_snap_candidate(true, false, true, false, false), "cloaked");
        assert!(!is_snap_candidate(true, false, false, true, false), "tool window");
        assert!(is_snap_candidate(true, false, false, true, true), "tool window with WS_EX_APPWINDOW");
    }

    #[test]
    fn window_rect_needs_more_than_one_pixel_each_way() {
        assert_eq!(window_rect(10, 20, 12, 23), Some(PhysRect { x: 10, y: 20, width: 2, height: 3 }));
        assert_eq!(window_rect(0, 0, 1, 50), None, "1 px wide");
        assert_eq!(window_rect(0, 0, 50, 1), None, "1 px high");
        assert_eq!(window_rect(5, 5, 3, 9), None, "inverted");
    }

    #[test]
    fn window_rect_handles_extreme_edges() {
        assert_eq!(window_rect(i32::MIN, 0, i32::MAX, 2), Some(PhysRect { x: i32::MIN, y: 0, width: u32::MAX, height: 2 }));
    }

    #[test]
    fn exe_name_takes_the_last_path_component() {
        assert_eq!(exe_name(r"C:\Program Files\App\app.exe"), "app.exe");
        assert_eq!(exe_name("/usr/bin/tool"), "tool");
        assert_eq!(exe_name("app.exe"), "app.exe");
        assert_eq!(exe_name(r"C:\dir\"), "", "a directory has no exe name");
        assert_eq!(exe_name(""), "");
    }
}
