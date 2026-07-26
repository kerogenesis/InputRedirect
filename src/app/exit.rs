//! Leaving the machine the way it was found, however the program ends.
//!
//! Quitting from the menu unwinds properly and every destructor runs. Closing
//! the window does not: Windows sends a control event and then terminates the
//! process, so the cleanup has to happen inside the handler, in the few
//! seconds granted before that.
//!
//! Which control event it is decides what the handler answers, and the two
//! cases are opposites. After a close, a logoff or a shutdown the process is
//! taken away whatever we say. After Ctrl+C or Ctrl+Break it is not: there the
//! answer is what decides whether the process ends at all.

use std::panic::catch_unwind;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::BOOL;
use windows::Win32::System::Console::{
    SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT,
    CTRL_SHUTDOWN_EVENT,
};

use crate::driver;
use crate::redirect;
use crate::ui;

/// Both ways out lead to the same cleanup, and it is worth doing only once.
static ALREADY_CLEANED_UP: AtomicBool = AtomicBool::new(false);

/// Asks Windows to tell us before the process is taken away.
pub fn watch_for_close() {
    // SAFETY: the handler is a plain function with the signature Windows
    // expects, and it stays valid for the whole life of the process.
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(on_console_event), true);
    }
}

/// Switches the redirects off, parks the virtual devices and gives the console
/// back. Safe to call from any thread and any number of times.
pub fn clean_up() {
    if ALREADY_CLEANED_UP.swap(true, Ordering::SeqCst) {
        return;
    }

    // Said first, and to the driver rather than to the engine: a pair of
    // virtual devices takes the better part of a second to create, and one
    // finished after the sweep below has looked would be left on the bus with
    // nobody to take it away.
    driver::begin_shutdown();

    redirect::emergency_stop();
    ui::release_console();
}

/// Windows calls this on a thread of its own while the window is closing.
///
/// The answer is not the same for every event. Windows ends the process itself
/// after a close, a logoff or a shutdown, so saying "handled" there only means
/// "the cleanup is done". Ctrl+C and Ctrl+Break are different: saying "handled"
/// is exactly what stops the default handler from ending the process, and the
/// program would then carry on with its devices unplugged and its driver given
/// away - showing a menu that no longer does anything, and refusing to stop.
unsafe extern "system" fn on_console_event(event: u32) -> BOOL {
    let ends_the_process = matches!(
        event,
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT
    );
    let interrupts = matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT);

    if !(ends_the_process || interrupts) {
        return BOOL::from(false);
    }

    // A panic here would abort the process on the spot and undo the very thing
    // this handler exists for.
    let _ = catch_unwind(clean_up);

    // False for an interrupt, so the default handler takes the process away as
    // the user asked. Everything of ours is already back the way it was found.
    BOOL::from(ends_the_process)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cleanup_only_happens_once() {
        ALREADY_CLEANED_UP.store(false, Ordering::SeqCst);

        assert!(!ALREADY_CLEANED_UP.swap(true, Ordering::SeqCst));
        assert!(ALREADY_CLEANED_UP.swap(true, Ordering::SeqCst));
    }
}
