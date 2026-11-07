//! Synthetic input for scrolling capture on Windows (M6 D4): `SetCursorPos` to the region's point,
//! then wheel notches or arrow and page keys through `SendInput`, or `WM_VSCROLL` to the window
//! under the point. The event plan is plain data, so its tests run on every target; the Win32 calls
//! and the `INPUT` records are Windows-only.

use laqta_domain::capture::ScrollMethod;

#[cfg(windows)]
use laqta_ports::{InputSynth, PortError};
#[cfg(windows)]
use windows::Win32::Foundation::{LPARAM, POINT, WPARAM};
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
    VIRTUAL_KEY,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{SB_LINEDOWN, SendMessageW, SetCursorPos, WM_VSCROLL, WindowFromPoint};

/// `mouseData` of one wheel notch down: `WHEEL_DELTA`, negated.
const WHEEL_DOWN: i32 = -120;
/// Virtual-key codes of Page Down (`VK_NEXT`) and Down (`VK_DOWN`).
const VK_NEXT: u16 = 0x22;
const VK_DOWN: u16 = 0x28;

/// One `INPUT` record a scroll step sends, as plain data. Each becomes one `INPUT` on Windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Record {
    /// A `MOUSEEVENTF_WHEEL` notch whose `mouseData` is `delta`.
    Wheel { delta: i32 },
    /// A key event for virtual key `vk`: a press, or a release (`KEYEVENTF_KEYUP`) when `up`.
    Key { vk: u16, up: bool },
}

/// The records that scroll the window under the point by `amount` steps, in order. `ScrollMessage`
/// sends a message per step instead of input, so it has no records.
pub(crate) fn scroll_records(method: ScrollMethod, amount: u32) -> Vec<Record> {
    let step = match method {
        ScrollMethod::MouseWheel => vec![Record::Wheel { delta: WHEEL_DOWN }],
        ScrollMethod::DownArrow => key_press(VK_DOWN),
        ScrollMethod::PageDown => key_press(VK_NEXT),
        ScrollMethod::ScrollMessage => return Vec::new(),
    };
    let mut records = Vec::new();
    for _ in 0..amount {
        records.extend_from_slice(&step);
    }
    records
}

/// A key press followed by its release.
fn key_press(vk: u16) -> Vec<Record> {
    vec![Record::Key { vk, up: false }, Record::Key { vk, up: true }]
}

/// Synthetic input through `SendInput` and window messages.
#[cfg(windows)]
#[derive(Debug)]
pub struct WindowsInput;

#[cfg(windows)]
impl InputSynth for WindowsInput {
    fn scroll(&self, x: i32, y: i32, method: ScrollMethod, amount: u32) -> Result<(), PortError> {
        // SAFETY: a plain call with two integers; it takes no pointers.
        unsafe { SetCursorPos(x, y) }.map_err(|e| PortError::Failed(format!("SetCursorPos: {e}")))?;
        match method {
            ScrollMethod::ScrollMessage => scroll_message(x, y, amount),
            ScrollMethod::MouseWheel | ScrollMethod::DownArrow | ScrollMethod::PageDown => send(&scroll_records(method, amount)),
        }
    }
}

/// `WM_VSCROLL` with `SB_LINEDOWN` to the window under the point, once per step.
#[cfg(windows)]
fn scroll_message(x: i32, y: i32, amount: u32) -> Result<(), PortError> {
    // SAFETY: a hit test on a point. A null handle means no window is there.
    let hwnd = unsafe { WindowFromPoint(POINT { x, y }) };
    if hwnd.is_invalid() {
        return Err(PortError::Failed(format!("no window under ({x}, {y})")));
    }
    for _ in 0..amount {
        // SAFETY: `hwnd` is the handle the hit test just returned. A window closed since then ignores the message.
        unsafe { SendMessageW(hwnd, WM_VSCROLL, Some(WPARAM(SB_LINEDOWN.0 as usize)), Some(LPARAM(0))) };
    }
    Ok(())
}

/// Sends the records as one `SendInput` batch. Fewer events inserted than sent means input is blocked
/// (UIPI, or a secure desktop).
#[cfg(windows)]
fn send(records: &[Record]) -> Result<(), PortError> {
    let inputs: Vec<INPUT> = records.iter().copied().map(to_input).collect();
    // SAFETY: `inputs` is a live slice of `INPUT` records, and the size passed is `INPUT`'s own.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(PortError::Failed(format!("SendInput inserted {sent} of {} events", inputs.len())))
    }
}

/// The Win32 `INPUT` record for `record`.
#[cfg(windows)]
fn to_input(record: Record) -> INPUT {
    match record {
        Record::Wheel { delta } => INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: delta as u32, dwFlags: MOUSEEVENTF_WHEEL, time: 0, dwExtraInfo: 0 },
            },
        },
        Record::Key { vk, up } => INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use laqta_domain::capture::ScrollMethod;

    use super::{Record, scroll_records};

    #[test]
    fn wheel_steps_send_one_downward_notch_each() {
        assert_eq!(scroll_records(ScrollMethod::MouseWheel, 3), vec![Record::Wheel { delta: -120 }; 3]);
    }

    #[test]
    fn key_steps_press_then_release_the_key_each_time() {
        let (press, release) = (Record::Key { vk: 0x28, up: false }, Record::Key { vk: 0x28, up: true });
        assert_eq!(scroll_records(ScrollMethod::DownArrow, 2), vec![press, release, press, release]);
    }

    #[test]
    fn page_down_presses_vk_next() {
        let (press, release) = (Record::Key { vk: 0x22, up: false }, Record::Key { vk: 0x22, up: true });
        assert_eq!(scroll_records(ScrollMethod::PageDown, 1), vec![press, release]);
    }

    #[test]
    fn zero_steps_and_scroll_messages_send_no_input_records() {
        assert!(scroll_records(ScrollMethod::MouseWheel, 0).is_empty());
        assert!(scroll_records(ScrollMethod::ScrollMessage, 5).is_empty());
    }
}

#[cfg(all(windows, test))]
mod win_tests {
    use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYEVENTF_KEYUP, MOUSEEVENTF_WHEEL};

    use super::{Record, to_input};

    #[test]
    fn input_record_has_the_win32_size() {
        let expected = if cfg!(target_pointer_width = "64") { 40 } else { 28 };
        assert_eq!(std::mem::size_of::<INPUT>(), expected);
    }

    #[test]
    fn wheel_record_is_a_mouse_input_with_the_negative_delta() {
        let input = to_input(Record::Wheel { delta: -120 });
        assert_eq!(input.r#type.0, INPUT_MOUSE.0);
        // SAFETY: a mouse record, so the `mi` member is the one set.
        let mi = unsafe { input.Anonymous.mi };
        assert_eq!(mi.dwFlags.0, MOUSEEVENTF_WHEEL.0);
        assert_eq!(mi.mouseData, (-120i32) as u32);
    }

    #[test]
    fn key_release_sets_keyeventf_keyup_and_press_does_not() {
        let press = to_input(Record::Key { vk: 0x28, up: false });
        let release = to_input(Record::Key { vk: 0x28, up: true });
        assert_eq!(press.r#type.0, INPUT_KEYBOARD.0);
        // SAFETY: keyboard records, so the `ki` member is the one set.
        let (p, r) = unsafe { (press.Anonymous.ki, release.Anonymous.ki) };
        assert_eq!(p.wVk.0, 0x28);
        assert_eq!(p.dwFlags.0, 0);
        assert_eq!(r.dwFlags.0, KEYEVENTF_KEYUP.0);
    }
}
