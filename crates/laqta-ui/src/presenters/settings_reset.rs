//! "Reset to default" for the settings window: which settings a page owns, how many of them
//! differ from the defaults, and putting them back. User data (accounts, custom uploaders and
//! actions, effect presets, watch folders, custom models, added workflows) is never reset.

use laqta_domain::Settings;
use laqta_domain::workflow::Workflow;
use serde::Serialize;

/// The settings a reset covers: one settings page, or everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    General,
    AfterCapture,
    Image,
    Naming,
    Capture,
    Shortcuts,
    Uploads,
    Recording,
    Automation,
    Models,
    All,
}

const PAGES: [Scope; 10] = [
    Scope::General,
    Scope::AfterCapture,
    Scope::Image,
    Scope::Naming,
    Scope::Capture,
    Scope::Shortcuts,
    Scope::Uploads,
    Scope::Recording,
    Scope::Automation,
    Scope::Models,
];

/// How many settings in `scope` differ from `defaults`.
pub fn changed(scope: Scope, draft: &Settings, defaults: &Settings) -> usize {
    if scope == Scope::All {
        return PAGES.iter().map(|s| changed(*s, draft, defaults)).sum::<usize>()
            + differ(&draft.tray_click, &defaults.tray_click)
            + differ(&draft.editor_hidden_tools, &defaults.editor_hidden_tools)
            + differ(&draft.history.max_items, &defaults.history.max_items)
            + differ(&draft.defaults.beautify, &defaults.defaults.beautify)
            + differ(&draft.defaults.effect_preset, &defaults.defaults.effect_preset)
            + differ(&draft.defaults.screenshots_dir, &defaults.defaults.screenshots_dir)
            + draft
                .workflows
                .iter()
                .filter_map(|w| default_workflow(w, defaults).map(|d| (w, d)))
                .map(|(w, d)| usize::from(w.name != d.name) + differ(&w.patch, &d.patch))
                .sum::<usize>();
    }
    if scope == Scope::Shortcuts {
        // A key is one setting, however many fields it serializes to.
        let keys = draft.workflows.iter().filter_map(|w| default_workflow(w, defaults).map(|d| (w, d)));
        return keys.map(|(w, d)| usize::from(w.hotkey != d.hotkey) + usize::from(w.enabled != d.enabled)).sum::<usize>()
            + usize::from(draft.disable_hotkeys != defaults.disable_hotkeys)
            + missing_workflows(draft, defaults);
    }
    let mut probe = draft.clone();
    reset(scope, &mut probe, defaults);
    differ(draft, &probe)
}

/// Puts the settings in `scope` back to `defaults`.
pub fn reset(scope: Scope, draft: &mut Settings, defaults: &Settings) {
    let (d, f) = (&mut draft.defaults, &defaults.defaults);
    match scope {
        Scope::General => {
            draft.theme = defaults.theme;
            draft.language.clone_from(&defaults.language);
            draft.screenshots_dir.clone_from(&defaults.screenshots_dir);
            draft.history.enabled = defaults.history.enabled;
            d.general.clone_from(&f.general);
        }
        Scope::AfterCapture => d.after_capture = f.after_capture,
        Scope::Image => d.image.clone_from(&f.image),
        Scope::Naming => d.naming.clone_from(&f.naming),
        Scope::Capture => {
            let scrolling = d.capture.scrolling.clone();
            d.capture.clone_from(&f.capture);
            d.capture.scrolling = scrolling;
        }
        Scope::Shortcuts => {
            draft.disable_hotkeys = defaults.disable_hotkeys;
            for workflow in &mut draft.workflows {
                if let Some(default) = default_workflow(workflow, defaults) {
                    reset_shortcut(workflow, default);
                }
            }
            restore_workflows(draft, defaults);
        }
        Scope::Uploads => {
            d.upload.clone_from(&f.upload);
            d.after_upload = f.after_upload;
        }
        Scope::Recording => d.recording.clone_from(&f.recording),
        Scope::Automation => {
            draft.auto_capture.clone_from(&defaults.auto_capture);
            d.capture.scrolling.clone_from(&f.capture.scrolling);
        }
        Scope::Models => {
            let custom = std::mem::take(&mut draft.models.custom);
            draft.models.clone_from(&defaults.models);
            draft.models.custom = custom;
            draft.cloud.clone_from(&defaults.cloud);
        }
        Scope::All => {
            for page in PAGES {
                reset(page, draft, defaults);
            }
            let (d, f) = (&mut draft.defaults, &defaults.defaults);
            d.beautify.clone_from(&f.beautify);
            d.effect_preset.clone_from(&f.effect_preset);
            d.screenshots_dir.clone_from(&f.screenshots_dir);
            draft.tray_click = defaults.tray_click;
            draft.editor_hidden_tools.clone_from(&defaults.editor_hidden_tools);
            draft.history.max_items = defaults.history.max_items;
            for workflow in &mut draft.workflows {
                if let Some(default) = default_workflow(workflow, defaults) {
                    workflow.name.clone_from(&default.name);
                    workflow.patch = default.patch.clone();
                }
            }
        }
    }
}

/// How many built-in workflows the draft no longer has (the Workflows page can remove one).
fn missing_workflows(draft: &Settings, defaults: &Settings) -> usize {
    defaults.workflows.iter().filter(|d| !has_workflow(draft, d)).count()
}

/// Brings back each removed built-in workflow, where it was in the defaults.
fn restore_workflows(draft: &mut Settings, defaults: &Settings) {
    for (index, default) in defaults.workflows.iter().enumerate() {
        if !has_workflow(draft, default) {
            let at = index.min(draft.workflows.len());
            draft.workflows.insert(at, default.clone());
        }
    }
}

/// Whether `draft` still has the built-in workflow `default`.
fn has_workflow(draft: &Settings, default: &Workflow) -> bool {
    draft.workflows.iter().any(|w| w.id == default.id && w.action == default.action)
}

/// The built-in workflow `workflow` started as (same id and action), if it is one.
pub fn default_workflow<'a>(workflow: &Workflow, defaults: &'a Settings) -> Option<&'a Workflow> {
    defaults.workflows.iter().find(|d| d.id == workflow.id && d.action == workflow.action)
}

/// True when a built-in workflow's key or on/off state differs from its default.
pub fn shortcut_changed(workflow: &Workflow, defaults: &Settings) -> bool {
    default_workflow(workflow, defaults).is_some_and(|d| d.hotkey != workflow.hotkey || d.enabled != workflow.enabled)
}

/// Puts a built-in workflow's key and on/off state back.
pub fn reset_shortcut(workflow: &mut Workflow, default: &Workflow) {
    workflow.hotkey = default.hotkey;
    workflow.enabled = default.enabled;
}

/// The text each settings field shows for a value of the settings, so a field can tell whether
/// it differs from the default and what its reset puts back. The settings window fills its fields
/// from this too, so the two can't drift apart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputTexts {
    pub dir: String,
    pub pattern: String,
    pub window_pattern: String,
    pub subfolder: String,
    pub jpeg_quality: String,
    pub auto_jpeg: String,
    pub thumb_w: String,
    pub thumb_h: String,
    pub delay: String,
    pub toast_ms: String,
    pub custom_region: String,
    pub window_title: String,
    pub sound_capture: String,
    pub sound_task: String,
    pub sound_error: String,
    pub clipboard_format: String,
    pub retries: String,
    pub rec_fps: String,
    pub rec_gif_fps: String,
    pub rec_crf: String,
    pub rec_countdown: String,
    pub rec_duration: String,
    pub rec_microphone: String,
    pub rec_args: String,
    pub rec_ffmpeg: String,
    pub rec_camera_device: String,
    pub rec_camera_size: String,
    pub auto_interval: String,
    pub auto_count: String,
    /// Start delay, scroll delay, max frames, scroll amount, ignore top, ignore bottom.
    pub scroll: [String; 6],
    /// The analyzer's base URL, model and prompt.
    pub cloud: [String; 3],
}

fn path_text(p: &Option<std::path::PathBuf>) -> String {
    p.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
}

impl InputTexts {
    pub fn of(s: &Settings) -> Self {
        let d = &s.defaults;
        let o = &d.capture.scrolling;
        let a = &s.cloud.analyzer;
        InputTexts {
            dir: s.screenshots_dir.display().to_string(),
            pattern: d.naming.pattern.clone(),
            window_pattern: d.naming.window_pattern.clone(),
            subfolder: d.naming.subfolder.clone(),
            jpeg_quality: d.image.jpeg_quality.to_string(),
            auto_jpeg: d.image.auto_jpeg_above_bytes.map(|b| (b / 1024).to_string()).unwrap_or_default(),
            thumb_w: d.image.thumbnail_width.to_string(),
            thumb_h: d.image.thumbnail_height.to_string(),
            delay: d.capture.delay_ms.to_string(),
            toast_ms: d.general.toast.duration_ms.to_string(),
            custom_region: d.capture.custom_region.map(|r| format!("{}, {}, {}, {}", r.x, r.y, r.width, r.height)).unwrap_or_default(),
            window_title: d.capture.custom_window_title.clone(),
            sound_capture: path_text(&d.general.custom_capture_sound),
            sound_task: path_text(&d.general.custom_task_sound),
            sound_error: path_text(&d.general.custom_error_sound),
            clipboard_format: d.upload.clipboard_format.clone(),
            retries: d.upload.retries.to_string(),
            rec_fps: d.recording.fps.to_string(),
            rec_gif_fps: d.recording.gif_fps.to_string(),
            rec_crf: match d.recording.quality {
                laqta_domain::record::Quality::Crf(c) => c.to_string(),
                _ => String::new(),
            },
            rec_countdown: d.recording.countdown_secs.to_string(),
            rec_duration: d.recording.fixed_duration_secs.to_string(),
            rec_microphone: d.recording.microphone.clone().unwrap_or_default(),
            rec_args: d.recording.custom_args.clone(),
            rec_ffmpeg: d.recording.ffmpeg_path.clone(),
            rec_camera_device: d.recording.camera.device.clone(),
            rec_camera_size: d.recording.camera.size_percent.to_string(),
            auto_interval: (s.auto_capture.interval_ms / 1000).to_string(),
            auto_count: s.auto_capture.count.to_string(),
            scroll: [o.start_delay_ms, o.scroll_delay_ms, o.max_frames, o.scroll_amount, o.ignore_top, o.ignore_bottom]
                .map(|v| v.to_string()),
            cloud: [a.base_url.clone(), a.model.clone(), a.prompt.clone()],
        }
    }
}

/// Whether a field's text differs from its default. Spaces at the ends don't count.
pub fn text_changed(current: &str, default: &str) -> bool {
    current.trim() != default.trim()
}

/// Leaf values that differ between two serializable values (1 for a changed scalar, or a list that
/// changed length).
fn differ<T: Serialize>(a: &T, b: &T) -> usize {
    match (serde_json::to_value(a), serde_json::to_value(b)) {
        (Ok(a), Ok(b)) => leaves(&a, &b),
        _ => 0,
    }
}

fn leaves(a: &serde_json::Value, b: &serde_json::Value) -> usize {
    use serde_json::Value;
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            let null = Value::Null;
            let keys: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            keys.into_iter().map(|k| leaves(a.get(k).unwrap_or(&null), b.get(k).unwrap_or(&null))).sum()
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => a.iter().zip(b).map(|(a, b)| leaves(a, b)).sum(),
        (a, b) => usize::from(a != b),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use laqta_domain::hotkey::KeyChord;
    use laqta_domain::settings::Theme;

    fn defaults() -> Settings {
        Settings::default_for("/home/u/Pictures".into())
    }

    #[test]
    fn defaults_have_nothing_to_reset() {
        let d = defaults();
        for scope in PAGES.into_iter().chain([Scope::All]) {
            assert_eq!(changed(scope, &d, &d), 0, "{scope:?}");
        }
    }

    #[test]
    fn reset_all_counts_a_renamed_built_in_workflow() {
        let d = defaults();
        let mut s = d.clone();
        s.workflows[0].name = "Mine".into();
        assert_eq!(changed(Scope::All, &s, &d), 1, "the Reset all button would do nothing");
        reset(Scope::All, &mut s, &d);
        assert_eq!(changed(Scope::All, &s, &d), 0);
    }

    #[test]
    fn reset_all_brings_back_a_removed_built_in_workflow() {
        let d = defaults();
        let mut s = d.clone();
        let removed = s.workflows.remove(1);
        assert_eq!(changed(Scope::All, &s, &d), 1, "the Reset all button would do nothing");
        reset(Scope::All, &mut s, &d);
        assert_eq!(s.workflows[1], removed, "back where it was");
        assert_eq!(changed(Scope::All, &s, &d), 0);
    }

    #[test]
    fn the_shortcuts_reset_brings_back_a_removed_built_in_workflow() {
        let d = defaults();
        let mut s = d.clone();
        let removed = s.workflows.remove(2);
        assert_eq!(changed(Scope::Shortcuts, &s, &d), 1, "the page would hide its reset button");
        reset(Scope::Shortcuts, &mut s, &d);
        assert_eq!(s.workflows[2], removed);
        assert_eq!(changed(Scope::Shortcuts, &s, &d), 0);
    }

    #[test]
    fn a_page_counts_and_resets_only_its_own_settings() {
        let d = defaults();
        let mut s = d.clone();
        s.defaults.image.jpeg_quality = 42;
        s.defaults.image.thumbnail_width += 1;
        s.theme = Theme::Dark;
        assert_eq!(changed(Scope::Image, &s, &d), 2);
        assert_eq!(changed(Scope::General, &s, &d), 1);
        reset(Scope::Image, &mut s, &d);
        assert_eq!(changed(Scope::Image, &s, &d), 0);
        assert_eq!(s.theme, Theme::Dark, "another page's setting was reset");
    }

    #[test]
    fn capture_and_automation_split_the_capture_options() {
        let d = defaults();
        let mut s = d.clone();
        s.defaults.capture.scrolling.max_frames += 1;
        s.defaults.capture.delay_ms += 1;
        reset(Scope::Capture, &mut s, &d);
        assert_eq!(s.defaults.capture.delay_ms, d.defaults.capture.delay_ms);
        assert_ne!(s.defaults.capture.scrolling.max_frames, d.defaults.capture.scrolling.max_frames);
        reset(Scope::Automation, &mut s, &d);
        assert_eq!(s, d);
    }

    #[test]
    fn shortcuts_reset_built_in_keys_and_keep_added_workflows() {
        let d = defaults();
        let mut s = d.clone();
        s.workflows[0].hotkey = Some("Ctrl+W".parse::<KeyChord>().unwrap());
        s.workflows[1].enabled = false;
        let mut added = s.workflows[0].clone();
        added.id = laqta_domain::workflow::WorkflowId(99);
        added.hotkey = Some("Alt+B".parse().unwrap());
        s.workflows.push(added);
        assert!(shortcut_changed(&s.workflows[0], &d));
        assert!(!shortcut_changed(&s.workflows[5], &d), "an added workflow has no default");
        assert_eq!(changed(Scope::Shortcuts, &s, &d), 2);
        reset(Scope::Shortcuts, &mut s, &d);
        assert_eq!(s.workflows.len(), 6);
        assert_eq!(s.workflows[..5], d.workflows[..]);
        assert_eq!(s.workflows[5].hotkey, Some("Alt+B".parse().unwrap()));
    }

    #[test]
    fn reset_all_keeps_user_data() {
        let d = defaults();
        let mut s = d.clone();
        s.models.custom.push(laqta_domain::settings::CustomModel {
            id: "custom-1".into(),
            name: "Mine".into(),
            task: "background".into(),
            family: "matting".into(),
        });
        s.watch_folders.push(Default::default());
        s.models.provider = laqta_domain::settings::ModelProvider::Cpu;
        s.defaults.effect_preset = "Shadow".into();
        assert_eq!(changed(Scope::All, &s, &d), 2);
        reset(Scope::All, &mut s, &d);
        assert_eq!(changed(Scope::All, &s, &d), 0);
        assert_eq!(s.models.custom.len(), 1);
        assert_eq!(s.watch_folders.len(), 1);
    }
}
