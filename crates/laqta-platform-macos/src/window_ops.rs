//! Window tools (`WindowOps`). The inspector reads the window server's description of a window.
//! Making another app's window borderless or always on top is not possible on macOS.

use laqta_domain::region::HoverTarget;
use laqta_ports::{PortError, WindowOps};
use objc2_core_graphics::{CGWindowListOption, kCGNullWindowID};

use crate::hover::{WindowInfo, window_list};

#[derive(Debug, Default)]
pub struct MacWindowOps;

impl WindowOps for MacWindowOps {
    // macOS has no public API to change the style or level of a window that belongs to another
    // process: AppKit only reaches the app's own windows, and the Accessibility API can move,
    // resize and raise a window but not restyle it or pin it above the rest. Only private
    // WindowServer calls could, and they are off limits for a shipped app.
    fn set_borderless(&self, _window: &HoverTarget, _borderless: bool) -> Result<(), PortError> {
        Err(unsupported())
    }

    fn set_topmost(&self, _window: &HoverTarget, _topmost: bool) -> Result<(), PortError> {
        Err(unsupported())
    }

    /// Needs no permission: the window server describes every window to every process.
    fn inspect(&self, window: &HoverTarget) -> Result<Vec<(String, String)>, PortError> {
        let id = u32::try_from(window.handle)
            .ok()
            .filter(|&id| id != 0)
            .ok_or_else(|| PortError::Failed("the target has no window handle".into()))?;
        let info = describe(id).ok_or_else(|| PortError::Failed("the window is gone".into()))?;
        Ok(properties(&info))
    }
}

fn unsupported() -> PortError {
    PortError::Unsupported("macOS does not let one app change another app's window".into())
}

/// The window server's entry for window `id`, on screen or not.
fn describe(id: u32) -> Option<WindowInfo> {
    // Asking for the one window is cheap. If the call does not answer that way, search them all.
    window_list(CGWindowListOption::OptionIncludingWindow, id)
        .and_then(|windows| windows.into_iter().find(|window| window.id == id))
        .or_else(|| window_list(CGWindowListOption::OptionAll, kCGNullWindowID)?.into_iter().find(|window| window.id == id))
}

/// The inspector's (name, value) pairs. The rect is in points, as the window server reports it.
fn properties(info: &WindowInfo) -> Vec<(String, String)> {
    let frame = info.frame;
    let mut pairs = vec![
        ("title".to_owned(), info.name.clone()),
        ("process".to_owned(), info.owner.clone()),
        ("pid".to_owned(), info.pid.to_string()),
        ("window id".to_owned(), info.id.to_string()),
        ("rect (points)".to_owned(), format!("{},{},{},{}", frame.x, frame.y, frame.width, frame.height)),
        ("layer".to_owned(), info.layer.to_string()),
        ("on screen".to_owned(), if info.on_screen { "yes" } else { "no" }.to_owned()),
    ];
    if let Some(bytes) = info.memory {
        pairs.push(("memory".to_owned(), format!("{bytes} bytes")));
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ax::Frame;

    fn info(memory: Option<i64>) -> WindowInfo {
        WindowInfo {
            id: 42,
            pid: 311,
            owner: "Notes".into(),
            name: "Groceries".into(),
            layer: 0,
            alpha: 1.0,
            frame: Frame { x: 100.0, y: 50.0, width: 300.5, height: 200.0 },
            on_screen: true,
            memory,
        }
    }

    #[test]
    fn properties_describe_the_window() {
        let pairs = properties(&info(Some(4096)));
        let get = |name: &str| pairs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
        assert_eq!(get("title"), Some("Groceries"));
        assert_eq!(get("process"), Some("Notes"));
        assert_eq!(get("pid"), Some("311"));
        assert_eq!(get("window id"), Some("42"));
        assert_eq!(get("rect (points)"), Some("100,50,300.5,200"));
        assert_eq!(get("layer"), Some("0"));
        assert_eq!(get("on screen"), Some("yes"));
        assert_eq!(get("memory"), Some("4096 bytes"));
    }

    #[test]
    fn memory_is_left_out_when_the_window_server_gives_none() {
        assert!(properties(&info(None)).iter().all(|(name, _)| name != "memory"));
    }

    #[test]
    fn restyling_another_apps_window_is_unsupported() {
        let target = HoverTarget {
            rect: laqta_domain::geometry::PhysRect { x: 0, y: 0, width: 10, height: 10 },
            handle: 42,
            title: String::new(),
            process: String::new(),
            kind: laqta_domain::region::TargetKind::Window,
        };
        assert!(matches!(MacWindowOps.set_topmost(&target, true), Err(PortError::Unsupported(_))));
        assert!(matches!(MacWindowOps.set_borderless(&target, true), Err(PortError::Unsupported(_))));
    }
}
