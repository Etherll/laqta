//! The pointer highlight: a translucent disc that follows the pointer, as on Windows and X11. It
//! is a small click-through window above everything else.
//!
//! Recordings leave Laqta's own windows out, and the highlight has to be in them. So the recorder
//! calls [`prepare`] before it builds its content filter and names this window as the exception.
//! The window is made once and then only shown, hidden and moved.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSBox, NSBoxType, NSColor, NSScreenSaverWindowLevel, NSTitlePosition, NSWindow, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_core_graphics::{CGDisplayBounds, CGMainDisplayID};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::main_thread::on_main_later;

/// The disc's diameter in points, as on Windows.
const SIZE: f64 = 48.0;

thread_local! {
    /// The window, on the main thread only.
    static WINDOW: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
}

/// The pointer position the window should move to, as two `f32` bit patterns (x high, y low).
static TARGET: AtomicU64 = AtomicU64::new(0);
/// A move is already queued on the main thread; the tap sends hundreds of events a second.
static MOVE_QUEUED: AtomicBool = AtomicBool::new(false);
static VISIBLE: AtomicBool = AtomicBool::new(false);

fn window(mtm: MainThreadMarker) -> Retained<NSWindow> {
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow().as_ref() {
            return window.clone();
        }
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(SIZE, SIZE));
        // SAFETY: a borderless buffered window with a valid frame, created on the main thread.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: the window is kept in `WINDOW`, so AppKit must not free it when it closes.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(false);
        window.setIgnoresMouseEvents(true);
        window.setLevel(NSScreenSaverWindowLevel);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        // A custom box is the one stock view that draws a filled, bordered, rounded rect.
        let disc = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
        disc.setBoxType(NSBoxType::Custom);
        disc.setTitlePosition(NSTitlePosition::NoTitle);
        disc.setCornerRadius(SIZE / 2.0);
        disc.setBorderWidth(2.0);
        disc.setFillColor(&NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.92, 0.23, 0.35));
        disc.setBorderColor(&NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.6, 0.0, 0.9));
        window.setContentView(Some(&disc));
        *slot.borrow_mut() = Some(window.clone());
        window
    })
}

fn apply(mtm: MainThreadMarker) {
    let window = window(mtm);
    window.setAlphaValue(if VISIBLE.load(Ordering::Relaxed) { 1.0 } else { 0.0 });
    let bits = TARGET.load(Ordering::Relaxed);
    let (x, y) = (f64::from(f32::from_bits((bits >> 32) as u32)), f64::from(f32::from_bits(bits as u32)));
    // Events use a top-left origin; AppKit places windows from the bottom-left of the main display.
    let main_height = CGDisplayBounds(CGMainDisplayID()).size.height;
    window.setFrameOrigin(NSPoint::new(x - SIZE / 2.0, main_height - y - SIZE / 2.0));
}

/// Makes the window (invisible until the highlighter is on) and returns its window number, for the
/// recorder's content filter. `None` when the main thread does not answer within a second.
pub(crate) fn prepare() -> Option<u32> {
    let (tx, rx) = mpsc::channel();
    on_main_later(move |mtm| {
        apply(mtm);
        let window = window(mtm);
        window.orderFrontRegardless();
        let _ = tx.send(window.windowNumber());
    });
    rx.recv_timeout(Duration::from_secs(1)).ok().and_then(|number| u32::try_from(number).ok())
}

/// Shows or hides the disc.
pub(crate) fn set_visible(visible: bool) {
    VISIBLE.store(visible, Ordering::Relaxed);
    on_main_later(move |mtm| {
        apply(mtm);
        if visible {
            window(mtm).orderFrontRegardless();
        }
    });
}

/// Moves the disc to a pointer position in global points (top-left origin). Called for every mouse
/// event, so at most one move waits on the main thread.
pub(crate) fn follow(x: f64, y: f64) {
    TARGET.store((u64::from((x as f32).to_bits()) << 32) | u64::from((y as f32).to_bits()), Ordering::Relaxed);
    if !MOVE_QUEUED.swap(true, Ordering::AcqRel) {
        on_main_later(|mtm| {
            MOVE_QUEUED.store(false, Ordering::Release);
            apply(mtm);
        });
    }
}
