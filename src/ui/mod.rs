//! The console interface.
//!
//! Everything the user reads is written here, in plain sentences. The layers
//! below report what happened; this one decides how it looks.

mod console;
mod prompt;
mod screen;
mod theme;

pub use console::{claim_console, release_console};
pub use prompt::{
    confirm, discard_pending_keys, wait_before_the_window_closes, wait_for_any_key,
    wait_for_command, Command, MenuKey, TICK_MS,
};
pub use screen::{Dashboard, Screen};
pub use theme::Tone;
