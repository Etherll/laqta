//! DPAPI-encrypted ShareX secrets. ShareX stores some as base64 of a `CryptProtectData` blob. Such a
//! blob decrypts on Windows only, and only for the same user account. Elsewhere the caller reports the
//! secret and leaves it out.

use base64::Engine as _;
use base64::prelude::BASE64_STANDARD;

/// The first 20 bytes of every DPAPI blob: format version 1, then the DPAPI provider GUID
/// `{df9d8cd0-1501-11d1-8c7a-00c04fc297eb}` in little-endian order. In base64 these start `AQAAANCMnd8`.
const BLOB_PREFIX: [u8; 20] =
    [0x01, 0x00, 0x00, 0x00, 0xD0, 0x8C, 0x9D, 0xDF, 0x01, 0x15, 0xD1, 0x11, 0x8C, 0x7A, 0x00, 0xC0, 0x4F, 0xC2, 0x97, 0xEB];

/// The DPAPI blob a stored value holds, when it is base64 of one. Plain values return `None`.
pub(crate) fn blob_of(stored: &str) -> Option<Vec<u8>> {
    let bytes = BASE64_STANDARD.decode(stored.trim()).ok()?;
    bytes.starts_with(&BLOB_PREFIX).then_some(bytes)
}

#[cfg(windows)]
pub(crate) fn unprotect(blob: &[u8]) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptUnprotectData};

    let input = CRYPT_INTEGER_BLOB { cbData: u32::try_from(blob.len()).ok()?, pbData: blob.as_ptr().cast_mut() };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: `input` points at `blob`, which outlives the call. On success `output` holds memory from
    // LocalAlloc, which is copied out and freed below.
    unsafe { CryptUnprotectData(&input, None, None, None, None, 0, &mut output) }.ok()?;
    let plain = if output.cbData == 0 {
        // An empty output may have a null pointer, so nothing is read.
        Vec::new()
    } else {
        // SAFETY: the call succeeded, so `output.pbData` points at `output.cbData` readable bytes.
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec()
    };
    // SAFETY: `output.pbData` came from CryptUnprotectData and is freed exactly once.
    let _ = unsafe { LocalFree(Some(HLOCAL(output.pbData.cast()))) };
    Some(plain)
}

#[cfg(not(windows))]
pub(crate) fn unprotect(_blob: &[u8]) -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn detects_only_dpapi_blobs() {
        let mut blob = BLOB_PREFIX.to_vec();
        blob.extend_from_slice(&[7; 12]);
        let encoded = BASE64_STANDARD.encode(&blob);
        assert_eq!(blob_of(&encoded), Some(blob));
        assert_eq!(blob_of("hunter2"), None);
        assert_eq!(blob_of(&BASE64_STANDARD.encode(b"plain text")), None);
    }
}
