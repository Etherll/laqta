//! Plain values and small adapters `main` builds itself.

use std::sync::Arc;

use laqta_ports::{HostInfo, PortError, Rng, SecretStore};

#[derive(Debug)]
pub struct SystemRng(std::sync::Mutex<fastrand::Rng>);

impl Rng for SystemRng {
    fn next_u64(&self) -> u64 {
        match self.0.lock() {
            Ok(mut r) => r.u64(..),
            Err(p) => p.into_inner().u64(..),
        }
    }
}

pub fn system_rng() -> Arc<dyn Rng> {
    Arc::new(SystemRng(std::sync::Mutex::new(fastrand::Rng::new())))
}

pub fn host_info() -> HostInfo {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let user = if cfg!(windows) { env("USERNAME") } else { env("USER") };
    let computer = if cfg!(windows) {
        env("COMPUTERNAME")
    } else {
        std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).unwrap_or_else(|_| env("HOSTNAME"))
    };
    HostInfo { user_name: user.clone(), user_login: user, computer_name: computer }
}

/// Sandbox mode's secret store: kept in memory, gone on exit.
#[derive(Debug, Default)]
pub struct MemorySecrets(std::sync::Mutex<std::collections::HashMap<String, String>>);

impl laqta_ports::SecretStore for MemorySecrets {
    fn get(&self, key: &str) -> Result<Option<String>, laqta_ports::PortError> {
        Ok(self.0.lock().ok().and_then(|m| m.get(key).cloned()))
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), laqta_ports::PortError> {
        if let Ok(mut m) = self.0.lock() {
            m.insert(key.to_owned(), secret.to_owned());
        }
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), laqta_ports::PortError> {
        if let Ok(mut m) = self.0.lock() {
            m.remove(key);
        }
        Ok(())
    }
}

/// The OS keyring with a session fallback (ADR 018). A keyring that cannot be used (`Unsupported`: no Secret Service
/// on this desktop) does not lose the credentials a host or the native-messaging setup hands over: they stay in
/// memory for the session, and are asked for again next time. A working keyring is used as before, and keeps the only
/// copy.
pub struct SessionSecrets {
    keyring: Arc<dyn SecretStore>,
    /// What the keyring refused.
    session: MemorySecrets,
}

impl SessionSecrets {
    pub fn new(keyring: Arc<dyn SecretStore>) -> SessionSecrets {
        SessionSecrets { keyring, session: MemorySecrets::default() }
    }
}

impl std::fmt::Debug for SessionSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionSecrets").finish_non_exhaustive()
    }
}

impl SecretStore for SessionSecrets {
    /// A secret kept for the session, else the keyring's. With no keyring and no session copy there is no secret,
    /// which a host that does not need one (an SMTP relay without a login) can live with.
    fn get(&self, key: &str) -> Result<Option<String>, PortError> {
        if let Some(secret) = self.session.get(key)? {
            return Ok(Some(secret));
        }
        match self.keyring.get(key) {
            Err(PortError::Unsupported(_)) => Ok(None),
            found => found,
        }
    }

    /// Any failure of the keyring still leaves the secret in memory for this session. `Unsupported` is no failure
    /// to report: the session copy is the whole store then.
    fn set(&self, key: &str, secret: &str) -> Result<(), PortError> {
        match self.keyring.set(key, secret) {
            Ok(()) => self.session.delete(key),
            Err(PortError::Unsupported(why)) => {
                tracing::debug!(key, "no keyring ({why}); the secret is kept for this session");
                self.session.set(key, secret)
            }
            Err(error) => {
                self.session.set(key, secret)?;
                Err(error)
            }
        }
    }

    fn delete(&self, key: &str) -> Result<(), PortError> {
        self.session.delete(key)?;
        match self.keyring.delete(key) {
            Err(PortError::Unsupported(_)) => Ok(()),
            done => done,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod session_secrets_tests {
    use super::*;

    /// A keyring that answers every call with `error`, or stores in a map when it has none.
    #[derive(Default)]
    struct Keyring {
        error: Option<PortError>,
        kept: MemorySecrets,
    }

    impl SecretStore for Keyring {
        fn get(&self, key: &str) -> Result<Option<String>, PortError> {
            self.error.clone().map_or_else(|| self.kept.get(key), Err)
        }
        fn set(&self, key: &str, secret: &str) -> Result<(), PortError> {
            self.error.clone().map_or_else(|| self.kept.set(key, secret), Err)
        }
        fn delete(&self, key: &str) -> Result<(), PortError> {
            self.error.clone().map_or_else(|| self.kept.delete(key), Err)
        }
    }

    fn without_keyring() -> SessionSecrets {
        SessionSecrets::new(Arc::new(Keyring { error: Some(PortError::Unsupported("no secret service".into())), ..Keyring::default() }))
    }

    #[test]
    fn without_a_keyring_a_password_set_this_session_is_read_back() {
        let secrets = without_keyring();
        assert_eq!(secrets.get("ftp.password").unwrap(), None, "nothing is known before it is set");

        secrets.set("ftp.password", "hunter2").unwrap();
        assert_eq!(secrets.get("ftp.password").unwrap().as_deref(), Some("hunter2"));
        assert_eq!(secrets.get("sftp.password").unwrap(), None, "other keys are untouched");

        secrets.delete("ftp.password").unwrap();
        assert_eq!(secrets.get("ftp.password").unwrap(), None);
    }

    #[test]
    fn a_working_keyring_keeps_the_only_copy() {
        let keyring = Arc::new(Keyring::default());
        let secrets = SessionSecrets::new(keyring.clone());

        secrets.set("smtp.password", "pw").unwrap();

        assert_eq!(keyring.kept.get("smtp.password").unwrap().as_deref(), Some("pw"));
        assert_eq!(secrets.session.get("smtp.password").unwrap(), None, "no session copy beside the keyring's");
        assert_eq!(secrets.get("smtp.password").unwrap().as_deref(), Some("pw"));
    }

    #[test]
    fn a_keyring_that_fails_reports_it_and_the_secret_still_lasts_the_session() {
        let secrets = SessionSecrets::new(Arc::new(Keyring { error: Some(PortError::Failed("locked".into())), ..Keyring::default() }));

        assert!(matches!(secrets.set("bitly.token", "t"), Err(PortError::Failed(_))));
        assert_eq!(secrets.get("bitly.token").unwrap().as_deref(), Some("t"));
    }
}
