//! Reading the redirects asked for on the command line.
//!
//! With no arguments `InputRedirect` shows its menu as it always has. Given
//! `--mouse`, `--keyboard`, or both, it switches those redirects on at once
//! and stays out of the way: no menu is drawn, one green line says what is
//! running, and closing the window or pressing Ctrl+C switches everything
//! back, exactly as the menu does.

use crate::error::{Error, Result};

const MOUSE_FLAG: &str = "--mouse";
const KEYBOARD_FLAG: &str = "--keyboard";

/// Which redirects the command line switched on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Requested {
    pub mouse: bool,
    pub keyboard: bool,
}

impl Requested {
    /// Whether the command line asked for anything at all. `false` is the
    /// signal to fall back to the interactive menu.
    #[must_use]
    pub fn any(self) -> bool {
        self.mouse || self.keyboard
    }

    /// The single line the flag mode prints in place of the menu, naming
    /// exactly which redirects are running.
    #[must_use]
    pub fn active_message(self) -> &'static str {
        match (self.mouse, self.keyboard) {
            (true, true) => "Mouse and keyboard redirect active",
            (true, false) => "Mouse redirect active",
            (false, true) => "Keyboard redirect active",
            // The flag mode runs only when something was asked for, so this
            // arm is never reached; it keeps the match total without a panic.
            (false, false) => "Nothing is redirected",
        }
    }
}

/// Reads the requested redirects from the arguments, with the program name
/// already removed.
///
/// An unrecognised argument is refused rather than ignored: a misspelt
/// `--mouse` that quietly opened the menu instead would look like the flag
/// does nothing.
pub fn parse<I>(arguments: I) -> Result<Requested>
where
    I: IntoIterator<Item = String>,
{
    let mut requested = Requested::default();

    for argument in arguments {
        match argument.as_str() {
            MOUSE_FLAG => requested.mouse = true,
            KEYBOARD_FLAG => requested.keyboard = true,
            other => return Err(Error::Usage(format!("unknown argument {other:?}"))),
        }
    }

    Ok(requested)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(arguments: &[&str]) -> Result<Requested> {
        parse(arguments.iter().copied().map(String::from))
    }

    #[test]
    fn no_arguments_means_the_menu() {
        let requested = parse_args(&[]).unwrap();

        assert!(!requested.any());
        assert!(!requested.mouse);
        assert!(!requested.keyboard);
    }

    #[test]
    fn the_mouse_flag_switches_the_mouse_on_and_leaves_the_keyboard_alone() {
        let requested = parse_args(&["--mouse"]).unwrap();

        assert!(requested.mouse);
        assert!(!requested.keyboard);
    }

    #[test]
    fn the_keyboard_flag_switches_the_keyboard_on_and_leaves_the_mouse_alone() {
        let requested = parse_args(&["--keyboard"]).unwrap();

        assert!(requested.keyboard);
        assert!(!requested.mouse);
    }

    #[test]
    fn the_two_flags_together_switch_both_on() {
        let requested = parse_args(&["--mouse", "--keyboard"]).unwrap();

        assert!(requested.mouse);
        assert!(requested.keyboard);
    }

    #[test]
    fn the_order_of_the_flags_does_not_matter() {
        let one_way = parse_args(&["--mouse", "--keyboard"]).unwrap();
        let other_way = parse_args(&["--keyboard", "--mouse"]).unwrap();

        assert_eq!(one_way, other_way);
    }

    #[test]
    fn a_flag_given_twice_is_still_just_on() {
        let requested = parse_args(&["--mouse", "--mouse"]).unwrap();

        assert!(requested.mouse);
        assert!(!requested.keyboard);
    }

    #[test]
    fn an_unknown_argument_is_refused() {
        assert!(parse_args(&["--trackball"]).is_err());
    }

    #[test]
    fn an_unknown_argument_after_a_good_one_is_still_refused() {
        assert!(parse_args(&["--mouse", "--trackball"]).is_err());
    }

    #[test]
    fn the_active_line_names_exactly_what_is_running() {
        let mouse = parse_args(&["--mouse"]).unwrap();
        let keyboard = parse_args(&["--keyboard"]).unwrap();
        let both = parse_args(&["--mouse", "--keyboard"]).unwrap();

        assert_eq!(mouse.active_message(), "Mouse redirect active");
        assert_eq!(keyboard.active_message(), "Keyboard redirect active");
        assert_eq!(both.active_message(), "Mouse and keyboard redirect active");
    }
}
