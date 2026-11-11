//! The application's view of the upload module (`laqta_ports::Uploaders`): a registry that settings
//! can change at run time, plus downloads for "upload from URL".

use std::sync::{Arc, RwLock};

use laqta_ports::upload::Downloaded;
use laqta_ports::{BoxFuture, Destination, Sharer, Shortener, UploadError, Uploaders};

use crate::registry::Registry;

#[derive(Debug)]
pub struct SharedRegistry {
    inner: RwLock<Registry>,
}

impl SharedRegistry {
    pub fn new(registry: Registry) -> Arc<SharedRegistry> {
        Arc::new(SharedRegistry { inner: RwLock::new(registry) })
    }

    /// Runs `f` with write access (adding a custom uploader, a signed-in host, …).
    pub fn update<T>(&self, f: impl FnOnce(&mut Registry) -> T) -> T {
        match self.inner.write() {
            Ok(mut g) => f(&mut g),
            Err(p) => f(&mut p.into_inner()),
        }
    }

    fn read<T>(&self, f: impl FnOnce(&Registry) -> T) -> T {
        match self.inner.read() {
            Ok(g) => f(&g),
            Err(p) => f(&p.into_inner()),
        }
    }
}

/// `https://x.y/a/b.png?q=1` → `b.png`; empty → `download`.
pub fn file_name_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let last = path.rsplit('/').next().unwrap_or("");
    let name = percent_encoding::percent_decode_str(last).decode_utf8_lossy().into_owned();
    if name.is_empty() || name.contains("://") { "download".into() } else { name }
}

impl Uploaders for SharedRegistry {
    fn destination(&self, id: &str) -> Option<Arc<dyn Destination>> {
        self.read(|r| r.destination(id))
    }
    fn shortener(&self, id: &str) -> Option<Arc<dyn Shortener>> {
        self.read(|r| r.shortener(id))
    }
    fn sharer(&self, id: &str) -> Option<Arc<dyn Sharer>> {
        self.read(|r| r.sharer(id))
    }
    fn destination_list(&self) -> Vec<(String, String)> {
        self.read(|r| r.destination_ids())
    }
    fn custom_shortener_list(&self) -> Vec<(String, String)> {
        self.read(|r| r.custom_shortener_ids())
    }
    fn custom_sharer_list(&self) -> Vec<(String, String)> {
        self.read(|r| r.custom_sharer_ids())
    }
    fn download(&self, url: &str) -> BoxFuture<'_, Result<Downloaded, UploadError>> {
        let url = url.to_string();
        Box::pin(async move {
            let resp = laqta_http::send(laqta_http::Request::new(laqta_http::Method::Get, url.clone()))
                .await
                .map_err(|e| UploadError::Network(e.to_string()))?;
            if !(200..300).contains(&resp.status) {
                return Err(UploadError::Server { status: resp.status, body: resp.text() });
            }
            let mime = resp.header("content-type").unwrap_or("application/octet-stream").split(';').next().unwrap_or("").trim().to_string();
            Ok((resp.body, file_name_from_url(&resp.url), mime))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_urls() {
        assert_eq!(file_name_from_url("https://x.y/a/b%20c.png?q=1#f"), "b c.png");
        assert_eq!(file_name_from_url("https://x.y/"), "download");
    }
}
