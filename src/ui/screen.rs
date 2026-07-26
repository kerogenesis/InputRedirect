//! Drawing the screen.
//!
//! Once the driver is up the console becomes a single screen that is redrawn
//! in place, so the window never fills up with repeated copies of the status.
//! The commentary is one line that gets replaced, not a log that grows.

use std::cell::RefCell;

use super::console;
use super::theme::{self, Tone};

/// Everything the status block and the menu need to know.
#[derive(Clone, Copy, Debug, Default)]
pub struct Dashboard {
    pub mouse_redirect: bool,
    pub keyboard_redirect: bool,
    pub driver_connected: bool,
    pub virtual_keyboard: bool,
    pub virtual_mouse: bool,
    pub keystrokes: u64,
    pub clicks: u64,
}

impl Dashboard {
    fn devices_ready(self) -> bool {
        self.virtual_keyboard && self.virtual_mouse
    }
}

/// Owns what the console currently shows.
#[derive(Default)]
pub struct Screen {
    message: Option<(Tone, String)>,
    /// The frame the console is showing right now, so an unchanged one can be
    /// left alone.
    shown: RefCell<Option<String>>,
}

impl Screen {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the line under the menu. There is only ever one.
    pub fn say(&mut self, tone: Tone, message: impl Into<String>) {
        self.message = Some((tone, message.into()));
    }

    /// The title block. Every full-screen step starts with it.
    pub fn banner(&self) {
        self.forget_what_is_shown();
        console::write_text(theme::BLANK);
        console::write_text(&theme::title(theme::TITLE));
        console::write_text(&theme::hint(theme::SUBTITLE));
        console::write_text(theme::BLANK);
    }

    /// Clears the window and puts the banner back at the top, ready for a step
    /// that takes over the screen for a moment.
    pub fn begin_screen(&self) {
        console::show_cursor(false);
        console::write_frame(theme::ERASE_BELOW);
        self.banner();
    }

    /// A line that scrolls past, used while something is being set up.
    pub fn report(&self, tone: Tone, message: &str) {
        self.forget_what_is_shown();
        console::write_text(&theme::line(tone, message));
    }

    pub fn note(&self, text: &str) {
        self.forget_what_is_shown();
        console::write_text(&theme::hint(text));
    }

    pub fn blank(&self) {
        self.forget_what_is_shown();
        console::write_text(theme::BLANK);
    }

    /// Asks a question and leaves the cursor after it.
    pub fn ask(&self, question: &str, answers: &str) {
        self.forget_what_is_shown();
        console::write_text(&theme::question(question, answers));
        console::show_cursor(true);
    }

    /// Redraws the whole screen from the top of the window.
    ///
    /// The loop comes back twice a second only to refresh the counters, and
    /// most of those visits change nothing. Writing the same frame again would
    /// give the console a chance to flicker for no reason.
    pub fn draw(&self, dashboard: Dashboard) {
        let frame = self.frame(dashboard);

        if self.shown.borrow().as_deref() == Some(frame.as_str()) {
            return;
        }

        console::show_cursor(false);
        console::write_frame(&frame);
        console::show_cursor(true);

        self.shown.replace(Some(frame));
    }

    /// Anything written outside `draw` moves the cursor, so what is remembered
    /// no longer describes the window.
    fn forget_what_is_shown(&self) {
        self.shown.replace(None);
    }

    /// The frame as text. Keeping this apart from writing it is what lets the
    /// tests below check the layout without a console.
    fn frame(&self, dashboard: Dashboard) -> String {
        let mut frame = String::with_capacity(2048);

        frame.push_str(theme::ERASE_BELOW);
        frame.push_str(theme::BLANK);
        frame.push_str(&theme::title(theme::TITLE));
        frame.push_str(&theme::hint(theme::SUBTITLE));
        frame.push_str(theme::BLANK);
        frame.push_str(&status(dashboard));
        frame.push_str(&menu(dashboard));
        frame.push_str(theme::BLANK);
        frame.push_str(&self.message_line());
        frame.push_str("   Your choice: ");

        frame
    }

    fn message_line(&self) -> String {
        match &self.message {
            Some((tone, message)) => {
                format!("   {}{message}{}\r\n", tone.color(), theme::RESET)
            }
            None => theme::BLANK.to_owned(),
        }
    }
}

fn status(dashboard: Dashboard) -> String {
    let mut text = theme::rule();

    text.push_str(&redirect_field("Mouse buttons", dashboard.mouse_redirect));
    text.push_str(&redirect_field("Keyboard", dashboard.keyboard_redirect));

    let color = if dashboard.driver_connected && dashboard.devices_ready() {
        theme::GREEN
    } else {
        theme::YELLOW
    };
    text.push_str(&theme::field("Driver", &driver_state(dashboard), color));

    let sent = format!(
        "{} keystrokes, {} clicks",
        dashboard.keystrokes, dashboard.clicks
    );
    text.push_str(&theme::field("Sent through driver", &sent, theme::DIM));
    text.push_str(&theme::rule());

    text
}

/// A redirect is either running or it is not, and the colour says which.
fn redirect_field(label: &str, redirected: bool) -> String {
    let (value, color) = if redirected {
        ("redirected", theme::GREEN)
    } else {
        ("normal", theme::DIM)
    };

    theme::field(label, value, color)
}

/// Says what is wrong as well as what is right: a connected driver with no
/// virtual devices behind it looks fine but cannot do anything.
fn driver_state(dashboard: Dashboard) -> String {
    let mut state = if dashboard.driver_connected {
        "connected".to_owned()
    } else {
        "not responding".to_owned()
    };

    match (dashboard.virtual_keyboard, dashboard.virtual_mouse) {
        (true, true) => {}
        (true, false) => state.push_str(", virtual mouse missing"),
        (false, true) => state.push_str(", virtual keyboard missing"),
        (false, false) => state.push_str(", virtual devices missing"),
    }

    state
}

fn menu(dashboard: Dashboard) -> String {
    let mut text = theme::BLANK.to_owned();

    text.push_str(&theme::switch(
        '1',
        "redirect mouse / touchpad",
        dashboard.mouse_redirect,
    ));
    text.push_str(&theme::switch(
        '2',
        "redirect keyboard",
        dashboard.keyboard_redirect,
    ));
    text.push_str(&theme::action('3', "stop everything"));
    text.push_str(&theme::action('4', "re-create virtual devices"));
    text.push_str(&theme::action('D', "remove driver"));
    text.push_str(&theme::action('Q', "quit"));

    text.push_str(theme::BLANK);
    for note in NOTES {
        text.push_str(&theme::hint(note));
    }

    text
}

/// The four things worth knowing that the menu itself cannot say.
const NOTES: [&str; 4] = [
    "1 and 2 switch their own redirect on and off.",
    "Pointer movement and the wheel are never touched, only the buttons.",
    "Combinations with Shift, Ctrl, Alt or Win are left to the real keyboard.",
    "Closing this window switches everything back to normal.",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> Dashboard {
        Dashboard {
            mouse_redirect: true,
            keyboard_redirect: false,
            driver_connected: true,
            virtual_keyboard: true,
            virtual_mouse: true,
            keystrokes: 12,
            clicks: 3,
        }
    }

    fn line_with<'a>(frame: &'a str, needle: &str) -> &'a str {
        frame
            .lines()
            .find(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("a line containing {needle}"))
    }

    #[test]
    fn a_frame_starts_by_erasing_the_previous_one() {
        let frame = Screen::new().frame(running());

        assert!(frame.starts_with(theme::ERASE_BELOW));
        assert!(frame.contains(theme::TITLE));
    }

    #[test]
    fn the_frame_ends_where_the_user_is_expected_to_answer() {
        let frame = Screen::new().frame(running());

        assert!(frame.ends_with("   Your choice: "));
    }

    #[test]
    fn every_menu_key_is_on_the_screen() {
        let frame = Screen::new().frame(running());

        for (key, label) in [
            ("[1]", "redirect mouse / touchpad"),
            ("[2]", "redirect keyboard"),
            ("[3]", "stop everything"),
            ("[4]", "re-create virtual devices"),
            ("[D]", "remove driver"),
            ("[Q]", "quit"),
        ] {
            assert!(line_with(&frame, key).contains(label), "{key}");
        }
    }

    #[test]
    fn the_switch_that_is_on_is_the_one_that_is_green() {
        let frame = Screen::new().frame(running());

        let mouse = line_with(&frame, "[1]");
        let keyboard = line_with(&frame, "[2]");

        assert!(mouse.contains(theme::GREEN) && mouse.contains("on"));
        assert!(!keyboard.contains(theme::GREEN) && keyboard.contains("off"));
    }

    #[test]
    fn the_status_block_repeats_what_the_switches_say() {
        let frame = Screen::new().frame(running());

        assert!(line_with(&frame, "Mouse buttons").contains("redirected"));
        assert!(line_with(&frame, "Keyboard ").contains("normal"));
        assert!(frame.contains("12 keystrokes, 3 clicks"));
    }

    #[test]
    fn a_driver_without_its_devices_says_which_one_is_missing() {
        let half_there = Dashboard {
            driver_connected: true,
            virtual_keyboard: true,
            ..Dashboard::default()
        };

        let frame = Screen::new().frame(half_there);
        let driver = line_with(&frame, "Driver");

        assert!(driver.contains("connected, virtual mouse missing"));
        assert!(driver.contains(theme::YELLOW));
    }

    #[test]
    fn a_driver_that_is_not_answering_is_never_shown_as_fine() {
        let frame = Screen::new().frame(Dashboard::default());
        let driver = line_with(&frame, "Driver");

        assert!(driver.contains("not responding, virtual devices missing"));
        assert!(!driver.contains(theme::GREEN));
    }

    #[test]
    fn the_message_is_replaced_rather_than_piled_up() {
        let mut screen = Screen::new();

        screen.say(Tone::Done, "first");
        screen.say(Tone::Warning, "second");
        let frame = screen.frame(running());

        assert!(!frame.contains("first"));
        assert!(line_with(&frame, "second").contains(theme::YELLOW));
    }

    #[test]
    fn a_screen_without_a_message_keeps_the_line_free() {
        let quiet = Screen::new().frame(running());

        let mut screen = Screen::new();
        screen.say(Tone::Done, "something happened");
        let spoken = screen.frame(running());

        // The only difference is the one line above the prompt.
        assert!(quiet.len() < spoken.len());
        assert!(quiet.ends_with("\r\n   Your choice: "));
    }

    #[test]
    fn a_dashboard_that_did_not_move_produces_the_very_same_frame() {
        let screen = Screen::new();

        assert_eq!(screen.frame(running()), screen.frame(running()));
    }

    #[test]
    fn one_more_keystroke_is_a_different_frame() {
        let screen = Screen::new();

        let busier = Dashboard {
            keystrokes: running().keystrokes + 1,
            ..running()
        };

        assert_ne!(screen.frame(running()), screen.frame(busier));
    }

    #[test]
    fn a_line_printed_outside_the_frame_forces_the_next_draw() {
        let screen = Screen::new();
        screen.shown.replace(Some("whatever was there".to_owned()));

        screen.forget_what_is_shown();

        assert!(screen.shown.borrow().is_none());
    }
}
