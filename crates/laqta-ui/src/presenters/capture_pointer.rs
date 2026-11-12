//! The capture overlay's pointer as data: which layer takes a press, and which cursor shows over
//! the selection. The overlay view reads the scene and does the work.

use laqta_domain::geometry::PhysRect;
use laqta_domain::region::{Button, Handle};
use laqta_editor::Tool;

/// What the pointer is over, read from the region and editor state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointerScene {
    /// The held selection.
    pub frame: Option<PhysRect>,
    /// The handle or edge under the pointer.
    pub grab: Option<Handle>,
    /// The handle being dragged.
    pub resizing: Option<Handle>,
    /// A region gesture is in progress.
    pub gesture: bool,
    /// The drawing tool; `None` is the region tool.
    pub tool: Option<Tool>,
    /// The pointer is over an annotation.
    pub over_mark: bool,
    /// Text read in place is under the pointer and takes the press.
    pub over_text: bool,
    /// The capture has annotations.
    pub marks: bool,
}

/// The layer that takes a press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// The text read in place.
    Text,
    /// The region session: making, moving or resizing the selection.
    Session,
    /// The editor: drawing or editing annotations.
    Editor,
    /// Nothing: the press is dropped.
    Ignore,
}

/// The cursor to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    Crosshair,
    Move,
    Resize(Handle),
    Draw,
    NotAllowed,
    Text,
}

/// Whether `at` is inside `frame`. The left and top edges count; the right and bottom do not.
fn inside(frame: PhysRect, at: (i32, i32)) -> bool {
    (frame.x..frame.right()).contains(&at.0) && (frame.y..frame.bottom()).contains(&at.1)
}

/// The layer that takes a press at `at`. Buttons other than left go to the session, or are
/// dropped when there are annotations. A left press on text goes to the text; otherwise an
/// ongoing gesture or a handle keeps it in the session. Inside the selection the editor takes it
/// when a tool is picked or the pointer is over an annotation. Outside, only the region tool
/// starts a selection, and only when there are no annotations.
pub fn route(s: &PointerScene, at: (i32, i32), button: Button) -> Route {
    if button != Button::Left {
        return if s.marks { Route::Ignore } else { Route::Session };
    }
    if s.over_text {
        return Route::Text;
    }
    if s.gesture {
        return Route::Session;
    }
    let Some(frame) = s.frame else { return Route::Session };
    if s.grab.is_some() {
        return Route::Session;
    }
    if inside(frame, at) {
        return if s.over_mark || s.tool.is_some() { Route::Editor } else { Route::Session };
    }
    if s.tool.is_none() && !s.marks { Route::Session } else { Route::Ignore }
}

/// The cursor at `at`. A handle being dragged keeps its cursor, a gesture moves the selection or
/// crosshairs without one, and text comes next. With a selection, a handle under the pointer
/// shows its resize cursor. Inside the selection the cursor draws with a tool and moves without
/// one. Outside it is a crosshair where a press would start a selection, and not-allowed where
/// the press would be dropped.
pub fn pointer(s: &PointerScene, at: (i32, i32)) -> Pointer {
    if let Some(h) = s.resizing {
        return Pointer::Resize(h);
    }
    if s.gesture {
        return if s.frame.is_some() { Pointer::Move } else { Pointer::Crosshair };
    }
    if s.over_text {
        return Pointer::Text;
    }
    let Some(frame) = s.frame else { return Pointer::Crosshair };
    if let Some(h) = s.grab {
        return Pointer::Resize(h);
    }
    if inside(frame, at) {
        return if s.tool.is_some() { Pointer::Draw } else { Pointer::Move };
    }
    if route(s, at, Button::Left) == Route::Session { Pointer::Crosshair } else { Pointer::NotAllowed }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: PhysRect = PhysRect { x: 100, y: 100, width: 200, height: 100 };
    const INSIDE: (i32, i32) = (150, 150);
    const OUTSIDE: (i32, i32) = (10, 10);

    fn framed() -> PointerScene {
        PointerScene { frame: Some(FRAME), ..PointerScene::default() }
    }

    #[test]
    fn inside_the_selection_the_editor_takes_the_press_only_with_a_tool_or_a_mark() {
        let s = framed();
        assert_eq!(route(&s, INSIDE, Button::Left), Route::Session);
        assert_eq!(route(&PointerScene { tool: Some(Tool::Rectangle), ..s }, INSIDE, Button::Left), Route::Editor);
        assert_eq!(route(&PointerScene { over_mark: true, ..s }, INSIDE, Button::Left), Route::Editor);
    }

    #[test]
    fn the_left_and_top_edges_are_inside_and_the_right_and_bottom_are_not() {
        let s = PointerScene { tool: Some(Tool::Rectangle), ..framed() };
        assert_eq!(route(&s, (100, 100), Button::Left), Route::Editor);
        assert_eq!(route(&s, (299, 199), Button::Left), Route::Editor);
        assert_eq!(route(&s, (300, 150), Button::Left), Route::Ignore);
        assert_eq!(route(&s, (150, 200), Button::Left), Route::Ignore);
    }

    #[test]
    fn outside_the_selection_only_the_region_tool_without_marks_starts_one() {
        let s = framed();
        assert_eq!(route(&s, OUTSIDE, Button::Left), Route::Session);
        assert_eq!(route(&PointerScene { marks: true, ..s }, OUTSIDE, Button::Left), Route::Ignore);
        assert_eq!(route(&PointerScene { tool: Some(Tool::Arrow), ..s }, OUTSIDE, Button::Left), Route::Ignore);
    }

    #[test]
    fn other_buttons_go_to_the_session_unless_there_are_marks() {
        let s = framed();
        assert_eq!(route(&s, INSIDE, Button::Right), Route::Session);
        assert_eq!(route(&PointerScene { marks: true, ..s }, INSIDE, Button::Middle), Route::Ignore);
    }

    #[test]
    fn text_takes_the_press_first_and_a_gesture_or_handle_keeps_it_in_the_session() {
        let tool = Some(Tool::Rectangle);
        assert_eq!(route(&PointerScene { over_text: true, tool, ..framed() }, INSIDE, Button::Left), Route::Text);
        assert_eq!(route(&PointerScene { gesture: true, tool, ..framed() }, INSIDE, Button::Left), Route::Session);
        assert_eq!(route(&PointerScene { grab: Some(Handle::NE), tool, ..framed() }, INSIDE, Button::Left), Route::Session);
    }

    #[test]
    fn without_a_selection_the_session_takes_the_press() {
        let s = PointerScene { tool: Some(Tool::Rectangle), ..PointerScene::default() };
        assert_eq!(route(&s, INSIDE, Button::Left), Route::Session);
    }

    #[test]
    fn a_handle_being_dragged_or_under_the_pointer_shows_its_resize_cursor() {
        let tool = Some(Tool::Rectangle);
        let s = PointerScene { resizing: Some(Handle::W), grab: Some(Handle::E), tool, ..framed() };
        assert_eq!(pointer(&s, INSIDE), Pointer::Resize(Handle::W));
        let s = PointerScene { grab: Some(Handle::NW), tool, ..framed() };
        assert_eq!(pointer(&s, INSIDE), Pointer::Resize(Handle::NW));
    }

    #[test]
    fn a_gesture_shows_a_move_over_a_selection_and_a_crosshair_without_one() {
        let s = PointerScene { gesture: true, ..framed() };
        assert_eq!(pointer(&s, OUTSIDE), Pointer::Move);
        let s = PointerScene { gesture: true, ..PointerScene::default() };
        assert_eq!(pointer(&s, OUTSIDE), Pointer::Crosshair);
    }

    #[test]
    fn text_shows_the_text_cursor_and_no_selection_shows_a_crosshair() {
        let s = PointerScene { over_text: true, tool: Some(Tool::Text), ..framed() };
        assert_eq!(pointer(&s, INSIDE), Pointer::Text);
        assert_eq!(pointer(&PointerScene::default(), INSIDE), Pointer::Crosshair);
    }

    #[test]
    fn inside_the_selection_a_tool_draws_and_the_region_tool_moves() {
        assert_eq!(pointer(&framed(), INSIDE), Pointer::Move);
        let s = PointerScene { tool: Some(Tool::Ellipse), ..framed() };
        assert_eq!(pointer(&s, INSIDE), Pointer::Draw);
    }

    #[test]
    fn outside_the_selection_the_cursor_is_a_crosshair_only_where_a_press_would_start_one() {
        assert_eq!(pointer(&framed(), OUTSIDE), Pointer::Crosshair);
        let s = PointerScene { tool: Some(Tool::Arrow), ..framed() };
        assert_eq!(pointer(&s, OUTSIDE), Pointer::NotAllowed);
        let s = PointerScene { marks: true, ..framed() };
        assert_eq!(pointer(&s, OUTSIDE), Pointer::NotAllowed);
    }
}
