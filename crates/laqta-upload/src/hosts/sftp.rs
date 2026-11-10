//! SFTP (ADR 025). On Linux, `russh` and `russh-sftp` run on `laqta_http`'s Tokio runtime (ADR 007):
//! one `run` call connects, checks the host key, authenticates, and writes the file. On Windows, the
//! system OpenSSH `sftp.exe` runs in batch mode, with argv built here and a batch script on stdin.
//! Passwords are read from `SecretStore` under `laqta/sftp/password` (Linux only), and the key
//! passphrase under `laqta/sftp/key_passphrase`.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadData, UploadError, UploadRequest, UploadResult};
use laqta_ports::{BoxFuture, PortError};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::{Deserialize, Serialize};

#[cfg(not(windows))]
use std::time::Duration;

#[cfg(windows)]
use std::io::Write;
#[cfg(windows)]
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(windows)]
use std::sync::{Mutex, MutexGuard, PoisonError};
#[cfg(windows)]
use std::task::{Poll, Waker};

/// Where the password lives in `SecretStore`.
pub const PASSWORD_KEY: &str = "laqta/sftp/password";
/// Where the key passphrase lives in `SecretStore`.
pub const PASSPHRASE_KEY: &str = "laqta/sftp/key_passphrase";

#[cfg(not(windows))]
const IO_TIMEOUT: Duration = Duration::from_secs(60);

/// One SFTP destination.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SftpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// Private key file. Without one, Linux signs in with the stored password, and Windows uses
    /// the agent and the default keys.
    pub key_path: Option<PathBuf>,
    /// Remote directory, created when missing. A leading `/` makes it absolute.
    pub directory: String,
    /// Base of the URL shared with users; the file name is appended.
    pub url_base: String,
}

/// The SFTP destination.
#[derive(Clone)]
pub struct Sftp {
    config: SftpConfig,
    secrets: Arc<dyn SecretStore>,
}

impl Sftp {
    pub fn new(config: SftpConfig, secrets: Arc<dyn SecretStore>) -> Self {
        Sftp { config, secrets }
    }
}

impl fmt::Debug for Sftp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sftp").field("config", &self.config).finish_non_exhaustive()
    }
}

impl Destination for Sftp {
    fn id(&self) -> &str {
        "sftp"
    }

    fn name(&self) -> &str {
        "SFTP"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(transfer(self.config.clone(), Arc::clone(&self.secrets), request, progress))
    }
}

async fn transfer(
    config: SftpConfig,
    secrets: Arc<dyn SecretStore>,
    request: UploadRequest,
    progress: ProgressCallback,
) -> Result<UploadResult, UploadError> {
    let name = leaf_name(&request.file_name)?.to_owned();
    let url = join_url(&config.url_base, &name);
    put(config, secrets, name, request.data, progress).await?;
    Ok(UploadResult { url: Some(url), ..UploadResult::default() })
}

/// Splits a remote directory into the directory to `cd` into and the prefixes to create, outermost
/// first. `""` is the home directory, `"/"` the root. Pure, so it is tested on every target.
pub fn layout(directory: &str) -> (String, Vec<String>) {
    let absolute = directory.starts_with('/');
    let parts: Vec<&str> = directory.split('/').filter(|part| !part.is_empty() && *part != ".").collect();
    let dir = if absolute { format!("/{}", parts.join("/")) } else { parts.join("/") };
    let creates = (1..=parts.len())
        .map(|count| {
            let path = parts[..count].join("/");
            if absolute { format!("/{path}") } else { path }
        })
        .collect();
    (dir, creates)
}

/// The `sftp.exe` arguments: batch mode from stdin, the port, the key, and `user@host`. Host and
/// user are checked so that neither can read as an option. Pure, so it is tested on every target.
pub fn command_args(config: &SftpConfig, key: Option<&Path>) -> Result<Vec<String>, UploadError> {
    for value in [&config.host, &config.username] {
        if value.is_empty() || value.starts_with('-') || value.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(UploadError::Unsupported(format!("{value:?} is not a valid SFTP host or user name")));
        }
    }
    let mut args = vec![
        "-b".to_owned(),
        "-".to_owned(),
        // Fails instead of prompting: a password or an unknown host key would hang the batch.
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
        // A server that never answers, or stops answering, ends the batch instead of hanging it.
        "-o".to_owned(),
        "ConnectTimeout=30".to_owned(),
        "-o".to_owned(),
        "ServerAliveInterval=15".to_owned(),
        "-o".to_owned(),
        "ServerAliveCountMax=4".to_owned(),
        "-P".to_owned(),
        config.port.to_string(),
    ];
    if let Some(key) = key {
        args.push("-i".to_owned());
        args.push(key.to_string_lossy().into_owned());
    }
    args.push(format!("{}@{}", config.username, config.host));
    Ok(args)
}

/// The batch script for `sftp.exe`: `-mkdir` for each prefix (`-` ignores "exists"), `cd`, then
/// `put`. Every argument is quoted; a control character is refused, because a newline would start
/// another command. Backslashes in the local path become `/`, which the Windows client accepts.
pub fn batch_script(directory: &str, local: &Path, remote_name: &str) -> Result<String, UploadError> {
    let (dir, creates) = layout(directory);
    let mut script = String::new();
    for path in &creates {
        script.push_str(&format!("-mkdir {}\n", quote(path)?));
    }
    if !dir.is_empty() {
        script.push_str(&format!("cd {}\n", quote(&dir)?));
    }
    let local = local.to_string_lossy().replace('\\', "/");
    script.push_str(&format!("put {} {}\n", quote(&local)?, quote(remote_name)?));
    Ok(script)
}

fn quote(argument: &str) -> Result<String, UploadError> {
    if argument.chars().any(char::is_control) {
        return Err(UploadError::Unsupported(format!("{argument:?} has a control character, which an SFTP batch cannot carry")));
    }
    let mut quoted = String::with_capacity(argument.len() + 2);
    quoted.push('"');
    for c in argument.chars() {
        if c == '"' || c == '\\' {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    Ok(quoted)
}

/// The remote path of the file inside `dir` (the value `layout` returned).
#[cfg(not(windows))]
fn remote_file(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

#[cfg(not(windows))]
async fn put(
    config: SftpConfig,
    secrets: Arc<dyn SecretStore>,
    name: String,
    data: UploadData,
    progress: ProgressCallback,
) -> Result<(), UploadError> {
    let login = match &config.key_path {
        Some(path) => {
            let passphrase = read_secret(secrets.as_ref(), PASSPHRASE_KEY)?;
            Login::Key(Arc::new(load_key(path, passphrase.as_deref())?))
        }
        None => match read_secret(secrets.as_ref(), PASSWORD_KEY)? {
            Some(password) => Login::Password(password),
            None => {
                return Err(UploadError::Auth(format!("no key file or password is set for {}", config.host)));
            }
        },
    };
    let job = Put {
        host: config.host,
        port: config.port,
        username: config.username,
        login,
        directory: config.directory,
        name,
        bytes: into_bytes(data)?,
        progress,
    };
    laqta_http::run(put_over_ssh(job)).await
}

#[cfg(not(windows))]
pub(crate) enum Login {
    Key(Arc<russh::keys::PrivateKey>),
    Password(String),
}

#[cfg(not(windows))]
struct Put {
    host: String,
    port: u16,
    username: String,
    login: Login,
    directory: String,
    name: String,
    bytes: Vec<u8>,
    progress: ProgressCallback,
}

/// Connects, authenticates, creates the directories and writes the file. It runs on the HTTP
/// runtime, so it awaits and never blocks it.
#[cfg(not(windows))]
async fn put_over_ssh(put: Put) -> Result<(), UploadError> {
    let Put { host, port, username, login, directory, name, bytes, progress } = put;
    let config = Arc::new(russh::client::Config { inactivity_timeout: Some(IO_TIMEOUT), ..russh::client::Config::default() });
    let handler = HostKeys { host: host.clone(), port };
    let mut handle = russh::client::connect(config, (host.clone(), port), handler).await.map_err(client_error)?;

    let accepted = match login {
        Login::Key(key) => handle.authenticate_publickey(username.clone(), russh::keys::PrivateKeyWithHashAlg::new(key, None)).await,
        Login::Password(password) => handle.authenticate_password(username.clone(), password).await,
    };
    match accepted.map_err(ssh_error)? {
        russh::client::AuthResult::Success => {}
        russh::client::AuthResult::Failure { .. } => {
            return Err(UploadError::Auth(format!("{host} refused the login for {username}")));
        }
    }

    let channel = handle.channel_open_session().await.map_err(ssh_error)?;
    channel.request_subsystem(true, "sftp").await.map_err(ssh_error)?;
    let sftp = russh_sftp::client::SftpSession::new(channel.into_stream()).await.map_err(sftp_error)?;

    let (dir, creates) = layout(&directory);
    for path in &creates {
        if !sftp.try_exists(path.as_str()).await.map_err(sftp_error)? {
            sftp.create_dir(path.as_str()).await.map_err(sftp_error)?;
        }
    }
    let remote = remote_file(&dir, &name);
    let total = bytes.len() as u64;
    progress(0, Some(total));
    // `write` opens without CREATE, so the file is created (and truncated) first.
    sftp.create(remote.as_str()).await.map_err(sftp_error)?.close().await.map_err(io_failure)?;
    sftp.write(remote.as_str(), &bytes).await.map_err(sftp_error)?;
    progress(total, Some(total));

    let _ = sftp.close().await;
    let _ = handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
    Ok(())
}

/// Accepts a server key only when `~/.ssh/known_hosts` already records it, as `sftp.exe` does on
/// Windows. Unknown and changed keys fail, and the message says what to do.
#[cfg(not(windows))]
pub(crate) struct HostKeys {
    pub(crate) host: String,
    pub(crate) port: u16,
}

#[cfg(not(windows))]
impl russh::client::Handler for HostKeys {
    type Error = ClientError;

    async fn check_server_key(&mut self, server_key: &russh::keys::PublicKeyOrCertificate) -> Result<bool, ClientError> {
        let russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } = server_key else {
            return Err(ClientError::HostKey(format!("{} sent a certificate, which is not accepted", self.host)));
        };
        match russh::keys::check_known_hosts(&self.host, self.port, key) {
            Ok(true) => Ok(true),
            Ok(false) => {
                Err(ClientError::HostKey(format!("{} is not in ~/.ssh/known_hosts; connect once with ssh to record its key", self.host)))
            }
            Err(error) => Err(ClientError::HostKey(format!("cannot verify the host key of {}: {error}", self.host))),
        }
    }
}

#[cfg(not(windows))]
#[derive(Debug)]
pub(crate) enum ClientError {
    Ssh(russh::Error),
    HostKey(String),
}

#[cfg(not(windows))]
impl From<russh::Error> for ClientError {
    fn from(error: russh::Error) -> Self {
        ClientError::Ssh(error)
    }
}

#[cfg(not(windows))]
pub(crate) fn client_error(error: ClientError) -> UploadError {
    match error {
        ClientError::Ssh(error) => ssh_error(error),
        ClientError::HostKey(message) => UploadError::Auth(message),
    }
}

#[cfg(not(windows))]
pub(crate) fn ssh_error(error: russh::Error) -> UploadError {
    UploadError::Network(error.to_string())
}

#[cfg(not(windows))]
pub(crate) fn sftp_error(error: russh_sftp::client::error::Error) -> UploadError {
    use russh_sftp::client::error::Error;
    use russh_sftp::protocol::StatusCode;
    match error {
        Error::Status(status) if status.status_code == StatusCode::PermissionDenied => UploadError::Auth(status.error_message),
        Error::Timeout => UploadError::Network("the SFTP server did not answer in time".to_owned()),
        Error::IO(message) => UploadError::Network(message),
        other => UploadError::BadResponse(other.to_string()),
    }
}

#[cfg(not(windows))]
pub(crate) fn load_key(path: &Path, passphrase: Option<&str>) -> Result<russh::keys::PrivateKey, UploadError> {
    russh::keys::load_secret_key(path, passphrase)
        .map_err(|error| UploadError::Auth(format!("cannot read the SFTP key {}: {error}", path.display())))
}

#[cfg(not(windows))]
fn into_bytes(data: UploadData) -> Result<Vec<u8>, UploadError> {
    match data {
        UploadData::Bytes(bytes) => Ok(bytes),
        UploadData::Path(path) => {
            std::fs::read(&path).map_err(|error| UploadError::Unsupported(format!("cannot read {}: {error}", path.display())))
        }
    }
}

/// Windows: `sftp.exe` in batch mode. A password is not available here, so a stored password
/// with no key file is `Unsupported`, and the user is asked to set up a key.
#[cfg(windows)]
async fn put(
    config: SftpConfig,
    secrets: Arc<dyn SecretStore>,
    name: String,
    data: UploadData,
    _progress: ProgressCallback,
) -> Result<(), UploadError> {
    if config.key_path.is_none() && read_secret(secrets.as_ref(), PASSWORD_KEY)?.is_some() {
        return Err(UploadError::Unsupported(
            "SFTP password sign-in is not available on Windows; set a key file for this destination".to_owned(),
        ));
    }
    on_worker(move |cancelled| windows_put(&config, &name, data, cancelled)).await?
}

#[cfg(windows)]
fn windows_put(config: &SftpConfig, name: &str, data: UploadData, cancelled: &std::sync::atomic::AtomicBool) -> Result<(), UploadError> {
    let (local, temporary) = match data {
        UploadData::Path(path) => (path, None),
        UploadData::Bytes(bytes) => {
            let path = write_temporary(&bytes)?;
            (path.clone(), Some(path))
        }
    };
    let result = run_batch(config, name, &local, cancelled);
    if let Some(path) = temporary {
        let _ = std::fs::remove_file(path);
    }
    result
}

#[cfg(windows)]
fn run_batch(config: &SftpConfig, name: &str, local: &Path, cancelled: &std::sync::atomic::AtomicBool) -> Result<(), UploadError> {
    let script = batch_script(&config.directory, local, name)?;
    let args = command_args(config, config.key_path.as_deref())?;
    let output = run_sftp(&args, &script, cancelled)?;
    if output.status.success() { Ok(()) } else { Err(sftp_failure(&output.stderr)) }
}

/// The system client, from `%SystemRoot%\System32\OpenSSH` when present, else from `PATH`.
#[cfg(windows)]
fn sftp_program() -> PathBuf {
    if let Some(root) = std::env::var_os("SystemRoot") {
        let path = PathBuf::from(root).join("System32").join("OpenSSH").join("sftp.exe");
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from("sftp.exe")
}

/// Runs the batch and waits for it. When `cancelled` is set (the upload's future was dropped), the
/// child is killed and reaped, so a cancelled upload leaves no `sftp.exe` behind.
#[cfg(windows)]
fn run_sftp(args: &[String], script: &str, cancelled: &std::sync::atomic::AtomicBool) -> Result<std::process::Output, UploadError> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    // A GUI process would otherwise open a console window for the child.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut child = Command::new(sftp_program())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                UploadError::Unsupported("sftp.exe was not found; install the OpenSSH client feature".to_owned())
            } else {
                io_failure(error)
            }
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        // A failed write means sftp already exited; its status and stderr explain why.
        let _ = stdin.write_all(script.as_bytes());
    }
    // Drain both pipes on their own threads so a full pipe can't stall the child while we poll it.
    let drain = |pipe: Option<Box<dyn io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn io::Read + Send>));
    let stderr = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn io::Read + Send>));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io_failure(error));
            }
        }
        if cancelled.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(UploadError::Cancelled);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    Ok(std::process::Output { status, stdout: stdout.join().unwrap_or_default(), stderr: stderr.join().unwrap_or_default() })
}

#[cfg(windows)]
fn sftp_failure(stderr: &[u8]) -> UploadError {
    let text = String::from_utf8_lossy(stderr).trim().to_owned();
    let lower = text.to_ascii_lowercase();
    let auth = ["permission denied", "host key verification failed", "too many authentication failures"];
    let network = ["could not resolve", "connection", "timed out", "no route"];
    if auth.iter().any(|needle| lower.contains(needle)) {
        UploadError::Auth(text)
    } else if network.iter().any(|needle| lower.contains(needle)) {
        UploadError::Network(text)
    } else {
        UploadError::BadResponse(text)
    }
}

#[cfg(windows)]
fn write_temporary(bytes: &[u8]) -> Result<PathBuf, UploadError> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_nanos());
    let path = std::env::temp_dir().join(format!("laqta-sftp-{}-{nanos}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(io_failure)?;
    file.write_all(bytes).map_err(io_failure)?;
    Ok(path)
}

/// Sets its flag when dropped: held by the upload's future, so dropping the future tells the worker.
#[cfg(windows)]
struct CancelOnDrop(Arc<std::sync::atomic::AtomicBool>);

#[cfg(windows)]
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Runs blocking `work` on its own thread and resolves with its result on any executor. `work`
/// gets a flag that turns true when this future is dropped before the work ends. A panic in `work`
/// becomes `BadResponse`, which is not retried.
#[cfg(windows)]
async fn on_worker<R, F>(work: F) -> Result<R, UploadError>
where
    R: Send + 'static,
    F: FnOnce(&std::sync::atomic::AtomicBool) -> R + Send + 'static,
{
    let slot = Arc::new(Mutex::new(Slot { value: None, waker: None }));
    let writer = Arc::clone(&slot);
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _cancel_on_drop = CancelOnDrop(Arc::clone(&cancelled));
    std::thread::spawn(move || {
        let value = catch_unwind(AssertUnwindSafe(|| work(&cancelled)));
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
    outcome.map_err(|_| UploadError::BadResponse("the SFTP worker thread panicked".to_owned()))
}

#[cfg(windows)]
struct Slot<R> {
    value: Option<std::thread::Result<R>>,
    waker: Option<Waker>,
}

#[cfg(windows)]
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn leaf_name(name: &str) -> Result<&str, UploadError> {
    let plain = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\']) && !name.chars().any(char::is_control);
    if plain { Ok(name) } else { Err(UploadError::Unsupported(format!("{name:?} is not a plain file name"))) }
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

fn join_url(base: &str, name: &str) -> String {
    let encoded = utf8_percent_encode(name, SEGMENT).to_string();
    format!("{}/{encoded}", base.trim_end_matches('/'))
}

fn read_secret(store: &dyn SecretStore, key: &str) -> Result<Option<String>, UploadError> {
    store.get(key).map_err(|error| match error {
        PortError::Unsupported(message) => UploadError::Unsupported(message),
        PortError::PermissionDenied => UploadError::Auth("the credential store denied access".to_owned()),
        PortError::Cancelled => UploadError::Cancelled,
        other => UploadError::Auth(other.to_string()),
    })
}

fn io_failure(error: io::Error) -> UploadError {
    UploadError::Network(error.to_string())
}
