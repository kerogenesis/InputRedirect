//! Recognising our own input coming back.
//!
//! The virtual keyboard and mouse are real HID devices, so everything they
//! report arrives at the hooks a moment later. Each injected event is counted
//! here and the matching echo is let through untouched; without this the first
//! key press would loop forever.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::hid::MouseButtons;

/// How many echoes may pile up for one key before the oldest are forgotten.
/// A backlog can only mean the echo never arrived - a lost event is better
/// than a key that stays swallowed.
const MAX_PENDING: u8 = 8;

/// How long an expected echo stays expected. The driver answers in about a
/// millisecond; anything still waiting after this never arrived, and holding on
/// to it would swallow a real key press the user makes later. The cap above is
/// not enough on its own: a key pressed once and lost keeps its single count
/// until the same key is pressed again, which may be minutes later.
const ECHO_LIFETIME: Duration = Duration::from_millis(50);

/// The echoes still owed for one key or button, and when they stop counting.
#[derive(Clone, Copy, Debug)]
struct Pending {
    count: u8,
    expires_at: Instant,
}

type Counters = HashMap<(u8, bool), Pending>;

#[derive(Debug, Default)]
pub struct EchoFilter {
    keys: Counters,
    buttons: Counters,
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
    /// what it was owed: clearing both would drop echoes the other one is still
    /// waiting for, and every dropped echo is an event let through as the real
    /// device's.
    pub fn clear_keys(&mut self) {
        self.keys.clear();
    }

    pub fn clear_buttons(&mut self) {
        self.buttons.clear();
    }

    // The clock is passed in below so the lifetime above can be tested without
    // the tests having to sleep through it.

    fn expect_key_at(&mut self, usage: u8, pressed: bool, now: Instant) {
        increment(&mut self.keys, (usage, pressed), now);
    }

    fn expect_button_at(&mut self, button: MouseButtons, pressed: bool, now: Instant) {
        increment(&mut self.buttons, (button.bits(), pressed), now);
    }

    fn take_key_at(&mut self, usage: u8, pressed: bool, now: Instant) -> bool {
        decrement(&mut self.keys, (usage, pressed), now)
    }

    fn take_button_at(&mut self, button: MouseButtons, pressed: bool, now: Instant) -> bool {
        decrement(&mut self.buttons, (button.bits(), pressed), now)
    }
}

/// Each new injection pushes the deadline out, so a held key repeating stays
/// expected for as long as it repeats.
fn increment(counters: &mut Counters, key: (u8, bool), now: Instant) {
    let expires_at = now + ECHO_LIFETIME;
    let pending = counters.entry(key).or_insert(Pending {
        count: 0,
        expires_at,
    });

    pending.count = pending.count.saturating_add(1).min(MAX_PENDING);
    pending.expires_at = expires_at;
}

fn decrement(counters: &mut Counters, key: (u8, bool), now: Instant) -> bool {
    // Checked before taking the entry out mutably: a stale count is dropped
    // whole, and the event that found it is treated as the user's own.
    if counters
        .get(&key)
        .is_some_and(|pending| pending.expires_at <= now)
    {
        counters.remove(&key);
        return false;
    }

    match counters.get_mut(&key) {
        Some(pending) if pending.count > 0 => {
            pending.count -= 1;
            if pending.count == 0 {
                counters.remove(&key);
            }
            true
        }
        _ => false,
    }
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
