//! E-mail through SMTP, with `lettre`'s blocking transport. The upload is one message with the file
//! as its attachment. The send runs on a short-lived thread (`run_blocking`), so the upload future
//! never blocks an executor. TLS follows the target (ADR 025), because lettre's features pick rustls
//! with the platform verifier on Linux and native-tls on Windows. The password is read from
//! `SecretStore` under `laqta/email/password`. The result URL is `mailto:` the recipients.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Poll, Waker};
use std::time::Duration;

use laqta_ports::BoxFuture;
use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadError, UploadRequest, UploadResult};
use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, Message, MultiPart};
use lettre::transport::smtp::Error as SmtpError;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{SmtpTransport, Transport};

use crate::hosts::{data_bytes, port_error};

/// Where the password lives in `SecretStore`. The account kind's id is `email`.
pub const PASSWORD_KEY: &str = "laqta/email/password";

const TIMEOUT: Duration = Duration::from_secs(60);
const OCTET_STREAM: &str = "application/octet-stream";

/// How the connection is secured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmtpTls {
    /// Plain, upgraded with STARTTLS (the default, port 587).
    StartTls,
    /// Implicit TLS from the first byte (port 465).
    Tls,
    /// No TLS. The password is sent in clear text when there is one.
    None,
}

/// One SMTP destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub tls: SmtpTls,
    /// The sign-in user name. Sent without authentication when empty.
    pub username: String,
    /// The From address, e.g. `Laqta <laqta@example.com>`.
    pub from: String,
    /// One or more addresses, comma-separated.
    pub to: String,
    /// `%filename%` is replaced by the upload's file name.
    pub subject: String,
}

/// The SMTP destination.
#[derive(Clone)]
pub struct Smtp {
    config: SmtpConfig,
    secrets: Arc<dyn SecretStore>,
}

impl Smtp {
    pub fn new(config: SmtpConfig, secrets: Arc<dyn SecretStore>) -> Self {
        Smtp { config, secrets }
    }
}

impl fmt::Debug for Smtp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Smtp").field("config", &self.config).finish_non_exhaustive()
    }
}

impl Destination for Smtp {
    fn id(&self) -> &str {
        "email"
    }

    fn name(&self) -> &str {
        "E-mail (SMTP)"
    }

    fn upload(&self, request: UploadRequest, _progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(transfer(self.config.clone(), Arc::clone(&self.secrets), request))
    }
}

async fn transfer(config: SmtpConfig, secrets: Arc<dyn SecretStore>, request: UploadRequest) -> Result<UploadResult, UploadError> {
    let UploadRequest { file_name, data, mime, .. } = request;
    let message = build_message(&config, &file_name, &mime, data_bytes(data)?)?;
    let url = mailto(&config.to)?;
    let password = secrets.get(PASSWORD_KEY).map_err(port_error)?;
    run_blocking(move || send(&config, password, &message)).await??;
    Ok(UploadResult { url: Some(url), ..UploadResult::default() })
}

/// The message: From, To and the subject, with the file as its only part. It does not connect, so
/// tests check it directly.
pub fn build_message(config: &SmtpConfig, file_name: &str, mime: &str, data: Vec<u8>) -> Result<Message, UploadError> {
    let from: Mailbox =
        config.from.trim().parse().map_err(|error| UploadError::Unsupported(format!("the From address {:?}: {error}", config.from)))?;
    let recipients = recipients(&config.to)?;
    let content_type = ContentType::parse(mime)
        .or_else(|_| ContentType::parse(OCTET_STREAM))
        .map_err(|error| UploadError::Unsupported(format!("the content type {mime:?}: {error}")))?;
    let attachment = Attachment::new(file_name.to_owned()).body(data, content_type);

    let mut builder = Message::builder().from(from).subject(config.subject.replace("%filename%", file_name));
    for mailbox in recipients {
        builder = builder.to(mailbox);
    }
    builder
        .multipart(MultiPart::mixed().singlepart(attachment))
        .map_err(|error| UploadError::Unsupported(format!("cannot build the e-mail: {error}")))
}

/// The recipients, from a comma-separated list. At least one is needed.
fn recipients(to: &str) -> Result<Vec<Mailbox>, UploadError> {
    let mailboxes = to
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<Mailbox>().map_err(|error| UploadError::Unsupported(format!("the To address {part:?}: {error}"))))
        .collect::<Result<Vec<_>, _>>()?;
    if mailboxes.is_empty() {
        return Err(UploadError::Unsupported("no To address is set".to_owned()));
    }
    Ok(mailboxes)
}

/// The result URL: `mailto:` the recipients' addresses.
fn mailto(to: &str) -> Result<String, UploadError> {
    let addresses: Vec<String> = recipients(to)?.iter().map(|mailbox| mailbox.email.to_string()).collect();
    Ok(format!("mailto:{}", addresses.join(",")))
}

/// Connects, signs in when a user name is set, and sends.
fn send(config: &SmtpConfig, password: Option<String>, message: &Message) -> Result<(), UploadError> {
    let transport = transport(config, password)?;
    transport.send(message).map(|_| ()).map_err(smtp_error)
}

fn transport(config: &SmtpConfig, password: Option<String>) -> Result<SmtpTransport, UploadError> {
    let relay = match config.tls {
        SmtpTls::StartTls => SmtpTransport::starttls_relay(&config.host),
        SmtpTls::Tls => SmtpTransport::relay(&config.host),
        SmtpTls::None => Ok(SmtpTransport::builder_dangerous(config.host.as_str())),
    }
    .map_err(smtp_error)?;
    let mut builder = relay.port(config.port).timeout(Some(TIMEOUT));
    if !config.username.is_empty() {
        builder = builder.credentials(Credentials::new(config.username.clone(), password.unwrap_or_default()));
    }
    Ok(builder.build())
}

/// SMTP failures: a refused sign-in is `Auth`, a permanent reply `BadResponse`, and a temporary one
/// or a dropped connection `Network`, which retries.
fn smtp_error(error: SmtpError) -> UploadError {
    if error.is_timeout() {
        return UploadError::Network("the SMTP server timed out".to_owned());
    }
    match error.status().map(|code| code.to_string()).as_deref() {
        Some("530" | "534" | "535") => UploadError::Auth(error.to_string()),
        _ if error.is_permanent() => UploadError::BadResponse(error.to_string()),
        _ if error.is_client() => UploadError::Unsupported(error.to_string()),
        _ => UploadError::Network(error.to_string()),
    }
}

/// Runs blocking `work` on its own thread and resolves with its result on any executor. A panic in
/// `work` becomes `BadResponse`, which is not retried. Same pattern as `ftp.rs`.
pub(crate) async fn run_blocking<R, F>(work: F) -> Result<R, UploadError>
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
    outcome.map_err(|_| UploadError::BadResponse("the blocking worker thread panicked".to_owned()))
}

struct Slot<R> {
    value: Option<std::thread::Result<R>>,
    waker: Option<Waker>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
