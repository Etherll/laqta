//! A tiny HTTP/1.1 server for the tests. It accepts one connection per canned reply, records each
//! request (method, target, headers, body) and answers with the canned status, headers and body.
//! It reads `Content-Length` and chunked request bodies and listens on 127.0.0.1 only.
#![allow(dead_code)]

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// One request as the server saw it. Header names are lowercase.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }
}

/// One reply the server sends. A `Content-Length` header is added when none is set.
#[derive(Clone, Debug)]
pub struct Canned {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Canned {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self::new(200, body)
    }

    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Canned {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

pub struct Server {
    addr: SocketAddr,
    thread: JoinHandle<Vec<Recorded>>,
}

impl Server {
    /// Starts the server. It answers `replies` in order, one connection each.
    pub fn start(replies: Vec<Canned>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the test server");
        let addr = listener.local_addr().expect("test server address");
        listener
            .set_nonblocking(true)
            .expect("non-blocking test listener");
        let thread = std::thread::spawn(move || {
            let mut recorded = Vec::new();
            for reply in replies {
                let mut stream = accept(&listener);
                recorded.push(read_request(&mut stream));
                write_reply(&mut stream, &reply);
            }
            recorded
        });
        Server { addr, thread }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    /// Waits until every reply has been sent, then returns the recorded requests.
    pub fn finish(self) -> Vec<Recorded> {
        self.thread.join().expect("test server thread")
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn accept(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .expect("blocking test connection");
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("read timeout");
                return stream;
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "the test server got no connection in 10 s"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(err) => panic!("test server accept failed: {err}"),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Recorded {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).expect("request line");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("header line");
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }

    let chunked = header(&headers, "transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"));
    let body = if chunked {
        read_chunked(&mut reader)
    } else if let Some(length) = header(&headers, "content-length") {
        let length: usize = length.parse().expect("numeric content-length");
        let mut body = vec![0; length];
        reader.read_exact(&mut body).expect("request body");
        body
    } else {
        Vec::new()
    };
    Recorded {
        method,
        target,
        headers,
        body,
    }
}

fn read_chunked(reader: &mut impl BufRead) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let mut size_line = String::new();
        reader.read_line(&mut size_line).expect("chunk size");
        let digits = size_line
            .trim()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        let size = usize::from_str_radix(digits, 16).expect("hex chunk size");
        if size == 0 {
            loop {
                let mut trailer = String::new();
                reader.read_line(&mut trailer).expect("chunk trailer");
                if trailer.trim_end().is_empty() {
                    return body;
                }
            }
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader.read_exact(&mut body[start..]).expect("chunk data");
        let mut line_end = String::new();
        reader.read_line(&mut line_end).expect("chunk terminator");
    }
}

fn write_reply(stream: &mut TcpStream, reply: &Canned) {
    let mut head = format!("HTTP/1.1 {} {}\r\n", reply.status, reason(reply.status));
    for (name, value) in &reply.headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    if header(&reply.headers, "content-length").is_none() {
        let _ = write!(head, "Content-Length: {}\r\n", reply.body.len());
    }
    head.push_str("Connection: close\r\n\r\n");
    // The client may hang up early (a cancelled download does); the test then asserts on what it got.
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&reply.body);
    let _ = stream.flush();
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    }
}
