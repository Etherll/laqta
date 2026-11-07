//! The HTTP library (ADR 007): one Tokio runtime on its own thread, started on the first request,
//! with one shared `reqwest` client. Results come back over channels, so callers on any executor
//! can await them. Filled in by M2 task U1; the request and response types below are the contract.
#![forbid(unsafe_code)]

/// An HTTP method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// One part of a multipart body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Text { name: String, value: String },
    File { name: String, file_name: String, mime: String, data: Vec<u8> },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Body {
    #[default]
    Empty,
    Bytes { data: Vec<u8>, content_type: String },
    Form(Vec<(String, String)>),
    Multipart(Vec<Part>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub url: String,
    /// Query parameters appended to the URL (percent-encoded by the library).
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Body,
    /// Overall timeout in seconds (0 = the client default, 120 s).
    pub timeout_secs: u64,
}

impl Request {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Request { method, url: url.into(), query: Vec::new(), headers: Vec::new(), body: Body::Empty, timeout_secs: 0 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// The final URL after redirects.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    /// Case-insensitive header lookup.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    #[error("network: {0}")]
    Network(String),
    #[error("timed out")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    Checksum { expected: String, actual: String },
    #[error("i/o: {0}")]
    Io(String),
}
