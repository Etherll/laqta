//! Per-event sounds (docs/FEATURES.md, the per-event sounds row). A user file replaces the built-in
//! sound for an event. Built-in sounds are the shutter WAV for captures and the system aliases for
//! the rest.

use std::path::Path;
use std::sync::OnceLock;

use laqta_ports::{PortError, Sound};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_FILENAME, SND_FLAGS, SND_MEMORY, SND_NODEFAULT};
use windows::core::{HSTRING, PCWSTR, w};

use crate::shutter::shutter_wav;

/// The shutter WAV. `PlaySound` with `SND_MEMORY | SND_ASYNC` reads it after the call returns, so it
/// lives for the whole process.
static SHUTTER: OnceLock<Vec<u8>> = OnceLock::new();

/// Plays `sound`, or the file `custom` in its place. Returns once playback has started.
pub fn play_sound(sound: Sound, custom: Option<&Path>) -> Result<(), PortError> {
    if let Some(path) = custom {
        let file = HSTRING::from(path.as_os_str());
        return play(PCWSTR(file.as_ptr()), SND_FILENAME | SND_ASYNC | SND_NODEFAULT);
    }
    match sound {
        Sound::Capture => {
            let wav = SHUTTER.get_or_init(shutter_wav);
            play(PCWSTR(wav.as_ptr().cast()), SND_MEMORY | SND_ASYNC | SND_NODEFAULT)
        }
        Sound::TaskCompleted | Sound::ActionCompleted => play(w!("SystemAsterisk"), SND_ALIAS | SND_ASYNC | SND_NODEFAULT),
        Sound::Error => play(w!("SystemHand"), SND_ALIAS | SND_ASYNC | SND_NODEFAULT),
    }
}

/// `name` is a file name, a system alias, or (with `SND_MEMORY`) a WAV image. The caller keeps it
/// alive for the call, and for async playback where the flags require it.
fn play(name: PCWSTR, flags: SND_FLAGS) -> Result<(), PortError> {
    // SAFETY: `name` is valid for the flags it is paired with, as described above. No window is passed.
    let played = unsafe { PlaySoundW(name, None, flags) };
    if played.as_bool() { Ok(()) } else { Err(PortError::Failed("could not play sound".into())) }
}
