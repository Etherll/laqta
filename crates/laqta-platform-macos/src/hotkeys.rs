//! Global hotkeys (`HotkeyRegistrar`) through Carbon `RegisterEventHotKey`.

use laqta_domain::action::Action;
use laqta_domain::hotkey::KeyChord;
use laqta_ports::{ActionCallback, HotkeyRegistrar, PortError};

#[derive(Debug, Default)]
pub struct MacHotkeys;

impl HotkeyRegistrar for MacHotkeys {
    fn register(&self, bindings: &[(KeyChord, Action)], _on_fire: ActionCallback) -> Vec<(KeyChord, Result<(), PortError>)> {
        bindings.iter().map(|(chord, _)| (*chord, Err(PortError::Unsupported("global hotkeys on macOS".into())))).collect()
    }
}
