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

pub(crate) async fn execute(client: &Client, request: Request, progress: Option<Progress>) -> Result<Response, HttpError> {
    let request_limit = request.max_response_bytes.unwrap_or(u64::MAX);
    let url = checked_url(&request.url, &request.query)?;
    let mut builder = client.request(reqwest_method(request.method), url.as_str());
    if request.timeout_secs > 0 {
        builder = builder.timeout(Duration::from_secs(request.timeout_secs));
    }

    // The body's content type replaces any Content-Type the caller set, so the request never
    // carries two of them.
    let body_sets_type = matches!(request.body, Body::Bytes { .. } | Body::Form(_) | Body::Multipart(_));
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
                    builder.header("content-length", total.to_string()).body(streamed(data, Arc::clone(report)))
                }
                Some(_) => {
                    completion = Some(0);
                    builder.body(data)
                }
                None => builder.body(data),
            }
        }
        Body::Form(pairs) => builder.header("content-type", "application/x-www-form-urlencoded").body(encode_form(&pairs)),
        Body::Multipart(parts) => {
            let (form, total) = multipart_form(parts)?;
            if let Some(report) = &progress {
                report(0, Some(total));
                completion = Some(total);
            }
            builder.multipart(form)
        }
    };

    let mut response = builder.send().await.map_err(map_error)?;
    if let (Some(report), Some(total)) = (&progress, completion) {
        report(total, Some(total));
    }

    let status = response.status().as_u16();
    let url = response.url().to_string();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| (name.as_str().to_owned(), String::from_utf8_lossy(value.as_bytes()).into_owned()))
        .collect();
    let body = read_body(&mut response, request_limit).await?;
    Ok(Response { status, url, headers, body })
}

/// Reads a response body of at most `limit` bytes. A declared length over the limit fails before
/// anything is read; a streamed body fails once it grows past the limit.
async fn read_body(response: &mut reqwest::Response, limit: u64) -> Result<Vec<u8>, HttpError> {
    if response.content_length().is_some_and(|declared| declared > limit) {
        return Err(too_large(limit));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(map_error)? {
        append_capped(&mut body, &chunk, limit)?;
    }
    Ok(body)
}

/// Appends `chunk` to `body`, failing when the body would pass `limit` bytes.
fn append_capped(body: &mut Vec<u8>, chunk: &[u8], limit: u64) -> Result<(), HttpError> {
    if body.len() as u64 + chunk.len() as u64 > limit {
        return Err(too_large(limit));
    }
    body.extend_from_slice(chunk);
    Ok(())
}

fn too_large(limit: u64) -> HttpError {
    HttpError::Network(format!("response body is larger than {limit} bytes"))
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
    let mut url = url::Url::parse(raw).map_err(|err| HttpError::Invalid(format!("{raw}: {err}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(HttpError::Invalid(format!("{raw}: only http and https are supported")));
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
    url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish()
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
            Part::File { name, file_name, mime, data } => {
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
    let chunks = futures_lite::stream::unfold((data, 0usize, report), move |(data, offset, report)| async move {
        if offset >= data.len() {
            return None;
        }
        let end = (offset + CHUNK).min(data.len());
        let chunk = data[offset..end].to_vec();
        report(end as u64, Some(total));
        Some((Ok::<Vec<u8>, Infallible>(chunk), (data, end, report)))
    });
    reqwest::Body::wrap_stream(chunks)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;

    /// Serves `response` to the first connection on a loopback port and returns its URL.
    fn serve(response: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let _ = stream.write_all(response);
        });
        url
    }

    fn client() -> Client {
        Client::builder().no_proxy().build().unwrap()
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(future)
    }

    /// Fetches `url` and reads the body under `limit`, as `execute` does.
    fn read_with_limit(url: &str, limit: u64) -> Result<Vec<u8>, HttpError> {
        block_on(async {
            let mut response = client().get(url).send().await.map_err(map_error)?;
            read_body(&mut response, limit).await
        })
    }

    #[test]
    fn append_capped_accepts_up_to_the_limit() {
        let mut body = Vec::new();
        append_capped(&mut body, b"12", 4).unwrap();
        append_capped(&mut body, b"34", 4).unwrap();
        assert_eq!(body, b"1234");
        assert_eq!(append_capped(&mut body, b"5", 4), Err(too_large(4)));
        assert_eq!(body, b"1234");
    }

    #[test]
    fn declared_length_over_the_limit_fails_before_reading() {
        let url = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n");
        assert_eq!(read_with_limit(&url, 10), Err(too_large(10)));
    }

    #[test]
    fn streamed_body_over_the_limit_fails() {
        let url = serve(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\nworld!\r\n0\r\n\r\n");
        assert_eq!(read_with_limit(&url, 8), Err(too_large(8)));
    }

    #[test]
    fn streamed_body_within_the_limit_is_read_whole() {
        let url = serve(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\nworld!\r\n0\r\n\r\n");
        assert_eq!(read_with_limit(&url, 11), Ok(b"helloworld!".to_vec()));
    }

    #[test]
    fn execute_returns_the_body_of_a_small_response() {
        let url = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello");
        let response = block_on(execute(&client(), Request::new(Method::Get, url), None)).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"hello");
    }
}
