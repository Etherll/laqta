//! Helpers for the OAuth tests: the clock and config they share, token JSON, a secret store on a
//! platform without a keyring, and a stand-in for the browser that answers the loopback redirect.
#![allow(clippy::unwrap_used)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use laqta_ports::fakes::{CountingRng, FakeShell};
use laqta_ports::{PortError, SecretStore};
use laqta_upload::oauth::{OAuth, OAuthConfig};
use serde_json::json;

/// The time every test runs at, in Unix seconds.
pub const NOW: i64 = 1_000_000;

/// A config whose token endpoint is `token_url`.
pub fn config(token_url: String) -> OAuthConfig {
    OAuthConfig {
        auth_url: "https://auth.example.test/authorize".to_owned(),
        token_url,
        client_id: "test-client".to_owned(),
        client_secret: None,
        scopes: vec!["files.write".to_owned(), "offline_access".to_owned()],
        extra_auth_params: vec![("access_type".to_owned(), "offline".to_owned())],
    }
}

/// A sign-in at `NOW`, with `CountingRng` for its randomness.
pub fn make_oauth(config: OAuthConfig, secrets: Arc<dyn SecretStore>, shell: Arc<FakeShell>, key: &str) -> Arc<OAuth> {
    Arc::new(OAuth::new(config, secrets, shell, Arc::new(CountingRng::default()), key, Arc::new(|| NOW)))
}

/// The token JSON that the store holds.
pub fn stored(access: &str, refresh: Option<&str>, expires_at: Option<i64>) -> String {
    json!({"access_token": access, "refresh_token": refresh, "expires_at_unix": expires_at}).to_string()
}

/// A token endpoint's reply.
pub fn token_reply(access: &str, refresh: Option<&str>, expires_in: Option<i64>) -> String {
    let mut body = json!({"access_token": access, "token_type": "bearer"});
    if let Some(refresh) = refresh {
        body["refresh_token"] = json!(refresh);
    }
    if let Some(seconds) = expires_in {
        body["expires_in"] = json!(seconds);
    }
    body.to_string()
}

/// A platform with no keyring: every operation is `Unsupported`.
#[derive(Debug)]
pub struct NoKeyring;

impl SecretStore for NoKeyring {
    fn get(&self, _key: &str) -> Result<Option<String>, PortError> {
        Err(PortError::Unsupported("no keyring".into()))
    }

    fn set(&self, _key: &str, _secret: &str) -> Result<(), PortError> {
        Err(PortError::Unsupported("no keyring".into()))
    }

    fn delete(&self, _key: &str) -> Result<(), PortError> {
        Err(PortError::Unsupported("no keyring".into()))
    }
}

/// Plays the browser on another thread. Once `shell` has been asked to open a sign-in URL, it sends
/// the loopback redirect with the query `query(state)`, then returns the URL it was sent to.
pub fn answer_sign_in(shell: Arc<FakeShell>, query: impl FnOnce(&str) -> String + Send + 'static) -> JoinHandle<url::Url> {
    std::thread::spawn(move || {
        let auth = url::Url::parse(&wait_for_url(&shell)).unwrap();
        let param = |name: &str| auth.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned());
        let redirect = url::Url::parse(&param("redirect_uri").unwrap()).unwrap();
        let state = param("state").unwrap();
        let port = redirect.port().unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(stream, "GET {}?{} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n", redirect.path(), query(&state))
            .unwrap();
        let mut reply = String::new();
        let _ = stream.read_to_string(&mut reply);
        auth
    })
}

fn wait_for_url(shell: &FakeShell) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let first = shell.urls.lock().unwrap().first().cloned();
        if let Some(url) = first {
            return url;
        }
        assert!(Instant::now() < deadline, "the browser was never opened");
        std::thread::sleep(Duration::from_millis(5));
    }
}
