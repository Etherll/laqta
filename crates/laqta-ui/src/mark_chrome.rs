//! What the capture overlay and the image editor draw the same way over annotations: the picked
//! mark's handle dot and the cursor for a `mark_pointer`/`capture_pointer` answer.

use gpui::{Div, Styled as _, div, px};

use laqta_domain::region::Handle;

use crate::presenters::mark_pointer::Pointer;

/// A handle of the picked mark, centred on (`x`, `y`) in the parent's pixels. The editor window and
/// the capture overlay draw the same one.
pub(crate) fn mark_handle(x: f32, y: f32, accent: gpui::Hsla) -> Div {
    div()
        .absolute()
        .left(px(x - 5.0))
        .top(px(y - 5.0))
        .w(px(10.))
        .h(px(10.))
        .rounded(px(2.))
        .bg(gpui::white())
        .border_1()
        .border_color(accent)
}

/// The pointer shape for a pointer answer; a move under way shows a closed hand.
pub(crate) fn pointer_style(pointer: Pointer, gesture: bool) -> gpui::CursorStyle {
    use gpui::CursorStyle;
    match pointer {
        Pointer::Crosshair | Pointer::Draw => CursorStyle::Crosshair,
        Pointer::Move if gesture => CursorStyle::ClosedHand,
        Pointer::Move => CursorStyle::OpenHand,
        Pointer::Text => CursorStyle::IBeam,
        Pointer::NotAllowed => CursorStyle::OperationNotAllowed,
        Pointer::Resize(Handle::N | Handle::S) => CursorStyle::ResizeUpDown,
        Pointer::Resize(Handle::E | Handle::W) => CursorStyle::ResizeLeftRight,
        Pointer::Resize(Handle::NW | Handle::SE) => CursorStyle::ResizeUpLeftDownRight,
        Pointer::Resize(Handle::NE | Handle::SW) => CursorStyle::ResizeUpRightDownLeft,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_handle_shows_the_arrow_of_the_way_it_drags() {
        let style = |h| pointer_style(Pointer::Resize(h), false);
        assert_eq!(style(Handle::W), gpui::CursorStyle::ResizeLeftRight);
        assert_eq!(style(Handle::E), gpui::CursorStyle::ResizeLeftRight);
        assert_eq!(style(Handle::N), gpui::CursorStyle::ResizeUpDown);
        assert_eq!(style(Handle::S), gpui::CursorStyle::ResizeUpDown);
        assert_eq!(style(Handle::NW), gpui::CursorStyle::ResizeUpLeftDownRight);
        assert_eq!(style(Handle::SE), gpui::CursorStyle::ResizeUpLeftDownRight);
        assert_eq!(style(Handle::NE), gpui::CursorStyle::ResizeUpRightDownLeft);
        assert_eq!(style(Handle::SW), gpui::CursorStyle::ResizeUpRightDownLeft);
    }

    #[test]
    fn a_move_shows_a_hand_that_closes_while_it_drags() {
        assert_eq!(pointer_style(Pointer::Move, false), gpui::CursorStyle::OpenHand);
        assert_eq!(pointer_style(Pointer::Move, true), gpui::CursorStyle::ClosedHand);
        assert_eq!(pointer_style(Pointer::NotAllowed, false), gpui::CursorStyle::OperationNotAllowed);
        assert_eq!(pointer_style(Pointer::Crosshair, false), gpui::CursorStyle::Crosshair);
    }
}
