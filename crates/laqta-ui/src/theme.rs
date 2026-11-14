//! The design system (ARCHITECTURE §5): one token set (colour, radius, spacing, type scale) with
//! light, dark and system modes, applied through gpui-component theming.
//!
//! Views read colours from `cx.theme()` and geometry from the constants here, never literals.
//! Routine chrome stays neutral grey; the green `primary` is for the one key action in a region.

use gpui::{App, Hsla, Pixels, Rgba, px, rgb};
use gpui_component::{Theme as KitTheme, ThemeMode};
use laqta_domain::settings::Theme;

/// Fixed radius scale. Radii do not grow with UI size. Anything one line tall is a pill; nested
/// tiers are concentric (outer = inner + inset).
#[allow(dead_code)] // the full scale; views adopt it window by window
pub(crate) mod radius {
    use gpui::{Pixels, px};
    /// Tiny tiles and swatches.
    pub const SM: Pixels = px(8.);
    /// Rows in menus and lists, tiles inside a card.
    pub const MD: Pixels = px(12.);
    /// Compact multi-line boxes: thumbnails, code blocks.
    pub const LG: Pixels = px(16.);
    /// Popovers, list rows that hold two lines, boxes inside a card.
    pub const XL: Pixels = px(20.);
    /// Cards and panels.
    pub const CARD: Pixels = px(24.);
    /// Toasts.
    pub const TOAST: Pixels = px(18.);
    /// Dialogs and large floating surfaces.
    pub const DIALOG: Pixels = px(28.);
    pub const PILL: Pixels = px(9999.);
}

/// 4px spacing rhythm.
pub(crate) mod space {
    use gpui::{Pixels, px};
    pub const S1: Pixels = px(4.);
    pub const S2: Pixels = px(8.);
    pub const S3: Pixels = px(12.);
    pub const S4: Pixels = px(16.);
    pub const S5: Pixels = px(20.);
    pub const S6: Pixels = px(24.);
    pub const S8: Pixels = px(32.);
    pub const S10: Pixels = px(40.);
}

/// Type scale (effective sizes at the default UI size).
#[allow(dead_code)] // the full scale; views adopt it window by window
pub(crate) mod text {
    use gpui::{Pixels, px};
    pub const CAPTION: Pixels = px(11.25);
    /// Small controls and secondary lines.
    pub const SMALL: Pixels = px(12.5);
    pub const LABEL: Pixels = px(13.125);
    /// Row titles and nav labels.
    pub const ROW: Pixels = px(14.);
    pub const BODY: Pixels = px(15.);
    pub const SECTION: Pixels = px(16.875);
    pub const HEADING: Pixels = px(18.75);
    pub const PAGE_TITLE: Pixels = px(31.875);
    /// Body and operational text.
    pub const UI_FONT: &str = "Inter";
    /// Headings and page titles. The bundled file names its family this way.
    pub const HEADING_FONT: &str = "Space Grotesk Light";
}

/// Motion durations (ms). Feedback 150, routine 200, large or spatial 300. Callers skip motion when
/// `cx.reduce_motion()` is set.
#[allow(dead_code)] // the full scale; views adopt it window by window
pub(crate) mod motion {
    pub const FAST: u64 = 150;
    pub const BASE: u64 = 200;
    pub const SLOW: u64 = 300;
}

/// Icon sizes: inline with small text, the default, and toolbar glyphs.
#[allow(dead_code)] // the full scale; views adopt it window by window
pub(crate) mod icon {
    use gpui::{Pixels, px};
    pub const SM: Pixels = px(14.);
    pub const MD: Pixels = px(16.);
    pub const LG: Pixels = px(18.);
}

/// Row and card actions at rest: visible, so keyboard and touch users can find them, but quieter
/// than on hover.
pub(crate) const QUIET_OPACITY: f32 = 0.6;

/// How far a disabled control fades. It keeps its tab stop and says why in its tooltip.
pub(crate) const DISABLED_OPACITY: f32 = 0.45;

/// One-line control height (buttons, inputs, nav rows are pills of this height).
pub(crate) const CONTROL_H: Pixels = px(36.);
/// Sidebar nav row height.
pub(crate) const NAV_ROW_H: Pixels = px(33.);
pub(crate) const SIDEBAR_W: Pixels = px(248.);

/// The brand green, for the one key action in a region. Text on it is `INK`.
pub(crate) const PRIMARY: u32 = 0x17B88B;
/// Ink on the brand green and on dark-mode danger: 7.4:1 and 6.5:1.
pub(crate) const INK: u32 = 0x0F1114;
/// Selections and work in progress (`Extra::info`). Surfaces that always sit on a dark scrim (the
/// capture overlay) read these as raw values.
pub(crate) const INFO: u32 = 0x3B82F6;

/// Colours the gpui-component palette has no slot for.
pub(crate) struct Extra {
    /// Card surface (white in light, raised grey in dark).
    pub card: Hsla,
    /// Hairline ring around cards: foreground at 10%.
    pub ring: Hsla,
    /// Neutral hover/selection wash: foreground at 5%.
    pub wash: Hsla,
    /// Ink for the strong neutral ("dark") action.
    pub action: Hsla,
    /// Icon tile tint behind section icons (primary at 12%).
    pub chip: Hsla,
    /// The stage behind an image being edited: a step away from the chrome, so the image reads
    /// as the subject.
    pub stage: Hsla,
    /// The library's card surface: a step above the page.
    pub surface: Hsla,
    /// Work in progress and selections (busy badges, the editor's selection, pin borders).
    pub info: Hsla,
    /// Paused or waiting. A dot, border or fill only: under 3:1 on the light page as text.
    pub warning: Hsla,
    /// Done or healthy. A dot or icon beside a word, never the only signal.
    pub success: Hsla,
    /// The window Close button's hover.
    pub close_hover: Hsla,
}

pub(crate) fn extra(cx: &App) -> Extra {
    let t = KitTheme::global(cx);
    let fg = t.foreground;
    let dark = t.is_dark();
    Extra {
        card: if dark { hex(0x1F1F1F) } else { hex(0xFFFFFF) },
        ring: fg.opacity(if dark { 0.09 } else { 0.10 }),
        wash: fg.opacity(0.05),
        action: if dark { fg } else { hex(0x1A1C1F) },
        chip: t.primary.opacity(0.12),
        stage: if dark { hex(0x131313) } else { hex(0xEAEAE8) },
        surface: if dark { hex(0x232323) } else { hex(0xFFFFFF) },
        info: hex(INFO),
        warning: hex(0xEAB308),
        success: hex(0x16A34A),
        close_hover: hex(0xC42B1C),
    }
}

/// The keyboard focus ring: foreground at 60%, at least 4.5:1 on every surface in both modes.
pub(crate) fn focus_ring(cx: &App) -> Hsla {
    KitTheme::global(cx).foreground.opacity(0.6)
}

pub fn apply(theme: Theme, cx: &mut App) {
    match theme {
        Theme::System => KitTheme::sync_system_appearance(None, cx),
        Theme::Light => KitTheme::change(ThemeMode::Light, None, cx),
        Theme::Dark => KitTheme::change(ThemeMode::Dark, None, cx),
    }
    // `change` loads the registered kit palette; lay ours over it in a second update.
    KitTheme::update(cx, |t| {
        let p = if t.is_dark() { Palette::DARK } else { Palette::LIGHT };
        p.write(t);
        t.radius = radius::XL;
        t.radius_lg = radius::DIALOG;
        t.shadow = false;
    });
}

/// Re-applies `Theme::System` when the OS switches between light and dark. Every window reports
/// the change, so it only acts when the kit theme no longer matches.
pub(crate) fn follow_system(theme: Theme, window: &gpui::Window, cx: &mut App) {
    if theme != Theme::System {
        return;
    }
    let dark = matches!(window.appearance(), gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark);
    if KitTheme::global(cx).is_dark() != dark {
        apply(Theme::System, cx);
    }
}

/// Registers the bundled UI fonts with GPUI and makes them the theme fonts. Called when the first
/// window opens, so the tray-only process never loads them (ARCHITECTURE §1.4).
pub(crate) fn ensure_fonts(cx: &mut App) {
    struct Loaded;
    impl gpui::Global for Loaded {}
    if cx.has_global::<Loaded>() {
        return;
    }
    cx.set_global(Loaded);
    let fonts: Vec<std::borrow::Cow<'static, [u8]>> = vec![
        include_bytes!("../assets/fonts/inter-400.ttf").as_slice().into(),
        include_bytes!("../assets/fonts/inter-500.ttf").as_slice().into(),
        include_bytes!("../assets/fonts/inter-600.ttf").as_slice().into(),
        include_bytes!("../assets/fonts/space-grotesk-500.ttf").as_slice().into(),
        include_bytes!("../assets/fonts/space-grotesk-600.ttf").as_slice().into(),
    ];
    match cx.text_system().add_fonts(fonts) {
        Ok(()) => KitTheme::update(cx, |t| t.font_family = text::UI_FONT.into()),
        Err(e) => tracing::warn!("bundled fonts failed to load, keeping the system font: {e:#}"),
    }
}

fn hex(v: u32) -> Hsla {
    rgb(v).into()
}

fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b): (Rgba, Rgba) = (a.into(), b.into());
    Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }.into()
}

struct Palette {
    background: u32,
    foreground: u32,
    surface: u32,
    /// Titlebar and sidebar: the frame the content sheet sits in.
    chrome: u32,
    secondary: u32,
    secondary_fg: u32,
    muted: u32,
    muted_fg: u32,
    accent: u32,
    border: u32,
    /// Text field edges: at least 3:1 against the page (WCAG 1.4.11), unlike the hairline `border`.
    input: u32,
    ring: u32,
    danger: u32,
}

impl Palette {
    const LIGHT: Palette = Palette {
        background: 0xFEFEFD,
        foreground: 0x0F1114,
        surface: 0xFFFFFF,
        chrome: 0xF6F6F5,
        secondary: 0xF2F2F2,
        secondary_fg: 0x262626,
        muted: 0xF5F5F5,
        muted_fg: 0x6A6A6A,
        accent: 0xECECEC,
        border: 0xE5E5E5,
        input: 0x8F8F8F,
        ring: 0x8A8A8B,
        danger: 0xCE4035,
    };
    const DARK: Palette = Palette {
        background: 0x181818,
        foreground: 0xFCFCFC,
        surface: 0x272727,
        chrome: 0x202020,
        secondary: 0x303030,
        secondary_fg: 0xFCFCFC,
        muted: 0x303030,
        muted_fg: 0xB0B0B0,
        accent: 0x3A3A3A,
        border: 0x303030,
        input: 0x707070,
        ring: 0x7A7A7A,
        danger: 0xED716A,
    };

    fn write(&self, t: &mut KitTheme) {
        let fg = hex(self.foreground);
        let bg = hex(self.background);
        let primary = hex(PRIMARY);
        let white = hex(0xFFFFFF);
        // Text on the green and on the dark-mode danger: white reads at 2.5:1 and 2.9:1 there,
        // this ink at 7.4:1 and 6.5:1 (WCAG 1.4.3).
        let ink = hex(INK);
        let c = &mut t.colors;
        c.background = bg;
        c.foreground = fg;
        c.border = hex(self.border);
        c.input = hex(self.input);
        c.ring = hex(self.ring);
        c.caret = fg;
        c.selection = fg.opacity(0.15);
        c.muted = hex(self.muted);
        c.muted_foreground = hex(self.muted_fg);
        c.accent = hex(self.accent);
        c.accent_foreground = fg;
        c.popover = hex(self.surface);
        c.popover_foreground = fg;
        c.group_box = hex(self.surface);
        c.group_box_foreground = fg;
        c.sidebar = hex(self.chrome);
        c.sidebar_foreground = fg;
        c.sidebar_accent = hex(self.accent);
        c.sidebar_accent_foreground = fg;
        c.sidebar_border = hex(self.border);
        c.sidebar_primary = primary;
        c.sidebar_primary_foreground = ink;
        c.primary = primary;
        c.primary_hover = mix(primary, bg, 0.12);
        c.primary_active = mix(primary, fg, 0.12);
        c.primary_foreground = ink;
        c.secondary = hex(self.secondary);
        c.secondary_hover = hex(self.accent);
        c.secondary_active = mix(hex(self.accent), fg, 0.06);
        c.secondary_foreground = hex(self.secondary_fg);
        c.button = hex(self.secondary);
        c.button_hover = hex(self.accent);
        c.button_active = mix(hex(self.accent), fg, 0.06);
        c.button_foreground = hex(self.secondary_fg);
        c.button_primary = primary;
        c.button_primary_hover = mix(primary, bg, 0.12);
        c.button_primary_active = mix(primary, fg, 0.12);
        c.button_primary_foreground = ink;
        c.button_secondary = hex(self.secondary);
        c.button_secondary_hover = hex(self.accent);
        c.button_secondary_active = mix(hex(self.accent), fg, 0.06);
        c.button_secondary_foreground = hex(self.secondary_fg);
        c.danger = hex(self.danger);
        c.danger_hover = mix(hex(self.danger), bg, 0.12);
        c.danger_active = mix(hex(self.danger), fg, 0.12);
        c.danger_foreground = if self.background == Palette::DARK.background { ink } else { white };
        c.list_hover = fg.opacity(0.05);
        c.list_active = fg.opacity(0.08);
        c.list_active_border = hex(self.border);
        c.tab_bar = hex(self.secondary);
        c.tab_active = hex(self.surface);
        c.tab_active_foreground = fg;
        c.tab_foreground = hex(self.muted_fg);
        c.switch = fg.opacity(0.09);
        c.switch_thumb = white;
        c.slider_bar = primary;
        c.slider_thumb = white;
        c.progress_bar = primary;
        c.link = fg;
        c.link_hover = fg.opacity(0.8);
        c.link_active = fg;
        c.scrollbar = Hsla::transparent_black();
        c.scrollbar_thumb = fg.opacity(0.18);
        c.scrollbar_thumb_hover = fg.opacity(0.28);
        c.drop_target = fg.opacity(0.05);
        c.drag_border = fg.opacity(0.3);
        c.skeleton = hex(self.muted);
    }
}
