//! Checks for the recording argument lists. They run the argument lists `laqta_media::argv` builds against the
//! local `ffmpeg`, with OS inputs (pulse, v4l2, raw stdin) swapped for `lavfi` test sources so no
//! screen, camera or microphone is opened. They need `ffmpeg` and `ffprobe` on PATH and fail without.
// The inputs swapped out are the Linux ones, so the checks run on Unix only.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use futures_lite::future::block_on;
use laqta_domain::geometry::PhysRect;
use laqta_domain::record::{CameraOverlay, CameraShape, Corner, Quality, RecordSpec, VideoCodec};
use laqta_domain::{Bitmap, PixelFormat};
use laqta_media::argv::{self, AudioInputDesc, Host, VideoInputDesc, WindowsCapture};
use laqta_media::{FfmpegMedia, find_ffmpeg};
use laqta_ports::PortError;
use laqta_ports::recording::{FrameSource, Media, RecordingInput, VideoSource};

fn spec(codec: VideoCodec, quality: Quality) -> RecordSpec {
    RecordSpec {
        area: PhysRect { x: 0, y: 0, width: 320, height: 240 },
        codec,
        fps: 15,
        quality,
        show_cursor: false,
        audio: Vec::new(),
        custom_args: String::new(),
        camera: None,
    }
}

fn mic(id: &str) -> AudioInputDesc {
    AudioInputDesc::Device { id: id.into(), name: id.into(), loopback: false }
}

/// Swaps every OS input in `args` for a lavfi source and limits the run to one second.
fn with_test_inputs(args: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "-f" && i + 1 < args.len() {
            match args[i + 1].as_str() {
                "rawvideo" => {
                    // -f rawvideo -pix_fmt bgra -s WxH -r fps -i -
                    let size = args[i + 5].clone();
                    let rate = args[i + 7].clone();
                    out.extend(["-f".into(), "lavfi".into(), "-i".into(), format!("testsrc=size={size}:rate={rate}")]);
                    i += 10;
                    continue;
                }
                "pulse" => {
                    out.extend(["-f".into(), "lavfi".into(), "-i".into(), "sine=frequency=440:sample_rate=48000".into()]);
                    i += 4;
                    continue;
                }
                "v4l2" => {
                    out.extend(["-f".into(), "lavfi".into(), "-i".into(), "testsrc2=size=640x480:rate=15".into()]);
                    i += 4;
                    continue;
                }
                _ => {}
            }
        }
        out.push(a.clone());
        i += 1;
    }
    // `-t 1` goes right before the output file (the last argument).
    let last = out.pop().expect("an output");
    out.extend(["-t".into(), "1".into(), last]);
    out
}

fn run(program: &str, args: &[String]) -> std::process::Output {
    Command::new(program).args(args).output().unwrap_or_else(|e| panic!("{program} must be on PATH for this probe: {e}"))
}

fn probe_streams(path: &Path) -> String {
    let out = run(
        "ffprobe",
        &[
            "-v".into(),
            "error".into(),
            "-show_entries".into(),
            "stream=codec_type,codec_name".into(),
            "-of".into(),
            "csv=p=0".into(),
            path.to_string_lossy().into_owned(),
        ],
    );
    assert!(out.status.success(), "ffprobe failed on {}: {}", path.display(), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Records one segment from `args`, then joins it into `final_ext` the way `process.rs` does.
fn segment_then_concat(dir: &Path, s: &RecordSpec, video: &VideoInputDesc, audio: &[AudioInputDesc]) -> PathBuf {
    let seg = dir.join("seg-0001.mkv");
    let args = with_test_inputs(argv::record_args_on(Host::Linux, WindowsCapture::Ddagrab, s, video, audio, &seg));
    let out = run("ffmpeg", &args);
    assert!(out.status.success(), "segment failed for {:?}\nargs: {args:?}\n{}", s.codec, String::from_utf8_lossy(&out.stderr));
    std::fs::write(dir.join("list.txt"), argv::concat_list(&["seg-0001.mkv".into()])).expect("list");
    let output = dir.join(format!("final.{}", s.codec.extension()));
    let concat = argv::concat_args("list.txt", &output.to_string_lossy(), None);
    let out = Command::new("ffmpeg").args(&concat).current_dir(dir).output().expect("ffmpeg");
    assert!(out.status.success(), "concat failed for {:?}: {}", s.codec, String::from_utf8_lossy(&out.stderr));
    output
}

#[test]
fn camera_overlay_graphs_with_two_mixed_audio_inputs_run_in_ffmpeg() {
    for shape in [CameraShape::Circle, CameraShape::RoundedRectangle, CameraShape::Rectangle] {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut s = spec(VideoCodec::X264, Quality::Crf(23));
        s.camera =
            Some(CameraOverlay { enabled: true, device: String::new(), shape, corner: Corner::TopLeft, size_percent: 25, margin: 16 });
        let video = VideoInputDesc::RawFrames { width: 320, height: 240, fps: 15 };
        let output = segment_then_concat(dir.path(), &s, &video, &[mic("a"), mic("b")]);
        let streams = probe_streams(&output);
        assert!(streams.contains("h264,video") && streams.contains("aac,audio"), "{shape:?}: {streams}");
    }
}

#[test]
fn every_codec_with_audio_records_and_joins_into_its_container() {
    let video = VideoInputDesc::RawFrames { width: 320, height: 240, fps: 15 };
    for (codec, vname, aname) in [
        (VideoCodec::X264, "h264", "aac"),
        (VideoCodec::X265, "hevc", "aac"),
        (VideoCodec::Vp8, "vp8", "opus"),
        (VideoCodec::Vp9, "vp9", "opus"),
        (VideoCodec::Av1, "av1", "aac"),
        (VideoCodec::Xvid, "mpeg4", "aac"),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = segment_then_concat(dir.path(), &spec(codec, Quality::Crf(30)), &video, &[mic("a")]);
        let streams = probe_streams(&output);
        assert!(streams.contains(&format!("{vname},video")) && streams.contains(&format!("{aname},audio")), "{codec:?}: {streams}");
    }
}

/// Frames paced in real time (as `tests/ffmpeg.rs` does).
struct Paced {
    left: u32,
    frame: u32,
}

impl FrameSource for Paced {
    fn size(&self) -> (u32, u32) {
        (64, 48)
    }
    fn fps(&self) -> u32 {
        15
    }
    fn next_frame(&mut self) -> Result<Option<Bitmap>, PortError> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        thread::sleep(Duration::from_secs(1) / 15);
        self.frame += 1;
        let pixels = vec![(self.frame * 9 % 256) as u8; 64 * 48 * 4];
        Bitmap::new(64, 48, PixelFormat::Bgra8, pixels.into()).map(Some).map_err(|e| PortError::Failed(e.to_string()))
    }
    fn stop(&mut self) {
        self.left = 0;
    }
}

#[test]
fn webp_and_apng_recordings_finish_through_the_real_pipeline() {
    for (codec, name) in [(VideoCodec::WebP, "webp"), (VideoCodec::Apng, "apng")] {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = find_ffmpeg("", dir.path()).expect("ffmpeg and ffprobe on PATH");
        if codec == VideoCodec::WebP {
            let listed = std::process::Command::new(&paths.ffmpeg).args(["-hide_banner", "-encoders"]).output().expect("list the encoders");
            if !String::from_utf8_lossy(&listed.stdout).split_whitespace().any(|word| word == "libwebp_anim") {
                eprintln!("skipping WebP: this ffmpeg has no libwebp_anim");
                continue;
            }
        }
        let media = FfmpegMedia::new(paths, dir.path().join("work"));
        let output = dir.path().join(format!("clip.{}", codec.extension()));
        let input = RecordingInput { video: VideoSource::Frames(Box::new(Paced { left: 15, frame: 0 })), audio: Vec::new() };
        let mut s = spec(codec, Quality::Default);
        s.area = PhysRect { x: 0, y: 0, width: 64, height: 48 };
        let handle = block_on(media.start(input, s, output.clone())).expect("start");
        thread::sleep(Duration::from_millis(1500));
        let path = block_on(handle.stop()).expect("stop");
        let streams = probe_streams(&path);
        assert!(streams.contains(name), "{codec:?}: {streams}");
    }
}

/// The quality flag each hardware encoder gets must be one of that encoder's options. FFmpeg
/// ignores an unknown codec option with only a warning (hidden by `-loglevel error`), so the
/// quality setting would silently do nothing.
#[test]
#[ignore = "known issue, not fixed yet"]
fn audit_bug_hardware_quality_flag_is_an_option_of_its_encoder() {
    let mut wrong = Vec::new();
    for (codec, encoder) in [
        (VideoCodec::Nvenc, "h264_nvenc"),
        (VideoCodec::NvencHevc, "hevc_nvenc"),
        (VideoCodec::Amf, "h264_amf"),
        (VideoCodec::Qsv, "h264_qsv"),
        (VideoCodec::Vaapi, "h264_vaapi"),
    ] {
        let video = VideoInputDesc::RawFrames { width: 64, height: 48, fps: 15 };
        let args =
            argv::record_args_on(Host::Linux, WindowsCapture::Ddagrab, &spec(codec, Quality::Crf(23)), &video, &[], Path::new("/o.mkv"));
        let flag = args.windows(2).find(|w| w[1] == "23").map(|w| w[0].clone()).expect("a quality flag");
        let help = run("ffmpeg", &["-hide_banner".into(), "-h".into(), format!("encoder={encoder}")]);
        let help = String::from_utf8_lossy(&help.stdout).into_owned();
        if !help.lines().any(|line| line.trim_start().starts_with(&format!("{flag} "))) {
            wrong.push(format!("{encoder} gets {flag} 23, which it does not have"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
