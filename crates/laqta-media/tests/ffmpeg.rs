//! FFmpeg end to end: record, pause and resume, GIF, transcode, thumbnail and crash survival.
//! Each test skips (and says so) when `ffmpeg` and `ffprobe` are not on PATH.

use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use futures_lite::future::block_on;
use laqta_domain::Bitmap;
use laqta_domain::PixelFormat;
use laqta_domain::geometry::PhysRect;
use laqta_domain::record::{Quality, RecordSpec, VideoCodec};
use laqta_media::{FfmpegMedia, argv, find_ffmpeg};
use laqta_ports::PortError;
#[cfg(unix)]
use laqta_ports::recording::{AudioInput, PcmFormat, PcmSource};
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

/// Frames that arrive at the source's own rate, not at the recording's: one every `gap` (none
/// waits for the next at all) from the first call on. After `run` of wall time it ends (None =
/// endless). The same instance is reused across segments.
struct TimedFrames {
    gap: Duration,
    run: Option<Duration>,
    started: Option<Instant>,
    frame: u32,
}

impl TimedFrames {
    fn new(gap: Duration, run: Option<Duration>) -> Self {
        TimedFrames { gap, run, started: None, frame: 0 }
    }
}

impl FrameSource for TimedFrames {
    fn size(&self) -> (u32, u32) {
        (64, 48)
    }

    fn fps(&self) -> u32 {
        15
    }

    fn next_frame(&mut self) -> Result<Option<Bitmap>, PortError> {
        let started = *self.started.get_or_insert_with(Instant::now);
        if self.frame > 0 {
            thread::sleep(self.gap);
        }
        if self.run.is_some_and(|run| started.elapsed() >= run) {
            return Ok(None);
        }
        self.frame = self.frame.wrapping_add(1);
        let pixels = vec![(self.frame % 256) as u8; 64 * 48 * 4];
        Bitmap::new(64, 48, PixelFormat::Bgra8, pixels.into()).map(Some).map_err(|err| PortError::Failed(err.to_string()))
    }

    fn stop(&mut self) {
        self.run = Some(Duration::ZERO);
    }
}

/// `total` frames of 320x200 with a pattern that moves from frame to frame, handed out without
/// waiting (the recorder paces them). Then the source ends.
struct MovingFrames {
    fps: u32,
    left: u32,
    frame: u32,
}

impl FrameSource for MovingFrames {
    fn size(&self) -> (u32, u32) {
        (320, 200)
    }

    fn fps(&self) -> u32 {
        self.fps
    }

    fn next_frame(&mut self) -> Result<Option<Bitmap>, PortError> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        self.frame += 1;
        let shift = self.frame as usize * 3;
        let mut pixels = Vec::with_capacity(320 * 200 * 4);
        for y in 0..200usize {
            for x in 0..320usize {
                pixels.extend_from_slice(&[((x + shift) % 256) as u8, (y * 255 / 199) as u8, ((x + y + shift) % 256) as u8, 255]);
            }
        }
        Bitmap::new(320, 200, PixelFormat::Bgra8, pixels.into()).map(Some).map_err(|err| PortError::Failed(err.to_string()))
    }

    fn stop(&mut self) {
        self.left = 0;
    }
}

fn input(source: impl FrameSource + 'static) -> RecordingInput {
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

/// A 440 Hz tone at wall-clock speed, as an OS loopback gives it.
#[cfg(unix)]
struct Tone {
    started: Option<Instant>,
    sent: u64,
    stopped: bool,
}

#[cfg(unix)]
impl PcmSource for Tone {
    fn format(&self) -> PcmFormat {
        PcmFormat { sample_rate: 48_000, channels: 2 }
    }

    fn read(&mut self, buf: &mut [i16]) -> Result<usize, PortError> {
        if self.stopped {
            return Ok(0);
        }
        thread::sleep(Duration::from_millis(10));
        let due = (self.started.get_or_insert_with(Instant::now).elapsed().as_secs_f64() * 48_000.0) as u64;
        let frames = (due.saturating_sub(self.sent) as usize).min(buf.len() / 2);
        for (i, frame) in buf[..frames * 2].chunks_mut(2).enumerate() {
            let t = (self.sent + i as u64) as f64 / 48_000.0;
            frame.fill(((t * 440.0 * std::f64::consts::TAU).sin() * 8000.0) as i16);
        }
        self.sent += frames as u64;
        // Never 0 while running: 0 means the end of the sound.
        if frames == 0 {
            buf[..2].fill(0);
            return Ok(2);
        }
        Ok(frames * 2)
    }

    fn stop(&mut self) {
        self.stopped = true;
    }
}

/// macOS hands over frames and system sound together; the sound goes through a named pipe.
#[cfg(unix)]
#[test]
fn raw_frames_and_pcm_sound_are_recorded_together_across_a_pause() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(media) = media(dir.path()) else { return };
    let output = dir.path().join("sound.mp4");
    let input = RecordingInput {
        video: VideoSource::Frames(Box::new(PacedFrames::new(30, None))),
        audio: vec![AudioInput::Pcm(Box::new(Tone { started: None, sent: 0, stopped: false }))],
    };
    let mut recording = block_on(media.start(input, spec(VideoCodec::X264, 30), output.clone())).expect("start");
    thread::sleep(Duration::from_millis(1500));
    block_on(recording.pause()).expect("pause");
    thread::sleep(Duration::from_millis(300));
    block_on(recording.resume()).expect("resume");
    thread::sleep(Duration::from_millis(1500));
    assert!(recording.is_alive());
    let started = Instant::now();
    block_on(recording.stop()).expect("stop");
    assert!(started.elapsed() < Duration::from_secs(4), "stop did not wait on the sound pipe");
    let info = probe(&media, &output);
    assert!(info.audio_codec.is_some(), "the recording has a sound stream");
    assert_near(info.duration_secs, 3.0, 0.9);
    assert!(!dir.path().join("work").read_dir().is_ok_and(|mut entries| entries.next().is_some()), "the session folder is gone");
}

#[test]
fn a_fast_source_is_paced_to_wall_time() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(media) = media(dir.path()) else { return };
    let output = dir.path().join("fast.mp4");
    let source = TimedFrames::new(Duration::ZERO, Some(Duration::from_secs(2)));
    let recording = block_on(media.start(input(source), spec(VideoCodec::X264, 15), output.clone())).expect("start");
    thread::sleep(Duration::from_millis(2400));
    block_on(recording.stop()).expect("stop");
    assert_near(probe(&media, &output).duration_secs, 2.0, 0.3);
}

#[test]
fn a_slow_source_is_filled_to_wall_time() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(media) = media(dir.path()) else { return };
    let output = dir.path().join("slow.mp4");
    let source = TimedFrames::new(Duration::from_millis(200), Some(Duration::from_secs(2)));
    let recording = block_on(media.start(input(source), spec(VideoCodec::X264, 15), output.clone())).expect("start");
    thread::sleep(Duration::from_millis(2400));
    block_on(recording.stop()).expect("stop");
    assert_near(probe(&media, &output).duration_secs, 2.0, 0.3);
}

#[test]
fn a_resumed_segment_does_not_fill_the_pause() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(media) = media(dir.path()) else { return };
    let output = dir.path().join("resumed.mp4");
    let source = TimedFrames::new(Duration::ZERO, None);
    let mut recording = block_on(media.start(input(source), spec(VideoCodec::X264, 15), output.clone())).expect("start");
    thread::sleep(Duration::from_secs(1));
    block_on(recording.pause()).expect("pause");
    thread::sleep(Duration::from_millis(800));
    block_on(recording.resume()).expect("resume");
    thread::sleep(Duration::from_secs(1));
    block_on(recording.stop()).expect("stop");
    // Two running stretches of 1 s each, not the 0.8 s pause between them.
    assert_near(probe(&media, &output).duration_secs, 2.0, 0.4);
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

/// The number of video packets in `path`, which for a GIF is its number of frames.
fn frame_count(media: &FfmpegMedia, path: &Path) -> u64 {
    let output = Command::new(&media.paths().ffprobe).args(argv::packet_count_args(path)).output().expect("run ffprobe");
    assert!(output.status.success(), "ffprobe failed: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().parse().expect("a packet count")
}

#[test]
fn a_120_frame_gif_keeps_its_frames_and_leaves_no_temporary_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(media) = media(dir.path()) else { return };
    let output = dir.path().join("clip.gif");
    let mut gif = spec(VideoCodec::Gif, 30);
    gif.area = PhysRect { x: 0, y: 0, width: 320, height: 200 };
    let source = MovingFrames { fps: 30, left: 120, frame: 0 };
    let recording = block_on(media.start(input(source), gif, output.clone())).expect("start");
    // 120 frames at 30 fps take 4 s to feed; the source then ends and FFmpeg finishes.
    thread::sleep(Duration::from_millis(4600));
    let path = block_on(recording.stop()).expect("stop");
    assert_eq!(path, output);
    let info = probe(&media, &output);
    assert_eq!(info.video_codec, "gif");
    assert_eq!((info.width, info.height), (320, 200));
    // A frame that arrives late is written again for the period it missed, so a loaded machine can
    // add a few frames. None may be lost.
    let frames = frame_count(&media, &output);
    assert!((120..=126).contains(&frames), "expected 120 frames, got {frames}");
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .expect("list the output folder")
        .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!names.iter().any(|name| name.ends_with(".part") || name.ends_with(".mkv")), "left over: {names:?}");
    let leftovers = std::fs::read_dir(dir.path().join("work")).map(|entries| entries.count()).unwrap_or(0);
    assert_eq!(leftovers, 0, "the session folder, with its palette, is removed");
}

#[test]
fn a_failed_gif_conversion_leaves_no_gif_and_keeps_the_intermediate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(media) = media(dir.path()) else { return };
    let output = dir.path().join("clip.gif");
    // A directory where the temporary file belongs makes FFmpeg unable to write it.
    std::fs::create_dir(dir.path().join("clip.gif.part")).expect("block the temporary name");
    let recording = block_on(media.start(input(PacedFrames::new(15, Some(15))), spec(VideoCodec::Gif, 15), output.clone())).expect("start");
    thread::sleep(Duration::from_millis(1500));
    let error = block_on(recording.stop()).expect_err("the conversion fails");
    let kept = dir.path().join("clip.gif.mkv");
    assert!(error.to_string().contains(&kept.display().to_string()), "the error names the intermediate: {error}");
    assert!(!output.exists(), "no broken GIF is left");
    // The intermediate is a playable file, and the session folder (palette, logs) is gone.
    assert!(probe(&media, &kept).duration_secs > 0.5);
    let leftovers = std::fs::read_dir(dir.path().join("work")).map(|entries| entries.count()).unwrap_or(0);
    assert_eq!(leftovers, 0, "the session folder is removed");
}

#[test]
fn webp_and_apng_are_converted_through_a_temporary_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(media) = media(dir.path()) else { return };
    for (codec, name, expected) in [(VideoCodec::WebP, "clip.webp", "webp"), (VideoCodec::Apng, "clip.png", "apng")] {
        let output = dir.path().join(name);
        let recording = block_on(media.start(input(PacedFrames::new(15, Some(15))), spec(codec, 15), output.clone())).expect("start");
        thread::sleep(Duration::from_millis(1500));
        assert_eq!(block_on(recording.stop()).expect("stop"), output);
        // FFmpeg names the WebP codec `webp_anim`.
        let codec_name = probe(&media, &output).video_codec;
        assert!(codec_name.contains(expected), "{name}: {codec_name}");
        assert!(!dir.path().join(format!("{name}.part")).exists());
    }
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
