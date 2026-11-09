//! FFmpeg argument lists (ARCHITECTURE §6). They are built as data and passed to `Command`
//! without a shell. Every function here is pure, so `process` and `media` only run what it returns.

use std::path::Path;

use laqta_domain::record::{Quality, RecordSpec, VideoCodec};
use laqta_ports::recording::{OsScreen, TranscodeSpec};

/// Every run starts with these: overwrite the output, and print only errors.
const COMMON: [&str; 5] = ["-y", "-hide_banner", "-loglevel", "error", "-nostats"];
/// Segments are Matroska, which survives a crash. The final remux writes the chosen container.
const SEGMENT_FORMAT: &str = "matroska";
/// Clusters close every 500 ms, and packets are flushed as they are written. A segment killed
/// midway then keeps everything up to its last closed cluster. Without this the file is only a
/// header (checked with ffmpeg 9.0.2 on a SIGKILL after 2 s).
const CLUSTER_MS: &str = "500";
/// The VAAPI render node used when none is configured.
const VAAPI_DEVICE: &str = "/dev/dri/renderD128";
/// VP8 and VP9 without a set quality use constant-quality CRF with `-b:v 0`.
const VPX_DEFAULT_CRF: u8 = 30;
const AUDIO_BITRATE: &str = "192k";

/// The OS whose audio devices the argument lists address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    Windows,
    Linux,
}

impl Host {
    /// The host this build runs on.
    pub fn current() -> Host {
        if cfg!(windows) { Host::Windows } else { Host::Linux }
    }
}

/// How Windows captures a `Target`. `ddagrab` is the default. `gdigrab` is the fallback
/// when `ddagrab` fails at start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowsCapture {
    Ddagrab,
    Gdigrab,
}

/// The video input, as the process layer describes it.
#[derive(Clone, Debug, PartialEq)]
pub enum VideoInputDesc {
    /// The OS captures the area itself: `ddagrab` or `gdigrab` on Windows, `x11grab` on X11.
    Target(OsScreen),
    /// BGRA frames on stdin, at `width` x `height`.
    RawFrames { width: u32, height: u32, fps: u32 },
}

/// An audio input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioInputDesc {
    /// An OS device: `dshow` by its name on Windows, `pulse` by its id on Linux.
    Device { id: String, name: String, loopback: bool },
    /// Interleaved s16le PCM from `pipe`. `-` means stdin.
    Pcm { rate: u32, channels: u16, pipe: String },
}

/// The recording command for one segment, for the host this build runs on. Windows captures with
/// `ddagrab`. Use `record_args_on` to pick the `gdigrab` fallback or another host.
pub fn record_args(spec: &RecordSpec, video: &VideoInputDesc, audio: &[AudioInputDesc], output: &Path) -> Vec<String> {
    record_args_on(Host::current(), WindowsCapture::Ddagrab, spec, video, audio, output)
}

/// The recording command for `host`, with the given Windows capture tool. A stdin input (raw frames
/// or PCM) ends with EOF, so `-shortest` is added, and `amix` ends with the shortest input. At most
/// one input may use stdin.
pub fn record_args_on(
    host: Host,
    capture: WindowsCapture,
    spec: &RecordSpec,
    video: &VideoInputDesc,
    audio: &[AudioInputDesc],
    output: &Path,
) -> Vec<String> {
    let codec = spec.codec;
    // GIF, WebP and APNG carry no sound.
    let audio: &[AudioInputDesc] = if codec.is_animation() { &[] } else { audio };
    let stdin_fed = stdin_fed(video, audio);
    let mut a = Argv::default();
    a.flag(&COMMON);
    device_options(&mut a, codec, video, capture);
    video_input(&mut a, spec, video, capture);
    for input in audio {
        audio_input(&mut a, host, input);
    }
    let camera = spec.camera.as_ref().filter(|c| c.enabled && camera_device(host, c).is_some());
    if let Some(cam) = camera {
        camera_input(&mut a, host, cam);
        let index = 1 + audio.len();
        a.flag(&["-filter_complex"]).push(camera_graph(spec, cam, index, &input_filters(video, capture), &codec_filters(codec)));
        a.flag(&["-map", "[vout]"]);
    } else {
        let filters: Vec<&str> = input_filters(video, capture).into_iter().chain(codec_filters(codec)).collect();
        if !filters.is_empty() {
            a.opt("-vf", filters.join(","));
        }
        a.flag(&["-map", "0:v"]);
    }
    map_audio(&mut a, audio.len(), stdin_fed);
    encoder(&mut a, codec, spec.quality);
    if !audio.is_empty() {
        audio_encoder(&mut a, codec);
    }
    if stdin_fed && !audio.is_empty() {
        a.flag(&["-shortest"]);
    }
    for arg in split_args(&spec.custom_args) {
        a.push(arg);
    }
    a.flag(&["-f", SEGMENT_FORMAT, "-flush_packets", "1", "-cluster_time_limit", CLUSTER_MS]);
    a.push(output.to_string_lossy().into_owned());
    a.finish()
}

/// The command that converts a finished file: a transcode, a trim, a scale, or a single thumbnail
/// frame. A codec of `None` means H.264.
pub fn transcode_args(spec: &TranscodeSpec, input: &Path, output: &Path) -> Vec<String> {
    let codec = spec.codec.unwrap_or(VideoCodec::X264);
    let input = input.to_string_lossy().into_owned();
    let output = output.to_string_lossy().into_owned();
    let mut a = Argv::default();
    a.flag(&COMMON);
    if codec == VideoCodec::Vaapi {
        a.flag(&["-vaapi_device", VAAPI_DEVICE]);
    }
    if let Some(start) = spec.start_secs {
        a.opt("-ss", seconds(start));
    }
    a.flag(&["-i"]).push(input);
    if spec.single_frame {
        if let Some(width) = spec.width {
            a.opt("-vf", scale(width));
        }
        a.flag(&["-frames:v", "1", "-q:v", "2", "-an"]);
    } else if codec.is_animation() {
        animation_transcode(&mut a, codec, spec);
    } else {
        let filters: Vec<String> = spec.width.map(scale).into_iter().chain(codec_filters(codec).into_iter().map(String::from)).collect();
        if !filters.is_empty() {
            a.opt("-vf", filters.join(","));
        }
        encoder(&mut a, codec, spec.quality);
        audio_encoder(&mut a, codec);
        if let Some(fps) = spec.fps {
            a.opt("-r", fps);
        }
    }
    if let Some(duration) = spec.duration_secs.filter(|_| !spec.single_frame) {
        a.opt("-t", seconds(duration));
    }
    for arg in split_args(&spec.extra_args) {
        a.push(arg);
    }
    a.push(output);
    a.finish()
}

/// A transcode to GIF is two runs, like a recording's: this one writes the clip's palette (same
/// start, length, scale and rate as the second run), `transcode_gif_args` then maps through it.
pub fn transcode_palette_args(spec: &TranscodeSpec, input: &Path, palette: &Path) -> Vec<String> {
    let mut a = Argv::default();
    a.flag(&COMMON);
    if let Some(start) = spec.start_secs {
        a.opt("-ss", seconds(start));
    }
    a.flag(&["-i"]).push(input.to_string_lossy().into_owned());
    if let Some(duration) = spec.duration_secs {
        a.opt("-t", seconds(duration));
    }
    a.opt("-vf", format!("{},palettegen=stats_mode=diff", gif_prefilter(spec)));
    a.flag(&["-update", "1"]).push(palette.to_string_lossy().into_owned());
    a.finish()
}

/// The second run of a transcode to GIF, through the palette from `transcode_palette_args`.
pub fn transcode_gif_args(spec: &TranscodeSpec, input: &Path, palette: &Path, output: &Path) -> Vec<String> {
    let mut a = Argv::default();
    a.flag(&COMMON);
    if let Some(start) = spec.start_secs {
        a.opt("-ss", seconds(start));
    }
    a.flag(&["-i"]).push(input.to_string_lossy().into_owned());
    a.flag(&["-i"]).push(palette.to_string_lossy().into_owned());
    a.opt("-filter_complex", format!("[0:v]{}[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=5", gif_prefilter(spec)));
    a.flag(&["-an"]);
    if let Some(duration) = spec.duration_secs {
        a.opt("-t", seconds(duration));
    }
    for arg in split_args(&spec.extra_args) {
        a.push(arg);
    }
    a.push(output.to_string_lossy().into_owned());
    a.finish()
}

fn gif_prefilter(spec: &TranscodeSpec) -> String {
    let scale_part = spec.width.map(|w| format!("{},", scale(w))).unwrap_or_default();
    format!("{scale_part}fps={}", spec.fps.unwrap_or(15))
}

/// Converts a lossless intermediate to a WebP or APNG animation in one run. Any other codec falls
/// back to APNG. Both stream: their memory stays flat with the clip's length. A GIF is two runs
/// instead (`palette_args`, then `paletteuse_args`). The container is named, so `output` may carry
/// any extension (the recorder writes to a `.part` name first).
pub fn animation_args(codec: VideoCodec, input: &Path, output: &Path) -> Vec<String> {
    let input = input.to_string_lossy().into_owned();
    let output = output.to_string_lossy().into_owned();
    let mut a = Argv::default();
    a.flag(&COMMON).flag(&["-i"]).push(input);
    match codec {
        VideoCodec::WebP => {
            a.flag(&["-c:v", "libwebp_anim", "-loop", "0", "-lossless", "0", "-q:v", "75", "-f", "webp"]);
        }
        _ => {
            a.flag(&["-plays", "0", "-f", "apng"]);
        }
    }
    a.push(output);
    a.finish()
}

/// The first GIF run: one palette for the whole clip, written as a single-frame PNG. `palettegen`
/// keeps only a histogram, so the clip is not held in memory.
pub fn palette_args(input: &Path, fps: u32, output: &Path) -> Vec<String> {
    let mut a = Argv::default();
    a.flag(&COMMON).flag(&["-i"]).push(input.to_string_lossy().into_owned());
    a.opt("-vf", format!("fps={fps},palettegen=stats_mode=diff"));
    a.flag(&["-update", "1"]).push(output.to_string_lossy().into_owned());
    a.finish()
}

/// The second GIF run: maps `input` through the palette in `palette`. One graph with `split`
/// feeding both `palettegen` and `paletteuse` would buffer every frame (about 9 MB each at
/// 1920x1188); two runs keep the memory flat. The container is named, so `output` may carry any
/// extension.
pub fn paletteuse_args(input: &Path, palette: &Path, fps: u32, output: &Path) -> Vec<String> {
    let mut a = Argv::default();
    a.flag(&COMMON).flag(&["-i"]).push(input.to_string_lossy().into_owned());
    a.flag(&["-i"]).push(palette.to_string_lossy().into_owned());
    a.opt("-filter_complex", format!("[0:v]fps={fps}[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=5"));
    a.flag(&["-f", "gif"]).push(output.to_string_lossy().into_owned());
    a.finish()
}

/// Joins the segments in `list` (a file of `file '<name>'` lines in the same folder) into `output`.
/// `format` names the container when it is not the extension of `output`.
pub fn concat_args(list: &str, output: &str, format: Option<&str>) -> Vec<String> {
    let mut a = Argv::default();
    a.flag(&COMMON).flag(&["-f", "concat", "-safe", "0", "-i"]).push(list).flag(&["-c", "copy"]);
    if let Some(format) = format {
        a.flag(&["-f", format]);
    }
    a.push(output);
    a.finish()
}

/// The concat demuxer list for `names`, which are bare file names next to the list file. A quote
/// in a name is escaped the shell-free way the concat demuxer expects.
pub fn concat_list(names: &[String]) -> String {
    names.iter().map(|name| format!("file '{}'\n", name.replace('\'', r"'\''"))).collect()
}

/// The `ffprobe` command that counts the video packets of a file. It demuxes and does not decode,
/// and it works on a killed Matroska file, which has no duration element.
pub fn packet_count_args(input: &Path) -> Vec<String> {
    let mut a = Argv::default();
    a.flag(&["-v", "error", "-count_packets", "-select_streams", "v:0", "-show_entries", "stream=nb_read_packets", "-of", "csv=p=0"]);
    a.push(input.to_string_lossy().into_owned());
    a.finish()
}

/// The `ffprobe` command that prints the container and stream facts as JSON.
pub fn probe_args(input: &Path) -> Vec<String> {
    let mut a = Argv::default();
    a.flag(&["-v", "error", "-print_format", "json", "-show_format", "-show_streams"]);
    a.push(input.to_string_lossy().into_owned());
    a.finish()
}

/// Splits a user-written argument string the way a shell would, without running one. Words are
/// separated by whitespace. Single quotes keep everything literal. Double quotes allow `\"` and
/// `\\`. Outside quotes, a backslash escapes a quote, a backslash or whitespace, and is literal
/// otherwise, so Windows paths keep their backslashes.
pub fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            c if c.is_whitespace() => {
                if in_word {
                    args.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    current.push(c);
                }
            }
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.peek().copied() {
                            Some(next @ ('"' | '\\')) => {
                                chars.next();
                                current.push(next);
                            }
                            _ => current.push('\\'),
                        },
                        other => current.push(other),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.peek().copied() {
                    Some(next) if next.is_whitespace() || next == '"' || next == '\'' || next == '\\' => {
                        chars.next();
                        current.push(next);
                    }
                    _ => current.push('\\'),
                }
            }
            other => {
                in_word = true;
                current.push(other);
            }
        }
    }
    if in_word {
        args.push(current);
    }
    args
}

/// Accumulates an argument list.
#[derive(Default)]
struct Argv(Vec<String>);

impl Argv {
    fn flag(&mut self, items: &[&str]) -> &mut Self {
        self.0.extend(items.iter().map(|item| (*item).to_owned()));
        self
    }

    fn opt(&mut self, key: &str, value: impl std::fmt::Display) -> &mut Self {
        self.0.push(key.to_owned());
        self.0.push(value.to_string());
        self
    }

    fn push(&mut self, value: impl Into<String>) -> &mut Self {
        self.0.push(value.into());
        self
    }

    fn finish(self) -> Vec<String> {
        self.0
    }
}

/// True when stdin carries an input (raw frames, or PCM through `-`). Such a file ends by EOF.
pub fn stdin_fed(video: &VideoInputDesc, audio: &[AudioInputDesc]) -> bool {
    matches!(video, VideoInputDesc::RawFrames { .. })
        || audio.iter().any(|input| matches!(input, AudioInputDesc::Pcm { pipe, .. } if pipe == "-"))
}

/// Options that must come before the inputs: the D3D11 device for `ddagrab`, and the VAAPI device.
fn device_options(a: &mut Argv, codec: VideoCodec, video: &VideoInputDesc, capture: WindowsCapture) {
    if let (VideoInputDesc::Target(OsScreen::Windows { adapter_index, .. }), WindowsCapture::Ddagrab) = (video, capture) {
        a.opt("-init_hw_device", format!("d3d11va=dx:{adapter_index}")).flag(&["-filter_hw_device", "dx"]);
    }
    if codec == VideoCodec::Vaapi {
        a.flag(&["-vaapi_device", VAAPI_DEVICE]);
    }
}

fn video_input(a: &mut Argv, spec: &RecordSpec, video: &VideoInputDesc, capture: WindowsCapture) {
    let cursor = if spec.show_cursor { "1" } else { "0" };
    let fps = spec.fps;
    match video {
        VideoInputDesc::Target(OsScreen::Windows { output_index, offset_x, offset_y, width, height, desktop_x, desktop_y, .. }) => {
            match capture {
                WindowsCapture::Ddagrab => {
                    a.flag(&["-f", "lavfi", "-i"]).push(format!(
                    "ddagrab=output_idx={output_index}:framerate={fps}:offset_x={offset_x}:offset_y={offset_y}:video_size={width}x{height}:draw_mouse={cursor}"
                ));
                }
                WindowsCapture::Gdigrab => {
                    a.flag(&["-f", "gdigrab"])
                        .opt("-framerate", fps)
                        .opt("-offset_x", desktop_x)
                        .opt("-offset_y", desktop_y)
                        .opt("-video_size", format!("{width}x{height}"))
                        .opt("-draw_mouse", cursor)
                        .flag(&["-i", "desktop"]);
                }
            }
        }
        VideoInputDesc::Target(OsScreen::X11 { display, x, y, width, height }) => {
            a.flag(&["-f", "x11grab"])
                .opt("-framerate", fps)
                .opt("-video_size", format!("{width}x{height}"))
                .opt("-draw_mouse", cursor)
                .flag(&["-i"])
                .push(format!("{display}+{x},{y}"));
        }
        VideoInputDesc::RawFrames { width, height, fps: raw_fps } => {
            a.flag(&["-f", "rawvideo", "-pix_fmt", "bgra"]).opt("-s", format!("{width}x{height}")).opt("-r", raw_fps).flag(&["-i", "-"]);
        }
    }
}

fn audio_input(a: &mut Argv, host: Host, input: &AudioInputDesc) {
    match input {
        AudioInputDesc::Device { id, name, .. } => match host {
            Host::Windows => {
                a.flag(&["-f", "dshow", "-i"]).push(format!("audio={name}"));
            }
            Host::Linux => {
                a.flag(&["-f", "pulse", "-i"]).push(id.as_str());
            }
        },
        AudioInputDesc::Pcm { rate, channels, pipe } => {
            a.flag(&["-f", "s16le"]).opt("-ar", rate).opt("-ac", channels).flag(&["-i"]).push(pipe.as_str());
        }
    }
}

/// Filters that bring a frame from the OS capture to the encoder.
fn input_filters(video: &VideoInputDesc, capture: WindowsCapture) -> Vec<&'static str> {
    match (video, capture) {
        (VideoInputDesc::Target(OsScreen::Windows { .. }), WindowsCapture::Ddagrab) => vec!["hwdownload", "format=bgra"],
        _ => Vec::new(),
    }
}

/// Filters that the encoder needs. VAAPI takes NV12 uploaded to the device.
fn codec_filters(codec: VideoCodec) -> Vec<&'static str> {
    if codec == VideoCodec::Vaapi { vec!["format=nv12", "hwupload"] } else { Vec::new() }
}

/// The camera device FFmpeg opens: the setting, or the first V4L2 device on Linux. Windows needs a
/// DirectShow name, so an empty setting there means no camera.
fn camera_device(host: Host, cam: &laqta_domain::record::CameraOverlay) -> Option<String> {
    match (host, cam.device.trim()) {
        (Host::Linux, "") => Some("/dev/video0".into()),
        (Host::Windows, "") => None,
        (_, name) => Some(name.to_owned()),
    }
}

/// The camera input, after the screen and the audio inputs (so audio keeps indices 1..).
fn camera_input(a: &mut Argv, host: Host, cam: &laqta_domain::record::CameraOverlay) {
    let device = camera_device(host, cam).unwrap_or_default();
    a.flag(&["-thread_queue_size", "512"]);
    match host {
        Host::Linux => {
            a.flag(&["-f", "v4l2", "-i"]).push(device);
        }
        Host::Windows => {
            a.flag(&["-f", "dshow", "-i"]).push(format!("video={device}"));
        }
    }
}

/// The video graph with the camera: the screen through its input filters, the camera scaled to
/// `size_percent` of the recording's width and masked to its shape, overlaid at the corner, then the
/// codec's filters. The result is `[vout]`.
fn camera_graph(spec: &RecordSpec, cam: &laqta_domain::record::CameraOverlay, index: usize, input: &[&str], codec: &[&str]) -> String {
    use laqta_domain::record::{CameraShape, Corner};
    let width = (spec.area.width * u32::from(cam.size_percent.clamp(5, 100)) / 100).max(16) & !1;
    let base = if input.is_empty() { "null".to_owned() } else { input.join(",") };
    let shape = match cam.shape {
        CameraShape::Circle => format!(
            "crop='min(iw,ih)':'min(iw,ih)',scale={width}:{width},format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='if(lte(hypot(X-W/2,Y-H/2),W/2),255,0)'"
        ),
        CameraShape::RoundedRectangle => format!(
            "scale={width}:-2,format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='if(gt(abs(X-W/2),W/2-W/8)*gt(abs(Y-H/2),H/2-W/8)*gt(hypot(abs(X-W/2)-(W/2-W/8),abs(Y-H/2)-(H/2-W/8)),W/8),0,255)'"
        ),
        CameraShape::Rectangle => format!("scale={width}:-2"),
    };
    let m = cam.margin;
    let (x, y) = match cam.corner {
        Corner::TopLeft => (format!("{m}"), format!("{m}")),
        Corner::TopRight => (format!("main_w-overlay_w-{m}"), format!("{m}")),
        Corner::BottomLeft => (format!("{m}"), format!("main_h-overlay_h-{m}")),
        Corner::BottomRight => (format!("main_w-overlay_w-{m}"), format!("main_h-overlay_h-{m}")),
    };
    let tail = if codec.is_empty() { String::new() } else { format!(",{}", codec.join(",")) };
    format!("[0:v]{base}[base];[{index}:v]{shape}[cam];[base][cam]overlay={x}:{y}:shortest=1{tail}[vout]")
}

/// Maps the video, then the audio: one input as is, several through `amix`. A stdin input ends
/// `amix` with the shortest input, so a stopped PCM stream ends the file.
fn map_audio(a: &mut Argv, count: usize, stdin_fed: bool) {
    match count {
        0 => {}
        1 => {
            a.flag(&["-map", "1:a"]);
        }
        _ => {
            let labels: String = (1..=count).map(|index| format!("[{index}:a]")).collect();
            let shortest = if stdin_fed { ":duration=shortest" } else { "" };
            a.flag(&["-filter_complex"]).push(format!("{labels}amix=inputs={count}{shortest}[aout]")).flag(&["-map", "[aout]"]);
        }
    }
}

/// The encoder and its quality. Software encoders also set `yuv420p`. Hardware encoders take the
/// frame format they are given. GIF, WebP and APNG record a lossless H.264 intermediate.
fn encoder(a: &mut Argv, codec: VideoCodec, quality: Quality) {
    match codec {
        VideoCodec::X264 => {
            a.flag(&["-c:v", "libx264", "-preset", "veryfast"]);
            crf_or_bitrate(a, quality, "-crf");
            a.flag(&["-pix_fmt", "yuv420p"]);
        }
        VideoCodec::X265 => {
            a.flag(&["-c:v", "libx265", "-preset", "veryfast"]);
            crf_or_bitrate(a, quality, "-crf");
            a.flag(&["-pix_fmt", "yuv420p"]);
        }
        VideoCodec::Vp8 => {
            a.flag(&["-c:v", "libvpx"]);
            vpx_quality(a, quality);
            a.flag(&["-deadline", "realtime", "-pix_fmt", "yuv420p"]);
        }
        VideoCodec::Vp9 => {
            a.flag(&["-c:v", "libvpx-vp9", "-row-mt", "1"]);
            vpx_quality(a, quality);
            a.flag(&["-deadline", "realtime", "-pix_fmt", "yuv420p"]);
        }
        VideoCodec::Av1 => {
            a.flag(&["-c:v", "libsvtav1", "-preset", "10"]);
            crf_or_bitrate(a, quality, "-crf");
            a.flag(&["-pix_fmt", "yuv420p"]);
        }
        VideoCodec::Xvid => {
            a.flag(&["-c:v", "mpeg4", "-vtag", "xvid"]);
            xvid_quality(a, quality);
            a.flag(&["-pix_fmt", "yuv420p"]);
        }
        VideoCodec::Nvenc => {
            a.flag(&["-c:v", "h264_nvenc", "-preset", "p4"]);
            crf_or_bitrate(a, quality, "-cq");
        }
        VideoCodec::NvencHevc => {
            a.flag(&["-c:v", "hevc_nvenc", "-preset", "p4"]);
            crf_or_bitrate(a, quality, "-cq");
        }
        VideoCodec::Amf => {
            a.flag(&["-c:v", "h264_amf"]);
            crf_or_bitrate(a, quality, "-qp");
        }
        VideoCodec::Qsv => {
            a.flag(&["-c:v", "h264_qsv"]);
            crf_or_bitrate(a, quality, "-qp");
        }
        VideoCodec::Vaapi => {
            a.flag(&["-c:v", "h264_vaapi"]);
            crf_or_bitrate(a, quality, "-qp");
        }
        VideoCodec::Gif | VideoCodec::WebP | VideoCodec::Apng => {
            a.flag(&["-c:v", "libx264rgb", "-crf", "0", "-preset", "ultrafast"]);
        }
    }
}

fn audio_encoder(a: &mut Argv, codec: VideoCodec) {
    if matches!(codec, VideoCodec::Vp8 | VideoCodec::Vp9) {
        a.flag(&["-c:a", "libopus", "-b:a", AUDIO_BITRATE]);
    } else {
        a.flag(&["-c:a", "aac", "-b:a", AUDIO_BITRATE]);
    }
}

/// `Crf` goes to `flag` (`-crf`, `-cq` or `-qp`). `Bitrate` is `-b:v` in kbit/s. `Default` adds
/// nothing and keeps the encoder's own default.
fn crf_or_bitrate(a: &mut Argv, quality: Quality, flag: &str) {
    match quality {
        Quality::Crf(value) => {
            a.opt(flag, value);
        }
        Quality::Bitrate(kbps) => {
            a.opt("-b:v", format!("{kbps}k"));
        }
        Quality::Default => {}
    }
}

/// libvpx needs `-b:v 0` for constant quality. Without a set quality it uses CRF 30.
fn vpx_quality(a: &mut Argv, quality: Quality) {
    match quality {
        Quality::Crf(value) => {
            a.opt("-crf", value).flag(&["-b:v", "0"]);
        }
        Quality::Bitrate(kbps) => {
            a.opt("-b:v", format!("{kbps}k"));
        }
        Quality::Default => {
            a.flag(&["-b:v", "0"]).opt("-crf", VPX_DEFAULT_CRF);
        }
    }
}

/// MPEG-4 part 2 has no CRF. `Crf(n)` maps to `-q:v`, clamped to the encoder's 1 to 31 scale.
fn xvid_quality(a: &mut Argv, quality: Quality) {
    match quality {
        Quality::Crf(value) => {
            a.opt("-q:v", value.clamp(1, 31));
        }
        Quality::Bitrate(kbps) => {
            a.opt("-b:v", format!("{kbps}k"));
        }
        Quality::Default => {}
    }
}

fn animation_transcode(a: &mut Argv, codec: VideoCodec, spec: &TranscodeSpec) {
    match codec {
        VideoCodec::Gif => {
            let fps = spec.fps.unwrap_or(15);
            a.flag(&["-filter_complex"]).push(gif_filter(spec.width, fps)).flag(&["-an"]);
        }
        VideoCodec::WebP => {
            if let Some(width) = spec.width {
                a.opt("-vf", scale(width));
            }
            a.flag(&["-c:v", "libwebp_anim", "-loop", "0", "-lossless", "0", "-q:v", "75", "-an"]);
        }
        _ => {
            if let Some(width) = spec.width {
                a.opt("-vf", scale(width));
            }
            a.flag(&["-plays", "0", "-an", "-f", "apng"]);
        }
    }
}

/// One palette for the whole clip in a single graph. The `split` buffers every frame, so the
/// `Media` adapter runs GIFs as two runs instead (`transcode_palette_args`, `palette_args`).
fn gif_filter(width: Option<u32>, fps: u32) -> String {
    let scale_part = width.map(|w| format!("{},", scale(w))).unwrap_or_default();
    format!("{scale_part}fps={fps},split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=5")
}

fn scale(width: u32) -> String {
    format!("scale={width}:-2")
}

fn seconds(value: f64) -> String {
    format!("{value:.3}")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use laqta_domain::geometry::PhysRect;
    use laqta_ports::recording::OsScreenRef;

    use super::*;

    fn spec(codec: VideoCodec, quality: Quality) -> RecordSpec {
        RecordSpec {
            area: PhysRect { x: 0, y: 0, width: 640, height: 480 },
            codec,
            fps: 30,
            quality,
            show_cursor: true,
            audio: Vec::new(),
            custom_args: String::new(),
            camera: None,
        }
    }

    fn windows_target() -> VideoInputDesc {
        let screen = OsScreenRef::new(OsScreen::Windows {
            adapter_index: 1,
            output_index: 2,
            offset_x: 10,
            offset_y: 20,
            width: 640,
            height: 480,
            desktop_x: 1930,
            desktop_y: -50,
        });
        VideoInputDesc::Target(screen.screen().clone())
    }

    fn x11_target() -> VideoInputDesc {
        VideoInputDesc::Target(OsScreen::X11 { display: ":0".into(), x: 100, y: 200, width: 1920, height: 1080 })
    }

    fn raw() -> VideoInputDesc {
        VideoInputDesc::RawFrames { width: 64, height: 48, fps: 30 }
    }

    fn device(id: &str, name: &str) -> AudioInputDesc {
        AudioInputDesc::Device { id: id.into(), name: name.into(), loopback: false }
    }

    fn pcm() -> AudioInputDesc {
        AudioInputDesc::Pcm { rate: 48_000, channels: 2, pipe: "-".into() }
    }

    fn out() -> PathBuf {
        PathBuf::from("/work/seg-0001.mkv")
    }

    /// True when `expected` appears as consecutive arguments.
    fn seq(args: &[String], expected: &[&str]) -> bool {
        args.windows(expected.len()).any(|window| window.iter().zip(expected).all(|(got, want)| got.as_str() == *want))
    }

    fn record(host: Host, capture: WindowsCapture, s: &RecordSpec, video: &VideoInputDesc, audio: &[AudioInputDesc]) -> Vec<String> {
        record_args_on(host, capture, s, video, audio, &out())
    }

    fn quality_of(codec: VideoCodec, quality: Quality) -> Vec<String> {
        record(Host::Linux, WindowsCapture::Ddagrab, &spec(codec, quality), &x11_target(), &[])
    }

    #[test]
    fn camera_overlay_adds_an_input_and_a_graph() {
        use laqta_domain::record::{CameraOverlay, CameraShape, Corner};
        let mut s = spec(VideoCodec::X264, Quality::Crf(23));
        s.camera = Some(CameraOverlay {
            enabled: true,
            device: String::new(),
            shape: CameraShape::Circle,
            corner: Corner::BottomRight,
            size_percent: 25,
            margin: 16,
        });
        let video = VideoInputDesc::RawFrames { width: 64, height: 48, fps: 30 };
        let args = record_args_on(Host::Linux, WindowsCapture::Ddagrab, &s, &video, &[], Path::new("/o.mkv"));
        assert!(seq(&args, &["-thread_queue_size", "512", "-f", "v4l2", "-i", "/dev/video0"]));
        let graph = args.iter().skip_while(|a| *a != "-filter_complex").nth(1).expect("a filter graph");
        assert!(graph.starts_with("[0:v]null[base];[1:v]crop="), "{graph}");
        assert!(graph.ends_with("[base][cam]overlay=main_w-overlay_w-16:main_h-overlay_h-16:shortest=1[vout]"), "{graph}");
        assert!(seq(&args, &["-map", "[vout]"]));
        assert!(!args.iter().any(|a| a == "-vf"));
        // Windows needs a device name; without one there is no camera.
        let none = record_args_on(Host::Windows, WindowsCapture::Ddagrab, &s, &video, &[], Path::new("/o.mkv"));
        assert!(!none.iter().any(|a| a == "dshow" || a.starts_with("video=")));
    }

    #[test]
    fn starts_with_common_flags_and_ends_with_segment_output() {
        let args = record(Host::Linux, WindowsCapture::Ddagrab, &spec(VideoCodec::X264, Quality::Default), &x11_target(), &[]);
        assert_eq!(args[..5], COMMON.map(String::from));
        assert!(seq(&args, &["-f", "matroska", "-flush_packets", "1", "-cluster_time_limit", "500", "/work/seg-0001.mkv"]));
        assert_eq!(args.last().map(String::as_str), Some("/work/seg-0001.mkv"));
    }

    #[test]
    fn ddagrab_targets_adapter_and_output_with_download_filter() {
        let args = record(Host::Windows, WindowsCapture::Ddagrab, &spec(VideoCodec::X264, Quality::Default), &windows_target(), &[]);
        assert!(seq(&args, &["-init_hw_device", "d3d11va=dx:1", "-filter_hw_device", "dx"]));
        assert!(seq(
            &args,
            &["-f", "lavfi", "-i", "ddagrab=output_idx=2:framerate=30:offset_x=10:offset_y=20:video_size=640x480:draw_mouse=1"]
        ));
        assert!(seq(&args, &["-vf", "hwdownload,format=bgra"]));
        // The device setup comes before the first input.
        let first_input = args.iter().position(|arg| arg == "-i").unwrap_or(usize::MAX);
        let device = args.iter().position(|arg| arg == "-init_hw_device").unwrap_or(usize::MAX);
        assert!(device < first_input);
    }

    #[test]
    fn ddagrab_hides_cursor_when_asked() {
        let mut s = spec(VideoCodec::X264, Quality::Default);
        s.show_cursor = false;
        let args = record(Host::Windows, WindowsCapture::Ddagrab, &s, &windows_target(), &[]);
        assert!(args.iter().any(|arg| arg.ends_with("draw_mouse=0")));
    }

    #[test]
    fn gdigrab_fallback_uses_desktop_offsets_and_no_download_filter() {
        let args = record(Host::Windows, WindowsCapture::Gdigrab, &spec(VideoCodec::X264, Quality::Default), &windows_target(), &[]);
        assert!(seq(
            &args,
            &[
                "-f",
                "gdigrab",
                "-framerate",
                "30",
                "-offset_x",
                "1930",
                "-offset_y",
                "-50",
                "-video_size",
                "640x480",
                "-draw_mouse",
                "1",
                "-i",
                "desktop"
            ]
        ));
        assert!(!args.iter().any(|arg| arg == "-init_hw_device"));
        assert!(!args.iter().any(|arg| arg == "-vf"));
    }

    #[test]
    fn x11grab_target_takes_display_and_offset() {
        let args = record(Host::Linux, WindowsCapture::Ddagrab, &spec(VideoCodec::X264, Quality::Default), &x11_target(), &[]);
        assert!(seq(&args, &["-f", "x11grab", "-framerate", "30", "-video_size", "1920x1080", "-draw_mouse", "1", "-i", ":0+100,200"]));
        assert!(!args.iter().any(|arg| arg == "-vf"));
    }

    #[test]
    fn raw_frames_read_bgra_from_stdin() {
        let args = record(Host::Linux, WindowsCapture::Ddagrab, &spec(VideoCodec::X264, Quality::Default), &raw(), &[]);
        assert!(seq(&args, &["-f", "rawvideo", "-pix_fmt", "bgra", "-s", "64x48", "-r", "30", "-i", "-"]));
        assert!(!args.iter().any(|arg| arg == "-shortest"));
    }

    #[test]
    fn stdin_input_with_audio_adds_shortest() {
        let args =
            record(Host::Linux, WindowsCapture::Ddagrab, &spec(VideoCodec::X264, Quality::Default), &raw(), &[device("mon", "Monitor")]);
        assert!(args.iter().any(|arg| arg == "-shortest"));
    }

    #[test]
    fn dshow_takes_the_device_name_on_windows() {
        let args = record(
            Host::Windows,
            WindowsCapture::Ddagrab,
            &spec(VideoCodec::X264, Quality::Default),
            &windows_target(),
            &[device("{0.0.1}", "Microphone (Realtek)")],
        );
        assert!(seq(&args, &["-f", "dshow", "-i", "audio=Microphone (Realtek)"]));
        assert!(seq(&args, &["-map", "1:a"]));
    }

    #[test]
    fn pulse_takes_the_source_id_on_linux() {
        let args = record(
            Host::Linux,
            WindowsCapture::Ddagrab,
            &spec(VideoCodec::X264, Quality::Default),
            &x11_target(),
            &[device("alsa_output.x.monitor", "Monitor")],
        );
        assert!(seq(&args, &["-f", "pulse", "-i", "alsa_output.x.monitor"]));
        assert!(seq(&args, &["-c:a", "aac", "-b:a", "192k"]));
    }

    #[test]
    fn pcm_on_stdin_with_target_video_uses_shortest() {
        let args = record(Host::Windows, WindowsCapture::Ddagrab, &spec(VideoCodec::X264, Quality::Default), &windows_target(), &[pcm()]);
        assert!(seq(&args, &["-f", "s16le", "-ar", "48000", "-ac", "2", "-i", "-"]));
        assert!(seq(&args, &["-map", "1:a"]));
        assert!(args.iter().any(|arg| arg == "-shortest"));
    }

    #[test]
    fn several_audio_inputs_are_mixed_and_shortest_follows_stdin() {
        let plain = record(
            Host::Linux,
            WindowsCapture::Ddagrab,
            &spec(VideoCodec::X264, Quality::Default),
            &x11_target(),
            &[device("a", "A"), device("b", "B")],
        );
        assert!(seq(&plain, &["-filter_complex", "[1:a][2:a]amix=inputs=2[aout]", "-map", "[aout]"]));
        assert!(!plain.iter().any(|arg| arg.contains("duration=shortest")));

        let with_pcm = record(
            Host::Windows,
            WindowsCapture::Ddagrab,
            &spec(VideoCodec::X264, Quality::Default),
            &windows_target(),
            &[pcm(), device("m", "Mic")],
        );
        assert!(seq(&with_pcm, &["-filter_complex", "[1:a][2:a]amix=inputs=2:duration=shortest[aout]", "-map", "[aout]"]));
    }

    #[test]
    fn x264_and_x265_use_crf_and_yuv420p() {
        let x264 = quality_of(VideoCodec::X264, Quality::Crf(20));
        assert!(seq(&x264, &["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p"]));
        let x265 = quality_of(VideoCodec::X265, Quality::Bitrate(4000));
        assert!(seq(&x265, &["-c:v", "libx265", "-preset", "veryfast", "-b:v", "4000k", "-pix_fmt", "yuv420p"]));
    }

    #[test]
    fn vp8_and_vp9_use_libvpx_with_realtime_deadline() {
        let vp8 = quality_of(VideoCodec::Vp8, Quality::Crf(10));
        assert!(seq(&vp8, &["-c:v", "libvpx", "-crf", "10", "-b:v", "0", "-deadline", "realtime"]));
        let vp9 = quality_of(VideoCodec::Vp9, Quality::Default);
        assert!(seq(&vp9, &["-c:v", "libvpx-vp9", "-row-mt", "1", "-b:v", "0", "-crf", "30", "-deadline", "realtime"]));
        let vp9_bitrate = quality_of(VideoCodec::Vp9, Quality::Bitrate(2000));
        assert!(seq(&vp9_bitrate, &["-b:v", "2000k"]));
    }

    #[test]
    fn webm_codecs_take_opus_audio() {
        let s = spec(VideoCodec::Vp9, Quality::Default);
        let args = record(Host::Linux, WindowsCapture::Ddagrab, &s, &x11_target(), &[device("a", "A")]);
        assert!(seq(&args, &["-c:a", "libopus", "-b:a", "192k"]));
    }

    #[test]
    fn av1_uses_svt_preset_and_crf() {
        let args = quality_of(VideoCodec::Av1, Quality::Crf(35));
        assert!(seq(&args, &["-c:v", "libsvtav1", "-preset", "10", "-crf", "35", "-pix_fmt", "yuv420p"]));
    }

    #[test]
    fn xvid_is_mpeg4_with_xvid_tag_and_maps_crf_to_qscale() {
        let args = quality_of(VideoCodec::Xvid, Quality::Crf(99));
        assert!(seq(&args, &["-c:v", "mpeg4", "-vtag", "xvid", "-q:v", "31", "-pix_fmt", "yuv420p"]));
        let default = quality_of(VideoCodec::Xvid, Quality::Default);
        assert!(seq(&default, &["-c:v", "mpeg4", "-vtag", "xvid", "-pix_fmt", "yuv420p"]));
    }

    #[test]
    fn hardware_encoders_use_their_quality_flag() {
        let nvenc = quality_of(VideoCodec::Nvenc, Quality::Crf(23));
        assert!(seq(&nvenc, &["-c:v", "h264_nvenc", "-preset", "p4", "-cq", "23"]));
        let hevc = quality_of(VideoCodec::NvencHevc, Quality::Bitrate(8000));
        assert!(seq(&hevc, &["-c:v", "hevc_nvenc", "-preset", "p4", "-b:v", "8000k"]));
        let amf = quality_of(VideoCodec::Amf, Quality::Crf(22));
        assert!(seq(&amf, &["-c:v", "h264_amf", "-qp", "22"]));
        let qsv = quality_of(VideoCodec::Qsv, Quality::Crf(24));
        assert!(seq(&qsv, &["-c:v", "h264_qsv", "-qp", "24"]));
        assert!(!amf.iter().any(|arg| arg == "-pix_fmt"));
    }

    #[test]
    fn vaapi_sets_device_nv12_upload_and_qp() {
        let s = spec(VideoCodec::Vaapi, Quality::Crf(25));
        let args = record(Host::Linux, WindowsCapture::Ddagrab, &s, &x11_target(), &[]);
        assert!(seq(&args, &["-vaapi_device", "/dev/dri/renderD128"]));
        assert!(seq(&args, &["-vf", "format=nv12,hwupload"]));
        assert!(seq(&args, &["-c:v", "h264_vaapi", "-qp", "25"]));
        let device = args.iter().position(|arg| arg == "-vaapi_device").unwrap_or(usize::MAX);
        let first_input = args.iter().position(|arg| arg == "-i").unwrap_or(usize::MAX);
        assert!(device < first_input);
    }

    #[test]
    fn animations_record_lossless_without_sound() {
        for codec in [VideoCodec::Gif, VideoCodec::WebP, VideoCodec::Apng] {
            let args = record(
                Host::Windows,
                WindowsCapture::Ddagrab,
                &spec(codec, Quality::Crf(18)),
                &windows_target(),
                &[device("m", "Mic"), pcm()],
            );
            assert!(seq(&args, &["-c:v", "libx264rgb", "-crf", "0", "-preset", "ultrafast"]), "{codec:?}");
            assert!(!args.iter().any(|arg| arg == "-c:a" || arg == "-shortest"), "{codec:?}");
            assert!(!args.iter().any(|arg| arg == "dshow"), "{codec:?}");
            assert!(!args.iter().any(|arg| arg.contains("amix")), "{codec:?}");
        }
    }

    #[test]
    fn custom_args_go_before_the_segment_output() {
        let mut s = spec(VideoCodec::X264, Quality::Default);
        s.custom_args = "-tune film -x264-params 'a=b c' \"d e\"".into();
        let args = record(Host::Linux, WindowsCapture::Ddagrab, &s, &x11_target(), &[]);
        assert!(seq(&args, &["-tune", "film", "-x264-params", "a=b c", "d e"]));
        let custom = args.iter().position(|arg| arg == "-tune").unwrap_or(usize::MAX);
        let segment = args.iter().rposition(|arg| arg == "matroska").unwrap_or(0);
        assert!(custom < segment);
    }

    #[test]
    fn audio_only_maps_video_and_leaves_audio_unset_when_none() {
        let args = record(Host::Linux, WindowsCapture::Ddagrab, &spec(VideoCodec::X264, Quality::Default), &x11_target(), &[]);
        assert!(seq(&args, &["-map", "0:v"]));
        assert!(!args.iter().any(|arg| arg == "-c:a"));
    }

    #[test]
    fn codec_names_match_the_table() {
        let expected = [
            (VideoCodec::X264, "libx264"),
            (VideoCodec::X265, "libx265"),
            (VideoCodec::Vp8, "libvpx"),
            (VideoCodec::Vp9, "libvpx-vp9"),
            (VideoCodec::Av1, "libsvtav1"),
            (VideoCodec::Xvid, "mpeg4"),
            (VideoCodec::Nvenc, "h264_nvenc"),
            (VideoCodec::NvencHevc, "hevc_nvenc"),
            (VideoCodec::Amf, "h264_amf"),
            (VideoCodec::Qsv, "h264_qsv"),
            (VideoCodec::Vaapi, "h264_vaapi"),
            (VideoCodec::Gif, "libx264rgb"),
            (VideoCodec::WebP, "libx264rgb"),
            (VideoCodec::Apng, "libx264rgb"),
        ];
        assert_eq!(expected.len(), VideoCodec::ALL.len());
        for (codec, encoder) in expected {
            let args = quality_of(codec, Quality::Default);
            assert!(seq(&args, &["-c:v", encoder]), "{codec:?} should use {encoder}");
        }
    }

    #[test]
    fn gif_palette_run_reads_the_clip_once_and_writes_one_png() {
        let args = palette_args(Path::new("/work/merged.mkv"), 15, Path::new("/work/palette.png"));
        assert_eq!(
            args,
            [
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostats",
                "-i",
                "/work/merged.mkv",
                "-vf",
                "fps=15,palettegen=stats_mode=diff",
                "-update",
                "1",
                "/work/palette.png"
            ]
        );
    }

    #[test]
    fn gif_paletteuse_run_maps_the_clip_through_the_palette() {
        let args = paletteuse_args(Path::new("/work/merged.mkv"), Path::new("/work/palette.png"), 15, Path::new("/out/clip.gif.part"));
        assert_eq!(
            args,
            [
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostats",
                "-i",
                "/work/merged.mkv",
                "-i",
                "/work/palette.png",
                "-filter_complex",
                "[0:v]fps=15[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=5",
                "-f",
                "gif",
                "/out/clip.gif.part"
            ]
        );
        // No `split`: it would buffer the whole clip.
        assert!(!args.iter().any(|arg| arg.contains("split")));
    }

    #[test]
    fn webp_and_apng_conversion_args() {
        let webp = animation_args(VideoCodec::WebP, Path::new("/w/m.mkv"), Path::new("/o/a.webp.part"));
        assert!(seq(&webp, &["-c:v", "libwebp_anim", "-loop", "0", "-lossless", "0", "-q:v", "75", "-f", "webp", "/o/a.webp.part"]));
        let apng = animation_args(VideoCodec::Apng, Path::new("/w/m.mkv"), Path::new("/o/a.png.part"));
        assert!(seq(&apng, &["-plays", "0", "-f", "apng", "/o/a.png.part"]));
    }

    #[test]
    fn transcode_trims_scales_and_resamples() {
        let spec = TranscodeSpec {
            codec: Some(VideoCodec::X264),
            quality: Quality::Crf(23),
            fps: Some(15),
            start_secs: Some(1.5),
            duration_secs: Some(2.0),
            width: Some(640),
            single_frame: false,
            extra_args: String::new(),
        };
        let args = transcode_args(&spec, Path::new("/in.mp4"), Path::new("/out.mp4"));
        assert!(seq(&args, &["-ss", "1.500", "-i", "/in.mp4"]));
        assert!(seq(&args, &["-vf", "scale=640:-2"]));
        assert!(seq(&args, &["-c:v", "libx264", "-preset", "veryfast", "-crf", "23", "-pix_fmt", "yuv420p"]));
        assert!(seq(&args, &["-c:a", "aac", "-b:a", "192k"]));
        assert!(seq(&args, &["-r", "15"]));
        assert!(seq(&args, &["-t", "2.000", "/out.mp4"]));
    }

    #[test]
    fn transcode_default_codec_is_x264() {
        let spec = TranscodeSpec::default();
        let args = transcode_args(&spec, Path::new("/in.mp4"), Path::new("/out.mp4"));
        assert!(seq(&args, &["-c:v", "libx264"]));
        assert!(!args.iter().any(|arg| arg == "-ss" || arg == "-t"));
    }

    #[test]
    fn thumbnail_extracts_one_frame_without_encoder() {
        let spec = TranscodeSpec { start_secs: Some(0.5), single_frame: true, width: Some(320), ..TranscodeSpec::default() };
        let args = transcode_args(&spec, Path::new("/in.mp4"), Path::new("/thumb.png"));
        assert!(seq(&args, &["-ss", "0.500", "-i", "/in.mp4"]));
        assert!(seq(&args, &["-vf", "scale=320:-2", "-frames:v", "1", "-q:v", "2", "-an", "/thumb.png"]));
        assert!(!args.iter().any(|arg| arg == "-c:v"));
    }

    #[test]
    fn transcode_gif_uses_palette_and_scale() {
        let spec = TranscodeSpec { codec: Some(VideoCodec::Gif), fps: Some(10), width: Some(480), ..TranscodeSpec::default() };
        let args = transcode_args(&spec, Path::new("/in.mp4"), Path::new("/out.gif"));
        assert!(seq(
            &args,
            &[
                "-filter_complex",
                "scale=480:-2,fps=10,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=5",
                "-an",
                "/out.gif"
            ]
        ));
    }

    #[test]
    fn a_gif_transcode_is_two_runs_with_the_same_trim_and_scale() {
        let spec = TranscodeSpec {
            codec: Some(VideoCodec::Gif),
            fps: Some(10),
            width: Some(480),
            start_secs: Some(1.5),
            duration_secs: Some(3.0),
            ..TranscodeSpec::default()
        };
        let palette = transcode_palette_args(&spec, Path::new("/in.mp4"), Path::new("/p.png"));
        assert!(
            seq(&palette, &["-ss", "1.500", "-i", "/in.mp4", "-t", "3.000", "-vf", "scale=480:-2,fps=10,palettegen=stats_mode=diff"]),
            "{palette:?}"
        );
        let gif = transcode_gif_args(&spec, Path::new("/in.mp4"), Path::new("/p.png"), Path::new("/out.gif"));
        assert!(seq(&gif, &["-ss", "1.500", "-i", "/in.mp4", "-i", "/p.png"]), "{gif:?}");
        assert!(seq(&gif, &["[0:v]scale=480:-2,fps=10[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=5", "-an", "-t", "3.000"]), "{gif:?}");
        assert_eq!(gif.last().map(String::as_str), Some("/out.gif"));
    }

    #[test]
    fn concat_list_quotes_names_for_the_demuxer() {
        let list = concat_list(&["seg-0001.mkv".to_owned(), "it's.mkv".to_owned()]);
        assert_eq!(list, "file 'seg-0001.mkv'\nfile 'it'\\''s.mkv'\n");
    }

    #[test]
    fn concat_copies_and_names_the_container() {
        let args = concat_args("list.txt", "/out/clip.mp4", None);
        assert!(seq(&args, &["-f", "concat", "-safe", "0", "-i", "list.txt", "-c", "copy", "/out/clip.mp4"]));
        let intermediate = concat_args("list.txt", "/work/merged.mkv", Some("matroska"));
        assert!(seq(&intermediate, &["-c", "copy", "-f", "matroska", "/work/merged.mkv"]));
    }

    #[test]
    fn packet_count_demuxes_the_video_stream_only() {
        let args = packet_count_args(Path::new("/seg.mkv"));
        assert!(seq(
            &args,
            &["-count_packets", "-select_streams", "v:0", "-show_entries", "stream=nb_read_packets", "-of", "csv=p=0", "/seg.mkv"]
        ));
        assert!(!args.iter().any(|arg| arg == "-count_frames"));
    }

    #[test]
    fn probe_prints_json_for_the_input() {
        let args = probe_args(Path::new("/clip.mp4"));
        assert_eq!(args.last().map(String::as_str), Some("/clip.mp4"));
        assert!(seq(&args, &["-print_format", "json", "-show_format", "-show_streams"]));
    }

    #[test]
    fn split_args_handles_quotes_and_backslashes() {
        assert_eq!(split_args("-a  'b c' \"d\\\"e\" f\\ g"), ["-a", "b c", "d\"e", "f g"]);
        assert_eq!(split_args(r"-i C:\clips\in.mp4"), ["-i", r"C:\clips\in.mp4"]);
        assert_eq!(split_args("\"\" x"), ["", "x"]);
        assert!(split_args("   ").is_empty());
    }
}
