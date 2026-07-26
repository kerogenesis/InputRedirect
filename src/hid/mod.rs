//! The data the virtual devices speak.
//!
//! Nothing in this module touches Windows, which is what makes it the one part
//! of the program that is fully covered by tests.

mod keyboard;
mod mouse;
mod scancode;

pub use keyboard::{KeyboardReport, Modifiers};
pub use mouse::{MouseButtons, MouseReport};
pub use scancode::{modifier_of, ScanCode};
