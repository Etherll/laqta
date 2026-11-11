//! `UploadersConfig.json`: custom uploaders (`.sxcu`), host accounts, their secrets and the custom
//! uploader each custom destination uses.

use laqta_domain::hosts::{HostAccount, HostKind};

use crate::dpapi;
use crate::jobs::CustomKind;
use crate::json::{self, Object};

/// The custom uploader each custom destination uses, by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Selections {
    image: Option<String>,
    text: Option<String>,
    file: Option<String>,
    url_shortener: Option<String>,
}

impl Selections {
    pub(crate) fn get(&self, kind: CustomKind) -> Option<&str> {
        match kind {
            CustomKind::Image => self.image.as_deref(),
            CustomKind::Text => self.text.as_deref(),
            CustomKind::File => self.file.as_deref(),
            CustomKind::UrlShortener => self.url_shortener.as_deref(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Uploaders {
    /// `.sxcu` documents as JSON text.
    pub custom: Vec<String>,
    pub hosts: Vec<HostAccount>,
    pub secrets: Vec<(String, String)>,
    pub selections: Selections,
}

pub(crate) fn read(root: &Object, warnings: &mut Vec<String>) -> Uploaders {
    let mut out = Uploaders::default();

    // Indexes into the list stay aligned with ShareX's selection indexes, so skipped entries keep a slot.
    let mut names = Vec::new();
    for (index, item) in json::list(root, "CustomUploadersList").iter().enumerate() {
        match (item.as_object(), item.as_object().and_then(|object| json::text(object, "Name"))) {
            (Some(_), Some(name)) => {
                out.custom.push(item.to_string());
                names.push(name.to_owned());
            }
            _ => {
                names.push(String::new());
                warnings.push(format!("UploadersConfig.json custom uploader {} has no Name or is not an object; skipped", index + 1));
            }
        }
    }
    out.selections = Selections {
        image: selected(root, "Image", &names),
        text: selected(root, "Text", &names),
        file: selected(root, "File", &names),
        url_shortener: selected(root, "URLShortener", &names),
    };

    if json::text(root, "ImgurAccountType").is_some_and(|kind| kind.eq_ignore_ascii_case("User")) {
        warnings.push("the Imgur account login is not transferred; sign in to Imgur again in Laqta".to_owned());
    }
    if let Some(client_id) = json::text(root, "ImgurClientID") {
        out.hosts.push(account(HostKind::Imgur, &[("client_id", client_id)]));
    }

    read_ftp_accounts(root, &mut out, warnings);

    if let Some(s3) = json::sub(root, "AmazonS3Settings") {
        read_s3(s3, &mut out, warnings);
    }

    if let Some(token) = json::sub(root, "BitlyOAuth2Info").and_then(|info| json::text(info, "AccessToken")) {
        let account = HostAccount::new(HostKind::Bitly);
        if let Some(plain) = reveal(token, "the Bitly access token", warnings) {
            out.secrets.push((account.secret_key("access_token"), plain));
            out.hosts.push(account);
        }
    }

    for key in ["DropboxOAuth2Info", "GoogleDriveOAuth2Info", "OneDriveOAuth2Info"] {
        if json::sub(root, key).is_some_and(|info| json::text(info, "AccessToken").is_some() || json::text(info, "RefreshToken").is_some())
        {
            warnings.push(format!("{key}: the sign-in is not transferred; sign in again in Laqta"));
        }
    }
    if json::list(root, "LocalhostAccountList").iter().any(|item| item.is_object()) {
        warnings.push("LocalhostAccountList is not imported; set the shared folder in Laqta".to_owned());
    }
    out
}

/// The name of the custom uploader a slot selected. The name is stored as text when ShareX wrote it that
/// way. Otherwise the slot holds an index into the uploader list, which ShareX writes even when it is 0.
fn selected(root: &Object, slot: &str, names: &[String]) -> Option<String> {
    if let Some(name) = json::text(root, &format!("Custom{slot}UploaderSelectedName")) {
        return Some(name.to_owned());
    }
    let keys = [format!("Custom{slot}UploaderSelected"), format!("Custom{slot}UploaderSelectedIndex"), format!("Custom{slot}Selected")];
    let index = keys.iter().find_map(|key| json::number(root, key))?;
    let index = usize::try_from(index as i64).ok()?;
    names.get(index).filter(|name| !name.is_empty()).cloned()
}

/// The TLS mode of an FTPS account: `FTPSEncryption` when ShareX wrote it. Otherwise the port decides,
/// because 990 is the implicit-TLS port.
fn tls_mode(object: &Object, protocol: &str) -> &'static str {
    if !protocol.eq_ignore_ascii_case("FTPS") {
        return "none";
    }
    let mode = json::text(object, "FTPSEncryption").unwrap_or("");
    let implicit = if mode.is_empty() {
        json::number(object, "Port").map(|port| port.round() as u32) == Some(990)
    } else {
        mode.eq_ignore_ascii_case("Implicit")
    };
    if implicit { "implicit" } else { "explicit" }
}

/// ShareX allows many FTP accounts; Laqta has one per kind, because its secret keys have no account
/// index. The account ShareX selects for images, text or files is taken first. Then the first FTP account
/// and the first SFTP account in the list are taken.
fn read_ftp_accounts(root: &Object, out: &mut Uploaders, warnings: &mut Vec<String>) {
    let mut ftp_taken = false;
    let mut sftp_taken = false;
    let accounts = json::list(root, "FTPAccountList");
    let selected = ["FTPSelectedImage", "FTPSelectedText", "FTPSelectedFile"]
        .iter()
        .find_map(|key| json::number(root, key))
        .filter(|index| *index >= 0.0 && (*index as usize) < accounts.len())
        .map(|index| index as usize);
    let order = selected.into_iter().chain((0..accounts.len()).filter(|index| Some(*index) != selected));
    for item in order.map(|index| &accounts[index]) {
        let Some(object) = item.as_object() else {
            warnings.push("an FTP account is not an object; skipped".to_owned());
            continue;
        };
        let name = json::text(object, "Name").unwrap_or("(unnamed)");
        let protocol = json::text(object, "Protocol").unwrap_or("FTP");
        let kind = if protocol.eq_ignore_ascii_case("SFTP") { HostKind::Sftp } else { HostKind::Ftp };
        let taken = if kind == HostKind::Sftp { &mut sftp_taken } else { &mut ftp_taken };
        if std::mem::replace(taken, true) {
            warnings.push(format!("FTP account `{name}` skipped: Laqta keeps one {} account", kind.label()));
            continue;
        }
        if !matches!(protocol.to_ascii_uppercase().as_str(), "FTP" | "FTPS" | "SFTP") {
            warnings.push(format!("FTP account `{name}` has protocol `{protocol}`; treated as FTP"));
        }
        let mut account = HostAccount::new(kind);
        set(&mut account, "host", json::text(object, "Host"));
        if let Some(port) = json::number(object, "Port").filter(|port| (1.0..=65535.0).contains(port)) {
            account.values.insert("port".to_owned(), (port as u32).to_string());
        }
        set(&mut account, "username", json::text(object, "Username"));
        set(&mut account, "directory", json::text(object, "SubFolderPath"));
        set(&mut account, "url_base", json::text(object, "HttpHomePath"));
        if kind == HostKind::Sftp {
            set(&mut account, "key_path", json::text(object, "Keypath").or_else(|| json::text(object, "KeyPath")));
        } else {
            account.values.insert("tls".to_owned(), tls_mode(object, protocol).to_owned());
        }
        let password =
            json::text(object, "Password").and_then(|stored| reveal(stored, &format!("the password of FTP account `{name}`"), warnings));
        if let Some(plain) = password {
            out.secrets.push((account.secret_key("password"), plain));
        }
        out.hosts.push(account);
    }
}

fn read_s3(object: &Object, out: &mut Uploaders, warnings: &mut Vec<String>) {
    let access_key = json::text(object, "AccessKeyID");
    let secret_key = json::text(object, "SecretAccessKey");
    let bucket = json::text(object, "Bucket");
    if access_key.is_none() && secret_key.is_none() && bucket.is_none() {
        return;
    }
    let mut account = HostAccount::new(HostKind::S3);
    set(&mut account, "endpoint", json::text(object, "Endpoint"));
    set(&mut account, "region", json::text(object, "Region"));
    set(&mut account, "bucket", bucket);
    set(&mut account, "path_prefix", json::text(object, "ObjectPrefix"));
    set(&mut account, "access_key_id", access_key);
    // ShareX uses the custom domain only while `UseCustomCNAME` is on. A file without the switch keeps the domain.
    if json::flag(object, "UseCustomCNAME") == Some(false) {
        // `HostAccount::new` fills every field with its default, so the stored domain is dropped, not left empty.
        account.values.remove("public_url_base");
    } else {
        set(&mut account, "public_url_base", json::text(object, "CustomDomain"));
    }
    if let Some(path_style) = json::flag(object, "UsePathStyle") {
        account.values.insert("use_path_style".to_owned(), path_style.to_string());
    }
    if let Some(public_acl) = json::flag(object, "SetPublicACL") {
        account.values.insert("acl_public_read".to_owned(), public_acl.to_string());
    }
    let secret = secret_key.and_then(|stored| reveal(stored, "the S3 secret access key", warnings));
    if let Some(plain) = secret {
        out.secrets.push((account.secret_key("secret_access_key"), plain));
    }
    out.hosts.push(account);
}

fn account(kind: HostKind, values: &[(&str, &str)]) -> HostAccount {
    let mut account = HostAccount::new(kind);
    for (key, value) in values {
        account.values.insert((*key).to_owned(), (*value).to_owned());
    }
    account
}

fn set(account: &mut HostAccount, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        account.values.insert(key.to_owned(), value.to_owned());
    }
}

/// The plain text of a ShareX secret. A DPAPI blob is decrypted on Windows only. Elsewhere, or when it
/// cannot be decrypted, the secret is reported and left out.
fn reveal(stored: &str, what: &str, warnings: &mut Vec<String>) -> Option<String> {
    let Some(blob) = dpapi::blob_of(stored) else {
        return Some(stored.to_owned());
    };
    let plain = dpapi::unprotect(&blob).and_then(|bytes| String::from_utf8(bytes).ok());
    if plain.is_none() {
        warnings.push(format!("{what} is encrypted with DPAPI and cannot be read here; enter it again in Laqta"));
    }
    plain
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn selection_by_name_or_index() {
        let root: Object = serde_json::from_str(
            r#"{"CustomImageUploaderSelected": 1, "CustomTextUploaderSelectedName": "Named", "CustomURLShortenerSelected": 0}"#,
        )
        .unwrap();
        let names = vec!["First".to_owned(), "Second".to_owned()];
        assert_eq!(selected(&root, "Image", &names), Some("Second".to_owned()));
        assert_eq!(selected(&root, "Text", &names), Some("Named".to_owned()));
        assert_eq!(selected(&root, "URLShortener", &names), Some("First".to_owned()));
        assert_eq!(selected(&root, "File", &names), None);
    }

    #[test]
    fn ftps_mode_comes_from_the_setting_or_the_port() {
        let object = |text: &str| -> Object { serde_json::from_str(text).unwrap() };
        assert_eq!(tls_mode(&object(r#"{"FTPSEncryption": "Implicit", "Port": 21}"#), "FTPS"), "implicit");
        assert_eq!(tls_mode(&object(r#"{"FTPSEncryption": "Explicit", "Port": 990}"#), "FTPS"), "explicit");
        assert_eq!(tls_mode(&object(r#"{"Port": 990}"#), "FTPS"), "implicit");
        assert_eq!(tls_mode(&object(r#"{"Port": 21}"#), "FTPS"), "explicit");
        assert_eq!(tls_mode(&object(r#"{"Port": 990}"#), "FTP"), "none");
    }
}
