//! Audit probes for the video tools (tools area). They run the real `ffmpeg` on tiny lavfi inputs and
//! skip when it is not on PATH. A probe named `audit_bug_*` fails while the bug it shows is present.
//!
//! The tools window's trimmer (`VideoTrimmer` and `GifTrimmer` share it) writes `<name>-trimmed.<same
//! extension>` (`with_suffix(&p, "-trimmed", None)` in laqta-ui `tools_window.rs`) and calls
//! `trim_video`, whose spec has `codec: None`, which `transcode_args` turns into H.264 + AAC.

use std::path::Path;
use std::process::Command;

use laqta_media::argv::transcode_args;
use laqta_ports::TranscodeSpec;

fn ffmpeg_ok() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

fn make_input(path: &Path, extra: &[&str]) {
    let status = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc=size=64x48:rate=10", "-t", "2"])
        .args(extra)
        .arg(path)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "fixture {}", path.display());
}

fn trim(input: &Path, output: &Path) -> (bool, String) {
    let spec = TranscodeSpec { start_secs: Some(0.5), duration_secs: Some(1.0), ..TranscodeSpec::default() };
    let args = transcode_args(&spec, input, output);
    let out = Command::new("ffmpeg").args(&args).output().expect("ffmpeg runs");
    (out.status.success() && output.is_file(), format!("{args:?}\n{}", String::from_utf8_lossy(&out.stderr)))
}

/// The animated GIF trimmer re-encodes a GIF to H.264 into a `.gif` file, which FFmpeg's GIF muxer refuses.
#[test]
fn audit_bug_gif_trimmer_cannot_trim_a_gif() {
    if !ffmpeg_ok() {
        eprintln!("skipped: no ffmpeg");
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let input = dir.path().join("in.gif");
    make_input(&input, &[]);
    let (ok, log) = trim(&input, &dir.path().join("in-trimmed.gif"));
    assert!(ok, "GIF trim failed:\n{log}");
}

/// The video trimmer forces H.264 + AAC, which the WebM muxer refuses, so a `.webm` cannot be trimmed.
#[test]
fn audit_bug_video_trimmer_cannot_trim_a_webm() {
    if !ffmpeg_ok() {
        eprintln!("skipped: no ffmpeg");
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let input = dir.path().join("in.webm");
    make_input(&input, &["-c:v", "libvpx-vp9"]);
    let (ok, log) = trim(&input, &dir.path().join("in-trimmed.webm"));
    assert!(ok, "WebM trim failed:\n{log}");
}

/// Control: an MP4 trims fine with the same arguments.
#[test]
fn audit_mp4_trim_works() {
    if !ffmpeg_ok() {
        eprintln!("skipped: no ffmpeg");
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let input = dir.path().join("in.mp4");
    make_input(&input, &["-c:v", "libx264", "-pix_fmt", "yuv420p"]);
    let (ok, log) = trim(&input, &dir.path().join("in-trimmed.mp4"));
    assert!(ok, "MP4 trim failed:\n{log}");
}
