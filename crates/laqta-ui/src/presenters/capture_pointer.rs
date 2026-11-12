//! The capture overlay's pointer as data: which layer takes a press, and which cursor shows over
//! the selection. The overlay view reads the scene and does the work.

use laqta_domain::geometry::PhysRect;
use laqta_domain::region::{Button, Handle};
use laqta_editor::Tool;

use super::mark_pointer::{Grip, Pointer};

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
    /// The drawing tool out. `None` is the Pointer: a press on a mark picks and drags it, and a
    /// press on empty space moves the selection.
    pub tool: Option<Tool>,
    /// The pointer is over an annotation.
    pub over_mark: bool,
    /// The pointer is inside the picked annotation's box, where a press drags it.
    pub over_picked: bool,
    /// Text read in place is under the pointer and takes the press.
    pub over_text: bool,
    /// The capture has annotations.
    pub marks: bool,
    /// A handle of the picked annotation under the pointer, as the way it drags (`grip`).
    pub mark_grip: Option<Grip>,
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

/// Whether `at` is inside `frame`. The left and top edges count; the right and bottom do not.
fn inside(frame: PhysRect, at: (i32, i32)) -> bool {
    (frame.x..frame.right()).contains(&at.0) && (frame.y..frame.bottom()).contains(&at.1)
}

/// Whether the Pointer is out: no drawing tool. The engine's own select tool counts as one.
fn pointer_tool(s: &PointerScene) -> bool {
    matches!(s.tool, None | Some(Tool::Select))
}

/// The layer that takes a press at `at`. Buttons other than left go to the session, or are
/// dropped when there are annotations. A left press on text goes to the text; otherwise an
/// ongoing gesture keeps it in the session. With a selection the first match wins: a handle of
/// the picked annotation, then the selection's handles and edges, then (under the Pointer) an
/// annotation or the picked one's box, then the inside of the selection, which a drawing tool
/// draws on and the Pointer moves. Outside, only the Pointer starts a selection, and only when
/// there are no annotations.
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
    // A picked annotation's handle wins over the selection's grab area, with any tool.
    if s.mark_grip.is_some() {
        return Route::Editor;
    }
    if s.grab.is_some() {
        return Route::Session;
    }
    if inside(frame, at) {
        let on_mark = s.over_mark || s.over_picked;
        return if !pointer_tool(s) || on_mark { Route::Editor } else { Route::Session };
    }
    if pointer_tool(s) && !s.marks { Route::Session } else { Route::Ignore }
}

/// The cursor at `at`. A handle being dragged keeps its cursor, a gesture moves the selection or
/// crosshairs without one, and text comes next. With a selection, a picked annotation's handle
/// and then the selection's own handle under the pointer show their resize cursors. Inside the selection the cursor draws with a tool and moves without
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
    match s.mark_grip {
        Some(Grip::Axis(h)) => return Pointer::Resize(h),
        Some(Grip::Free) => return Pointer::Move,
        None => {}
    }
    if let Some(h) = s.grab {
        return Pointer::Resize(h);
    }
    if inside(frame, at) {
        return if pointer_tool(s) { Pointer::Move } else { Pointer::Draw };
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
    fn inside_the_selection_the_pointer_takes_marks_and_moves_the_rest() {
        let s = framed();
        assert_eq!(route(&s, INSIDE, Button::Left), Route::Session, "empty space moves the selection");
        assert_eq!(route(&PointerScene { over_mark: true, ..s }, INSIDE, Button::Left), Route::Editor, "a mark is picked");
        assert_eq!(route(&PointerScene { over_picked: true, ..s }, INSIDE, Button::Left), Route::Editor, "the picked mark's box drags it");
        assert_eq!(route(&PointerScene { tool: Some(Tool::Rectangle), ..s }, INSIDE, Button::Left), Route::Editor);
        assert_eq!(route(&PointerScene { tool: Some(Tool::Rectangle), over_mark: true, ..s }, INSIDE, Button::Left), Route::Editor);
        assert_eq!(pointer(&s, INSIDE), Pointer::Move);
        assert_eq!(pointer(&PointerScene { over_mark: true, ..s }, INSIDE), Pointer::Move);
    }

    #[test]
    fn the_selection_s_edges_beat_a_mark_under_them() {
        let s = PointerScene { grab: Some(Handle::E), over_mark: true, ..framed() };
        assert_eq!(route(&s, INSIDE, Button::Left), Route::Session);
        assert_eq!(pointer(&s, INSIDE), Pointer::Resize(Handle::E));
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

    #[test]
    fn a_picked_mark_s_handle_takes_the_press_and_shows_its_axis() {
        let frame = PhysRect { x: 0, y: 0, width: 100, height: 100 };
        let s =
            PointerScene { frame: Some(frame), grab: Some(Handle::NW), mark_grip: Some(Grip::Axis(Handle::NW)), ..PointerScene::default() };
        assert_eq!(route(&s, (1, 1), Button::Left), Route::Editor, "the mark's handle beats the frame's corner");
        let drawing = PointerScene { tool: Some(Tool::Arrow), ..s };
        assert_eq!(route(&drawing, (1, 1), Button::Left), Route::Editor, "with a drawing tool out too");
        assert_eq!(route(&PointerScene { mark_grip: None, ..s }, (1, 1), Button::Left), Route::Session, "else the corner resizes");
        assert_eq!(pointer(&s, (1, 1)), Pointer::Resize(Handle::NW));
        let free = PointerScene { mark_grip: Some(Grip::Free), ..s };
        assert_eq!(pointer(&free, (1, 1)), Pointer::Move);
    }
}
