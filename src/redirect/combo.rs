//! Deciding which keys are part of a shortcut.
//!
//! Rebuilding a shortcut on the virtual keyboard is unreliable: the modifier is
//! seen on one device and the letter on another, and some combinations - the
//! secure attention sequence above all - are never delivered. So while a
//! modifier is held, the keyboard is left alone.
//!
//! This runs inside the low level keyboard hook, so the set of held modifiers
//! is a single bitflag and nothing here allocates.
//!
//! Only presses are decided here. A release is decided by what the virtual
//! keyboard is actually holding, which only the report knows.

use crate::hid::{modifier_of, Modifiers};

#[derive(Debug, Default)]
pub struct ComboWatcher {
    held_modifiers: Modifiers,
}

impl ComboWatcher {
    /// Records a modifier going down or up.
    ///
    /// Has to be called for every key, and before anything else looks at the
    /// watcher: the set of held modifiers is what later presses are judged
    /// against.
    pub fn note(&mut self, usage: u8, pressed: bool) {
        let Some(modifier) = modifier_of(usage) else {
            return;
        };

        if pressed {
            self.held_modifiers.insert(modifier);
        } else {
            self.held_modifiers.remove(modifier);
        }
    }

    /// Whether the press being handled belongs to a shortcut.
    ///
    /// `live` reports the modifiers really held, and is asked only when this
    /// watcher believes one is down - the one belief that can be wrong. A
    /// modifier released while another desktop had the keyboard is never seen
    /// here, and would otherwise be believed held for the rest of the session.
    pub fn press_belongs_to_shortcut(&mut self, live: impl FnOnce() -> Modifiers) -> bool {
        if self.held_modifiers.is_empty() {
            return false;
        }

        self.held_modifiers &= live();

        !self.held_modifiers.is_empty()
    }

    /// Forgets modifiers held when the redirect was switched off.
    pub fn clear(&mut self) {
        self.held_modifiers = Modifiers::empty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT_CTRL: u8 = 0xE0;
    const LEFT_ALT: u8 = 0xE2;
    const KEY_A: u8 = 0x04;
    const KEY_C: u8 = 0x06;

    /// Stands in for the real keyboard agreeing with everything the hook saw.
    fn honest(watcher: &ComboWatcher) -> Modifiers {
        watcher.held_modifiers
    }

    fn belongs_to_shortcut(watcher: &mut ComboWatcher) -> bool {
        let live = honest(watcher);
        watcher.press_belongs_to_shortcut(|| live)
    }

    #[test]
    fn a_plain_key_is_redirected() {
        let mut watcher = ComboWatcher::default();
        watcher.note(KEY_A, true);

        assert!(!belongs_to_shortcut(&mut watcher));
    }

    #[test]
    fn a_key_pressed_under_a_modifier_is_left_to_windows() {
        let mut watcher = ComboWatcher::default();
        watcher.note(LEFT_CTRL, true);
        watcher.note(KEY_C, true);

        assert!(belongs_to_shortcut(&mut watcher));
    }

    #[test]
    fn typing_resumes_being_redirected_once_the_last_modifier_is_up() {
        let mut watcher = ComboWatcher::default();
        watcher.note(LEFT_CTRL, true);
        watcher.note(LEFT_ALT, true);
        watcher.note(LEFT_CTRL, false);

        // One modifier is still held, so the key belongs to a shortcut.
        assert!(belongs_to_shortcut(&mut watcher));

        watcher.note(LEFT_ALT, false);

        assert!(!belongs_to_shortcut(&mut watcher));
    }

    #[test]
    fn a_key_that_is_not_a_modifier_does_not_change_what_is_held() {
        let mut watcher = ComboWatcher::default();
        watcher.note(KEY_A, true);
        watcher.note(KEY_A, false);

        assert!(!belongs_to_shortcut(&mut watcher));
    }

    #[test]
    fn clearing_forgets_modifiers_that_were_never_released() {
        let mut watcher = ComboWatcher::default();
        watcher.note(LEFT_CTRL, true);
        watcher.clear();

        assert!(!belongs_to_shortcut(&mut watcher));
    }

    /// The lock screen takes the keyboard away and gives back a modifier that
    /// was never released here.
    #[test]
    fn a_modifier_released_on_another_desktop_stops_holding_the_redirect_back() {
        let mut watcher = ComboWatcher::default();
        watcher.note(LEFT_CTRL, true);

        // Windows says nothing is held any more: the release went elsewhere.
        assert!(!watcher.press_belongs_to_shortcut(Modifiers::empty));

        // And the watcher believes it from now on, without asking again.
        assert!(!belongs_to_shortcut(&mut watcher));
    }

    /// Only the modifier that really went away is forgotten.
    #[test]
    fn a_modifier_that_is_still_held_survives_the_reconciliation() {
        let mut watcher = ComboWatcher::default();
        watcher.note(LEFT_CTRL, true);
        watcher.note(LEFT_ALT, true);

        assert!(watcher.press_belongs_to_shortcut(|| Modifiers::LEFT_ALT));
        assert_eq!(watcher.held_modifiers, Modifiers::LEFT_ALT);
    }

    /// Nothing is asked of Windows on the path every keystroke takes.
    #[test]
    fn the_common_case_never_asks_what_is_really_held() {
        let mut watcher = ComboWatcher::default();
        watcher.note(KEY_A, true);

        assert!(!watcher.press_belongs_to_shortcut(|| panic!("asked Windows for nothing")));
    }
}
