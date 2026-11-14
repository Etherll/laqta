//! The settings window's text fields read back into `Settings`: the inverse of `settings_reset::InputTexts::of`.

use std::fmt;
use std::path::PathBuf;

use laqta_domain::Settings;
use laqta_domain::geometry::PhysRect;
use laqta_domain::record::Quality;
use laqta_domain::settings::{AutoCaptureOptions, limits};

use crate::presenters::settings_numbers::{NumberError, parse_bounded, parse_bounded_or};
use crate::presenters::settings_reset::InputTexts;

/// A field the settings window refuses: its name and what is wrong with its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldError {
    pub field: &'static str,
    pub message: String,
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

/// Writes the window's text into `s`, field by field, the way `InputTexts::of` read them. The first
/// refused field stops the write: the fields before it are already in `s`.
pub fn apply(texts: &InputTexts, s: &mut Settings) -> Result<(), FieldError> {
    if texts.dir.is_empty() {
        return Err(FieldError { field: "Screenshots folder", message: "Choose a folder for screenshots".into() });
    }
    s.screenshots_dir = texts.dir.as_str().into();
    let d = &mut s.defaults;
    d.naming.pattern = texts.pattern.clone();
    d.naming.window_pattern = texts.window_pattern.clone();
    d.naming.subfolder = texts.subfolder.clone();
    d.image.jpeg_quality = bounded(&texts.jpeg_quality, "JPEG quality", limits::JPEG_QUALITY, 90)? as u8;
    let kb = whole(&texts.auto_jpeg, "The JPEG size threshold")?;
    d.image.auto_jpeg_above_bytes = (kb > 0).then_some(kb as u64 * 1024);
    d.image.thumbnail_width = whole(&texts.thumb_w, "Thumbnail width")?;
    d.image.thumbnail_height = whole(&texts.thumb_h, "Thumbnail height")?;
    d.capture.delay_ms = whole(&texts.delay, "Capture delay")?;
    d.capture.custom_window_title = texts.window_title.clone();
    d.capture.custom_region = parse_region(&texts.custom_region).map_err(|message| FieldError { field: "Custom region", message })?;
    d.general.custom_capture_sound = path(&texts.sound_capture);
    d.general.custom_task_sound = path(&texts.sound_task);
    d.general.custom_error_sound = path(&texts.sound_error);
    d.general.toast.duration_ms = bounded(&texts.toast_ms, "Notification time", limits::TOAST_DURATION_MS, 3000)?;
    let r = &mut d.recording;
    r.fps = bounded(&texts.rec_fps, "Frame rate", limits::FPS, 30)?;
    r.gif_fps = bounded(&texts.rec_gif_fps, "GIF frame rate", limits::GIF_FPS, 15)?;
    r.gif_max_width = whole(&texts.rec_gif_max_width, "GIF maximum width")?;
    r.quality = if texts.rec_crf.trim().is_empty() {
        Quality::Default
    } else {
        Quality::Crf(parse_bounded(&texts.rec_crf, limits::CRF.0, limits::CRF.1).map_err(|e| refused("Quality (CRF)", e))? as u8)
    };
    r.countdown_secs = whole(&texts.rec_countdown, "Countdown")?;
    r.fixed_duration_secs = whole(&texts.rec_duration, "Fixed duration")?;
    r.microphone = (!texts.rec_microphone.is_empty()).then(|| texts.rec_microphone.clone());
    r.custom_args = texts.rec_args.clone();
    r.ffmpeg_path = texts.rec_ffmpeg.clone();
    r.camera.device = texts.rec_camera_device.clone();
    r.camera.size_percent = bounded(&texts.rec_camera_size, "Camera size", limits::CAMERA_SIZE_PERCENT, 25)? as u8;
    let o = &mut d.capture.scrolling;
    o.start_delay_ms = whole(&texts.scroll[0], "Start delay")?;
    o.scroll_delay_ms = bounded(&texts.scroll[1], "Scroll delay", limits::SCROLL_DELAY_MS, 300)?;
    o.max_frames = bounded(&texts.scroll[2], "Maximum frames", limits::SCROLL_FRAMES, 50)?;
    o.scroll_amount = bounded(&texts.scroll[3], "Scroll amount", limits::SCROLL_AMOUNT, 2)?;
    o.ignore_top = whole(&texts.scroll[4], "Fixed rows at the top")?;
    o.ignore_bottom = whole(&texts.scroll[5], "Fixed rows at the bottom")?;
    s.cloud.analyzer.base_url = texts.cloud[0].clone();
    s.cloud.analyzer.model = texts.cloud[1].clone();
    s.cloud.analyzer.prompt = texts.cloud[2].clone();
    let (min_s, max_s) = (AutoCaptureOptions::MIN_INTERVAL_MS / 1000, AutoCaptureOptions::MAX_INTERVAL_MS / 1000);
    s.auto_capture.interval_ms = bounded(&texts.auto_interval, "Capture every (seconds)", (min_s, max_s), 60)? * 1000;
    s.auto_capture.count = whole(&texts.auto_count, "Auto-capture count")?;
    d.upload.clipboard_format = texts.clipboard_format.clone();
    d.upload.retries = bounded(&texts.retries, "Retries", limits::RETRIES, 2)?;
    Ok(())
}

/// A whole number from `min` to `max`; an empty field is `empty`.
fn bounded(text: &str, field: &'static str, (min, max): (u32, u32), empty: u32) -> Result<u32, FieldError> {
    parse_bounded_or(text, min, max, empty).map_err(|e| refused(field, e))
}

/// A whole number with no upper limit; an empty field is 0.
fn whole(text: &str, field: &'static str) -> Result<u32, FieldError> {
    bounded(text, field, (0, u32::MAX), 0)
}

fn refused(field: &'static str, e: NumberError) -> FieldError {
    FieldError { field, message: e.to_string() }
}

/// A file typed into the window; an empty field is no file.
fn path(text: &str) -> Option<PathBuf> {
    (!text.is_empty()).then(|| PathBuf::from(text))
}

/// "x, y, width, height" → a rect; empty → None.
pub fn parse_region(text: &str) -> Result<Option<PhysRect>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    let bad = || "Enter four numbers: x, y, width, height".to_string();
    if parts.len() != 4 {
        return Err(bad());
    }
    let x = parts[0].parse::<i32>().map_err(|_| bad())?;
    let y = parts[1].parse::<i32>().map_err(|_| bad())?;
    let width = parts[2].parse::<u32>().map_err(|_| bad())?;
    let height = parts[3].parse::<u32>().map_err(|_| bad())?;
    if width == 0 || height == 0 {
        return Err(bad());
    }
    Ok(Some(PhysRect { x, y, width, height }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A settings value with a non-default text or number in every field the window shows.
    fn every_field_set() -> Settings {
        let mut s = Settings::default_for(PathBuf::from("/home/u/Pictures"));
        s.screenshots_dir = "/home/u/Shots".into();
        let d = &mut s.defaults;
        d.naming.pattern = "{date}-{n}".into();
        d.naming.window_pattern = "{title}".into();
        d.naming.subfolder = "2026".into();
        d.image.jpeg_quality = 75;
        d.image.auto_jpeg_above_bytes = Some(512 * 1024);
        d.image.thumbnail_width = 200;
        d.image.thumbnail_height = 150;
        d.capture.delay_ms = 500;
        d.capture.custom_window_title = "Laqta".into();
        d.capture.custom_region = Some(PhysRect { x: -10, y: 20, width: 300, height: 400 });
        d.general.custom_capture_sound = Some("/s/capture.wav".into());
        d.general.custom_task_sound = Some("/s/task.wav".into());
        d.general.custom_error_sound = Some("/s/error.wav".into());
        d.general.toast.duration_ms = 5000;
        d.upload.clipboard_format = "png".into();
        d.upload.retries = 3;
        let r = &mut d.recording;
        r.fps = 60;
        r.gif_fps = 10;
        r.gif_max_width = 640;
        r.quality = Quality::Crf(20);
        r.countdown_secs = 3;
        r.fixed_duration_secs = 30;
        r.microphone = Some("Mic".into());
        r.custom_args = "-x 1".into();
        r.ffmpeg_path = "/usr/bin/ffmpeg".into();
        r.camera.device = "cam0".into();
        r.camera.size_percent = 30;
        let o = &mut d.capture.scrolling;
        o.start_delay_ms = 100;
        o.scroll_delay_ms = 400;
        o.max_frames = 60;
        o.scroll_amount = 3;
        o.ignore_top = 5;
        o.ignore_bottom = 6;
        s.cloud.analyzer.base_url = "https://ai.example".into();
        s.cloud.analyzer.model = "model-x".into();
        s.cloud.analyzer.prompt = "Describe it".into();
        s.auto_capture.interval_ms = 120_000;
        s.auto_capture.count = 7;
        s
    }

    type Slot = fn(&mut InputTexts) -> &mut String;

    /// Each bounded field: its name in the message, the text it takes, and its bounds.
    fn bounded_fields() -> [(&'static str, Slot, (u32, u32)); 11] {
        [
            ("JPEG quality", |t| &mut t.jpeg_quality, limits::JPEG_QUALITY),
            ("Notification time", |t| &mut t.toast_ms, limits::TOAST_DURATION_MS),
            ("Frame rate", |t| &mut t.rec_fps, limits::FPS),
            ("GIF frame rate", |t| &mut t.rec_gif_fps, limits::GIF_FPS),
            ("Quality (CRF)", |t| &mut t.rec_crf, limits::CRF),
            ("Camera size", |t| &mut t.rec_camera_size, limits::CAMERA_SIZE_PERCENT),
            ("Scroll delay", |t| &mut t.scroll[1], limits::SCROLL_DELAY_MS),
            ("Maximum frames", |t| &mut t.scroll[2], limits::SCROLL_FRAMES),
            ("Scroll amount", |t| &mut t.scroll[3], limits::SCROLL_AMOUNT),
            (
                "Capture every (seconds)",
                |t| &mut t.auto_interval,
                (AutoCaptureOptions::MIN_INTERVAL_MS / 1000, AutoCaptureOptions::MAX_INTERVAL_MS / 1000),
            ),
            ("Retries", |t| &mut t.retries, limits::RETRIES),
        ]
    }

    #[test]
    fn the_text_of_a_setting_reads_back_to_it() {
        let default = Settings::default();
        let mut back = default.clone();
        apply(&InputTexts::of(&default), &mut back).unwrap();
        assert_eq!(back, default);

        let full = every_field_set();
        let mut back = full.clone();
        apply(&InputTexts::of(&full), &mut back).unwrap();
        assert_eq!(back, full);

        let mut fresh = Settings::default();
        apply(&InputTexts::of(&full), &mut fresh).unwrap();
        assert_eq!(fresh, full, "every field the texts carry is written");
    }

    #[test]
    fn a_bounded_field_refuses_a_value_just_outside_its_bounds_by_name() {
        let base = InputTexts::of(&Settings::default());
        for (field, slot, (min, max)) in bounded_fields() {
            let typed = |text: String| {
                let mut texts = base.clone();
                *slot(&mut texts) = text;
                apply(&texts, &mut Settings::default())
            };
            assert!(typed(min.to_string()).is_ok(), "{field} takes its minimum");
            assert!(typed(max.to_string()).is_ok(), "{field} takes its maximum");
            let over = typed((max + 1).to_string()).expect_err(field);
            assert_eq!(over.to_string(), format!("{field}: Must be at most {max}"));
            if let Some(under) = min.checked_sub(1) {
                let err = typed(under.to_string()).expect_err(field);
                assert_eq!(err.to_string(), format!("{field}: Must be at least {min}"));
            }
        }
    }

    #[test]
    fn an_empty_field_takes_its_default() {
        let texts = InputTexts { dir: "/home/u/Shots".into(), ..InputTexts::default() };
        let mut s = Settings::default();
        apply(&texts, &mut s).unwrap();
        let d = &s.defaults;
        assert_eq!(d.image.jpeg_quality, 90);
        assert_eq!(d.general.toast.duration_ms, 3000);
        assert_eq!(d.recording.fps, 30);
        assert_eq!(d.recording.gif_fps, 15);
        assert_eq!(d.recording.quality, Quality::Default);
        assert_eq!(d.recording.camera.size_percent, 25);
        assert_eq!(d.capture.scrolling.scroll_delay_ms, 300);
        assert_eq!(d.capture.scrolling.max_frames, 50);
        assert_eq!(d.capture.scrolling.scroll_amount, 2);
        assert_eq!(s.auto_capture.interval_ms, 60_000);
        assert_eq!(d.upload.retries, 2);
        assert_eq!(d.image.auto_jpeg_above_bytes, None);
        assert_eq!(d.image.thumbnail_width, 0);
        assert_eq!(d.capture.delay_ms, 0);
        assert_eq!(d.recording.countdown_secs, 0);
        assert_eq!(s.auto_capture.count, 0);
        assert_eq!(d.capture.custom_region, None);
        assert_eq!(d.recording.microphone, None);
        assert_eq!(d.general.custom_capture_sound, None);
    }

    #[test]
    fn an_empty_folder_is_refused() {
        let err = apply(&InputTexts::default(), &mut Settings::default()).expect_err("no folder");
        assert_eq!(err.to_string(), "Screenshots folder: Choose a folder for screenshots");
    }

    #[test]
    fn a_custom_region_is_four_numbers_with_a_size() {
        assert_eq!(parse_region(""), Ok(None));
        assert_eq!(parse_region(" 1, -2, 30, 40 "), Ok(Some(PhysRect { x: 1, y: -2, width: 30, height: 40 })));
        for bad in ["1, 2, 3", "1, 2, 0, 4", "a, 2, 3, 4", "1, 2, -3, 4"] {
            assert!(parse_region(bad).is_err(), "{bad}");
        }
    }
}
