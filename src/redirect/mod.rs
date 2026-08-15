//! Turning real input into virtual input.

mod combo;
mod echo;
mod hook;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use crate::driver::Driver;
use crate::error::{Error, Result};
use crate::hid::{KeyboardReport, Modifiers, MouseButtons, MouseReport, ScanCode, modifier_of};
use combo::ComboWatcher;
use echo::EchoFilter;

pub use hook::{ButtonEvent, KeyEvent};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub keystrokes: u64,
    pub clicks: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Swallow,
    PassThrough,
}

struct Shared {
    driver: Option<Arc<Driver>>,
    keyboard_enabled: bool,
    mouse_enabled: bool,
    keyboard: KeyboardReport,
    buttons: MouseButtons,
    echo: EchoFilter,
    combo: ComboWatcher,
    stats: Stats,
}

static SHARED: OnceLock<Mutex<Shared>> = OnceLock::new();
static INSTALLED: AtomicBool = AtomicBool::new(false);

fn state() -> Option<MutexGuard<'static, Shared>> {
    let lock = SHARED.get()?;
    Some(lock.lock().unwrap_or_else(PoisonError::into_inner))
}

pub struct Engine {
    hooks: hook::HookThread,
}

impl Engine {
    pub fn install(driver: Arc<Driver>) -> Result<Self> {
        if INSTALLED.swap(true, Ordering::SeqCst) {
            return Err(Error::Hook(
                "the redirect engine is already installed".to_owned(),
            ));
        }
        let fresh = Shared {
            driver: Some(driver),
            keyboard_enabled: false,
            mouse_enabled: false,
            keyboard: KeyboardReport::EMPTY,
            buttons: MouseButtons::empty(),
            echo: EchoFilter::default(),
            combo: ComboWatcher::default(),
            stats: Stats::default(),
        };
        match state() {
            Some(mut existing) => *existing = fresh,
            None => {
                let _ = SHARED.set(Mutex::new(fresh));
            }
        }
        match hook::HookThread::spawn() {
            Ok(hooks) => Ok(Self { hooks }),
            Err(error) => {
                INSTALLED.store(false, Ordering::SeqCst);
                Err(error)
            }
        }
    }

    pub fn set_keyboard(&self, enabled: bool) {
        let driver = self
            .update(|shared| apply_keyboard(shared, enabled))
            .flatten();
        release_keyboard_waiting(driver);
    }

    pub fn set_mouse(&self, enabled: bool) {
        let driver = self.update(|shared| apply_mouse(shared, enabled)).flatten();
        release_mouse_waiting(driver);
    }

    pub fn toggle_keyboard(&self) -> bool {
        let (enabled, driver) = self
            .update(|shared| {
                let enabled = !shared.keyboard_enabled;
                (enabled, apply_keyboard(shared, enabled))
            })
            .unwrap_or((false, None));
        release_keyboard_waiting(driver);
        enabled
    }

    pub fn toggle_mouse(&self) -> bool {
        let (enabled, driver) = self
            .update(|shared| {
                let enabled = !shared.mouse_enabled;
                (enabled, apply_mouse(shared, enabled))
            })
            .unwrap_or((false, None));
        release_mouse_waiting(driver);
        enabled
    }

    #[must_use]
    pub fn is_keyboard_enabled(&self) -> bool {
        self.read(|shared| shared.keyboard_enabled).unwrap_or(false)
    }

    #[must_use]
    pub fn is_mouse_enabled(&self) -> bool {
        self.read(|shared| shared.mouse_enabled).unwrap_or(false)
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        self.read(|shared| shared.stats).unwrap_or_default()
    }

    pub fn stop(&self) {
        self.set_keyboard(false);
        self.set_mouse(false);
    }

    pub fn release_driver(&self) {
        self.stop();
        let _ = self.update(|shared| shared.driver = None);
    }

    fn read<T>(&self, reader: impl FnOnce(&Shared) -> T) -> Option<T> {
        state().map(|shared| reader(&shared))
    }

    fn update<T>(&self, updater: impl FnOnce(&mut Shared) -> T) -> Option<T> {
        state().map(|mut shared| updater(&mut shared))
    }
}

fn apply_keyboard(shared: &mut Shared, enabled: bool) -> Option<Arc<Driver>> {
    shared.keyboard_enabled = enabled;
    if enabled {
        return None;
    }
    shared.keyboard.clear();
    shared.combo.clear();
    shared.echo.clear_keys();
    shared.driver.clone()
}

fn apply_mouse(shared: &mut Shared, enabled: bool) -> Option<Arc<Driver>> {
    shared.mouse_enabled = enabled;
    if enabled {
        return None;
    }
    shared.buttons = MouseButtons::empty();
    shared.echo.clear_buttons();
    shared.driver.clone()
}

fn release_keyboard_waiting(driver: Option<Arc<Driver>>) {
    if let Some(driver) = driver {
        let _ = driver.release_keyboard_waiting();
    }
}

fn release_mouse_waiting(driver: Option<Arc<Driver>>) {
    if let Some(driver) = driver {
        let _ = driver.release_mouse_waiting();
    }
}

fn release_keyboard_without_waiting(driver: Option<Arc<Driver>>) {
    if let Some(driver) = driver {
        let _ = driver.send_keyboard(KeyboardReport::EMPTY);
    }
}

fn release_mouse_without_waiting(driver: Option<Arc<Driver>>) {
    if let Some(driver) = driver {
        let _ = driver.send_mouse(MouseReport::EMPTY);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
        self.hooks.stop();
        INSTALLED.store(false, Ordering::SeqCst);
    }
}

pub fn emergency_stop() {
    let driver = {
        let Some(mut shared) = state() else {
            return;
        };
        shared.keyboard_enabled = false;
        shared.mouse_enabled = false;
        shared.keyboard.clear();
        shared.buttons = MouseButtons::empty();
        shared.combo.clear();
        shared.echo.clear();
        shared.driver.take()
    };
    if let Some(driver) = driver {
        driver.park();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyOutcome {
    Pass,
    Repeat(KeyboardReport),
    Send(KeyboardReport),
}

fn decide_key(
    shared: &mut Shared,
    usage: u8,
    pressed: bool,
    still_held: impl FnOnce(Modifiers) -> Modifiers,
) -> KeyOutcome {
    if shared.echo.take_key(usage, pressed) {
        return KeyOutcome::Pass;
    }

    shared.combo.note(usage, pressed);

    if modifier_of(usage).is_some() {
        return KeyOutcome::Pass;
    }

    let mut report = shared.keyboard;
    if pressed {
        if shared.combo.press_belongs_to_shortcut(still_held) || !report.press(usage) {
            return KeyOutcome::Pass;
        }
    } else if report.holds(usage) {
        report.release(usage);
    } else {
        return KeyOutcome::Pass;
    }

    if pressed && report == shared.keyboard {
        return KeyOutcome::Repeat(report);
    }

    KeyOutcome::Send(report)
}

fn commit_key(shared: &mut Shared, report: KeyboardReport, usage: u8, pressed: bool) {
    shared.keyboard = report;
    shared.echo.expect_key(usage, pressed);
    if pressed {
        shared.stats.keystrokes += 1;
    }
}

fn on_key(event: KeyEvent) -> Decision {
    let Some(mut shared) = state() else {
        return Decision::PassThrough;
    };
    if !shared.keyboard_enabled {
        return Decision::PassThrough;
    }
    let Some(driver) = shared.driver.clone() else {
        return Decision::PassThrough;
    };
    let key = ScanCode::new(event.scan_code, event.extended);
    let Some(usage) = key.hid_usage() else {
        return Decision::PassThrough;
    };

    match decide_key(&mut shared, usage, event.pressed, hook::still_held) {
        KeyOutcome::Pass => Decision::PassThrough,
        KeyOutcome::Repeat(report) => {
            drop(shared);
            let mut release_report = report;
            release_report.release(usage);
            let _ = driver.send_keyboard(release_report);
            if driver.send_keyboard(report).is_err() {
                return Decision::PassThrough;
            }
            if let Some(mut shared) = state() {
                shared.echo.expect_key(usage, false);
                shared.echo.expect_key(usage, true);
                shared.stats.keystrokes += 1;
            }
            Decision::Swallow
        }
        KeyOutcome::Send(report) => {
            drop(shared);
            if driver.send_keyboard(report).is_err() {
                return Decision::PassThrough;
            }
            remember_key(&driver, report, usage, event.pressed);
            Decision::Swallow
        }
    }
}

fn remember_key(driver: &Arc<Driver>, report: KeyboardReport, usage: u8, pressed: bool) {
    let undo = {
        let Some(mut shared) = state() else {
            return release_keyboard_without_waiting(Some(Arc::clone(driver)));
        };
        if shared.keyboard_enabled {
            commit_key(&mut shared, report, usage, pressed);
            None
        } else {
            Some(Arc::clone(driver))
        }
    };
    release_keyboard_without_waiting(undo);
}

fn decide_button(shared: &mut Shared, event: ButtonEvent) -> Option<MouseButtons> {
    if shared.echo.take_button(event.button, event.pressed) {
        return None;
    }
    let mut buttons = shared.buttons;
    if event.pressed {
        buttons.insert(event.button);
    } else if buttons.contains(event.button) {
        buttons.remove(event.button);
    } else {
        return None;
    }
    Some(buttons)
}

fn commit_button(shared: &mut Shared, buttons: MouseButtons, event: ButtonEvent) {
    shared.buttons = buttons;
    shared.echo.expect_button(event.button, event.pressed);
    if event.pressed {
        shared.stats.clicks += 1;
    }
}

fn on_button(event: ButtonEvent) -> Decision {
    let Some(mut shared) = state() else {
        return Decision::PassThrough;
    };
    if !shared.mouse_enabled {
        return Decision::PassThrough;
    }
    let Some(driver) = shared.driver.clone() else {
        return Decision::PassThrough;
    };
    let Some(buttons) = decide_button(&mut shared, event) else {
        return Decision::PassThrough;
    };
    drop(shared);
    if driver.send_mouse(MouseReport::buttons(buttons)).is_err() {
        return Decision::PassThrough;
    }
    remember_button(&driver, buttons, event);
    Decision::Swallow
}

fn remember_button(driver: &Arc<Driver>, buttons: MouseButtons, event: ButtonEvent) {
    let undo = {
        let Some(mut shared) = state() else {
            return release_mouse_without_waiting(Some(Arc::clone(driver)));
        };
        if shared.mouse_enabled {
            commit_button(&mut shared, buttons, event);
            None
        } else {
            Some(Arc::clone(driver))
        }
    };
    release_mouse_without_waiting(undo);
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: u8 = 0x04;
    const KEY_B: u8 = 0x05;
    const LEFT_CTRL: u8 = 0xE0;

    fn session() -> Shared {
        Shared {
            driver: None,
            keyboard_enabled: true,
            mouse_enabled: true,
            keyboard: KeyboardReport::EMPTY,
            buttons: MouseButtons::empty(),
            echo: EchoFilter::default(),
            combo: ComboWatcher::default(),
            stats: Stats::default(),
        }
    }

    fn nothing_held(_believed: Modifiers) -> Modifiers {
        Modifiers::empty()
    }

    fn press(shared: &mut Shared, usage: u8, pressed: bool) -> KeyOutcome {
        let outcome = decide_key(shared, usage, pressed, nothing_held);
        if let KeyOutcome::Send(report) = outcome {
            commit_key(shared, report, usage, pressed);
        }
        outcome
    }

    fn echo_of(shared: &mut Shared, usage: u8, pressed: bool) -> KeyOutcome {
        decide_key(shared, usage, pressed, nothing_held)
    }

    fn click(shared: &mut Shared, button: MouseButtons, pressed: bool) -> Option<MouseButtons> {
        let event = ButtonEvent { button, pressed };
        let decided = decide_button(shared, event);
        if let Some(buttons) = decided {
            commit_button(shared, buttons, event);
        }
        decided
    }

    #[test]
    fn an_ordinary_key_is_sent_and_its_release_lets_it_go() {
        let mut shared = session();
        let KeyOutcome::Send(down) = press(&mut shared, KEY_A, true) else {
            panic!("the press should have been sent");
        };
        assert!(down.holds(KEY_A));
        echo_of(&mut shared, KEY_A, true);
        let KeyOutcome::Send(up) = press(&mut shared, KEY_A, false) else {
            panic!("the release should have been sent");
        };
        assert!(!up.holds(KEY_A));
    }

    #[test]
    fn our_own_echo_is_let_through_rather_than_sent_again() {
        let mut shared = session();
        press(&mut shared, KEY_A, true);
        assert_eq!(echo_of(&mut shared, KEY_A, true), KeyOutcome::Pass);
    }

    #[test]
    fn a_key_held_while_a_modifier_is_tapped_is_still_released_on_the_device() {
        let mut shared = session();
        press(&mut shared, KEY_A, true);
        echo_of(&mut shared, KEY_A, true);
        press(&mut shared, LEFT_CTRL, true);
        assert_eq!(
            decide_key(&mut shared, KEY_A, true, |_| Modifiers::LEFT_CTRL),
            KeyOutcome::Pass,
            "a repeat under a modifier belongs to the shortcut"
        );
        press(&mut shared, LEFT_CTRL, false);
        let KeyOutcome::Send(up) = press(&mut shared, KEY_A, false) else {
            panic!("the release must reach the virtual keyboard, or the key stays down");
        };
        assert!(!up.holds(KEY_A));
    }

    #[test]
    fn the_release_of_a_key_the_device_never_pressed_goes_to_windows() {
        let mut shared = session();
        assert_eq!(press(&mut shared, KEY_B, false), KeyOutcome::Pass);
    }

    #[test]
    fn a_repeat_of_a_held_key_returns_repeat_outcome() {
        let mut shared = session();
        press(&mut shared, KEY_A, true);
        echo_of(&mut shared, KEY_A, true);
        assert!(matches!(
            press(&mut shared, KEY_A, true),
            KeyOutcome::Repeat(_)
        ));
    }

    #[test]
    fn modifiers_are_always_left_to_windows() {
        assert_eq!(press(&mut session(), LEFT_CTRL, true), KeyOutcome::Pass);
        assert_eq!(press(&mut session(), LEFT_CTRL, false), KeyOutcome::Pass);
    }

    #[test]
    fn the_seventh_key_and_its_release_both_go_to_windows() {
        let mut shared = session();
        for usage in 0x04..0x0A {
            let KeyOutcome::Send(_) = press(&mut shared, usage, true) else {
                panic!("{usage:#04X} should have been sent");
            };
            echo_of(&mut shared, usage, true);
        }
        let seventh = 0x0A;
        assert_eq!(press(&mut shared, seventh, true), KeyOutcome::Pass);
        assert_eq!(press(&mut shared, seventh, false), KeyOutcome::Pass);
    }

    #[test]
    fn a_click_is_sent_and_its_release_lets_the_button_go() {
        let mut shared = session();
        assert_eq!(
            click(&mut shared, MouseButtons::LEFT, true),
            Some(MouseButtons::LEFT)
        );
        click(&mut shared, MouseButtons::LEFT, true);
        assert_eq!(
            click(&mut shared, MouseButtons::LEFT, false),
            Some(MouseButtons::empty())
        );
    }

    #[test]
    fn the_release_of_a_button_the_device_never_pressed_goes_to_windows() {
        let mut shared = session();
        assert_eq!(click(&mut shared, MouseButtons::LEFT, false), None);
    }

    #[test]
    fn releasing_one_button_leaves_the_others_held() {
        let mut shared = session();
        click(&mut shared, MouseButtons::LEFT, true);
        click(&mut shared, MouseButtons::LEFT, true);
        click(&mut shared, MouseButtons::RIGHT, true);
        click(&mut shared, MouseButtons::RIGHT, true);
        assert_eq!(
            click(&mut shared, MouseButtons::LEFT, false),
            Some(MouseButtons::RIGHT)
        );
    }

    #[test]
    fn the_counters_follow_what_the_devices_were_told() {
        let mut shared = session();
        press(&mut shared, KEY_A, true);
        echo_of(&mut shared, KEY_A, true);
        press(&mut shared, KEY_A, false);
        click(&mut shared, MouseButtons::LEFT, true);
        assert_eq!(shared.stats.keystrokes, 1);
        assert_eq!(shared.stats.clicks, 1);
    }

    struct Keyboard {
        held: [bool; 256],
    }

    impl Keyboard {
        fn new() -> Self {
            Self { held: [false; 256] }
        }

        fn modifiers(&self) -> Modifiers {
            let mut held = Modifiers::empty();
            for usage in 0..=u8::MAX {
                if self.held[usize::from(usage)] {
                    if let Some(modifier) = modifier_of(usage) {
                        held.insert(modifier);
                    }
                }
            }
            held
        }

        fn keys_down(&self) -> impl Iterator<Item = u8> + '_ {
            (0..=u8::MAX).filter(|usage| self.held[usize::from(*usage)])
        }
    }

    fn hook_event(shared: &mut Shared, keyboard: &Keyboard, usage: u8, pressed: bool) {
        let live = keyboard.modifiers();
        let outcome = decide_key(shared, usage, pressed, |believed| believed & live);
        if let KeyOutcome::Send(report) = outcome {
            commit_key(shared, report, usage, pressed);
            assert_eq!(
                decide_key(shared, usage, pressed, |believed| believed & live),
                KeyOutcome::Pass,
                "our own echo must be let through"
            );
        }
    }

    struct Rng(u64);

    impl Rng {
        fn below(&mut self, bound: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % bound
        }
    }

    const ALPHABET: [u8; 12] = [
        0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0xE0, 0xE4, 0xE1, 0xE5,
    ];

    #[test]
    fn no_stream_of_keys_can_leave_one_held_on_the_device() {
        for seed in 0..256 {
            let mut rng = Rng(seed);
            let mut shared = session();
            let mut keyboard = Keyboard::new();
            for _ in 0..400 {
                let usage = ALPHABET[rng.below(ALPHABET.len() as u64) as usize];
                let pressed = rng.below(3) != 0;
                keyboard.held[usize::from(usage)] = pressed;
                hook_event(&mut shared, &keyboard, usage, pressed);
                for held in 0..=u8::MAX {
                    assert!(
                        !shared.keyboard.holds(held) || keyboard.held[usize::from(held)],
                        "seed {seed}: the device holds {held:#04X} and the user does not"
                    );
                }
            }
            let down: Vec<u8> = keyboard.keys_down().collect();
            for usage in down.iter().copied().filter(|u| modifier_of(*u).is_some()) {
                keyboard.held[usize::from(usage)] = false;
                hook_event(&mut shared, &keyboard, usage, false);
            }
            for usage in down.iter().copied().filter(|u| modifier_of(*u).is_none()) {
                keyboard.held[usize::from(usage)] = false;
                hook_event(&mut shared, &keyboard, usage, false);
            }
            assert_eq!(
                shared.keyboard,
                KeyboardReport::EMPTY,
                "seed {seed}: a key was left held on the virtual keyboard"
            );
        }
    }
}
