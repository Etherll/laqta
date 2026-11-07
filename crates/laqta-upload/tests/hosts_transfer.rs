//! The native transfer hosts: S3 presigning against a local server, FTP and SFTP argument shapes,
//! and the shared folder. No network: FTP and SFTP run against a closed local port.
#![allow(clippy::unwrap_used)]

// The HTTP test server of laqta-http, reused as is.
#[path = "../../laqta-http/tests/support/mod.rs"]
mod http_support;

use std::collections::HashMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures_lite::future::block_on;
use http_support::{Canned, Server};
use laqta_ports::fakes::FakeSecretStore;
use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadData, UploadError, UploadKind, UploadRequest};

use laqta_upload::hosts::{ftp, s3, sftp, shared_folder};

fn store(entries: &[(&str, &str)]) -> Arc<dyn SecretStore> {
    let secrets: HashMap<String, String> = entries.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect();
    Arc::new(FakeSecretStore { secrets: Mutex::new(secrets) })
}

fn progress() -> ProgressCallback {
    Arc::new(|_sent, _total| {})
}

fn png(name: &str, bytes: &[u8]) -> UploadRequest {
    UploadRequest {
        file_name: name.to_owned(),
        kind: UploadKind::Image,
        data: UploadData::Bytes(bytes.to_vec()),
        mime: "image/png".to_owned(),
    }
}

fn s3_config(endpoint: String) -> s3::S3Config {
    s3::S3Config {
        endpoint,
        region: "us-east-1".to_owned(),
        bucket: "bucket".to_owned(),
        path_prefix: "shots/".to_owned(),
        access_key_id: "AKIDEXAMPLE".to_owned(),
        use_path_style: true,
        public_url_base: None,
        acl_public_read: false,
    }
}

/// A local port with nothing listening on it: connecting to it is refused.
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[test]
fn s3_put_is_presigned_and_returns_the_object_url() {
    let server = Server::start(vec![Canned::ok(Vec::new())]);
    let endpoint = server.url("");
    let destination = s3::S3::new(s3_config(endpoint.clone()), store(&[(s3::SECRET_KEY, "secret")]));
    let bytes = b"\x89PNG test bytes".to_vec();

    let result = block_on(destination.upload(png("Laqta 1.png", &bytes), progress())).unwrap();

    let recorded = server.finish();
    assert_eq!(recorded.len(), 1);
    let request = &recorded[0];
    assert_eq!(request.method, "PUT");
    assert!(request.target.starts_with("/bucket/shots/Laqta%201.png?"), "{}", request.target);
    assert!(request.target.contains("X-Amz-Signature="), "{}", request.target);
    assert_eq!(request.body, bytes);
    assert_eq!(request.header("content-type"), Some("image/png"));
    assert_eq!(result.url.as_deref(), Some(format!("{endpoint}/bucket/shots/Laqta%201.png").as_str()));
}

#[test]
fn s3_public_read_acl_is_signed_and_sent_and_the_public_url_is_returned() {
    let server = Server::start(vec![Canned::ok(Vec::new())]);
    let mut config = s3_config(server.url(""));
    config.acl_public_read = true;
    config.public_url_base = Some("https://cdn.example.com/media/".to_owned());
    let destination = s3::S3::new(config, store(&[(s3::SECRET_KEY, "secret")]));

    let result = block_on(destination.upload(png("Laqta_1.png", b"x"), progress())).unwrap();

    let recorded = server.finish();
    let request = &recorded[0];
    assert_eq!(request.header("x-amz-acl"), Some("public-read"));
    assert!(request.target.contains("X-Amz-SignedHeaders=host%3Bx-amz-acl"), "{}", request.target);
    assert_eq!(result.url.as_deref(), Some("https://cdn.example.com/media/shots/Laqta_1.png"));
}

#[test]
fn s3_forbidden_is_an_auth_error() {
    let server = Server::start(vec![Canned::new(403, "<Error>SignatureDoesNotMatch</Error>")]);
    let destination = s3::S3::new(s3_config(server.url("")), store(&[(s3::SECRET_KEY, "secret")]));

    let error = block_on(destination.upload(png("a.png", b"x"), progress())).unwrap_err();

    server.finish();
    assert!(matches!(error, UploadError::Auth(_)), "{error:?}");
    assert!(!error.is_retryable());
}

#[test]
fn s3_server_error_is_retryable() {
    let server = Server::start(vec![Canned::new(503, "busy")]);
    let destination = s3::S3::new(s3_config(server.url("")), store(&[(s3::SECRET_KEY, "secret")]));

    let error = block_on(destination.upload(png("a.png", b"x"), progress())).unwrap_err();

    server.finish();
    assert!(matches!(error, UploadError::Server { status: 503, .. }), "{error:?}");
    assert!(error.is_retryable());
}

#[test]
fn s3_without_a_stored_secret_fails_before_sending() {
    let destination = s3::S3::new(s3_config("http://127.0.0.1:9".to_owned()), store(&[]));

    let error = block_on(destination.upload(png("a.png", b"x"), progress())).unwrap_err();

    assert!(matches!(error, UploadError::Auth(_)), "{error:?}");
}

#[test]
fn s3_object_urls_for_path_style_virtual_host_and_public_base() {
    let mut config = s3_config("https://s3.eu-west-1.amazonaws.com".to_owned());
    config.region = "eu-west-1".to_owned();
    config.bucket = "my-bucket".to_owned();
    assert_eq!(s3::object_url(&config, "a b.png").unwrap(), "https://s3.eu-west-1.amazonaws.com/my-bucket/a%20b.png");

    config.use_path_style = false;
    assert_eq!(s3::object_url(&config, "a.png").unwrap(), "https://my-bucket.s3.eu-west-1.amazonaws.com/a.png");

    config.public_url_base = Some("https://cdn.example.com/media".to_owned());
    assert_eq!(s3::object_url(&config, "shots/a b.png").unwrap(), "https://cdn.example.com/media/shots/a%20b.png");
}

#[test]
fn ftp_file_urls_join_the_base_and_encode_the_name() {
    assert_eq!(ftp::file_url("https://files.example.com/pub/", "Laqta 1#2.png"), "https://files.example.com/pub/Laqta%201%232.png");
    assert_eq!(ftp::file_url("https://files.example.com", "a.png"), "https://files.example.com/a.png");
}

#[test]
fn ftps_to_a_closed_port_is_a_network_error_in_both_modes() {
    for tls in [ftp::FtpTls::Explicit, ftp::FtpTls::Implicit] {
        let config = ftp::FtpConfig {
            host: "127.0.0.1".to_owned(),
            port: closed_port(),
            username: "laqta".to_owned(),
            directory: String::new(),
            url_base: "https://files.example.com".to_owned(),
            passive: true,
            tls,
        };
        let destination = ftp::Ftp::new(config, store(&[]));

        let error = block_on(destination.upload(png("a.png", b"x"), progress())).unwrap_err();

        assert!(matches!(error, UploadError::Network(_)), "{tls:?}: {error:?}");
        assert!(error.is_retryable(), "{tls:?}: {error:?}");
    }
}

#[test]
fn ftp_to_a_closed_port_is_a_network_error() {
    let config = ftp::FtpConfig {
        host: "127.0.0.1".to_owned(),
        port: closed_port(),
        username: "laqta".to_owned(),
        directory: "uploads".to_owned(),
        url_base: "https://files.example.com".to_owned(),
        passive: true,
        tls: ftp::FtpTls::None,
    };
    let destination = ftp::Ftp::new(config, store(&[]));

    let error = block_on(destination.upload(png("a.png", b"x"), progress())).unwrap_err();

    assert!(matches!(error, UploadError::Network(_)), "{error:?}");
    assert!(error.is_retryable());
}

#[test]
fn sftp_layout_splits_the_directory_into_the_prefixes_to_create() {
    assert_eq!(sftp::layout("/uploads/laqta/"), ("/uploads/laqta".to_owned(), vec!["/uploads".to_owned(), "/uploads/laqta".to_owned()]));
    assert_eq!(sftp::layout("a/b"), ("a/b".to_owned(), vec!["a".to_owned(), "a/b".to_owned()]));
    assert_eq!(sftp::layout(""), (String::new(), Vec::new()));
    assert_eq!(sftp::layout("/"), ("/".to_owned(), Vec::new()));
}

#[test]
fn sftp_batch_script_quotes_every_argument_and_converts_the_local_path() {
    let script = sftp::batch_script("/uploads/laqta", Path::new(r"C:\Users\me\Temp\Laqta 1.png"), "Laqta 1.png").unwrap();
    assert_eq!(
        script,
        "-mkdir \"/uploads\"\n-mkdir \"/uploads/laqta\"\ncd \"/uploads/laqta\"\n\
         put \"C:/Users/me/Temp/Laqta 1.png\" \"Laqta 1.png\"\n"
    );
}

#[test]
fn sftp_batch_script_escapes_quotes_and_refuses_control_characters() {
    let script = sftp::batch_script("", Path::new("/tmp/a.png"), r#"say "hi".png"#).unwrap();
    assert_eq!(script, "put \"/tmp/a.png\" \"say \\\"hi\\\".png\"\n");

    // A newline in the name would start a second batch command.
    assert!(sftp::batch_script("", Path::new("/tmp/a.png"), "a\nrm -rf ~").is_err());
    assert!(sftp::batch_script("dir\n-rm", Path::new("/tmp/a.png"), "a.png").is_err());
}

#[test]
fn sftp_command_args_use_batch_mode_the_port_the_key_and_user_at_host() {
    let config = sftp::SftpConfig {
        host: "example.com".to_owned(),
        port: 2222,
        username: "laqta".to_owned(),
        key_path: None,
        directory: String::new(),
        url_base: "https://files.example.com".to_owned(),
    };
    assert_eq!(sftp::command_args(&config, None).unwrap(), vec!["-b", "-", "-o", "BatchMode=yes", "-P", "2222", "laqta@example.com"]);

    let key = PathBuf::from(r"C:\keys\id_ed25519");
    assert_eq!(
        sftp::command_args(&config, Some(&key)).unwrap(),
        vec!["-b", "-", "-o", "BatchMode=yes", "-P", "2222", "-i", r"C:\keys\id_ed25519", "laqta@example.com"]
    );
}

#[test]
fn sftp_command_args_refuse_a_host_or_user_that_reads_as_an_option() {
    let mut config = sftp::SftpConfig {
        host: "-oProxyCommand=evil".to_owned(),
        port: 22,
        username: "laqta".to_owned(),
        key_path: None,
        directory: String::new(),
        url_base: "https://files.example.com".to_owned(),
    };
    assert!(sftp::command_args(&config, None).is_err());

    config.host = "example.com".to_owned();
    config.username = "me and you".to_owned();
    assert!(sftp::command_args(&config, None).is_err());
}

#[cfg(not(windows))]
#[test]
fn sftp_to_a_closed_port_is_a_network_error() {
    let config = sftp::SftpConfig {
        host: "127.0.0.1".to_owned(),
        port: closed_port(),
        username: "laqta".to_owned(),
        key_path: None,
        directory: "uploads".to_owned(),
        url_base: "https://files.example.com".to_owned(),
    };
    let destination = sftp::Sftp::new(config, store(&[(sftp::PASSWORD_KEY, "secret")]));

    let error = block_on(destination.upload(png("a.png", b"x"), progress())).unwrap_err();

    assert!(matches!(error, UploadError::Network(_)), "{error:?}");
}

#[cfg(not(windows))]
#[test]
fn sftp_without_a_key_or_a_password_is_an_auth_error_before_connecting() {
    let config = sftp::SftpConfig {
        host: "127.0.0.1".to_owned(),
        port: 22,
        username: "laqta".to_owned(),
        key_path: None,
        directory: String::new(),
        url_base: "https://files.example.com".to_owned(),
    };
    let destination = sftp::Sftp::new(config, store(&[]));

    let error = block_on(destination.upload(png("a.png", b"x"), progress())).unwrap_err();

    assert!(matches!(error, UploadError::Auth(_)), "{error:?}");
}

#[test]
fn shared_folder_copies_the_bytes_and_returns_a_file_url() {
    let dir = tempfile::tempdir().unwrap();
    let share = dir.path().join("share");
    let destination = shared_folder::SharedFolder::new(shared_folder::SharedFolderConfig { directory: share.clone(), url_base: None });

    let result = block_on(destination.upload(png("Laqta 1.png", b"bytes"), progress())).unwrap();

    let target = share.join("Laqta 1.png");
    assert_eq!(std::fs::read(&target).unwrap(), b"bytes");
    let expected = url::Url::from_file_path(std::path::absolute(&target).unwrap()).unwrap().to_string();
    assert_eq!(result.url.as_deref(), Some(expected.as_str()));
}

#[test]
fn shared_folder_with_a_url_base_returns_the_base_and_the_encoded_name() {
    let dir = tempfile::tempdir().unwrap();
    let destination = shared_folder::SharedFolder::new(shared_folder::SharedFolderConfig {
        directory: dir.path().to_path_buf(),
        url_base: Some("https://files.example.com/shared/".to_owned()),
    });

    let result = block_on(destination.upload(png("Laqta 1.png", b"bytes"), progress())).unwrap();

    assert_eq!(result.url.as_deref(), Some("https://files.example.com/shared/Laqta%201.png"));
}

#[test]
fn shared_folder_copies_a_file_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.bin");
    std::fs::write(&source, b"copied").unwrap();
    let share = dir.path().join("share");
    let destination = shared_folder::SharedFolder::new(shared_folder::SharedFolderConfig { directory: share.clone(), url_base: None });
    let request = UploadRequest {
        file_name: "copy.bin".to_owned(),
        kind: UploadKind::File,
        data: UploadData::Path(source),
        mime: "application/octet-stream".to_owned(),
    };

    block_on(destination.upload(request, progress())).unwrap();

    assert_eq!(std::fs::read(share.join("copy.bin")).unwrap(), b"copied");
}

#[test]
fn shared_folder_refuses_a_name_that_leaves_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let share = dir.path().join("share");
    let destination = shared_folder::SharedFolder::new(shared_folder::SharedFolderConfig { directory: share.clone(), url_base: None });

    let error = block_on(destination.upload(png("../escape.png", b"x"), progress())).unwrap_err();

    assert!(matches!(error, UploadError::Unsupported(_)), "{error:?}");
    assert!(!dir.path().join("escape.png").exists());
}
