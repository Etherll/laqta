//! The region overlay's toolbar as data: its buttons in order, their names and shortcuts, where
//! the bar sits next to the selection, and which key presses act on it.
//!
//! The drawing tools are the editor engine's, so the overlay and the editor share one tool set.

use laqta_domain::color::Rgba;
use laqta_domain::geometry::PhysRect;
use laqta_editor::{Style, Tool};

/// One button on the capture toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// Move the selection, its marks with it (no editor tool). Alt leaves the marks in place.
    Move,
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
        [Item::Move, Item::Tool(Tool::Select)].into_iter().chain(TOOLS.iter().map(|t| Item::Tool(*t))).collect(),
        vec![Item::Undo, Item::Redo],
        vec![Item::CopyText],
        vec![Item::Pin, Item::Edit, Item::Upload, Item::Save, Item::Copy],
        vec![Item::Cancel, Item::Confirm],
    ]
}

/// The tooltip text.
pub fn label(item: Item) -> &'static str {
    match item {
        Item::Move => "Move the selection with its marks (hold Alt to leave the marks)",
        Item::Tool(Tool::Select) => "Select, move and resize marks",
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
        Item::Move => "M",
        Item::Tool(Tool::Select) => "V",
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
        "m" => Item::Move,
        "v" => Item::Tool(Tool::Select),
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

/// The colour swatches, in order.
pub const PALETTE: [Rgba; 7] = [
    Rgba::rgb(0xff, 0x3b, 0x30),
    Rgba::rgb(0xff, 0x95, 0x00),
    Rgba::rgb(0xff, 0xcc, 0x00),
    Rgba::rgb(0x34, 0xc7, 0x59),
    Rgba::rgb(0x3d, 0x8b, 0xfd),
    Rgba::rgb(0xff, 0xff, 0xff),
    Rgba::rgb(0x00, 0x00, 0x00),
];

/// The style controls a tool shows under the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleRow {
    pub colors: bool,
    pub sizes: bool,
}

/// The style row for `tool`, or `None` when there is no tool or the bar does not offer it.
/// Blur and Pixelate have sizes only; Highlight has colours only.
pub fn style_row(tool: Option<Tool>) -> Option<StyleRow> {
    let tool = tool?;
    if !TOOLS.contains(&tool) {
        return None;
    }
    Some(match tool {
        Tool::Blur | Tool::Pixelate => StyleRow { colors: false, sizes: true },
        Tool::Highlight => StyleRow { colors: true, sizes: false },
        _ => StyleRow { colors: true, sizes: true },
    })
}

/// `style` with the tool's colour set to `c`. Highlight keeps its alpha, Text sets the font
/// colour, and the other tools set the border colour.
pub fn with_color(style: &Style, tool: Tool, c: Rgba) -> Style {
    let mut out = style.clone();
    match tool {
        Tool::Highlight => out.highlight_color = Rgba { a: style.highlight_color.a, ..c },
        Tool::Text => out.font.color = c,
        _ => out.border_color = c,
    }
    out
}

/// The colour the tool reads, from the same field `with_color` writes.
pub fn color_of(style: &Style, tool: Tool) -> Rgba {
    match tool {
        Tool::Highlight => style.highlight_color,
        Tool::Text => style.font.color,
        _ => style.border_color,
    }
}

/// How many size steps the bar offers.
pub const SIZES: usize = 3;

/// `style` with the tool's size set to step `index`, where 0 is the smallest. An index past the
/// last step takes the last one.
pub fn with_size(style: &Style, tool: Tool, index: usize) -> Style {
    let i = index.min(SIZES - 1);
    let mut out = style.clone();
    match tool {
        Tool::Blur => out.blur_radius = [4, 8, 16][i],
        Tool::Pixelate => out.pixel_size = [6, 10, 16][i],
        Tool::Text => out.font.size = [16.0, 24.0, 36.0][i],
        _ => out.border_size = [2.0, 4.0, 8.0][i],
    }
    out
}

/// The size step `style` holds for the tool, if it is one of the steps.
pub fn size_index(style: &Style, tool: Tool) -> Option<usize> {
    match tool {
        Tool::Blur => [4, 8, 16].iter().position(|&v| v == style.blur_radius),
        Tool::Pixelate => [6, 10, 16].iter().position(|&v| v == style.pixel_size),
        Tool::Text => [16.0, 24.0, 36.0].iter().position(|&v| v == style.font.size),
        _ => [2.0, 4.0, 8.0].iter().position(|&v| v == style.border_size),
    }
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
    fn the_palette_is_seven_opaque_colours_from_red_to_black() {
        assert_eq!(PALETTE.len(), 7);
        assert_eq!(PALETTE[0], Rgba::rgb(0xff, 0x3b, 0x30));
        assert_eq!(PALETTE[6], Rgba::rgb(0, 0, 0));
        assert!(PALETTE.iter().all(|c| c.a == 255));
    }

    #[test]
    fn the_style_row_depends_on_the_tool() {
        let both = Some(StyleRow { colors: true, sizes: true });
        assert_eq!(style_row(None), None);
        assert_eq!(style_row(Some(Tool::Select)), None, "not on the bar");
        assert_eq!(style_row(Some(Tool::Rectangle)), both);
        assert_eq!(style_row(Some(Tool::Text)), both);
        assert_eq!(style_row(Some(Tool::Blur)), Some(StyleRow { colors: false, sizes: true }));
        assert_eq!(style_row(Some(Tool::Pixelate)), Some(StyleRow { colors: false, sizes: true }));
        assert_eq!(style_row(Some(Tool::Highlight)), Some(StyleRow { colors: true, sizes: false }));
    }

    #[test]
    fn a_colour_lands_in_the_field_the_tool_draws_with() {
        let style = Style::default();
        let red = PALETTE[0];
        assert_eq!(with_color(&style, Tool::Rectangle, red).border_color, red);
        assert_eq!(with_color(&style, Tool::Text, red).font.color, red);
        assert_eq!(with_color(&style, Tool::Text, red).border_color, style.border_color);
        let kept = Style { highlight_color: Rgba { a: 128, ..Rgba::BLACK }, ..Style::default() };
        assert_eq!(with_color(&kept, Tool::Highlight, red).highlight_color, Rgba { a: 128, ..red });
    }

    #[test]
    fn a_colour_reads_back_from_the_field_it_was_written_to() {
        let style = Style { highlight_color: Rgba::rgb(0xff, 0xee, 0x00), ..Style::default() };
        for tool in TOOLS {
            assert_eq!(color_of(&with_color(&style, tool, PALETTE[2]), tool), PALETTE[2], "{tool:?}");
        }
    }

    #[test]
    fn sizes_step_through_the_tools_own_fields() {
        let style = Style::default();
        assert_eq!(with_size(&style, Tool::Blur, 2).blur_radius, 16);
        assert_eq!(with_size(&style, Tool::Pixelate, 0).pixel_size, 6);
        assert_eq!(with_size(&style, Tool::Text, 1).font.size, 24.0);
        assert_eq!(with_size(&style, Tool::Rectangle, 2).border_size, 8.0);
        assert_eq!(with_size(&style, Tool::Rectangle, 9).border_size, 8.0, "past the last step takes the last");
    }

    #[test]
    fn a_size_index_reads_back_its_step_and_is_none_off_the_steps() {
        let style = Style::default();
        for tool in TOOLS {
            for i in 0..SIZES {
                assert_eq!(size_index(&with_size(&style, tool, i), tool), Some(i), "{tool:?} step {i}");
            }
        }
        assert_eq!(size_index(&style, Tool::Blur), None, "the default radius 15 is not a step");
        assert_eq!(size_index(&style, Tool::Text), None, "the default font size 20 is not a step");
    }

    #[test]
    fn the_badge_gives_the_size_and_the_origin_when_asked() {
        let phys = PhysRect { x: 40, y: 60, width: 1280, height: 720 };
        assert_eq!(badge_text(phys, None), "1280 × 720");
        assert_eq!(badge_text(phys, Some((40, 60))), "1280 × 720  ·  40, 60");
    }
}
