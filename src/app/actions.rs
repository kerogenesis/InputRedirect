//! What each menu entry does.
//!
//! The sentences live at the top rather than inside the calls: they read as a
//! whole that way, and they are the part most likely to be reworded.

use std::sync::Arc;

use crate::driver::{self, Step};
use crate::ui::{self, Tone};

use super::{App, Outcome};

const MOUSE_ON: &str = "Your clicks now go through the Logitech driver";
const MOUSE_OFF: &str = "Mouse buttons work the normal way again";
const KEYBOARD_ON: &str = "Your typing now goes through the Logitech driver";
const KEYBOARD_OFF: &str = "The keyboard works the normal way again";
const NOTHING_REDIRECTED: &str = "Nothing is being redirected";
const STOPPED: &str = "Stopped, your devices work the normal way again";

const RECREATING: &str = "Re-creating the virtual devices";
const DEVICES_BACK: &str = "Devices re-created";
const DEVICES_RESUMED: &str = "Devices re-created, redirection resumed";

const ABOUT_TO_REMOVE: &str = "About to remove the Logitech driver from this computer";
const REMOVE_QUESTION: &str = "Remove the driver?";
const REMOVE_ANSWERS: &str = "Y = yes, N = no";
const NOTHING_REMOVED: &str = "Nothing was removed";
const REMOVING: &str = "Removing the driver";
const REMOVED: &str = "The driver has been removed from this computer";
const STILL_IN_USE: &str = "The driver is still in use";
const START_AGAIN: &str = "Start the program again and remove the driver right away.";
const SESSION_ENDS: &str = "Redirection is off and this session has to end.";
const TRY_AGAIN: &str = "Restart the computer and try again.";

const REMOVAL_NOTES: [&str; 8] = [
    "The virtual keyboard and mouse are unplugged, the driver",
    "services are stopped and the driver package is deleted from",
    "Windows. The computer ends up exactly as it was before the",
    "very first run of this program.",
    "",
    "The computer has to be restarted afterwards: until then",
    "Windows keeps the old driver in memory and this program",
    "cannot work.",
];

const RESTART_TO_FINISH: &str = "Restart the computer to finish";
const STILL_LOADED: &str = "Windows keeps the driver loaded in memory until the next start.";
const RESTART_QUESTION: &str = "Restart the computer now?";
const RESTART_ANSWERS: &str = "Y = yes, N = later";
const RESTARTING: &str = "The computer restarts in a few seconds";
const CANCEL_RESTART: &str = "Cancel with:  shutdown /a";
const RESTART_FAILED: &str = "The restart could not be started, please restart manually";
const NO_RESTART: &str = "Without a restart the driver cannot be removed completely";

const NO_RESTART_NOTES: [&str; 4] = [
    "Windows keeps the old driver loaded until the computer restarts,",
    "so the removal stays half finished: the driver still answers, but",
    "no virtual keyboard or mouse can be created. Restart when it",
    "suits you and everything is done.",
];

const PENDING_RESTART: &str = "The driver was removed, the computer has not restarted yet";

const PENDING_NOTES: [&str; 3] = [
    "Until the restart, Windows still keeps the old driver in memory",
    "and refuses to create the virtual keyboard and mouse, so there",
    "is nothing this program can do right now.",
];

const PRESS_ANY_KEY: &str = "Press any key to close this window.";
const PRESS_ANY_KEY_TO_RETURN: &str = "Press any key to go back to the menu.";

impl App {
    pub(super) fn toggle_mouse(&mut self) {
        let Some(engine) = self.engine.as_ref() else {
            return;
        };

        let (tone, message) = if engine.toggle_mouse() {
            (Tone::Done, MOUSE_ON)
        } else {
            (Tone::Muted, MOUSE_OFF)
        };

        self.screen.say(tone, message);
    }

    pub(super) fn toggle_keyboard(&mut self) {
        let Some(engine) = self.engine.as_ref() else {
            return;
        };

        let (tone, message) = if engine.toggle_keyboard() {
            (Tone::Done, KEYBOARD_ON)
        } else {
            (Tone::Muted, KEYBOARD_OFF)
        };

        self.screen.say(tone, message);
    }

    pub(super) fn stop_everything(&mut self) {
        let Some(engine) = self.engine.as_ref() else {
            return;
        };

        if !engine.is_mouse_enabled() && !engine.is_keyboard_enabled() {
            self.screen.say(Tone::Muted, NOTHING_REDIRECTED);
            return;
        }

        engine.stop();
        self.screen.say(Tone::Done, STOPPED);
    }

    /// Unplugs the virtual keyboard and mouse and plugs them back in. Takes
    /// over the screen while it runs, then puts the redirects back the way the
    /// user had them.
    pub(super) fn recreate_devices(&mut self) {
        let Some(driver) = self.driver.clone() else {
            return;
        };

        let (mouse_was, keyboard_was) = match self.engine.as_ref() {
            Some(engine) => {
                let switches = (engine.is_mouse_enabled(), engine.is_keyboard_enabled());
                // The devices being written to are about to disappear.
                engine.stop();
                switches
            }
            None => (false, false),
        };

        self.screen.begin_screen();
        self.screen.report(Tone::Working, RECREATING);

        let screen = &self.screen;
        let result = driver.recreate_virtual_devices(&mut |step: Step| {
            screen.report(Tone::Working, &step.to_string());
        });

        match result {
            Ok(()) => self.devices_are_back(mouse_was, keyboard_was),
            Err(error) => {
                let message = error.to_string();
                self.screen.report(Tone::Failure, &message);
                // The session carries on here - the devices are gone but the
                // menu still works - so the prompt must not promise a window
                // that is about to close.
                self.wait_for_menu();
                self.screen.say(Tone::Warning, message);
            }
        }

        ui::discard_pending_keys();
    }

    fn devices_are_back(&mut self, mouse_was: bool, keyboard_was: bool) {
        if let Some(engine) = self.engine.as_ref() {
            if mouse_was {
                engine.set_mouse(true);
            }
            if keyboard_was {
                engine.set_keyboard(true);
            }
        }

        let message = if mouse_was || keyboard_was {
            DEVICES_RESUMED
        } else {
            DEVICES_BACK
        };

        self.screen.say(Tone::Done, message);
    }

    /// Takes the driver package back out of Windows, so the next start behaves
    /// exactly like the very first one on a clean machine.
    ///
    /// Returns an outcome when the program has to close.
    pub(super) fn remove_driver(&mut self) -> Option<Outcome> {
        if !self.confirm_removal() {
            self.screen.say(Tone::Muted, NOTHING_REMOVED);
            ui::discard_pending_keys();
            return None;
        }

        self.screen.begin_screen();
        self.screen.report(Tone::Working, REMOVING);

        // The hooks hold the driver too, and they hold it on another thread.
        // Asking them to let go is what makes this the last owner.
        if let Some(engine) = self.engine.take() {
            engine.release_driver();
        }

        let driver = self.driver.take()?;
        let Ok(driver) = Arc::try_unwrap(driver) else {
            self.screen.report(Tone::Failure, STILL_IN_USE);
            self.screen.note(START_AGAIN);
            self.wait_for_close();
            return Some(Outcome::Finished);
        };

        match driver.remove() {
            Ok(()) => Some(self.finish_removal()),
            Err(error) => Some(self.removal_failed(&error.to_string())),
        }
    }

    /// Explains what removal means before asking for a yes.
    fn confirm_removal(&mut self) -> bool {
        self.screen.begin_screen();
        self.screen.report(Tone::Warning, ABOUT_TO_REMOVE);
        self.screen.blank();

        for note in REMOVAL_NOTES {
            self.screen.note(note);
        }

        self.screen.blank();
        self.screen.ask(REMOVE_QUESTION, REMOVE_ANSWERS);

        ui::confirm()
    }

    /// The driver is gone from this session either way, so there is nothing
    /// left to go back to: the window says what happened and closes.
    fn removal_failed(&mut self, reason: &str) -> Outcome {
        // The reason is already a whole sentence naming what failed - the error
        // type writes it that way - so it is shown as it is. Introducing it
        // again here produced "The driver could not be removed: the driver could
        // not be removed: ...", and worse when the step that failed was an
        // install, which the introduction then misnamed.
        self.screen.report(Tone::Failure, reason);
        self.screen.blank();
        self.screen.note(TRY_AGAIN);
        self.screen.blank();
        self.screen.report(Tone::Warning, SESSION_ENDS);
        self.wait_for_close();

        Outcome::Finished
    }

    /// The files are gone, but the running copies stay in the kernel until the
    /// machine restarts, so the removal is only really finished afterwards.
    fn finish_removal(&mut self) -> Outcome {
        driver::mark_restart_pending();

        self.screen.blank();
        self.screen.report(Tone::Done, REMOVED);
        self.screen.blank();
        self.screen.report(Tone::Warning, RESTART_TO_FINISH);
        self.screen.note(STILL_LOADED);
        self.screen.blank();

        self.offer_restart();
        self.wait_for_close();

        Outcome::RestartRequired
    }

    /// A previous run removed the driver and the machine has not restarted yet.
    /// Installing again now would only rebuild the half-removed stack.
    pub(super) fn offer_restart_from_last_session(&mut self) -> Outcome {
        self.screen.begin_screen();
        self.screen.report(Tone::Warning, PENDING_RESTART);
        self.screen.blank();

        for note in PENDING_NOTES {
            self.screen.note(note);
        }

        self.screen.blank();

        self.offer_restart();
        self.wait_for_close();

        Outcome::RestartRequired
    }

    /// Offers the restart and, when the answer is no, says what stays broken.
    fn offer_restart(&mut self) {
        self.screen.ask(RESTART_QUESTION, RESTART_ANSWERS);

        let yes = ui::confirm();
        self.screen.blank();

        if yes {
            self.start_restart();
            return;
        }

        self.screen.report(Tone::Warning, NO_RESTART);
        for note in NO_RESTART_NOTES {
            self.screen.note(note);
        }
    }

    fn start_restart(&mut self) {
        if driver::request_restart() {
            self.screen.report(Tone::Done, RESTARTING);
            self.screen.note(CANCEL_RESTART);
        } else {
            self.screen.report(Tone::Warning, RESTART_FAILED);
        }
    }

    /// Ends a screen the session does not come back from.
    fn wait_for_close(&self) {
        self.screen.blank();
        self.screen.note(PRESS_ANY_KEY);
        ui::wait_for_any_key();
    }

    /// Ends a screen the menu comes back from.
    fn wait_for_menu(&self) {
        self.screen.blank();
        self.screen.note(PRESS_ANY_KEY_TO_RETURN);
        ui::wait_for_any_key();
    }
}
