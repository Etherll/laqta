//! Keychain generic passwords (`SecretStore`): one item per key in the login keychain, under one
//! service name. Security.framework terms stay in this file.

use std::ptr::{self, NonNull};

use laqta_ports::{PortError, SecretStore};
use objc2_core_foundation::{CFBoolean, CFData, CFDictionary, CFRetained, CFString, CFType};
use objc2_security::{
    SecItemAdd, SecItemCopyMatching, SecItemDelete, SecItemUpdate, errSecDuplicateItem, errSecItemNotFound, errSecSuccess, kSecAttrAccount,
    kSecAttrService, kSecClass, kSecClassGenericPassword, kSecMatchLimit, kSecMatchLimitOne, kSecReturnData, kSecValueData,
};

/// The service name on every Laqta item; the account is the key.
const SERVICE: &str = "io.github.etherll.Laqta";

/// A CFDictionary from (key, value) pairs.
fn dictionary(pairs: &[(&CFType, &CFType)]) -> CFRetained<CFDictionary<CFType, CFType>> {
    let keys: Vec<&CFType> = pairs.iter().map(|(key, _)| *key).collect();
    let values: Vec<&CFType> = pairs.iter().map(|(_, value)| *value).collect();
    CFDictionary::from_slices(&keys, &values)
}

/// The query that finds the item for `key`, plus `extra` pairs.
fn query(key: &str, extra: &[(&CFType, &CFType)]) -> CFRetained<CFDictionary<CFType, CFType>> {
    let service = CFString::from_str(SERVICE);
    let account = CFString::from_str(key);
    // SAFETY: these are constants the framework defines; reading them has no other requirement.
    let (class_key, class_value, service_key, account_key) =
        unsafe { (kSecClass, kSecClassGenericPassword, kSecAttrService, kSecAttrAccount) };
    let mut pairs: Vec<(&CFType, &CFType)> = vec![(class_key, class_value), (service_key, &service), (account_key, &account)];
    pairs.extend_from_slice(extra);
    dictionary(&pairs)
}

fn failed(action: &str, status: i32) -> PortError {
    PortError::Failed(format!("the Keychain could not {action} (status {status})"))
}

/// Generic passwords in the user's login keychain.
#[derive(Debug, Default)]
pub struct Keychain;

impl SecretStore for Keychain {
    fn get(&self, key: &str) -> Result<Option<String>, PortError> {
        // SAFETY: constants the framework defines.
        let (return_data, limit_key, limit_one) = unsafe { (kSecReturnData, kSecMatchLimit, kSecMatchLimitOne) };
        let query = query(key, &[(return_data, CFBoolean::new(true)), (limit_key, limit_one)]);
        let mut found: *const CFType = ptr::null();
        // SAFETY: `query` is a valid dictionary alive for the call, and `found` is a valid place for
        // the result. On success the call hands over one retained reference, taken below.
        let status = unsafe { SecItemCopyMatching(query.as_opaque(), &mut found) };
        if status == errSecItemNotFound {
            return Ok(None);
        }
        if status != errSecSuccess {
            return Err(failed("read the secret", status));
        }
        let Some(found) = NonNull::new(found.cast_mut()) else {
            return Err(PortError::Failed("the Keychain returned no data".into()));
        };
        // SAFETY: SecItemCopyMatching returned a +1 reference that nothing else owns.
        let found = unsafe { CFRetained::from_raw(found) };
        let Ok(data) = found.downcast::<CFData>() else {
            return Err(PortError::Failed("the Keychain returned something other than data".into()));
        };
        String::from_utf8(data.to_vec()).map(Some).map_err(|e| PortError::Failed(format!("the secret for {key} is not UTF-8: {e}")))
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), PortError> {
        let data = CFData::from_bytes(secret.as_bytes());
        // SAFETY: a constant the framework defines.
        let value_key = unsafe { kSecValueData };
        let add = query(key, &[(value_key, &data)]);
        // SAFETY: `add` is a valid dictionary alive for the call; the result is not requested, so
        // the null pointer is allowed.
        let status = unsafe { SecItemAdd(add.as_opaque(), ptr::null_mut()) };
        if status == errSecSuccess {
            return Ok(());
        }
        if status != errSecDuplicateItem {
            return Err(failed("save the secret", status));
        }
        let find = query(key, &[]);
        let change = dictionary(&[(value_key, &data)]);
        // SAFETY: both dictionaries are valid and alive for the call.
        let status = unsafe { SecItemUpdate(find.as_opaque(), change.as_opaque()) };
        if status == errSecSuccess { Ok(()) } else { Err(failed("update the secret", status)) }
    }

    fn delete(&self, key: &str) -> Result<(), PortError> {
        let find = query(key, &[]);
        // SAFETY: `find` is a valid dictionary alive for the call.
        let status = unsafe { SecItemDelete(find.as_opaque()) };
        if status == errSecSuccess || status == errSecItemNotFound { Ok(()) } else { Err(failed("delete the secret", status)) }
    }
}

#[cfg(test)]
mod tests {
    use laqta_ports::SecretStore;

    use super::Keychain;

    /// Needs a login keychain, so it runs on a Mac (or the macOS runner) only.
    #[test]
    fn a_secret_round_trips_and_deletes() {
        let key = format!("test.round-trip.{}", std::process::id());
        let keychain = Keychain;
        assert_eq!(keychain.get(&key).expect("get"), None);
        keychain.set(&key, "first").expect("set");
        assert_eq!(keychain.get(&key).expect("get").as_deref(), Some("first"));
        keychain.set(&key, "second \u{1F510}").expect("overwrite");
        assert_eq!(keychain.get(&key).expect("get").as_deref(), Some("second \u{1F510}"));
        keychain.delete(&key).expect("delete");
        assert_eq!(keychain.get(&key).expect("get"), None);
        keychain.delete(&key).expect("delete again");
    }
}
