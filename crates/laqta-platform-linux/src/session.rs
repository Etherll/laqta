/// The display session Laqta runs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    Wayland,
    X11,
}

impl Session {
    /// `WAYLAND_DISPLAY` set → Wayland, else `DISPLAY` → X11, else none.
    pub fn detect() -> Option<Session> {
        let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
        if set("WAYLAND_DISPLAY") {
            Some(Session::Wayland)
        } else if set("DISPLAY") {
            Some(Session::X11)
        } else {
            None
        }
    }
}
