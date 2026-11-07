//! FTP through `suppaftp`'s blocking client, run on a short-lived thread so the upload future
//! never blocks an executor. Plain FTP, explicit FTPS (`AUTH TLS`) and implicit FTPS. TLS is rustls
//! with the platform verifier on Linux and native-tls (SChannel) on Windows (ADR 025). The password
//! is read from `SecretStore` under `laqta/ftp/password`.

use std::fmt;
use std::fs::File;
use std::io::{self, Cursor, Read};
use std::net::{TcpStream, ToSocketAddrs};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Poll, Waker};
use std::time::Duration;

use laqta_ports::upload::{
    Destination, ProgressCallback, SecretStore, UploadData, UploadError, UploadRequest, UploadResult,
};
use laqta_ports::{BoxFuture, PortError};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use suppaftp::types::FileType;
use suppaftp::{FtpError, FtpResult, FtpStream, ImplFtpStream, Mode, TlsConnector, TlsStream};

/// Where the password lives in `SecretStore`.
pub const PASSWORD_KEY: &str = "laqta/ftp/password";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const IO_TIMEOUT: Duration = Duration::from_secs(60);

const FTP_PORT: u16 = 21;
/// The port of implicit FTPS. A config still on the plain FTP port uses it instead.
const FTPS_IMPLICIT_PORT: u16 = 990;

/// FTP reply codes that matter to the upload.
const SERVICE_NOT_AVAILABLE: u32 = 421;
const NOT_LOGGED_IN: u32 = 530;
const NEED_ACCOUNT: u32 = 532;
const INSUFFICIENT_STORAGE: u32 = 452;
const EXCEEDED_STORAGE: u32 = 552;

/// The FTPS stream of this target. Explicit mode upgrades the control channel and implicit mode
/// starts with TLS, so both end up with this type.
#[cfg(not(windows))]
type TlsFtp = suppaftp::RustlsFtpStream;
#[cfg(windows)]
type TlsFtp = suppaftp::NativeTlsFtpStream;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FtpTls {
    #[default]
    None,
    /// FTPS, starting in clear text and upgraded with `AUTH TLS`.
    Explicit,
    /// FTPS over an implicit TLS port (990 when the config still has the plain FTP port 21).
    Implicit,
}

/// One FTP destination.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// Remote directory, created when missing. A leading `/` makes it absolute.
    pub directory: String,
    /// Base of the URL shared with users; the file name is appended.
    pub url_base: String,
    /// Passive data connections (the default). False uses active mode.
    #[serde(default = "default_passive")]
    pub passive: bool,
    #[serde(default)]
    pub tls: FtpTls,
}

fn default_passive() -> bool {
    true
}

/// The FTP destination.
#[derive(Clone)]
pub struct Ftp {
    config: FtpConfig,
    secrets: Arc<dyn SecretStore>,
}

impl Ftp {
    pub fn new(config: FtpConfig, secrets: Arc<dyn SecretStore>) -> Self {
        Ftp { config, secrets }
    }
}

impl fmt::Debug for Ftp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ftp").field("config", &self.config).finish_non_exhaustive()
    }
}

impl Destination for Ftp {
    fn id(&self) -> &str {
        "ftp"
    }

    fn name(&self) -> &str {
        "FTP"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(transfer(self.config.clone(), Arc::clone(&self.secrets), request, progress))
    }
}

async fn transfer(
    config: FtpConfig,
    secrets: Arc<dyn SecretStore>,
    request: UploadRequest,
    progress: ProgressCallback,
) -> Result<UploadResult, UploadError> {
    let name = leaf_name(&request.file_name)?.to_owned();
    let password = read_secret(secrets.as_ref(), PASSWORD_KEY)?.unwrap_or_default();

    let url = file_url(&config.url_base, &name);
    let job = move || put(&config, &password, &name, request.data, progress);
    on_worker(job).await??;
    Ok(UploadResult { url: Some(url), ..UploadResult::default() })
}

/// The URL that users see for a file: the base plus the encoded file name.
pub fn file_url(url_base: &str, name: &str) -> String {
    let encoded = utf8_percent_encode(name, SEGMENT).to_string();
    format!("{}/{encoded}", url_base.trim_end_matches('/'))
}

/// Characters that stay literal in a URL path segment.
const SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// The blocking part: connect, secure the connection when configured, then transfer.
fn put(
    config: &FtpConfig,
    password: &str,
    name: &str,
    data: UploadData,
    progress: ProgressCallback,
) -> Result<(), UploadError> {
    match config.tls {
        FtpTls::None => {
            let ftp: FtpStream = open_control(tcp_connect(config)?)?;
            transfer_over(ftp, config, password, name, data, progress)
        }
        FtpTls::Explicit => {
            // Built before the connection, so a broken TLS setup fails without touching the server.
            let connector = connector()?;
            let ftp: TlsFtp = open_control(tcp_connect(config)?)?;
            let ftp = ftp.into_secure(connector, &config.host).map_err(ftp_error)?;
            transfer_over(ftp, config, password, name, data, progress)
        }
        FtpTls::Implicit => {
            let connector = TimedTls(connector()?);
            let port = if config.port == FTP_PORT { FTPS_IMPLICIT_PORT } else { config.port };
            let ftp = TlsFtp::connect_secure_implicit((config.host.as_str(), port), connector, &config.host)
                .map_err(ftp_error)?;
            transfer_over(ftp, config, password, name, data, progress)
        }
    }
}

/// Logs in, enters the directory and stores the file over an open control connection. Plain and
/// TLS connections share this.
fn transfer_over<T>(
    mut ftp: ImplFtpStream<T>,
    config: &FtpConfig,
    password: &str,
    name: &str,
    data: UploadData,
    progress: ProgressCallback,
) -> Result<(), UploadError>
where
    T: TlsStream + Send + 'static,
{
    ftp.login(config.username.as_str(), password).map_err(ftp_error)?;
    ftp.set_mode(if config.passive { Mode::Passive } else { Mode::Active });
    enter_directory(&mut ftp, &config.directory)?;
    // ASCII mode rewrites LF to CRLF, which corrupts binary files.
    ftp.transfer_type(FileType::Binary).map_err(ftp_error)?;
    match data {
        UploadData::Bytes(bytes) => {
            let total = bytes.len() as u64;
            store(&mut ftp, name, Cursor::new(bytes), total, progress)?;
        }
        UploadData::Path(path) => {
            let file = File::open(&path).map_err(|error| unreadable(&path, &error))?;
            let total = file.metadata().map_err(|error| unreadable(&path, &error))?.len();
            store(&mut ftp, name, file, total, progress)?;
        }
    }
    // The file is stored. A failed QUIT changes nothing for the user.
    let _ = ftp.quit();
    Ok(())
}

fn store<T>(
    ftp: &mut ImplFtpStream<T>,
    name: &str,
    source: impl Read,
    total: u64,
    progress: ProgressCallback,
) -> Result<(), UploadError>
where
    T: TlsStream + Send + 'static,
{
    let mut reader = Counting { inner: source, sent: 0, total, progress };
    ftp.put_file(name, &mut reader).map(|_| ()).map_err(ftp_error)
}

/// Reports bytes read from the source to the caller's progress callback.
struct Counting<R> {
    inner: R,
    sent: u64,
    total: u64,
    progress: ProgressCallback,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.sent += read as u64;
        (self.progress)(self.sent, Some(self.total));
        Ok(read)
    }
}

/// Opens the TCP connection, trying each resolved address within the connect timeout.
fn tcp_connect(config: &FtpConfig) -> Result<TcpStream, UploadError> {
    let addresses = (config.host.as_str(), config.port)
        .to_socket_addrs()
        .map_err(|error| UploadError::Network(format!("cannot resolve {}: {error}", config.host)))?;
    let mut last_error = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    let reason = last_error.map_or_else(|| "no address found".to_owned(), |error| error.to_string());
    Err(UploadError::Network(format!("cannot connect to {}:{}: {reason}", config.host, config.port)))
}

/// Sets timeouts before the greeting is read, so a silent server cannot hold the thread.
fn open_control<T>(stream: TcpStream) -> Result<ImplFtpStream<T>, UploadError>
where
    T: TlsStream + Send + 'static,
{
    set_io_timeouts(&stream).map_err(|error| UploadError::Network(error.to_string()))?;
    ImplFtpStream::connect_with_stream(stream).map_err(ftp_error)
}

fn set_io_timeouts(stream: &TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))
}

/// Puts the IO timeouts on the socket before the handshake. `connect_secure_implicit` opens the
/// socket and runs the handshake itself, so the timeouts have to be set here.
#[derive(Debug)]
struct TimedTls<C>(C);

impl<C> TlsConnector for TimedTls<C>
where
    C: TlsConnector + Send + Sync + 'static,
{
    type Stream = C::Stream;

    fn connect(&self, domain: &str, stream: TcpStream) -> FtpResult<Self::Stream> {
        set_io_timeouts(&stream).map_err(FtpError::ConnectionError)?;
        self.0.connect(domain, stream)
    }
}

/// The rustls connector: the platform verifier on the explicit AWS-LC provider, so the
/// certificate check uses the OS trust store and no other provider is picked up by accident.
#[cfg(not(windows))]
fn connector() -> Result<suppaftp::RustlsConnector, UploadError> {
    use rustls::ClientConfig;
    use rustls::crypto::aws_lc_rs::default_provider;
    use rustls_platform_verifier::BuilderVerifierExt;

    let config = ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .and_then(|builder| builder.with_platform_verifier())
        .map(|builder| builder.with_no_client_auth())
        .map_err(|error| UploadError::Network(format!("TLS setup: {error}")))?;
    Ok(suppaftp::RustlsConnector::from(Arc::new(config)))
}

/// The native-tls connector: SChannel, which checks certificates against the Windows trust store.
#[cfg(windows)]
fn connector() -> Result<suppaftp::NativeTlsConnector, UploadError> {
    let connector = native_tls::TlsConnector::new()
        .map_err(|error| UploadError::Network(format!("TLS setup: {error}")))?;
    Ok(suppaftp::NativeTlsConnector::from(connector))
}

/// `mkdir -p`: enters each segment, creating it when `CWD` fails. A failed `MKD` is ignored (the
/// directory may exist already), so the `CWD` after it reports the real error.
fn enter_directory<T>(ftp: &mut ImplFtpStream<T>, directory: &str) -> Result<(), UploadError>
where
    T: TlsStream + Send + 'static,
{
    if directory.starts_with('/') {
        ftp.cwd("/").map_err(ftp_error)?;
    }
    for part in directory.split('/').filter(|part| !part.is_empty() && *part != ".") {
        if ftp.cwd(part).is_err() {
            let _ = ftp.mkdir(part);
        }
        ftp.cwd(part).map_err(ftp_error)?;
    }
    Ok(())
}

fn leaf_name(name: &str) -> Result<&str, UploadError> {
    let plain = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control);
    if plain {
        Ok(name)
    } else {
        Err(UploadError::Unsupported(format!("{name:?} is not a plain file name")))
    }
}

fn read_secret(store: &dyn SecretStore, key: &str) -> Result<Option<String>, UploadError> {
    store.get(key).map_err(|error| match error {
        PortError::Unsupported(message) => UploadError::Unsupported(message),
        PortError::PermissionDenied => UploadError::Auth("the credential store denied access".to_owned()),
        PortError::Cancelled => UploadError::Cancelled,
        other => UploadError::Auth(other.to_string()),
    })
}

fn unreadable(path: &std::path::Path, error: &io::Error) -> UploadError {
    UploadError::Unsupported(format!("cannot read {}: {error}", path.display()))
}

fn ftp_error(error: FtpError) -> UploadError {
    match error {
        FtpError::ConnectionError(error) => network_error(error.to_string()),
        FtpError::SecureError(message) => network_error(format!("TLS: {message}")),
        FtpError::UnexpectedResponse(response) => {
            let code = response.status.code();
            let message = format!("FTP {code}: {}", String::from_utf8_lossy(&response.body).trim());
            match code {
                NOT_LOGGED_IN | NEED_ACCOUNT => UploadError::Auth(message),
                INSUFFICIENT_STORAGE | EXCEEDED_STORAGE => UploadError::Quota(message),
                SERVICE_NOT_AVAILABLE => UploadError::Network(message),
                _ => UploadError::BadResponse(message),
            }
        }
        other => UploadError::BadResponse(other.to_string()),
    }
}

/// A transport or TLS failure. A refused certificate is `Auth`, because the same server refuses it
/// again on every retry.
fn network_error(message: String) -> UploadError {
    if message.to_ascii_lowercase().contains("certificate") {
        UploadError::Auth(message)
    } else {
        UploadError::Network(message)
    }
}

/// Runs blocking `work` on its own thread and resolves with its result on any executor. A panic
/// in `work` becomes `BadResponse`, which is not retried.
async fn on_worker<R, F>(work: F) -> Result<R, UploadError>
where
    R: Send + 'static,
    F: FnOnce() -> R + Send + 'static,
{
    let slot = Arc::new(Mutex::new(Slot { value: None, waker: None }));
    let writer = Arc::clone(&slot);
    std::thread::spawn(move || {
        let value = catch_unwind(AssertUnwindSafe(work));
        let waker = {
            let mut slot = lock(&writer);
            slot.value = Some(value);
            slot.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    });
    let outcome = std::future::poll_fn(|cx| {
        let mut slot = lock(&slot);
        match slot.value.take() {
            Some(value) => Poll::Ready(value),
            None => {
                slot.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    })
    .await;
    outcome.map_err(|_| UploadError::BadResponse("the FTP worker thread panicked".to_owned()))
}

struct Slot<R> {
    value: Option<std::thread::Result<R>>,
    waker: Option<Waker>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_certificate_is_auth_and_other_tls_failures_are_network() {
        let refused = network_error("TLS: invalid peer certificate: UnknownIssuer".to_owned());
        assert!(matches!(refused, UploadError::Auth(_)), "{refused:?}");

        let failed = network_error("TLS: received fatal alert: HandshakeFailure".to_owned());
        assert!(matches!(failed, UploadError::Network(_)), "{failed:?}");
    }

    #[test]
    fn the_tls_connector_builds_on_this_target() {
        assert!(connector().is_ok());
    }
}
