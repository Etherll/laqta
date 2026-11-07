//! FFmpeg end to end: record, pause and resume, GIF, transcode, thumbnail and crash survival.
//! Each test skips (and says so) when `ffmpeg` and `ffprobe` are not on PATH.

use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

use futures_lite::future::block_on;
use laqta_domain::Bitmap;
use laqta_domain::PixelFormat;
use laqta_domain::geometry::PhysRect;
use laqta_domain::record::{Quality, RecordSpec, VideoCodec};
use laqta_media::{FfmpegMedia, find_ffmpeg};
use laqta_ports::PortError;
use laqta_ports::recording::{FrameSource, Media, MediaInfo, RecordingHandle, RecordingInput, TranscodeSpec, VideoSource};

/// Frames paced in real time, so a recording can be paused and killed while it runs. `left` ends
/// the source after that many frames (None = endless). The same instance is reused across
/// segments.
struct PacedFrames {
    size: (u32, u32),
    fps: u32,
    left: Option<u32>,
    frame: u32,
}

impl PacedFrames {
    fn new(fps: u32, left: Option<u32>) -> Self {
        PacedFrames { size: (64, 48), fps, left, frame: 0 }
    }
}

impl FrameSource for PacedFrames {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn fps(&self) -> u32 {
        self.fps
    }

    fn next_frame(&mut self) -> Result<Option<Bitmap>, PortError> {
        if let Some(left) = self.left {
            if left == 0 {
                return Ok(None);
            }
            self.left = Some(left - 1);
        }
        thread::sleep(Duration::from_secs(1) / self.fps);
        self.frame = self.frame.wrapping_add(1);
        let (width, height) = self.size;
        let shade = (self.frame % 256) as u8;
        let pixels = vec![shade; width as usize * height as usize * 4];
        Bitmap::new(width, height, PixelFormat::Bgra8, pixels.into()).map(Some).map_err(|err| PortError::Failed(err.to_string()))
    }

    fn stop(&mut self) {
        self.left = Some(0);
    }
}

fn input(source: PacedFrames) -> RecordingInput {
    RecordingInput { video: VideoSource::Frames(Box::new(source)), audio: Vec::new() }
}

fn spec(codec: VideoCodec, fps: u32) -> RecordSpec {
    RecordSpec {
        area: PhysRect { x: 0, y: 0, width: 64, height: 48 },
        codec,
        fps,
        quality: Quality::Default,
        show_cursor: false,
        audio: Vec::new(),
        custom_args: String::new(),
        camera: None,
    }
}

/// The media adapter over the FFmpeg on PATH, with its work folder inside `dir`. `None` skips.
fn media(dir: &Path) -> Option<FfmpegMedia> {
    match find_ffmpeg("", dir) {
        Some(paths) => Some(FfmpegMedia::new(paths, dir.join("work"))),
        None => {
            eprintln!("skipping: ffmpeg and ffprobe are not on PATH");
            None
        }
    }
}

fn probe(media: &FfmpegMedia, path: &Path) -> MediaInfo {
    block_on(media.probe(path)).expect("probe the output")
}

fn assert_near(actual: f64, expected: f64, tolerance: f64) {
    assert!((actual - expected).abs() <= tolerance, "expected {expected} +/- {tolerance}, got {actual}");
}

#[test]
fn records_mp4_and_probes_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(media) = media(dir.path()) else { return };
    let output = dir.path().join("clip.mp4");
    let recording =
        block_on(media.start(input(PacedFrames::new(30, Some(30))), spec(VideoCodec::X264, 30), output.clone())).expect("start");
    thread::sleep(Duration::from_millis(1500));
    let path = block_on(recording.stop()).expect("stop");
    assert_eq!(path, output);
    let info = probe(&media, &output);
    assert_eq!((info.width, info.height), (64, 48));
    assert_eq!(info.video_codec, "h264");
    assert_near(info.duration_secs, 1.0, 0.4);
}

#[test]
fn pause_and_resume_join_two_segments() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(media) = media(dir.path()) else { return };
    let output = dir.path().join("paused.mp4");
    let mut recording =
        block_on(media.start(input(PacedFrames::new(30, None)), spec(VideoCodec::X264, 30), output.clone())).expect("start");
    thread::sleep(Duration::from_secs(1));
    block_on(recording.pause()).expect("pause");
    thread::sleep(Duration::from_millis(500));
    block_on(recording.resume()).expect("resume");
    thread::sleep(Duration::from_secs(1));
    assert!(recording.is_alive());
    let path = block_on(recording.stop()).expect("stop");
    assert_eq!(path, output);
    let info = probe(&media, &output);
    // Only the two running stretches have frames: about 2 s, not 2.5 s.
    assert_near(info.duration_secs, 2.0, 0.6);
    assert_eq!((info.width, info.height), (64, 48));
}

#[test]
fn pause_right_after_start_leaves_out_the_empty_segment() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(media) = media(dir.path()) else { return };
    let output = dir.path().join("quick.mp4");
    let recording = block_on(media.start(input(PacedFrames::new(30, None)), spec(VideoCodec::X264, 30), output.clone())).expect("start");
    thread::sleep(Duration::from_millis(50));
    let mut recording = recording;
    block_on(recording.pause()).expect("pause");
    thread::sleep(Duration::from_millis(300));
    block_on(recording.resume()).expect("resume");
    thread::sleep(Duration::from_secs(1));
    block_on(recording.stop()).expect("stop joins the usable segments");
    // The first segment may be too short to hold a cluster. Either way about 1 s is kept.
    assert_near(probe(&media, &output).duration_secs, 1.0, 0.6);
}

#[test]
fn gif_is_converted_from_the_lossless_intermediate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(media) = media(dir.path()) else { return };
    let output = dir.path().join("clip.gif");
    let recording = block_on(media.start(input(PacedFrames::new(15, Some(15))), spec(VideoCodec::Gif, 15), output.clone())).expect("start");
    thread::sleep(Duration::from_millis(1500));
    let path = block_on(recording.stop()).expect("stop");
    assert_eq!(path, output);
    let info = probe(&media, &output);
    assert_eq!(info.video_codec, "gif");
    assert_eq!((info.width, info.height), (64, 48));
    let head = std::fs::read(&output).expect("read gif");
    assert!(head.starts_with(b"GIF8"), "not a GIF");
}

#[test]
fn transcode_trims_and_a_thumbnail_is_one_frame() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(media) = media(dir.path()) else { return };
    let source = dir.path().join("source.mp4");
    let recording =
        block_on(media.start(input(PacedFrames::new(30, Some(60))), spec(VideoCodec::X264, 30), source.clone())).expect("start");
    thread::sleep(Duration::from_millis(2500));
    block_on(recording.stop()).expect("stop");
    assert_near(probe(&media, &source).duration_secs, 2.0, 0.5);

    let trimmed = dir.path().join("trimmed.mp4");
    let trim = TranscodeSpec { start_secs: Some(0.5), duration_secs: Some(1.0), ..TranscodeSpec::default() };
    block_on(media.transcode(&source, &trimmed, trim)).expect("trim");
    assert_near(probe(&media, &trimmed).duration_secs, 1.0, 0.4);

    let thumb = dir.path().join("thumb.png");
    let frame = TranscodeSpec { start_secs: Some(0.5), single_frame: true, ..TranscodeSpec::default() };
    block_on(media.transcode(&source, &thumb, frame)).expect("thumbnail");
    let info = probe(&media, &thumb);
    assert_eq!(info.video_codec, "png");
    assert_eq!((info.width, info.height), (64, 48));
}

#[test]
fn crash_survival_keeps_a_playable_partial_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(media) = media(dir.path()) else { return };
    let output = dir.path().join("crashed.mp4");
    let recording =
        block_on(media.start_recorder(input(PacedFrames::new(30, None)), spec(VideoCodec::X264, 30), output.clone())).expect("start");
    thread::sleep(Duration::from_secs(2));
    let pid = recording.pid().expect("the segment has a process");
    // A hard kill, as a crash would be: SIGKILL on Unix, TerminateProcess on Windows.
    let killed = if cfg!(windows) {
        Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).status()
    } else {
        Command::new("kill").args(["-KILL", &pid.to_string()]).status()
    }
    .expect("run the kill tool");
    assert!(killed.success());
    thread::sleep(Duration::from_millis(500));

    let mut handle: Box<dyn RecordingHandle> = Box::new(recording);
    assert!(!handle.is_alive(), "the killed FFmpeg is reported as gone");
    let path = block_on(handle.stop()).expect("stop returns the partial file");
    assert_eq!(path, output);
    let info = probe(&media, &output);
    assert!(info.duration_secs > 0.5, "partial file has frames: {info:?}");
    assert_eq!((info.width, info.height), (64, 48));
}

#[test]
fn abort_deletes_everything() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(media) = media(dir.path()) else { return };
    let output = dir.path().join("aborted.mp4");
    let recording = block_on(media.start(input(PacedFrames::new(30, None)), spec(VideoCodec::X264, 30), output.clone())).expect("start");
    thread::sleep(Duration::from_millis(500));
    block_on(recording.abort()).expect("abort");
    assert!(!output.exists());
    let leftovers = std::fs::read_dir(dir.path().join("work")).map(|entries| entries.count()).unwrap_or(0);
    assert_eq!(leftovers, 0, "the session folder is removed");
}
