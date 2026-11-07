//! Native upload host accounts as data. `main` turns them into `laqta-upload` hosts; the settings UI
//! builds its forms from `HostKind::fields`. Secret fields are kept in the OS secret store, never here.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum HostKind {
    Imgur,
    Dropbox,
    GoogleDrive,
    OneDrive,
    Bitly,
    S3,
    Ftp,
    Sftp,
    SharedFolder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostField {
    pub key: &'static str,
    pub label: &'static str,
    /// Stored in the secret store under `laqta/<host id>/<key>`.
    pub secret: bool,
    pub default: &'static str,
}

/// A field literal (a struct expression, so the arrays below are promoted to statics).
macro_rules! f {
    ($key:expr, $label:expr, $secret:expr, $default:expr) => {
        HostField { key: $key, label: $label, secret: $secret, default: $default }
    };
}

impl HostKind {
    pub const ALL: [HostKind; 9] = [
        HostKind::Imgur,
        HostKind::Dropbox,
        HostKind::GoogleDrive,
        HostKind::OneDrive,
        HostKind::Bitly,
        HostKind::S3,
        HostKind::Ftp,
        HostKind::Sftp,
        HostKind::SharedFolder,
    ];

    /// The destination id in the upload registry.
    pub fn id(self) -> &'static str {
        match self {
            HostKind::Imgur => "imgur",
            HostKind::Dropbox => "dropbox",
            HostKind::GoogleDrive => "google-drive",
            HostKind::OneDrive => "onedrive",
            HostKind::Bitly => "bitly",
            HostKind::S3 => "s3",
            HostKind::Ftp => "ftp",
            HostKind::Sftp => "sftp",
            HostKind::SharedFolder => "shared-folder",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            HostKind::Imgur => "Imgur",
            HostKind::Dropbox => "Dropbox",
            HostKind::GoogleDrive => "Google Drive",
            HostKind::OneDrive => "OneDrive",
            HostKind::Bitly => "Bitly",
            HostKind::S3 => "Amazon S3 / S3-compatible",
            HostKind::Ftp => "FTP / FTPS",
            HostKind::Sftp => "SFTP",
            HostKind::SharedFolder => "Shared folder",
        }
    }

    pub fn fields(self) -> &'static [HostField] {
        match self {
            HostKind::Imgur => &[f!("client_id", "Client ID (from your Imgur app)", false, "")],
            HostKind::Dropbox => &[f!("app_key", "App key (from your Dropbox app)", false, ""), f!("direct_link", "Direct links (true/false)", false, "false")],
            HostKind::GoogleDrive => &[
                f!("client_id", "OAuth client ID", false, ""),
                f!("client_secret", "OAuth client secret", true, ""),
                f!("public", "Share publicly (true/false)", false, "true"),
            ],
            HostKind::OneDrive => &[f!("client_id", "Application (client) ID", false, "")],
            HostKind::Bitly => &[f!("access_token", "Generic access token", true, "")],
            HostKind::S3 => &[
                f!("endpoint", "Endpoint", false, "https://s3.amazonaws.com"),
                f!("region", "Region", false, "us-east-1"),
                f!("bucket", "Bucket", false, ""),
                f!("path_prefix", "Path prefix", false, ""),
                f!("access_key_id", "Access key ID", false, ""),
                f!("secret_access_key", "Secret access key", true, ""),
                f!("use_path_style", "Path-style URLs (true/false)", false, "false"),
                f!("public_url_base", "Public URL base (optional)", false, ""),
                f!("acl_public_read", "Public-read ACL (true/false)", false, "true"),
            ],
            HostKind::Ftp => &[
                f!("host", "Host", false, ""),
                f!("port", "Port", false, "21"),
                f!("username", "User name", false, ""),
                f!("password", "Password", true, ""),
                f!("directory", "Directory", false, "/"),
                f!("url_base", "URL base", false, ""),
                f!("tls", "TLS (none/explicit/implicit)", false, "none"),
            ],
            HostKind::Sftp => &[
                f!("host", "Host", false, ""),
                f!("port", "Port", false, "22"),
                f!("username", "User name", false, ""),
                f!("password", "Password (Linux only)", true, ""),
                f!("key_path", "Private key file", false, ""),
                f!("directory", "Directory", false, ""),
                f!("url_base", "URL base", false, ""),
            ],
            HostKind::SharedFolder => &[f!("directory", "Folder", false, ""), f!("url_base", "URL base (optional)", false, "")],
        }
    }
}

/// One configured account: values for the non-secret fields.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostAccount {
    pub kind: HostKind,
    #[serde(default)]
    pub values: BTreeMap<String, String>,
}

impl HostAccount {
    pub fn new(kind: HostKind) -> Self {
        HostAccount { kind, values: kind.fields().iter().filter(|f| !f.secret).map(|f| (f.key.to_string(), f.default.to_string())).collect() }
    }

    /// The value of a non-secret field, or its default.
    pub fn get(&self, key: &str) -> &str {
        self.values
            .get(key)
            .map(String::as_str)
            .or_else(|| self.kind.fields().iter().find(|f| f.key == key).map(|f| f.default))
            .unwrap_or("")
    }

    pub fn flag(&self, key: &str) -> bool {
        matches!(self.get(key).trim(), "true" | "yes" | "1")
    }

    /// The secret-store key for a secret field.
    pub fn secret_key(&self, key: &str) -> String {
        format!("laqta/{}/{key}", self.kind.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_have_defaults() {
        let a = HostAccount::new(HostKind::Ftp);
        assert_eq!(a.get("port"), "21");
        assert!(!a.values.contains_key("password"));
        assert_eq!(a.secret_key("password"), "laqta/ftp/password");
        let ids: std::collections::BTreeSet<_> = HostKind::ALL.iter().map(|k| k.id()).collect();
        assert_eq!(ids.len(), HostKind::ALL.len());
    }
}
