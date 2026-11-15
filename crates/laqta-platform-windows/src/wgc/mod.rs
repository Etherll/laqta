//! The native Windows recorder: Windows.Graphics.Capture frames into a Media Foundation sink
//! writer, offered as a second `Media` recording backend beside FFmpeg. It records one monitor at a time,
//! writes H.264 and, for system sound, AAC, and refuses everything else as `Unsupported`, so the app falls
//! back to FFmpeg. `pure` builds on every target, so its tests run on Linux.

pub(crate) mod pure;
pub(crate) mod still;

#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod encoder;
#[cfg(windows)]
mod session;

#[cfg(windows)]
use std::path::{Path, PathBuf};

#[cfg(windows)]
use laqta_domain::record::RecordSpec;
#[cfg(windows)]
use laqta_ports::recording::{
    AudioInput, Media, MediaInfo, OsScreen, PcmFormat, PcmSource, RecordingHandle, RecordingInput, TranscodeSpec, VideoSource,
};
#[cfg(windows)]
use laqta_ports::{BoxFuture, PortError};

#[cfg(windows)]
use capture::Monitor;
#[cfg(windows)]
use encoder::{AUDIO_CHANNELS, AUDIO_RATE};

/// The highest frame rate the native recorder accepts.
#[cfg(windows)]
const MAX_FPS: u32 = 240;

#[cfg(windows)]
const MONITOR_ONLY: &str = "the native recorder records monitors; use FFmpeg for this area";
#[cfg(windows)]
const H264_ONLY: &str = "the native recorder encodes H.264 only; use FFmpeg for this codec";
#[cfg(windows)]
const NO_CAMERA: &str = "the native recorder has no camera overlay; use FFmpeg for it";
#[cfg(windows)]
const NO_CUSTOM_ARGS: &str = "custom FFmpeg arguments need FFmpeg";
#[cfg(windows)]
const SYSTEM_SOUND_ONLY: &str = "the native recorder records system sound only; use FFmpeg for microphones";
#[cfg(windows)]
const SYSTEM_SOUND_FORMAT: &str = "the native recorder records 48 kHz stereo system sound only";

/// Maps a Windows error to a port failure that names the step that failed.
#[cfg(windows)]
fn failed(step: &'static str) -> impl FnOnce(windows::core::Error) -> PortError {
    move |error| PortError::Failed(format!("{step}: {error}"))
}

/// The native recorder, the `Media` adapter for monitors.
#[cfg(windows)]
#[derive(Debug)]
pub(crate) struct NativeRecorder;

#[cfg(windows)]
impl Media for NativeRecorder {
    fn start(
        &self,
        input: RecordingInput,
        spec: RecordSpec,
        output: PathBuf,
    ) -> BoxFuture<'_, Result<Box<dyn RecordingHandle>, PortError>> {
        Box::pin(blocking::unblock(move || {
            let job = plan(input, &spec, output)?;
            let recording = session::start(job)?;
            Ok(Box::new(recording) as Box<dyn RecordingHandle>)
        }))
    }

    fn transcode(&self, _input: &Path, _output: &Path, _spec: TranscodeSpec) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(std::future::ready(Err(PortError::Unsupported("file conversion is done by FFmpeg".into()))))
    }

    fn probe(&self, _input: &Path) -> BoxFuture<'_, Result<MediaInfo, PortError>> {
        Box::pin(std::future::ready(Err(PortError::Unsupported("probing is done by FFmpeg".into()))))
    }
}

#[cfg(windows)]
impl RecordingHandle for session::Recording {
    fn pause(&mut self) -> BoxFuture<'_, Result<(), PortError>> {
        self.pause_now();
        Box::pin(std::future::ready(Ok(())))
    }

    fn resume(&mut self) -> BoxFuture<'_, Result<(), PortError>> {
        self.resume_now();
        Box::pin(std::future::ready(Ok(())))
    }

    fn stop(mut self: Box<Self>) -> BoxFuture<'static, Result<PathBuf, PortError>> {
        Box::pin(blocking::unblock(move || self.finish()))
    }

    fn abort(mut self: Box<Self>) -> BoxFuture<'static, Result<(), PortError>> {
        Box::pin(blocking::unblock(move || self.discard()))
    }

    fn is_alive(&mut self) -> bool {
        self.alive()
    }
}

/// Checks what the native recorder can write and turns the port's inputs into a job. A refusal releases
/// the sources it was given, so no capture or audio thread keeps running for a recording that will not start.
#[cfg(windows)]
fn plan(input: RecordingInput, spec: &RecordSpec, output: PathBuf) -> Result<session::Job, PortError> {
    let RecordingInput { video, audio } = input;
    let monitor = match check(spec).and_then(|()| monitor_of(&video)) {
        Ok(monitor) => monitor,
        Err(error) => {
            release(video, audio);
            return Err(error);
        }
    };
    let audio = system_sound(audio)?;
    let bitrate = pure::bitrate_bps(spec.quality, monitor.area.width, monitor.area.height, spec.fps);
    Ok(session::Job { output, monitor, fps: spec.fps, bitrate, show_cursor: spec.show_cursor, audio })
}

/// Refuses the settings that only FFmpeg can apply.
#[cfg(windows)]
fn check(spec: &RecordSpec) -> Result<(), PortError> {
    if !pure::is_h264(spec.codec) {
        return Err(PortError::Unsupported(H264_ONLY.into()));
    }
    if !(1..=MAX_FPS).contains(&spec.fps) {
        return Err(PortError::Failed(format!("the frame rate must be between 1 and {MAX_FPS}")));
    }
    if spec.camera.as_ref().is_some_and(|camera| camera.enabled) {
        return Err(PortError::Unsupported(NO_CAMERA.into()));
    }
    if !spec.custom_args.trim().is_empty() {
        return Err(PortError::Unsupported(NO_CUSTOM_ARGS.into()));
    }
    Ok(())
}

/// The monitor and the area on it that a `Target` names. The area is rounded down to even sides.
#[cfg(windows)]
fn monitor_of(video: &VideoSource) -> Result<Monitor, PortError> {
    let VideoSource::Target(screen) = video else {
        return Err(PortError::Unsupported(MONITOR_ONLY.into()));
    };
    let OsScreen::Windows { adapter_index, output_index, offset_x, offset_y, width, height, .. } = screen.screen() else {
        return Err(PortError::Unsupported(MONITOR_ONLY.into()));
    };
    let (width, height) = (pure::even_floor(*width), pure::even_floor(*height));
    if width == 0 || height == 0 {
        return Err(PortError::Failed("the area is smaller than two pixels".into()));
    }
    Ok(Monitor {
        adapter_index: *adapter_index,
        output_index: *output_index,
        area: pure::Rect { x: *offset_x, y: *offset_y, width, height },
    })
}

/// The system sound source, if the recording has one. Microphones, other formats and a second source
/// refuse the whole recording, and the sources it had are stopped.
#[cfg(windows)]
fn system_sound(audio: Vec<AudioInput>) -> Result<Option<Box<dyn PcmSource>>, PortError> {
    let wanted = PcmFormat { sample_rate: AUDIO_RATE, channels: AUDIO_CHANNELS };
    let mut system = None;
    let mut refusal = None;
    for item in audio {
        match item {
            AudioInput::Pcm(source) if system.is_none() && source.format() == wanted => system = Some(source),
            AudioInput::Pcm(mut source) => {
                source.stop();
                refusal = refusal.or(Some(PortError::Unsupported(SYSTEM_SOUND_FORMAT.into())));
            }
            AudioInput::Device(_) => refusal = refusal.or(Some(PortError::Unsupported(SYSTEM_SOUND_ONLY.into()))),
        }
    }
    if let Some(error) = refusal {
        if let Some(mut source) = system {
            source.stop();
        }
        return Err(error);
    }
    Ok(system)
}

/// Stops the sources of a recording that will not run.
#[cfg(windows)]
fn release(video: VideoSource, audio: Vec<AudioInput>) {
    if let VideoSource::Frames(mut frames) = video {
        frames.stop();
    }
    for item in audio {
        if let AudioInput::Pcm(mut source) = item {
            source.stop();
        }
    }
}
