//! Global hotkeys (`HotkeyRegistrar`) through Carbon `RegisterEventHotKey`.
//!
//! Carbon is old but it is the one hotkey API that needs no Accessibility or Input Monitoring
//! permission, and it delivers events through the main run loop, so nothing polls. There is no
//! Carbon binding crate, so the few functions are declared here.
//!
//! Carbon hotkey calls must run on the main thread, so registration, unregistration and the event
//! handler all do. State is a static table because the handler is a plain C function.

use std::ffi::{c_ulong, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::{Mutex, MutexGuard};

use laqta_domain::action::Action;
use laqta_domain::hotkey::KeyChord;
use laqta_ports::{HotkeyCallback, HotkeyRegistrar, PortError};

use crate::keys::{key_code, modifier_mask};
use crate::main_thread::on_main;

type OsStatus = i32;
type HotkeyResults = Vec<(KeyChord, Result<(), PortError>)>;

const NO_ERR: OsStatus = 0;
/// `eventNotHandledErr`: tells Carbon to offer the event to the next handler.
const EVENT_NOT_HANDLED_ERR: OsStatus = -9874;
/// `eventHotKeyExistsErr`: another app (or the system) holds this key combination.
const EVENT_HOT_KEY_EXISTS_ERR: OsStatus = -9878;
/// `eventHotKeyInvalidErr`.
const EVENT_HOT_KEY_INVALID_ERR: OsStatus = -9879;

/// `kEventClassKeyboard`, 'keyb'.
const K_EVENT_CLASS_KEYBOARD: u32 = 0x6B65_7962;
/// `kEventHotKeyPressed`.
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
/// `kEventParamDirectObject`, '----'.
const K_EVENT_PARAM_DIRECT_OBJECT: u32 = 0x2D2D_2D2D;
/// `typeEventHotKeyID`, 'hkid'.
const TYPE_EVENT_HOT_KEY_ID: u32 = 0x686B_6964;
/// Our `EventHotKeyID.signature`, 'LQTA'.
const SIGNATURE: u32 = 0x4C51_5441;

/// `EventTypeSpec`.
#[repr(C)]
#[derive(Debug)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

/// `EventHotKeyID`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

/// `EventHandlerProcPtr`: (call ref, event, user data).
type EventHandlerProc = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> OsStatus;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: EventHandlerProc,
        num_types: c_ulong,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut *mut c_void,
    ) -> OsStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        hot_key_id: EventHotKeyId,
        target: *mut c_void,
        options: u32,
        out_ref: *mut *mut c_void,
    ) -> OsStatus;
    fn UnregisterEventHotKey(hot_key: *mut c_void) -> OsStatus;
    fn GetEventParameter(
        event: *mut c_void,
        name: u32,
        desired_type: u32,
        out_actual_type: *mut u32,
        buffer_size: c_ulong,
        out_actual_size: *mut c_ulong,
        out_data: *mut c_void,
    ) -> OsStatus;
}

/// An `EventHotKeyRef`. Carbon hands out opaque handles that are only used on the main thread, so
/// moving one into the static table is sound.
#[derive(Debug)]
struct HotKeyRef(*mut c_void);

// SAFETY: the pointer is never dereferenced here, only passed back to Carbon on the main thread.
unsafe impl Send for HotKeyRef {}

#[derive(Debug)]
struct Registered {
    id: u32,
    chord: KeyChord,
    hot_key: HotKeyRef,
}

struct State {
    handler_installed: bool,
    registered: Vec<Registered>,
    on_fire: Option<HotkeyCallback>,
}

static STATE: Mutex<State> = Mutex::new(State { handler_installed: false, registered: Vec::new(), on_fire: None });

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Default)]
pub struct MacHotkeys;

impl HotkeyRegistrar for MacHotkeys {
    fn grabs_keys(&self) -> bool {
        true
    }

    fn register(&self, bindings: &[(KeyChord, Action)], on_fire: HotkeyCallback) -> HotkeyResults {
        on_main(|_| register_on_main(bindings, on_fire))
    }

    fn unregister_all(&self) {
        // Nothing to undo before the first `register`, so don't wake the main thread for it.
        if state().registered.is_empty() {
            state().on_fire = None;
            return;
        }
        on_main(|_| unregister_registered());
    }
}

/// Carbon calls this on the main thread for every hotkey press.
extern "C" fn on_hot_key(_call: *mut c_void, event: *mut c_void, _user_data: *mut c_void) -> OsStatus {
    let mut hot_key_id = EventHotKeyId { signature: 0, id: 0 };
    // SAFETY: `event` is the live event Carbon passed to this handler, and `hot_key_id` is a
    // writable `EventHotKeyID`, which is the size given.
    let status = unsafe {
        GetEventParameter(
            event,
            K_EVENT_PARAM_DIRECT_OBJECT,
            TYPE_EVENT_HOT_KEY_ID,
            ptr::null_mut(),
            size_of::<EventHotKeyId>() as c_ulong,
            ptr::null_mut(),
            (&raw mut hot_key_id).cast(),
        )
    };
    if status != NO_ERR || hot_key_id.signature != SIGNATURE {
        return EVENT_NOT_HANDLED_ERR;
    }
    // Look up under the lock, call outside it: the callback may register again.
    let fire = {
        let s = state();
        s.registered.iter().find(|r| r.id == hot_key_id.id).map(|r| r.chord).zip(s.on_fire.clone())
    };
    let Some((chord, callback)) = fire else { return EVENT_NOT_HANDLED_ERR };
    // A panic must not unwind into Carbon.
    if catch_unwind(AssertUnwindSafe(|| callback(chord))).is_err() {
        tracing::error!("hotkey callback panicked for {chord:?}");
    }
    NO_ERR
}

/// Installs the hotkey handler on the application target once. Later calls are no-ops.
fn ensure_handler() -> Result<(), PortError> {
    if state().handler_installed {
        return Ok(());
    }
    let spec = EventTypeSpec { event_class: K_EVENT_CLASS_KEYBOARD, event_kind: K_EVENT_HOT_KEY_PRESSED };
    let mut handler_ref: *mut c_void = ptr::null_mut();
    // SAFETY: the target is the process-wide application target, `spec` is one valid entry, the
    // handler is a `'static` function, and `handler_ref` is a writable out pointer.
    let status = unsafe { InstallEventHandler(GetApplicationEventTarget(), on_hot_key, 1, &spec, ptr::null_mut(), &mut handler_ref) };
    if status != NO_ERR {
        tracing::warn!("InstallEventHandler failed: {status}");
        return Err(PortError::Failed(format!("macOS could not start listening for shortcuts (error {status})")));
    }
    state().handler_installed = true;
    Ok(())
}

/// Drops every registration and forgets the callback.
fn unregister_registered() {
    let old = {
        let mut s = state();
        s.on_fire = None;
        std::mem::take(&mut s.registered)
    };
    for registered in old {
        // SAFETY: the ref came from a successful `RegisterEventHotKey` and is unregistered once,
        // because it was just taken out of the table. We are on the main thread.
        let status = unsafe { UnregisterEventHotKey(registered.hot_key.0) };
        if status != NO_ERR {
            tracing::warn!("UnregisterEventHotKey failed: {status}");
        }
    }
}

fn register_on_main(bindings: &[(KeyChord, Action)], on_fire: HotkeyCallback) -> HotkeyResults {
    unregister_registered();
    if bindings.is_empty() {
        return Vec::new();
    }
    if let Err(e) = ensure_handler() {
        return bindings.iter().map(|(chord, _)| (*chord, Err(e.clone()))).collect();
    }
    let mut registered = Vec::new();
    let mut results = Vec::with_capacity(bindings.len());
    for (index, (chord, _)) in bindings.iter().enumerate() {
        let id = index as u32 + 1;
        let result = register_one(*chord, id).map(|hot_key| registered.push(Registered { id, chord: *chord, hot_key }));
        results.push((*chord, result));
    }
    let mut s = state();
    s.registered = registered;
    s.on_fire = Some(on_fire);
    results
}

fn register_one(chord: KeyChord, id: u32) -> Result<HotKeyRef, PortError> {
    let code = key_code(chord.key).ok_or_else(|| PortError::Unsupported("this key does not exist on a Mac keyboard".into()))?;
    let mut hot_key: *mut c_void = ptr::null_mut();
    // SAFETY: plain Carbon call on the main thread; the target is the application target and
    // `hot_key` is a writable out pointer.
    let status = unsafe {
        RegisterEventHotKey(
            code,
            modifier_mask(chord.modifiers),
            EventHotKeyId { signature: SIGNATURE, id },
            GetApplicationEventTarget(),
            0,
            &mut hot_key,
        )
    };
    if status != NO_ERR || hot_key.is_null() {
        return Err(PortError::Failed(refusal_text(status)));
    }
    Ok(HotKeyRef(hot_key))
}

/// The text shown for a shortcut that macOS refused.
fn refusal_text(status: OsStatus) -> String {
    match status {
        EVENT_HOT_KEY_EXISTS_ERR => "another app already uses this shortcut".into(),
        EVENT_HOT_KEY_INVALID_ERR => "macOS does not allow this shortcut".into(),
        _ => format!("macOS refused this shortcut (error {status})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortcut_already_taken_says_so() {
        assert_eq!(refusal_text(EVENT_HOT_KEY_EXISTS_ERR), "another app already uses this shortcut");
    }

    #[test]
    fn other_refusals_carry_the_status() {
        assert_eq!(refusal_text(EVENT_HOT_KEY_INVALID_ERR), "macOS does not allow this shortcut");
        assert_eq!(refusal_text(-50), "macOS refused this shortcut (error -50)");
    }
}
