//! Per-event sounds through `NSSound`. A user file replaces the built-in sound for an event; the
//! built-in ones are macOS system sounds.

use std::cell::RefCell;
use std::path::Path;

use laqta_ports::{PortError, Sound};
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_app_kit::NSSound;
use objc2_foundation::NSString;

use crate::main_thread::on_main;

thread_local! {
    /// Sounds that may still be playing. An `NSSound` stops when its last reference goes, so they wait
    /// here until `isPlaying` turns false. Only the main thread touches this.
    static PLAYING: RefCell<Vec<Retained<NSSound>>> = const { RefCell::new(Vec::new()) };
}

/// Plays `sound`, or the file `custom` in its place. Returns once playback has started.
pub fn play_sound(sound: Sound, custom: Option<&Path>) -> Result<(), PortError> {
    on_main(|_| {
        let nssound = match custom {
            Some(path) => from_file(path)?,
            None => from_system(system_sound(sound))?,
        };
        // The system sounds are shared instances; `play` does nothing on one that is already playing.
        nssound.stop();
        if !nssound.play() {
            return Err(PortError::Failed("could not play sound".into()));
        }
        PLAYING.with_borrow_mut(|playing| {
            playing.retain(|s| s.isPlaying());
            playing.push(nssound);
        });
        Ok(())
    })
}

/// The system sound for each event, by the name `NSSound::soundNamed` looks up.
fn system_sound(sound: Sound) -> &'static str {
    match sound {
        Sound::Capture => "Tink",
        Sound::TaskCompleted => "Glass",
        Sound::ActionCompleted => "Pop",
        Sound::Error => "Basso",
    }
}

fn from_system(name: &str) -> Result<Retained<NSSound>, PortError> {
    NSSound::soundNamed(&NSString::from_str(name)).ok_or_else(|| PortError::Failed(format!("system sound {name} not found")))
}

/// Loads the file into memory (`byReference: false`), so deleting it during playback is harmless.
fn from_file(path: &Path) -> Result<Retained<NSSound>, PortError> {
    let name = path.to_str().ok_or_else(|| PortError::Failed(format!("sound path is not UTF-8: {}", path.display())))?;
    NSSound::initWithContentsOfFile_byReference(NSSound::alloc(), &NSString::from_str(name), false)
        .ok_or_else(|| PortError::Failed(format!("could not load sound {}", path.display())))
}
