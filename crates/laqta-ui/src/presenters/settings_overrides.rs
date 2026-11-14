//! Which shortcuts give a settings section values of their own (ADR 008). A page that edits the
//! defaults uses this to say that some shortcuts ignore what it shows. Only a real difference counts:
//! a section copied from the defaults and never changed overrides nothing.

use laqta_domain::{Settings, WorkflowId};

/// A section of the task settings a shortcut can override, as the settings pages group them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    AfterCapture,
    AfterUpload,
    /// Destinations, link format and retries.
    Upload,
    Beautify,
    /// The effect preset applied after capture.
    Effects,
    Recording,
    /// Notifications and sounds.
    General,
    Image,
    /// Capture options (cursor, delay, own windows, region and scrolling options).
    Capture,
    Naming,
    ScreenshotsDir,
}

/// The shortcuts (in list order) whose `section` differs from the defaults the page edits. For
/// `ScreenshotsDir` the comparison is with the folder the shortcut would use without an override.
pub fn overriding(settings: &Settings, section: Section) -> Vec<WorkflowId> {
    let d = &settings.defaults;
    settings
        .workflows
        .iter()
        .filter(|w| {
            let p = &w.patch;
            match section {
                Section::AfterCapture => p.after_capture.is_some_and(|v| v != d.after_capture),
                Section::AfterUpload => p.after_upload.is_some_and(|v| v != d.after_upload),
                Section::Upload => p.upload.as_ref().is_some_and(|v| *v != d.upload),
                Section::Beautify => p.beautify.as_ref().is_some_and(|v| *v != d.beautify),
                Section::Effects => p.effect_preset.as_ref().is_some_and(|v| *v != d.effect_preset),
                Section::Recording => p.recording.as_ref().is_some_and(|v| *v != d.recording),
                Section::General => p.general.as_ref().is_some_and(|v| *v != d.general),
                Section::Image => p.image.as_ref().is_some_and(|v| *v != d.image),
                Section::Capture => p.capture.as_ref().is_some_and(|v| *v != d.capture),
                Section::Naming => p.naming.as_ref().is_some_and(|v| *v != d.naming),
                Section::ScreenshotsDir => {
                    p.screenshots_dir.as_ref().is_some_and(|v| *v != *d.screenshots_dir.as_ref().unwrap_or(&settings.screenshots_dir))
                }
            }
        })
        .map(|w| w.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use laqta_domain::Settings;

    use super::*;

    fn settings() -> Settings {
        Settings::default_for(PathBuf::from("/pics"))
    }

    #[test]
    fn nothing_overrides_in_the_default_settings() {
        let s = settings();
        for section in [Section::General, Section::Capture, Section::ScreenshotsDir, Section::AfterCapture] {
            assert!(overriding(&s, section).is_empty(), "{section:?}");
        }
    }

    #[test]
    fn a_changed_section_names_its_shortcut_and_no_other_section() {
        let mut s = settings();
        let id = s.workflows[2].id;
        let mut general = s.defaults.general.clone();
        general.show_toast = false;
        s.workflows[2].patch.general = Some(general);
        assert_eq!(overriding(&s, Section::General), vec![id]);
        assert!(overriding(&s, Section::Capture).is_empty());
    }

    #[test]
    fn a_section_copied_from_the_defaults_is_not_an_override() {
        let mut s = settings();
        s.workflows[0].patch.capture = Some(s.defaults.capture.clone());
        s.workflows[0].patch.screenshots_dir = Some(s.screenshots_dir.clone());
        s.workflows[1].patch.after_capture = Some(s.defaults.after_capture);
        assert!(overriding(&s, Section::Capture).is_empty());
        assert!(overriding(&s, Section::ScreenshotsDir).is_empty());
        assert!(overriding(&s, Section::AfterCapture).is_empty());
    }

    #[test]
    fn the_screenshots_folder_is_compared_with_the_one_the_shortcut_would_use() {
        let mut s = settings();
        s.workflows[1].patch.screenshots_dir = Some(PathBuf::from("/work"));
        s.workflows[3].patch.screenshots_dir = Some(PathBuf::from("/pics/Laqta"));
        let (a, b) = (s.workflows[1].id, s.workflows[3].id);
        assert_eq!(overriding(&s, Section::ScreenshotsDir), vec![a], "{b:?} saves where the default saves");
        s.defaults.screenshots_dir = Some(PathBuf::from("/work"));
        assert_eq!(overriding(&s, Section::ScreenshotsDir), vec![b], "now the other one differs");
    }

    #[test]
    fn several_shortcuts_come_back_in_list_order() {
        let mut s = settings();
        let mut capture = s.defaults.capture.clone();
        capture.show_cursor = !capture.show_cursor;
        for i in [4, 0, 2] {
            s.workflows[i].patch.capture = Some(capture.clone());
        }
        let ids: Vec<_> = [0, 2, 4].iter().map(|i| s.workflows[*i].id).collect();
        assert_eq!(overriding(&s, Section::Capture), ids);
    }
}
