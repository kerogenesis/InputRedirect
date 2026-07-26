//! The program as the user experiences it: a screen, a menu and a loop.

mod actions;
mod exit;
mod instance;

use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use crate::driver::{self, Driver, Step};
use crate::error::{Error, Result};
use crate::redirect::Engine;
use crate::ui::{self, Command, Dashboard, MenuKey, Screen, Tone};

/// Long enough for the setup lines to be read before the screen takes over.
const SETTLE: Duration = Duration::from_millis(600);

/// Why the program stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Finished,
    RestartRequired,
}

pub struct App {
    screen: Screen,
    driver: Option<Arc<Driver>>,
    engine: Option<Engine>,
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self {
            screen: Screen::new(),
            driver: None,
            engine: None,
        }
    }

    /// Brings the driver up and then serves the menu until the user leaves.
    pub fn run(mut self) -> Result<Outcome> {
        // Held for the whole session: dropping it is what lets the next copy
        // of the program start.
        let _only_copy = instance::SingleInstance::claim().ok_or(Error::AlreadyRunning)?;

        ui::claim_console();

        // From here on the window can be closed at any moment, and the
        // redirects have to stop even though no destructor will run.
        exit::watch_for_close();

        // A restart is only really owed while the driver is half-removed. If it
        // is installed and answering, the flag is stale - a build that wrote it
        // non-volatile would leave it set past the reboot that should have
        // cleared it - and offering a restart that cannot help, every start
        // from now on, is the one outcome reboot.rs set out to avoid.
        if driver::is_restart_pending() {
            if driver::is_running() {
                driver::clear_restart_pending();
            } else {
                return Ok(self.offer_restart_from_last_session());
            }
        }

        self.start()?;

        loop {
            self.redraw();

            match ui::wait_for_command(ui::TICK_MS) {
                // Nothing was pressed: the loop comes back only to refresh the
                // counters, which is what makes the screen feel alive.
                MenuKey::Tick => {}
                MenuKey::Unknown => self
                    .screen
                    .say(Tone::Warning, "Unknown key. Use 1, 2, 3, 4, D or Q."),
                MenuKey::Chosen(command) => {
                    if let Some(outcome) = self.carry_out(command) {
                        return Ok(outcome);
                    }
                }
            }
        }
    }

    /// Runs one menu entry. Returns an outcome when the program has to close.
    fn carry_out(&mut self, command: Command) -> Option<Outcome> {
        match command {
            Command::ToggleMouse => self.toggle_mouse(),
            Command::ToggleKeyboard => self.toggle_keyboard(),
            Command::StopEverything => self.stop_everything(),
            Command::RecreateDevices => self.recreate_devices(),
            Command::RemoveDriver => return self.remove_driver(),
            Command::Quit => return Some(self.shut_down()),
        }

        None
    }

    fn start(&mut self) -> Result<()> {
        self.screen.banner();

        // The setup lines scroll past as they happen, so a slow first
        // installation does not look like a frozen program.
        let screen = &self.screen;
        let driver = Driver::connect(&mut |step: Step| {
            let tone = if step == Step::InstallingDriver {
                Tone::Working
            } else {
                Tone::Done
            };
            screen.report(tone, &step.to_string());
        })?;

        let driver = Arc::new(driver);
        self.engine = Some(Engine::install(Arc::clone(&driver))?);
        self.driver = Some(driver);

        self.screen.report(Tone::Done, "Ready");
        sleep(SETTLE);

        self.screen.say(
            Tone::Muted,
            "Nothing is redirected yet. Press 1 or 2 to start.",
        );
        Ok(())
    }

    /// The last screen: says what was switched back before the window closes.
    fn shut_down(&mut self) -> Outcome {
        self.screen.begin_screen();
        self.screen.report(Tone::Working, "Shutting down");

        if let Some(engine) = self.engine.take() {
            engine.stop();
        }
        self.driver = None;

        self.screen
            .report(Tone::Done, "Your keyboard and mouse are back to normal");
        self.screen.blank();

        Outcome::Finished
    }

    pub(super) fn redraw(&mut self) {
        let dashboard = self.dashboard();
        self.screen.draw(dashboard);
    }

    pub(super) fn dashboard(&self) -> Dashboard {
        let status = self.driver.as_ref().map(|driver| driver.status());
        let counters = self.engine.as_ref().map(Engine::stats).unwrap_or_default();

        Dashboard {
            mouse_redirect: self.engine.as_ref().is_some_and(Engine::is_mouse_enabled),
            keyboard_redirect: self
                .engine
                .as_ref()
                .is_some_and(Engine::is_keyboard_enabled),
            driver_connected: status.is_some_and(|status| status.connected),
            virtual_keyboard: status.is_some_and(|status| status.virtual_keyboard),
            virtual_mouse: status.is_some_and(|status| status.virtual_mouse),
            keystrokes: counters.keystrokes,
            clicks: counters.clicks,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Quitting from the menu and panicking both end up here. Closing the
        // window does not - it goes straight to the same cleanup instead.
        if let Some(engine) = self.engine.take() {
            engine.stop();
        }

        exit::clean_up();
        self.driver = None;
    }
}
