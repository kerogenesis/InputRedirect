//! Colours, glyphs and the small pieces every line is built from.
//!
//! The indents and column widths are the ones the C++ build used, because the
//! layout is what makes the screen readable: two spaces for a message, three
//! for anything inside a block, and labels wide enough that the values line up.

/// What a line means, which is what decides how it looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Done,
    Working,
    Warning,
    Failure,
    Muted,
}

impl Tone {
    #[must_use]
    pub const fn color(self) -> &'static str {
        match self {
            Self::Done => GREEN,
            Self::Working => CYAN,
            Self::Warning => YELLOW,
            Self::Failure => RED,
            Self::Muted => DIM,
        }
    }

    /// Exactly one column wide, so every sentence starts at the same place.
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Done => "\u{2713}",
            Self::Working => "\u{2022}",
            Self::Warning => "!",
            Self::Failure => "\u{d7}",
            Self::Muted => " ",
        }
    }
}

pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[90m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const RED: &str = "\x1b[31m";
pub const CYAN: &str = "\x1b[36m";
pub const RESET: &str = "\x1b[0m";

/// Erases everything from the cursor down; every frame starts with it.
pub const ERASE_BELOW: &str = "\x1b[0J";

pub const BLANK: &str = "\r\n";

pub const TITLE: &str = "InputRedirect";
pub const SUBTITLE: &str = "Sends your typing and your clicks through the real signed driver";

const RULE_WIDTH: usize = 58;
const LABEL_WIDTH: usize = 20;
const SWITCH_WIDTH: usize = 34;

#[must_use]
pub fn rule() -> String {
    format!("  {DIM}{}{RESET}\r\n", "\u{2500}".repeat(RULE_WIDTH))
}

#[must_use]
pub fn title(text: &str) -> String {
    format!("  {BOLD}{text}{RESET}\r\n")
}

/// An indented aside, dimmed so it stays out of the way.
#[must_use]
pub fn hint(text: &str) -> String {
    format!("    {DIM}{text}{RESET}\r\n")
}

/// A line of commentary: a coloured glyph and the sentence next to it.
#[must_use]
pub fn line(tone: Tone, message: &str) -> String {
    format!("  {}{}{RESET} {message}\r\n", tone.color(), tone.glyph())
}

/// "Label               value", without the reader having to count spaces.
#[must_use]
pub fn field(label: &str, value: &str, color: &str) -> String {
    format!("   {DIM}{label:<LABEL_WIDTH$}{RESET}{color}{value}{RESET}\r\n")
}

/// A menu entry that is a switch: the state sits in its own column on the
/// right and is green for as long as the redirect is running.
#[must_use]
pub fn switch(key: char, label: &str, on: bool) -> String {
    let (state, color) = if on { ("on", GREEN) } else { ("off", DIM) };

    format!("   {BOLD}[{key}]{RESET}  {label:<SWITCH_WIDTH$}{color}{state}{RESET}\r\n")
}

/// A menu entry that simply does something.
#[must_use]
pub fn action(key: char, label: &str) -> String {
    format!("   {BOLD}[{key}]{RESET}  {label}\r\n")
}

/// A question the program waits on, with the cursor left after it.
#[must_use]
pub fn question(text: &str, answers: &str) -> String {
    format!("   {BOLD}{text}{RESET}  {answers}: ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_is_as_wide_as_the_layout() {
        assert_eq!(rule().matches('\u{2500}').count(), RULE_WIDTH);
    }

    #[test]
    fn every_glyph_is_one_column_wide_so_the_lines_stay_aligned() {
        for tone in [
            Tone::Done,
            Tone::Working,
            Tone::Warning,
            Tone::Failure,
            Tone::Muted,
        ] {
            assert_eq!(tone.glyph().chars().count(), 1, "{tone:?}");
        }
    }

    #[test]
    fn a_switch_that_is_on_is_green_and_one_that_is_off_is_not() {
        let on = switch('1', "redirect mouse / touchpad", true);
        let off = switch('1', "redirect mouse / touchpad", false);

        assert!(on.contains(GREEN) && on.contains("on"));
        assert!(!off.contains(GREEN) && off.contains("off"));
    }

    #[test]
    fn the_state_of_every_switch_starts_in_the_same_column() {
        let short = switch('2', "redirect keyboard", true);
        let long = switch('1', "redirect mouse / touchpad", false);

        let column = |text: &str| {
            text.find(GREEN)
                .or_else(|| text.find(DIM))
                .expect("the state")
        };
        assert_eq!(column(&short), column(&long));
    }

    #[test]
    fn an_action_carries_no_state_at_all() {
        let entry = action('3', "stop everything");

        assert!(!entry.contains("off"));
        assert!(!entry.contains(GREEN));
    }

    #[test]
    fn a_field_pads_the_label_so_the_values_line_up() {
        let short = field("Keyboard", "x", "");
        let long = field("Sent through driver", "x", "");

        let column = |text: &str| text.find('x').expect("the value");
        assert_eq!(column(&short), column(&long));
    }
}
