//! `InputRedirect` - sends the local keyboard, touchpad and mouse through a
//! signed Logitech driver.

mod app;
mod driver;
mod error;
mod hid;
mod redirect;
mod ui;

use std::process::ExitCode;

use app::{App, Outcome};
use error::Error;

/// Exit codes, so a script around the program can tell the cases apart.
const EXIT_NOT_ELEVATED: u8 = 1;
const EXIT_FAILED: u8 = 2;
const EXIT_RESTART_REQUIRED: u8 = 3;
const EXIT_ALREADY_RUNNING: u8 = 4;
const EXIT_USAGE: u8 = 5;

fn main() -> ExitCode {
    match App::new().run() {
        Ok(Outcome::Finished) => ExitCode::SUCCESS,
        // Both outcomes have already had their say on a screen of their own,
        // and have already waited to be read.
        Ok(Outcome::RestartRequired) => ExitCode::from(EXIT_RESTART_REQUIRED),
        Err(error) => give_up(&error),
    }
}

/// Says why the program cannot run, and leaves the window up long enough for it
/// to be read - these failures happen before there is a screen to say it on.
fn give_up(error: &Error) -> ExitCode {
    let code = match error {
        Error::NotElevated => {
            eprintln!("InputRedirect has to be started as an administrator.");
            eprintln!("Right-click the program and choose \"Run as administrator\".");
            EXIT_NOT_ELEVATED
        }
        Error::AlreadyRunning => {
            eprintln!("InputRedirect is already running in another window.");
            eprintln!("Switch to that window, or close it before starting again.");
            EXIT_ALREADY_RUNNING
        }
        Error::Usage(message) => {
            eprintln!("InputRedirect: {message}.");
            eprintln!("Run InputRedirect --help for usage.");
            EXIT_USAGE
        }
        Error::RestartRequired(reason) => {
            eprintln!("InputRedirect cannot start yet: {reason}.");
            eprintln!("Restart the computer, then start InputRedirect again.");
            EXIT_RESTART_REQUIRED
        }
        error => {
            eprintln!("InputRedirect could not start: {error}");
            EXIT_FAILED
        }
    };

    ui::wait_before_the_window_closes();

    ExitCode::from(code)
}
