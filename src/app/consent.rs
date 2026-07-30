//! Asking before the driver goes on the machine.
//!
//! Removing the driver has always explained itself and waited for a yes.
//! Installing it did not, and it is the less reversible of the two: the package
//! stays in the driver store after this window closes, protected games look for
//! exactly this driver, and on some machines Windows will not turn Memory
//! Integrity on while it is installed.
//!
//! Both screens are built the same way the removal screen is - a headline, a
//! block of plain sentences, then a question - because a user who has read one
//! of them should recognise the shape of the other.

use crate::driver::{Consent, Mismatch, Version};
use crate::ui::{self, Screen, Tone};

const ABOUT_TO_INSTALL: &str = "About to install the Logitech driver on this computer";

const INSTALL_NOTES: [&str; 13] = [
    "This program cannot create a virtual keyboard or mouse by",
    "itself. It carries Logitech's signed driver package and",
    "installs it: three kernel drivers, added to the Windows",
    "driver store and started as system services.",
    "",
    "They were written by Logitech, not by this project, and two",
    "things follow from that. Windows may refuse to turn Memory",
    "Integrity on while they are installed, and anti-cheat",
    "software looks for this driver by name.",
    "",
    "The driver stays on this computer after the window closes.",
    "Press D in the menu to remove it again; the computer then",
    "has to restart to finish the job.",
];

const INSTALL_QUESTION: &str = "Install the driver?";

const ABOUT_TO_REPLACE: &str = "A different build of the Logitech driver is installed";

const REPLACE_NOTES: [&str; 12] = [
    "The requests this program sends were read out of one build",
    "of the driver. Another build answers them with nothing more",
    "than \"invalid parameter\", so the installed one has to go and",
    "ours has to take its place.",
    "",
    "Whatever put that build there - Logitech G HUB, or a driver",
    "update - was using it, and may not work properly again until",
    "it installs its own.",
    "",
    "Pressing D in the menu removes our driver, but it does not",
    "bring the other build back. Only the software that owns it",
    "can do that.",
];

const REPLACE_QUESTION: &str = "Replace the installed driver?";

const ANSWERS: &str = "Y = yes, N = no";

/// Puts the questions on the screen the program is already drawing to.
pub struct Ask<'a> {
    screen: &'a Screen,
}

impl<'a> Ask<'a> {
    pub fn on(screen: &'a Screen) -> Self {
        Self { screen }
    }

    /// The part both screens end with.
    fn notes_then_question(&self, notes: &[&str], question: &str) -> bool {
        for note in notes {
            self.screen.note(note);
        }

        self.screen.blank();
        self.screen.ask(question, ANSWERS);

        ui::confirm()
    }
}

impl Consent for Ask<'_> {
    fn allow_install(&mut self) -> bool {
        self.screen.begin_screen();
        self.screen.report(Tone::Warning, ABOUT_TO_INSTALL);
        self.screen.blank();

        self.notes_then_question(&INSTALL_NOTES, INSTALL_QUESTION)
    }

    fn allow_replacement(&mut self, mismatch: Mismatch) -> bool {
        self.screen.begin_screen();
        self.screen.report(Tone::Warning, ABOUT_TO_REPLACE);
        self.screen.blank();

        // The two builds are what there is to weigh here, so they are named
        // above the explanation rather than left inside it.
        for line in builds(mismatch.installed, mismatch.ours) {
            self.screen.note(&line);
        }
        self.screen.blank();

        self.notes_then_question(&REPLACE_NOTES, REPLACE_QUESTION)
    }
}

/// Which build is on the machine and which one would take its place.
///
/// Two lines rather than one sentence: on one line the two version numbers sit
/// next to each other and the reader has to work out which is which.
fn builds(installed: Version, ours: Version) -> [String; 2] {
    [
        format!("Installed on this computer:  {installed}"),
        format!("The build this program speaks to:  {ours}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The widest a line may be. The status block draws a rule of 58 columns
    /// from the second, and a note is indented by four, so a line longer than
    /// this wraps and the block stops reading as a block.
    const WIDTH: usize = 60;

    /// A machine carrying a 2024 build, against the one this program speaks to.
    fn mismatch() -> Mismatch {
        Mismatch {
            installed: Version::from_words(0x07e8_0001, 0),
            ours: Version::from_words(0x07e5_0001, 0x0555_0000),
        }
    }

    fn install_screen() -> String {
        INSTALL_NOTES.join(" ")
    }

    fn replace_screen() -> String {
        REPLACE_NOTES.join(" ")
    }

    /// A note that wraps breaks the block it belongs to, and these are the two
    /// longest blocks in the program.
    #[test]
    fn every_line_of_both_screens_fits_the_console_layout() {
        for note in INSTALL_NOTES.iter().chain(REPLACE_NOTES.iter()) {
            assert!(
                note.chars().count() <= WIDTH,
                "{note:?} is {} columns wide",
                note.chars().count()
            );
        }

        for headline in [ABOUT_TO_INSTALL, ABOUT_TO_REPLACE] {
            assert!(headline.chars().count() <= WIDTH, "{headline:?}");
        }
    }

    /// The whole reason this screen exists. A rewording that quietly dropped one
    /// of these would put the program back where it started.
    #[test]
    fn the_installation_screen_says_what_the_driver_is_and_what_it_costs() {
        let screen = install_screen();

        for said in ["kernel", "Logitech", "Memory Integrity", "anti-cheat"] {
            assert!(
                screen.contains(said),
                "the screen does not mention {said:?}"
            );
        }
    }

    /// A user who agrees has to know how to change their mind afterwards.
    #[test]
    fn the_installation_screen_says_how_to_undo_it() {
        let screen = install_screen();

        assert!(screen.contains("Press D in the menu to remove it again"));
        assert!(screen.contains("restart"));
    }

    /// Replacing somebody else's working installation is a different question
    /// from installing into an empty one, and it is asked as one.
    #[test]
    fn the_replacement_screen_is_about_the_installation_it_would_overwrite() {
        let screen = replace_screen();

        assert!(screen.contains("G HUB"));
        assert!(
            screen.contains("does not bring the other build back"),
            "the screen has to say that D will not bring the other build back"
        );
    }

    /// "Replacing the driver" is a sentence a user can only wonder about; the
    /// two versions are what they can quote in a question.
    #[test]
    fn the_replacement_screen_names_both_builds() {
        let broken = mismatch();
        let [installed, ours] = builds(broken.installed, broken.ours);

        assert!(
            installed.contains("2024.1.0.0"),
            "{installed:?} hides what is there"
        );
        assert!(
            ours.contains("2021.1.1365.0"),
            "{ours:?} hides what we speak"
        );
    }

    /// The version numbers are read from the files, so the widest one Windows
    /// can report has to fit as well as the one this program ships with.
    #[test]
    fn the_widest_version_windows_could_report_still_fits_the_layout() {
        let widest = Version::from_words(u32::MAX, u32::MAX);

        for line in builds(widest, widest) {
            assert!(line.chars().count() <= WIDTH, "{line:?}");
        }
    }

    /// Both questions have to be answerable with the keys the prompt offers.
    #[test]
    fn both_questions_offer_the_same_two_answers() {
        assert!(ANSWERS.contains('Y') && ANSWERS.contains('N'));

        for question in [INSTALL_QUESTION, REPLACE_QUESTION] {
            assert!(question.ends_with('?'), "{question:?}");
        }
    }
}
