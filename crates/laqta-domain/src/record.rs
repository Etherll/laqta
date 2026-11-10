//! Recording as data: what to record and how to encode it (GLOSSARY: Record spec).

use crate::geometry::PhysRect;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum VideoCodec {
    #[default]
    X264,
    X265,
    Vp8,
    Vp9,
    Av1,
    Xvid,
    Nvenc,
    NvencHevc,
    Amf,
    Qsv,
    Vaapi,
    Gif,
    WebP,
    Apng,
}

impl VideoCodec {
    pub const ALL: [VideoCodec; 14] = [
        VideoCodec::X264,
        VideoCodec::X265,
        VideoCodec::Vp8,
        VideoCodec::Vp9,
        VideoCodec::Av1,
        VideoCodec::Xvid,
        VideoCodec::Nvenc,
        VideoCodec::NvencHevc,
        VideoCodec::Amf,
        VideoCodec::Qsv,
        VideoCodec::Vaapi,
        VideoCodec::Gif,
        VideoCodec::WebP,
        VideoCodec::Apng,
    ];

    /// The container the finished file uses.
    pub fn extension(self) -> &'static str {
        match self {
            VideoCodec::Vp8 | VideoCodec::Vp9 => "webm",
            VideoCodec::Xvid => "avi",
            VideoCodec::Gif => "gif",
            VideoCodec::WebP => "webp",
            VideoCodec::Apng => "png",
            _ => "mp4",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::X264 => "H.264 (x264)",
            VideoCodec::X265 => "H.265 (x265)",
            VideoCodec::Vp8 => "VP8",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::Av1 => "AV1 (SVT-AV1)",
            VideoCodec::Xvid => "Xvid",
            VideoCodec::Nvenc => "H.264 (NVIDIA NVENC)",
            VideoCodec::NvencHevc => "H.265 (NVIDIA NVENC)",
            VideoCodec::Amf => "H.264 (AMD AMF)",
            VideoCodec::Qsv => "H.264 (Intel Quick Sync)",
            VideoCodec::Vaapi => "H.264 (VA-API)",
            VideoCodec::Gif => "GIF",
            VideoCodec::WebP => "Animated WebP",
            VideoCodec::Apng => "Animated PNG",
        }
    }

    pub fn is_animation(self) -> bool {
        matches!(self, VideoCodec::Gif | VideoCodec::WebP | VideoCodec::Apng)
    }

    /// Whether `os` has this encoder. The OS-specific ones are hardware paths (VA-API, NVENC, AMF,
    /// Quick Sync) that exist on some systems only.
    pub fn available_on(self, os: Os) -> bool {
        !matches!(
            (self, os),
            (VideoCodec::Vaapi, Os::Windows | Os::MacOs)
                | (VideoCodec::Amf | VideoCodec::Qsv | VideoCodec::Nvenc | VideoCodec::NvencHevc, Os::MacOs)
        )
    }

    /// The codecs `os` has, in `ALL` order.
    pub fn available(os: Os) -> impl Iterator<Item = VideoCodec> {
        Self::ALL.into_iter().filter(move |codec| codec.available_on(os))
    }
}

/// The operating systems Laqta builds for. `main` passes the running one in; the core never detects it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    MacOs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Quality {
    /// Constant rate factor (lower = better); used by x264/x265/VP9/AV1.
    Crf(u8),
    /// Target bitrate in kbit/s.
    Bitrate(u32),
    #[default]
    Default,
}

/// An audio device, as the platform lists it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    /// System sound (loopback/monitor) rather than a microphone.
    pub loopback: bool,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RecordingSettings {
    pub codec: VideoCodec,
    pub gif_codec: VideoCodec,
    pub fps: u32,
    pub gif_fps: u32,
    pub quality: Quality,
    pub show_cursor: bool,
    pub record_system_audio: bool,
    pub microphone: Option<String>,
    pub countdown_secs: u32,
    /// Stop automatically after this many seconds (0 = off).
    pub fixed_duration_secs: u32,
    pub show_border: bool,
    /// Extra FFmpeg arguments added before the output (power users).
    pub custom_args: String,
    /// Path to FFmpeg; empty = PATH or the downloaded copy.
    pub ffmpeg_path: String,
    /// Turn the mouse highlighter on while recording.
    pub highlight_mouse: bool,
    /// The camera overlay.
    pub camera: CameraOverlay,
    /// FFmpeg or, on Windows, the native recorder.
    pub backend: RecorderBackend,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RecorderBackend {
    #[default]
    Ffmpeg,
    /// Windows.Graphics.Capture + Media Foundation (Windows only; elsewhere FFmpeg is used).
    Native,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CameraShape {
    #[default]
    Circle,
    RoundedRectangle,
    Rectangle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

/// A camera picture over the recording.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CameraOverlay {
    pub enabled: bool,
    /// The OS device name (`/dev/video0`, a DirectShow name); empty = the first camera.
    pub device: String,
    pub shape: CameraShape,
    pub corner: Corner,
    /// Width as a percentage of the recording's width.
    pub size_percent: u8,
    /// Distance from the edges, in pixels.
    pub margin: u32,
}

impl Default for CameraOverlay {
    fn default() -> Self {
        CameraOverlay {
            enabled: false,
            device: String::new(),
            shape: CameraShape::Circle,
            corner: Corner::BottomRight,
            size_percent: 25,
            margin: 16,
        }
    }
}

impl Default for RecordingSettings {
    fn default() -> Self {
        RecordingSettings {
            codec: VideoCodec::X264,
            gif_codec: VideoCodec::Gif,
            fps: 30,
            gif_fps: 15,
            quality: Quality::Crf(23),
            show_cursor: true,
            record_system_audio: false,
            microphone: None,
            countdown_secs: 0,
            fixed_duration_secs: 0,
            show_border: true,
            custom_args: String::new(),
            ffmpeg_path: String::new(),
            highlight_mouse: false,
            camera: CameraOverlay::default(),
            backend: RecorderBackend::Ffmpeg,
        }
    }
}

/// One recording: the area and how to encode it.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordSpec {
    pub area: PhysRect,
    pub codec: VideoCodec,
    pub fps: u32,
    pub quality: Quality,
    pub show_cursor: bool,
    pub audio: Vec<AudioDevice>,
    pub custom_args: String,
    /// The camera picture over the recording; `None` = off.
    pub camera: Option<CameraOverlay>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        assert_eq!(VideoCodec::Vp9.extension(), "webm");
        assert_eq!(VideoCodec::Gif.extension(), "gif");
        assert!(VideoCodec::Apng.is_animation());
        assert_eq!(VideoCodec::ALL.len(), 14);
    }

    #[test]
    fn windows_has_every_codec_but_vaapi() {
        assert!(!VideoCodec::Vaapi.available_on(Os::Windows));
        assert!(VideoCodec::Nvenc.available_on(Os::Windows));
        let codecs: Vec<_> = VideoCodec::available(Os::Windows).collect();
        assert_eq!(codecs.len(), 13);
        assert!(!codecs.contains(&VideoCodec::Vaapi));
    }

    #[test]
    fn linux_has_every_codec() {
        let codecs: Vec<_> = VideoCodec::available(Os::Linux).collect();
        assert_eq!(codecs, VideoCodec::ALL);
    }

    #[test]
    fn macos_lacks_the_five_hardware_encoders() {
        let gone = [VideoCodec::Vaapi, VideoCodec::Amf, VideoCodec::Qsv, VideoCodec::Nvenc, VideoCodec::NvencHevc];
        for codec in gone {
            assert!(!codec.available_on(Os::MacOs), "{codec:?}");
        }
        let codecs: Vec<_> = VideoCodec::available(Os::MacOs).collect();
        assert_eq!(codecs.len(), 9);
        assert!(codecs.contains(&VideoCodec::X264));
    }
}
