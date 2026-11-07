//! The loopback listener for OAuth redirects: one GET on 127.0.0.1, answered with a fixed page.

use std::io;
use std::net::TcpListener as StdListener;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::HttpError;

/// A connection that sends no request head within this time is dropped. Browsers open spare
/// connections that stay silent, and the listener handles connections one at a time, so this bounds
/// how long a silent one can delay the real redirect.
const HEAD_TIMEOUT: Duration = Duration::from_secs(2);
/// Header lines read before a request is treated as malformed.
const MAX_HEADER_LINES: usize = 100;

/// Accepts connections until one sends a GET. Answers it with `reply_html` and returns its
/// request target (path and query). Other methods get a 405 and the listener keeps waiting.
pub(crate) async fn serve_once(listener: StdListener, reply_html: String) -> Result<String, HttpError> {
    let listener = TcpListener::from_std(listener).map_err(io_error)?;
    loop {
        let (mut stream, _) = listener.accept().await.map_err(io_error)?;
        match tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut stream)).await {
            Ok(Ok((method, target))) if method.eq_ignore_ascii_case("GET") => {
                reply(&mut stream, "200 OK", &reply_html).await;
                return Ok(target);
            }
            Ok(Ok(_)) => reply(&mut stream, "405 Method Not Allowed", "").await,
            // A silent or unreadable connection is dropped, and the listener keeps waiting.
            _ => {}
        }
    }
}

async fn read_head(stream: &mut TcpStream) -> io::Result<(String, String)> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    for _ in 0..MAX_HEADER_LINES {
        let mut header = String::new();
        if reader.read_line(&mut header).await? == 0 || header.trim_end().is_empty() {
            break;
        }
    }
    Ok((method, target))
}

async fn reply(stream: &mut TcpStream, status: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    // The request has been read; a browser that hangs up early is not an error here.
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn io_error(err: io::Error) -> HttpError {
    HttpError::Io(err.to_string())
}
