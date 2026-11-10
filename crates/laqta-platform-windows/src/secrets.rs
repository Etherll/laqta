//! Windows Credential Manager for `SecretStore`: generic credentials kept for this machine. Win32
//! terms stay in this file. The naming helper also builds under `test`, so its test runs on Linux.

use laqta_ports::PortError;
#[cfg(windows)]
use laqta_ports::SecretStore;
#[cfg(windows)]
use windows::Win32::Foundation::ERROR_NOT_FOUND;
#[cfg(windows)]
use windows::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW,
};
#[cfg(windows)]
use windows::core::{PCWSTR, PWSTR};

/// The Credential Manager target name for `key`, e.g. `Laqta/imgur.token`.
fn target_name(key: &str) -> String {
    format!("Laqta/{key}")
}

/// The most a generic credential's blob holds: CRED_MAX_CREDENTIAL_BLOB_SIZE, 5 × 512 bytes.
const CRED_MAX_BLOB_BYTES: u32 = 2560;

/// The blob size `CredWriteW` takes for `secret`, or an error when Credential Manager would refuse it.
fn blob_size(secret: &str) -> Result<u32, PortError> {
    u32::try_from(secret.len())
        .ok()
        .filter(|&len| len <= CRED_MAX_BLOB_BYTES)
        .ok_or_else(|| PortError::Failed("the secret is too long for Windows Credential Manager (max 2560 bytes)".to_owned()))
}

/// `text` as NUL-terminated UTF-16, the form the `PWSTR` and `PCWSTR` fields take.
#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn failed(action: &str, e: windows::core::Error) -> PortError {
    PortError::Failed(format!("Credential Manager {action}: {e}"))
}

/// Generic credentials in the user's Credential Manager, one per key.
#[cfg(windows)]
#[derive(Debug)]
pub(crate) struct CredentialStore;

#[cfg(windows)]
impl SecretStore for CredentialStore {
    fn get(&self, key: &str) -> Result<Option<String>, PortError> {
        let target = wide(&target_name(key));
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `target` is NUL-terminated and alive for the call. On success `cred` receives a
        // credential that CredRead allocated; it is freed below.
        let read = unsafe { CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None, &mut cred) };
        match read {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => return Ok(None),
            Err(e) => return Err(failed("read", e)),
        }
        let blob = unsafe {
            // SAFETY: CredRead succeeded, so `cred` points at a valid credential. The blob is copied
            // out before the credential is freed. An empty blob may have a null pointer, so it is
            // not read at all.
            let credential = &*cred;
            if credential.CredentialBlobSize == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(credential.CredentialBlob, credential.CredentialBlobSize as usize).to_vec()
            }
        };
        // SAFETY: `cred` came from CredReadW and is not used after this call.
        unsafe { CredFree(cred.cast::<std::ffi::c_void>()) };
        String::from_utf8(blob).map(Some).map_err(|e| PortError::Failed(format!("credential {key} is not UTF-8: {e}")))
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), PortError> {
        let mut target = wide(&target_name(key));
        let size = blob_size(secret)?;
        let mut blob = secret.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_mut_ptr()),
            CredentialBlobSize: size,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        // SAFETY: `credential` points at `target` and `blob`, which stay alive until the call
        // returns. CredWriteW copies both.
        let written = unsafe { CredWriteW(&credential, 0) };
        written.map_err(|e| failed("write", e))
    }

    fn delete(&self, key: &str) -> Result<(), PortError> {
        let target = wide(&target_name(key));
        // SAFETY: `target` is NUL-terminated and alive for the call.
        let deleted = unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) };
        match deleted {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
            Err(e) => Err(failed("delete", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use laqta_ports::PortError;

    use super::{blob_size, target_name};

    #[test]
    fn target_name_prefixes_the_key_with_laqta() {
        assert_eq!(target_name("imgur.token"), "Laqta/imgur.token");
    }

    #[test]
    fn blob_size_accepts_up_to_the_credential_limit_and_refuses_more() {
        assert_eq!(blob_size(&"a".repeat(2560)).ok(), Some(2560));
        assert!(matches!(blob_size(&"a".repeat(2561)), Err(PortError::Failed(_))));
    }
}
