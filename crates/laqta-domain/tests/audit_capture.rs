//! Capture-area audit checks: name-pattern tokens and per-hotkey workflows. An ignored `audit_bug_*`
//! test describes behaviour Laqta does not have yet.
#![allow(clippy::unwrap_used)]

use laqta_domain::naming::{NameContext, expand};
use laqta_domain::workflow::{AfterCaptureTasks, Workflow, WorkflowId};
use laqta_domain::{Action, Key, KeyChord, Modifiers, Settings};

fn run(pattern: &str) -> String {
    let mut n = 0u64;
    let mut rng = move || {
        n += 1;
        n
    };
    let no_files = |_: &str| -> Vec<String> { Vec::new() };
    let mut ctx = NameContext {
        now: jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0),
        unix_seconds: 0,
        window_title: "Editor",
        process_name: "code",
        width: Some(10),
        height: Some(10),
        counter: 255,
        user_name: "ada",
        user_login: "ada",
        computer_name: "pc",
        read_lines: &no_files,
        rng: &mut rng,
    };
    expand(pattern, &mut ctx)
}

/// `%mon2` and `%w2` expand to the full English month and day names ("November", "Saturday"), as
/// ShareX's do.
#[test]
fn audit_bug_mon2_w2_are_full_english_names() {
    assert_eq!(run("%mon2"), "November");
    assert_eq!(run("%w2"), "Saturday");
}

/// The upper-case token variants match ShareX: `%iX` is hex in upper case, `%iA` is base 36 in upper
/// case, and `%GUID` is a GUID in upper case.
#[test]
fn audit_bug_uppercase_token_variants() {
    assert_eq!(run("%iX"), "FF");
    assert_eq!(run("%iA"), "73");
    assert!(run("%GUID").chars().all(|c| !c.is_ascii_lowercase()), "%GUID is upper case in ShareX");
}

/// `%rf{path}` takes a file path and draws a random line from it. The domain reads no files, so
/// `run` supplies a reader with no lines: the token is consumed, braces and all, and leaves nothing.
#[test]
fn audit_bug_rf_takes_a_file_argument() {
    let out = run("a%rf{C:/names.txt}b");
    assert_eq!(out, "ab");
}

/// `%ib{base,digits}` takes a base from 2 to 62 and an optional zero-pad width, as ShareX's does.
#[test]
fn audit_bug_ib_base_62_and_padding() {
    assert_eq!(run("%ib{62}"), "47", "255 in base 62");
    assert_eq!(run("%ib{16,4}"), "00ff");
}

/// Every hotkey is a workflow with its own task settings, and two hotkeys may share one
/// action (e.g. Ctrl+PrintScreen region→save, Ctrl+Shift+R region→upload). The fired chord picks
/// its own workflow (`workflow_for_hotkey`, as `apps/laqta/src/wiring.rs` dispatches it), not
/// the first workflow bound to the same action.
#[test]
fn audit_bug_hotkey_binding_drops_the_workflow() {
    let mut settings = Settings::default_for("/p".into());
    let mut upload = Workflow {
        id: WorkflowId(9),
        name: "Region and upload".into(),
        action: Action::RectangleRegion,
        hotkey: Some(KeyChord { modifiers: Modifiers::CTRL | Modifiers::SHIFT, key: Key::R }),
        enabled: true,
        patch: Default::default(),
    };
    upload.patch.after_capture = Some(AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST);
    settings.workflows.push(upload);
    let chord = KeyChord { modifiers: Modifiers::CTRL | Modifiers::SHIFT, key: Key::R };
    assert!(settings.hotkey_bindings().iter().any(|(c, _)| *c == chord), "the chord is registered");
    // What the hotkey callback does with the fired chord (wiring.rs, `hotkey_dispatch`).
    let picked = settings.workflow_for_hotkey(chord).map(|w| w.id);
    assert_eq!(picked, Some(WorkflowId(9)), "the Ctrl+Shift+R hotkey ran workflow {picked:?}'s settings");
}
