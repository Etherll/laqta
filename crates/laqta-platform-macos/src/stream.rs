//! Recording input (`ScreenStream`): frames and system audio from one `SCStream`.

use laqta_domain::geometry::PhysRect;
use laqta_domain::record::AudioDevice;
use laqta_ports::{BoxFuture, PortError, RecordingInput, ScreenStream};

#[derive(Debug, Default)]
pub struct MacStream;

impl ScreenStream for MacStream {
    fn open(&self, _area: PhysRect, _show_cursor: bool, _fps: u32, _audio: &[AudioDevice]) -> BoxFuture<'_, Result<RecordingInput, PortError>> {
        Box::pin(async { Err(PortError::Unsupported("screen recording on macOS".into())) })
    }

    fn audio_devices(&self) -> BoxFuture<'_, Result<Vec<AudioDevice>, PortError>> {
        Box::pin(async { Err(PortError::Unsupported("audio devices on macOS".into())) })
    }
}
