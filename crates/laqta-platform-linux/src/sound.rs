//! Per-event sounds through the desktop's audio tools. Playback runs on a thread of its own, so the
//! caller never waits for a sound to finish.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use laqta_ports::{PortError, Sound};

/// Where the freedesktop sound theme keeps its stereo files.
const THEME_DIR: &str = "/usr/share/sounds/freedesktop/stereo";

/// A program and its arguments, tried in order.
type Player = (&'static str, Vec<OsString>);

/// Plays `sound` from `custom` when that file exists, else from the freedesktop theme. Returns once
/// the sound is queued on a `laqta-sound` thread. `Unsupported` when no player is on `PATH`.
pub fn play_sound(sound: Sound, custom: Option<&Path>) -> Result<(), PortError> {
    let file = sound_file(sound, custom.filter(|path| path.is_file()));
    let players: Vec<Player> = players(sound, &file).into_iter().filter(|(program, _)| on_path(program)).collect();
    if players.is_empty() {
        return Err(PortError::Unsupported("no sound player (pw-play, paplay or canberra-gtk-play) on PATH".into()));
    }
    thread::Builder::new()
        .name("laqta-sound".into())
        .spawn(move || {
            for (program, args) in players {
                let played = Command::new(program)
                    .args(args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success());
                if played {
                    return;
                }
            }
            tracing::debug!("sound: no player played the sound");
        })
        .map(drop)
        .map_err(|e| PortError::Failed(format!("sound thread: {e}")))
}

/// The file `sound` plays: `custom` when given, else the theme's file for the event.
fn sound_file(sound: Sound, custom: Option<&Path>) -> PathBuf {
    match custom {
        Some(path) => path.to_path_buf(),
        None => Path::new(THEME_DIR).join(format!("{}.oga", theme_id(sound))),
    }
}

/// The freedesktop sound-theme name for `sound`. `canberra-gtk-play -i` takes the same name.
fn theme_id(sound: Sound) -> &'static str {
    match sound {
        Sound::Capture => "camera-shutter",
        Sound::TaskCompleted | Sound::ActionCompleted => "complete",
        Sound::Error => "dialog-error",
    }
}

/// The players to try for `file`. When the file is missing, canberra plays the theme's event by name.
fn players(sound: Sound, file: &Path) -> Vec<Player> {
    if file.is_file() {
        let path = file.as_os_str().to_os_string();
        vec![("pw-play", vec![path.clone()]), ("paplay", vec![path.clone()]), ("canberra-gtk-play", vec![OsString::from("-f"), path])]
    } else {
        vec![("canberra-gtk-play", vec![OsString::from("-i"), OsString::from(theme_id(sound))])]
    }
}

/// Whether `program` is a file in one of the `PATH` directories.
fn on_path(program: &str) -> bool {
    env::var_os("PATH").is_some_and(|paths| env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use laqta_ports::Sound;

    use super::{players, sound_file};

    const THEME: &str = "/usr/share/sounds/freedesktop/stereo";

    #[test]
    fn each_event_plays_its_theme_file() {
        assert_eq!(sound_file(Sound::Capture, None), PathBuf::from(format!("{THEME}/camera-shutter.oga")));
        assert_eq!(sound_file(Sound::TaskCompleted, None), PathBuf::from(format!("{THEME}/complete.oga")));
        assert_eq!(sound_file(Sound::ActionCompleted, None), PathBuf::from(format!("{THEME}/complete.oga")));
        assert_eq!(sound_file(Sound::Error, None), PathBuf::from(format!("{THEME}/dialog-error.oga")));
    }

    #[test]
    fn custom_file_overrides_the_theme_file() {
        let custom = Path::new("/home/me/sounds/done.wav");
        assert_eq!(sound_file(Sound::TaskCompleted, Some(custom)), PathBuf::from("/home/me/sounds/done.wav"));
    }

    #[test]
    fn missing_file_falls_back_to_canberra_by_event_name() {
        let got = players(Sound::Error, Path::new("/nonexistent/laqta-test/dialog-error.oga"));
        assert_eq!(got, vec![("canberra-gtk-play", vec![OsString::from("-i"), OsString::from("dialog-error")])]);
    }
}
