//! The region overlay's toolbar as data: its buttons in order, their names and shortcuts, where
//! the bar sits next to the selection, and which key presses act on it.
//!
//! The drawing tools are the editor engine's, so the overlay and the editor share one tool set.

use laqta_domain::capture::MarkStyle;
use laqta_domain::color::Rgba;
use laqta_domain::geometry::PhysRect;
use laqta_editor::{Style, Tool};

/// One button on the capture toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// The Pointer (no drawing tool): a press on a mark picks, moves or resizes it, and a press on
    /// empty space moves the selection with its marks (Alt leaves the marks in place).
    Pointer,
    Tool(Tool),
    Undo,
    Redo,
    /// Read the text in the selection in place, to pick words from and copy.
    CopyText,
    Pin,
    /// Open the capture in the image editor.
    Edit,
    Upload,
    Save,
    Copy,
    Cancel,
    /// Run the normal after-capture steps.
    Confirm,
}

/// The drawing tools on the bar, in order.
pub const TOOLS: [Tool; 9] =
    [Tool::Rectangle, Tool::Ellipse, Tool::Arrow, Tool::Freehand, Tool::Highlight, Tool::Text, Tool::Step, Tool::Blur, Tool::Pixelate];

/// The bar's groups, left to right; the view draws a divider between groups.
pub fn groups() -> Vec<Vec<Item>> {
    vec![
        [Item::Pointer].into_iter().chain(TOOLS.iter().map(|t| Item::Tool(*t))).collect(),
        vec![Item::Undo, Item::Redo],
        vec![Item::CopyText],
        vec![Item::Pin, Item::Edit, Item::Upload, Item::Save, Item::Copy],
        vec![Item::Cancel, Item::Confirm],
    ]
}

/// The tooltip text.
pub fn label(item: Item) -> &'static str {
    match item {
        Item::Pointer => "Pointer: move and resize marks, or drag empty space to move the selection (hold Space while drawing)",
        Item::Tool(t) => super::editor_toolbar::label(t),
        Item::Undo => "Undo",
        Item::Redo => "Redo",
        Item::CopyText => "Select and copy text (OCR)",
        Item::Pin => "Pin to screen",
        Item::Edit => "Open in editor",
        Item::Upload => "Upload",
        Item::Save => "Save",
        Item::Copy => "Copy",
        Item::Cancel => "Cancel",
        Item::Confirm => "Done",
    }
}

/// The shortcut shown next to the tooltip, if any.
pub fn shortcut(item: Item) -> Option<&'static str> {
    Some(match item {
        Item::Pointer => "V",
        Item::Tool(Tool::Rectangle) => "R",
        Item::Tool(Tool::Ellipse) => "E",
        Item::Tool(Tool::Arrow) => "A",
        Item::Tool(Tool::Freehand) => "F",
        Item::Tool(Tool::Highlight) => "H",
        Item::Tool(Tool::Text) => "T",
        Item::Tool(Tool::Step) => "I",
        Item::Tool(Tool::Blur) => "B",
        Item::Tool(Tool::Pixelate) => "P",
        Item::Tool(_) => return None,
        Item::Undo => "Ctrl+Z",
        Item::Redo => "Ctrl+Y",
        Item::CopyText => "Shift+C",
        Item::Pin => "Ctrl+T",
        Item::Edit => "Ctrl+E",
        Item::Upload => "Ctrl+U",
        Item::Save => "Ctrl+S",
        Item::Copy => "Ctrl+C",
        Item::Cancel => "Esc",
        Item::Confirm => "Enter",
    })
}

/// Modifier state for `key_item`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// The toolbar item a key press stands for, while the toolbar is shown. `key` is GPUI's lowercase
/// key name. Plain letters pick tools; Esc and Enter stay with the region session.
pub fn key_item(key: &str, mods: Mods) -> Option<Item> {
    if mods.alt {
        return None;
    }
    if mods.ctrl {
        return Some(match key {
            "z" if mods.shift => Item::Redo,
            "z" => Item::Undo,
            "y" => Item::Redo,
            "t" => Item::Pin,
            "e" => Item::Edit,
            "u" => Item::Upload,
            "s" => Item::Save,
            "c" => Item::Copy,
            _ => return None,
        });
    }
    if mods.shift {
        return (key == "c").then_some(Item::CopyText);
    }
    Some(match key {
        "v" | "m" => Item::Pointer,
        "r" => Item::Tool(Tool::Rectangle),
        "e" => Item::Tool(Tool::Ellipse),
        "a" => Item::Tool(Tool::Arrow),
        "f" => Item::Tool(Tool::Freehand),
        "h" => Item::Tool(Tool::Highlight),
        "t" => Item::Tool(Tool::Text),
        "i" => Item::Tool(Tool::Step),
        "b" => Item::Tool(Tool::Blur),
        "p" => Item::Tool(Tool::Pixelate),
        _ => return None,
    })
}

/// A rectangle in a window's logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn bottom(self) -> f32 {
        self.y + self.h
    }
    pub fn right(self) -> f32 {
        self.x + self.w
    }
}

/// Which side of the selection the toolbar ended up on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Below,
    Above,
    /// Neither fits: inside the selection, along its bottom edge.
    Inside,
}

/// Space between the selection and the bar, and between the bar and the screen edge.
pub const GAP: f32 = 8.0;

/// Where a `bar`-sized toolbar goes for `selection` on a `screen`-sized window: below the
/// selection, else above, else inside its bottom edge; right-aligned to the selection and kept on
/// screen.
pub fn place(selection: Rect, bar: (f32, f32), screen: (f32, f32)) -> (Rect, Side) {
    let (w, h) = bar;
    let (sw, sh) = screen;
    let (y, side) = if selection.bottom() + GAP + h + GAP <= sh {
        (selection.bottom() + GAP, Side::Below)
    } else if selection.y - GAP - h >= GAP {
        (selection.y - GAP - h, Side::Above)
    } else {
        ((selection.bottom() - GAP - h).clamp(GAP, (sh - h - GAP).max(GAP)), Side::Inside)
    };
    let x = (selection.right() - w).clamp(GAP, (sw - w - GAP).max(GAP));
    (Rect { x, y, w, h }, side)
}

/// Where the size badge goes: on the side of the selection away from the toolbar, left-aligned,
/// or inside the top-left corner when there is no room.
pub fn badge(selection: Rect, side: Side, badge: (f32, f32), screen: (f32, f32)) -> Rect {
    let (w, h) = badge;
    let x = selection.x.clamp(GAP, (screen.0 - w - GAP).max(GAP));
    let above = selection.y - GAP - h;
    let below = selection.bottom() + GAP;
    let y = match side {
        Side::Below | Side::Inside if above >= GAP => above,
        Side::Above if below + h + GAP <= screen.1 => below,
        _ => selection.y + GAP,
    };
    Rect { x, y, w, h }
}

/// A stroke or text size on the style strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Small,
    Medium,
    Large,
}

pub const SIZES: [Size; 3] = [Size::Small, Size::Medium, Size::Large];

impl Size {
    fn pick<T>(self, [s, m, l]: [T; 3]) -> T {
        match self {
            Size::Small => s,
            Size::Medium => m,
            Size::Large => l,
        }
    }

    pub fn index(self) -> u8 {
        match self {
            Size::Small => 0,
            Size::Medium => 1,
            Size::Large => 2,
        }
    }

    pub fn from_index(i: u8) -> Size {
        match i {
            0 => Size::Small,
            1 => Size::Medium,
            _ => Size::Large,
        }
    }

    /// The tooltip: blur and pixelate sizes are how strong the effect is.
    pub fn label(self, tool: Tool) -> &'static str {
        let strength = matches!(tool, Tool::Blur | Tool::Pixelate);
        match self {
            Size::Small if strength => "Light",
            Size::Large if strength => "Strong",
            Size::Small => "Small",
            Size::Medium => "Medium",
            Size::Large => "Large",
        }
    }
}

/// Ink colours for the pen and shapes.
const INK: [Rgba; 8] = [
    Rgba::rgb(0xef, 0x44, 0x44),
    Rgba::rgb(0xf9, 0x73, 0x16),
    Rgba::rgb(0xfa, 0xcc, 0x15),
    Rgba::rgb(0x22, 0xc5, 0x5e),
    Rgba::rgb(0x3b, 0x82, 0xf6),
    Rgba::rgb(0xa8, 0x55, 0xf7),
    Rgba::rgb(0x11, 0x11, 0x11),
    Rgba::rgb(0xff, 0xff, 0xff),
];

/// Marker colours: bright ones, which read through the see-through marker on light and dark
/// pictures (black would hide the text).
const MARKER: [Rgba; 5] = [
    Rgba::rgb(0xff, 0xee, 0x00),
    Rgba::rgb(0x9b, 0xf0, 0x6e),
    Rgba::rgb(0x7f, 0xd3, 0xff),
    Rgba::rgb(0xff, 0x9e, 0xcb),
    Rgba::rgb(0xff, 0xb8, 0x6b),
];

/// The colours the style strip offers for `tool`; empty when colour means nothing to it.
pub fn palette(tool: Tool) -> &'static [Rgba] {
    match tool {
        Tool::Highlight => &MARKER,
        Tool::Rectangle | Tool::Ellipse | Tool::Arrow | Tool::Line | Tool::Freehand | Tool::Text | Tool::Step => &INK,
        _ => &[],
    }
}

/// The name a swatch's tooltip shows.
pub fn color_name(c: Rgba) -> &'static str {
    const NAMES: [&str; 8] = ["Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Black", "White"];
    const MARKER_NAMES: [&str; 5] = ["Yellow", "Green", "Blue", "Pink", "Orange"];
    INK.iter().zip(NAMES).chain(MARKER.iter().zip(MARKER_NAMES)).find(|(k, _)| **k == c).map_or("Colour", |(_, n)| n)
}

/// Whether the style strip has anything for `tool`.
pub fn has_style(tool: Tool) -> bool {
    !palette(tool).is_empty() || matches!(tool, Tool::Blur | Tool::Pixelate)
}

/// The colour `tool` draws with in `style`.
pub fn color_of(style: &Style, tool: Tool) -> Rgba {
    match tool {
        Tool::Highlight => style.highlight_color,
        Tool::Text => style.font.color,
        _ => style.border_color,
    }
}

/// `style` with `tool`'s colour set to `color`. Text keeps its box's border colour: the strip
/// colours the letters.
pub fn with_color(style: &Style, tool: Tool, color: Rgba) -> Style {
    let mut s = style.clone();
    match tool {
        Tool::Highlight => s.highlight_color = color,
        Tool::Text => s.font.color = color,
        // A step's numeral sits on its colour: dark on light colours, white on the rest.
        Tool::Step => {
            s.border_color = color;
            s.font.color = numeral_on(color);
        }
        _ => s.border_color = color,
    }
    s
}

/// A numeral colour that reads on `fill`.
fn numeral_on(fill: Rgba) -> Rgba {
    let luma = 0.299 * f32::from(fill.r) + 0.587 * f32::from(fill.g) + 0.114 * f32::from(fill.b);
    if luma > 160.0 { INK[6] } else { Rgba::rgb(0xff, 0xff, 0xff) }
}

/// The small, medium and large values of the style field a size sets for `tool`.
fn size_values(tool: Tool) -> Option<[f32; 3]> {
    Some(match tool {
        Tool::Highlight => [12.0, 20.0, 32.0],
        Tool::Text => [16.0, 24.0, 36.0],
        // Medium is the editor's own step size, so a step starts on one the strip offers.
        Tool::Step => [14.0, 20.0, 28.0],
        Tool::Blur => [8.0, 15.0, 30.0],
        Tool::Pixelate => [6.0, 10.0, 18.0],
        Tool::Rectangle | Tool::Ellipse | Tool::Arrow | Tool::Line | Tool::Freehand => [2.0, 4.0, 8.0],
        _ => return None,
    })
}

/// The style value a size sets for `tool`.
fn size_value(style: &Style, tool: Tool) -> f32 {
    match tool {
        Tool::Highlight => style.highlight_size,
        Tool::Text | Tool::Step => style.font.size,
        Tool::Blur => style.blur_radius as f32,
        Tool::Pixelate => style.pixel_size as f32,
        _ => style.border_size,
    }
}

/// The size `tool` uses in `style`, when it is one the strip offers.
pub fn size_of(style: &Style, tool: Tool) -> Option<Size> {
    let values = size_values(tool)?;
    let now = size_value(style, tool);
    SIZES.into_iter().find(|size| size.pick(values) == now)
}

/// `style` with `tool`'s size set to `size`.
pub fn with_size(style: &Style, tool: Tool, size: Size) -> Style {
    let mut s = style.clone();
    let Some(values) = size_values(tool) else { return s };
    let v = size.pick(values);
    match tool {
        Tool::Highlight => s.highlight_size = v,
        Tool::Text | Tool::Step => s.font.size = v,
        Tool::Blur => s.blur_radius = v as u32,
        Tool::Pixelate => s.pixel_size = v as u32,
        _ => s.border_size = v,
    }
    s
}

/// The style a tool starts with on the capture toolbar. Text gets letters in the first ink colour
/// and no box around them, which reads on light and dark pictures alike; the editor's default
/// (white letters in a red box) vanishes on a light page.
pub fn base_style(tool: Tool) -> Style {
    let mut s = Style::default();
    if tool == Tool::Text {
        s.border_size = 0.0;
        s.font.color = INK[0];
        s.font.bold = true;
        s.font.size = Size::Medium.pick([16.0, 24.0, 36.0]);
    }
    s
}

/// The style `tool` draws with: its starting style with the colour and size last picked for it.
pub fn style_for(tool: Tool, marks: &[MarkStyle]) -> Style {
    let base = base_style(tool);
    match marks.iter().find(|m| m.tool == tool.name()) {
        Some(m) => with_size(&with_color(&base, tool, m.color), tool, Size::from_index(m.size)),
        None => base,
    }
}

/// Records `style` as `tool`'s picked colour and size.
pub fn remember(marks: &mut Vec<MarkStyle>, tool: Tool, style: &Style) {
    // A size the strip doesn't offer keeps the one picked before (or Medium): a colour pick alone
    // never changes the size.
    let earlier = marks.iter().find(|m| m.tool == tool.name()).map(|m| m.size);
    let size = size_of(style, tool).map(Size::index).or(earlier).unwrap_or(Size::Medium.index());
    let mark = MarkStyle { tool: tool.name().to_owned(), color: color_of(style, tool), size };
    match marks.iter_mut().find(|m| m.tool == mark.tool) {
        Some(m) => *m = mark,
        None => marks.push(mark),
    }
}

/// The colour a number key picks for `tool`: 1 is the first swatch.
pub fn color_key(key: &str, tool: Tool) -> Option<Rgba> {
    let n: usize = key.parse().ok()?;
    palette(tool).get(n.checked_sub(1)?).copied()
}

/// Where the style strip goes: on the side of the toolbar away from the selection (the other side
/// when that one has no room), centred under the chosen tool's button at `anchor_x`, and kept on
/// screen.
pub fn strip(bar: Rect, side: Side, strip: (f32, f32), screen: (f32, f32), anchor_x: f32) -> Rect {
    let (w, h) = strip;
    let below = bar.bottom() + GAP / 2.0;
    let above = bar.y - GAP / 2.0 - h;
    let fits_below = below + h + GAP <= screen.1;
    let fits_above = above >= GAP;
    let y = match side {
        Side::Below if fits_below || !fits_above => below,
        Side::Below => above,
        Side::Above | Side::Inside if fits_above || !fits_below => above,
        _ => below,
    };
    let x = (anchor_x - w / 2.0).clamp(GAP, (screen.0 - w - GAP).max(GAP));
    Rect { x, y, w, h }
}

/// The size badge's text: the selection's size, then its origin when given.
pub fn badge_text(rect: PhysRect, origin: Option<(i32, i32)>) -> String {
    let size = format!("{} × {}", rect.width, rect.height);
    match origin {
        Some((x, y)) => format!("{size}  ·  {x}, {y}"),
        None => size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_sets_each_tools_own_colour_and_size() {
        let base = Style::default();
        let red = Rgba::rgb(0xef, 0x44, 0x44);
        let yellow = MARKER[0];
        assert_eq!(with_color(&base, Tool::Highlight, yellow).highlight_color, yellow);
        assert_eq!(with_color(&base, Tool::Text, red).font.color, red);
        assert_eq!(with_color(&base, Tool::Text, red).border_color, base.border_color, "text keeps its box");
        assert_eq!(color_of(&with_color(&base, Tool::Arrow, INK[4]), Tool::Arrow), INK[4]);

        assert_eq!(size_of(&base, Tool::Rectangle), Some(Size::Medium), "the default border is medium");
        assert_eq!(size_of(&base, Tool::Highlight), Some(Size::Medium));
        let big = with_size(&base, Tool::Freehand, Size::Large);
        assert_eq!(big.border_size, 8.0);
        assert_eq!(size_of(&big, Tool::Freehand), Some(Size::Large));
        assert_eq!(with_size(&base, Tool::Pixelate, Size::Small).pixel_size, 6);
        assert_eq!(size_of(&with_size(&base, Tool::Blur, Size::Large), Tool::Blur), Some(Size::Large));
        assert_eq!(color_name(INK[4]), "Blue");
        assert_eq!(color_name(MARKER[3]), "Pink");
        assert_eq!(Size::Large.label(Tool::Blur), "Strong");
        assert_eq!(Size::Large.label(Tool::Freehand), "Large");
        assert!(!has_style(Tool::Select) && has_style(Tool::Blur) && palette(Tool::Blur).is_empty());
    }

    #[test]
    fn picks_are_remembered_per_tool_and_text_starts_readable() {
        let text = base_style(Tool::Text);
        assert_eq!((text.border_size, text.font.color), (0.0, INK[0]), "no box, red letters");
        assert_eq!(size_of(&text, Tool::Text), Some(Size::Medium));

        let mut marks = Vec::new();
        remember(
            &mut marks,
            Tool::Freehand,
            &with_size(&with_color(&base_style(Tool::Freehand), Tool::Freehand, INK[4]), Tool::Freehand, Size::Large),
        );
        remember(&mut marks, Tool::Highlight, &with_color(&base_style(Tool::Highlight), Tool::Highlight, MARKER[2]));
        remember(&mut marks, Tool::Freehand, &with_color(&base_style(Tool::Freehand), Tool::Freehand, INK[3]));
        assert_eq!(marks.len(), 2, "one entry per tool");
        let pen = style_for(Tool::Freehand, &marks);
        assert_eq!((pen.border_color, size_of(&pen, Tool::Freehand)), (INK[3], Some(Size::Medium)));
        assert_eq!(style_for(Tool::Highlight, &marks).highlight_color, MARKER[2]);
        assert_eq!(style_for(Tool::Rectangle, &marks), base_style(Tool::Rectangle), "an unpicked tool keeps its start");
    }

    #[test]
    fn a_step_starts_on_a_size_the_strip_offers() {
        let step = base_style(Tool::Step);
        assert_eq!(size_of(&step, Tool::Step), Some(Size::Medium));
        let mut marks = Vec::new();
        remember(&mut marks, Tool::Step, &with_color(&step, Tool::Step, INK[4]));
        assert_eq!(style_for(Tool::Step, &marks).font.size, step.font.size, "a colour pick leaves the size alone");
    }

    #[test]
    fn a_step_s_numeral_reads_on_every_swatch() {
        for &c in palette(Tool::Step) {
            let s = with_color(&base_style(Tool::Step), Tool::Step, c);
            assert_ne!(s.font.color, s.border_color, "{c:?}");
        }
        assert_eq!(with_color(&Style::default(), Tool::Step, INK[7]).font.color, INK[6], "dark on white");
        assert_eq!(with_color(&Style::default(), Tool::Step, INK[0]).font.color, Rgba::rgb(0xff, 0xff, 0xff), "white on red");
    }

    #[test]
    fn number_keys_pick_swatches() {
        assert_eq!(color_key("1", Tool::Arrow), Some(INK[0]));
        assert_eq!(color_key("8", Tool::Arrow), Some(INK[7]));
        assert_eq!(color_key("6", Tool::Highlight), None, "the marker has five");
        assert_eq!(color_key("0", Tool::Arrow), None);
        assert_eq!(color_key("2", Tool::Blur), None);
    }

    #[test]
    fn the_strip_opens_under_the_chosen_tool_on_the_side_with_room() {
        let bar = rect(600.0, 608.0, 600.0, 44.0);
        let s = strip(bar, Side::Below, (300.0, 40.0), SCREEN, 700.0);
        assert!(s.y >= bar.bottom(), "below the bar");
        assert_eq!(s.x + s.w / 2.0, 700.0, "centred on the tool");
        assert!(strip(bar, Side::Above, (300.0, 40.0), SCREEN, 700.0).bottom() <= bar.y);
        let low = rect(600.0, 1080.0 - 52.0, 600.0, 44.0);
        assert!(strip(low, Side::Below, (300.0, 40.0), SCREEN, 700.0).bottom() <= low.y, "no room below: above the bar");
        let s = strip(rect(0.0, 100.0, 600.0, 44.0), Side::Below, (300.0, 40.0), SCREEN, 20.0);
        assert_eq!(s.x, GAP, "kept on screen near the left edge");
    }

    const SCREEN: (f32, f32) = (1920.0, 1080.0);
    const BAR: (f32, f32) = (600.0, 44.0);

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn the_bar_sits_below_then_above_then_inside() {
        let (r, side) = place(rect(400.0, 200.0, 800.0, 400.0), BAR, SCREEN);
        assert_eq!(side, Side::Below);
        assert_eq!((r.x, r.y), (600.0, 608.0), "right-aligned under the selection");

        let (r, side) = place(rect(400.0, 600.0, 800.0, 460.0), BAR, SCREEN);
        assert_eq!(side, Side::Above);
        assert_eq!(r.y, 600.0 - GAP - 44.0);

        let (r, side) = place(rect(0.0, 0.0, 1920.0, 1080.0), BAR, SCREEN);
        assert_eq!(side, Side::Inside);
        assert!(r.bottom() <= 1080.0 - GAP + 0.01 && r.y >= GAP);
    }

    #[test]
    fn the_bar_stays_on_screen_for_a_small_selection_at_the_left_edge() {
        let (r, _) = place(rect(2.0, 100.0, 40.0, 40.0), BAR, SCREEN);
        assert_eq!(r.x, GAP);
        let (r, _) = place(rect(1900.0, 100.0, 20.0, 20.0), BAR, SCREEN);
        assert_eq!(r.right(), 1920.0 - GAP);
    }

    #[test]
    fn the_badge_takes_the_other_side() {
        let sel = rect(400.0, 200.0, 800.0, 400.0);
        assert!(badge(sel, Side::Below, (90.0, 24.0), SCREEN).bottom() <= sel.y);
        assert!(badge(sel, Side::Above, (90.0, 24.0), SCREEN).y >= sel.bottom());
        let full = rect(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(badge(full, Side::Inside, (90.0, 24.0), SCREEN).y, GAP);
    }

    #[test]
    fn keys_map_to_items_and_agree_with_the_tooltips() {
        let plain = Mods::default();
        let ctrl = Mods { ctrl: true, ..Mods::default() };
        assert_eq!(key_item("r", plain), Some(Item::Tool(Tool::Rectangle)));
        assert_eq!(key_item("c", ctrl), Some(Item::Copy));
        assert_eq!(key_item("z", Mods { ctrl: true, shift: true, alt: false }), Some(Item::Redo));
        assert_eq!(key_item("c", Mods { shift: true, ..Mods::default() }), Some(Item::CopyText));
        assert_eq!(key_item("enter", plain), None, "Enter stays with the region session");
        for item in groups().into_iter().flatten() {
            let Some(s) = shortcut(item) else { continue };
            let (mods, key) = match s.rsplit_once('+') {
                Some((m, k)) => (Mods { ctrl: m.contains("Ctrl"), shift: m.contains("Shift"), alt: false }, k.to_lowercase()),
                None => (Mods::default(), s.to_lowercase()),
            };
            if matches!(item, Item::Cancel | Item::Confirm) {
                continue;
            }
            assert_eq!(key_item(&key, mods), Some(item), "{s} for {item:?}");
        }
    }

    #[test]
    fn the_badge_gives_the_size_and_the_origin_when_asked() {
        let phys = PhysRect { x: 40, y: 60, width: 1280, height: 720 };
        assert_eq!(badge_text(phys, None), "1280 × 720");
        assert_eq!(badge_text(phys, Some((40, 60))), "1280 × 720  ·  40, 60");
    }
}
