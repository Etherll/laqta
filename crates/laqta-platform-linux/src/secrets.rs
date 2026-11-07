//! Secret Service credentials for `SecretStore`, through `oo7`. Secret Service terms stay in this
//! file; the port sees only keys and secrets.

use laqta_ports::{PortError, SecretStore};

/// The `application` attribute that marks Laqta's items.
const APPLICATION: &str = "laqta";

/// Attributes that find the item for `key`: `{"application": "laqta", "key": key}`.
fn attributes(key: &str) -> [(&'static str, &str); 2] {
    [("application", APPLICATION), ("key", key)]
}

/// The label shown in keyring tools for `key`, e.g. `Laqta: imgur.token`.
fn label(key: &str) -> String {
    format!("Laqta: {key}")
}

fn failed(e: oo7::Error) -> PortError {
    PortError::Failed(format!("secret service: {e}"))
}

/// Opens the default collection. Any failure means no usable keyring, which the port reports as
/// `Unsupported`.
async fn keyring() -> Result<oo7::Keyring, PortError> {
    oo7::Keyring::new().await.map_err(|e| {
        tracing::debug!("secret service unavailable: {e}");
        PortError::Unsupported("no secret service".into())
    })
}

async fn get_async(key: &str) -> Result<Option<String>, PortError> {
    let keyring = keyring().await?;
    let items = keyring.search_items(&attributes(key)).await.map_err(failed)?;
    let Some(item) = items.first() else {
        return Ok(None);
    };
    let secret = item.secret().await.map_err(failed)?;
    let text = std::str::from_utf8(secret.as_bytes()).map_err(|e| PortError::Failed(format!("secret for {key} is not UTF-8: {e}")))?;
    Ok(Some(text.to_owned()))
}

async fn set_async(key: &str, secret: &str) -> Result<(), PortError> {
    let keyring = keyring().await?;
    keyring.create_item(&label(key), &attributes(key), secret, true).await.map_err(failed)
}

async fn delete_async(key: &str) -> Result<(), PortError> {
    let keyring = keyring().await?;
    keyring.delete(&attributes(key)).await.map_err(failed)
}

/// Keeps secrets in the default collection of the Secret Service. Each call opens its own
/// connection: credentials are read and written rarely, so nothing is cached.
#[derive(Debug, Default)]
pub struct SecretServiceStore;

impl SecretStore for SecretServiceStore {
    fn get(&self, key: &str) -> Result<Option<String>, PortError> {
        async_io::block_on(get_async(key))
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), PortError> {
        async_io::block_on(set_async(key, secret))
    }

    fn delete(&self, key: &str) -> Result<(), PortError> {
        async_io::block_on(delete_async(key))
    }
}

#[cfg(test)]
mod tests {
    use super::{attributes, label};

    #[test]
    fn attributes_name_the_application_and_key() {
        assert_eq!(attributes("imgur.token"), [("application", "laqta"), ("key", "imgur.token")]);
    }

    #[test]
    fn label_names_the_key() {
        assert_eq!(label("imgur.token"), "Laqta: imgur.token");
    }
}
