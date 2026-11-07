//! Platform-neutral key chords.

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(transparent)]
    pub struct Modifiers: u8 {
        const CTRL = 1;
        const ALT = 2;
        const SHIFT = 4;
        const SUPER = 8;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum Key {
    PrintScreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct KeyChord {
    pub modifiers: Modifiers,
    pub key: Key,
}
