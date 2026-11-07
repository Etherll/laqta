use laqta_domain::{Action, Settings};
use laqta_ports::{TrayItem, TrayMenu};

/// The tray menu: left click captures the screen.
pub fn tray_menu(_settings: &Settings) -> TrayMenu {
    let item = |label: &str, action| TrayItem { label: label.to_string(), action };
    TrayMenu {
        tooltip: "Laqta".to_string(),
        default_action: Action::PrintScreen,
        items: vec![
            item("Capture fullscreen", Action::PrintScreen),
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
        assert_eq!(
            got,
            [("Capture fullscreen", Action::PrintScreen), ("Open screenshots folder", Action::OpenScreenshotsFolder), ("Quit Laqta", Action::ExitLaqta)]
        );
    }
}
