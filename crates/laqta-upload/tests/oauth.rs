//! Black-box tests of the OAuth sign-in: PKCE, the loopback round trip, refresh, and the token store.
#![allow(clippy::unwrap_used)]

mod support_oauth;

use std::collections::HashMap;
use std::sync::Arc;

use futures_lite::future::block_on;
use laqta_ports::fakes::{FakeSecretStore, FakeShell};
use laqta_ports::{SecretStore, UploadError};
use laqta_upload::oauth::pkce_challenge;
use serde_json::Value;
use support_oauth::sign_in::*;
use support_oauth::{Canned, Server};

const KEY: &str = "laqta/test";

fn params(url: &url::Url) -> HashMap<String, String> {
    url.query_pairs().into_owned().collect()
}

fn form(body: &[u8]) -> HashMap<String, String> {
    url::form_urlencoded::parse(body).into_owned().collect()
}

fn stored_json(secrets: &FakeSecretStore) -> Value {
    serde_json::from_str(&secrets.get(KEY).unwrap().unwrap()).unwrap()
}

#[test]
fn pkce_challenge_matches_the_rfc_7636_vector() {
    assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
}

#[test]
fn sign_in_runs_the_loopback_flow_and_stores_the_token() {
    let server = Server::start(vec![Canned::ok(token_reply("access-1", Some("refresh-1"), Some(3600)))]);
    let secrets = Arc::new(FakeSecretStore::default());
    let shell = Arc::new(FakeShell::default());
    let oauth = make_oauth(config(server.url("/token")), secrets.clone(), shell.clone(), KEY);
    let browser = answer_sign_in(shell.clone(), |state| format!("code=the-code&state={state}"));

    let token = block_on(oauth.access_token()).unwrap();
    let auth_url = browser.join().unwrap();
    let recorded = server.finish();

    assert_eq!(token, "access-1");
    assert_eq!(recorded.len(), 1);

    // The browser was sent to the authorization endpoint with PKCE and the loopback redirect.
    assert!(auth_url.as_str().starts_with("https://auth.example.test/authorize?"));
    let auth = params(&auth_url);
    assert_eq!(auth["response_type"], "code");
    assert_eq!(auth["client_id"], "test-client");
    assert_eq!(auth["scope"], "files.write offline_access");
    assert_eq!(auth["code_challenge_method"], "S256");
    assert_eq!(auth["access_type"], "offline");
    assert_eq!(auth["state"].len(), 22);
    let redirect = url::Url::parse(&auth["redirect_uri"]).unwrap();
    assert_eq!(redirect.host_str(), Some("127.0.0.1"));
    assert_eq!(redirect.path(), "/callback");

    // The code exchange carries the verifier that the challenge was made from.
    let request = &recorded[0];
    assert_eq!((request.method.as_str(), request.target.as_str()), ("POST", "/token"));
    assert_eq!(request.header("accept"), Some("application/json"));
    let body = form(&request.body);
    assert_eq!(body["grant_type"], "authorization_code");
    assert_eq!(body["code"], "the-code");
    assert_eq!(body["redirect_uri"], auth["redirect_uri"]);
    assert_eq!(body["client_id"], "test-client");
    assert_eq!(body["code_verifier"].len(), 43);
    assert_eq!(pkce_challenge(&body["code_verifier"]), auth["code_challenge"]);

    // The token is stored as JSON, and the session answers the next call without the browser.
    let stored = stored_json(&secrets);
    assert_eq!(stored["access_token"], "access-1");
    assert_eq!(stored["refresh_token"], "refresh-1");
    assert_eq!(stored["expires_at_unix"], NOW + 3600);
    assert_eq!(block_on(oauth.access_token()).unwrap(), "access-1");
    assert_eq!(shell.urls.lock().unwrap().len(), 1);
}

#[test]
fn an_authorization_url_without_scopes_has_no_scope_parameter() {
    let server = Server::start(vec![Canned::ok(token_reply("access-0", None, None))]);
    let shell = Arc::new(FakeShell::default());
    let mut config = config(server.url("/token"));
    config.scopes.clear();
    let oauth = make_oauth(config, Arc::new(FakeSecretStore::default()), shell.clone(), KEY);
    let browser = answer_sign_in(shell.clone(), |state| format!("code=c0&state={state}"));

    block_on(oauth.access_token()).unwrap();
    let auth = params(&browser.join().unwrap());
    server.finish();

    assert!(!auth.contains_key("scope"));
}

#[test]
fn a_stored_token_is_used_without_the_browser() {
    let secrets = Arc::new(FakeSecretStore::default());
    secrets.set(KEY, &stored("stored-token", Some("refresh"), Some(NOW + 3600))).unwrap();
    let shell = Arc::new(FakeShell::default());
    let oauth = make_oauth(config("http://127.0.0.1:9/token".into()), secrets.clone(), shell.clone(), KEY);

    assert!(oauth.is_signed_in());
    assert_eq!(block_on(oauth.access_token()).unwrap(), "stored-token");
    assert!(shell.urls.lock().unwrap().is_empty());
}

#[test]
fn an_expired_token_is_refreshed_with_its_refresh_token() {
    let server = Server::start(vec![Canned::ok(token_reply("access-2", None, Some(3600)))]);
    let secrets = Arc::new(FakeSecretStore::default());
    secrets.set(KEY, &stored("access-1", Some("refresh-1"), Some(NOW - 10))).unwrap();
    let shell = Arc::new(FakeShell::default());
    let oauth = make_oauth(config(server.url("/token")), secrets.clone(), shell.clone(), KEY);

    assert_eq!(block_on(oauth.access_token()).unwrap(), "access-2");

    let recorded = server.finish();
    let body = form(&recorded[0].body);
    assert_eq!(body["grant_type"], "refresh_token");
    assert_eq!(body["refresh_token"], "refresh-1");
    assert_eq!(body["client_id"], "test-client");
    assert!(shell.urls.lock().unwrap().is_empty(), "a refresh never opens the browser");
    // The reply had no refresh token, so the one in use is kept.
    let stored = stored_json(&secrets);
    assert_eq!(stored["access_token"], "access-2");
    assert_eq!(stored["refresh_token"], "refresh-1");
    assert_eq!(stored["expires_at_unix"], NOW + 3600);
}

#[test]
fn a_refresh_the_provider_rejects_starts_the_browser_sign_in() {
    let server = Server::start(vec![
        Canned::new(400, r#"{"error":"invalid_grant"}"#),
        Canned::ok(token_reply("access-3", Some("refresh-3"), Some(60))),
    ]);
    let secrets = Arc::new(FakeSecretStore::default());
    secrets.set(KEY, &stored("access-1", Some("revoked"), Some(NOW - 10))).unwrap();
    let shell = Arc::new(FakeShell::default());
    let oauth = make_oauth(config(server.url("/token")), secrets.clone(), shell.clone(), KEY);
    let browser = answer_sign_in(shell.clone(), |state| format!("code=c3&state={state}"));

    assert_eq!(block_on(oauth.access_token()).unwrap(), "access-3");
    browser.join().unwrap();

    let recorded = server.finish();
    assert_eq!(form(&recorded[0].body)["grant_type"], "refresh_token");
    assert_eq!(form(&recorded[1].body)["grant_type"], "authorization_code");
    assert_eq!(stored_json(&secrets)["access_token"], "access-3");
}

#[test]
fn a_redirect_with_another_state_is_refused_before_any_exchange() {
    let server = Server::start(Vec::new());
    let secrets = Arc::new(FakeSecretStore::default());
    let shell = Arc::new(FakeShell::default());
    let oauth = make_oauth(config(server.url("/token")), secrets.clone(), shell.clone(), KEY);
    let browser = answer_sign_in(shell.clone(), |_| "code=stolen&state=forged".to_owned());

    let error = block_on(oauth.access_token()).unwrap_err();
    browser.join().unwrap();

    assert!(matches!(error, UploadError::Auth(_)), "{error:?}");
    assert!(server.finish().is_empty(), "no code was exchanged");
    assert!(!oauth.is_signed_in());
    assert!(secrets.get(KEY).unwrap().is_none());
}

#[test]
fn a_provider_error_in_the_redirect_is_an_auth_failure_with_its_description() {
    let server = Server::start(Vec::new());
    let shell = Arc::new(FakeShell::default());
    let oauth = make_oauth(config(server.url("/token")), Arc::new(FakeSecretStore::default()), shell.clone(), KEY);
    let browser = answer_sign_in(shell.clone(), |state| format!("error=access_denied&error_description=The+user+said+no&state={state}"));

    let error = block_on(oauth.access_token()).unwrap_err();
    browser.join().unwrap();
    server.finish();

    match error {
        UploadError::Auth(message) => assert_eq!(message, "The user said no"),
        other => panic!("expected Auth, got {other:?}"),
    }
}

#[test]
fn without_a_keyring_the_token_stays_in_the_session() {
    let server = Server::start(vec![Canned::ok(token_reply("access-4", None, None))]);
    let shell = Arc::new(FakeShell::default());
    let oauth = make_oauth(config(server.url("/token")), Arc::new(NoKeyring), shell.clone(), KEY);
    let browser = answer_sign_in(shell.clone(), |state| format!("code=c4&state={state}"));

    assert_eq!(block_on(oauth.access_token()).unwrap(), "access-4");
    browser.join().unwrap();
    server.finish();

    // The second call needs neither the browser nor the server.
    assert_eq!(block_on(oauth.access_token()).unwrap(), "access-4");
    assert!(oauth.is_signed_in());
    assert_eq!(shell.urls.lock().unwrap().len(), 1);
}

#[test]
fn sign_out_forgets_the_session_and_the_stored_token() {
    let secrets = Arc::new(FakeSecretStore::default());
    secrets.set(KEY, &stored("access-5", None, None)).unwrap();
    let shell = Arc::new(FakeShell::default());
    let oauth = make_oauth(config("http://127.0.0.1:9/token".into()), secrets.clone(), shell, KEY);
    assert!(oauth.is_signed_in());

    oauth.sign_out().unwrap();

    assert!(!oauth.is_signed_in());
    assert!(secrets.get(KEY).unwrap().is_none());
}
