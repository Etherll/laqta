//! Remote storage for the browser window: the `RemoteStorage` port for every S3, FTP/FTPS
//! and SFTP account. Paths are the protocol's own: S3 uses object keys (folders end in `/`), FTP and
//! SFTP use server paths, relative to the login directory unless they start with `/`. An empty path
//! means the account's start folder: its `path_prefix` for S3, its `directory` for FTP and SFTP.
//! Every path a listing returns is complete, so it can be passed back as it is.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use laqta_domain::hosts::{HostAccount, HostKind};
use laqta_ports::tools::RemoteStorage;
use laqta_ports::upload::{SecretStore, UploadError};
use laqta_ports::{BoxFuture, PortError};

use crate::hosts::port_error;
use crate::native::HostDeps;

/// A future that fails at once with `error`.
fn failed<T: Send + 'static>(error: PortError) -> BoxFuture<'static, Result<T, PortError>> {
    Box::pin(async move { Err(error) })
}

/// The remote storage of every S3, FTP and SFTP account, with a label for the browser's list.
pub fn remote_storages(accounts: &[HostAccount], deps: &HostDeps) -> Vec<(String, Arc<dyn RemoteStorage>)> {
    let mut out: Vec<(String, Arc<dyn RemoteStorage>)> = Vec::new();
    for account in accounts {
        match account.kind {
            #[cfg(feature = "host-s3")]
            HostKind::S3 => {
                let config = s3_config(account);
                let label = format!("{}: {}", HostKind::S3.label(), config.bucket);
                out.push((label, Arc::new(s3_remote::S3Remote { config, secrets: deps.secrets.clone() })));
            }
            #[cfg(feature = "host-ftp")]
            HostKind::Ftp => {
                let config = ftp_config(account);
                let label = format!("{}: {}", HostKind::Ftp.label(), config.host);
                out.push((label, Arc::new(ftp_remote::FtpRemote { config, secrets: deps.secrets.clone() })));
            }
            #[cfg(feature = "host-sftp")]
            HostKind::Sftp => {
                let config = sftp_config(account);
                let label = format!("{}: {}", HostKind::Sftp.label(), config.host);
                out.push((label, Arc::new(sftp_remote::SftpRemote { config, secrets: deps.secrets.clone() })));
            }
            _ => {}
        }
    }
    out
}

/// A secret from the store; an empty value counts as no secret.
fn secret(store: &dyn SecretStore, key: &str) -> Result<Option<String>, UploadError> {
    store.get(key).map_err(port_error).map(|value| value.filter(|value| !value.is_empty()))
}

/// The port's error for a failed transfer. Unsupported and cancelled keep their meaning; the rest
/// keep their message.
fn to_port(error: UploadError) -> PortError {
    match error {
        UploadError::Unsupported(message) => PortError::Unsupported(message),
        UploadError::Cancelled => PortError::Cancelled,
        other => PortError::Failed(other.to_string()),
    }
}

/// `path` as given, or `start` when the path is empty.
fn start_or(path: String, start: &str) -> String {
    if path.is_empty() { start.to_owned() } else { path }
}

/// A folder path with its trailing `/` (S3 folders are prefixes). The root stays empty.
fn with_slash(path: &str) -> String {
    if path.is_empty() || path.ends_with('/') { path.to_owned() } else { format!("{path}/") }
}

/// `name` in `dir`, with one `/` between them.
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The file name of a local path, for an upload.
fn leaf(path: &Path) -> Result<String, UploadError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| UploadError::Unsupported(format!("{} has no file name", path.display())))
}

/// The content type for an upload to remote storage, from the extension.
#[cfg(feature = "host-s3")]
fn mime_for(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("txt" | "log" | "md") => "text/plain",
        Some("html" | "htm") => "text/html",
        Some("pdf") => "application/pdf",
        Some("mp4") => "video/mp4",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

/// One entry of a directory listing, as the line parsers read it.
#[cfg(feature = "host-ftp")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedEntry {
    pub name: String,
    pub is_dir: bool,
    /// Files only.
    pub size: Option<u64>,
}

#[cfg(feature = "host-ftp")]
fn listed(file: &suppaftp::list::File) -> ListedEntry {
    ListedEntry { name: file.name().to_owned(), is_dir: file.is_directory(), size: (!file.is_directory()).then(|| file.size() as u64) }
}

/// One line of an FTP `LIST` reply: the Unix (`ls -l`) layout or the Windows (DOS) layout.
#[cfg(feature = "host-ftp")]
pub fn parse_list_line(line: &str) -> Option<ListedEntry> {
    let file = suppaftp::list::ListParser::parse_posix(line).or_else(|_| suppaftp::list::ListParser::parse_dos(line)).ok()?;
    Some(listed(&file))
}

/// One line of an FTP `MLSD` reply (RFC 3659 facts, `type=…;size=…; name`).
#[cfg(feature = "host-ftp")]
pub fn parse_mlsd_line(line: &str) -> Option<ListedEntry> {
    suppaftp::list::ListParser::parse_mlsd(line).ok().map(|file| listed(&file))
}

#[cfg(feature = "host-s3")]
fn s3_config(account: &HostAccount) -> crate::hosts::s3::S3Config {
    crate::hosts::s3::S3Config {
        endpoint: account.get("endpoint").into(),
        region: account.get("region").into(),
        bucket: account.get("bucket").into(),
        path_prefix: account.get("path_prefix").into(),
        access_key_id: account.get("access_key_id").into(),
        use_path_style: account.flag("use_path_style"),
        public_url_base: None,
        acl_public_read: false,
    }
}

#[cfg(feature = "host-ftp")]
fn ftp_config(account: &HostAccount) -> crate::hosts::ftp::FtpConfig {
    use crate::hosts::ftp::FtpTls;
    let tls = match account.get("tls").trim() {
        "explicit" => FtpTls::Explicit,
        "implicit" => FtpTls::Implicit,
        _ => FtpTls::None,
    };
    crate::hosts::ftp::FtpConfig {
        host: account.get("host").into(),
        port: account.get("port").trim().parse().unwrap_or(21),
        username: account.get("username").into(),
        directory: account.get("directory").into(),
        url_base: String::new(),
        passive: true,
        tls,
    }
}

#[cfg(feature = "host-sftp")]
fn sftp_config(account: &HostAccount) -> crate::hosts::sftp::SftpConfig {
    let key = account.get("key_path");
    crate::hosts::sftp::SftpConfig {
        host: account.get("host").into(),
        port: account.get("port").trim().parse().unwrap_or(22),
        username: account.get("username").into(),
        key_path: (!key.is_empty()).then(|| PathBuf::from(key)),
        directory: account.get("directory").into(),
        url_base: String::new(),
    }
}

#[cfg(feature = "host-s3")]
mod s3_remote {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use laqta_http::{Method, Request};
    use laqta_ports::tools::{RemoteEntry, RemoteStorage};
    use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadData, UploadError, UploadKind, UploadRequest};
    use laqta_ports::{BoxFuture, PortError};
    use percent_encoding::utf8_percent_encode;
    use rusty_s3::actions::ListObjectsV2;
    use rusty_s3::{Credentials, S3Action};

    use super::{leaf, mime_for, secret, start_or, to_port, with_slash};
    use crate::hosts::PATH_SEGMENT;
    use crate::hosts::call;
    use crate::hosts::s3::{self, S3, S3Config};

    /// How long each presigned request stays valid. Each request starts at once.
    const PRESIGN: Duration = Duration::from_secs(15 * 60);

    pub(super) struct S3Remote {
        pub(super) config: S3Config,
        pub(super) secrets: Arc<dyn SecretStore>,
    }

    /// One listing: the folders directly under the prefix, and the objects under it.
    #[derive(Default)]
    struct Listing {
        folders: Vec<String>,
        objects: Vec<Object>,
    }

    struct Object {
        key: String,
        size: Option<u64>,
        modified: Option<String>,
    }

    fn credentials(secrets: &dyn SecretStore, config: &S3Config) -> Result<Credentials, UploadError> {
        let key = secret(secrets, s3::SECRET_KEY)?
            .ok_or_else(|| UploadError::Auth(format!("no S3 secret access key is stored under {}", s3::SECRET_KEY)))?;
        Ok(Credentials::new(config.access_key_id.clone(), key))
    }

    /// The keys under `prefix`, one page at a time. With `delimiter` set, the keys below the next
    /// `/` come back as folders. The prefix's own marker object is left out.
    async fn listing(config: &S3Config, secrets: &dyn SecretStore, prefix: &str, delimiter: Option<&str>) -> Result<Listing, UploadError> {
        let bucket = s3::bucket(config)?;
        let credentials = credentials(secrets, config)?;
        let mut listing = Listing::default();
        let mut token: Option<String> = None;
        loop {
            let mut action = bucket.list_objects_v2(Some(&credentials));
            action.with_prefix(prefix.to_owned());
            if let Some(delimiter) = delimiter {
                action.with_delimiter(delimiter.to_owned());
            }
            if let Some(token) = token.take() {
                action.with_continuation_token(token);
            }
            let url = action.sign(PRESIGN);
            let reply = call(Request::new(Method::Get, url.as_str()), None).await?;
            let page =
                ListObjectsV2::parse_response(&reply.text()).map_err(|error| UploadError::BadResponse(format!("S3 listing: {error}")))?;
            listing.folders.extend(page.common_prefixes.into_iter().map(|folder| folder.prefix));
            listing.objects.extend(page.contents.into_iter().filter(|object| object.key != prefix).map(|object| Object {
                key: object.key,
                size: object.size,
                modified: object.last_modified,
            }));
            match page.next_continuation_token {
                Some(next) => token = Some(next),
                None => return Ok(listing),
            }
        }
    }

    /// `key` without `prefix`, and without the trailing `/` of a folder.
    fn name_under(key: &str, prefix: &str) -> String {
        key.strip_prefix(prefix).unwrap_or(key).trim_end_matches('/').to_owned()
    }

    /// Each segment percent-encoded, slashes kept: the form a copy source header takes.
    fn encode_key(key: &str) -> String {
        key.split('/').map(|segment| utf8_percent_encode(segment, PATH_SEGMENT).to_string()).collect::<Vec<_>>().join("/")
    }

    async fn download(config: &S3Config, secrets: &dyn SecretStore, key: &str, dest: &Path) -> Result<(), UploadError> {
        let bucket = s3::bucket(config)?;
        let credentials = credentials(secrets, config)?;
        let url = bucket.get_object(Some(&credentials), key).sign(PRESIGN);
        let reply = call(Request::new(Method::Get, url.as_str()), None).await?;
        std::fs::write(dest, &reply.body).map_err(|error| UploadError::Unsupported(format!("cannot write {}: {error}", dest.display())))
    }

    /// A server-side copy (`PUT` with `x-amz-copy-source`), then the delete of the source. S3 has no
    /// rename.
    async fn rename(config: &S3Config, secrets: &dyn SecretStore, from: &str, to: &str) -> Result<(), UploadError> {
        // A folder is every key under its prefix, and copying the marker alone would leave them behind.
        if from.ends_with('/') {
            return Err(UploadError::Unsupported(format!("renaming the folder {from} is not supported on S3")));
        }
        let source = format!("/{}/{}", config.bucket, encode_key(from));
        let url = {
            let bucket = s3::bucket(config)?;
            let credentials = credentials(secrets, config)?;
            let mut copy = bucket.put_object(Some(&credentials), to);
            copy.headers_mut().insert("x-amz-copy-source", source.clone());
            copy.sign(PRESIGN)
        };
        let mut request = Request::new(Method::Put, url.as_str());
        request.headers.push(("x-amz-copy-source".to_owned(), source));
        call(request, None).await?;
        delete_key(config, secrets, from).await
    }

    async fn delete(config: &S3Config, secrets: &dyn SecretStore, path: &str) -> Result<(), UploadError> {
        if path.ends_with('/') && !listing(config, secrets, path, None).await?.objects.is_empty() {
            return Err(UploadError::Unsupported(format!("the folder {path} is not empty")));
        }
        delete_key(config, secrets, path).await
    }

    async fn delete_key(config: &S3Config, secrets: &dyn SecretStore, key: &str) -> Result<(), UploadError> {
        let bucket = s3::bucket(config)?;
        let credentials = credentials(secrets, config)?;
        let url = bucket.delete_object(Some(&credentials), key).sign(PRESIGN);
        call(Request::new(Method::Delete, url.as_str()), None).await.map(|_| ())
    }

    /// An empty object whose key ends in `/`: the folder marker that other S3 browsers show.
    async fn mkdir(config: &S3Config, secrets: &dyn SecretStore, path: &str) -> Result<(), UploadError> {
        let key = with_slash(path);
        if key.is_empty() {
            return Err(UploadError::Unsupported("a folder needs a name".to_owned()));
        }
        let bucket = s3::bucket(config)?;
        let credentials = credentials(secrets, config)?;
        let url = bucket.put_object(Some(&credentials), &key).sign(PRESIGN);
        call(Request::new(Method::Put, url.as_str()), None).await.map(|_| ())
    }

    impl RemoteStorage for S3Remote {
        fn list(&self, dir: String) -> BoxFuture<'_, Result<Vec<RemoteEntry>, PortError>> {
            let config = self.config.clone();
            let secrets = Arc::clone(&self.secrets);
            let prefix = with_slash(&start_or(dir, &config.path_prefix));
            Box::pin(async move {
                let found = listing(&config, secrets.as_ref(), &prefix, Some("/")).await.map_err(to_port)?;
                let folders = found.folders.into_iter().map(|folder| RemoteEntry {
                    name: name_under(&folder, &prefix),
                    path: folder,
                    is_dir: true,
                    size: None,
                    modified: None,
                });
                let files = found.objects.into_iter().map(|object| RemoteEntry {
                    name: name_under(&object.key, &prefix),
                    path: object.key,
                    is_dir: false,
                    size: object.size,
                    modified: object.modified,
                });
                Ok(folders.chain(files).collect())
            })
        }

        fn download(&self, path: String, dest: PathBuf) -> BoxFuture<'_, Result<(), PortError>> {
            let config = self.config.clone();
            let secrets = Arc::clone(&self.secrets);
            Box::pin(async move { download(&config, secrets.as_ref(), &path, &dest).await.map_err(to_port) })
        }

        fn upload(&self, src: PathBuf, dir: String) -> BoxFuture<'_, Result<String, PortError>> {
            let config = self.config.clone();
            let secrets = Arc::clone(&self.secrets);
            let prefix = with_slash(&start_or(dir, &config.path_prefix));
            Box::pin(async move {
                let name = leaf(&src).map_err(to_port)?;
                let destination =
                    S3::new(S3Config { path_prefix: prefix.clone(), acl_public_read: false, public_url_base: None, ..config }, secrets);
                let request = UploadRequest {
                    file_name: name.clone(),
                    kind: UploadKind::File,
                    mime: mime_for(&name).to_owned(),
                    data: UploadData::Path(src),
                };
                let progress: ProgressCallback = Arc::new(|_, _| {});
                destination.upload(request, progress).await.map_err(to_port)?;
                Ok(format!("{prefix}{name}"))
            })
        }

        fn rename(&self, from: String, to: String) -> BoxFuture<'_, Result<(), PortError>> {
            let config = self.config.clone();
            let secrets = Arc::clone(&self.secrets);
            Box::pin(async move { rename(&config, secrets.as_ref(), &from, &to).await.map_err(to_port) })
        }

        fn delete(&self, path: String) -> BoxFuture<'_, Result<(), PortError>> {
            let config = self.config.clone();
            let secrets = Arc::clone(&self.secrets);
            Box::pin(async move { delete(&config, secrets.as_ref(), &path).await.map_err(to_port) })
        }

        fn mkdir(&self, path: String) -> BoxFuture<'_, Result<(), PortError>> {
            let config = self.config.clone();
            let secrets = Arc::clone(&self.secrets);
            Box::pin(async move { mkdir(&config, secrets.as_ref(), &path).await.map_err(to_port) })
        }
    }
}

#[cfg(feature = "host-ftp")]
mod ftp_remote {
    use std::fs::File;
    use std::io;
    use std::path::PathBuf;
    use std::sync::Arc;

    use laqta_ports::tools::{RemoteEntry, RemoteStorage};
    use laqta_ports::upload::{SecretStore, UploadError};
    use laqta_ports::{BoxFuture, PortError};
    use suppaftp::types::FileType;
    use suppaftp::{FtpError, FtpStream, ImplFtpStream, Mode, TlsStream};

    use super::{ListedEntry, failed, join, leaf, parse_list_line, parse_mlsd_line, secret, start_or, to_port};
    use crate::hosts::ftp::{self, FtpConfig, FtpTls};

    pub(super) struct FtpRemote {
        pub(super) config: FtpConfig,
        pub(super) secrets: Arc<dyn SecretStore>,
    }

    /// One remote operation, run on the FTP worker thread.
    pub(super) enum FtpOp {
        List(String),
        Get(String, PathBuf),
        Put(PathBuf, String),
        Rename(String, String),
        Delete(String),
        Mkdir(String),
    }

    impl FtpOp {
        fn run<T: TlsStream + Send + 'static>(&self, ftp: &mut ImplFtpStream<T>) -> Result<Vec<RemoteEntry>, UploadError> {
            match self {
                FtpOp::List(dir) => list(ftp, dir),
                FtpOp::Get(path, dest) => {
                    ftp.retr(path.as_str(), |reader| {
                        let mut file = File::create(dest).map_err(FtpError::ConnectionError)?;
                        io::copy(reader, &mut file).map_err(FtpError::ConnectionError)?;
                        Ok(())
                    })
                    .map_err(ftp::ftp_error)?;
                    Ok(Vec::new())
                }
                FtpOp::Put(src, remote) => {
                    let mut file =
                        File::open(src).map_err(|error| UploadError::Unsupported(format!("cannot read {}: {error}", src.display())))?;
                    ftp.put_file(remote.as_str(), &mut file).map_err(ftp::ftp_error)?;
                    Ok(Vec::new())
                }
                FtpOp::Rename(from, to) => {
                    ftp.rename(from.as_str(), to.as_str()).map_err(ftp::ftp_error)?;
                    Ok(Vec::new())
                }
                FtpOp::Delete(path) => {
                    // DELE removes a file; a folder needs RMD.
                    if ftp.rm(path.as_str()).is_err() {
                        ftp.rmdir(path.as_str()).map_err(ftp::ftp_error)?;
                    }
                    Ok(Vec::new())
                }
                FtpOp::Mkdir(path) => {
                    ftp.mkdir(path.as_str()).map_err(ftp::ftp_error)?;
                    Ok(Vec::new())
                }
            }
        }
    }

    /// MLSD, which states each entry's type. A server that refuses it gets LIST, which is parsed in
    /// its Unix or DOS layout.
    fn list<T: TlsStream + Send + 'static>(ftp: &mut ImplFtpStream<T>, dir: &str) -> Result<Vec<RemoteEntry>, UploadError> {
        let target = (!dir.is_empty()).then_some(dir);
        let entries: Vec<ListedEntry> = match ftp.mlsd(target) {
            Ok(lines) => lines.iter().filter_map(|line| parse_mlsd_line(line)).collect(),
            Err(_) => ftp.list(target).map_err(ftp::ftp_error)?.iter().filter_map(|line| parse_list_line(line)).collect(),
        };
        Ok(entries
            .into_iter()
            .filter(|entry| entry.name != "." && entry.name != "..")
            .map(|entry| RemoteEntry {
                path: join(dir, &entry.name),
                name: entry.name,
                is_dir: entry.is_dir,
                size: entry.size,
                modified: None,
            })
            .collect())
    }

    /// Logs in and runs one operation over an open control connection, then quits.
    fn session<T: TlsStream + Send + 'static>(
        mut ftp: ImplFtpStream<T>,
        config: &FtpConfig,
        password: &str,
        op: &FtpOp,
    ) -> Result<Vec<RemoteEntry>, UploadError> {
        ftp.login(config.username.as_str(), password).map_err(ftp::ftp_error)?;
        ftp.set_mode(if config.passive { Mode::Passive } else { Mode::Active });
        // ASCII mode rewrites LF to CRLF, which corrupts binary files.
        ftp.transfer_type(FileType::Binary).map_err(ftp::ftp_error)?;
        let result = op.run(&mut ftp);
        let _ = ftp.quit();
        result
    }

    /// The connection setup mirrors `hosts::ftp`'s upload: plain, explicit FTPS, or implicit FTPS.
    fn run_ftp(config: FtpConfig, password: String, op: FtpOp) -> Result<Vec<RemoteEntry>, UploadError> {
        match config.tls {
            FtpTls::None => {
                let ftp: FtpStream = ftp::open_control(ftp::tcp_connect(&config)?)?;
                session(ftp, &config, &password, &op)
            }
            FtpTls::Explicit => {
                let connector = ftp::connector()?;
                let ftp: ftp::TlsFtp = ftp::open_control(ftp::tcp_connect(&config)?)?;
                let ftp = ftp.into_secure(connector, &config.host).map_err(ftp::ftp_error)?;
                session(ftp, &config, &password, &op)
            }
            FtpTls::Implicit => {
                let connector = ftp::TimedTls(ftp::connector()?);
                let port = if config.port == ftp::FTP_PORT { ftp::FTPS_IMPLICIT_PORT } else { config.port };
                let ftp =
                    ftp::TlsFtp::connect_secure_implicit((config.host.as_str(), port), connector, &config.host).map_err(ftp::ftp_error)?;
                session(ftp, &config, &password, &op)
            }
        }
    }

    impl FtpRemote {
        /// Runs `op` on the worker thread with the account's password.
        fn exec(&self, op: FtpOp) -> BoxFuture<'static, Result<Vec<RemoteEntry>, PortError>> {
            let config = self.config.clone();
            let secrets = Arc::clone(&self.secrets);
            Box::pin(async move {
                let password = secret(secrets.as_ref(), ftp::PASSWORD_KEY).map_err(to_port)?.unwrap_or_default();
                let outcome = ftp::on_worker(move || run_ftp(config, password, op)).await.map_err(to_port)?;
                outcome.map_err(to_port)
            })
        }
    }

    impl RemoteStorage for FtpRemote {
        fn list(&self, dir: String) -> BoxFuture<'_, Result<Vec<RemoteEntry>, PortError>> {
            self.exec(FtpOp::List(start_or(dir, &self.config.directory)))
        }

        fn download(&self, path: String, dest: PathBuf) -> BoxFuture<'_, Result<(), PortError>> {
            let fut = self.exec(FtpOp::Get(path, dest));
            Box::pin(async move { fut.await.map(|_| ()) })
        }

        fn upload(&self, src: PathBuf, dir: String) -> BoxFuture<'_, Result<String, PortError>> {
            let name = match leaf(&src) {
                Ok(name) => name,
                Err(error) => return failed(to_port(error)),
            };
            let remote = join(&start_or(dir, &self.config.directory), &name);
            let fut = self.exec(FtpOp::Put(src, remote.clone()));
            Box::pin(async move {
                fut.await?;
                Ok(remote)
            })
        }

        fn rename(&self, from: String, to: String) -> BoxFuture<'_, Result<(), PortError>> {
            let fut = self.exec(FtpOp::Rename(from, to));
            Box::pin(async move { fut.await.map(|_| ()) })
        }

        fn delete(&self, path: String) -> BoxFuture<'_, Result<(), PortError>> {
            let fut = self.exec(FtpOp::Delete(path));
            Box::pin(async move { fut.await.map(|_| ()) })
        }

        fn mkdir(&self, path: String) -> BoxFuture<'_, Result<(), PortError>> {
            let fut = self.exec(FtpOp::Mkdir(path));
            Box::pin(async move { fut.await.map(|_| ()) })
        }
    }
}

#[cfg(feature = "host-sftp")]
mod sftp_remote {
    use std::path::PathBuf;
    use std::sync::Arc;

    use laqta_ports::tools::{RemoteEntry, RemoteStorage};
    use laqta_ports::upload::SecretStore;
    use laqta_ports::{BoxFuture, PortError};

    #[cfg(not(windows))]
    use super::{failed, join, leaf, start_or, to_port};
    use crate::hosts::sftp::SftpConfig;

    /// The SFTP account as a browser. On Windows, the browser has no SFTP client: each call answers
    /// `Unsupported`, and the upload host still uses `sftp.exe`.
    #[cfg_attr(windows, allow(dead_code))]
    pub(super) struct SftpRemote {
        pub(super) config: SftpConfig,
        pub(super) secrets: Arc<dyn SecretStore>,
    }

    #[cfg(windows)]
    const WINDOWS_MESSAGE: &str = "browsing SFTP folders is not available on Windows yet; use the SFTP upload host";

    #[cfg(not(windows))]
    mod russh_side {
        use std::path::PathBuf;
        use std::sync::Arc;
        use std::time::Duration;

        use laqta_ports::tools::RemoteEntry;
        use laqta_ports::upload::{SecretStore, UploadError};
        use laqta_ports::{BoxFuture, PortError};
        use russh_sftp::client::SftpSession;

        use super::super::{join, secret, to_port};
        use crate::hosts::sftp::{self, Login, SftpConfig};

        /// Idle time after which a connection is dropped, as the upload's.
        const IO_TIMEOUT: Duration = Duration::from_secs(60);

        /// One operation, run over an authenticated SFTP session.
        pub(super) enum SftpOp {
            List(String),
            Get(String, PathBuf),
            Put(PathBuf, String),
            Rename(String, String),
            Delete(String),
            Mkdir(String),
        }

        struct Connected {
            handle: russh::client::Handle<sftp::HostKeys>,
            sftp: SftpSession,
        }

        impl Connected {
            async fn close(self) {
                let _ = self.sftp.close().await;
                let _ = self.handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
            }
        }

        /// Connects, checks the host key, authenticates and opens the SFTP subsystem. The setup is
        /// the upload's (`hosts::sftp`), so a host key that is not in `known_hosts` is refused here too.
        async fn connect(config: &SftpConfig, secrets: &dyn SecretStore) -> Result<Connected, UploadError> {
            let login = match &config.key_path {
                Some(path) => {
                    let passphrase = secret(secrets, sftp::PASSPHRASE_KEY)?;
                    Login::Key(Arc::new(sftp::load_key(path, passphrase.as_deref())?))
                }
                None => match secret(secrets, sftp::PASSWORD_KEY)? {
                    Some(password) => Login::Password(password),
                    None => return Err(UploadError::Auth(format!("no key file or password is set for {}", config.host))),
                },
            };
            let ssh = Arc::new(russh::client::Config { inactivity_timeout: Some(IO_TIMEOUT), ..russh::client::Config::default() });
            let handler = sftp::HostKeys { host: config.host.clone(), port: config.port };
            let mut handle = russh::client::connect(ssh, (config.host.clone(), config.port), handler).await.map_err(sftp::client_error)?;
            let accepted = match login {
                Login::Key(key) => {
                    handle.authenticate_publickey(config.username.clone(), russh::keys::PrivateKeyWithHashAlg::new(key, None)).await
                }
                Login::Password(password) => handle.authenticate_password(config.username.clone(), password).await,
            };
            match accepted.map_err(sftp::ssh_error)? {
                russh::client::AuthResult::Success => {}
                russh::client::AuthResult::Failure { .. } => {
                    return Err(UploadError::Auth(format!("{} refused the login for {}", config.host, config.username)));
                }
            }
            let channel = handle.channel_open_session().await.map_err(sftp::ssh_error)?;
            channel.request_subsystem(true, "sftp").await.map_err(sftp::ssh_error)?;
            let sftp = SftpSession::new(channel.into_stream()).await.map_err(sftp::sftp_error)?;
            Ok(Connected { handle, sftp })
        }

        async fn run_op(sftp: &SftpSession, op: SftpOp) -> Result<Vec<RemoteEntry>, UploadError> {
            match op {
                SftpOp::List(dir) => {
                    let mut entries = Vec::new();
                    for entry in sftp.read_dir(dir.clone()).await.map_err(sftp::sftp_error)? {
                        let name = entry.file_name();
                        if name == "." || name == ".." {
                            continue;
                        }
                        let is_dir = entry.file_type().is_dir();
                        let metadata = entry.metadata();
                        entries.push(RemoteEntry {
                            path: join(&dir, &name),
                            name,
                            is_dir,
                            size: (!is_dir).then(|| metadata.len()),
                            modified: None,
                        });
                    }
                    Ok(entries)
                }
                SftpOp::Get(path, dest) => {
                    let bytes = sftp.read(path).await.map_err(sftp::sftp_error)?;
                    std::fs::write(&dest, bytes)
                        .map_err(|error| UploadError::Unsupported(format!("cannot write {}: {error}", dest.display())))?;
                    Ok(Vec::new())
                }
                SftpOp::Put(src, remote) => {
                    let bytes =
                        std::fs::read(&src).map_err(|error| UploadError::Unsupported(format!("cannot read {}: {error}", src.display())))?;
                    // `write` opens without CREATE, so the file is created (and truncated) first.
                    sftp.create(remote.clone())
                        .await
                        .map_err(sftp::sftp_error)?
                        .close()
                        .await
                        .map_err(|error| UploadError::Network(error.to_string()))?;
                    sftp.write(remote, &bytes).await.map_err(sftp::sftp_error)?;
                    Ok(Vec::new())
                }
                SftpOp::Rename(from, to) => {
                    sftp.rename(from, to).await.map_err(sftp::sftp_error)?;
                    Ok(Vec::new())
                }
                SftpOp::Delete(path) => {
                    if sftp.remove_file(path.clone()).await.is_err() {
                        sftp.remove_dir(path).await.map_err(sftp::sftp_error)?;
                    }
                    Ok(Vec::new())
                }
                SftpOp::Mkdir(path) => {
                    sftp.create_dir(path).await.map_err(sftp::sftp_error)?;
                    Ok(Vec::new())
                }
            }
        }

        /// The whole operation: connect, run, disconnect. It runs on the HTTP runtime.
        async fn operate(config: SftpConfig, secrets: Arc<dyn SecretStore>, op: SftpOp) -> Result<Vec<RemoteEntry>, UploadError> {
            let connected = connect(&config, secrets.as_ref()).await?;
            let result = run_op(&connected.sftp, op).await;
            connected.close().await;
            result
        }

        pub(super) fn exec(
            config: &SftpConfig,
            secrets: &Arc<dyn SecretStore>,
            op: SftpOp,
        ) -> BoxFuture<'static, Result<Vec<RemoteEntry>, PortError>> {
            let config = config.clone();
            let secrets = Arc::clone(secrets);
            Box::pin(async move { laqta_http::run(operate(config, secrets, op)).await.map_err(to_port) })
        }
    }

    #[cfg(not(windows))]
    impl RemoteStorage for SftpRemote {
        fn list(&self, dir: String) -> BoxFuture<'_, Result<Vec<RemoteEntry>, PortError>> {
            russh_side::exec(&self.config, &self.secrets, russh_side::SftpOp::List(start_or(dir, &self.config.directory)))
        }

        fn download(&self, path: String, dest: PathBuf) -> BoxFuture<'_, Result<(), PortError>> {
            let fut = russh_side::exec(&self.config, &self.secrets, russh_side::SftpOp::Get(path, dest));
            Box::pin(async move { fut.await.map(|_| ()) })
        }

        fn upload(&self, src: PathBuf, dir: String) -> BoxFuture<'_, Result<String, PortError>> {
            let name = match leaf(&src) {
                Ok(name) => name,
                Err(error) => return failed(to_port(error)),
            };
            let remote = join(&start_or(dir, &self.config.directory), &name);
            let fut = russh_side::exec(&self.config, &self.secrets, russh_side::SftpOp::Put(src, remote.clone()));
            Box::pin(async move {
                fut.await?;
                Ok(remote)
            })
        }

        fn rename(&self, from: String, to: String) -> BoxFuture<'_, Result<(), PortError>> {
            let fut = russh_side::exec(&self.config, &self.secrets, russh_side::SftpOp::Rename(from, to));
            Box::pin(async move { fut.await.map(|_| ()) })
        }

        fn delete(&self, path: String) -> BoxFuture<'_, Result<(), PortError>> {
            let fut = russh_side::exec(&self.config, &self.secrets, russh_side::SftpOp::Delete(path));
            Box::pin(async move { fut.await.map(|_| ()) })
        }

        fn mkdir(&self, path: String) -> BoxFuture<'_, Result<(), PortError>> {
            let fut = russh_side::exec(&self.config, &self.secrets, russh_side::SftpOp::Mkdir(path));
            Box::pin(async move { fut.await.map(|_| ()) })
        }
    }

    #[cfg(windows)]
    fn unavailable<T: Send + 'static>() -> BoxFuture<'static, Result<T, PortError>> {
        Box::pin(async { Err(PortError::Unsupported(WINDOWS_MESSAGE.to_owned())) })
    }

    #[cfg(windows)]
    impl RemoteStorage for SftpRemote {
        fn list(&self, _dir: String) -> BoxFuture<'_, Result<Vec<RemoteEntry>, PortError>> {
            unavailable()
        }

        fn download(&self, _path: String, _dest: PathBuf) -> BoxFuture<'_, Result<(), PortError>> {
            unavailable()
        }

        fn upload(&self, _src: PathBuf, _dir: String) -> BoxFuture<'_, Result<String, PortError>> {
            unavailable()
        }

        fn rename(&self, _from: String, _to: String) -> BoxFuture<'_, Result<(), PortError>> {
            unavailable()
        }

        fn delete(&self, _path: String) -> BoxFuture<'_, Result<(), PortError>> {
            unavailable()
        }

        fn mkdir(&self, _path: String) -> BoxFuture<'_, Result<(), PortError>> {
            unavailable()
        }
    }
}
