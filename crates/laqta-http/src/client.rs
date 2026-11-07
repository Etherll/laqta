//! Maps a `Request` onto the shared `reqwest` client and back into a `Response`.

use std::convert::Infallible;
use std::error::Error as _;
use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use reqwest::multipart::{Form, Part as FormPart};

use crate::{Body, HttpError, Method, Part, Progress, Request, Response};

/// Size of each chunk handed to the transport when a body reports progress.
const CHUNK: usize = 64 * 1024;

pub(crate) async fn execute(
    client: &Client,
    request: Request,
    progress: Option<Progress>,
) -> Result<Response, HttpError> {
    let url = checked_url(&request.url, &request.query)?;
    let mut builder = client.request(reqwest_method(request.method), url.as_str());
    if request.timeout_secs > 0 {
        builder = builder.timeout(Duration::from_secs(request.timeout_secs));
    }

    // The body's content type replaces any Content-Type the caller set, so the request never
    // carries two of them.
    let body_sets_type = matches!(
        request.body,
        Body::Bytes { .. } | Body::Form(_) | Body::Multipart(_)
    );
    for (name, value) in &request.headers {
        if body_sets_type && name.eq_ignore_ascii_case("content-type") {
            continue;
        }
        builder = builder.header(name.as_str(), value.as_str());
    }

    // Progress reported once the request has been sent: the total for a multipart body, whose
    // encoded length reqwest does not expose.
    let mut completion = None;
    builder = match request.body {
        Body::Empty => builder,
        Body::Bytes { data, content_type } => {
            let builder = builder.header("content-type", content_type.as_str());
            match &progress {
                Some(report) if !data.is_empty() => {
                    let total = data.len() as u64;
                    builder
                        .header("content-length", total.to_string())
                        .body(streamed(data, Arc::clone(report)))
                }
                Some(_) => {
                    completion = Some(0);
                    builder.body(data)
                }
                None => builder.body(data),
            }
        }
        Body::Form(pairs) => builder
            .header("content-type", "application/x-www-form-urlencoded")
            .body(encode_form(&pairs)),
        Body::Multipart(parts) => {
            let (form, total) = multipart_form(parts)?;
            if let Some(report) = &progress {
                report(0, Some(total));
                completion = Some(total);
            }
            builder.multipart(form)
        }
    };

    let response = builder.send().await.map_err(map_error)?;
    if let (Some(report), Some(total)) = (&progress, completion) {
        report(total, Some(total));
    }

    let status = response.status().as_u16();
    let url = response.url().to_string();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let body = response.bytes().await.map_err(map_error)?.to_vec();
    Ok(Response {
        status,
        url,
        headers,
        body,
    })
}

/// Maps a `reqwest` error: a timeout, an invalid request, or anything else as a network failure.
pub(crate) fn map_error(err: reqwest::Error) -> HttpError {
    if err.is_timeout() {
        HttpError::Timeout
    } else if err.is_builder() {
        HttpError::Invalid(describe(&err))
    } else {
        HttpError::Network(describe(&err))
    }
}

/// The error message with its causes, so "connection refused" is not lost.
fn describe(err: &reqwest::Error) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

fn checked_url(raw: &str, query: &[(String, String)]) -> Result<url::Url, HttpError> {
    let mut url =
        url::Url::parse(raw).map_err(|err| HttpError::Invalid(format!("{raw}: {err}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(HttpError::Invalid(format!(
            "{raw}: only http and https are supported"
        )));
    }
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    Ok(url)
}

fn reqwest_method(method: Method) -> reqwest::Method {
    match method {
        Method::Get => reqwest::Method::GET,
        Method::Post => reqwest::Method::POST,
        Method::Put => reqwest::Method::PUT,
        Method::Patch => reqwest::Method::PATCH,
        Method::Delete => reqwest::Method::DELETE,
    }
}

fn encode_form(pairs: &[(String, String)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}

fn multipart_form(parts: Vec<Part>) -> Result<(Form, u64), HttpError> {
    let mut form = Form::new();
    let mut total = 0u64;
    for part in parts {
        match part {
            Part::Text { name, value } => {
                total += value.len() as u64;
                form = form.text(name, value);
            }
            Part::File {
                name,
                file_name,
                mime,
                data,
            } => {
                total += data.len() as u64;
                let part = FormPart::bytes(data)
                    .file_name(file_name)
                    .mime_str(&mime)
                    .map_err(|err| HttpError::Invalid(format!("content type {mime}: {err}")))?;
                form = form.part(name, part);
            }
        }
    }
    Ok((form, total))
}

/// A body that reports `(sent, total)` as each chunk is handed to the transport. The caller sets
/// an exact `Content-Length` alongside it, so the server never sees chunked encoding.
fn streamed(data: Vec<u8>, report: Progress) -> reqwest::Body {
    let total = data.len() as u64;
    let chunks = futures_lite::stream::unfold(
        (data, 0usize, report),
        move |(data, offset, report)| async move {
            if offset >= data.len() {
                return None;
            }
            let end = (offset + CHUNK).min(data.len());
            let chunk = data[offset..end].to_vec();
            report(end as u64, Some(total));
            Some((Ok::<Vec<u8>, Infallible>(chunk), (data, end, report)))
        },
    );
    reqwest::Body::wrap_stream(chunks)
}
