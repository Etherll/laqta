//! Light, dark or system theme (ARCHITECTURE §5) applied through gpui-component.

use gpui::App;
use gpui_component::{Theme as KitTheme, ThemeMode};
use laqta_domain::settings::Theme;

pub fn apply(theme: Theme, cx: &mut App) {
    match theme {
        Theme::System => KitTheme::sync_system_appearance(None, cx),
        Theme::Light => KitTheme::change(ThemeMode::Light, None, cx),
        Theme::Dark => KitTheme::change(ThemeMode::Dark, None, cx),
    }
}
