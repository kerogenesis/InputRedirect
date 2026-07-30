//! The program as the user experiences it: a screen, a menu and a loop.

mod actions;
mod cli;
mod exit;
mod instance;

use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use windows_registry::LOCAL_MACHINE;

use crate::driver::{self, Driver, Step};
use crate::error::{Error, Result};
use crate::redirect::Engine;
use crate::ui::{self, Command, Dashboard, MenuKey, Screen, Tone};

/// Long enough for the setup lines to be read before the screen takes over.
const SETTLE: Duration = Duration::from_millis(600);

/// The service whose presence means the driver package is installed.
const DRIVER_SERVICE_KEY: &str = r"SYSTEM\CurrentControlSet\Services\logi_joy_bus_enum";

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
    /// `--help` / `-h` prints the list of flags and exits before anything is
    /// claimed. `--remove-driver` / `-r` removes an installed driver without
    /// installing one when there is nothing to remove. `--mouse` / `-m` or
    /// `--keyboard` / `-k` switch those redirects on and wait without a menu
    /// instead; see `run_headless`.
    pub fn run(mut self) -> Result<Outcome> {
        // Read before anything is claimed or installed, so a misspelt flag
        // fails on the spot and `--help` answers without a driver.
        let request = cli::parse(std::env::args().skip(1))?;

        // Help touches nothing else: it is printed and the program leaves.
        if request == cli::Request::Help {
            show_help();
            return Ok(Outcome::Finished);
        }

        // Held for the whole session: dropping it is what lets the next copy
        // of the program start.
        let _only_copy = instance::SingleInstance::claim().ok_or(Error::AlreadyRunning)?;

        let restart_pending = driver::is_restart_pending();

        // Driver::connect installs a missing package. A removal request must
        // never create the very thing it was asked to take away, so answer the
        // no-op before preparing the console or calling start. A pending
        // restart is different: removal already happened and still has to be
        // finished, so the existing restart screen below takes precedence.
        if request == cli::Request::RemoveDriver
            && !restart_pending
            && !driver_is_installed()
        {
            println!("InputRedirect: no driver is installed, so there is nothing to remove.");
            return Ok(Outcome::Finished);
        }

        ui::claim_console();

        // From here on the window can be closed at any moment, and the
        // redirects have to stop even though no destructor will run.
        exit::watch_for_close();

        // A restart is only really owed while the driver is half-removed. If it
        // is installed and answering, the flag is stale - a build that wrote it
        // non-volatile would leave it set past the reboot that should have
        // cleared it - and offering a restart that cannot help, every start
        // from now on, is the one outcome reboot.rs set out to avoid.
        if restart_pending {
            if driver::is_running() {
                driver::clear_restart_pending();
            } else {
                return Ok(self.offer_restart_from_last_session());
            }
        }

        self.start()?;

        // Asked to remove the driver from the command line: run the very flow
        // the menu's R does - it confirms, removes, and offers the restart -
        // then ends, rather than dropping into the menu afterwards.
        if request == cli::Request::RemoveDriver {
            if let Some(outcome) = self.remove_driver() {
                return Ok(outcome);
            }

            // In menu mode `say` is rendered by the next redraw. There is no
            // next redraw here, so report the declined operation directly.
            self.screen.report(Tone::Muted, "Nothing was removed");
            return Ok(Outcome::Finished);
        }

        // The command line, not the menu, is driving: switch on what it asked
        // for and wait it out.
        if let cli::Request::Redirect(requested) = request {
            return Ok(self.run_headless(requested));
        }

        loop {
            self.redraw();

            match ui::wait_for_command(ui::TICK_MS) {
                // Nothing was pressed: the loop comes back only to refresh the
                // counters, which is what makes the screen feel alive.
                MenuKey::Tick => {}
                MenuKey::Unknown => self
                    .screen
                    .say(Tone::Warning, "Unknown key. Use 1, 2, 3, 4, R or Q."),
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

        // A blank line sets this apart from the setup lines that scrolled
        // past, so the one line worth reading stands on its own.
        self.screen.blank();
        self.screen.report(Tone::Done, requested.active_message());

        // Park rather than spin: there is nothing to do until the control
        // handler ends the process, and a spurious wake just parks again.
        loop {
            std::thread::park();
        }
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

/// Whether the driver package exists without starting or installing it.
fn driver_is_installed() -> bool {
    LOCAL_MACHINE
        .options()
        .read()
        .open(DRIVER_SERVICE_KEY)
        .is_ok()
}

/// Prints the command-line help and returns. Help is shown before the console
/// is prepared for drawing, so it is plain text, and it does not wait for a
/// key: a command-line tool that was asked for its usage prints it and exits.
fn show_help() {
    println!("{}", cli::HELP);
}
