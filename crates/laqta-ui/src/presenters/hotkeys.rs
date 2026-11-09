//! Hotkey hints for buttons that run an action.

use laqta_domain::Action;
use laqta_domain::hotkey::KeyChord;

/// The hotkey bound to `action`, as short text such as "Ctrl+PrtSc"; the first binding wins.
/// `bindings` is `Settings::hotkey_bindings()`, which is empty while hotkeys are disabled.
pub fn hotkey_label(bindings: &[(KeyChord, Action)], action: Action) -> Option<String> {
    bindings.iter().find(|(_, a)| *a == action).map(|(chord, _)| chord.to_string().replace("PrintScreen", "PrtSc"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use laqta_domain::settings::Settings;

    #[test]
    fn finds_the_first_binding_for_an_action() {
        let bindings = Settings::default_for("/p".into()).hotkey_bindings();
        let (chord, action) = bindings[0];
        let label = hotkey_label(&bindings, action).unwrap();
        assert_eq!(label, chord.to_string().replace("PrintScreen", "PrtSc"));
        assert!(!label.contains("PrintScreen"));
    }

    #[test]
    fn unbound_action_has_no_label() {
        assert_eq!(hotkey_label(&[], Action::PrintScreen), None);
    }
}
