//! Reading what the command line asks for.
//!
//! With no arguments `InputRedirect` shows its menu as it always has. Given
//! `--mouse` / `-m`, `--keyboard` / `-k`, or both, it switches those redirects
//! on at once and stays out of the way: no menu is drawn, one green line says
//! what is running, and closing the window or pressing Ctrl+C switches
//! everything back, exactly as the menu does. `--help` / `-h` lists the flags
//! and exits.

use crate::error::{Error, Result};

const MOUSE_FLAGS: [&str; 2] = ["--mouse", "-m"];
const KEYBOARD_FLAGS: [&str; 2] = ["--keyboard", "-k"];
const HELP_FLAGS: [&str; 2] = ["--help", "-h"];

/// The usage text `--help` prints. It is shown before the console is prepared
/// for drawing, so it carries no colour or glyphs: just the flags and what
/// each one does.
pub const HELP: &str = "\
InputRedirect - send your typing and your clicks through the real signed driver.

Usage:
    InputRedirect [options]

Options:
    -m, --mouse       redirect the mouse buttons
    -k, --keyboard    redirect the keyboard
    -h, --help        show this help and exit

With no options the interactive menu opens. Closing the window or pressing
Ctrl+C switches everything back and ends the program.";

/// What the command line asked the program to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// No redirect flags were given: open the interactive menu.
    Menu,
    /// Switch on exactly these redirects and skip the menu.
    Redirect(Requested),
    /// Print the usage text and leave, without touching anything else.
    Help,
}

/// Which redirects the command line switched on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Requested {
    pub mouse: bool,
    pub keyboard: bool,
}

impl Requested {
    /// The single line the flag mode prints in place of the menu, naming
    /// exactly which redirects are running.
    #[must_use]
    pub fn active_message(self) -> &'static str {
        match (self.mouse, self.keyboard) {
            (true, true) => "Mouse and keyboard redirect active",
            (true, false) => "Mouse redirect active",
            (false, true) => "Keyboard redirect active",
            // `parse` returns `Request::Menu` when nothing was asked for, so
            // this arm is never reached; it keeps the match total, no panic.
            (false, false) => "Nothing is redirected",
        }
    }
}

/// Reads what the arguments ask for, with the program name already removed.
///
/// An unrecognised argument is refused rather than ignored: a misspelt
/// `--mouse` that quietly opened the menu instead would look like the flag
/// does nothing. `--help` wins over any redirect alongside it, since someone
/// asking to read the flags did not mean to start one.
pub fn parse<I>(arguments: I) -> Result<Request>
where
    I: IntoIterator<Item = String>,
{
    let mut requested = Requested::default();

    for argument in arguments {
        let argument = argument.as_str();

        if HELP_FLAGS.contains(&argument) {
            return Ok(Request::Help);
        } else if MOUSE_FLAGS.contains(&argument) {
            requested.mouse = true;
        } else if KEYBOARD_FLAGS.contains(&argument) {
            requested.keyboard = true;
        } else {
            return Err(Error::Usage(format!("unknown argument {argument:?}")));
        }
    }

    if requested.mouse || requested.keyboard {
        Ok(Request::Redirect(requested))
    } else {
        Ok(Request::Menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(arguments: &[&str]) -> Result<Request> {
        parse(arguments.iter().copied().map(String::from))
    }

    fn requested(mouse: bool, keyboard: bool) -> Requested {
        Requested { mouse, keyboard }
    }

    fn redirect(mouse: bool, keyboard: bool) -> Request {
        Request::Redirect(requested(mouse, keyboard))
    }

    #[test]
    fn no_arguments_means_the_menu() {
        assert_eq!(parse_args(&[]).unwrap(), Request::Menu);
    }

    #[test]
    fn the_mouse_flag_switches_the_mouse_on_and_leaves_the_keyboard_alone() {
        let request = parse_args(&["--mouse"]).unwrap();

        assert_eq!(request, redirect(true, false));
    }

    #[test]
    fn the_keyboard_flag_switches_the_keyboard_on_and_leaves_the_mouse_alone() {
        let request = parse_args(&["--keyboard"]).unwrap();

        assert_eq!(request, redirect(false, true));
    }

    #[test]
    fn the_short_mouse_flag_means_the_same_as_the_long_one() {
        assert_eq!(parse_args(&["-m"]).unwrap(), redirect(true, false));
    }

    #[test]
    fn the_short_keyboard_flag_means_the_same_as_the_long_one() {
        assert_eq!(parse_args(&["-k"]).unwrap(), redirect(false, true));
    }

    #[test]
    fn the_two_flags_together_switch_both_on() {
        let request = parse_args(&["--mouse", "--keyboard"]).unwrap();

        assert_eq!(request, redirect(true, true));
    }

    #[test]
    fn the_short_flags_can_be_combined_too() {
        let request = parse_args(&["-m", "-k"]).unwrap();

        assert_eq!(request, redirect(true, true));
    }

    #[test]
    fn the_order_of_the_flags_does_not_matter() {
        let one_way = parse_args(&["--mouse", "--keyboard"]).unwrap();
        let other_way = parse_args(&["--keyboard", "--mouse"]).unwrap();

        assert_eq!(one_way, other_way);
    }

    #[test]
    fn a_flag_given_twice_is_still_just_on() {
        let request = parse_args(&["--mouse", "-m"]).unwrap();

        assert_eq!(request, redirect(true, false));
    }

    #[test]
    fn either_spelling_of_help_asks_for_help() {
        assert_eq!(parse_args(&["--help"]).unwrap(), Request::Help);
        assert_eq!(parse_args(&["-h"]).unwrap(), Request::Help);
    }

    #[test]
    fn help_wins_over_a_redirect_alongside_it() {
        let after_mouse = parse_args(&["--mouse", "--help"]).unwrap();
        let before_keyboard = parse_args(&["-h", "--keyboard"]).unwrap();

        assert_eq!(after_mouse, Request::Help);
        assert_eq!(before_keyboard, Request::Help);
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
        let mouse = requested(true, false);
        let keyboard = requested(false, true);
        let both = requested(true, true);

        assert_eq!(mouse.active_message(), "Mouse redirect active");
        assert_eq!(keyboard.active_message(), "Keyboard redirect active");
        assert_eq!(both.active_message(), "Mouse and keyboard redirect active");
    }
}
