//! The program as the user experiences it: a screen, a menu and a loop.

mod actions;
mod cli;
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
    ///
    /// With `--mouse` or `--keyboard` on the command line it switches those
    /// redirects on instead and waits without a menu; see `run_headless`.
    pub fn run(mut self) -> Result<Outcome> {
        // Refused before anything is claimed or installed, so a misspelt flag
        // fails on the spot rather than half-starting the program.
        let requested = cli::parse(std::env::args().skip(1))?;

        // Held for the whole session: dropping it is what lets the next copy
        // of the program start.
        let _only_copy = instance::SingleInstance::claim().ok_or(Error::AlreadyRunning)?;

        ui::claim_console();

        // From here on the window can be closed at any moment, and the
        // redirects have to stop even though no destructor will run.
        exit::watch_for_close();

        // A restart is only really owed while the driver is half-removed. If it
        // is installed and answering, the flag is stale, and offering a restart
        // that cannot help on every start from now on is the one outcome the
        // pending flag is meant to avoid.
        if driver::is_restart_pending() {
            if driver::is_running() {
                driver::clear_restart_pending();
            } else {
                return Ok(self.offer_restart_from_last_session());
            }
        }

        self.start()?;

        // The command line, not the menu, is driving: switch on what it asked
        // for and wait it out.
        if requested.any() {
            return Ok(self.run_headless(requested));
        }

        self.screen.say(
            Tone::Muted,
            "Nothing is redirected yet. Press 1 or 2 to start.",
        );

        loop {
            self.redraw();

            match ui::wait_for_command(ui::TICK_MS) {
                // Nothing was pressed: the loop comes back only to refresh the
                // counters, which is what keeps the screen feeling alive.
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

    /// Switches on the redirects named on the command line, says which ones in
    /// one green line, and then waits without drawing a menu or reading a key.
    ///
    /// The only ways out are the window closing and Ctrl+C. Windows delivers
    /// both to the console control handler set up in `exit`, which switches
    /// the redirects back and ends the process - the same path closing the
    /// menu takes, so there is nothing to tear down here.
    fn run_headless(&self, requested: cli::Requested) -> Outcome {
        if let Some(engine) = self.engine.as_ref() {
            if requested.mouse {
                engine.set_mouse(true);
            }
            if requested.keyboard {
                engine.set_keyboard(true);
            }
        }

        self.screen.report(Tone::Done, requested.active_message());

        // Park rather than spin: there is nothing to do until the control
        // handler ends the process, and a spurious wake just parks again.
        loop {
            std::thread::park();
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

        Ok(())
    }

    fn redraw(&self) {
        self.screen.draw(self.dashboard());
    }

    fn dashboard(&self) -> Dashboard {
        let engine = self.engine.as_ref();
        let stats = engine.map(Engine::stats).unwrap_or_default();
        let driver = self.driver.as_ref();
        let devices = driver.map(|driver| driver.devices()).unwrap_or_default();

        Dashboard {
            mouse_redirect: engine.is_some_and(Engine::is_mouse_enabled),
            keyboard_redirect: engine.is_some_and(Engine::is_keyboard_enabled),
            driver_connected: driver.is_some_and(|driver| driver.is_connected()),
            virtual_keyboard: devices.keyboard,
            virtual_mouse: devices.mouse,
            keystrokes: stats.keystrokes,
            clicks: stats.clicks,
        }
    }
}
