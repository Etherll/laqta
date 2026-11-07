//! SendSpace anonymous upload over the XML REST API.
//! `anonymous.uploadGetInfo` replies with an `<upload>` element whose values are attributes. The
//! file is posted to its `url` with the form fields `MAX_FILE_SIZE`, `UPLOAD_IDENTIFIER` and
//! `extra_info`, and the reply's `download_url` is the link. The API key is read from `SecretStore`
//! under `laqta/sendspace/api_key`.
//!
//! With a user name, Laqta signs in first: `auth.createToken`, then `auth.login` with
//! `tokened_password = md5(token + md5(password))` (lowercase hex), then `upload.getInfo` with the
//! `session_key`; the password is read from `SecretStore` under `laqta/sendspace/password`.

use std::fmt;
use std::sync::Arc;

use laqta_http::{Body, Method, Part, Request};
use laqta_ports::BoxFuture;
use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadError, UploadRequest, UploadResult};
use roxmltree::{Document, Node};

use crate::hosts::{call, data_bytes, port_error, upload_call};

/// Where the API key lives in `SecretStore`.
pub const API_KEY_KEY: &str = "laqta/sendspace/api_key";
pub const PASSWORD_KEY: &str = "laqta/sendspace/password";
/// The production REST endpoint.
pub const SENDSPACE_API: &str = "http://api.sendspace.com/rest/";

const API_VERSION: &str = "1.0";
const APP_VERSION: &str = "1.0";

/// One SendSpace destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendSpaceConfig {
    /// The REST endpoint, `SENDSPACE_API` in production.
    pub api_base: String,
    /// The sign-in user name. Empty for an anonymous upload.
    pub username: String,
}

/// The SendSpace destination.
#[derive(Clone)]
pub struct SendSpace {
    config: SendSpaceConfig,
    secrets: Arc<dyn SecretStore>,
}

impl SendSpace {
    pub fn new(config: SendSpaceConfig, secrets: Arc<dyn SecretStore>) -> Self {
        SendSpace { config, secrets }
    }
}

impl fmt::Debug for SendSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendSpace").field("config", &self.config).finish_non_exhaustive()
    }
}

impl Destination for SendSpace {
    fn id(&self) -> &str {
        "sendspace"
    }

    fn name(&self) -> &str {
        "SendSpace"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(transfer(self.config.clone(), Arc::clone(&self.secrets), request, progress))
    }
}

async fn transfer(
    config: SendSpaceConfig,
    secrets: Arc<dyn SecretStore>,
    request: UploadRequest,
    progress: ProgressCallback,
) -> Result<UploadResult, UploadError> {
    let api_key = secrets
        .get(API_KEY_KEY)
        .map_err(port_error)?
        .filter(|key| !key.is_empty())
        .ok_or_else(|| UploadError::Auth(format!("no secret is stored under {API_KEY_KEY}")))?;

    let mut info = Request::new(Method::Get, config.api_base.clone());
    info.query = if config.username.is_empty() {
        vec![
            ("method".to_owned(), "anonymous.uploadGetInfo".to_owned()),
            ("speed_limit".to_owned(), "0".to_owned()),
            ("api_key".to_owned(), api_key),
            ("api_version".to_owned(), API_VERSION.to_owned()),
            ("app_version".to_owned(), APP_VERSION.to_owned()),
        ]
    } else {
        let password = secrets
            .get(PASSWORD_KEY)
            .map_err(port_error)?
            .filter(|p| !p.is_empty())
            .ok_or_else(|| UploadError::Auth(format!("no secret is stored under {PASSWORD_KEY}")))?;
        let session_key = sign_in(&config, &api_key, &password).await?;
        vec![
            ("method".to_owned(), "upload.getInfo".to_owned()),
            ("session_key".to_owned(), session_key),
            ("speed_limit".to_owned(), "0".to_owned()),
        ]
    };
    let info_text = call(info, None).await?.text();
    let (upload_url, fields) = {
        let doc = parse(&info_text)?;
        let upload = doc
            .descendants()
            .find(|node| node.has_tag_name("upload"))
            .ok_or_else(|| UploadError::BadResponse("SendSpace: the reply has no upload element".to_owned()))?;
        let url = attribute(upload, "url");
        if url.is_empty() {
            return Err(UploadError::BadResponse("SendSpace: the upload element has no url".to_owned()));
        }
        // ShareX sends all three fields, empty or not.
        let fields = [
            ("MAX_FILE_SIZE", attribute(upload, "max_file_size")),
            ("UPLOAD_IDENTIFIER", attribute(upload, "upload_identifier")),
            ("extra_info", attribute(upload, "extra_info")),
        ];
        (url, fields)
    };

    let UploadRequest { file_name, data, mime, .. } = request;
    let mut parts: Vec<Part> = fields.into_iter().map(|(name, value)| Part::Text { name: name.to_owned(), value }).collect();
    parts.push(Part::File { name: "userfile".to_owned(), file_name, mime, data: data_bytes(data)? });
    let mut upload = Request::new(Method::Post, upload_url);
    upload.body = Body::Multipart(parts);
    let reply_text = upload_call(upload, progress, None).await?.text();

    let doc = parse(&reply_text)?;
    if child_text(&doc, "status").as_deref() != Some("ok") {
        return Err(UploadError::BadResponse("SendSpace: the upload was not accepted".to_owned()));
    }
    let url = child_text(&doc, "download_url")
        .ok_or_else(|| UploadError::BadResponse("SendSpace: the upload reply has no download_url".to_owned()))?;
    Ok(UploadResult { url: Some(url), response: reply_text, ..UploadResult::default() })
}

/// `md5(token + md5(password))`, both lowercase hex, as `auth.login` wants it.
pub fn tokened_password(token: &str, password: &str) -> String {
    use md5::{Digest, Md5};
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let inner = hex(&Md5::digest(password.as_bytes()));
    hex(&Md5::digest(format!("{token}{inner}").as_bytes()))
}

/// `auth.createToken` then `auth.login`; returns the session key.
async fn sign_in(config: &SendSpaceConfig, api_key: &str, password: &str) -> Result<String, UploadError> {
    let form =
        |args: Vec<(&str, String)>| Body::Multipart(args.into_iter().map(|(k, v)| Part::Text { name: k.to_owned(), value: v }).collect());
    let mut create = Request::new(Method::Post, config.api_base.clone());
    create.body = form(vec![
        ("method", "auth.createToken".to_owned()),
        ("api_key", api_key.to_owned()),
        ("api_version", API_VERSION.to_owned()),
        ("app_version", APP_VERSION.to_owned()),
        ("response_format", "xml".to_owned()),
    ]);
    let text = call(create, None).await?.text();
    let token = child_text(&parse(&text)?, "token").ok_or_else(|| UploadError::BadResponse("SendSpace: no token".to_owned()))?;
    let mut login = Request::new(Method::Post, config.api_base.clone());
    login.body = form(vec![
        ("method", "auth.login".to_owned()),
        ("token", token.clone()),
        ("user_name", config.username.clone()),
        ("tokened_password", tokened_password(&token, password)),
    ]);
    let text = call(login, None).await?.text();
    child_text(&parse(&text)?, "session_key").ok_or_else(|| UploadError::Auth("SendSpace: the sign-in was refused".to_owned()))
}

/// The XML document, or the error of a `status="fail"` reply (`<error code=… text=…/>`).
fn parse(text: &str) -> Result<Document<'_>, UploadError> {
    let doc = Document::parse(text).map_err(|error| UploadError::BadResponse(format!("SendSpace: {error}")))?;
    let root = doc.root_element();
    if root.attribute("status") == Some("fail") {
        let error = root.children().find(|node| node.has_tag_name("error"));
        let code = error.and_then(|node| node.attribute("code")).unwrap_or("unknown");
        let text = error.and_then(|node| node.attribute("text")).unwrap_or("the request failed");
        return Err(UploadError::BadResponse(format!("SendSpace error {code}: {text}")));
    }
    Ok(doc)
}

/// An attribute's value, or empty when it is missing.
fn attribute(node: Node<'_, '_>, name: &str) -> String {
    node.attribute(name).unwrap_or_default().to_owned()
}

/// The text of the first element called `tag`, when it has some.
fn child_text(doc: &Document<'_>, tag: &str) -> Option<String> {
    doc.descendants()
        .find(|node| node.has_tag_name(tag))
        .and_then(|node| node.text())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}
