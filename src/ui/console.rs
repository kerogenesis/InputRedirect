//! Talking to the Windows console itself.
//!
//! Three things have to be arranged before anything is drawn: the code page,
//! so a check mark does not arrive as a question mark; escape sequence
//! processing, so colours work at all; and the cursor, which is shown only
//! where the user is expected to answer. All of them are put back the way they
//! were when the program leaves.

use std::io::{stdout, Write};
use std::sync::OnceLock;

use windows::core::BOOL;
use windows::Win32::System::Console::{
    FlushConsoleInputBuffer, GetConsoleCursorInfo, GetConsoleMode, GetConsoleOutputCP,
    GetConsoleScreenBufferInfo, GetStdHandle, SetConsoleCursorInfo, SetConsoleCursorPosition,
    SetConsoleMode, SetConsoleOutputCP, CONSOLE_CURSOR_INFO, CONSOLE_MODE,
    CONSOLE_SCREEN_BUFFER_INFO, COORD, ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE,
};

const UTF8: u32 = 65001;

/// The console attributes `claim_console` changes, remembered so the console is
/// handed back exactly as it was found. Each is optional because the call that
/// reads it can fail, and what could not be read must not be written back.
struct Original {
    code_page: u32,
    mode: Option<CONSOLE_MODE>,
    cursor_visible: Option<bool>,
}

static ORIGINAL: OnceLock<Original> = OnceLock::new();

/// Prepares the console for drawing. Called once, at startup.
pub fn claim_console() {
    let mut original = Original {
        code_page: UTF8,
        mode: None,
        cursor_visible: None,
    };

    // SAFETY: every call below only reads or writes console attributes of the
    // handles this process already owns.
    unsafe {
        original.code_page = GetConsoleOutputCP();
        let _ = SetConsoleOutputCP(UTF8);

        if let Ok(output) = GetStdHandle(STD_OUTPUT_HANDLE) {
            let mut mode = CONSOLE_MODE::default();
            if GetConsoleMode(output, &mut mode).is_ok() {
                original.mode = Some(mode);
                let _ = SetConsoleMode(output, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }

            let mut info = CONSOLE_CURSOR_INFO::default();
            if GetConsoleCursorInfo(output, &mut info).is_ok() {
                original.cursor_visible = Some(info.bVisible.as_bool());
            }
        }
    }

    let _ = ORIGINAL.set(original);

    show_cursor(false);
}

/// Gives the console back to whoever started the program, in the state it was
/// found: the code page, the mode and the cursor are each put back, so running
/// from an existing terminal leaves no trace of the program having drawn to it.
pub fn release_console() {
    write_text("\x1b[0m\r\n");

    let original = ORIGINAL.get();

    // Visible is the right default for a cursor whose old state could not be
    // read - it is what the console shows before anything hides it.
    show_cursor(
        original
            .and_then(|original| original.cursor_visible)
            .unwrap_or(true),
    );

    let Some(original) = original else {
        return;
    };

    // SAFETY: the attributes written back are the ones read at startup from the
    // handles this process owns.
    unsafe {
        if let Some(mode) = original.mode {
            if let Ok(output) = GetStdHandle(STD_OUTPUT_HANDLE) {
                let _ = SetConsoleMode(output, mode);
            }
        }
        let _ = SetConsoleOutputCP(original.code_page);
    }
}

/// Writes a whole frame in one call, starting at the top of the window.
///
/// One write instead of one per line is the difference between a screen that
/// appears and a screen that visibly builds itself up.
pub(super) fn write_frame(frame: &str) {
    move_to_top();
    write_text(frame);
}

/// Writes text where the cursor currently is. Used while the program is still
/// starting up and the lines are meant to scroll past.
pub(super) fn write_text(text: &str) {
    let mut output = stdout().lock();
    let _ = output.write_all(text.as_bytes());
    let _ = output.flush();
}

/// Throws away key presses that arrived while the program was busy.
///
/// Without this a key held down for a moment queues up repeats, and the menu
/// then works through them long after the key was released.
pub(super) fn flush_input() {
    // SAFETY: the standard input handle belongs to this process.
    unsafe {
        if let Ok(input) = GetStdHandle(STD_INPUT_HANDLE) {
            let _ = FlushConsoleInputBuffer(input);
        }
    }
}

/// The cursor is hidden while a frame is drawn and shown again at the place
/// where the user is supposed to type.
pub(super) fn show_cursor(visible: bool) {
    // SAFETY: the cursor info is read back before it is written, so only the
    // visibility flag changes.
    unsafe {
        let Ok(output) = GetStdHandle(STD_OUTPUT_HANDLE) else {
            return;
        };

        let mut info = CONSOLE_CURSOR_INFO::default();
        if GetConsoleCursorInfo(output, &mut info).is_err() {
            return;
        }

        info.bVisible = BOOL::from(visible);
        let _ = SetConsoleCursorInfo(output, &info);
    }
}

/// Puts the cursor at the top of the visible window, so the next frame
/// overwrites the previous one. Whatever scrolled past before the screen took
/// over stays in the scrollback.
fn move_to_top() {
    // SAFETY: the handle belongs to this process, and the info structure is
    // filled in by Windows before it is read.
    unsafe {
        let Ok(output) = GetStdHandle(STD_OUTPUT_HANDLE) else {
            return;
        };

        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        if GetConsoleScreenBufferInfo(output, &mut info).is_err() {
            return;
        }

        let top = COORD {
            X: 0,
            Y: info.srWindow.Top,
        };
        let _ = SetConsoleCursorPosition(output, top);
    }
}
