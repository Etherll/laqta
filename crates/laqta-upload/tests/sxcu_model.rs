//! Black-box tests of the `.sxcu` file model: loading, ShareX 12 migration, the body types, and
//! reading responses from realistic files.
#![allow(clippy::unwrap_used)]

use futures_lite::future::block_on;
use laqta_http::{Body, Method, Part, Request, Response};
use laqta_ports::fakes::{CountingRng, FakePrompt};
use laqta_ports::{UploadError, UploadResult};
use laqta_upload::sxcu::{BodyType, CustomUploader, DestinationType, Env, Input, RequestMethod, SxcuError, build_request, parse_response};
use serde_json::json;

fn input(text: Option<&str>) -> Input<'_> {
    Input { file_name: "shot.png", data: b"PNG", mime: "image/png", text }
}

fn build(file: &str, text: Option<&str>) -> Result<Request, SxcuError> {
    let uploader = CustomUploader::parse(file)?;
    let prompt = FakePrompt::default();
    let rng = CountingRng::default();
    let env = Env { prompt: &prompt, rng: &rng };
    block_on(build_request(&uploader, &input(text), &env))
}

fn reply(status: u16, url: &str, body: &str) -> Response {
    Response { status, url: url.to_string(), headers: Vec::new(), body: body.as_bytes().to_vec() }
}

fn read(file: &str, response: &Response) -> Result<UploadResult, UploadError> {
    let uploader = CustomUploader::parse(file).map_err(UploadError::from)?;
    let prompt = FakePrompt::default();
    let rng = CountingRng::default();
    let env = Env { prompt: &prompt, rng: &rng };
    block_on(parse_response(&uploader, response, &input(None), &env))
}

#[test]
fn a_minimal_file_defaults_to_post_and_no_body() {
    let uploader = CustomUploader::parse(r#"{"RequestURL": "https://h/up"}"#).unwrap();
    assert_eq!(uploader.request_method, RequestMethod::Post);
    assert_eq!(uploader.body, BodyType::None);
    let request = build(r#"{"RequestURL": "https://h/up"}"#, None).unwrap();
    assert_eq!(request.method, Method::Post);
    assert_eq!(request.body, Body::Empty);
    assert!(request.query.is_empty() && request.headers.is_empty());
}

#[test]
fn request_methods_come_from_requestmethod_or_requesttype() {
    let put = build(r#"{"RequestURL": "https://h/", "RequestMethod": "PUT"}"#, None).unwrap();
    assert_eq!(put.method, Method::Put);
    let patch = build(r#"{"RequestURL": "https://h/", "RequestType": "patch"}"#, None).unwrap();
    assert_eq!(patch.method, Method::Patch);
    let delete = build(r#"{"RequestURL": "https://h/", "RequestMethod": "DELETE"}"#, None).unwrap();
    assert_eq!(delete.method, Method::Delete);
}

#[test]
fn destination_types_are_read_in_file_order() {
    let all = CustomUploader::parse(r#"{"RequestURL": "https://h/", "DestinationType": "ImageUploader, TextUploader, FileUploader, URLShortener, URLSharingService"}"#).unwrap();
    assert_eq!(
        all.destination_types(),
        vec![DestinationType::Image, DestinationType::Text, DestinationType::File, DestinationType::UrlShortener, DestinationType::UrlSharing]
    );
    let some = CustomUploader::parse(r#"{"RequestURL": "https://h/", "DestinationType": "TextUploader, Bogus, TextUploader"}"#).unwrap();
    assert_eq!(some.destination_types(), vec![DestinationType::Text]);
    let none = CustomUploader::parse(r#"{"RequestURL": "https://h/"}"#).unwrap();
    assert!(none.destination_types().is_empty());
}

// Raw text, not `json!`: serde_json's default map sorts keys, and these tests check file order.
#[test]
fn multipart_keeps_argument_order_and_adds_the_file_last() {
    let file = r#"{
        "RequestURL": "https://h/up",
        "FileFormName": "image",
        "Arguments": { "title": "{input}", "k": "v" }
    }"#;
    let request = build(file, Some("hello")).unwrap();
    assert_eq!(
        request.body,
        Body::Multipart(vec![
            Part::Text { name: "title".into(), value: "hello".into() },
            Part::Text { name: "k".into(), value: "v".into() },
            Part::File { name: "image".into(), file_name: "shot.png".into(), mime: "image/png".into(), data: b"PNG".to_vec() },
        ])
    );
}

#[test]
fn form_urlencoded_keeps_argument_order() {
    let file = r#"{ "RequestURL": "https://h/up", "Body": "FormURLEncoded", "Arguments": { "z": "1", "a": "{input}" } }"#;
    let request = build(file, Some("hello")).unwrap();
    assert_eq!(request.body, Body::Form(vec![("z".into(), "1".into()), ("a".into(), "hello".into())]));
}

#[test]
fn json_body_escapes_function_output() {
    // Literal braces are escaped; the `{input}` result is escaped as JSON text.
    let file = json!({ "RequestURL": "https://h/up", "Body": "JSON", "Data": r#"\{"text":"{input}"\}"# }).to_string();
    let text = "say \"hi\"\nnow";
    let request = build(&file, Some(text)).unwrap();
    let expected = r#"{"text":"say \"hi\"\nnow"}"#;
    assert_eq!(request.body, Body::Bytes { data: expected.as_bytes().to_vec(), content_type: "application/json".into() });
    let parsed: serde_json::Value = serde_json::from_slice(expected.as_bytes()).unwrap();
    assert_eq!(parsed["text"], text);
}

#[test]
fn json_body_takes_the_content_type_header_and_drops_the_header() {
    let file = json!({
        "RequestURL": "https://h/up",
        "Body": "JSON",
        "Headers": { "content-type": "application/vnd.test+json", "X-Keep": "1" },
        "Data": r#"\{"a":1\}"#
    })
    .to_string();
    let request = build(&file, None).unwrap();
    assert_eq!(request.body, Body::Bytes { data: br#"{"a":1}"#.to_vec(), content_type: "application/vnd.test+json".into() });
    assert_eq!(request.headers, vec![("X-Keep".to_string(), "1".to_string())]);
}

#[test]
fn xml_body_escapes_function_output() {
    let file = json!({ "RequestURL": "https://h/up", "Body": "XML", "Data": "<a>{input}</a>" }).to_string();
    let request = build(&file, Some("a<b & \"c\"")).unwrap();
    assert_eq!(request.body, Body::Bytes { data: b"<a>a&lt;b &amp; &quot;c&quot;</a>".to_vec(), content_type: "application/xml".into() });
}

#[test]
fn binary_body_is_the_raw_file_with_its_mime() {
    let file = json!({ "RequestURL": "https://h/up", "Body": "Binary" }).to_string();
    let request = build(&file, None).unwrap();
    assert_eq!(request.body, Body::Bytes { data: b"PNG".to_vec(), content_type: "image/png".into() });
}

#[test]
fn parameters_and_headers_are_evaluated_into_the_request() {
    let file = json!({
        "RequestURL": "https://h/up",
        "Parameters": { "key": "{input}" },
        "Headers": { "Authorization": "Client-ID abc" }
    })
    .to_string();
    let request = build(&file, Some("t")).unwrap();
    assert_eq!(request.query, vec![("key".to_string(), "t".to_string())]);
    assert_eq!(request.headers, vec![("Authorization".to_string(), "Client-ID abc".to_string())]);
}

#[test]
fn an_unknown_field_is_ignored() {
    assert!(CustomUploader::parse(r#"{"RequestURL": "https://h/", "SomethingNew": {"x": 1}}"#).is_ok());
}

#[test]
fn invalid_files_are_reported() {
    assert!(matches!(CustomUploader::parse("{"), Err(SxcuError::Json(_))));
    assert!(matches!(CustomUploader::parse(r#"{"Name": "no url"}"#), Err(SxcuError::Json(_))));
    assert!(matches!(CustomUploader::parse(r#"{"RequestURL": "https://h/", "Body": "Stream"}"#), Err(SxcuError::Unsupported(_))));
    assert!(matches!(CustomUploader::parse(r#"{"RequestURL": "https://h/", "RequestMethod": "HEAD"}"#), Err(SxcuError::Unsupported(_))));
}

/// A ShareX 12 Imgur-style file: `$name$` markers, `RequestType`, and a `RegexList`.
const SHAREX_12_FILE: &str = r#"{
    "Version": "12.0.1",
    "Name": "Old host",
    "DestinationType": "ImageUploader",
    "RequestType": "POST",
    "RequestURL": "https://up.example/api",
    "FileFormName": "image",
    "ResponseType": "Text",
    "URL": "$json:data.link$",
    "ThumbnailURL": "$json:data.thumb$",
    "DeletionURL": "https://up.example/delete/$json:data.id$",
    "ErrorMessage": "$json:error$",
    "RegexList": []
}"#;

#[test]
fn a_sharex_12_file_is_migrated_on_load() {
    let uploader = CustomUploader::parse(SHAREX_12_FILE).unwrap();
    assert_eq!(uploader.request_method, RequestMethod::Post);
    assert_eq!(uploader.body, BodyType::MultipartFormData);
    assert_eq!(uploader.url.as_deref(), Some("{json:data.link}"));

    let body = r#"{"data":{"link":"https://i.example/x.png","thumb":"https://i.example/t.png","id":"abc"}}"#;
    let result = read(SHAREX_12_FILE, &reply(200, "https://up.example/api", body)).unwrap();
    assert_eq!(result.url.as_deref(), Some("https://i.example/x.png"));
    assert_eq!(result.thumbnail_url.as_deref(), Some("https://i.example/t.png"));
    assert_eq!(result.deletion_url.as_deref(), Some("https://up.example/delete/abc"));
    assert_eq!(result.response, body);
}

#[test]
fn a_legacy_regex_marker_uses_the_regex_list_with_pipes_and_braces() {
    let file = json!({ "Version": "12.0.1", "RequestURL": "https://h/up", "URL": "$regex:1,1$", "RegexList": [r"(?:id|key)=(\d{2})"] }).to_string();
    let result = read(&file, &reply(200, "https://h/final", "key=42")).unwrap();
    assert_eq!(result.url.as_deref(), Some("42"));
}

#[test]
fn legacy_literal_braces_in_a_body_are_not_calls() {
    let file = json!({ "Version": "12.0.1", "RequestURL": "https://h/up", "Body": "JSON", "Data": r#"{"name":"$filename$"}"# }).to_string();
    let request = build(&file, None).unwrap();
    assert_eq!(request.body, Body::Bytes { data: br#"{"name":"shot.png"}"#.to_vec(), content_type: "application/json".into() });
}

#[test]
fn a_legacy_random_marker_uses_the_pipe_options() {
    let file = json!({ "Version": "12.0.1", "RequestURL": "https://h/$random:a|b$" }).to_string();
    assert_eq!(build(&file, None).unwrap().url, "https://h/a");
}

#[test]
fn a_legacy_redirection_reads_the_final_url() {
    let file = json!({ "Version": "12.0.1", "RequestURL": "https://h/up", "ResponseType": "Redirection" }).to_string();
    // The HTTP client follows redirects, so the engine sees the final response.
    let result = read(&file, &reply(200, "https://h/final", "")).unwrap();
    assert_eq!(result.url.as_deref(), Some("https://h/final"));
}

#[test]
fn a_modern_file_without_a_version_is_not_migrated() {
    // Braces in the templates are calls, not escapes, so nothing is rewritten.
    let file = json!({ "RequestURL": "https://h/up", "URL": "{json:data.link}" }).to_string();
    let result = read(&file, &reply(200, "https://h/up", r#"{"data":{"link":"https://i.example/a"}}"#)).unwrap();
    assert_eq!(result.url.as_deref(), Some("https://i.example/a"));
}

/// A ShareX 13 image host: the realistic shape of a simple `.sxcu` file.
const IMGUR_LIKE: &str = r#"{
    "Version": "13.0.1",
    "Name": "Imgur-like",
    "DestinationType": "ImageUploader, TextUploader, FileUploader",
    "RequestMethod": "POST",
    "RequestURL": "https://api.example.com/3/image",
    "Headers": { "Authorization": "Client-ID abc123" },
    "Body": "MultipartFormData",
    "FileFormName": "image",
    "URL": "{json:data.link}",
    "ThumbnailURL": "{json:data.link}",
    "DeletionURL": "https://example.com/delete/{json:data.deletehash}",
    "ErrorMessage": "{json:data.error}"
}"#;

#[test]
fn an_imgur_style_file_builds_and_reads() {
    let request = build(IMGUR_LIKE, None).unwrap();
    assert_eq!(request.method, Method::Post);
    assert_eq!(request.url, "https://api.example.com/3/image");
    assert_eq!(request.headers, vec![("Authorization".to_string(), "Client-ID abc123".to_string())]);
    assert_eq!(
        request.body,
        Body::Multipart(vec![Part::File { name: "image".into(), file_name: "shot.png".into(), mime: "image/png".into(), data: b"PNG".to_vec() }])
    );

    let body = r#"{"data":{"link":"https://i.example.com/q.png","deletehash":"dh1"},"success":true,"status":200}"#;
    let result = read(IMGUR_LIKE, &reply(200, "https://api.example.com/3/image", body)).unwrap();
    assert_eq!(result.url.as_deref(), Some("https://i.example.com/q.png"));
    assert_eq!(result.thumbnail_url.as_deref(), Some("https://i.example.com/q.png"));
    assert_eq!(result.deletion_url.as_deref(), Some("https://example.com/delete/dh1"));
}

#[test]
fn an_xml_response_file_reads_its_url() {
    let file = json!({ "RequestURL": "https://up.example/xml", "URL": "{xml:/rsp/image/url}" }).to_string();
    let body = r#"<?xml version="1.0"?><rsp status="ok"><image><url>https://x.example/a.png</url></image></rsp>"#;
    let result = read(&file, &reply(200, "https://up.example/xml", body)).unwrap();
    assert_eq!(result.url.as_deref(), Some("https://x.example/a.png"));
}

#[test]
fn a_regex_response_file_reads_its_url_from_the_whole_match() {
    // The host answers with prose; the URL is the first match, group 0.
    let file = json!({
        "RequestURL": "https://up.example/form",
        "Body": "MultipartFormData",
        "FileFormName": "file",
        "URL": r"{regex:https://up\.example/\w+|0}",
        "DeletionURL": r"https://up.example/del/{regex:key=(\w+)}"
    })
    .to_string();
    let body = "Uploaded to https://up.example/k3j9x (key=zz9) thanks";
    let result = read(&file, &reply(200, "https://up.example/form", body)).unwrap();
    assert_eq!(result.url.as_deref(), Some("https://up.example/k3j9x"));
    assert_eq!(result.deletion_url.as_deref(), Some("https://up.example/del/zz9"));
}

#[test]
fn a_non_2xx_status_uses_the_evaluated_error_message() {
    let file = json!({ "RequestURL": "https://h/up", "URL": "{json:url}", "ErrorMessage": "{json:error.message}" }).to_string();
    let result = read(&file, &reply(400, "https://h/up", r#"{"error":{"message":"File too big"}}"#));
    assert_eq!(result, Err(UploadError::Server { status: 400, body: "File too big".into() }));
}

#[test]
fn a_non_2xx_status_without_an_error_message_uses_the_body() {
    let file = json!({ "RequestURL": "https://h/up" }).to_string();
    let result = read(&file, &reply(503, "https://h/up", "busy"));
    assert_eq!(result, Err(UploadError::Server { status: 503, body: "busy".into() }));
}

#[test]
fn an_empty_url_template_uses_the_trimmed_body() {
    let file = json!({ "RequestURL": "https://h/up", "URL": "" }).to_string();
    let result = read(&file, &reply(200, "https://h/up", "  https://out.example/abc \n")).unwrap();
    assert_eq!(result.url.as_deref(), Some("https://out.example/abc"));
    assert_eq!(result.thumbnail_url, None);
    assert_eq!(result.deletion_url, None);
    assert_eq!(result.response, "  https://out.example/abc \n");
}

#[test]
fn a_blank_body_with_no_url_template_gives_no_url() {
    let file = json!({ "RequestURL": "https://h/up" }).to_string();
    let result = read(&file, &reply(200, "https://h/up", "   ")).unwrap();
    assert_eq!(result.url, None);
}
