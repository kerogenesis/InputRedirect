//! Reading the menu keys.
//!
//! The console is read through the raw input records rather than through a
//! character: the character depends on the active keyboard layout, the scan
//! code describes the key itself, so the menu works in any layout.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Console::{
    GetConsoleProcessList, GetStdHandle, ReadConsoleInputW, INPUT_RECORD, KEY_EVENT,
    STD_INPUT_HANDLE,
};
use windows::Win32::System::Threading::{WaitForSingleObject, INFINITE};

use super::console;

/// What the user asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    ToggleMouse,
    ToggleKeyboard,
    StopEverything,
    RecreateDevices,
    RemoveDriver,
    Quit,
}

/// The outcome of waiting at the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKey {
    Chosen(Command),
    /// A key the menu has no use for.
    Unknown,
    /// Nothing was pressed in time; the caller redraws and waits again, which
    /// is what keeps the counters on screen moving.
    Tick,
}

/// How long the menu waits before refreshing the screen on its own.
pub const TICK_MS: u32 = 500;

// A tick of zero would spin the processor, and a slow one would leave the
// counters looking frozen.
const _: () = assert!(TICK_MS > 0 && TICK_MS <= 1000);

/// Waits for a menu key, or gives up after `timeout_ms` and reports a tick.
pub fn wait_for_command(timeout_ms: u32) -> MenuKey {
    match next_key_down(timeout_ms) {
        KeyPress::None => MenuKey::Tick,
        // No console left to read from: carrying on would spin forever.
        KeyPress::Closed => MenuKey::Chosen(Command::Quit),
        KeyPress::ScanCode(scan_code) => match command_for(scan_code) {
            Some(command) => MenuKey::Chosen(command),
            None => MenuKey::Unknown,
        },
    }
}

/// Asks a yes or no question. Anything that is not "yes" means no.
pub fn confirm() -> bool {
    console::flush_input();

    loop {
        match next_key_down(INFINITE) {
            // y
            KeyPress::ScanCode(0x15) => return true,
            // n, escape
            KeyPress::ScanCode(0x31 | 0x01) | KeyPress::Closed => return false,
            _ => {}
        }
    }
}

/// Waits for anything at all, used before a window closes for good.
pub fn wait_for_any_key() {
    console::flush_input();

    loop {
        match next_key_down(INFINITE) {
            KeyPress::None => {}
            KeyPress::ScanCode(_) | KeyPress::Closed => return,
        }
    }
}

/// Drops whatever was typed while the program was busy elsewhere.
pub fn discard_pending_keys() {
    console::flush_input();
}

/// Holds a window open that is about to take its last message with it.
///
/// Started from Explorer the program gets a console window of its own, and that
/// window dies with the process. Started from a shell that was already there,
/// the window outlives us and stopping to ask for a key would only be in the
/// way - so this asks who else is using the console first.
pub fn wait_before_the_window_closes() {
    if !owns_the_window() {
        return;
    }

    println!("\nPress any key to close this window.");
    wait_for_any_key();
}

/// Whether this program is the only thing attached to the console.
fn owns_the_window() -> bool {
    // Room for two: the answer only has to tell "just us" from "somebody else
    // as well".
    let mut attached = [0u32; 2];

    // SAFETY: the list is described by the length of the buffer it is given.
    let sharing = unsafe { GetConsoleProcessList(&mut attached) };

    sharing == 1
}

/// Menu keys by their physical position, including the numeric pad.
fn command_for(scan_code: u16) -> Option<Command> {
    let command = match scan_code {
        0x02 | 0x4F => Command::ToggleMouse,     // 1
        0x03 | 0x50 => Command::ToggleKeyboard,  // 2
        0x04 | 0x51 => Command::StopEverything,  // 3
        0x05 | 0x4B => Command::RecreateDevices, // 4
        0x20 => Command::RemoveDriver,           // d
        0x10 | 0x01 => Command::Quit,            // q, escape
        _ => return None,
    };

    Some(command)
}

enum KeyPress {
    ScanCode(u16),
    /// The wait ran out before anything was pressed.
    None,
    /// The console cannot be read any more.
    Closed,
}

/// Waits for one key to go down, ignoring everything else the console reports.
///
/// The deadline is worked out once. Waiting `timeout_ms` again after every
/// record we have no use for would let a mouse moving over the window put the
/// tick off for as long as it kept moving, and the counters would stop.
fn next_key_down(timeout_ms: u32) -> KeyPress {
    let deadline = (timeout_ms != INFINITE)
        .then(|| Instant::now() + Duration::from_millis(u64::from(timeout_ms)));

    // SAFETY: the standard input handle is owned by the process.
    unsafe {
        let Ok(input) = GetStdHandle(STD_INPUT_HANDLE) else {
            return KeyPress::Closed;
        };

        loop {
            let left = match deadline {
                None => INFINITE,
                Some(deadline) => match deadline.checked_duration_since(Instant::now()) {
                    None => return KeyPress::None,
                    Some(left) => milliseconds_of(left),
                },
            };

            let wait = WaitForSingleObject(input, left);
            if wait == WAIT_TIMEOUT {
                return KeyPress::None;
            }
            if wait != WAIT_OBJECT_0 {
                return KeyPress::Closed;
            }

            match read_key_down(input) {
                Ok(Some(scan_code)) => return KeyPress::ScanCode(scan_code),
                // A mouse move or a resize: wait out what is left.
                Ok(None) => {}
                Err(()) => return KeyPress::Closed,
            }
        }
    }
}

/// A wait in milliseconds, kept below the value that means "forever".
fn milliseconds_of(left: Duration) -> u32 {
    u32::try_from(left.as_millis())
        .unwrap_or(u32::MAX)
        .min(INFINITE - 1)
}

/// Reads the one event the console is holding.
unsafe fn read_key_down(input: HANDLE) -> Result<Option<u16>, ()> {
    let mut record = [INPUT_RECORD::default(); 1];
    let mut read = 0;

    // SAFETY: the caller owns the handle and the buffer outlives the call.
    unsafe {
        if ReadConsoleInputW(input, &mut record, &mut read).is_err() || read == 0 {
            return Err(());
        }

        if record[0].EventType != KEY_EVENT as u16 {
            return Ok(None);
        }

        let key = record[0].Event.KeyEvent;
        Ok(key.bKeyDown.as_bool().then_some(key.wVirtualScanCode))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_answers_to_the_keys_printed_next_to_it() {
        assert_eq!(command_for(0x02), Some(Command::ToggleMouse));
        assert_eq!(command_for(0x03), Some(Command::ToggleKeyboard));
        assert_eq!(command_for(0x04), Some(Command::StopEverything));
        assert_eq!(command_for(0x05), Some(Command::RecreateDevices));
        assert_eq!(command_for(0x20), Some(Command::RemoveDriver));
        assert_eq!(command_for(0x10), Some(Command::Quit));
    }

    #[test]
    fn the_numeric_pad_works_as_well_as_the_number_row() {
        assert_eq!(command_for(0x4F), Some(Command::ToggleMouse));
        assert_eq!(command_for(0x50), Some(Command::ToggleKeyboard));
        assert_eq!(command_for(0x51), Some(Command::StopEverything));
        assert_eq!(command_for(0x4B), Some(Command::RecreateDevices));
    }

    #[test]
    fn escape_quits_like_the_letter_does() {
        assert_eq!(command_for(0x01), Some(Command::Quit));
    }

    #[test]
    fn keys_that_mean_nothing_here_are_ignored() {
        for scan_code in [0x1E, 0x39, 0x3B, 0x00] {
            assert_eq!(command_for(scan_code), None, "scan code {scan_code:#04X}");
        }
    }

    /// What is left of the tick is passed on as it is.
    #[test]
    fn the_time_left_is_asked_for_in_milliseconds() {
        assert_eq!(milliseconds_of(Duration::from_millis(0)), 0);
        assert_eq!(
            milliseconds_of(Duration::from_millis(u64::from(TICK_MS))),
            TICK_MS
        );
    }

    /// A wait must never come out as INFINITE by accident: that is the one
    /// value that means the tick never arrives.
    #[test]
    fn a_wait_longer_than_windows_can_count_is_not_mistaken_for_forever() {
        assert!(milliseconds_of(Duration::from_secs(60 * 60 * 24 * 365)) < INFINITE);
        assert!(milliseconds_of(Duration::MAX) < INFINITE);
    }
}
