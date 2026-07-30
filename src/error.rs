//! One error type for the whole program.
//!
//! Every message is written for the person in front of the screen: the CLI
//! prints these strings as they are, so they say what went wrong rather than
//! which Win32 call returned what.
//!
//! There is deliberately no `#[from] std::io::Error`. Every I/O failure here
//! happens at a point where the program knows what it was doing - writing a
//! driver file, running the plug and play utility - and that sentence is worth
//! more to the reader than "access denied" on its own. An automatic conversion
//! would make throwing that sentence away the path of least resistance.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("administrator rights are required")]
    NotElevated,

    #[error("another copy of InputRedirect is already running")]
    AlreadyRunning,

    #[error("the driver files could not be prepared: {0}")]
    Payload(String),

    #[error("the driver could not be installed: {0}")]
    Install(String),

    /// The driver package is in place, but the copy Windows has loaded is not
    /// it, and will not be until the machine restarts. Kept apart from
    /// `Install` because nothing is wrong and nothing is worth retrying: the
    /// answer is a restart, and saying so is the whole point.
    #[error("the computer has to be restarted first: {0}")]
    RestartRequired(String),

    #[error("the driver could not be removed: {0}")]
    Uninstall(String),

    #[error("the driver is installed but not reachable: {0}")]
    Device(String),

    #[error("the virtual {device} could not be created: {reason}")]
    VirtualDevice {
        device: &'static str,
        reason: String,
    },

    #[error("the input hooks could not be installed: {0}")]
    Hook(String),

    /// An argument on the command line was not understood. Kept apart from the
    /// driver failures because nothing was attempted: the program refuses the
    /// line before it touches the console or the driver.
    #[error("{0}")]
    Usage(String),
}
