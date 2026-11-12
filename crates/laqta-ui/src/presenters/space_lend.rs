//! Holding Space lends the pointer tool while a drawing tool is out, in the capture overlay and
//! the image editor alike. `T` is the window's tool type and `pointer` its pointer tool (`None` in
//! the overlay, `Tool::Select` in the editor). Each method returns the tool to switch to, if any.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpaceLend<T> {
    pointer: T,
    /// The drawing tool Space put away.
    lent: Option<T>,
    /// Space came up mid-drag; the lent tool comes back at the release.
    return_on_release: bool,
}

impl<T: Copy + PartialEq> SpaceLend<T> {
    pub fn new(pointer: T) -> SpaceLend<T> {
        SpaceLend { pointer, lent: None, return_on_release: false }
    }

    /// Whether Space has the pointer out: its repeats are Space's too.
    pub fn is_lent(&self) -> bool {
        self.lent.is_some()
    }

    /// Space went down with `current` out: the pointer, unless it is out already or lent.
    pub fn space_down(&mut self, current: T) -> Option<T> {
        if self.lent.is_some() || current == self.pointer {
            return None;
        }
        self.lent = Some(current);
        Some(self.pointer)
    }

    /// Space came up with `current` out and a press under way or not. The lent tool comes back
    /// now, or at the release when a drag is under way; not when another tool was picked meanwhile.
    pub fn space_up(&mut self, pressed: bool, current: T) -> Option<T> {
        if pressed {
            self.return_on_release = self.lent.is_some();
            return None;
        }
        self.return_on_release = false;
        self.lent.take().filter(|_| current == self.pointer)
    }

    /// The press ended with `current` out: the lent tool, when Space came up during it.
    pub fn released(&mut self, current: T) -> Option<T> {
        if self.return_on_release { self.space_up(false, current) } else { None }
    }

    /// The user picked a tool: that one stays, whatever Space does next.
    pub fn picked(&mut self) {
        self.lent = None;
        self.return_on_release = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum T {
        Pointer,
        Pen,
        Box,
    }

    #[test]
    fn space_lends_the_pointer_and_gives_the_tool_back() {
        let mut s = SpaceLend::new(T::Pointer);
        assert_eq!(s.space_down(T::Pen), Some(T::Pointer));
        assert_eq!(s.space_down(T::Pointer), None, "a repeat changes nothing");
        assert!(s.is_lent());
        assert_eq!(s.space_up(false, T::Pointer), Some(T::Pen));
        assert!(!s.is_lent());
    }

    #[test]
    fn with_the_pointer_out_space_does_nothing() {
        let mut s = SpaceLend::new(T::Pointer);
        assert_eq!(s.space_down(T::Pointer), None);
        assert_eq!(s.space_up(false, T::Pointer), None);
    }

    #[test]
    fn space_up_mid_drag_waits_for_the_release() {
        let mut s = SpaceLend::new(T::Pointer);
        s.space_down(T::Pen);
        assert_eq!(s.space_up(true, T::Pointer), None);
        assert_eq!(s.released(T::Pointer), Some(T::Pen));
        assert_eq!(s.released(T::Pointer), None, "once");
    }

    #[test]
    fn a_tool_picked_while_lent_stays() {
        let mut s = SpaceLend::new(T::Pointer);
        s.space_down(T::Pen);
        s.picked();
        assert_eq!(s.space_up(false, T::Box), None);
    }
}
