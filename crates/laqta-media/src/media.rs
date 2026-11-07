//! The `Media` port over FFmpeg: recording, file conversion and probing (ADR 005).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use laqta_domain::record::RecordSpec;
use laqta_ports::recording::{AudioInput, Media, MediaInfo, RecordingHandle, RecordingInput, TranscodeSpec, VideoSource};
use laqta_ports::{BoxFuture, PortError};
use serde::Deserialize;

use crate::argv::{self, AudioInputDesc, Host, VideoInputDesc};
use crate::locate::FfmpegPaths;
use crate::process::{Feed, FfmpegRecorder, Plan, ensure_success, run_tool};
use crate::run_blocking;

/// FFmpeg as the `Media` adapter. `work_dir` holds each recording's segments until it stops.
#[derive(Clone, Debug)]
pub struct FfmpegMedia {
    paths: FfmpegPaths,
    work_dir: PathBuf,
}

static NEXT_SESSION: AtomicU64 = AtomicU64::new(0);

impl FfmpegMedia {
    pub fn new(paths: FfmpegPaths, work_dir: PathBuf) -> Self {
        Self { paths, work_dir }
    }

    pub fn paths(&self) -> &FfmpegPaths {
        &self.paths
    }

    /// Starts a recording and returns the concrete recorder, which exposes its process id.
    #[doc(hidden)]
    pub fn start_recorder(
        &self,
        input: RecordingInput,
        spec: RecordSpec,
        output: PathBuf,
    ) -> BoxFuture<'static, Result<FfmpegRecorder, PortError>> {
        let host = Host::current();
        let prepared = prepare(input, &spec, host);
        let session = self.work_dir.join(format!("rec-{}-{}", std::process::id(), NEXT_SESSION.fetch_add(1, Ordering::Relaxed)));
        let paths = self.paths.clone();
        let plan = Plan { video: prepared.video, audio: prepared.audio, spec, host };
        let feed = prepared.feed;
        run_blocking(move || FfmpegRecorder::start(paths, plan, session, output, feed))
    }
}

impl Media for FfmpegMedia {
    fn start(
        &self,
        input: RecordingInput,
        spec: RecordSpec,
        output: PathBuf,
    ) -> BoxFuture<'_, Result<Box<dyn RecordingHandle>, PortError>> {
        let recording = self.start_recorder(input, spec, output);
        Box::pin(async move { recording.await.map(|recorder| Box::new(recorder) as Box<dyn RecordingHandle>) })
    }

    fn transcode(&self, input: &Path, output: &Path, spec: TranscodeSpec) -> BoxFuture<'_, Result<(), PortError>> {
        let paths = self.paths.clone();
        let input = input.to_owned();
        let output = output.to_owned();
        run_blocking(move || {
            let args = argv::transcode_args(&spec, &input, &output);
            let result = run_tool(&paths.ffmpeg, &args, None)?;
            ensure_success(&result, "transcode")?;
            if output.is_file() { Ok(()) } else { Err(PortError::Failed(format!("transcode wrote no file at {}", output.display()))) }
        })
    }

    fn probe(&self, input: &Path) -> BoxFuture<'_, Result<MediaInfo, PortError>> {
        let paths = self.paths.clone();
        let input = input.to_owned();
        run_blocking(move || {
            let result = run_tool(&paths.ffprobe, &argv::probe_args(&input), None)?;
            ensure_success(&result, "probe")?;
            let size = std::fs::metadata(&input).map(|meta| meta.len()).unwrap_or(0);
            parse_probe(&result.stdout, size)
        })
    }
}

/// The recording inputs as FFmpeg sees them, and the one stdin source they share, if any.
struct Prepared {
    video: VideoInputDesc,
    audio: Vec<AudioInputDesc>,
    feed: Option<Feed>,
}

/// Maps the port's inputs to FFmpeg's. Only one input can use stdin. Raw frames come first, so PCM
/// is dropped when both are present (M4 D6). GIF, WebP and APNG take no sound. A loopback device
/// cannot be recorded through dshow, so on Windows it is dropped (system sound is `Pcm`).
fn prepare(input: RecordingInput, spec: &RecordSpec, host: Host) -> Prepared {
    let animation = spec.codec.is_animation();
    let mut feed = None;
    let video = match input.video {
        VideoSource::Target(screen) => VideoInputDesc::Target(screen.screen().clone()),
        VideoSource::Frames(source) => {
            let (width, height) = source.size();
            feed = Some(Feed::Frames(source));
            VideoInputDesc::RawFrames { width, height, fps: spec.fps }
        }
    };
    let mut audio = Vec::new();
    for item in input.audio {
        match item {
            AudioInput::Device(device) => {
                if animation {
                    continue;
                }
                if host == Host::Windows && device.loopback {
                    tracing::warn!(device = %device.name, "dshow cannot record a loopback device; system sound needs the PCM input");
                    continue;
                }
                audio.push(AudioInputDesc::Device { id: device.id, name: device.name, loopback: device.loopback });
            }
            AudioInput::Pcm(mut source) => {
                let format = source.format();
                if animation {
                    source.stop();
                    continue;
                }
                if feed.is_some() {
                    tracing::warn!("PCM audio is ignored because raw frames already use stdin");
                    source.stop();
                    continue;
                }
                feed = Some(Feed::Pcm(source));
                audio.push(AudioInputDesc::Pcm { rate: format.sample_rate, channels: format.channels, pipe: "-".into() });
            }
        }
    }
    Prepared { video, audio, feed }
}

#[derive(Deserialize)]
struct ProbeOutput {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
    size: Option<String>,
}

/// Reads `ffprobe -print_format json` output. `file_size` is the fallback when the container does
/// not report one.
pub(crate) fn parse_probe(json: &[u8], file_size: u64) -> Result<MediaInfo, PortError> {
    let probe: ProbeOutput =
        serde_json::from_slice(json).map_err(|err| PortError::Failed(format!("could not read the probe output: {err}")))?;
    let video = probe.streams.iter().find(|stream| stream.codec_type.as_deref() == Some("video"));
    let audio = probe.streams.iter().find(|stream| stream.codec_type.as_deref() == Some("audio"));
    let format = probe.format.as_ref();
    let duration_secs = format.and_then(|f| f.duration.as_deref()).and_then(|text| text.parse::<f64>().ok()).unwrap_or(0.0);
    let size_bytes = format.and_then(|f| f.size.as_deref()).and_then(|text| text.parse::<u64>().ok()).unwrap_or(file_size);
    let fps = video
        .map(|stream| [stream.avg_frame_rate.as_deref(), stream.r_frame_rate.as_deref()])
        .into_iter()
        .flatten()
        .flatten()
        .map(parse_rate)
        .find(|rate| *rate > 0.0)
        .unwrap_or(0.0);
    Ok(MediaInfo {
        duration_secs,
        width: video.and_then(|stream| stream.width).unwrap_or(0),
        height: video.and_then(|stream| stream.height).unwrap_or(0),
        fps,
        video_codec: video.and_then(|stream| stream.codec_name.clone()).unwrap_or_default(),
        audio_codec: audio.and_then(|stream| stream.codec_name.clone()),
        size_bytes,
    })
}

/// A frame rate such as `30000/1001`, or a plain number. Zero denominators give 0.
fn parse_rate(rate: &str) -> f64 {
    match rate.split_once('/') {
        Some((num, den)) => {
            let num = num.parse::<f64>().unwrap_or(0.0);
            let den = den.parse::<f64>().unwrap_or(0.0);
            if den > 0.0 { num / den } else { 0.0 }
        }
        None => rate.parse().unwrap_or(0.0),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use laqta_domain::PixelFormat;
    use laqta_domain::geometry::PhysRect;
    use laqta_domain::record::AudioDevice;
    use laqta_ports::PortError;
    use laqta_ports::fakes::FakeFrames;
    use laqta_ports::recording::{FrameSource, OsScreenRef, PcmFormat, PcmSource};

    use super::*;

    /// A PCM source that records whether it was stopped.
    struct Pcm {
        stopped: Arc<AtomicBool>,
    }

    impl PcmSource for Pcm {
        fn format(&self) -> PcmFormat {
            PcmFormat { sample_rate: 48_000, channels: 2 }
        }
        fn read(&mut self, _buf: &mut [i16]) -> Result<usize, PortError> {
            Ok(0)
        }
        fn stop(&mut self) {
            self.stopped.store(true, Ordering::SeqCst);
        }
    }

    fn spec(codec: laqta_domain::record::VideoCodec) -> RecordSpec {
        RecordSpec {
            area: PhysRect { x: 0, y: 0, width: 64, height: 48 },
            codec,
            fps: 30,
            quality: laqta_domain::record::Quality::Default,
            show_cursor: true,
            audio: Vec::new(),
            custom_args: String::new(),
        }
    }

    fn frames() -> VideoSource {
        VideoSource::Frames(Box::new(FakeFrames { size: (64, 48), left: 1 }))
    }

    fn device(name: &str, loopback: bool) -> AudioInput {
        AudioInput::Device(AudioDevice { id: format!("{name}-id"), name: name.into(), loopback })
    }

    #[test]
    fn probe_reads_streams_and_format() {
        let json = br#"{
            "streams": [
                {"codec_type": "video", "codec_name": "h264", "width": 64, "height": 48,
                 "avg_frame_rate": "30/1", "r_frame_rate": "30/1"},
                {"codec_type": "audio", "codec_name": "aac"}
            ],
            "format": {"duration": "1.000000", "size": "2048"}
        }"#;
        let info = parse_probe(json, 0).expect("parse");
        assert_eq!(info.duration_secs, 1.0);
        assert_eq!((info.width, info.height), (64, 48));
        assert_eq!(info.fps, 30.0);
        assert_eq!(info.video_codec, "h264");
        assert_eq!(info.audio_codec.as_deref(), Some("aac"));
        assert_eq!(info.size_bytes, 2048);
    }

    #[test]
    fn probe_falls_back_to_the_real_rate_and_file_size() {
        let json = br#"{"streams": [{"codec_type": "video", "codec_name": "vp9", "width": 2, "height": 2,
            "avg_frame_rate": "0/0", "r_frame_rate": "30000/1001"}], "format": {}}"#;
        let info = parse_probe(json, 777).expect("parse");
        assert!((info.fps - 29.97).abs() < 0.01);
        assert_eq!(info.size_bytes, 777);
        assert_eq!(info.audio_codec, None);
        assert_eq!(info.duration_secs, 0.0);
    }

    #[test]
    fn probe_rejects_non_json() {
        assert!(parse_probe(b"not json", 0).is_err());
    }

    #[test]
    fn frames_feed_stdin_and_pcm_is_dropped_with_them() {
        let stopped = Arc::new(AtomicBool::new(false));
        let input = RecordingInput {
            video: frames(),
            audio: vec![AudioInput::Pcm(Box::new(Pcm { stopped: Arc::clone(&stopped) })), device("Mic", false)],
        };
        let prepared = prepare(input, &spec(laqta_domain::record::VideoCodec::X264), Host::Linux);
        assert!(matches!(prepared.feed, Some(Feed::Frames(_))));
        assert!(stopped.load(Ordering::SeqCst), "the dropped PCM source is stopped");
        assert!(matches!(prepared.video, VideoInputDesc::RawFrames { width: 64, height: 48, fps: 30 }));
        assert_eq!(prepared.audio.len(), 1);
        assert!(matches!(&prepared.audio[0], AudioInputDesc::Device { name, .. } if name == "Mic"));
    }

    #[test]
    fn target_with_pcm_puts_pcm_on_stdin() {
        let stopped = Arc::new(AtomicBool::new(false));
        let screen = OsScreenRef::new(laqta_ports::recording::OsScreen::Windows {
            adapter_index: 0,
            output_index: 0,
            offset_x: 0,
            offset_y: 0,
            width: 64,
            height: 48,
            desktop_x: 0,
            desktop_y: 0,
        });
        let input = RecordingInput {
            video: VideoSource::Target(screen),
            audio: vec![AudioInput::Pcm(Box::new(Pcm { stopped: Arc::clone(&stopped) }))],
        };
        let prepared = prepare(input, &spec(laqta_domain::record::VideoCodec::X264), Host::Windows);
        assert!(matches!(prepared.feed, Some(Feed::Pcm(_))));
        assert!(matches!(&prepared.audio[0], AudioInputDesc::Pcm { pipe, rate: 48_000, channels: 2 } if pipe == "-"));
        assert!(!stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn animations_take_no_audio_and_windows_drops_loopback_devices() {
        let input = RecordingInput { video: frames(), audio: vec![device("Mic", false)] };
        let gif = prepare(input, &spec(laqta_domain::record::VideoCodec::Gif), Host::Linux);
        assert!(gif.audio.is_empty());

        let input = RecordingInput { video: frames(), audio: vec![device("Speakers", true), device("Mic", false)] };
        let windows = prepare(input, &spec(laqta_domain::record::VideoCodec::X264), Host::Windows);
        assert_eq!(windows.audio.len(), 1);
        assert!(matches!(&windows.audio[0], AudioInputDesc::Device { name, .. } if name == "Mic"));
    }

    #[test]
    fn fake_frames_are_bgra_sized() {
        let mut source = FakeFrames { size: (4, 2), left: 1 };
        let frame = source.next_frame().expect("frame").expect("some");
        assert_eq!(frame.format(), PixelFormat::Bgra8);
        assert_eq!(frame.pixels().len(), 4 * 2 * 4);
    }
}
