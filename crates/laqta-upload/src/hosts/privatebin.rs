//! PrivateBin v2 pastes. The text is encrypted here, so the server stores only ciphertext. Each
//! paste draws a 32-byte key, a 16-byte salt and a 16-byte IV. PBKDF2-HMAC-SHA256 (100 000
//! iterations) turns key‖password and the salt into the AES-256-GCM key. The tag is 128 bits, and
//! the authenticated data is the `adata` array that goes into the request. The plaintext is the
//! JSON `{"paste": text}`, raw-deflated. The key goes into the URL fragment in base58, so the server
//! never sees it. The paste password is read from `SecretStore` under `laqta/privatebin/password`.

use std::fmt;
use std::io::{self, Write};
use std::sync::Arc;

use aes_gcm::AesGcm;
use aes_gcm::aead::consts::U16;
use aes_gcm::aead::{Aead, KeyInit, Nonce, Payload};
use aes_gcm::aes::Aes256;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use flate2::Compression;
use flate2::write::{DeflateDecoder, DeflateEncoder};
use laqta_http::{Method, Request};
use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadError, UploadRequest, UploadResult};
use laqta_ports::{BoxFuture, Rng};
use serde::Deserialize;
use serde_json::Value;

use crate::hosts::{call, data_bytes, json, json_body, port_error};

/// Where the paste password lives in `SecretStore`.
pub const PASSWORD_KEY: &str = "laqta/privatebin/password";

/// PBKDF2 iterations of the v2 format.
const ITERATIONS: u32 = 100_000;
const KEY_LEN: usize = 32;
const SALT_LEN: usize = 16;
const IV_LEN: usize = 16;

type Cipher = AesGcm<Aes256, U16>;

/// One encrypted paste, ready to post.
#[derive(Clone)]
pub struct Sealed {
    /// The paste key. It goes into the URL fragment and never reaches the server.
    pub key: [u8; KEY_LEN],
    /// The `adata` array: the cipher parameters, the format, the discussion flag and the burn flag.
    pub adata: Value,
    /// The base64 ciphertext with the 16-byte tag appended.
    pub ct: String,
}

impl fmt::Debug for Sealed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sealed").field("adata", &self.adata).finish_non_exhaustive()
    }
}

/// Where the paste goes and how long it lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivateBinConfig {
    /// The server, e.g. `https://privatebin.net`.
    pub server: String,
    /// PrivateBin's expiry name: `5min`, `10min`, `1hour`, `1day`, `1week`, `1month`, `1year` or `never`.
    pub expire: String,
    pub burn_after_reading: bool,
}

/// The PrivateBin destination.
#[derive(Clone)]
pub struct PrivateBin {
    config: PrivateBinConfig,
    secrets: Arc<dyn SecretStore>,
    rng: Arc<dyn Rng>,
}

impl PrivateBin {
    /// `rng` must be cryptographically secure: it draws the paste key, salt and IV.
    pub fn new(config: PrivateBinConfig, secrets: Arc<dyn SecretStore>, rng: Arc<dyn Rng>) -> Self {
        PrivateBin { config, secrets, rng }
    }
}

impl fmt::Debug for PrivateBin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrivateBin").field("config", &self.config).finish_non_exhaustive()
    }
}

impl Destination for PrivateBin {
    fn id(&self) -> &str {
        "privatebin"
    }

    fn name(&self) -> &str {
        "PrivateBin"
    }

    fn upload(&self, request: UploadRequest, _progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(transfer(self.config.clone(), Arc::clone(&self.secrets), Arc::clone(&self.rng), request))
    }
}

async fn transfer(
    config: PrivateBinConfig,
    secrets: Arc<dyn SecretStore>,
    rng: Arc<dyn Rng>,
    request: UploadRequest,
) -> Result<UploadResult, UploadError> {
    let text = String::from_utf8(data_bytes(request.data)?)
        .map_err(|_| UploadError::Unsupported("a PrivateBin paste takes UTF-8 text".to_owned()))?;
    let password = secrets.get(PASSWORD_KEY).map_err(port_error)?.unwrap_or_default();
    let sealed = seal(&text, &password, config.burn_after_reading, rng.as_ref())?;

    let server = config.server.trim_end_matches('/');
    let body = serde_json::json!({ "v": 2, "adata": sealed.adata, "meta": { "expire": config.expire }, "ct": sealed.ct });
    let mut http = Request::new(Method::Post, format!("{server}/"));
    http.headers = vec![("X-Requested-With".to_owned(), "JSONHttpRequest".to_owned())];
    http.body = json_body(&body);
    let response = call(http, None).await?;

    let reply: Reply = json(&response)?;
    if reply.status != 0 {
        let message = reply.message.unwrap_or_else(|| format!("status {}", reply.status));
        return Err(UploadError::BadResponse(format!("PrivateBin: {message}")));
    }
    let id =
        reply.id.filter(|id| !id.is_empty()).ok_or_else(|| UploadError::BadResponse("PrivateBin: the reply has no paste id".to_owned()))?;
    Ok(UploadResult { url: Some(paste_url(server, &id, &sealed.key)), response: response.text(), ..UploadResult::default() })
}

#[derive(Deserialize)]
struct Reply {
    status: i64,
    id: Option<String>,
    message: Option<String>,
}

/// Encrypts `text` for a paste. An empty `password` means none. The randomness comes from `rng`.
pub fn seal(text: &str, password: &str, burn_after_reading: bool, rng: &dyn Rng) -> Result<Sealed, UploadError> {
    let key: [u8; KEY_LEN] = random_bytes(rng);
    let salt: [u8; SALT_LEN] = random_bytes(rng);
    let iv: [u8; IV_LEN] = random_bytes(rng);

    let adata = serde_json::json!([
        [STANDARD.encode(iv), STANDARD.encode(salt), ITERATIONS, 256, 128, "aes", "gcm", "zlib"],
        "plaintext",
        0,
        u8::from(burn_after_reading)
    ]);
    let aad = adata.to_string();
    let compressed = deflate(serde_json::json!({ "paste": text }).to_string().as_bytes())?;
    let ciphertext = cipher(&key, password, &salt)?
        .encrypt(&nonce(&iv)?, Payload { msg: &compressed, aad: aad.as_bytes() })
        .map_err(|_| UploadError::Unsupported("PrivateBin encryption failed".to_owned()))?;
    Ok(Sealed { key, adata, ct: STANDARD.encode(ciphertext) })
}

/// Decrypts a paste from the key (the URL fragment, decoded), the password and the stored `adata`
/// and ciphertext. Fails when any of them differs from what was sealed.
pub fn open(key: &[u8], adata: &Value, ct: &str, password: &str) -> Result<String, UploadError> {
    let params = adata.get(0).and_then(Value::as_array);
    let iv = decode_field(params.and_then(|p| p.first()))?;
    let salt = decode_field(params.and_then(|p| p.get(1)))?;
    let ciphertext = STANDARD.decode(ct).map_err(|error| bad_paste(format!("ciphertext: {error}")))?;

    let aad = adata.to_string();
    let compressed = cipher(key, password, &salt)?
        .decrypt(&nonce(&iv)?, Payload { msg: &ciphertext, aad: aad.as_bytes() })
        .map_err(|_| UploadError::Auth("the paste does not open with this key and password".to_owned()))?;
    let plaintext = inflate(&compressed)?;
    let value: Value = serde_json::from_slice(&plaintext).map_err(|error| bad_paste(format!("plaintext: {error}")))?;
    value.get("paste").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| bad_paste("no paste field".to_owned()))
}

/// The paste's address: the server, the paste id as the query, and the base58 key as the fragment.
pub fn paste_url(server: &str, id: &str, key: &[u8]) -> String {
    format!("{}/?{id}#{}", server.trim_end_matches('/'), bs58::encode(key).into_string())
}

/// The AES-256-GCM cipher for a paste: the key is PBKDF2 over key‖password with the salt.
fn cipher(key: &[u8], password: &str, salt: &[u8]) -> Result<Cipher, UploadError> {
    let mut input = Vec::with_capacity(key.len() + password.len());
    input.extend_from_slice(key);
    input.extend_from_slice(password.as_bytes());
    let mut derived = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(&input, salt, ITERATIONS, &mut derived);
    Cipher::new_from_slice(&derived).map_err(|_| UploadError::Unsupported("PrivateBin key length".to_owned()))
}

fn nonce(iv: &[u8]) -> Result<Nonce<Cipher>, UploadError> {
    Nonce::<Cipher>::try_from(iv).map_err(|_| bad_paste(format!("IV of {} bytes", iv.len())))
}

fn random_bytes<const N: usize>(rng: &dyn Rng) -> [u8; N] {
    let mut out = [0u8; N];
    for chunk in out.chunks_mut(8) {
        let word = rng.next_u64().to_le_bytes();
        let len = chunk.len();
        chunk.copy_from_slice(&word[..len]);
    }
    out
}

fn decode_field(value: Option<&Value>) -> Result<Vec<u8>, UploadError> {
    let text = value.and_then(Value::as_str).ok_or_else(|| bad_paste("a missing cipher parameter".to_owned()))?;
    STANDARD.decode(text).map_err(|error| bad_paste(format!("cipher parameter: {error}")))
}

fn deflate(bytes: &[u8]) -> Result<Vec<u8>, UploadError> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).map_err(io_error)?;
    encoder.finish().map_err(io_error)
}

fn inflate(bytes: &[u8]) -> Result<Vec<u8>, UploadError> {
    let mut decoder = DeflateDecoder::new(Vec::new());
    decoder.write_all(bytes).map_err(|error| bad_paste(format!("compressed data: {error}")))?;
    decoder.finish().map_err(|error| bad_paste(format!("compressed data: {error}")))
}

fn bad_paste(message: String) -> UploadError {
    UploadError::BadResponse(format!("PrivateBin paste: {message}"))
}

fn io_error(error: io::Error) -> UploadError {
    UploadError::Unsupported(format!("PrivateBin compression: {error}"))
}
