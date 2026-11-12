//! The annotation editor's toolbar as data: which tools sit in which group, their names and
//! hints, and which style controls apply to the active tool.

use laqta_editor::Tool;

/// A group of tools on the tool rail, separated from the next by a divider.
#[derive(Debug)]
pub struct ToolGroup {
    pub name: &'static str,
    pub tools: Vec<Tool>,
}

const GROUPS: [(&str, &[Tool]); 6] = [
    ("Select", &[Tool::Select]),
    ("Draw", &[Tool::Rectangle, Tool::Ellipse, Tool::Line, Tool::Arrow, Tool::Freehand]),
    ("Text", &[Tool::Text, Tool::SpeechBalloon, Tool::Step]),
    (
        "Hide and focus",
        &[Tool::Blur, Tool::Pixelate, Tool::Highlight, Tool::Magnify, Tool::Spotlight, Tool::SmartEraser, Tool::ObjectEraser],
    ),
    ("Insert", &[Tool::Image, Tool::Emoji, Tool::Cursor, Tool::Clone]),
    ("Canvas", &[Tool::Crop, Tool::CutOut]),
];

/// The rail's groups without the tools the user hid. Groups left empty are dropped. Select is
/// never hidden, so there is always a way back to it.
pub fn groups(hidden: &[String]) -> Vec<ToolGroup> {
    GROUPS
        .iter()
        .map(|(name, tools)| ToolGroup {
            name,
            tools: tools.iter().copied().filter(|t| *t == Tool::Select || !hidden.iter().any(|h| h == t.name())).collect(),
        })
        .filter(|g| !g.tools.is_empty())
        .collect()
}

/// Every tool the user can show or hide, in rail order.
pub fn hideable() -> impl Iterator<Item = Tool> {
    GROUPS.iter().flat_map(|(_, tools)| tools.iter().copied()).filter(|t| *t != Tool::Select)
}

/// The name a tool shows in tooltips and menus.
pub fn label(tool: Tool) -> &'static str {
    match tool {
        Tool::Freehand => "Pen",
        Tool::SmartEraser => "Eraser",
        Tool::Magnify => "Magnifier",
        t => t.name(),
    }
}

/// One line on how to use the tool, shown in the properties bar when it has no style controls.
pub fn hint(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => "Click an annotation to select it. Drag to move, use the handles to resize.",
        Tool::Rectangle => "Drag to draw a rectangle. Hold Shift for a square.",
        Tool::Ellipse => "Drag to draw an ellipse. Hold Shift for a circle.",
        Tool::Line => "Drag to draw a line.",
        Tool::Arrow => "Drag from the tail to the point.",
        Tool::Freehand => "Drag to draw freehand.",
        Tool::Text => "Click to place text, then type.",
        Tool::SpeechBalloon => "Drag to draw a balloon, then type.",
        Tool::Step => "Click to place numbered steps: 1, 2, 3…",
        Tool::Image => "Click to place the picked image.",
        Tool::Clone => "Drag over an area to copy it, then place the copy.",
        Tool::Emoji => "Pick an emoji, then click to place it.",
        Tool::Cursor => "Click to place a mouse cursor.",
        Tool::Highlight => "Draw over text to highlight it, like a marker.",
        Tool::SmartEraser => "Drag to paint over with the colour around the area.",
        Tool::ObjectEraser => "Drag a box around an object to remove it and fill in the background.",
        Tool::Blur => "Drag over an area to blur it.",
        Tool::Pixelate => "Drag over an area to pixelate it.",
        Tool::Magnify => "Drag over an area to magnify it.",
        Tool::Spotlight => "Drag over the area to keep lit; the rest is dimmed.",
        Tool::Crop => "Drag the area to keep, then press Enter.",
        Tool::CutOut => "Drag a band to cut out of the image, then press Enter.",
    }
}

/// The single-key shortcut that picks a tool, if it has one. The engine owns the key map
/// (`Tool::shortcut`), so the tooltip and the engine cannot disagree. The view handles these
/// keys only when no text is being edited.
pub fn shortcut(tool: Tool) -> Option<char> {
    tool.shortcut()
}

/// The tool a plain key press picks, if any.
pub fn tool_for_key(key: char) -> Option<Tool> {
    Tool::ALL.into_iter().find(|t| shortcut(*t) == Some(key))
}

/// Which style controls the properties bar shows for a tool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StyleControls {
    pub color: bool,
    pub stroke: bool,
    pub fill: bool,
    pub shadow: bool,
    pub font: bool,
}

pub fn style_controls(tool: Tool) -> StyleControls {
    let all = StyleControls { color: true, stroke: true, fill: true, shadow: true, font: true };
    match tool {
        Tool::Select => all,
        Tool::Rectangle | Tool::Ellipse => StyleControls { font: false, ..all },
        Tool::Line | Tool::Arrow | Tool::Freehand => StyleControls { fill: false, font: false, ..all },
        Tool::Text => StyleControls { stroke: false, fill: false, ..all },
        Tool::SpeechBalloon | Tool::Step => StyleControls { stroke: false, ..all },
        _ => StyleControls::default(),
    }
}

#[cfg(test)]
mod tests {
    use laqta_domain::{Bitmap, PixelFormat};
    use laqta_editor::{Command, Editor, EditorInput, EditorKey, Fonts, Mods};

    use super::*;

    #[test]
    fn every_tool_is_on_the_rail_once() {
        let on_rail: Vec<Tool> = groups(&[]).into_iter().flat_map(|g| g.tools).collect();
        assert_eq!(on_rail.len(), Tool::ALL.len());
        for t in Tool::ALL {
            assert_eq!(on_rail.iter().filter(|x| **x == t).count(), 1, "{t:?}");
        }
    }

    #[test]
    fn hiding_drops_tools_and_empty_groups_but_never_select() {
        let hidden = vec!["Crop".to_string(), "Cut out".to_string(), "Select".to_string()];
        let g = groups(&hidden);
        assert!(g.iter().all(|g| g.name != "Canvas"));
        assert_eq!(g[0].tools, vec![Tool::Select]);
        assert!(hideable().all(|t| t != Tool::Select));
    }

    #[test]
    fn shortcuts_are_unique_and_round_trip() {
        for t in Tool::ALL {
            if let Some(k) = shortcut(t) {
                assert_eq!(tool_for_key(k), Some(t));
            }
        }
        assert_eq!(tool_for_key('z'), None);
    }

    #[test]
    fn shortcuts_agree_with_the_engine_key_map() {
        let engine_pick = |c: char| {
            let bitmap = Bitmap::new(8, 8, PixelFormat::Rgba8, vec![0u8; 8 * 8 * 4].into()).unwrap_or_else(|e| panic!("bitmap: {e}"));
            let mut editor = Editor::new(bitmap, Fonts::empty());
            // No key maps to the Object eraser, so a tool change is the engine's answer.
            editor.execute(Command::SetTool(Tool::ObjectEraser));
            let out = editor.handle(EditorInput::Key { key: EditorKey::Char(c), mods: Mods::default() });
            (out.chrome.tool != Tool::ObjectEraser).then_some(out.chrome.tool)
        };
        for c in ('a'..='z').chain('0'..='9') {
            let engine = engine_pick(c);
            if c.is_ascii_digit() {
                // Digits are engine-only aliases; the rail shows letters.
                assert_eq!(engine, Tool::from_key(c), "key {c}");
            } else {
                assert_eq!(tool_for_key(c), engine, "key {c}");
            }
        }
        for t in Tool::ALL {
            assert_eq!(shortcut(t).and_then(engine_pick), shortcut(t).map(|_| t), "{t:?}");
        }
    }

    #[test]
    fn the_clashing_letters_pick_what_the_engine_picks() {
        assert_eq!(tool_for_key('b'), Some(Tool::Blur));
        assert_eq!(tool_for_key('p'), Some(Tool::Pixelate));
        assert_eq!(tool_for_key('s'), Some(Tool::SpeechBalloon));
        assert_eq!(shortcut(Tool::ObjectEraser), None);
    }

    #[test]
    fn blur_has_no_style_controls() {
        assert_eq!(style_controls(Tool::Blur), StyleControls::default());
        assert!(style_controls(Tool::Arrow).stroke && !style_controls(Tool::Arrow).fill);
    }
}
