//! Makes a window cover its whole screen, menu bar and Dock included. The region overlay needs
//! this: GPUI opens it as a titled panel, and AppKit keeps a titled window below the menu bar, so
//! the frozen screen it draws would sit lower than the real one.
//!
//! The style change has to happen before the window is key. Changing a window's style mask swaps
//! its frame view; AppKit's Touch Bar finder watches the key window's responder chain, frame view
//! included, and a later click makes it remove an observer from the frame view that is gone. That
//! throws, and AppKit ends the process (`_NSTouchBarFinderObservation invalidate`). So the caller
//! opens the window hidden and unfocused, and this restyles it, then shows it and gives it the
//! keyboard.

use std::ffi::c_void;
use std::ptr::NonNull;

use dispatch2::MainThreadBound;
use objc2::{MainThreadMarker, Message};
use objc2_app_kit::{NSView, NSWindowStyleMask};

use crate::main_thread::on_main_later;

/// Covers the screen with the window that holds `view` (an `NSView`, from the UI toolkit's window
/// handle), then orders it front as the key window with `view` taking the keys. The change is
/// queued, because the caller is inside the toolkit's own code and a resize calls back into it.
/// False off the main thread or when the view has no window; the caller then shows the window.
pub fn cover_screen(view: NonNull<c_void>) -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    // SAFETY: the caller passes the live `NSView` of a window it owns, on the main thread.
    let view = unsafe { view.cast::<NSView>().as_ref() }.retain();
    let Some(window) = view.window() else { return false };
    let held = MainThreadBound::new((window, view), mtm);
    on_main_later(move |mtm| {
        let (window, view) = held.get(mtm);
        // The toolkit clears the delegate when it drops the window; a closed overlay stays hidden.
        if window.delegate().is_none() {
            return;
        }
        if let Some(screen) = window.screen() {
            // A window without a title bar is not kept below the menu bar.
            window.setStyleMask(NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel);
            window.setFrame_display(screen.frame(), true);
        }
        window.makeKeyAndOrderFront(None);
        // Changing the style mask drops the first responder; the keys must still reach the view.
        window.makeFirstResponder(Some(view));
    });
    true
}
