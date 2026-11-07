//! Black-box tests of the `.sxcu` template language: one test per function, plus escapes,
//! nesting, phase rules and errors. Request templates go through `RequestURL`, response
//! templates through `URL`.
#![allow(clippy::unwrap_used)]

use std::sync::Mutex;

use futures_lite::future::block_on;
use laqta_http::{Request, Response};
use laqta_ports::fakes::{CountingRng, FakePrompt};
use laqta_ports::{BoxFuture, PortError, Prompt, UploadError, UploadResult};
use laqta_upload::sxcu::{CustomUploader, Env, Input, SxcuError, build_request, parse_response};
use serde_json::json;

fn input(text: Option<&str>) -> Input<'_> {
    Input { file_name: "shot.png", data: b"PNG", mime: "image/png", text }
}

fn prompt_with(choices: &[usize], texts: &[&str]) -> FakePrompt {
    FakePrompt {
        choices: Mutex::new(choices.iter().copied().collect()),
        texts: Mutex::new(texts.iter().map(|t| t.to_string()).collect()),
        ..Default::default()
    }
}

fn build(file: &str, text: Option<&str>, prompt: &FakePrompt) -> Result<Request, SxcuError> {
    let uploader = CustomUploader::parse(file)?;
    let rng = CountingRng::default();
    let env = Env { prompt, rng: &rng };
    block_on(build_request(&uploader, &input(text), &env))
}

/// The URL that `template` builds as a `RequestURL`.
fn url_with(template: &str, text: Option<&str>, prompt: &FakePrompt) -> Result<String, SxcuError> {
    let file = json!({ "RequestURL": template }).to_string();
    Ok(build(&file, text, prompt)?.url)
}

fn url(template: &str, text: Option<&str>) -> Result<String, SxcuError> {
    url_with(template, text, &FakePrompt::default())
}

fn response(status: u16, url: &str, headers: &[(&str, &str)], body: &str) -> Response {
    Response {
        status,
        url: url.to_string(),
        headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        body: body.as_bytes().to_vec(),
    }
}

/// The URL that `template` reads from `response` as its `URL`.
fn read_url(template: &str, response: &Response) -> Result<Option<String>, UploadError> {
    let file = json!({ "RequestURL": "https://h/up", "URL": template }).to_string();
    let uploader = CustomUploader::parse(&file).map_err(UploadError::from)?;
    let prompt = FakePrompt::default();
    let rng = CountingRng::default();
    let env = Env { prompt: &prompt, rng: &rng };
    block_on(parse_response(&uploader, response, &input(None), &env)).map(|result| result.url)
}

fn read_result(file: &str, response: &Response) -> Result<UploadResult, UploadError> {
    let uploader = CustomUploader::parse(file).map_err(UploadError::from)?;
    let prompt = FakePrompt::default();
    let rng = CountingRng::default();
    let env = Env { prompt: &prompt, rng: &rng };
    block_on(parse_response(&uploader, response, &input(None), &env))
}

fn assert_send<T: Send>(_: &T) {}

#[test]
fn input_is_the_text_or_empty() {
    assert_eq!(url("https://h/{input}", Some("abc")).unwrap(), "https://h/abc");
    assert_eq!(url("https://h/{input}", None).unwrap(), "https://h/");
}

#[test]
fn filename_is_the_input_file_name() {
    assert_eq!(url("https://h/{filename}", None).unwrap(), "https://h/shot.png");
}

#[test]
fn random_picks_by_the_rng_in_order() {
    // CountingRng yields 0, 1, 2, 3: indices 0, 1, 2, 0 over three options.
    assert_eq!(url("https://h/{random:a|b|c}{random:a|b|c}{random:a|b|c}{random:a|b|c}", None).unwrap(), "https://h/abca");
}

#[test]
fn random_without_options_is_a_syntax_error() {
    assert!(matches!(url("https://h/{random}", None), Err(SxcuError::Syntax(_))));
}

#[test]
fn select_returns_the_chosen_option() {
    let prompt = prompt_with(&[2], &[]);
    assert_eq!(url_with("https://h/{select:a|b|c}", None, &prompt).unwrap(), "https://h/c");
}

#[test]
fn select_rejects_an_out_of_range_answer() {
    let prompt = prompt_with(&[7], &[]);
    assert!(matches!(url_with("https://h/{select:a|b|c}", None, &prompt), Err(SxcuError::Syntax(_))));
}

#[test]
fn inputbox_uses_the_answer_or_the_default() {
    let answered = prompt_with(&[], &["typed"]);
    assert_eq!(url_with("https://h/{inputbox:Name|fallback}", None, &answered).unwrap(), "https://h/typed");
    // No queued answer: the fake returns the default.
    assert_eq!(url("https://h/{inputbox:Name|fallback}", None).unwrap(), "https://h/fallback");
}

#[test]
fn outputbox_shows_text_and_evaluates_to_empty() {
    let prompt = FakePrompt::default();
    assert_eq!(url_with("https://h/{outputbox:Title|Some text}", None, &prompt).unwrap(), "https://h/");
    assert_eq!(*prompt.shown.lock().unwrap(), vec![("Title".to_string(), "Some text".to_string())]);
}

#[test]
fn base64_encodes_its_argument() {
    assert_eq!(url("https://h/{base64:{input}}", Some("hi")).unwrap(), "https://h/aGk=");
}

#[test]
fn base64_takes_a_nested_call_whole() {
    // The `|` inside the nested call separates its options, not base64's arguments.
    assert_eq!(url("https://h/{base64:{random:a|b}}", None).unwrap(), "https://h/YQ==");
}

#[test]
fn response_is_the_trimmed_body() {
    let body = response(200, "https://h/up", &[], "https://x.example/y\n");
    assert_eq!(read_url("{response}", &body).unwrap().as_deref(), Some("https://x.example/y"));
}

#[test]
fn responseurl_is_the_final_url() {
    let body = response(200, "https://final.example/z", &[], "");
    assert_eq!(read_url("{responseurl}", &body).unwrap().as_deref(), Some("https://final.example/z"));
}

#[test]
fn header_looks_up_case_insensitively() {
    let reply = response(200, "https://h/up", &[("x-link", "https://hdr.example/1")], "");
    assert_eq!(read_url("{header:X-Link}", &reply).unwrap().as_deref(), Some("https://hdr.example/1"));
    assert_eq!(read_url("{header:X-Missing}", &reply).unwrap(), None);
}

const JSON_BODY: &str = r#"{"data":{"link":"https://i.example/q.png","count":3,"files":[{"url":"https://f.example/0"}],"name":"n"}}"#;

#[test]
fn json_reads_a_path_in_either_form() {
    let reply = response(200, "https://h/up", &[], JSON_BODY);
    assert_eq!(read_url("{json:data.link}", &reply).unwrap().as_deref(), Some("https://i.example/q.png"));
    assert_eq!(read_url("{json:$.data.link}", &reply).unwrap().as_deref(), Some("https://i.example/q.png"));
    assert_eq!(read_url("{json:data.files[0].url}", &reply).unwrap().as_deref(), Some("https://f.example/0"));
}

#[test]
fn json_returns_non_strings_as_json_text() {
    let reply = response(200, "https://h/up", &[], JSON_BODY);
    assert_eq!(read_url("{json:data.count}", &reply).unwrap().as_deref(), Some("3"));
    assert_eq!(read_url("{json:data.files}", &reply).unwrap().as_deref(), Some(r#"[{"url":"https://f.example/0"}]"#));
}

#[test]
fn json_without_a_match_is_empty() {
    let reply = response(200, "https://h/up", &[], JSON_BODY);
    assert_eq!(read_url("{json:data.missing}", &reply).unwrap(), None);
}

#[test]
fn json_accepts_an_explicit_input() {
    let reply = response(200, "https://h/up", &[], "not json");
    let explicit = r#"{"a":{"b":"https://explicit.example/1"}}"#;
    let template = format!("{{json:{}|a.b}}", explicit.replace('{', r"\{").replace('}', r"\}"));
    assert_eq!(read_url(&template, &reply).unwrap().as_deref(), Some("https://explicit.example/1"));
}

#[test]
fn json_on_a_body_that_is_not_json_is_a_bad_response() {
    let reply = response(200, "https://h/up", &[], "<html>");
    assert!(matches!(read_url("{json:a}", &reply), Err(UploadError::BadResponse(_))));
}

const XML_BODY: &str = r#"<?xml version="1.0"?><rsp><image id="7"><url>https://x.example/a.png</url></image><image id="8"><url>https://x.example/b.png</url></image></rsp>"#;

#[test]
fn xml_follows_absolute_paths_and_double_slash() {
    let reply = response(200, "https://h/up", &[], XML_BODY);
    assert_eq!(read_url("{xml:/rsp/image/url}", &reply).unwrap().as_deref(), Some("https://x.example/a.png"));
    assert_eq!(read_url("{xml://url}", &reply).unwrap().as_deref(), Some("https://x.example/a.png"));
}

#[test]
fn xml_supports_positions_attributes_text_and_wildcards() {
    let reply = response(200, "https://h/up", &[], XML_BODY);
    assert_eq!(read_url("{xml:/rsp/image[2]/url}", &reply).unwrap().as_deref(), Some("https://x.example/b.png"));
    assert_eq!(read_url("{xml:/rsp/image[1]/@id}", &reply).unwrap().as_deref(), Some("7"));
    assert_eq!(read_url("{xml:/rsp/image/url/text()}", &reply).unwrap().as_deref(), Some("https://x.example/a.png"));
    assert_eq!(read_url("{xml:/rsp/*/url}", &reply).unwrap().as_deref(), Some("https://x.example/a.png"));
    assert_eq!(read_url("{xml:/rsp/nothing}", &reply).unwrap(), None);
}

#[test]
fn xml_unsupported_xpath_is_reported() {
    let reply = response(200, "https://h/up", &[], XML_BODY);
    let result = read_url("{xml:/rsp/image[@id='7']/url}", &reply);
    assert!(matches!(result, Err(UploadError::Unsupported(_))), "got {result:?}");
}

#[test]
fn xml_on_a_body_that_is_not_xml_is_a_bad_response() {
    let reply = response(200, "https://h/up", &[], "plain text");
    assert!(matches!(read_url("{xml:/a}", &reply), Err(UploadError::BadResponse(_))));
}

const REGEX_BODY: &str = "id=42; name=x";

#[test]
fn regex_takes_group_one_by_default() {
    let reply = response(200, "https://h/up", &[], REGEX_BODY);
    assert_eq!(read_url(r"{regex:id=(\d+)}", &reply).unwrap().as_deref(), Some("42"));
}

#[test]
fn regex_takes_a_numbered_or_named_group() {
    let reply = response(200, "https://h/up", &[], REGEX_BODY);
    assert_eq!(read_url(r"{regex:id=(\d+)|0}", &reply).unwrap().as_deref(), Some("id=42"));
    assert_eq!(read_url(r"{regex:name=(?<n>\w)|n}", &reply).unwrap().as_deref(), Some("x"));
}

#[test]
fn regex_without_groups_takes_the_whole_match() {
    let reply = response(200, "https://h/up", &[], "see https://up.example/k3j9x now");
    assert_eq!(read_url(r"{regex:https://\S+}", &reply).unwrap().as_deref(), Some("https://up.example/k3j9x"));
}

#[test]
fn regex_without_a_match_is_empty() {
    let reply = response(200, "https://h/up", &[], REGEX_BODY);
    assert_eq!(read_url(r"{regex:zzz(\d)}", &reply).unwrap(), None);
}

#[test]
fn regex_rejects_a_group_that_does_not_exist() {
    let reply = response(200, "https://h/up", &[], REGEX_BODY);
    assert!(matches!(read_url(r"{regex:id=(\d+)|5}", &reply), Err(UploadError::BadResponse(_))));
}

#[test]
fn escapes_make_braces_pipes_and_backslashes_literal() {
    assert_eq!(url(r"https://h/\{a\}\|b\\c", None).unwrap(), r"https://h/{a}|b\c");
    assert_eq!(url(r"https://h/\{input\}", Some("x")).unwrap(), "https://h/{input}");
}

#[test]
fn a_backslash_before_other_characters_is_kept() {
    assert_eq!(url(r"https://h/\d\.x", None).unwrap(), r"https://h/\d\.x");
}

#[test]
fn a_lone_close_brace_or_pipe_outside_a_call_is_literal() {
    assert_eq!(url("https://h/}|", None).unwrap(), "https://h/}|");
}

#[test]
fn an_unknown_function_is_a_syntax_error_naming_it() {
    match url("https://h/{nope}", None) {
        Err(SxcuError::Syntax(message)) => assert!(message.contains("nope"), "{message}"),
        other => panic!("expected a syntax error, got {other:?}"),
    }
}

#[test]
fn an_unclosed_call_is_a_syntax_error() {
    assert!(matches!(url("https://h/{input", None), Err(SxcuError::Syntax(_))));
}

#[test]
fn response_functions_are_invalid_in_a_request() {
    for template in ["{response}", "{responseurl}", "{header:X}", "{json:a}", "{xml:/a}", "{regex:a}"] {
        assert!(matches!(url(template, None), Err(SxcuError::Syntax(_))), "{template} should be a syntax error in a request");
    }
}

struct CancellingPrompt;

impl Prompt for CancellingPrompt {
    fn ask_text(&self, _title: &str, _default: &str) -> BoxFuture<'_, Result<String, PortError>> {
        Box::pin(async { Err(PortError::Cancelled) })
    }
    fn choose(&self, _title: &str, _options: &[String]) -> BoxFuture<'_, Result<usize, PortError>> {
        Box::pin(async { Err(PortError::Cancelled) })
    }
    fn show_text(&self, _title: &str, _text: &str) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Err(PortError::Cancelled) })
    }
    fn confirm(&self, _title: &str, _text: &str) -> BoxFuture<'_, Result<bool, PortError>> {
        Box::pin(async { Err(PortError::Cancelled) })
    }
}

#[test]
fn a_cancelled_prompt_reaches_upload_cancelled() {
    let file = json!({ "RequestURL": "https://h/{select:a|b}" }).to_string();
    let uploader = CustomUploader::parse(&file).unwrap();
    let rng = CountingRng::default();
    let prompt = CancellingPrompt;
    let env = Env { prompt: &prompt, rng: &rng };
    let error = block_on(build_request(&uploader, &input(None), &env)).unwrap_err();
    assert_eq!(error, SxcuError::Cancelled);
    assert_eq!(UploadError::from(error), UploadError::Cancelled);
}

#[test]
fn a_failing_response_template_falls_back_to_the_body() {
    let reply = response(500, "https://h/up", &[], "<html>oops</html>");
    let file = json!({ "RequestURL": "https://h/up", "ErrorMessage": "{json:error}" }).to_string();
    let result = read_result(&file, &reply);
    assert_eq!(result, Err(UploadError::Server { status: 500, body: "<html>oops</html>".into() }));
}

#[test]
fn the_request_and_response_futures_are_send() {
    let uploader = CustomUploader::parse(&json!({ "RequestURL": "https://h/{input}", "URL": "{json:a}" }).to_string()).unwrap();
    let prompt = FakePrompt::default();
    let rng = CountingRng::default();
    let env = Env { prompt: &prompt, rng: &rng };
    let upload = Input { file_name: "f", data: b"x", mime: "text/plain", text: Some("abc") };
    let request = build_request(&uploader, &upload, &env);
    assert_send(&request);
    let reply = response(200, "https://h/up", &[], "{}");
    let no_text = input(None);
    let result = parse_response(&uploader, &reply, &no_text, &env);
    assert_send(&result);
}
