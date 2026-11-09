//! Capture-area gaps: name-pattern tokens and per-hotkey workflows. Each `audit_bug_*` test
//! describes behaviour Laqta does not have yet, and fails until it does.
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
        random_file_lines: &[],
        rng: &mut rng,
    };
    expand(pattern, &mut ctx)
}

/// `%mon2` and `%w2` expand to the full English month and day names ("November", "Saturday").
/// Laqta writes three-letter abbreviations.
#[test]
#[ignore = "known issue, not fixed yet"]
fn audit_bug_mon2_w2_are_full_english_names() {
    assert_eq!(run("%mon2"), "November");
    assert_eq!(run("%w2"), "Saturday");
}

/// Laqta does not yet have the upper-case token variants `%iX` (hex upper), `%iA` (base 36 upper),
/// `%iaA` (base 62 inverse), `%iB{b,n}`, `%rX` (random hex upper) and `%GUID`. Until it does, `%iX`
/// expands as `%i` followed by a literal `X`.
#[test]
#[ignore = "known issue, not fixed yet"]
fn audit_bug_uppercase_token_variants() {
    assert_eq!(run("%iX"), "FF");
    assert_eq!(run("%iA"), "73");
    assert!(run("%GUID").chars().all(|c| !c.is_ascii_lowercase()), "%GUID is upper case in ShareX");
}

/// `%rf{path}` should read a random line from the file named in braces. Laqta's `%rf` takes no
/// argument, leaves `{path}` in the name, and production passes no lines (`task/save.rs`
/// `random_file_lines: &[]`).
#[test]
#[ignore = "known issue, not fixed yet"]
fn audit_bug_rf_takes_a_file_argument() {
    let out = run("a%rf{C:/names.txt}b");
    assert!(!out.contains('{'), "the {{path}} argument stayed literal: {out:?}");
}

/// `%ib{base,digits}` should take a base up to 62 and a pad width. Laqta caps the base at 36 and
/// has no pad argument.
#[test]
#[ignore = "known issue, not fixed yet"]
fn audit_bug_ib_base_62_and_padding() {
    assert_eq!(run("%ib{62}"), "47", "255 in base 62");
    assert_eq!(run("%ib{16,4}"), "00ff");
}

/// Every hotkey is a workflow with its own task settings, and two hotkeys may share one
/// action (e.g. Ctrl+PrintScreen region→save, Ctrl+Shift+R region→upload). Laqta registers
/// `(KeyChord, Action)` pairs and `apps/laqta/src/wiring.rs` looks the workflow up by action
/// ("first enabled workflow bound to its action"), so the second workflow's settings never apply.
#[test]
#[ignore = "known issue, not fixed yet"]
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
    let bindings = settings.hotkey_bindings();
    let chord = KeyChord { modifiers: Modifiers::CTRL | Modifiers::SHIFT, key: Key::R };
    let (_, action) = bindings.iter().find(|(c, _)| *c == chord).copied().unwrap();
    // What the hotkey callback does with the fired action (wiring.rs, `dispatch` closure).
    let picked = settings.workflows.iter().find(|w| w.enabled && w.action == action).map(|w| w.id);
    assert_eq!(picked, Some(WorkflowId(9)), "the Ctrl+Shift+R hotkey ran workflow {picked:?}'s settings");
}
