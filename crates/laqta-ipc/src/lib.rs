//! Single-instance lock and command forwarding between Laqta processes. The first process to claim
//! the endpoint serves actions; later ones forward theirs to it.
#![forbid(unsafe_code)]

mod protocol;

pub use protocol::{Request, action_name, parse_action};

use interprocess::local_socket::{GenericFilePath, ListenerOptions, prelude::*};
use laqta_domain::Action;
use std::io::{self, BufReader, Write};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// How long the accept loop waits after a failed accept before trying again.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);

/// The endpoint for this user: a named pipe on Windows, a socket file on Unix.
pub fn endpoint_name() -> String {
    if cfg!(windows) {
        let user = std::env::var("USERNAME").ok().filter(|user| !user.is_empty()).unwrap_or_else(|| "user".to_owned());
        return format!(r"\\.\pipe\laqta-{user}");
    }
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => std::path::Path::new(&dir).join("laqta.sock").to_string_lossy().into_owned(),
        None => {
            let user = std::env::var("USER").ok().filter(|user| !user.is_empty()).unwrap_or_else(|| "user".to_owned());
            format!("/tmp/laqta-{user}.sock")
        }
    }
}

/// The result of [`claim`] or [`claim_at`].
#[derive(Debug)]
pub enum Claim {
    /// This process owns the endpoint. Call [`Server::serve`] to answer requests.
    Primary(Server),
    /// Another process owns the endpoint. Send it actions through this client.
    Secondary(Client),
}

/// Claims [`endpoint_name`].
pub fn claim() -> io::Result<Claim> {
    claim_at(&endpoint_name())
}

/// Binds `endpoint`. If something already listens there, connects to it instead. If something is
/// bound but nothing answers (a socket file left by a dead process), removes it and binds again.
pub fn claim_at(endpoint: &str) -> io::Result<Claim> {
    let name = endpoint.to_fs_name::<GenericFilePath>()?;
    let bind_error = match ListenerOptions::new().name(name.borrow()).create_sync() {
        Ok(listener) => return Ok(Claim::Primary(Server { listener })),
        Err(error) => error,
    };
    if let Ok(stream) = LocalSocketStream::connect(name.borrow()) {
        return Ok(Claim::Secondary(Client { reader: BufReader::new(stream) }));
    }
    if cfg!(unix) && bind_error.kind() == io::ErrorKind::AddrInUse {
        match std::fs::remove_file(endpoint) {
            Ok(()) => tracing::info!(endpoint, "removed stale ipc socket"),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = ListenerOptions::new().name(name.borrow()).create_sync()?;
        return Ok(Claim::Primary(Server { listener }));
    }
    Err(bind_error)
}

/// The listening end, held by the primary process. Dropping it releases the endpoint.
#[derive(Debug)]
pub struct Server {
    listener: LocalSocketListener,
}

impl Server {
    /// Answers requests on a thread named `laqta-ipc`. Each connection is served on a thread of its
    /// own, so a slow request does not hold up other clients. `on_action` runs once per request and
    /// returns the reply: `Ok` or an error message.
    pub fn serve(self, on_action: impl Fn(Action) -> Result<(), String> + Send + Sync + 'static) {
        self.serve_requests(move |r| match r {
            Request::Action(a) => on_action(a),
            _ => Err("this instance only accepts actions".to_owned()),
        });
    }

    /// Like `serve`, for every request kind. Connections run on different threads at once, so the
    /// handler must be `Sync`.
    pub fn serve_requests(self, on_request: impl Fn(Request) -> Result<(), String> + Send + Sync + 'static) {
        let on_request = Arc::new(on_request);
        let spawned = std::thread::Builder::new().name("laqta-ipc".to_owned()).spawn(move || {
            for connection in self.listener {
                match connection {
                    Ok(stream) => serve_on_thread(stream, &on_request),
                    Err(error) => {
                        tracing::warn!(%error, "ipc accept failed");
                        std::thread::sleep(ACCEPT_RETRY_DELAY);
                    }
                }
            }
        });
        if let Err(error) = spawned {
            tracing::error!(%error, "cannot start the ipc thread; other instances cannot forward actions");
        }
    }
}

/// Serves one connection on a thread named `laqta-ipc-conn`. If that thread cannot start, serves
/// the connection on the calling thread instead.
fn serve_on_thread<F>(stream: LocalSocketStream, on_request: &Arc<F>)
where
    F: Fn(Request) -> Result<(), String> + Send + Sync + 'static,
{
    // A failed `spawn` drops its closure, so the stream waits in a slot the fallback can take back.
    let slot = Arc::new(Mutex::new(Some(stream)));
    let thread_slot = Arc::clone(&slot);
    let handler = Arc::clone(on_request);
    let spawned = std::thread::Builder::new().name("laqta-ipc-conn".to_owned()).spawn(move || {
        if let Some(stream) = take_stream(&thread_slot) {
            serve_connection(stream, &*handler);
        }
    });
    if let Err(error) = spawned {
        tracing::error!(%error, "cannot start an ipc connection thread; serving the connection on the accept thread");
        if let Some(stream) = take_stream(&slot) {
            serve_connection(stream, &**on_request);
        }
    }
}

fn take_stream(slot: &Mutex<Option<LocalSocketStream>>) -> Option<LocalSocketStream> {
    slot.lock().unwrap_or_else(PoisonError::into_inner).take()
}

/// Answers requests on one connection until the client hangs up.
fn serve_connection(stream: LocalSocketStream, on_action: &impl Fn(Request) -> Result<(), String>) {
    let mut reader = BufReader::new(stream);
    loop {
        let line = match protocol::read_line(&mut reader) {
            Ok(Some(line)) => line,
            Ok(None) => return,
            Err(error) => {
                // The line framing is lost after a bad line, so reply once and hang up.
                if error.kind() == io::ErrorKind::InvalidData {
                    let reply = protocol::encode_reply(&Err("malformed request".to_owned()));
                    let _ = reader.get_mut().write_all(reply.as_bytes());
                }
                tracing::debug!(%error, "ipc connection closed after a read error");
                return;
            }
        };
        let reply = protocol::encode_reply(&handle_request(&line, on_action));
        if let Err(error) = reader.get_mut().write_all(reply.as_bytes()) {
            tracing::debug!(%error, "ipc reply not delivered");
            return;
        }
    }
}

/// Runs one request. A panic in `on_action` is contained, as in the dispatcher (ADR 020).
fn handle_request(line: &str, on_action: &impl Fn(Request) -> Result<(), String>) -> Result<(), String> {
    let request = protocol::parse(line)?;
    match panic::catch_unwind(AssertUnwindSafe(|| on_action(request.clone()))) {
        Ok(result) => result,
        Err(_) => {
            tracing::error!(?request, "ipc request handler panicked");
            Err("internal error".to_owned())
        }
    }
}

/// The connecting end, held by a secondary process. One connection carries any number of
/// requests.
#[derive(Debug)]
pub struct Client {
    reader: BufReader<LocalSocketStream>,
}

impl Client {
    /// Sends one action and waits for its reply. `Err` carries the primary's error message, or
    /// the text of an I/O error.
    pub fn send(&mut self, action: Action) -> Result<(), String> {
        self.send_request(&Request::Action(action))
    }

    /// Sends any request and waits for its reply.
    pub fn send_request(&mut self, request: &Request) -> Result<(), String> {
        let request = protocol::encode(request);
        self.reader.get_mut().write_all(request.as_bytes()).map_err(|error| error.to_string())?;
        match protocol::read_line(&mut self.reader).map_err(|error| error.to_string())? {
            Some(line) => protocol::parse_reply(&line),
            None => Err("connection closed before a reply".to_owned()),
        }
    }
}
