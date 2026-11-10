//! The `.sxcu` custom uploader engine (ADR 004, ADR 014). It parses ShareX `.sxcu` files, builds
//! the `laqta_http::Request` for an upload, and reads the `laqta_http::Response`. It sends nothing.

mod migrate;
mod model;
mod syntax;
mod xml;

use laqta_http::{Body, Method, Part, Request, Response};
use laqta_ports::{Prompt, Rng, UploadError, UploadResult};

pub use model::{BodyType, CustomUploader, DestinationType, RequestMethod};
use syntax::{Ctx, Escape};

/// What one upload sends: the file (or text) and its type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Input<'a> {
    pub file_name: &'a str,
    pub data: &'a [u8],
    pub mime: &'a str,
    /// The text or URL for text uploads and shorteners.
    pub text: Option<&'a str>,
}

/// The questions a template can ask and the randomness it can draw on.
#[derive(Clone, Copy)]
pub struct Env<'a> {
    pub prompt: &'a dyn Prompt,
    pub rng: &'a dyn Rng,
}

impl std::fmt::Debug for Env<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Env").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SxcuError {
    /// The `.sxcu` file, or a response read as JSON or XML, is not valid.
    #[error("JSON: {0}")]
    Json(String),
    /// A template is malformed, or uses a function the place does not allow.
    #[error("syntax: {0}")]
    Syntax(String),
    /// The file uses something this engine does not implement.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The user cancelled a prompt. Not in the contract's three variants: prompt cancellation has
    /// to reach `UploadError::Cancelled` and cannot be carried by any of them.
    #[error("cancelled")]
    Cancelled,
}

impl From<SxcuError> for UploadError {
    fn from(error: SxcuError) -> Self {
        match error {
            SxcuError::Json(message) | SxcuError::Syntax(message) => UploadError::BadResponse(message),
            SxcuError::Unsupported(message) => UploadError::Unsupported(message),
            SxcuError::Cancelled => UploadError::Cancelled,
        }
    }
}

/// Builds the request for one upload. Templates in the URL, `Parameters`, `Headers`, `Arguments`
/// and `Data` are evaluated in that order. The response-only functions are errors here.
pub async fn build_request(u: &CustomUploader, input: &Input<'_>, env: &Env<'_>) -> Result<Request, SxcuError> {
    let ctx = Ctx { input, env, response: None };
    let url = syntax::render(&ctx, &u.request_url, Escape::Plain).await?;
    let mut request = Request::new(method(u.request_method), url);
    request.query = rendered(&ctx, &u.parameters).await?;
    let mut headers = rendered(&ctx, &u.headers).await?;
    request.body = match u.body {
        BodyType::None => Body::Empty,
        BodyType::MultipartFormData => {
            let mut parts: Vec<Part> =
                rendered(&ctx, &u.arguments).await?.into_iter().map(|(name, value)| Part::Text { name, value }).collect();
            if let Some(name) = &u.file_form_name {
                parts.push(Part::File {
                    name: name.clone(),
                    file_name: input.file_name.to_string(),
                    mime: input.mime.to_string(),
                    data: input.data.to_vec(),
                });
            }
            Body::Multipart(parts)
        }
        BodyType::FormUrlEncoded => Body::Form(rendered(&ctx, &u.arguments).await?),
        BodyType::Json => text_body(&ctx, u, &mut headers, "application/json", Escape::Json).await?,
        BodyType::Xml => text_body(&ctx, u, &mut headers, "application/xml", Escape::Xml).await?,
        BodyType::Binary => {
            let content_type = take_content_type(&mut headers).unwrap_or_else(|| input.mime.to_string());
            Body::Bytes { data: input.data.to_vec(), content_type }
        }
    };
    request.headers = headers;
    Ok(request)
}

/// Reads the response into an upload result. A non-2xx status is `Server`, with the evaluated
/// `ErrorMessage` as its body when there is one. An empty URL template means the trimmed body.
pub async fn parse_response(
    u: &CustomUploader,
    response: &Response,
    input: &Input<'_>,
    env: &Env<'_>,
) -> Result<UploadResult, UploadError> {
    let ctx = Ctx { input, env, response: Some(response) };
    let body = response.text();
    if !(200..300).contains(&response.status) {
        let body = server_message(u, &ctx, &body).await;
        return Err(UploadError::Server { status: response.status, body });
    }
    let url = match template(u.url.as_deref()) {
        Some(template) => evaluated(&ctx, template).await?,
        None => non_empty(body.trim()),
    };
    let thumbnail_url = match template(u.thumbnail_url.as_deref()) {
        Some(template) => evaluated(&ctx, template).await?,
        None => None,
    };
    let deletion_url = match template(u.deletion_url.as_deref()) {
        Some(template) => evaluated(&ctx, template).await?,
        None => None,
    };
    Ok(UploadResult { url, thumbnail_url, deletion_url, response: body })
}

fn method(method: RequestMethod) -> Method {
    match method {
        RequestMethod::Get => Method::Get,
        RequestMethod::Post => Method::Post,
        RequestMethod::Put => Method::Put,
        RequestMethod::Patch => Method::Patch,
        RequestMethod::Delete => Method::Delete,
    }
}

/// Evaluates every pair's key and value.
async fn rendered(ctx: &Ctx<'_>, pairs: &[(String, String)]) -> Result<Vec<(String, String)>, SxcuError> {
    let mut out = Vec::with_capacity(pairs.len());
    for (key, value) in pairs {
        out.push((syntax::render(ctx, key, Escape::Plain).await?, syntax::render(ctx, value, Escape::Plain).await?));
    }
    Ok(out)
}

/// The body of a `JSON` or `XML` request. A `Content-Type` header is moved into the body.
async fn text_body(
    ctx: &Ctx<'_>,
    u: &CustomUploader,
    headers: &mut Vec<(String, String)>,
    default_type: &str,
    escape: Escape,
) -> Result<Body, SxcuError> {
    let data = match &u.data {
        Some(template) => syntax::render_data(ctx, template, escape).await?,
        None => String::new(),
    };
    let content_type = take_content_type(headers).unwrap_or_else(|| default_type.to_string());
    Ok(Body::Bytes { data: data.into_bytes(), content_type })
}

fn take_content_type(headers: &mut Vec<(String, String)>) -> Option<String> {
    let index = headers.iter().position(|(name, _)| name.eq_ignore_ascii_case("content-type"))?;
    Some(headers.remove(index).1)
}

/// The error body: the evaluated `ErrorMessage`, or the raw body when there is none or it fails.
async fn server_message(u: &CustomUploader, ctx: &Ctx<'_>, body: &str) -> String {
    let Some(template) = template(u.error_message.as_deref()) else {
        return body.to_string();
    };
    match syntax::render(ctx, template, Escape::Plain).await {
        Ok(message) if !message.trim().is_empty() => message.trim().to_string(),
        _ => body.to_string(),
    }
}

/// A template that is present and not blank.
fn template(source: Option<&str>) -> Option<&str> {
    source.filter(|source| !source.trim().is_empty())
}

/// Evaluates a result template. Blank output is `None`.
async fn evaluated(ctx: &Ctx<'_>, template: &str) -> Result<Option<String>, SxcuError> {
    let value = syntax::render(ctx, template, Escape::Plain).await?;
    Ok(non_empty(value.trim()))
}

fn non_empty(value: &str) -> Option<String> {
    if value.is_empty() { None } else { Some(value.to_string()) }
}
