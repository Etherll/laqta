//! Keychain generic passwords (`SecretStore`).

use laqta_ports::{PortError, SecretStore};

#[derive(Debug, Default)]
pub struct Keychain;

impl SecretStore for Keychain {
    fn get(&self, _key: &str) -> Result<Option<String>, PortError> {
        Err(PortError::Unsupported("the Keychain on macOS".into()))
    }

    fn set(&self, _key: &str, _secret: &str) -> Result<(), PortError> {
        Err(PortError::Unsupported("the Keychain on macOS".into()))
    }

    fn delete(&self, _key: &str) -> Result<(), PortError> {
        Err(PortError::Unsupported("the Keychain on macOS".into()))
    }
}
