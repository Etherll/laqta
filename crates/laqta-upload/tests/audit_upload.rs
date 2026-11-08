//! Audit probes (upload area). Each `audit_bug_*` test states ShareX's behaviour from the ShareX
//! behaviour and fails while Laqta diverges.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use futures_lite::future::block_on;
use laqta_http::{Body, Request, Response};
use laqta_ports::fakes::{CountingRng, FakePrompt};
use laqta_ports::{UploadError, UploadResult};
use laqta_upload::registry::Registry;
use laqta_upload::sxcu::{CustomUploader, Env, Input, SxcuError, build_request, parse_response};
use serde_json::json;

fn input(text: Option<&str>) -> Input<'_> {
    Input { file_name: "shot.png", data: b"PNG", mime: "image/png", text }
}

fn build_with(file: &str, text: Option<&str>, prompt: &FakePrompt) -> Result<Request, SxcuError> {
    let uploader = CustomUploader::parse(file)?;
    let rng = CountingRng::default();
    let env = Env { prompt, rng: &rng };
    block_on(build_request(&uploader, &input(text), &env))
}

fn build(file: &str, text: Option<&str>) -> Result<Request, SxcuError> {
    build_with(file, text, &FakePrompt::default())
}

fn read(file: &str, body: &str) -> Result<UploadResult, UploadError> {
    let uploader = CustomUploader::parse(file).map_err(UploadError::from)?;
    let prompt = FakePrompt::default();
    let rng = CountingRng::default();
    let env = Env { prompt: &prompt, rng: &rng };
    let response = Response { status: 200, url: "https://h/up".into(), headers: Vec::new(), body: body.as_bytes().to_vec() };
    block_on(parse_response(&uploader, &response, &input(None), &env))
}

/// ShareX `CustomUploaderItem.GetData` only replaces `{input}` and `{filename}` in `Data`; it never
/// runs the syntax parser on it, so a ShareX JSON body is written with plain braces. Laqta parses
/// `Data` as a template, so every ShareX-made JSON uploader fails.
#[test]
fn audit_bug_sharex_json_data_with_plain_braces() {
    let file = json!({ "RequestURL": "https://h/up", "Body": "JSON", "Data": r#"{"text":"{input}"}"# }).to_string();
    let request = build(&file, Some("hello")).expect("a ShareX JSON body must build");
    assert_eq!(request.body, Body::Bytes { data: br#"{"text":"hello"}"#.to_vec(), content_type: "application/json".into() });
}

/// `CustomUploaderFunctionInputBox` has the alias `prompt`.
#[test]
fn audit_bug_prompt_alias_of_inputbox() {
    let file = json!({ "RequestURL": "https://h/up", "Headers": { "X-Key": "{prompt:API key}" } }).to_string();
    let prompt = FakePrompt::default();
    prompt.texts.lock().unwrap().push_back("k123".into());
    let request = build_with(&file, None, &prompt).expect("{prompt:…} is a ShareX alias of {inputbox:…}");
    assert_eq!(request.headers, vec![("X-Key".to_string(), "k123".to_string())]);
}

/// `CustomUploaderFunctionRegex`: with no group argument the result is `match.Value`, the whole
/// match, even when the pattern has groups.
#[test]
fn audit_bug_regex_without_group_is_whole_match() {
    let file = json!({ "RequestURL": "https://h/up", "URL": "{regex:https://(x)[.]io/[a-z]+}" }).to_string();
    let result = read(&file, "see https://x.io/abc ok").unwrap();
    assert_eq!(result.url.as_deref(), Some("https://x.io/abc"));
}

/// `CustomUploaderFunctionRegex`: three parameters are `input|pattern|group`.
#[test]
fn audit_bug_regex_three_argument_form() {
    let file = json!({ "RequestURL": "https://h/up", "URL": "{regex:abc123|[0-9]+|0}" }).to_string();
    let result = read(&file, "zzz").unwrap();
    assert_eq!(result.url.as_deref(), Some("123"));
}

/// `CustomUploaderFunctionXml`: two parameters are `input|xpath`.
#[test]
fn audit_bug_xml_two_argument_form() {
    let file = json!({ "RequestURL": "https://h/up", "URL": "{xml:<a><b>v</b></a>|/a/b}" }).to_string();
    let result = read(&file, "<other/>").expect("{xml:input|xpath} is valid in ShareX");
    assert_eq!(result.url.as_deref(), Some("v"));
}

/// `CustomUploaderFunctionOutputBox`: one parameter is the text (title defaults to "Output").
#[test]
fn audit_bug_outputbox_single_argument_is_the_text() {
    let file = json!({ "RequestURL": "https://h/up", "Headers": { "X": "{outputbox:Your code is 42}" } }).to_string();
    let prompt = FakePrompt::default();
    build_with(&file, None, &prompt).unwrap();
    let shown = prompt.shown.lock().unwrap().clone();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].1, "Your code is 42", "the single argument must be shown as the text, got {shown:?}");
}

/// `CustomUploaderItem.GetRequestURL` parses with `URLEncode = true`, so `{input}` in the request URL
/// is percent-encoded and cannot add query parameters of its own.
#[test]
fn audit_bug_input_in_request_url_is_not_url_encoded() {
    let file = json!({ "RequestURL": "https://h/s?url={input}" }).to_string();
    let request = build(&file, Some("https://a.b/?x=1&y=2")).unwrap();
    assert!(!request.url.contains("&y=2"), "raw `&` from {{input}} splits the query: {}", request.url);
}

/// ShareX's `GetArguments`/`GetParameters`/`GetHeaders` run the NameParser first, so `%y` (year)
/// and other name-pattern tokens expand in `.sxcu` arguments.
#[test]
fn audit_bug_name_pattern_tokens_not_expanded_in_arguments() {
    let file = json!({ "RequestURL": "https://h/up", "Body": "FormURLEncoded", "Arguments": { "year": "%y" } }).to_string();
    let request = build(&file, None).unwrap();
    let Body::Form(pairs) = request.body else { panic!("form body") };
    assert_ne!(pairs[0].1, "%y", "ShareX expands %y to the year");
}

/// ShareX omits `Name` when it equals the request URL's host name (`ShouldSerializeName`), and
/// `ToString()` falls back to that host name. Such files are common; Laqta refuses them.
#[test]
fn audit_bug_nameless_sxcu_is_rejected() {
    let mut registry = Registry::new(Arc::new(FakePrompt::default()), Arc::new(CountingRng::default()));
    let file = json!({
        "Version": "17.0.0",
        "DestinationType": "ImageUploader",
        "RequestURL": "https://i.example.com/upload",
        "Body": "MultipartFormData",
        "FileFormName": "file"
    })
    .to_string();
    let id = registry.add_custom(&file).expect("a ShareX file without Name must import (named after the host)");
    assert!(id.contains("i.example.com"));
}

/// ShareX has `CustomURLSharingService`; a `.sxcu` with `DestinationType: URLSharingService` is a
/// valid uploader there. Laqta refuses it.
#[test]
fn audit_bug_url_sharing_sxcu_rejected() {
    let mut registry = Registry::new(Arc::new(FakePrompt::default()), Arc::new(CountingRng::default()));
    let file = json!({ "Name": "MySharer", "DestinationType": "URLSharingService", "RequestURL": "https://s.example/?u={input}" }).to_string();
    registry.add_custom(&file).expect("URL sharing .sxcu files are a ShareX destination type");
}
