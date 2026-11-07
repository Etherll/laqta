//! Cloud ML adapters (M7 D7): remove.bg as a `BackgroundRemover`, and one OpenAI-compatible client as
//! the `ImageAnalyzer`. API keys come from `SecretStore`. HTTP goes through `laqta-http`, behind a
//! `Transport` so tests run without the network.

use std::sync::Arc;

use base64::Engine as _;
use laqta_domain::Bitmap;
use laqta_http::{Body, Method, Part, Request, Response};
use laqta_ports::{BackgroundRemover, BoxFuture, ImageAnalyzer, PortError, SecretStore};

/// Sends one request. `laqta_http::send` in the app, a canned reply in tests.
pub type Transport = Arc<dyn Fn(Request) -> BoxFuture<'static, Result<Response, laqta_http::HttpError>> + Send + Sync>;
/// PNG bytes for a bitmap, and a bitmap for image bytes (the app passes its `Codecs`).
/// Async, so the app can run them on its compute pool.
pub type Encode = Arc<dyn Fn(Bitmap) -> BoxFuture<'static, Result<Vec<u8>, String>> + Send + Sync>;
pub type Decode = Arc<dyn Fn(Vec<u8>) -> BoxFuture<'static, Result<Bitmap, String>> + Send + Sync>;

pub const REMOVE_BG_KEY: &str = "cloud:remove.bg";
pub const ANALYZER_KEY: &str = "cloud:analyzer";

/// The real transport.
pub fn http() -> Transport {
    Arc::new(|request| Box::pin(laqta_http::send(request)))
}

fn net(e: laqta_http::HttpError) -> PortError {
    PortError::Failed(e.to_string())
}

/// The error text of a non-2xx reply: the service's message when it sent JSON with one.
fn status_error(service: &str, r: &Response) -> PortError {
    let detail = serde_json::from_slice::<serde_json::Value>(&r.body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.pointer("/errors/0/title"))
                .or_else(|| v.pointer("/error"))
                .and_then(|m| m.as_str().map(str::to_owned))
        })
        .unwrap_or_else(|| r.text().chars().take(200).collect());
    match r.status {
        401 | 403 => PortError::Failed(format!("{service} rejected the API key ({}): {detail}", r.status)),
        402 => PortError::Failed(format!("{service}: no credits left: {detail}")),
        _ => PortError::Failed(format!("{service} answered {}: {detail}", r.status)),
    }
}

/// remove.bg: `POST /v1.0/removebg` with the image as `image_file`, `size=auto`, PNG back.
pub struct RemoveBg {
    pub endpoint: String,
    pub secrets: Arc<dyn SecretStore>,
    pub transport: Transport,
    pub encode: Encode,
    pub decode: Decode,
}

impl RemoveBg {
    pub fn new(secrets: Arc<dyn SecretStore>, encode: Encode, decode: Decode) -> Self {
        RemoveBg { endpoint: "https://api.remove.bg/v1.0/removebg".into(), secrets, transport: http(), encode, decode }
    }
}

impl BackgroundRemover for RemoveBg {
    fn remove(&self, image: Bitmap) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        Box::pin(async move {
            let key = self
                .secrets
                .get(REMOVE_BG_KEY)?
                .filter(|k| !k.is_empty())
                .ok_or_else(|| PortError::Failed("remove.bg needs an API key (Settings → Models → Cloud)".into()))?;
            let png = (self.encode)(image).await.map_err(PortError::Failed)?;
            let mut request = Request::new(Method::Post, self.endpoint.clone());
            request.headers.push(("X-Api-Key".into(), key));
            request.headers.push(("Accept".into(), "image/png".into()));
            request.body = Body::Multipart(vec![
                Part::File { name: "image_file".into(), file_name: "capture.png".into(), mime: "image/png".into(), data: png },
                Part::Text { name: "size".into(), value: "auto".into() },
                Part::Text { name: "format".into(), value: "png".into() },
            ]);
            request.timeout_secs = 120;
            let reply = (self.transport)(request).await.map_err(net)?;
            if !(200..300).contains(&reply.status) {
                return Err(status_error("remove.bg", &reply));
            }
            (self.decode)(reply.body).await.map_err(|e| PortError::Failed(format!("remove.bg sent an unreadable image: {e}")))
        })
    }
}

impl std::fmt::Debug for RemoveBg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoveBg").field("endpoint", &self.endpoint).finish_non_exhaustive()
    }
}

/// An OpenAI-compatible `POST {base}/chat/completions` with the image as a `data:` URL.
pub struct OpenAiCompatible {
    pub base_url: String,
    pub model: String,
    pub secrets: Arc<dyn SecretStore>,
    pub transport: Transport,
    pub encode: Encode,
}

impl std::fmt::Debug for OpenAiCompatible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatible").field("base_url", &self.base_url).field("model", &self.model).finish_non_exhaustive()
    }
}

/// The JSON body for one image and one prompt.
pub fn chat_body(model: &str, prompt: &str, png: &[u8]) -> serde_json::Value {
    let url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png));
    serde_json::json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": prompt },
                { "type": "image_url", "image_url": { "url": url } }
            ]
        }]
    })
}

/// The answer text from a chat-completions reply: `choices[0].message.content` as a string or as
/// a list of text parts.
pub fn answer_text(body: &[u8]) -> Result<String, PortError> {
    let v: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| PortError::Failed(format!("the analyzer sent invalid JSON: {e}")))?;
    let content = v.pointer("/choices/0/message/content").ok_or_else(|| PortError::Failed("the analyzer's reply has no answer".into()))?;
    let text = match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join(""),
        _ => String::new(),
    };
    Ok(text.trim().to_owned())
}

impl ImageAnalyzer for OpenAiCompatible {
    fn analyze(&self, image: Bitmap, prompt: String) -> BoxFuture<'_, Result<String, PortError>> {
        Box::pin(async move {
            if self.base_url.trim().is_empty() || self.model.trim().is_empty() {
                return Err(PortError::Failed("set the analyzer's endpoint and model (Settings → Models → Cloud)".into()));
            }
            let png = (self.encode)(image).await.map_err(PortError::Failed)?;
            let body = chat_body(&self.model, &prompt, &png);
            let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
            let mut request = Request::new(Method::Post, url);
            // Local servers (Ollama, LM Studio) need no key.
            if let Some(key) = self.secrets.get(ANALYZER_KEY)?.filter(|k| !k.is_empty()) {
                request.headers.push(("Authorization".into(), format!("Bearer {key}")));
            }
            request.body = Body::Bytes { data: body.to_string().into_bytes(), content_type: "application/json".into() };
            request.timeout_secs = 180;
            let reply = (self.transport)(request).await.map_err(net)?;
            if !(200..300).contains(&reply.status) {
                return Err(status_error("The analyzer", &reply));
            }
            answer_text(&reply.body)
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Mutex;

    use futures_lite::future::block_on;
    use laqta_domain::PixelFormat;
    use laqta_ports::fakes::FakeSecretStore;

    use super::*;

    fn image() -> Bitmap {
        Bitmap::new(2, 1, PixelFormat::Rgba8, vec![1, 2, 3, 255, 4, 5, 6, 255].into()).unwrap()
    }

    fn reply(status: u16, body: &[u8]) -> Response {
        Response { status, url: String::new(), headers: Vec::new(), body: body.to_vec() }
    }

    /// A transport that records the request and answers `answer`.
    fn canned(answer: Response) -> (Transport, Arc<Mutex<Vec<Request>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let t: Transport = Arc::new(move |r| {
            s.lock().unwrap().push(r);
            let a = answer.clone();
            Box::pin(async move { Ok(a) })
        });
        (t, seen)
    }

    fn secrets(key: &str, value: &str) -> Arc<dyn SecretStore> {
        let s = FakeSecretStore::default();
        s.set(key, value).unwrap();
        Arc::new(s)
    }

    fn encode() -> Encode {
        Arc::new(|_| Box::pin(async { Ok(b"PNG".to_vec()) }))
    }

    #[test]
    fn remove_bg_posts_the_image_and_decodes_the_reply() {
        let (transport, seen) = canned(reply(200, b"OUT"));
        let decode: Decode = Arc::new(|bytes| {
            assert_eq!(bytes, b"OUT");
            Box::pin(async { Ok(image()) })
        });
        let rb =
            RemoveBg { endpoint: "https://x/removebg".into(), secrets: secrets(REMOVE_BG_KEY, "k1"), transport, encode: encode(), decode };
        block_on(rb.remove(image())).unwrap();
        let r = &seen.lock().unwrap()[0];
        assert_eq!(r.url, "https://x/removebg");
        assert!(r.headers.contains(&("X-Api-Key".into(), "k1".into())));
        let Body::Multipart(parts) = &r.body else { panic!("multipart") };
        assert!(matches!(&parts[0], Part::File { name, data, .. } if name == "image_file" && data == b"PNG"));
    }

    #[test]
    fn remove_bg_needs_a_key_and_reports_errors() {
        let (transport, _) = canned(reply(402, br#"{"errors":[{"title":"Insufficient credits"}]}"#));
        let decode: Decode = Arc::new(|_| Box::pin(async { Ok(image()) }));
        let no_key = RemoveBg {
            endpoint: "u".into(),
            secrets: Arc::new(FakeSecretStore::default()),
            transport: transport.clone(),
            encode: encode(),
            decode: decode.clone(),
        };
        assert!(block_on(no_key.remove(image())).unwrap_err().to_string().contains("API key"));
        let rb = RemoveBg { endpoint: "u".into(), secrets: secrets(REMOVE_BG_KEY, "k"), transport, encode: encode(), decode };
        assert!(block_on(rb.remove(image())).unwrap_err().to_string().contains("Insufficient credits"));
    }

    #[test]
    fn analyzer_sends_an_image_part_and_reads_the_answer() {
        let (transport, seen) = canned(reply(200, br#"{"choices":[{"message":{"content":" A cat. "}}]}"#));
        let a = OpenAiCompatible {
            base_url: "http://localhost:11434/v1/".into(),
            model: "llava".into(),
            secrets: Arc::new(FakeSecretStore::default()),
            transport,
            encode: encode(),
        };
        assert_eq!(block_on(a.analyze(image(), "What is it?".into())).unwrap(), "A cat.");
        let r = &seen.lock().unwrap()[0];
        assert_eq!(r.url, "http://localhost:11434/v1/chat/completions");
        assert!(!r.headers.iter().any(|(k, _)| k == "Authorization"), "no key, no header");
        let Body::Bytes { data, .. } = &r.body else { panic!("json body") };
        let v: serde_json::Value = serde_json::from_slice(data).unwrap();
        assert_eq!(v["model"], "llava");
        assert_eq!(v["messages"][0]["content"][0]["text"], "What is it?");
        assert!(v["messages"][0]["content"][1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
    }

    #[test]
    fn analyzer_uses_the_key_and_reads_part_lists_and_errors() {
        assert_eq!(
            answer_text(br#"{"choices":[{"message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}]}"#).unwrap(),
            "ab"
        );
        let (transport, seen) = canned(reply(401, br#"{"error":{"message":"bad key"}}"#));
        let a = OpenAiCompatible {
            base_url: "https://api.example/v1".into(),
            model: "m".into(),
            secrets: secrets(ANALYZER_KEY, "sk"),
            transport,
            encode: encode(),
        };
        let err = block_on(a.analyze(image(), "p".into())).unwrap_err().to_string();
        assert!(err.contains("bad key") && err.contains("401"), "{err}");
        assert!(seen.lock().unwrap()[0].headers.contains(&("Authorization".into(), "Bearer sk".into())));
    }
}
