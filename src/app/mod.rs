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

const SETTLE: Duration = Duration::from_millis(50);
const DRIVER_SERVICE_KEY: &str = r"SYSTEM\CurrentControlSet\Services\logi_joy_bus_enum";

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

    pub fn run(mut self) -> Result<Outcome> {
        let request = cli::parse(std::env::args().skip(1))?;
        if request == cli::Request::Help {
            show_help();
            return Ok(Outcome::Finished);
        }

        let _only_copy = instance::SingleInstance::claim().ok_or(Error::AlreadyRunning)?;
        let restart_pending = driver::is_restart_pending();

        if request == cli::Request::RemoveDriver && !restart_pending && !driver_is_installed() {
            println!("InputRedirect: no driver is installed, so there is nothing to remove.");
            return Ok(Outcome::Finished);
        }

        ui::claim_console();
        exit::watch_for_close();

        if restart_pending {
            if driver::is_running() {
                driver::clear_restart_pending();
            } else {
                return Ok(self.offer_restart_from_last_session());
            }
        }

        if request == cli::Request::RemoveDriver {
            if let Some(outcome) = self.remove_driver() {
                return Ok(outcome);
            }
            self.screen.report(Tone::Muted, "Nothing was removed");
            return Ok(Outcome::Finished);
        }

        self.start()?;

        if let cli::Request::Redirect(requested) = request {
            return Ok(self.run_headless(requested));
        }

        loop {
            self.redraw();
            match ui::wait_for_command(ui::TICK_MS) {
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

    fn run_headless(&self, requested: cli::Requested) -> Outcome {
        if let Some(engine) = self.engine.as_ref() {
            if requested.mouse {
                engine.set_mouse(true);
            }
            if requested.keyboard {
                engine.set_keyboard(true);
            }
        }

        self.screen.blank();
        self.screen.report(Tone::Done, requested.active_message());

        loop {
            std::thread::park();
        }
    }

    fn start(&mut self) -> Result<()> {
        self.screen.banner();
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
        if let Some(engine) = self.engine.take() {
            engine.stop();
        }
        exit::clean_up();
        self.driver = None;
    }
}

fn driver_is_installed() -> bool {
    LOCAL_MACHINE
        .options()
        .read()
        .open(DRIVER_SERVICE_KEY)
        .is_ok()
}

fn show_help() {
    println!("{}", cli::HELP);
}
