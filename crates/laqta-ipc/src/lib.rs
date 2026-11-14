//! Single-instance lock and command forwarding between Laqta processes. The first process to claim
//! the endpoint serves actions; later ones forward theirs to it.
#![forbid(unsafe_code)]

mod protocol;

pub use protocol::{Request, action_name, parse_action};

use interprocess::local_socket::{GenericFilePath, ListenerOptions, Name, prelude::*};
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

/// Like [`claim`], for a process that may run elevated. On Windows, with the SID text of the current
/// user, the pipe is created with an explicit access list: that user and the system get full access,
/// and the pipe carries a medium integrity label. Without it an elevated process makes a pipe a
/// non-elevated launch of Laqta cannot open, so it could not forward its action. Elsewhere the SID
/// is ignored.
pub fn claim_for_user(user_sid: Option<&str>) -> io::Result<Claim> {
    claim_with(&endpoint_name(), user_sid)
}

/// Binds `endpoint`. If something already listens there, connects to it instead. If something is
/// bound but nothing answers (a socket file left by a dead process), removes it and binds again.
pub fn claim_at(endpoint: &str) -> io::Result<Claim> {
    claim_with(endpoint, None)
}

fn claim_with(endpoint: &str, user_sid: Option<&str>) -> io::Result<Claim> {
    let name = endpoint.to_fs_name::<GenericFilePath>()?;
    let bind_error = match listen(name.borrow(), user_sid) {
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
        let listener = listen(name.borrow(), user_sid)?;
        return Ok(Claim::Primary(Server { listener }));
    }
    Err(bind_error)
}

/// Binds `name`, with the pipe's access list from `user_sid` where the platform has one.
fn listen(name: Name<'_>, user_sid: Option<&str>) -> io::Result<LocalSocketListener> {
    let options = ListenerOptions::new().name(name);
    #[cfg(windows)]
    let options = match user_sid {
        Some(sid) => with_pipe_security(options, sid),
        None => options,
    };
    #[cfg(not(windows))]
    let _ = user_sid;
    options.create_sync()
}

/// Gives `options` the access list for `user_sid`. If the list cannot be built, logs it and keeps
/// the default one: a pipe only the elevated process can open still beats no single instance.
#[cfg(windows)]
fn with_pipe_security<'a>(options: ListenerOptions<'a>, user_sid: &str) -> ListenerOptions<'a> {
    use interprocess::os::windows::local_socket::ListenerOptionsExt;
    use interprocess::os::windows::security_descriptor::SecurityDescriptor;

    let descriptor = pipe_sddl(user_sid)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a SID"))
        .and_then(|sddl| widestring::U16CString::from_str(sddl).map_err(io::Error::other))
        .and_then(|sddl| SecurityDescriptor::deserialize(&sddl));
    match descriptor {
        Ok(descriptor) => options.security_descriptor(descriptor),
        Err(error) => {
            tracing::warn!(%error, "cannot build the ipc pipe's access list; using the default one");
            options
        }
    }
}

/// The SDDL for the pipe: full access for `user_sid` and the system, and a medium mandatory label that
/// lets a non-elevated process write to a pipe an elevated one created. `None` unless `user_sid` looks
/// like a SID (`S-1-5-21-…`), so nothing else can end up in the string.
#[cfg(any(windows, test))]
fn pipe_sddl(user_sid: &str) -> Option<String> {
    let digits = user_sid.strip_prefix("S-1-")?;
    let valid =
        !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit() || c == '-') && !digits.starts_with('-') && !digits.ends_with('-');
    valid.then(|| format!("D:(A;;GA;;;{user_sid})(A;;GA;;;SY)S:(ML;;NW;;;ME)"))
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
        let request = protocol::encode_checked(request)?;
        self.reader.get_mut().write_all(request.as_bytes()).map_err(|error| error.to_string())?;
        match protocol::read_line(&mut self.reader).map_err(|error| error.to_string())? {
            Some(line) => protocol::parse_reply(&line),
            None => Err("connection closed before a reply".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pipe_sddl;

    #[test]
    fn the_pipe_grants_the_user_and_system_and_a_medium_label() {
        assert_eq!(
            pipe_sddl("S-1-5-21-1004336348-1177238915-682003330-1001").as_deref(),
            Some("D:(A;;GA;;;S-1-5-21-1004336348-1177238915-682003330-1001)(A;;GA;;;SY)S:(ML;;NW;;;ME)")
        );
    }

    #[test]
    fn only_a_sid_gets_into_the_descriptor() {
        for bad in ["", "S-1-", "S-1--5", "S-1-5-", "S-1-5-21)(A;;GA;;;WD", "S-2-5-18", "user", "S-1-5 18"] {
            assert_eq!(pipe_sddl(bad), None, "{bad:?}");
        }
    }

    /// Where there is no pipe access list the SID changes nothing. (On Windows a made-up SID would
    /// lock the test out of its own pipe.)
    #[cfg(unix)]
    #[test]
    fn claiming_for_a_user_still_claims_the_endpoint() {
        let dir = tempfile::tempdir().expect("temp dir");
        let endpoint = dir.path().join("laqta.sock").to_string_lossy().into_owned();
        let first = super::claim_with(&endpoint, Some("S-1-5-21-1-2-3-1001")).expect("claim");
        assert!(matches!(first, super::Claim::Primary(_)));
        let second = super::claim_with(&endpoint, Some("S-1-5-21-1-2-3-1001")).expect("claim again");
        assert!(matches!(second, super::Claim::Secondary(_)));
    }
}
