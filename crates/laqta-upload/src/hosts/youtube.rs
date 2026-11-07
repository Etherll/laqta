//! YouTube: a resumable upload (ADR 014). The session is opened with the video's metadata, then the
//! bytes go to the session URI in one `PUT`. Only video files are accepted. It needs a sign-in.

use std::sync::Arc;

use laqta_http::{Body, Method, Request};
use laqta_ports::{BoxFuture, Destination, ProgressCallback, UploadError, UploadRequest, UploadResult};
use serde::Deserialize;
use serde_json::json;

use crate::hosts::{PATH_SEGMENT, bearer, call, data_bytes, json, json_body, upload_call};
use crate::oauth::{OAuth, OAuthConfig};

/// The production root for the resumable upload.
pub const GOOGLE_UPLOAD: &str = "https://www.googleapis.com";
/// Placeholder until the app is registered with Google. The real client id comes from settings.
pub const YOUTUBE_CLIENT_ID: &str = "laqta-youtube-client-id";
/// The `SecretStore` key for the token.
pub const SECRET_KEY: &str = "laqta/youtube";
/// The YouTube category "People & Blogs", which the API requires.
const CATEGORY_PEOPLE_AND_BLOGS: &str = "22";

/// Google's endpoints. `youtube.upload` is the only scope asked for. `access_type=offline` with
/// `prompt=consent` asks for a refresh token.
pub fn oauth_config() -> OAuthConfig {
    OAuthConfig {
        auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_owned(),
        token_url: "https://oauth2.googleapis.com/token".to_owned(),
        client_id: YOUTUBE_CLIENT_ID.to_owned(),
        client_secret: None,
        scopes: vec!["https://www.googleapis.com/auth/youtube.upload".to_owned()],
        extra_auth_params: vec![("access_type".to_owned(), "offline".to_owned()), ("prompt".to_owned(), "consent".to_owned())],
    }
}

/// `public`, `unlisted` or `private`; anything else is `unlisted`.
pub fn privacy_status(value: &str) -> &'static str {
    match value.trim() {
        "public" => "public",
        "private" => "private",
        _ => "unlisted",
    }
}

#[derive(Debug)]
pub struct YouTube {
    oauth: Arc<OAuth>,
    upload_base: String,
    privacy: String,
}

#[derive(Deserialize)]
struct Video {
    id: String,
}

impl YouTube {
    /// `privacy` is one of `public`, `unlisted`, `private` (see `privacy_status`).
    pub fn new(oauth: Arc<OAuth>, upload_base: String, privacy: &str) -> Self {
        YouTube { oauth, upload_base, privacy: privacy_status(privacy).to_owned() }
    }

    async fn run(&self, request: UploadRequest, progress: ProgressCallback) -> Result<UploadResult, UploadError> {
        let UploadRequest { file_name, data, mime, .. } = request;
        if !mime.starts_with("video/") {
            return Err(UploadError::Unsupported(format!("YouTube takes video files, not {mime}")));
        }
        let data = data_bytes(data)?;
        let authorization = bearer(&self.oauth.access_token().await?);
        let title = file_name.rsplit_once('.').map_or(file_name.as_str(), |(stem, _)| stem);
        let metadata = json!({
            "snippet": {"title": title, "categoryId": CATEGORY_PEOPLE_AND_BLOGS},
            "status": {"privacyStatus": self.privacy, "selfDeclaredMadeForKids": false},
        });

        let mut session = Request::new(Method::Post, format!("{}/upload/youtube/v3/videos", self.upload_base));
        session.query = vec![("uploadType".to_owned(), "resumable".to_owned()), ("part".to_owned(), "snippet,status".to_owned())];
        session.headers.push(authorization.clone());
        session.headers.push(("X-Upload-Content-Type".to_owned(), mime.clone()));
        session.headers.push(("X-Upload-Content-Length".to_owned(), data.len().to_string()));
        session.body = json_body(&metadata);
        let started = call(session, Some(self.oauth.as_ref())).await?;
        let location = started
            .header("location")
            .map(str::to_owned)
            .ok_or_else(|| UploadError::BadResponse("YouTube gave no upload session location".to_owned()))?;

        let mut put = Request::new(Method::Put, location);
        put.headers.push(authorization);
        put.body = Body::Bytes { data, content_type: mime };
        let response = upload_call(put, progress, Some(self.oauth.as_ref())).await?;
        let video: Video = json(&response)?;
        let id = percent_encoding::utf8_percent_encode(&video.id, PATH_SEGMENT);
        Ok(UploadResult { url: Some(format!("https://youtu.be/{id}")), thumbnail_url: None, deletion_url: None, response: response.text() })
    }
}

impl Destination for YouTube {
    fn id(&self) -> &str {
        "youtube"
    }

    fn name(&self) -> &str {
        "YouTube"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(self.run(request, progress))
    }
}
