//! `HotkeysConfig.json`: each entry is a ShareX task with the hotkey that starts it and the task settings
//! it overrides. Each entry becomes a workflow.

use laqta_domain::workflow::{TaskSettings, Workflow, WorkflowId};

use crate::json::{self, Object};
use crate::uploaders::Selections;
use crate::{jobs, keys, task};

pub(crate) fn read(root: &Object, defaults: &TaskSettings, selections: &Selections, warnings: &mut Vec<String>) -> Vec<Workflow> {
    let mut workflows = Vec::new();
    for (index, entry) in json::list(root, "Hotkeys").iter().enumerate() {
        let what = format!("HotkeysConfig.json hotkey {}", index + 1);
        let Some(entry) = entry.as_object() else {
            warnings.push(format!("{what} is not an object, skipped"));
            continue;
        };
        let Some(task_object) = json::sub(entry, "TaskSettings") else {
            warnings.push(format!("{what} has no TaskSettings, skipped"));
            continue;
        };
        let Some(job) = json::text(task_object, "Job") else {
            warnings.push(format!("{what} has no Job, skipped"));
            continue;
        };
        if job.eq_ignore_ascii_case("None") {
            continue;
        }
        let Some(action) = jobs::action(job) else {
            warnings.push(format!("{what}: task `{job}` has no Laqta equivalent, skipped"));
            continue;
        };

        let info = json::sub(entry, "HotkeyInfo");
        let win = info.and_then(|info| json::flag(info, "Win")).unwrap_or(false);
        let hotkey = match info.and_then(|info| json::text(info, "Hotkey")) {
            None => None,
            Some(text) => keys::chord(text, win).unwrap_or_else(|reason| {
                warnings.push(format!("{what}: hotkey `{text}` {reason}; kept without a hotkey"));
                None
            }),
        };

        let id = WorkflowId(u32::try_from(workflows.len() + 1).unwrap_or(u32::MAX));
        let name = json::text(task_object, "Description").map_or_else(|| action.label().to_owned(), str::to_owned);
        let patch = task::patch(task_object, defaults, selections, &what, warnings);
        workflows.push(Workflow { id, name, action, hotkey, enabled: true, patch });
    }
    workflows
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn unknown_tasks_are_skipped_with_a_warning() {
        let root: Object = serde_json::from_str(
            r#"{"Hotkeys": [
                {"HotkeyInfo": {"Hotkey": "F9", "Win": false}, "TaskSettings": {"Job": "NoSuchTask"}},
                {"HotkeyInfo": {"Hotkey": "F10", "Win": false}, "TaskSettings": {"Job": "None"}},
                {"HotkeyInfo": {"Hotkey": "F11", "Win": false}, "TaskSettings": {"Job": "Ruler"}}
            ]}"#,
        )
        .unwrap();
        let mut warnings = Vec::new();
        let workflows = read(&root, &TaskSettings::default(), &Selections::default(), &mut warnings);
        assert_eq!(workflows.len(), 1);
        assert_eq!(workflows[0].action, laqta_domain::action::Action::Ruler);
        assert_eq!(workflows[0].id, WorkflowId(1));
        assert!(warnings.iter().any(|warning| warning.contains("NoSuchTask")));
    }
}
