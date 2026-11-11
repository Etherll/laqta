//! Windows Snipping and the Print Screen key. Whether Snipping takes a bare Print Screen is a pure
//! rule that also builds under `test`, so it is tested on Linux. The registry reads and writes are
//! Windows-only. Before Laqta changes Snipping's setting it saves the old value, so uninstalling can
//! put it back.

#[cfg(windows)]
use std::io;

#[cfg(windows)]
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
#[cfg(windows)]
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, REG_SZ,
    REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
#[cfg(windows)]
use windows::core::PCWSTR;

/// Windows 11 22H2. From this build on, Snipping takes bare Print Screen unless the setting says otherwise.
#[cfg(any(windows, test))]
const SNIPPING_DEFAULT_BUILD: u32 = 22621;

#[cfg(windows)]
const CONTROL_KEYBOARD: &str = r"Control Panel\Keyboard";
#[cfg(windows)]
const SNIPPING_VALUE: &str = "PrintScreenKeyForSnippingEnabled";
#[cfg(windows)]
const BUILD_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
#[cfg(windows)]
const BUILD_VALUE: &str = "CurrentBuildNumber";
#[cfg(windows)]
const LAQTA_KEY: &str = r"Software\Laqta";
#[cfg(windows)]
const SAVED_VALUE: &str = "PrevPrintScreenKeyForSnipping";
/// The saved marker for a machine where Snipping had no value of its own.
#[cfg(windows)]
const MISSING: &str = "missing";

/// Whether Snipping takes bare Print Screen: the stored setting when there is one, otherwise the
/// default for the build.
#[cfg(any(windows, test))]
pub(crate) fn print_screen_taken(value: Option<u32>, build: u32) -> bool {
    match value {
        Some(v) => v != 0,
        None => build >= SNIPPING_DEFAULT_BUILD,
    }
}

/// Snipping's Print Screen setting, or None when the value is missing.
#[cfg(windows)]
pub fn print_screen_for_snipping() -> Option<u32> {
    let keyboard = RegKey::open(HKEY_CURRENT_USER, CONTROL_KEYBOARD, KEY_READ).ok().flatten()?;
    keyboard.dword(SNIPPING_VALUE).ok().flatten()
}

/// Sets Snipping's Print Screen setting: `true` lets Snipping take bare Print Screen, `false` leaves
/// the key to Laqta. The first call saves the value it replaces, for `restore_print_screen_for_snipping`.
#[cfg(windows)]
pub fn set_print_screen_for_snipping(on: bool) -> io::Result<()> {
    let keyboard = RegKey::create(CONTROL_KEYBOARD)?;
    let laqta = RegKey::create(LAQTA_KEY)?;
    if laqta.read(SAVED_VALUE)?.is_none() {
        let previous = keyboard.dword(SNIPPING_VALUE)?.map_or_else(|| MISSING.to_owned(), |v| v.to_string());
        laqta.set_text(SAVED_VALUE, &previous)?;
    }
    keyboard.set_dword(SNIPPING_VALUE, u32::from(on))
}

/// Puts back the Snipping setting that `set_print_screen_for_snipping` saved, and forgets it. Does
/// nothing when Laqta has not changed the setting.
#[cfg(windows)]
pub fn restore_print_screen_for_snipping() -> io::Result<()> {
    let Some(laqta) = RegKey::open(HKEY_CURRENT_USER, LAQTA_KEY, KEY_READ | KEY_WRITE)? else {
        return Ok(());
    };
    let Some(saved) = laqta.text(SAVED_VALUE)? else {
        return Ok(());
    };
    let keyboard = RegKey::create(CONTROL_KEYBOARD)?;
    if saved == MISSING {
        keyboard.delete(SNIPPING_VALUE)?;
    } else {
        let value = saved.trim().parse::<u32>().map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("saved Print Screen setting {saved:?} is not a number: {e}"))
        })?;
        keyboard.set_dword(SNIPPING_VALUE, value)?;
    }
    laqta.delete(SAVED_VALUE)
}

/// Whether Snipping takes bare Print Screen on this machine.
#[cfg(windows)]
pub(crate) fn snipping_takes_print_screen() -> bool {
    print_screen_taken(print_screen_for_snipping(), windows_build())
}

/// The Windows build number from `HKLM`, or 0 when it cannot be read.
#[cfg(windows)]
fn windows_build() -> u32 {
    RegKey::open(HKEY_LOCAL_MACHINE, BUILD_KEY, KEY_READ)
        .ok()
        .flatten()
        .and_then(|key| key.text(BUILD_VALUE).ok().flatten())
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// An open registry key, closed on drop.
#[cfg(windows)]
struct RegKey(HKEY);

#[cfg(windows)]
impl RegKey {
    /// Opens `path` under `root` with `access`, or returns None when it does not exist.
    fn open(root: HKEY, path: &str, access: REG_SAM_FLAGS) -> io::Result<Option<Self>> {
        let path = wide(path);
        let mut key = HKEY(std::ptr::null_mut());
        // SAFETY: `path` is NUL-terminated and alive for the call; `key` receives a handle that `Drop` closes.
        let status = unsafe { RegOpenKeyExW(root, PCWSTR(path.as_ptr()), None, access, &mut key) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(status)?;
        Ok(Some(Self(key)))
    }

    /// Opens `path` under `HKCU` for reading and writing, creating it and any missing parents.
    fn create(path: &str) -> io::Result<Self> {
        let path = wide(path);
        let mut key = HKEY(std::ptr::null_mut());
        // SAFETY: as in `open`.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(path.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                None,
                &mut key,
                None,
            )
        };
        check(status)?;
        Ok(Self(key))
    }

    /// The type and bytes of the value `name`, or None when it does not exist.
    fn read(&self, name: &str) -> io::Result<Option<(REG_VALUE_TYPE, Vec<u8>)>> {
        let name = wide(name);
        let mut kind = REG_VALUE_TYPE(0);
        let mut len = 0u32;
        // SAFETY: `name` is NUL-terminated and alive for the call. With no data buffer, the call only
        // reports the type and the size.
        let status =
            unsafe { RegQueryValueExW(self.0, PCWSTR(name.as_ptr()), None, Some(&mut kind as *mut _), None, Some(&mut len as *mut _)) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(status)?;
        let mut bytes = vec![0u8; len as usize];
        // SAFETY: `bytes` has room for `len` bytes, and `len` tells the call so.
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                PCWSTR(name.as_ptr()),
                None,
                Some(&mut kind as *mut _),
                Some(bytes.as_mut_ptr()),
                Some(&mut len as *mut _),
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(status)?;
        bytes.truncate(len as usize);
        Ok(Some((kind, bytes)))
    }

    /// The DWORD value `name`, or None when it is missing or not a DWORD.
    fn dword(&self, name: &str) -> io::Result<Option<u32>> {
        Ok(match self.read(name)? {
            Some((kind, bytes)) if kind == REG_DWORD => <[u8; 4]>::try_from(bytes.as_slice()).ok().map(u32::from_le_bytes),
            _ => None,
        })
    }

    /// The REG_SZ value `name` as text, or None when it is missing or not text.
    fn text(&self, name: &str) -> io::Result<Option<String>> {
        Ok(match self.read(name)? {
            Some((kind, bytes)) if kind == REG_SZ => {
                let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
                let text: Vec<u16> = units.into_iter().take_while(|&unit| unit != 0).collect();
                Some(String::from_utf16_lossy(&text))
            }
            _ => None,
        })
    }

    fn set_dword(&self, name: &str, value: u32) -> io::Result<()> {
        self.set(name, REG_DWORD, &value.to_le_bytes())
    }

    /// Sets the REG_SZ value `name`: UTF-16LE with the NUL terminator the registry expects.
    fn set_text(&self, name: &str, text: &str) -> io::Result<()> {
        let bytes: Vec<u8> = wide(text).iter().flat_map(|unit| unit.to_le_bytes()).collect();
        self.set(name, REG_SZ, &bytes)
    }

    fn set(&self, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> io::Result<()> {
        let name = wide(name);
        // SAFETY: `name` is NUL-terminated and alive for the call; `data` is the slice the call reads.
        let status = unsafe { RegSetValueExW(self.0, PCWSTR(name.as_ptr()), None, kind, Some(data)) };
        check(status)
    }

    /// Deletes the value `name`. A missing value is not an error.
    fn delete(&self, name: &str) -> io::Result<()> {
        let name = wide(name);
        // SAFETY: `name` is NUL-terminated and alive for the call.
        let status = unsafe { RegDeleteValueW(self.0, PCWSTR(name.as_ptr())) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        check(status)
    }
}

#[cfg(windows)]
impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful open or create and is closed exactly once. A
        // failed close leaves nothing a caller could act on, so the status is dropped.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// `text` as NUL-terminated UTF-16, the form the registry and `PCWSTR` take.
#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `Ok` on success. Any other status becomes an `io::Error` that keeps the Win32 code.
#[cfg(windows)]
fn check(status: WIN32_ERROR) -> io::Result<()> {
    if status == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(status.0 as i32)) }
}

#[cfg(test)]
mod tests {
    use super::print_screen_taken;

    #[test]
    fn stored_setting_wins_over_the_build_default() {
        assert!(print_screen_taken(Some(1), 19045));
        assert!(!print_screen_taken(Some(0), 22631));
    }

    #[test]
    fn missing_setting_follows_the_build() {
        assert!(print_screen_taken(None, 22621));
        assert!(!print_screen_taken(None, 22620));
        assert!(!print_screen_taken(None, 0));
    }
}
