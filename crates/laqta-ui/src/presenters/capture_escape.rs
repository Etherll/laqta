//! The capture overlay's Esc key as data: which step an Esc press takes, from the most local
//! (stop typing) to the last (cancel the capture), and the notice shown while a discard waits for
//! a second Esc.

/// What the overlay knows when Esc is pressed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EscapeScene {
    /// Text is being typed into an annotation.
    pub typing: bool,
    /// An annotation is selected.
    pub mark_selected: bool,
    /// A drawing tool is picked.
    pub tool_out: bool,
    /// Annotations on the capture.
    pub marks: usize,
    /// A discard is waiting for a second Esc.
    pub armed: bool,
}

/// What an Esc press does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeStep {
    /// Leave text entry.
    StopTyping,
    /// Clear the annotation selection.
    Deselect,
    /// Back to the region tool.
    PutToolAway,
    /// Ask for a second Esc before discarding the annotations.
    ArmDiscard,
    /// Cancel the capture.
    Cancel,
}

/// How long a discard stays armed before it lapses.
pub const DISARM_AFTER: std::time::Duration = std::time::Duration::from_secs(3);

/// The step for an Esc press. Each level goes before the next; the last is `cancel_button`.
pub fn escape(s: &EscapeScene) -> EscapeStep {
    if s.typing {
        EscapeStep::StopTyping
    } else if s.mark_selected {
        EscapeStep::Deselect
    } else if s.tool_out {
        EscapeStep::PutToolAway
    } else {
        cancel_button(s)
    }
}

/// The step for the Cancel button, and for Esc once nothing else is in the way. With annotations
/// on the capture the first press arms a discard; a second one, while armed, cancels.
pub fn cancel_button(s: &EscapeScene) -> EscapeStep {
    if s.marks > 0 && !s.armed { EscapeStep::ArmDiscard } else { EscapeStep::Cancel }
}

/// The notice shown while a discard is armed.
pub fn discard_notice(marks: usize) -> String {
    let noun = if marks == 1 { "annotation" } else { "annotations" };
    format!("Press Esc again to discard {marks} {noun}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_level_of_escape_goes_before_the_next() {
        let all = EscapeScene { typing: true, mark_selected: true, tool_out: true, marks: 2, armed: false };
        assert_eq!(escape(&all), EscapeStep::StopTyping);
        let s = EscapeScene { typing: false, ..all };
        assert_eq!(escape(&s), EscapeStep::Deselect);
        let s = EscapeScene { mark_selected: false, ..s };
        assert_eq!(escape(&s), EscapeStep::PutToolAway);
        let s = EscapeScene { tool_out: false, ..s };
        assert_eq!(escape(&s), EscapeStep::ArmDiscard);
    }

    #[test]
    fn with_nothing_in_the_way_escape_is_the_cancel_button() {
        assert_eq!(escape(&EscapeScene::default()), EscapeStep::Cancel);
        let s = EscapeScene { marks: 1, ..EscapeScene::default() };
        assert_eq!(escape(&s), cancel_button(&s));
    }

    #[test]
    fn cancel_arms_a_discard_once_when_there_are_annotations() {
        let s = EscapeScene { marks: 1, ..EscapeScene::default() };
        assert_eq!(cancel_button(&s), EscapeStep::ArmDiscard);
        assert_eq!(cancel_button(&EscapeScene { armed: true, ..s }), EscapeStep::Cancel);
        assert_eq!(cancel_button(&EscapeScene { marks: 0, armed: false, ..s }), EscapeStep::Cancel);
    }

    #[test]
    fn the_discard_notice_counts_annotations() {
        assert_eq!(discard_notice(1), "Press Esc again to discard 1 annotation");
        assert_eq!(discard_notice(3), "Press Esc again to discard 3 annotations");
    }

    #[test]
    fn a_discard_lapses_after_three_seconds() {
        assert_eq!(DISARM_AFTER, std::time::Duration::from_secs(3));
    }
}
