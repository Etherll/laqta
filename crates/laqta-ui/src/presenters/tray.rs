use laqta_domain::{Action, Settings};
use laqta_ports::{TrayItem, TrayMenu};

/// The tray menu. Left click runs `settings.tray_click` (default: capture the screen).
pub fn tray_menu(settings: &Settings) -> TrayMenu {
    let item = |label: &str, action| TrayItem { label: crate::i18n::action(action, label), action };
    TrayMenu {
        tooltip: "Laqta".to_string(),
        default_action: settings.tray_click,
        items: vec![
            item("Capture region", Action::RectangleRegion),
            item("Capture fullscreen", Action::PrintScreen),
            item("Capture active window", Action::ActiveWindow),
            item("Capture active monitor", Action::ActiveMonitor),
            item("Capture last region", Action::LastRegion),
            item("Open main window", Action::OpenMainWindow),
            item("History", Action::OpenHistory),
            item("Settings", Action::OpenSettings),
            item("Open screenshots folder", Action::OpenScreenshotsFolder),
            item("Quit Laqta", Action::ExitLaqta),
        ],
    }
}

/// Tooltip used when hotkey registration returned `Unsupported` (M0 §5.4).
pub const TOOLTIP_NO_HOTKEY: &str = "Laqta — bind `laqta --action print-screen` in your desktop's keyboard settings";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_items_and_default() {
        let menu = tray_menu(&Settings::m0_default("/p".into()));
        assert_eq!(menu.tooltip, "Laqta");
        assert_eq!(menu.default_action, Action::PrintScreen);
        let got: Vec<_> = menu.items.iter().map(|i| (i.label.as_str(), i.action)).collect();
        assert_eq!(got.len(), 10);
        assert!(got.contains(&("Capture fullscreen", Action::PrintScreen)));
        assert!(got.contains(&("Open screenshots folder", Action::OpenScreenshotsFolder)));
        assert_eq!(got.last(), Some(&("Quit Laqta", Action::ExitLaqta)));
    }
}
