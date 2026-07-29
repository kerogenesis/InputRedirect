//! Recognising our own input coming back.
//!
//! The virtual keyboard and mouse are real HID devices, so everything they
//! report arrives at the hooks a moment later. Each injected event is counted
//! here and the matching echo is let through; without this the first key press
//! would loop forever.
//!
//! Every lookup happens inside a low-level hook, so the counters live in flat
//! arrays indexed by the event itself: no hashing, no allocation, no resizing.

use std::time::{Duration, Instant};

use crate::hid::MouseButtons;

/// How many echoes may pile up for one key before the oldest are forgotten. A
/// backlog can only mean the echo never arrived.
const MAX_PENDING: u8 = 8;

/// How long an expected echo stays expected. The driver answers in about a
/// millisecond, and holding on longer would swallow a real key press. The cap
/// above is not enough on its own: a single lost count would otherwise wait
/// until the same key is pressed again, which may be minutes later.
const ECHO_LIFETIME: Duration = Duration::from_millis(50);

/// Every HID usage, in both directions.
const KEY_SLOTS: usize = 256 * 2;

/// Every bit a button flag can occupy, in both directions.
const BUTTON_SLOTS: usize = 8 * 2;

/// The echoes still owed for one key or button, and when they stop counting.
#[derive(Clone, Copy, Debug)]
struct Pending {
    count: u8,
    expires_at: Instant,
}

#[derive(Debug)]
pub struct EchoFilter {
    keys: [Option<Pending>; KEY_SLOTS],
    buttons: [Option<Pending>; BUTTON_SLOTS],
}

impl Default for EchoFilter {
    fn default() -> Self {
        Self {
            keys: [None; KEY_SLOTS],
            buttons: [None; BUTTON_SLOTS],
        }
    }
}

impl EchoFilter {
    /// Records that we just sent this key through the driver.
    pub fn expect_key(&mut self, usage: u8, pressed: bool) {
        self.expect_key_at(usage, pressed, Instant::now());
    }

    pub fn expect_button(&mut self, button: MouseButtons, pressed: bool) {
        self.expect_button_at(button, pressed, Instant::now());
    }

    /// True when this event is an echo of one of ours, which also consumes it.
    pub fn take_key(&mut self, usage: u8, pressed: bool) -> bool {
        self.take_key_at(usage, pressed, Instant::now())
    }

    pub fn take_button(&mut self, button: MouseButtons, pressed: bool) -> bool {
        self.take_button_at(button, pressed, Instant::now())
    }

    /// Forgets everything expected of both devices.
    pub fn clear(&mut self) {
        self.clear_keys();
        self.clear_buttons();
    }

    /// The two devices are switched on and off separately, so each forgets only
    /// what it was owed: a dropped echo is an event let through as the real
    /// device's.
    pub fn clear_keys(&mut self) {
        self.keys = [None; KEY_SLOTS];
    }

    pub fn clear_buttons(&mut self) {
        self.buttons = [None; BUTTON_SLOTS];
    }

    // The clock is passed in below so the lifetime above can be tested without
    // sleeping through it.

    fn expect_key_at(&mut self, usage: u8, pressed: bool, now: Instant) {
        increment(&mut self.keys[slot(usage, pressed)], now);
    }

    fn expect_button_at(&mut self, button: MouseButtons, pressed: bool, now: Instant) {
        if let Some(index) = button_slot(button, pressed) {
            increment(&mut self.buttons[index], now);
        }
    }

    fn take_key_at(&mut self, usage: u8, pressed: bool, now: Instant) -> bool {
        decrement(&mut self.keys[slot(usage, pressed)], now)
    }

    fn take_button_at(&mut self, button: MouseButtons, pressed: bool, now: Instant) -> bool {
        match button_slot(button, pressed) {
            Some(index) => decrement(&mut self.buttons[index], now),
            None => false,
        }
    }
}

/// Press and release are counted apart, so the direction is the low bit.
fn slot(index: u8, pressed: bool) -> usize {
    usize::from(index) << 1 | usize::from(pressed)
}

/// The hooks report one button at a time; anything else has no slot and is
/// therefore never an echo of ours.
fn button_slot(button: MouseButtons, pressed: bool) -> Option<usize> {
    let bit = button.bits();
    if !bit.is_power_of_two() {
        return None;
    }

    Some(slot(bit.trailing_zeros() as u8, pressed))
}

/// Each new injection pushes the deadline out, so a held key repeating stays
/// expected for as long as it repeats.
fn increment(slot: &mut Option<Pending>, now: Instant) {
    let expires_at = now + ECHO_LIFETIME;
    let pending = slot.get_or_insert(Pending {
        count: 0,
        expires_at,
    });

    pending.count = pending.count.saturating_add(1).min(MAX_PENDING);
    pending.expires_at = expires_at;
}

fn decrement(slot: &mut Option<Pending>, now: Instant) -> bool {
    let Some(pending) = slot.as_mut() else {
        return false;
    };

    // A stale count is dropped whole, and the event that found it is treated as
    // the user's own.
    let stale = pending.expires_at <= now;
    let mut matched = false;
    if !stale && pending.count > 0 {
        pending.count -= 1;
        matched = true;
    }

    let spent = stale || pending.count == 0;
    if spent {
        *slot = None;
    }

    matched
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start() -> Instant {
        Instant::now()
    }

    #[test]
    fn an_event_we_never_sent_is_not_an_echo() {
        let mut filter = EchoFilter::default();

        assert!(!filter.take_key(0x04, true));
    }

    #[test]
    fn the_echo_of_an_injected_key_is_recognised_exactly_once() {
        let mut filter = EchoFilter::default();
        filter.expect_key(0x04, true);

        assert!(filter.take_key(0x04, true));
        assert!(!filter.take_key(0x04, true));
    }

    #[test]
    fn a_press_and_a_release_are_counted_apart() {
        let mut filter = EchoFilter::default();
        filter.expect_key(0x04, true);

        assert!(!filter.take_key(0x04, false));
        assert!(filter.take_key(0x04, true));
    }

    #[test]
    fn keys_do_not_borrow_each_others_echoes() {
        let mut filter = EchoFilter::default();
        filter.expect_key(0x04, true);

        assert!(!filter.take_key(0x05, true));
    }

    /// The slot arithmetic has to hold at both ends of the usage range.
    #[test]
    fn every_usage_has_a_slot_of_its_own() {
        let mut filter = EchoFilter::default();
        for usage in [0x00, 0x01, 0x7F, 0xFE, 0xFF] {
            for pressed in [true, false] {
                filter.expect_key(usage, pressed);
                assert!(filter.take_key(usage, pressed));
                assert!(!filter.take_key(usage, pressed));
            }
        }
    }

    #[test]
    fn a_held_key_repeating_is_matched_echo_for_echo() {
        let mut filter = EchoFilter::default();
        for _ in 0..3 {
            filter.expect_key(0x04, true);
        }

        for _ in 0..3 {
            assert!(filter.take_key(0x04, true));
        }
        assert!(!filter.take_key(0x04, true));
    }

    #[test]
    fn a_backlog_of_lost_echoes_cannot_grow_without_bound() {
        let mut filter = EchoFilter::default();
        for _ in 0..100 {
            filter.expect_key(0x04, true);
        }

        let matched = (0..100).filter(|_| filter.take_key(0x04, true)).count();

        assert_eq!(matched, usize::from(MAX_PENDING));
    }

    #[test]
    fn an_echo_that_never_arrived_stops_swallowing_a_real_key_press() {
        let mut filter = EchoFilter::default();
        let sent = start();
        filter.expect_key_at(0x04, true, sent);

        let late = sent + ECHO_LIFETIME + Duration::from_millis(1);
        assert!(!filter.take_key_at(0x04, true, late));

        // And the stale count is gone rather than waiting for the one after it.
        assert!(!filter.take_key_at(0x04, true, late));
    }

    #[test]
    fn an_echo_that_arrives_in_time_is_still_recognised() {
        let mut filter = EchoFilter::default();
        let sent = start();
        filter.expect_key_at(0x04, true, sent);

        assert!(filter.take_key_at(0x04, true, sent + Duration::from_millis(1)));
    }

    #[test]
    fn buttons_are_tracked_the_same_way_as_keys() {
        let mut filter = EchoFilter::default();
        filter.expect_button(MouseButtons::LEFT, true);

        assert!(!filter.take_button(MouseButtons::RIGHT, true));
        assert!(filter.take_button(MouseButtons::LEFT, true));
        assert!(!filter.take_button(MouseButtons::LEFT, true));
    }

    /// Each flag the hooks can report gets a slot, and no two share one.
    #[test]
    fn every_button_has_a_slot_of_its_own() {
        let buttons = [
            MouseButtons::LEFT,
            MouseButtons::RIGHT,
            MouseButtons::MIDDLE,
            MouseButtons::BACK,
            MouseButtons::FORWARD,
        ];

        let mut filter = EchoFilter::default();
        for button in buttons {
            filter.expect_button(button, true);
        }
        for button in buttons {
            assert!(filter.take_button(button, true), "{button:?}");
        }
    }

    /// Nothing the hooks send looks like this, and it must not be mistaken for
    /// an echo if it ever does.
    #[test]
    fn an_event_for_no_button_at_all_is_not_an_echo() {
        let mut filter = EchoFilter::default();
        filter.expect_button(MouseButtons::empty(), true);

        assert!(!filter.take_button(MouseButtons::empty(), true));
    }

    #[test]
    fn a_button_echo_expires_just_like_a_key_one() {
        let mut filter = EchoFilter::default();
        let sent = start();
        filter.expect_button_at(MouseButtons::LEFT, true, sent);

        let late = sent + ECHO_LIFETIME + Duration::from_millis(1);
        assert!(!filter.take_button_at(MouseButtons::LEFT, true, late));
    }

    #[test]
    fn clearing_forgets_everything_that_was_still_expected() {
        let mut filter = EchoFilter::default();
        filter.expect_key(0x04, true);
        filter.expect_button(MouseButtons::LEFT, true);
        filter.clear();

        assert!(!filter.take_key(0x04, true));
        assert!(!filter.take_button(MouseButtons::LEFT, true));
    }
}
