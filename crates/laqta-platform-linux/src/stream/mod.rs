//! Recording sources (ADR 016). X11 hands `laqta-media` a target it captures itself. Wayland pulls
//! frames from a ScreenCast portal stream through PipeWire. Audio devices come from `pactl`.

mod audio;
mod pipewire_frames;
mod screencast;
mod x11;

use std::sync::{Arc, Mutex};

use laqta_domain::PhysRect;
use laqta_domain::record::AudioDevice;
use laqta_ports::{BoxFuture, PortError, RecordingInput, ScreenStream, VideoSource};

use crate::session::Session;

/// The `ScreenStream` of the detected session. It keeps the portal's restore token (ADR 019): the
/// token it starts with is the one `main` saved, and each new one goes to `on_token` for saving.
pub struct LinuxScreenStream {
    session: Session,
    token: Mutex<Option<String>>,
    on_token: Arc<dyn Fn(String) + Send + Sync>,
}

impl LinuxScreenStream {
    pub fn new(session: Session, restore_token: Option<String>, on_token: Arc<dyn Fn(String) + Send + Sync>) -> Self {
        Self { session, token: Mutex::new(restore_token), on_token }
    }

    fn restore_token(&self) -> Option<String> {
        self.token.lock().ok().and_then(|token| token.clone())
    }

    fn remember(&self, token: String) {
        if let Ok(mut current) = self.token.lock() {
            *current = Some(token.clone());
        }
        (self.on_token)(token);
    }

    async fn open_wayland(&self, area: PhysRect, show_cursor: bool, fps: u32, audio: &[AudioDevice]) -> Result<RecordingInput, PortError> {
        let area = even_area(area)?;
        let token = self.restore_token();
        let mut cast = screencast::start(token.as_deref(), show_cursor).await?;
        if let Some(next) = cast.restore_token.take() {
            self.remember(next);
        }
        // Connecting to PipeWire blocks (and so does dropping the frames), so it runs off the async worker.
        let frames = blocking::unblock(move || pipewire_frames::PipeWireFrames::start(cast, area, fps)).await?;
        Ok(RecordingInput { video: VideoSource::Frames(Box::new(frames)), audio: audio::inputs(audio) })
    }
}

impl ScreenStream for LinuxScreenStream {
    fn open(&self, area: PhysRect, show_cursor: bool, fps: u32, audio: &[AudioDevice]) -> BoxFuture<'_, Result<RecordingInput, PortError>> {
        let audio = audio.to_vec();
        Box::pin(async move {
            match self.session {
                Session::X11 => Ok(RecordingInput { video: x11::open(area)?, audio: audio::inputs(&audio) }),
                Session::Wayland => self.open_wayland(area, show_cursor, fps, &audio).await,
            }
        })
    }

    fn audio_devices(&self) -> BoxFuture<'_, Result<Vec<AudioDevice>, PortError>> {
        Box::pin(blocking::unblock(audio::list_devices))
    }
}

/// The area with width and height rounded down to even numbers, because 4:2:0 encoders need them.
fn even_area(area: PhysRect) -> Result<PhysRect, PortError> {
    let rounded = PhysRect { width: area.width & !1, height: area.height & !1, ..area };
    if rounded.width == 0 || rounded.height == 0 {
        return Err(PortError::Failed(format!("area {}x{} is too small to record", area.width, area.height)));
    }
    Ok(rounded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_area_rounds_down() {
        let area = PhysRect { x: -3, y: 5, width: 1921, height: 1081 };
        assert_eq!(even_area(area).ok(), Some(PhysRect { x: -3, y: 5, width: 1920, height: 1080 }));
    }

    #[test]
    fn even_area_rejects_an_empty_side() {
        assert!(even_area(PhysRect { x: 0, y: 0, width: 1, height: 100 }).is_err());
        assert!(even_area(PhysRect { x: 0, y: 0, width: 100, height: 0 }).is_err());
    }
}
