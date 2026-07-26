//! Set 1 scan codes, as reported by the low-level keyboard hook, translated
//! into HID usage ids, as understood by the virtual keyboard.
//!
//! Going through the scan code rather than the virtual key is what makes the
//! redirect independent of the active keyboard layout: the scan code describes
//! the physical key, the virtual key describes the letter printed on it.

use super::Modifiers;

/// A physical key press as it arrives from `WH_KEYBOARD_LL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanCode {
    pub code: u16,
    /// Set for the keys the keyboard prefixes with `0xE0`: the arrow block,
    /// the right modifiers, the numpad divide and so on.
    pub extended: bool,
}

impl ScanCode {
    #[must_use]
    pub const fn new(code: u16, extended: bool) -> Self {
        Self { code, extended }
    }

    #[must_use]
    pub fn hid_usage(self) -> Option<u8> {
        hid_usage(self)
    }
}

const UNMAPPED: u8 = 0;

#[rustfmt::skip]
const BASE: [u8; 0x59] = [
    /* 0x00 */ 0x00, 0x29, 0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23,
    /* 0x08 */ 0x24, 0x25, 0x26, 0x27, 0x2D, 0x2E, 0x2A, 0x2B,
    /* 0x10 */ 0x14, 0x1A, 0x08, 0x15, 0x17, 0x1C, 0x18, 0x0C,
    /* 0x18 */ 0x12, 0x13, 0x2F, 0x30, 0x28, 0xE0, 0x04, 0x16,
    /* 0x20 */ 0x07, 0x09, 0x0A, 0x0B, 0x0D, 0x0E, 0x0F, 0x33,
    /* 0x28 */ 0x34, 0x35, 0xE1, 0x31, 0x1D, 0x1B, 0x06, 0x19,
    /* 0x30 */ 0x05, 0x11, 0x10, 0x36, 0x37, 0x38, 0xE5, 0x55,
    /* 0x38 */ 0xE2, 0x2C, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E,
    /* 0x40 */ 0x3F, 0x40, 0x41, 0x42, 0x43, 0x53, 0x47, 0x5F,
    /* 0x48 */ 0x60, 0x61, 0x56, 0x5C, 0x5D, 0x5E, 0x57, 0x59,
    /* 0x50 */ 0x5A, 0x5B, 0x62, 0x63, 0x00, 0x00, 0x64, 0x44,
    /* 0x58 */ 0x45,
];

#[must_use]
pub fn hid_usage(key: ScanCode) -> Option<u8> {
    let usage = if key.extended {
        extended_usage(key.code)
    } else {
        BASE.get(usize::from(key.code)).copied().unwrap_or(UNMAPPED)
    };

    (usage != UNMAPPED).then_some(usage)
}

#[rustfmt::skip]
fn extended_usage(code: u16) -> u8 {
    match code {
        0x1C => 0x58, // numpad enter
        0x1D => 0xE4, // right ctrl
        0x35 => 0x54, // numpad divide
        0x37 => 0x46, // print screen
        0x38 => 0xE6, // right alt
        0x46 => 0x48, // pause / break
        0x47 => 0x4A, // home
        0x48 => 0x52, // arrow up
        0x49 => 0x4B, // page up
        0x4B => 0x50, // arrow left
        0x4D => 0x4F, // arrow right
        0x4F => 0x4D, // end
        0x50 => 0x51, // arrow down
        0x51 => 0x4E, // page down
        0x52 => 0x49, // insert
        0x53 => 0x4C, // delete
        0x5B => 0xE3, // left windows
        0x5C => 0xE7, // right windows
        0x5D => 0x65, // context menu
        _ => UNMAPPED,
    }
}

/// The modifier bit a usage id stands for, if it is a modifier at all.
#[must_use]
pub fn modifier_of(usage: u8) -> Option<Modifiers> {
    let modifier = match usage {
        0xE0 => Modifiers::LEFT_CTRL,
        0xE1 => Modifiers::LEFT_SHIFT,
        0xE2 => Modifiers::LEFT_ALT,
        0xE3 => Modifiers::LEFT_GUI,
        0xE4 => Modifiers::RIGHT_CTRL,
        0xE5 => Modifiers::RIGHT_SHIFT,
        0xE6 => Modifiers::RIGHT_ALT,
        0xE7 => Modifiers::RIGHT_GUI,
        _ => return None,
    };

    Some(modifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(code: u16) -> Option<u8> {
        hid_usage(ScanCode::new(code, false))
    }

    fn extended(code: u16) -> Option<u8> {
        hid_usage(ScanCode::new(code, true))
    }

    #[test]
    fn the_letter_row_matches_the_hid_usage_tables() {
        assert_eq!(usage(0x1E), Some(0x04)); // a
        assert_eq!(usage(0x30), Some(0x05)); // b
        assert_eq!(usage(0x10), Some(0x14)); // q
        assert_eq!(usage(0x2C), Some(0x1D)); // z
    }

    #[test]
    fn digits_start_at_one_and_wrap_around_to_zero() {
        assert_eq!(usage(0x02), Some(0x1E)); // 1
        assert_eq!(usage(0x0A), Some(0x26)); // 9
        assert_eq!(usage(0x0B), Some(0x27)); // 0
    }

    #[test]
    fn function_keys_are_contiguous() {
        for (index, code) in (0x3B..=0x44).enumerate() {
            assert_eq!(
                usage(code),
                Some(0x3A + index as u8),
                "scan code {code:#04X}"
            );
        }
        assert_eq!(usage(0x57), Some(0x44)); // f11
        assert_eq!(usage(0x58), Some(0x45)); // f12
    }

    #[test]
    fn the_prefixed_keys_are_a_different_key_than_the_plain_ones() {
        // 0x1D is left ctrl, 0xE0 0x1D is right ctrl - the classic mistake
        // this table exists to avoid.
        assert_eq!(usage(0x1D), Some(0xE0));
        assert_eq!(extended(0x1D), Some(0xE4));

        // Numpad 8 versus arrow up, numpad enter versus the main one.
        assert_eq!(usage(0x48), Some(0x60));
        assert_eq!(extended(0x48), Some(0x52));
        assert_eq!(usage(0x1C), Some(0x28));
        assert_eq!(extended(0x1C), Some(0x58));
    }

    #[test]
    fn ctrl_alt_delete_resolves_to_the_three_expected_usages() {
        assert_eq!(usage(0x1D), Some(0xE0));
        assert_eq!(usage(0x38), Some(0xE2));
        assert_eq!(extended(0x53), Some(0x4C));
    }

    #[test]
    fn unknown_and_reserved_codes_report_nothing_instead_of_a_wrong_key() {
        assert_eq!(usage(0x00), None);
        assert_eq!(usage(0x54), None);
        assert_eq!(usage(0x55), None);
        assert_eq!(usage(0xFF), None);
        assert_eq!(extended(0x01), None);
    }

    #[test]
    fn every_modifier_usage_maps_to_exactly_one_bit() {
        let modifiers = [
            (0xE0, Modifiers::LEFT_CTRL),
            (0xE1, Modifiers::LEFT_SHIFT),
            (0xE2, Modifiers::LEFT_ALT),
            (0xE3, Modifiers::LEFT_GUI),
            (0xE4, Modifiers::RIGHT_CTRL),
            (0xE5, Modifiers::RIGHT_SHIFT),
            (0xE6, Modifiers::RIGHT_ALT),
            (0xE7, Modifiers::RIGHT_GUI),
        ];

        for (usage, expected) in modifiers {
            assert_eq!(modifier_of(usage), Some(expected), "usage {usage:#04X}");
        }
        assert_eq!(modifier_of(0x04), None);
    }

    #[test]
    fn no_two_physical_keys_share_a_usage_id() {
        let mut seen = std::collections::HashMap::new();
        for code in 0..=0xFFu16 {
            for is_extended in [false, true] {
                let key = ScanCode::new(code, is_extended);
                if let Some(usage) = hid_usage(key) {
                    if let Some(previous) = seen.insert(usage, key) {
                        panic!("usage {usage:#04X} claimed by {previous:?} and {key:?}");
                    }
                }
            }
        }
    }
}
