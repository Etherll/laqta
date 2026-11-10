//! The pointer highlighter (`PointerHighlighter`): the event tracking half.
//!
//! While the highlighter is on, a thread of its own runs a listen-only `CGEventTap` on its CFRunLoop
//! and keeps the pointer's last position and button state. The tap needs Input Monitoring; it never
//! changes or swallows an event. Nothing runs while the highlighter is off: turning it off ends the
//! run loop, removes the tap and joins the thread.
//!
//! The Windows and X11 adapters also draw the disc themselves (a click-through, top-most window that
//! follows the pointer). This module does not: there is no overlay window here. `MacHighlighter::pointer`
//! hands the tracked state to whatever draws it.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use laqta_ports::{PointerHighlighter, PortError};
use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CFRunLoopRunResult, CFRunLoopSource, kCFRunLoopCommonModes, kCFRunLoopDefaultMode,
};
use objc2_core_graphics::{CGEvent, CGEventMask, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType};

use crate::permissions;

/// How long `set_enabled(true)` waits for the tap thread to report back.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// The run loop returns this often even when idle, so the thread sees the stop flag (a stop that
/// arrives before the loop is running is lost) and can turn a tap macOS switched off back on.
const POLL: Duration = Duration::from_millis(250);

/// The events the tap listens to: moves, drags and left/right button presses and releases.
const WATCHED: [CGEventType; 7] = [
    CGEventType::MouseMoved,
    CGEventType::LeftMouseDragged,
    CGEventType::RightMouseDragged,
    CGEventType::LeftMouseDown,
    CGEventType::LeftMouseUp,
    CGEventType::RightMouseDown,
    CGEventType::RightMouseUp,
];

/// `CGEventMaskBit` for each watched event type.
fn event_mask() -> CGEventMask {
    WATCHED.iter().fold(0, |mask, kind| mask | (1u64 << kind.0))
}

/// What the tap last saw of the pointer. `x` and `y` are in the global display space CoreGraphics
/// uses for events: points (not pixels), origin at the top-left of the main display, y down.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[allow(dead_code, reason = "read by the overlay that draws the highlight, which is not wired yet")]
pub(crate) struct PointerState {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) left_down: bool,
    pub(crate) right_down: bool,
    /// Counts the events seen, so a reader can tell that something moved or clicked.
    pub(crate) events: u64,
}

impl PointerState {
    /// Takes in one watched event at (`x`, `y`).
    fn apply(&mut self, kind: CGEventType, x: f64, y: f64) {
        self.x = x;
        self.y = y;
        match kind {
            CGEventType::LeftMouseDown => self.left_down = true,
            CGEventType::LeftMouseUp => self.left_down = false,
            CGEventType::RightMouseDown => self.right_down = true,
            CGEventType::RightMouseUp => self.right_down = false,
            _ => {}
        }
        self.events = self.events.wrapping_add(1);
    }
}

/// The pointer highlighter's event tracking. The tap lives on its own thread while it is on.
#[derive(Debug, Default)]
pub struct MacHighlighter {
    running: Mutex<Option<Running>>,
    state: Arc<Mutex<PointerState>>,
}

impl MacHighlighter {
    fn lock(&self) -> MutexGuard<'_, Option<Running>> {
        self.running.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn reset_state(&self) {
        *self.state.lock().unwrap_or_else(|p| p.into_inner()) = PointerState::default();
    }

    /// The pointer as the tap last saw it, or `None` while the highlighter is off. Before the first
    /// event it is all zeros (`events == 0`).
    #[allow(dead_code, reason = "called by the overlay that draws the highlight, which is not wired yet")]
    pub(crate) fn pointer(&self) -> Option<PointerState> {
        self.is_enabled().then(|| *self.state.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

impl PointerHighlighter for MacHighlighter {
    fn set_enabled(&self, on: bool) -> Result<(), PortError> {
        let mut running = self.lock();
        if let Some(current) = running.take() {
            // A thread that has exited counts as off, so turning on restarts it.
            if on && !current.thread.is_finished() {
                *running = Some(current);
                return Ok(());
            }
            current.stop();
            self.reset_state();
            crate::disc::set_visible(false);
        }
        if on {
            permissions::input_monitoring()?;
            *running = Some(Running::start(&self.state)?);
            // The disc follows mouse events; until the first one it would sit at the screen's
            // origin, so it starts where the pointer is.
            if let Some(event) = CGEvent::new(None) {
                let at = CGEvent::location(Some(&event));
                crate::disc::follow(at.x, at.y);
            }
            crate::disc::set_visible(true);
        }
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.lock().as_ref().is_some_and(|running| !running.thread.is_finished())
    }
}

impl Drop for MacHighlighter {
    fn drop(&mut self) {
        let _ = self.set_enabled(false);
    }
}

/// The thread that owns the tap while the highlighter is on.
#[derive(Debug)]
struct Running {
    stop: Arc<AtomicBool>,
    run_loop: RunLoopHandle,
    thread: JoinHandle<()>,
}

impl Running {
    fn start(state: &Arc<Mutex<PointerState>>) -> Result<Self, PortError> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready, started) = mpsc::channel();
        let (thread_state, thread_stop) = (state.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("laqta-highlight".into())
            .spawn(move || run(thread_state, thread_stop, ready))
            .map_err(|e| PortError::Failed(format!("highlight thread: {e}")))?;
        match started.recv_timeout(START_TIMEOUT) {
            Ok(Ok(run_loop)) => Ok(Self { stop, run_loop, thread }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                // The thread is stuck or gone; it will see the flag if it ever gets going again.
                stop.store(true, Ordering::Release);
                Err(PortError::Failed("the highlight thread did not start".into()))
            }
        }
    }

    /// Ends the run loop and waits for the thread, which removes the tap on its way out.
    fn stop(self) {
        self.stop.store(true, Ordering::Release);
        self.run_loop.0.stop();
        let _ = self.thread.join();
    }
}

/// The tap thread's run loop, held by the thread that stops it.
struct RunLoopHandle(CFRetained<CFRunLoop>);

impl std::fmt::Debug for RunLoopHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RunLoopHandle")
    }
}

// SAFETY: the handle is only used to call `CFRunLoopStop` and to release the reference. Apple
// documents both as safe from any thread; the rest of CFRunLoop's API stays on the tap thread.
unsafe impl Send for RunLoopHandle {}

/// The thread body: installs the tap, reports the result, then serves events until the flag is set.
/// The tap is removed when this function returns.
fn run(state: Arc<Mutex<PointerState>>, stop: Arc<AtomicBool>, ready: mpsc::Sender<Result<RunLoopHandle, PortError>>) {
    let tap = match Tap::open(state) {
        Ok(tap) => tap,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(RunLoopHandle(tap.run_loop.clone())));
    while !stop.load(Ordering::Acquire) {
        tap.turn();
    }
}

/// An installed event tap. Dropping it switches the tap off and takes it out of the run loop.
struct Tap {
    port: CFRetained<CFMachPort>,
    source: CFRetained<CFRunLoopSource>,
    run_loop: CFRetained<CFRunLoop>,
    // The callback reads this through a raw pointer, so it must outlive the tap (fields drop last).
    _state: Arc<Mutex<PointerState>>,
}

impl Tap {
    /// Creates the tap and adds it to the current thread's run loop.
    fn open(state: Arc<Mutex<PointerState>>) -> Result<Self, PortError> {
        let run_loop = CFRunLoop::current().ok_or_else(|| PortError::Failed("the highlight thread has no run loop".into()))?;
        let info = Arc::as_ptr(&state).cast_mut().cast::<c_void>();
        // SAFETY: `on_event` has the callback's signature. `info` points into `state`, which the
        // returned `Tap` keeps alive until the tap is invalidated in `drop`.
        let port = unsafe {
            CGEvent::tap_create(
                CGEventTapLocation::SessionEventTap,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::ListenOnly,
                event_mask(),
                Some(on_event),
                info,
            )
        }
        .ok_or_else(|| PortError::Failed("macOS did not create the pointer event tap (is Input Monitoring granted?)".into()))?;
        let Some(source) = CFMachPort::new_run_loop_source(None, Some(&port), 0) else {
            port.invalidate();
            return Err(PortError::Failed("could not make a run loop source for the pointer event tap".into()));
        };
        // SAFETY: reads a constant CFString that CoreFoundation initialises when it loads.
        let modes = unsafe { kCFRunLoopCommonModes };
        run_loop.add_source(Some(&source), modes);
        CGEvent::tap_enable(&port, true);
        Ok(Self { port, source, run_loop, _state: state })
    }

    /// Serves events for up to `POLL`, then turns the tap back on if macOS switched it off (it does
    /// when a callback is too slow or the user's input is disturbed).
    fn turn(&self) {
        // SAFETY: reads a constant CFString that CoreFoundation initialises when it loads.
        let mode = unsafe { kCFRunLoopDefaultMode };
        if CFRunLoop::run_in_mode(mode, POLL.as_secs_f64(), false) == CFRunLoopRunResult::Finished {
            // Nothing to wait on (the source is gone); sleep instead of spinning.
            std::thread::sleep(POLL);
        }
        if !CGEvent::tap_is_enabled(&self.port) {
            CGEvent::tap_enable(&self.port, true);
        }
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        CGEvent::tap_enable(&self.port, false);
        // SAFETY: reads a constant CFString that CoreFoundation initialises when it loads.
        let modes = unsafe { kCFRunLoopCommonModes };
        self.run_loop.remove_source(Some(&self.source), modes);
        self.source.invalidate();
        // Invalidating the port is what removes the tap; no callback runs after this.
        self.port.invalidate();
    }
}

/// The tap callback. It records the event and returns it untouched (a listen-only tap ignores the
/// return value). It must not panic: the frames above it are C.
unsafe extern "C-unwind" fn on_event(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    info: *mut c_void,
) -> *mut CGEvent {
    // macOS also sends "tap disabled" notices here; `turn` re-enables the tap, so they are ignored.
    if WATCHED.contains(&kind) {
        // SAFETY: `info` is the `Mutex` inside the `Arc` that `Tap` keeps alive until after the tap
        // is invalidated, and the event is valid for the duration of the callback.
        let (shared, event_ref) = unsafe { (&*info.cast::<Mutex<PointerState>>(), event.as_ref()) };
        let at = CGEvent::location(Some(event_ref));
        shared.lock().unwrap_or_else(|p| p.into_inner()).apply(kind, at.x, at.y);
        crate::disc::follow(at.x, at.y);
    }
    event.as_ptr()
}

#[cfg(test)]
mod tests {
    use objc2_core_graphics::CGEventType;

    use super::{PointerState, event_mask};

    #[test]
    fn the_mask_has_one_bit_per_watched_event_type() {
        // Types 1 to 7: left down/up, right down/up, moved, left dragged, right dragged.
        assert_eq!(event_mask(), 0xFE);
    }

    #[test]
    fn buttons_follow_presses_and_releases_and_every_event_moves_the_pointer() {
        let mut state = PointerState::default();
        state.apply(CGEventType::LeftMouseDown, 10.0, 20.0);
        assert_eq!(state, PointerState { x: 10.0, y: 20.0, left_down: true, right_down: false, events: 1 });
        state.apply(CGEventType::RightMouseDown, 11.0, 21.0);
        state.apply(CGEventType::LeftMouseUp, 12.0, 22.0);
        assert_eq!(state, PointerState { x: 12.0, y: 22.0, left_down: false, right_down: true, events: 3 });
        state.apply(CGEventType::MouseMoved, 5.0, 6.0);
        state.apply(CGEventType::RightMouseUp, 5.0, 6.0);
        assert_eq!(state, PointerState { x: 5.0, y: 6.0, left_down: false, right_down: false, events: 5 });
    }
}
