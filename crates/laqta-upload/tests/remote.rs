//! The remote storage browser's back ends: S3 listings from canned ListObjectsV2 pages, the
//! FTP listing parsers (Unix, DOS and MLSD), and the wiring from accounts. No network.
#![allow(clippy::unwrap_used)]

mod support_oauth;

use std::sync::Arc;

use futures_lite::future::block_on;
use laqta_domain::hosts::{HostAccount, HostKind};
use laqta_ports::SecretStore;
use laqta_ports::fakes::{CountingRng, FakeSecretStore, FakeShell};
use laqta_upload::hosts::s3::SECRET_KEY;
use laqta_upload::native::HostDeps;
use laqta_upload::remote::{parse_list_line, parse_mlsd_line, remote_storages};
use support_oauth::sign_in::NOW;
use support_oauth::{Canned, Server};

fn deps(secrets: Arc<dyn SecretStore>) -> HostDeps {
    HostDeps { secrets, shell: Arc::new(FakeShell::default()), csprng: Arc::new(CountingRng::default()), now_unix: Arc::new(|| NOW) }
}

fn s3_account(endpoint: String) -> HostAccount {
    let mut account = HostAccount::new(HostKind::S3);
    account.values.insert("endpoint".to_owned(), endpoint);
    account.values.insert("bucket".to_owned(), "bucket".to_owned());
    account.values.insert("path_prefix".to_owned(), "shots/".to_owned());
    account.values.insert("access_key_id".to_owned(), "AKIDEXAMPLE".to_owned());
    account.values.insert("use_path_style".to_owned(), "true".to_owned());
    account
}

fn s3_secrets() -> Arc<dyn SecretStore> {
    let secrets = FakeSecretStore::default();
    secrets.set(SECRET_KEY, "secret").unwrap();
    Arc::new(secrets)
}

/// The first page of a listing: a truncated page that names its continuation token.
const FIRST_PAGE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Name>bucket</Name>
  <Prefix>shots/</Prefix>
  <Delimiter>/</Delimiter>
  <EncodingType>url</EncodingType>
  <MaxKeys>1000</MaxKeys>
  <IsTruncated>true</IsTruncated>
  <NextContinuationToken>page-2</NextContinuationToken>
  <Contents>
    <Key>shots/</Key>
    <LastModified>2026-11-07T10:00:00.000Z</LastModified>
    <Size>0</Size>
    <StorageClass>STANDARD</StorageClass>
  </Contents>
  <Contents>
    <Key>shots/Laqta%201.png</Key>
    <LastModified>2026-11-07T10:01:00.000Z</LastModified>
    <Size>1234</Size>
    <StorageClass>STANDARD</StorageClass>
  </Contents>
  <CommonPrefixes>
    <Prefix>shots/sub/</Prefix>
  </CommonPrefixes>
</ListBucketResult>"#;

/// The last page: no continuation token.
const LAST_PAGE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Name>bucket</Name>
  <Prefix>shots/</Prefix>
  <Delimiter>/</Delimiter>
  <EncodingType>url</EncodingType>
  <MaxKeys>1000</MaxKeys>
  <IsTruncated>false</IsTruncated>
  <Contents>
    <Key>shots/b.png</Key>
    <LastModified>2026-11-07T10:02:00.000Z</LastModified>
    <Size>5</Size>
    <StorageClass>STANDARD</StorageClass>
  </Contents>
</ListBucketResult>"#;

/// A listing of the folder `shots/sub/`.
const SUB_PAGE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Name>bucket</Name>
  <Prefix>shots/sub/</Prefix>
  <Delimiter>/</Delimiter>
  <EncodingType>url</EncodingType>
  <MaxKeys>1000</MaxKeys>
  <IsTruncated>false</IsTruncated>
  <Contents>
    <Key>shots/sub/b.png</Key>
    <Size>5</Size>
  </Contents>
</ListBucketResult>"#;

fn query_of(target: &str) -> Vec<(String, String)> {
    url::Url::parse(&format!("http://localhost{target}"))
        .unwrap()
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

#[test]
fn s3_listing_gives_folders_and_objects_under_the_prefix_across_pages() {
    let server = Server::start(vec![Canned::ok(FIRST_PAGE), Canned::ok(LAST_PAGE)]);
    let storages = remote_storages(&[s3_account(server.url(""))], &deps(s3_secrets()));
    assert_eq!(storages.len(), 1);
    assert_eq!(storages[0].0, "Amazon S3 / S3-compatible: bucket");

    let entries = block_on(storages[0].1.list(String::new())).unwrap();

    let shape: Vec<(&str, &str, bool)> = entries.iter().map(|entry| (entry.name.as_str(), entry.path.as_str(), entry.is_dir)).collect();
    assert_eq!(shape, vec![("sub", "shots/sub/", true), ("Laqta 1.png", "shots/Laqta 1.png", false), ("b.png", "shots/b.png", false),]);
    assert_eq!(entries[1].size, Some(1234));
    assert_eq!(entries[1].modified.as_deref(), Some("2026-11-07T10:01:00.000Z"));

    let recorded = server.finish();
    assert_eq!(recorded.len(), 2);
    let first = &recorded[0];
    assert_eq!(first.method, "GET");
    assert!(first.target.starts_with("/bucket/?"), "{}", first.target);
    let query = query_of(&first.target);
    let value = |name: &str| query.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str());
    assert_eq!(value("list-type"), Some("2"));
    assert_eq!(value("prefix"), Some("shots/"));
    assert_eq!(value("delimiter"), Some("/"));
    assert!(first.target.contains("X-Amz-Signature="), "{}", first.target);
    let second_query = query_of(&recorded[1].target);
    assert!(second_query.iter().any(|(key, value)| key == "continuation-token" && value == "page-2"), "{}", recorded[1].target);
}

#[test]
fn s3_listing_of_a_folder_names_its_entries_relative_to_it() {
    let server = Server::start(vec![Canned::ok(SUB_PAGE)]);
    let storages = remote_storages(&[s3_account(server.url(""))], &deps(s3_secrets()));

    let entries = block_on(storages[0].1.list("shots/sub/".to_owned())).unwrap();

    let recorded = server.finish();
    assert!(query_of(&recorded[0].target).iter().any(|(key, value)| key == "prefix" && value == "shots/sub/"));
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, vec!["b.png"]);
    assert_eq!(entries[0].path, "shots/sub/b.png");
}

#[test]
fn s3_upload_puts_the_file_under_the_folder_and_returns_its_key() {
    let server = Server::start(vec![Canned::new(200, "")]);
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("Laqta 2.png");
    std::fs::write(&local, b"PNG-BYTES").unwrap();
    let storages = remote_storages(&[s3_account(server.url(""))], &deps(s3_secrets()));

    let key = block_on(storages[0].1.upload(local, "shots/sub/".to_owned())).unwrap();

    assert_eq!(key, "shots/sub/Laqta 2.png");
    let recorded = server.finish();
    let put = &recorded[0];
    assert_eq!(put.method, "PUT");
    assert!(put.target.starts_with("/bucket/shots/sub/Laqta%202.png?"), "{}", put.target);
    assert_eq!(put.body, b"PNG-BYTES");
    assert_eq!(put.header("content-type"), Some("image/png"));
}

#[test]
fn s3_refuses_to_rename_a_folder_without_sending_anything() {
    let server = Server::start(vec![]);
    let storages = remote_storages(&[s3_account(server.url(""))], &deps(s3_secrets()));

    let error = block_on(storages[0].1.rename("shots/sub/".to_owned(), "shots/other/".to_owned())).unwrap_err();

    assert!(server.finish().is_empty());
    assert!(matches!(error, laqta_ports::PortError::Unsupported(_)), "{error:?}");
}

#[test]
fn s3_rename_copies_the_object_then_deletes_the_source() {
    let server = Server::start(vec![Canned::new(200, ""), Canned::new(204, "")]);
    let storages = remote_storages(&[s3_account(server.url(""))], &deps(s3_secrets()));

    block_on(storages[0].1.rename("shots/a.png".to_owned(), "shots/sub/a.png".to_owned())).unwrap();

    let recorded = server.finish();
    assert_eq!((recorded[0].method.as_str(), recorded[0].target.split('?').next().unwrap()), ("PUT", "/bucket/shots/sub/a.png"));
    assert_eq!(recorded[0].header("x-amz-copy-source"), Some("/bucket/shots/a.png"));
    assert_eq!((recorded[1].method.as_str(), recorded[1].target.split('?').next().unwrap()), ("DELETE", "/bucket/shots/a.png"));
}

#[test]
fn s3_folder_creation_puts_an_empty_object_with_a_trailing_slash() {
    let server = Server::start(vec![Canned::new(200, "")]);
    let storages = remote_storages(&[s3_account(server.url(""))], &deps(s3_secrets()));

    block_on(storages[0].1.mkdir("shots/new".to_owned())).unwrap();

    let recorded = server.finish();
    assert_eq!(recorded[0].method, "PUT");
    assert_eq!(recorded[0].target.split('?').next().unwrap(), "/bucket/shots/new/");
    assert!(recorded[0].body.is_empty());
}

#[test]
fn s3_refuses_to_delete_a_folder_that_still_has_objects() {
    let server = Server::start(vec![Canned::ok(LAST_PAGE)]);
    let storages = remote_storages(&[s3_account(server.url(""))], &deps(s3_secrets()));

    let error = block_on(storages[0].1.delete("shots/".to_owned())).unwrap_err();

    server.finish();
    assert!(matches!(error, laqta_ports::PortError::Unsupported(_)), "{error:?}");
}

#[test]
fn unix_listing_lines_give_files_folders_and_names_with_spaces() {
    let file = parse_list_line("-rw-r--r--   1 user group  1234 Jan 01 12:00 file.png").unwrap();
    assert_eq!((file.name.as_str(), file.is_dir, file.size), ("file.png", false, Some(1234)));

    let folder = parse_list_line("drwxr-xr-x   2 user group  4096 Jan 01 12:00 photos").unwrap();
    assert_eq!((folder.name.as_str(), folder.is_dir, folder.size), ("photos", true, None));

    let with_year = parse_list_line("-rw-r--r-- 1 user group 99 Mar 03  2025 old.txt").unwrap();
    assert_eq!((with_year.name.as_str(), with_year.size), ("old.txt", Some(99)));

    let spaced = parse_list_line("-rw-r--r-- 1 user group 5 Jan 01 12:00 my file.png").unwrap();
    assert_eq!(spaced.name, "my file.png");
}

#[test]
fn dos_listing_lines_give_folders_and_sizes() {
    let folder = parse_list_line("01-02-26  10:00AM       <DIR>          photos").unwrap();
    assert_eq!((folder.name.as_str(), folder.is_dir, folder.size), ("photos", true, None));

    let file = parse_list_line("01-02-26  10:00AM                 1234 file.png").unwrap();
    assert_eq!((file.name.as_str(), file.is_dir, file.size), ("file.png", false, Some(1234)));
}

#[test]
fn mlsd_lines_give_the_type_size_and_name_from_the_facts() {
    let file = parse_mlsd_line("type=file;size=12;modify=20260101120000;perm=r; a.png").unwrap();
    assert_eq!((file.name.as_str(), file.is_dir, file.size), ("a.png", false, Some(12)));

    let folder = parse_mlsd_line("type=dir;modify=20260101120000; photos").unwrap();
    assert_eq!((folder.name.as_str(), folder.is_dir), ("photos", true));
}

#[test]
fn a_total_line_is_not_a_listing_entry() {
    assert!(parse_list_line("total 8").is_none());
}

#[test]
fn ftp_and_sftp_accounts_get_a_browser_each_and_other_hosts_none() {
    let mut ftp = HostAccount::new(HostKind::Ftp);
    ftp.values.insert("host".to_owned(), "files.example.com".to_owned());
    let mut sftp = HostAccount::new(HostKind::Sftp);
    sftp.values.insert("host".to_owned(), "sftp.example.com".to_owned());
    let dropbox = HostAccount::new(HostKind::Dropbox);

    let storages = remote_storages(&[dropbox, ftp, sftp], &deps(Arc::new(FakeSecretStore::default())));

    let labels: Vec<&str> = storages.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(labels, vec!["FTP / FTPS: files.example.com", "SFTP: sftp.example.com"]);
}
