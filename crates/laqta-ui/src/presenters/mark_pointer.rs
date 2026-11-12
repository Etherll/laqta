//! The pointer over annotations, shared by the capture overlay and the image editor: how far a
//! picked annotation's handle reaches, which way it drags, and the cursor over the editor's canvas.
//! The capture overlay adds its selection on top (`capture_pointer`).

use laqta_domain::region::Handle;
use laqta_editor::{Handle as MarkHandle, PointF, Tool};

/// How far from a picked annotation's handle, along each axis, a press still takes it, in screen
/// pixels: a 20 px square around the 10 px dot. The cursor and the engine (`set_handle_reach`,
/// divided by the zoom) use the same reach, so a resize cursor always means a resize.
pub const GRAB: f32 = 10.0;

/// The picked annotation's handle within `reach` of (`x`, `y`), as the way it drags. `handles` is
/// `Chrome::handles`, in the same pixels as the point and the reach.
pub fn handle_under(handles: &[(MarkHandle, PointF)], (x, y): (f32, f32), reach: f32) -> Option<Grip> {
    let (handle, _) = handles.iter().find(|(_, p)| (p.x - x).abs() <= reach && (p.y - y).abs() <= reach)?;
    Some(grip(*handle, handles))
}

/// The way a picked annotation's handle drags, for its cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    /// Along one axis, shown as the selection handle that drags the same way.
    Axis(Handle),
    /// Any way at once (a curve point).
    Free,
}

/// The way the picked annotation's `handle` drags. `handles` is the annotation's handle list
/// (`Chrome::handles`): a line's or arrow's end drags along the line, so it reads the other end.
pub fn grip(handle: MarkHandle, handles: &[(MarkHandle, PointF)]) -> Grip {
    let point = |h: MarkHandle| handles.iter().find(|(k, _)| *k == h).map(|(_, p)| *p);
    match handle {
        MarkHandle::N | MarkHandle::S => Grip::Axis(Handle::N),
        MarkHandle::E | MarkHandle::W => Grip::Axis(Handle::E),
        MarkHandle::NW | MarkHandle::SE => Grip::Axis(Handle::NW),
        MarkHandle::NE | MarkHandle::SW => Grip::Axis(Handle::NE),
        MarkHandle::Start | MarkHandle::End => {
            let other = if handle == MarkHandle::Start { MarkHandle::End } else { MarkHandle::Start };
            match (point(handle), point(other)) {
                (Some(a), Some(b)) => line_grip(b.x - a.x, b.y - a.y),
                _ => Grip::Free,
            }
        }
        MarkHandle::Control => Grip::Free,
    }
}

/// The axis nearest a line running (`dx`, `dy`), y pointing down.
fn line_grip(dx: f32, dy: f32) -> Grip {
    if dx == 0.0 && dy == 0.0 {
        return Grip::Free;
    }
    let angle = dy.atan2(dx).to_degrees().rem_euclid(180.0);
    Grip::Axis(match angle {
        a if !(22.5..157.5).contains(&a) => Handle::E,
        a if a < 67.5 => Handle::NW,
        a if a < 112.5 => Handle::N,
        _ => Handle::NE,
    })
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

/// The cursor over the image editor's canvas, which has no selection to move: a handle of the
/// picked annotation (`grip`) shows its resize arrow, and the Select tool shows a hand over an
/// annotation (`over_mark`) or inside the picked one's box (`in_picked`). `None` is the plain
/// arrow, under Select over nothing.
pub fn canvas_pointer(tool: Tool, grip: Option<Grip>, over_mark: bool, in_picked: bool) -> Option<Pointer> {
    Some(match (grip, tool) {
        (Some(Grip::Axis(h)), _) => Pointer::Resize(h),
        (Some(Grip::Free), _) => Pointer::Move,
        (None, Tool::Select) if over_mark || in_picked => Pointer::Move,
        (None, Tool::Select) => return None,
        (None, Tool::Text) => Pointer::Text,
        (None, _) => Pointer::Draw,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arrow_end_drags_along_the_arrow() {
        let p = |x, y| PointF::new(x, y);
        let arrow = |end: PointF| [(MarkHandle::Start, p(0.0, 0.0)), (MarkHandle::End, end), (MarkHandle::Control, p(1.0, 1.0))];
        assert_eq!(grip(MarkHandle::Start, &arrow(p(100.0, 3.0))), Grip::Axis(Handle::E));
        assert_eq!(grip(MarkHandle::End, &arrow(p(2.0, 100.0))), Grip::Axis(Handle::N));
        assert_eq!(grip(MarkHandle::Start, &arrow(p(100.0, 100.0))), Grip::Axis(Handle::NW), "down and right");
        assert_eq!(grip(MarkHandle::End, &arrow(p(-100.0, 100.0))), Grip::Axis(Handle::NE), "down and left");
        assert_eq!(grip(MarkHandle::Control, &arrow(p(100.0, 100.0))), Grip::Free);
        assert_eq!(grip(MarkHandle::SW, &[]), Grip::Axis(Handle::NE));
    }

    #[test]
    fn the_editor_canvas_shows_handles_then_marks_under_select_and_draws_otherwise() {
        let axis = Some(Grip::Axis(Handle::E));
        assert_eq!(canvas_pointer(Tool::Arrow, axis, false, false), Some(Pointer::Resize(Handle::E)));
        assert_eq!(canvas_pointer(Tool::Select, None, true, false), Some(Pointer::Move));
        assert_eq!(canvas_pointer(Tool::Select, None, false, true), Some(Pointer::Move));
        assert_eq!(canvas_pointer(Tool::Select, None, false, false), None);
        assert_eq!(canvas_pointer(Tool::Rectangle, None, true, false), Some(Pointer::Draw));
        assert_eq!(canvas_pointer(Tool::Text, None, false, false), Some(Pointer::Text));
    }

    #[test]
    fn a_handle_takes_the_pointer_anywhere_in_its_square() {
        let handles = [(MarkHandle::NW, PointF::new(10.0, 10.0))];
        assert_eq!(handle_under(&handles, (1.0, 1.0), GRAB), Some(Grip::Axis(Handle::NW)), "the corner of the square");
        assert_eq!(handle_under(&handles, (-1.0, 10.0), GRAB), None, "past it");
    }
}
