//! The settings value the application receives from `main`.

use std::path::PathBuf;

use crate::action::Action;
use crate::hotkey::{Key, KeyChord, Modifiers};

#[derive(Clone, Debug)]
pub struct Settings {
    pub screenshots_dir: PathBuf,
    pub hotkeys: Vec<(KeyChord, Action)>,
}

impl Settings {
    /// screenshots_dir = `<pictures>/Laqta`; hotkeys = [PrintScreen → Action::PrintScreen].
    pub fn m0_default(pictures_dir: PathBuf) -> Settings {
        Settings {
            screenshots_dir: pictures_dir.join("Laqta"),
            hotkeys: vec![(KeyChord { modifiers: Modifiers::empty(), key: Key::PrintScreen }, Action::PrintScreen)],
        }
    }
}
