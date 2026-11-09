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
    Gist,
    PrivateBin,
    Pushbullet,
    Nextcloud,
    SendSpace,
    MediaFire,
    Smtp,
    Flickr,
    Box,
    YouTube,
    GoogleCloudStorage,
    AzureBlob,
    BackblazeB2,
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
    pub const ALL: [HostKind; 22] = [
        HostKind::Imgur,
        HostKind::Dropbox,
        HostKind::GoogleDrive,
        HostKind::OneDrive,
        HostKind::Bitly,
        HostKind::S3,
        HostKind::Ftp,
        HostKind::Sftp,
        HostKind::SharedFolder,
        HostKind::Gist,
        HostKind::PrivateBin,
        HostKind::Pushbullet,
        HostKind::Nextcloud,
        HostKind::SendSpace,
        HostKind::MediaFire,
        HostKind::Smtp,
        HostKind::Flickr,
        HostKind::Box,
        HostKind::YouTube,
        HostKind::GoogleCloudStorage,
        HostKind::AzureBlob,
        HostKind::BackblazeB2,
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
            HostKind::Gist => "gist",
            HostKind::PrivateBin => "privatebin",
            HostKind::Pushbullet => "pushbullet",
            HostKind::Nextcloud => "nextcloud",
            HostKind::SendSpace => "sendspace",
            HostKind::MediaFire => "mediafire",
            HostKind::Smtp => "email",
            HostKind::Flickr => "flickr",
            HostKind::Box => "box",
            HostKind::YouTube => "youtube",
            HostKind::GoogleCloudStorage => "google-cloud-storage",
            HostKind::AzureBlob => "azure-blob",
            HostKind::BackblazeB2 => "backblaze-b2",
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
            HostKind::Gist => "GitHub Gist",
            HostKind::PrivateBin => "PrivateBin",
            HostKind::Pushbullet => "Pushbullet",
            HostKind::Nextcloud => "Nextcloud / ownCloud",
            HostKind::SendSpace => "SendSpace",
            HostKind::MediaFire => "MediaFire",
            HostKind::Smtp => "E-mail (SMTP)",
            HostKind::Flickr => "Flickr",
            HostKind::Box => "Box",
            HostKind::YouTube => "YouTube",
            HostKind::GoogleCloudStorage => "Google Cloud Storage",
            HostKind::AzureBlob => "Azure Blob Storage",
            HostKind::BackblazeB2 => "Backblaze B2",
        }
    }

    pub fn fields(self) -> &'static [HostField] {
        match self {
            HostKind::Imgur => &[f!("client_id", "Client ID (from your Imgur app)", false, "")],
            HostKind::Dropbox => &[
                f!("app_key", "App key (from your Dropbox app)", false, ""),
                f!("direct_link", "Direct links (true/false)", false, "false"),
            ],
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
            HostKind::Gist => {
                &[f!("token", "Personal access token (gist scope)", true, ""), f!("public", "Public gist (true/false)", false, "false")]
            }
            HostKind::PrivateBin => &[
                f!("url", "Server URL", false, "https://privatebin.net"),
                f!("expire", "Expires (5min/10min/1hour/1day/1week/1month/1year/never)", false, "1week"),
                f!("burn_after_reading", "Burn after reading (true/false)", false, "false"),
                f!("password", "Paste password (optional)", true, ""),
            ],
            HostKind::Pushbullet => {
                &[f!("access_token", "Access token", true, ""), f!("device_iden", "Device (empty = all devices)", false, "")]
            }
            HostKind::Nextcloud => &[
                f!("url", "Server URL", false, ""),
                f!("username", "User name", false, ""),
                f!("app_password", "App password", true, ""),
                f!("directory", "Folder", false, "/Laqta"),
                f!("share", "Create a public share link (true/false)", false, "true"),
                f!("direct_link", "Direct download link (true/false)", false, "false"),
            ],
            HostKind::SendSpace => &[
                f!("api_key", "API key", true, ""),
                f!("username", "User name (optional)", false, ""),
                f!("password", "Password (optional)", true, ""),
            ],
            HostKind::MediaFire => &[
                f!("app_id", "Application ID", false, ""),
                f!("api_key", "API key", true, ""),
                f!("email", "E-mail", false, ""),
                f!("password", "Password", true, ""),
                f!("folder", "Folder key (empty = root)", false, ""),
            ],
            HostKind::Smtp => &[
                f!("host", "SMTP server", false, ""),
                f!("port", "Port", false, "587"),
                f!("tls", "Security (starttls/tls/none)", false, "starttls"),
                f!("username", "User name", false, ""),
                f!("password", "Password", true, ""),
                f!("from", "From", false, ""),
                f!("to", "To", false, ""),
                f!("subject", "Subject", false, "Laqta: %filename%"),
            ],
            HostKind::Flickr => &[
                f!("api_key", "API key", false, ""),
                f!("api_secret", "API secret", true, ""),
                f!("public", "Public (true/false)", false, "false"),
            ],
            HostKind::Box => &[
                f!("client_id", "OAuth client ID", false, ""),
                f!("client_secret", "OAuth client secret", true, ""),
                f!("folder_id", "Folder ID", false, "0"),
                f!("share", "Create a shared link (true/false)", false, "true"),
            ],
            HostKind::YouTube => &[
                f!("client_id", "OAuth client ID", false, ""),
                f!("client_secret", "OAuth client secret", true, ""),
                f!("privacy", "Privacy (public/unlisted/private)", false, "unlisted"),
            ],
            HostKind::GoogleCloudStorage => &[
                f!("bucket", "Bucket", false, ""),
                f!("path_prefix", "Path prefix", false, ""),
                f!("access_key_id", "HMAC access ID", false, ""),
                f!("secret_access_key", "HMAC secret", true, ""),
                f!("public_url_base", "Public URL base (optional)", false, ""),
            ],
            HostKind::AzureBlob => &[
                f!("account", "Storage account", false, ""),
                f!("container", "Container", false, ""),
                f!("sas_token", "SAS token (or use the account key)", true, ""),
                f!("account_key", "Account key (optional)", true, ""),
                f!("path_prefix", "Path prefix", false, ""),
                f!("public_url_base", "Public URL base (optional)", false, ""),
            ],
            HostKind::BackblazeB2 => &[
                f!("key_id", "Application key ID", false, ""),
                f!("application_key", "Application key", true, ""),
                f!("bucket", "Bucket name", false, ""),
                f!("path_prefix", "Path prefix", false, ""),
                f!("public_url_base", "Public URL base (optional)", false, ""),
            ],
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
        HostAccount {
            kind,
            values: kind.fields().iter().filter(|f| !f.secret).map(|f| (f.key.to_string(), f.default.to_string())).collect(),
        }
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
