//! Builds the native hosts from the user's host accounts (`laqta_domain::hosts`).

use std::sync::Arc;

use laqta_domain::hosts::{HostAccount, HostKind};
use laqta_ports::{Destination, Rng, SecretStore, Shell, Shortener};

use crate::oauth::OAuth;

/// What native hosts need from the outside.
#[derive(Clone)]
pub struct HostDeps {
    pub secrets: Arc<dyn SecretStore>,
    pub shell: Arc<dyn Shell>,
    /// Must be a cryptographically secure generator: it makes the PKCE verifiers.
    pub csprng: Arc<dyn Rng>,
    pub now_unix: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl std::fmt::Debug for HostDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostDeps")
    }
}

#[derive(Default)]
pub struct NativeHosts {
    pub destinations: Vec<Arc<dyn Destination>>,
    pub shorteners: Vec<Arc<dyn Shortener>>,
}

impl std::fmt::Debug for NativeHosts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeHosts").field("destinations", &self.destinations.len()).field("shorteners", &self.shorteners.len()).finish()
    }
}

fn oauth(deps: &HostDeps, mut config: crate::oauth::OAuthConfig, client_id: &str, secret: Option<String>, key: &str) -> Arc<OAuth> {
    if !client_id.is_empty() {
        config.client_id = client_id.to_string();
    }
    if secret.is_some() {
        config.client_secret = secret;
    }
    Arc::new(OAuth::new(config, deps.secrets.clone(), deps.shell.clone(), deps.csprng.clone(), key, deps.now_unix.clone()))
}

fn secret(deps: &HostDeps, account: &HostAccount, field: &str) -> Option<String> {
    deps.secrets.get(&account.secret_key(field)).ok().flatten().filter(|s| !s.is_empty())
}

fn port(a: &HostAccount, default: u16) -> u16 {
    a.get("port").trim().parse().unwrap_or(default)
}

/// One host per account. Imgur is always present (anonymous uploads need only a client id).
pub fn native_hosts(accounts: &[HostAccount], deps: &HostDeps) -> NativeHosts {
    let mut out = NativeHosts::default();
    let imgur_account = accounts.iter().find(|a| a.kind == HostKind::Imgur).cloned().unwrap_or_else(|| HostAccount::new(HostKind::Imgur));
    #[cfg(feature = "host-imgur")]
    {
        use crate::hosts::imgur;
        let id = match imgur_account.get("client_id") {
            "" => imgur::IMGUR_CLIENT_ID.to_string(),
            v => v.to_string(),
        };
        let signed_in = deps.secrets.get(imgur::SECRET_KEY).ok().flatten().is_some();
        let auth = signed_in.then(|| oauth(deps, imgur::oauth_config(), &id, None, imgur::SECRET_KEY));
        out.destinations.push(Arc::new(imgur::Imgur::new(id, auth, imgur::IMGUR_API.to_string())));
    }
    let _ = &imgur_account;
    for a in accounts {
        match a.kind {
            HostKind::Imgur => {}
            #[cfg(feature = "host-dropbox")]
            HostKind::Dropbox => {
                use crate::hosts::dropbox as d;
                let o = oauth(deps, d::oauth_config(), a.get("app_key"), None, d::SECRET_KEY);
                out.destinations.push(Arc::new(d::Dropbox::new(
                    o,
                    d::DROPBOX_API.into(),
                    d::DROPBOX_CONTENT.into(),
                    a.flag("direct_link"),
                )));
            }
            #[cfg(feature = "host-google-drive")]
            HostKind::GoogleDrive => {
                use crate::hosts::google_drive as g;
                let o = oauth(deps, g::oauth_config(), a.get("client_id"), secret(deps, a, "client_secret"), g::SECRET_KEY);
                out.destinations.push(Arc::new(g::GoogleDrive::new(o, g::GOOGLE_API.into(), a.flag("public"))));
            }
            #[cfg(feature = "host-onedrive")]
            HostKind::OneDrive => {
                use crate::hosts::onedrive as od;
                let o = oauth(deps, od::oauth_config(), a.get("client_id"), None, od::SECRET_KEY);
                out.destinations.push(Arc::new(od::OneDrive::new(o, od::ONEDRIVE_API.into())));
            }
            #[cfg(feature = "host-bitly")]
            HostKind::Bitly => {
                use crate::hosts::bitly as b;
                if let Some(token) = secret(deps, a, "access_token") {
                    let _ = deps.secrets.set(b::SECRET_KEY, &token);
                }
                out.shorteners.push(Arc::new(b::Bitly::new(None, deps.secrets.clone(), b::BITLY_API.into())));
            }
            #[cfg(feature = "host-s3")]
            HostKind::S3 => {
                use crate::hosts::s3;
                if let Some(sk) = secret(deps, a, "secret_access_key") {
                    let _ = deps.secrets.set(s3::SECRET_KEY, &sk);
                }
                let opt = |k: &str| Some(a.get(k).to_string()).filter(|s| !s.is_empty());
                let cfg = s3::S3Config {
                    endpoint: a.get("endpoint").into(),
                    region: a.get("region").into(),
                    bucket: a.get("bucket").into(),
                    path_prefix: a.get("path_prefix").into(),
                    access_key_id: a.get("access_key_id").into(),
                    use_path_style: a.flag("use_path_style"),
                    public_url_base: opt("public_url_base"),
                    acl_public_read: a.flag("acl_public_read"),
                };
                out.destinations.push(Arc::new(s3::S3::new(cfg, deps.secrets.clone())));
            }
            #[cfg(feature = "host-ftp")]
            HostKind::Ftp => {
                use crate::hosts::ftp;
                if let Some(pw) = secret(deps, a, "password") {
                    let _ = deps.secrets.set(ftp::PASSWORD_KEY, &pw);
                }
                let tls = match a.get("tls").trim() {
                    "explicit" => ftp::FtpTls::Explicit,
                    "implicit" => ftp::FtpTls::Implicit,
                    _ => ftp::FtpTls::None,
                };
                let cfg = ftp::FtpConfig {
                    host: a.get("host").into(),
                    port: port(a, 21),
                    username: a.get("username").into(),
                    directory: a.get("directory").into(),
                    url_base: a.get("url_base").into(),
                    passive: true,
                    tls,
                };
                out.destinations.push(Arc::new(ftp::Ftp::new(cfg, deps.secrets.clone())));
            }
            #[cfg(feature = "host-sftp")]
            HostKind::Sftp => {
                use crate::hosts::sftp;
                if let Some(pw) = secret(deps, a, "password") {
                    let _ = deps.secrets.set(sftp::PASSWORD_KEY, &pw);
                }
                let key = a.get("key_path");
                let cfg = sftp::SftpConfig {
                    host: a.get("host").into(),
                    port: port(a, 22),
                    username: a.get("username").into(),
                    key_path: (!key.is_empty()).then(|| key.into()),
                    directory: a.get("directory").into(),
                    url_base: a.get("url_base").into(),
                };
                out.destinations.push(Arc::new(sftp::Sftp::new(cfg, deps.secrets.clone())));
            }
            #[cfg(feature = "host-shared-folder")]
            HostKind::SharedFolder => {
                use crate::hosts::shared_folder as sf;
                let url = a.get("url_base");
                let cfg = sf::SharedFolderConfig { directory: a.get("directory").into(), url_base: (!url.is_empty()).then(|| url.into()) };
                out.destinations.push(Arc::new(sf::SharedFolder::new(cfg)));
            }
            HostKind::Gist => {
                use crate::hosts::gist as g;
                let config = g::GistConfig { api_base: g::GITHUB_API.into(), public: a.flag("public") };
                out.destinations.push(Arc::new(g::Gist::new(config, deps.secrets.clone())));
            }
            HostKind::PrivateBin => {
                use crate::hosts::privatebin as p;
                let config = p::PrivateBinConfig {
                    server: a.get("url").into(),
                    expire: a.get("expire").into(),
                    burn_after_reading: a.flag("burn_after_reading"),
                };
                out.destinations.push(Arc::new(p::PrivateBin::new(config, deps.secrets.clone(), deps.csprng.clone())));
            }
            HostKind::Pushbullet => {
                use crate::hosts::pushbullet as pb;
                let device = Some(a.get("device_iden").trim().to_owned()).filter(|device| !device.is_empty());
                let config = pb::PushbulletConfig { api_base: pb::PUSHBULLET_API.into(), device_iden: device };
                out.destinations.push(Arc::new(pb::Pushbullet::new(config, deps.secrets.clone())));
            }
            HostKind::Nextcloud => {
                use crate::hosts::nextcloud as nc;
                let config = nc::NextcloudConfig {
                    server: a.get("url").into(),
                    username: a.get("username").into(),
                    directory: a.get("directory").into(),
                    share: a.flag("share"),
                    direct_link: a.flag("direct_link"),
                };
                out.destinations.push(Arc::new(nc::Nextcloud::new(config, deps.secrets.clone())));
            }
            HostKind::SendSpace => {
                use crate::hosts::sendspace as ss;
                let config = ss::SendSpaceConfig { api_base: ss::SENDSPACE_API.into(), username: a.get("username").into() };
                out.destinations.push(Arc::new(ss::SendSpace::new(config, deps.secrets.clone())));
            }
            HostKind::MediaFire => {
                use crate::hosts::mediafire;
                let cfg = mediafire::MediaFireConfig {
                    api_base: mediafire::API_BASE.into(),
                    app_id: a.get("app_id").into(),
                    email: a.get("email").into(),
                    folder: a.get("folder").into(),
                    poll_interval: std::time::Duration::from_secs(1),
                };
                out.destinations.push(Arc::new(mediafire::MediaFire::new(cfg, deps.secrets.clone())));
            }
            HostKind::Smtp => {
                use crate::hosts::smtp;
                let tls = match a.get("tls").trim() {
                    "tls" => smtp::SmtpTls::Tls,
                    "none" => smtp::SmtpTls::None,
                    _ => smtp::SmtpTls::StartTls,
                };
                let config = smtp::SmtpConfig {
                    host: a.get("host").into(),
                    port: port(a, 587),
                    tls,
                    username: a.get("username").into(),
                    from: a.get("from").into(),
                    to: a.get("to").into(),
                    subject: a.get("subject").into(),
                };
                out.destinations.push(Arc::new(smtp::Smtp::new(config, deps.secrets.clone())));
            }
            HostKind::Flickr => {
                use crate::hosts::flickr::{Flickr, FlickrConfig, FlickrEndpoints};
                let config =
                    FlickrConfig { api_key: a.get("api_key").into(), public: a.flag("public"), endpoints: FlickrEndpoints::production() };
                out.destinations.push(Arc::new(Flickr::new(
                    config,
                    deps.secrets.clone(),
                    deps.shell.clone(),
                    deps.csprng.clone(),
                    deps.now_unix.clone(),
                )));
            }
            HostKind::Box => {
                use crate::hosts::box_com as b;
                let o = oauth(deps, b::oauth_config(), a.get("client_id"), secret(deps, a, "client_secret"), b::SECRET_KEY);
                out.destinations.push(Arc::new(b::BoxCom::new(
                    o,
                    b::BOX_API.into(),
                    b::BOX_UPLOAD.into(),
                    a.get("folder_id").into(),
                    a.flag("share"),
                )));
            }
            HostKind::YouTube => {
                use crate::hosts::youtube as y;
                let o = oauth(deps, y::oauth_config(), a.get("client_id"), secret(deps, a, "client_secret"), y::SECRET_KEY);
                out.destinations.push(Arc::new(y::YouTube::new(o, y::GOOGLE_UPLOAD.into(), a.get("privacy"))));
            }
            #[cfg(feature = "host-s3")]
            HostKind::GoogleCloudStorage => {
                use crate::hosts::gcs as g;
                let public = Some(a.get("public_url_base").to_owned()).filter(|s| !s.is_empty());
                let cfg = g::GcsConfig {
                    endpoint: g::GCS_ENDPOINT.into(),
                    bucket: a.get("bucket").into(),
                    path_prefix: a.get("path_prefix").into(),
                    access_key_id: a.get("access_key_id").into(),
                    public_url_base: public,
                };
                out.destinations.push(Arc::new(g::GoogleCloudStorage::new(&cfg, deps.secrets.clone())));
            }
            HostKind::AzureBlob => {
                use crate::hosts::azure_blob as az;
                let account = a.get("account");
                let public = Some(a.get("public_url_base").to_owned()).filter(|s| !s.is_empty());
                let cfg = az::AzureConfig {
                    account: account.into(),
                    container: a.get("container").into(),
                    path_prefix: a.get("path_prefix").into(),
                    public_url_base: public,
                    blob_base: format!("https://{account}.blob.core.windows.net"),
                };
                out.destinations.push(Arc::new(az::AzureBlob::new(cfg, deps.secrets.clone(), deps.now_unix.clone())));
            }
            HostKind::BackblazeB2 => {
                use crate::hosts::b2;
                let public = Some(a.get("public_url_base").to_owned()).filter(|s| !s.is_empty());
                let cfg = b2::B2Config {
                    api_base: b2::B2_API.into(),
                    key_id: a.get("key_id").into(),
                    bucket: a.get("bucket").into(),
                    path_prefix: a.get("path_prefix").into(),
                    public_url_base: public,
                };
                out.destinations.push(Arc::new(b2::BackblazeB2::new(cfg, deps.secrets.clone())));
            }
            #[allow(unreachable_patterns)]
            _ => tracing::warn!(kind = ?a.kind, "host not built into this binary"),
        }
    }
    out
}
