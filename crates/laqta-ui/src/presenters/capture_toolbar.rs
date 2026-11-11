//! The region overlay's toolbar as data: its buttons in order, their names and shortcuts, where
//! the bar sits next to the selection, and which key presses act on it.
//!
//! The drawing tools are the editor engine's, so the overlay and the editor share one tool set.

use laqta_editor::Tool;

/// One button on the capture toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// Back to adjusting the selection (no drawing tool).
    Region,
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
        std::iter::once(Item::Region).chain(TOOLS.iter().map(|t| Item::Tool(*t))).collect(),
        vec![Item::Undo, Item::Redo],
        vec![Item::CopyText],
        vec![Item::Pin, Item::Edit, Item::Upload, Item::Save, Item::Copy],
        vec![Item::Cancel, Item::Confirm],
    ]
}

/// The tooltip text.
pub fn label(item: Item) -> &'static str {
    match item {
        Item::Region => "Adjust selection",
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
        Item::Region => "V",
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
        "v" => Item::Region,
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
}
