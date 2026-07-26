//! The low-level hooks and the thread that owns them.
//!
//! Windows delivers hook callbacks on the thread that installed them, and only
//! while that thread pumps messages - so the hooks get a thread of their own,
//! and the user interface keeps its own.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, Sender};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VIRTUAL_KEY, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL,
    VK_RMENU, VK_RSHIFT, VK_RWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_QUIT,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_XBUTTONDOWN, WM_XBUTTONUP, XBUTTON1,
};

use crate::error::{Error, Result};
use crate::hid::{Modifiers, MouseButtons};

use super::{on_button, on_key, Decision};

/// One entry per bit in [`Modifiers`], by the key that stands for that side.
///
/// The sided virtual keys are the only ones that will do: the merged `VK_CONTROL`
/// and friends answer for either side at once, and could never clear just one.
const SIDED_MODIFIERS: [(VIRTUAL_KEY, Modifiers); 8] = [
    (VK_LCONTROL, Modifiers::LEFT_CTRL),
    (VK_LSHIFT, Modifiers::LEFT_SHIFT),
    (VK_LMENU, Modifiers::LEFT_ALT),
    (VK_LWIN, Modifiers::LEFT_GUI),
    (VK_RCONTROL, Modifiers::RIGHT_CTRL),
    (VK_RSHIFT, Modifiers::RIGHT_SHIFT),
    (VK_RMENU, Modifiers::RIGHT_ALT),
    (VK_RWIN, Modifiers::RIGHT_GUI),
];

/// The modifiers Windows itself reports as held right now.
///
/// Asked only when the combo watcher believes a modifier is down, so the path
/// every ordinary keystroke takes never reaches it.
///
/// The async key state is the right thing to ask: it belongs to the session
/// rather than to a window, so it is still right after the keyboard has been
/// somewhere this hook cannot follow. `GetKeyboardState` would not do - it
/// answers for the calling thread's input queue, and the hook thread is not
/// attached to the one the user is typing into.
pub fn live_modifiers() -> Modifiers {
    let mut held = Modifiers::empty();

    for (key, modifier) in SIDED_MODIFIERS {
        // SAFETY: the call takes a virtual key code and no pointer or handle.
        let state = unsafe { GetAsyncKeyState(i32::from(key.0)) };

        // The high bit is "down now"; the low one only means "pressed since
        // this was last asked", which is a different question.
        if state as u16 & 0x8000 != 0 {
            held.insert(modifier);
        }
    }

    held
}

/// A key as the hook reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub scan_code: u16,
    pub extended: bool,
    pub pressed: bool,
}

/// A mouse button as the hook reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonEvent {
    pub button: MouseButtons,
    pub pressed: bool,
}

/// Owns the hook thread and takes the hooks down when dropped.
pub struct HookThread {
    thread_id: u32,
    handle: Option<JoinHandle<()>>,
}

impl HookThread {
    pub fn spawn() -> Result<Self> {
        let (ready, started) = channel();

        let handle = std::thread::Builder::new()
            .name("input-hooks".to_owned())
            .spawn(move || run(&ready))
            .map_err(|error| Error::Hook(error.to_string()))?;

        match started.recv() {
            Ok(Some(thread_id)) => Ok(Self {
                thread_id,
                handle: Some(handle),
            }),
            _ => Err(Error::Hook(
                "Windows did not accept the input hooks".to_owned(),
            )),
        }
    }

    /// Asks the hook thread to unhook and stop, and waits for it.
    pub fn stop(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };

        // SAFETY: the thread id belongs to the thread we started; the worst a
        // stale id can do is deliver a quit message nobody is waiting for.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }

        let _ = handle.join();
    }
}

impl Drop for HookThread {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(ready: &Sender<Option<u32>>) {
    // SAFETY: the hooks are installed and removed on this thread, and the
    // message loop between the two calls is what keeps them alive.
    unsafe {
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0);
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0);

        let (Ok(keyboard), Ok(mouse)) = (keyboard, mouse) else {
            let _ = ready.send(None);
            return;
        };

        let _ = ready.send(Some(GetCurrentThreadId()));

        let mut message = MSG::default();
        loop {
            // Three answers, not two: a positive number is a message, zero is
            // the quit request, and -1 is a failure. Reading -1 as "carry on"
            // is what turns a broken loop into a busy one.
            let waiting = GetMessageW(&mut message, None, 0, 0).0;
            if waiting <= 0 {
                break;
            }

            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        let _ = UnhookWindowsHookEx(keyboard);
        let _ = UnhookWindowsHookEx(mouse);
    }
}

const SWALLOWED: LRESULT = LRESULT(1);

/// Keeps a panic on our side of the boundary.
///
/// Unwinding out of an `extern "system"` function aborts the process, and an
/// abort skips every destructor - which is precisely how a key ends up held
/// down forever. Letting the event through is always the safe answer.
fn decided_safely(decide: impl FnOnce() -> Decision) -> Decision {
    catch_unwind(AssertUnwindSafe(decide)).unwrap_or(Decision::PassThrough)
}

unsafe extern "system" fn keyboard_hook(code: i32, event: WPARAM, data: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, event, data) };
    }

    // SAFETY: for code >= 0 Windows guarantees lparam points at a KBDLLHOOKSTRUCT.
    let info = unsafe { &*(data.0 as *const KBDLLHOOKSTRUCT) };
    let message = event.0 as u32;

    let key = KeyEvent {
        scan_code: info.scanCode as u16,
        extended: info.flags.0 & 0x01 != 0,
        pressed: message == WM_KEYDOWN || message == WM_SYSKEYDOWN,
    };

    if decided_safely(|| on_key(key)) == Decision::Swallow {
        return SWALLOWED;
    }

    unsafe { CallNextHookEx(None, code, event, data) }
}

unsafe extern "system" fn mouse_hook(code: i32, event: WPARAM, data: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, event, data) };
    }

    // SAFETY: for code >= 0 Windows guarantees lparam points at a MSLLHOOKSTRUCT.
    let info = unsafe { &*(data.0 as *const MSLLHOOKSTRUCT) };

    if let Some(button) = button_of(event.0 as u32, info.mouseData) {
        if decided_safely(|| on_button(button)) == Decision::Swallow {
            return SWALLOWED;
        }
    }

    unsafe { CallNextHookEx(None, code, event, data) }
}

/// Pointer movement and wheel messages are deliberately not translated: they
/// already work, and repeating them would only add lag.
fn button_of(message: u32, extra: u32) -> Option<ButtonEvent> {
    let (button, pressed) = match message {
        WM_LBUTTONDOWN => (MouseButtons::LEFT, true),
        WM_LBUTTONUP => (MouseButtons::LEFT, false),
        WM_RBUTTONDOWN => (MouseButtons::RIGHT, true),
        WM_RBUTTONUP => (MouseButtons::RIGHT, false),
        WM_MBUTTONDOWN => (MouseButtons::MIDDLE, true),
        WM_MBUTTONUP => (MouseButtons::MIDDLE, false),
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let button = if (extra >> 16) as u16 == XBUTTON1 {
                MouseButtons::BACK
            } else {
                MouseButtons::FORWARD
            };
            (button, message == WM_XBUTTONDOWN)
        }
        _ => return None,
    };

    Some(ButtonEvent { button, pressed })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_button_message_maps_to_its_button_and_direction() {
        let cases = [
            (WM_LBUTTONDOWN, MouseButtons::LEFT, true),
            (WM_LBUTTONUP, MouseButtons::LEFT, false),
            (WM_RBUTTONDOWN, MouseButtons::RIGHT, true),
            (WM_RBUTTONUP, MouseButtons::RIGHT, false),
            (WM_MBUTTONDOWN, MouseButtons::MIDDLE, true),
            (WM_MBUTTONUP, MouseButtons::MIDDLE, false),
        ];

        for (message, button, pressed) in cases {
            assert_eq!(button_of(message, 0), Some(ButtonEvent { button, pressed }));
        }
    }

    #[test]
    fn the_two_side_buttons_are_told_apart_by_the_extra_word() {
        let back = button_of(WM_XBUTTONDOWN, u32::from(XBUTTON1) << 16);
        let forward = button_of(WM_XBUTTONDOWN, 2 << 16);

        assert_eq!(back.unwrap().button, MouseButtons::BACK);
        assert_eq!(forward.unwrap().button, MouseButtons::FORWARD);
    }

    #[test]
    fn movement_and_wheel_are_not_treated_as_buttons() {
        assert_eq!(button_of(0x0200, 0), None); // WM_MOUSEMOVE
        assert_eq!(button_of(0x020A, 0), None); // WM_MOUSEWHEEL
    }

    #[test]
    fn a_callback_that_panics_lets_the_event_through() {
        let decision = decided_safely(|| panic!("a bug in the decision"));

        assert_eq!(decision, Decision::PassThrough);
    }

    #[test]
    fn a_callback_that_works_keeps_its_answer() {
        assert_eq!(decided_safely(|| Decision::Swallow), Decision::Swallow);
    }
}
