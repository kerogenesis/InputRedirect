use bitflags::bitflags;

/// A boot keyboard report is one modifier byte, one reserved byte and six key
/// slots - see the USB HID usage tables, appendix B.1.
pub const KEYBOARD_REPORT_LEN: usize = 8;
pub const MAX_PRESSED_KEYS: usize = 6;

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Modifiers: u8 {
        const LEFT_CTRL   = 0x01;
        const LEFT_SHIFT  = 0x02;
        const LEFT_ALT    = 0x04;
        const LEFT_GUI    = 0x08;
        const RIGHT_CTRL  = 0x10;
        const RIGHT_SHIFT = 0x20;
        const RIGHT_ALT   = 0x40;
        const RIGHT_GUI   = 0x80;
    }
}

/// The set of keys the virtual keyboard currently reports as held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyboardReport {
    modifiers: Modifiers,
    keys: [u8; MAX_PRESSED_KEYS],
}

impl KeyboardReport {
    pub const EMPTY: Self = Self {
        modifiers: Modifiers::empty(),
        keys: [0; MAX_PRESSED_KEYS],
    };

    /// Returns `false` when all six slots are taken.
    pub fn press(&mut self, usage: u8) -> bool {
        // Zero is how an empty slot is spelled, so it can never be a key.
        if usage == 0 {
            return false;
        }

        // Already held: the report says what it should, and no slot is spent.
        if self.keys.contains(&usage) {
            return true;
        }

        match self.keys.iter_mut().find(|slot| **slot == 0) {
            Some(slot) => {
                *slot = usage;
                true
            }
            None => false,
        }
    }

    pub fn release(&mut self, usage: u8) {
        for slot in &mut self.keys {
            if *slot == usage {
                *slot = 0;
            }
        }
    }

    /// Whether this key is one of the held slots.
    ///
    /// The release path asks this rather than trusting a separate record: only
    /// the report knows what the virtual keyboard is really holding.
    #[must_use]
    pub fn holds(self, usage: u8) -> bool {
        // Zero is an empty slot, not a key, exactly as `press` refuses it.
        usage != 0 && self.keys.contains(&usage)
    }

    pub fn clear(&mut self) {
        *self = Self::EMPTY;
    }

    #[must_use]
    pub fn to_bytes(self) -> [u8; KEYBOARD_REPORT_LEN] {
        let mut bytes = [0u8; KEYBOARD_REPORT_LEN];
        bytes[0] = self.modifiers.bits();
        bytes[2..].copy_from_slice(&self.keys);
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_report_is_all_zeroes() {
        assert_eq!(KeyboardReport::EMPTY.to_bytes(), [0; KEYBOARD_REPORT_LEN]);
    }

    #[test]
    fn a_key_lands_after_the_modifier_and_reserved_bytes() {
        let mut report = KeyboardReport::EMPTY;
        assert!(report.press(0x04)); // "a"

        // Byte 0 is the modifier bitmap, byte 1 is reserved, keys start at 2.
        assert_eq!(report.to_bytes(), [0x00, 0x00, 0x04, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_press_of_nothing_is_refused_and_leaves_the_report_alone() {
        let mut report = KeyboardReport::EMPTY;

        assert!(!report.press(0));
        assert_eq!(report, KeyboardReport::EMPTY);
    }

    #[test]
    fn pressing_the_same_key_twice_does_not_take_a_second_slot() {
        let mut report = KeyboardReport::EMPTY;
        assert!(report.press(0x04));
        assert!(report.press(0x04));

        assert_eq!(report.to_bytes()[2..], [0x04, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn the_seventh_simultaneous_key_is_refused_instead_of_overwriting_another() {
        let mut report = KeyboardReport::EMPTY;
        for usage in 0x04..0x0A {
            assert!(report.press(usage));
        }

        assert!(!report.press(0x0A));
        assert_eq!(report.to_bytes()[2..], [0x04, 0x05, 0x06, 0x07, 0x08, 0x09]);
    }

    #[test]
    fn releasing_frees_the_slot_for_reuse() {
        let mut report = KeyboardReport::EMPTY;
        assert!(report.press(0x04));
        assert!(report.press(0x05));
        report.release(0x04);
        assert!(report.press(0x06));

        assert_eq!(report.to_bytes()[2..], [0x06, 0x05, 0, 0, 0, 0]);
    }

    #[test]
    fn a_report_knows_which_keys_it_is_holding() {
        let mut report = KeyboardReport::EMPTY;
        assert!(!report.holds(0x04));

        assert!(report.press(0x04));
        assert!(report.holds(0x04));
        assert!(!report.holds(0x05));

        report.release(0x04);
        assert!(!report.holds(0x04));

        // Zero is the empty slot, never a held key.
        assert!(!report.holds(0));
    }

    #[test]
    fn clearing_empties_every_slot() {
        let mut report = KeyboardReport::EMPTY;
        assert!(report.press(0x04));
        assert!(report.press(0x05));
        report.clear();

        assert_eq!(report, KeyboardReport::EMPTY);
        assert_eq!(report.to_bytes(), [0; KEYBOARD_REPORT_LEN]);
    }
}
