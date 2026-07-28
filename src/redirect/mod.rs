//! Turning real input into virtual input.
//!
//! The low-level hooks run on their own thread and decide, for every event,
//! whether to swallow it and repeat it through the driver or to let it pass.
//! All the state that decision needs lives in [`Shared`] behind one lock, so
//! the hook callbacks stay short and there is one place to reason about.

mod combo;
mod echo;
mod hook;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use crate::driver::Driver;
use crate::error::{Error, Result};
use crate::hid::{modifier_of, KeyboardReport, Modifiers, MouseButtons, MouseReport, ScanCode};

use combo::ComboWatcher;
use echo::EchoFilter;
pub use hook::{ButtonEvent, KeyEvent};

/// What the dashboard shows about the current session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub keystrokes: u64,
    pub clicks: u64,
}

/// What a hook callback should do with the event it was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Hide the real event; the virtual device has already repeated it.
    Swallow,
    /// Let Windows see the real event.
    PassThrough,
}

struct Shared {
    /// Held only while there is something to redirect to. The hooks reach the
    /// driver from their own thread, so this reference outlives any local one
    /// and has to be given back explicitly before the driver can be taken
    /// apart.
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

/// Whether an engine currently owns the shared state. The state is a
/// process-wide singleton, so a second engine would reset the first one's
/// counters and leave its hooks pointing at state it no longer owns.
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// The shared state, whether or not some thread panicked while holding it.
///
/// A poisoned lock would otherwise take the whole program down through a hook
/// callback, which is the one place where a panic must not happen.
fn state() -> Option<MutexGuard<'static, Shared>> {
    let lock = SHARED.get()?;

    Some(lock.lock().unwrap_or_else(PoisonError::into_inner))
}

/// The installed hooks and the state behind them.
pub struct Engine {
    hooks: hook::HookThread,
}

impl Engine {
    /// Installs the hooks. They stay installed, doing nothing, until one of the
    /// two redirects is switched on.
    ///
    /// There can only be one engine at a time; a second attempt is turned down
    /// rather than quietly taking the first one's state away.
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
            // A later run in the same process reuses the slot: OnceLock cannot
            // be reset, and the previous engine has been dropped by now.
            Some(mut existing) => *existing = fresh,
            None => {
                let _ = SHARED.set(Mutex::new(fresh));
            }
        }

        match hook::HookThread::spawn() {
            Ok(hooks) => Ok(Self { hooks }),
            Err(error) => {
                // Nothing was installed after all, so the next attempt must not
                // be turned away.
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

    /// Flips one redirect and reports its new state.
    ///
    /// Reading the flag and writing it back are one visit to the lock: two
    /// visits could disagree if a menu choice and a hotkey arrive together.
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

    /// Stops everything and gives the driver back, leaving this the last owner
    /// of it. Needed before the driver can be removed from the system.
    pub fn release_driver(&self) {
        self.stop();
        let _ = self.update(|shared| shared.driver = None);
    }

    /// `None` means there is no state to read, which is not the same answer as
    /// a default value: the caller decides what standing in for it should be.
    fn read<T>(&self, reader: impl FnOnce(&Shared) -> T) -> Option<T> {
        state().map(|shared| reader(&shared))
    }

    fn update<T>(&self, updater: impl FnOnce(&mut Shared) -> T) -> Option<T> {
        state().map(|mut shared| updater(&mut shared))
    }
}

/// Switches the keyboard redirect and, when switching it off, hands back the
/// driver that still has keys held down on it.
///
/// Sending the empty report is left to the caller, after the lock is released:
/// the hooks must never wait on a driver call.
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

/// Lets the keys go from the interface thread, where the connection is worth
/// waiting for: nothing sends another report afterwards, so a report given up
/// on here is a key held down for the rest of the session.
fn release_keyboard_waiting(driver: Option<Arc<Driver>>) {
    if let Some(driver) = driver {
        // A failure here means the connection is closed, which happens only
        // while the bus is being rebuilt - and that takes the devices with it.
        let _ = driver.release_keyboard_waiting();
    }
}

fn release_mouse_waiting(driver: Option<Arc<Driver>>) {
    if let Some(driver) = driver {
        let _ = driver.release_mouse_waiting();
    }
}

/// The same, from a hook callback, where waiting is not allowed: Windows drops
/// a low-level hook that takes too long, and that would cost every later key.
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

/// Puts the keyboard and the mouse back the way they were, from any thread.
///
/// The console control handler runs on a thread Windows injects while the
/// window is closing, and it cannot wait for owners to be dropped in order:
/// there is no unwinding on that path at all.
pub fn emergency_stop() {
    // The driver is taken out from under the lock and parked only once it is
    // let go: `park` sends to the driver and takes the connection lock, and the
    // hooks must never be left waiting on the state lock while that happens.
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

/// What is to be done with one key, before anything is sent.
///
/// Kept apart from [`on_key`] so the rules can be exercised without a driver:
/// every one of them exists because of a way a key once got stuck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyOutcome {
    /// Let Windows have it.
    Pass,
    /// Swallow it, but send nothing: the device already reports exactly this.
    AlreadyReported,
    /// Swallow it and send this report.
    Send(KeyboardReport),
}

/// Decides one key against everything the session knows.
///
/// `still_held` is handed the modifiers the watcher believes are down and
/// answers with the ones Windows agrees about. It is only consulted when the
/// watcher believes something is down - see
/// [`ComboWatcher::press_belongs_to_shortcut`].
fn decide_key(
    shared: &mut Shared,
    usage: u8,
    pressed: bool,
    still_held: impl FnOnce(Modifiers) -> Modifiers,
) -> KeyOutcome {
    // Our own virtual keyboard reports back through the same hook. Letting the
    // echo through is what keeps the two devices from feeding each other.
    if shared.echo.take_key(usage, pressed) {
        return KeyOutcome::Pass;
    }

    // The watcher is told about every key, before anything else looks at it, so
    // that what it believes about held modifiers stays true.
    shared.combo.note(usage, pressed);

    // Modifiers themselves are never rebuilt on the virtual keyboard; only the
    // watcher above needs to know they are held.
    if modifier_of(usage).is_some() {
        return KeyOutcome::Pass;
    }

    let mut report = shared.keyboard;
    if pressed {
        // Shortcuts are left to Windows: a combination assembled from two
        // devices at once is what made them unreliable in the first place. And
        // a full report has no slot left to give.
        if shared.combo.press_belongs_to_shortcut(still_held) || !report.press(usage) {
            return KeyOutcome::Pass;
        }
    } else if report.holds(usage) {
        // Let go of exactly what the virtual keyboard is holding.
        report.release(usage);
    } else {
        // A key it never sent - one passed through under a modifier, or dropped
        // for want of a slot - has its release go to Windows too, in step with
        // its press. Deciding this by what the device holds, rather than by what
        // the watcher recorded, is what keeps a key from being stranded down
        // when a modifier is tapped mid-press.
        return KeyOutcome::Pass;
    }

    // Windows repeats a held key by sending the press again, and the report for
    // a key the device already holds is the one it is already reporting.
    // Sending it changes nothing on the wire, so no echo ever comes back - and
    // an echo that is expected but never arrives is spent on the next real
    // press of that key instead, letting it through as the physical keyboard's.
    if report == shared.keyboard {
        return KeyOutcome::AlreadyReported;
    }

    KeyOutcome::Send(report)
}

/// Writes down a report that has been sent.
fn commit_key(shared: &mut Shared, report: KeyboardReport, usage: u8, pressed: bool) {
    shared.keyboard = report;
    shared.echo.expect_key(usage, pressed);
    if pressed {
        shared.stats.keystrokes += 1;
    }
}

/// Called from the keyboard hook, on the hook thread.
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

    let report = match decide_key(&mut shared, usage, event.pressed, hook::still_held) {
        KeyOutcome::Pass => return Decision::PassThrough,
        KeyOutcome::AlreadyReported => return Decision::Swallow,
        KeyOutcome::Send(report) => report,
    };

    // The lock is dropped before the driver is asked to repeat the key. This
    // runs inside a low-level hook, and everything the hook waits for is time
    // taken from the whole system's input.
    drop(shared);
    if driver.send_keyboard(report).is_err() {
        return Decision::PassThrough;
    }

    remember_key(&driver, report, usage, event.pressed);

    Decision::Swallow
}

/// Writes down what was just sent - or, if the redirect was switched off while
/// the key was on its way, releases it again, so a key can never outlive the
/// redirect that sent it.
///
/// The driver is the one the report went out on, not whatever the shared state
/// holds now: `emergency_stop` takes that field, and a key sent just before it
/// would find nothing there to be released on.
fn remember_key(driver: &Arc<Driver>, report: KeyboardReport, usage: u8, pressed: bool) {
    let undo = {
        let Some(mut shared) = state() else {
            // No state at all means the engine is being taken down around us,
            // and the key just sent is still held.
            return release_keyboard_without_waiting(Some(Arc::clone(driver)));
        };

        if shared.keyboard_enabled {
            commit_key(&mut shared, report, usage, pressed);
            None
        } else {
            // Switched off between the send above and this point. The
            // switch-off released everything it knew about, and the key just
            // sent is not among those.
            Some(Arc::clone(driver))
        }
    };

    release_keyboard_without_waiting(undo);
}

/// Decides one button, the way [`decide_key`] decides one key. `None` means the
/// event belongs to Windows.
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
        // A button the virtual mouse is not holding - one whose press went to
        // Windows because the redirect was off, or because the driver refused
        // it - has its release go to Windows as well, in step with its press.
        // Swallowing it would leave the application in a click it never saw end.
        return None;
    }

    Some(buttons)
}

/// Writes down a button report that has been sent.
fn commit_button(shared: &mut Shared, buttons: MouseButtons, event: ButtonEvent) {
    shared.buttons = buttons;
    shared.echo.expect_button(event.button, event.pressed);
    if event.pressed {
        shared.stats.clicks += 1;
    }
}

/// Called from the mouse hook, on the hook thread.
///
/// Only buttons are redirected: pointer movement already works, and repeating
/// it through the driver only adds jitter.
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

/// The mouse half of [`remember_key`], down to which driver the undo goes to.
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

    /// A session with both redirects on and nothing held anywhere. No driver:
    /// none of the rules under test reach one.
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

    /// Nothing is held on the real keyboard - the ordinary case.
    fn nothing_held(_believed: Modifiers) -> Modifiers {
        Modifiers::empty()
    }

    /// One key through the whole loop the hook runs: decide, then write down
    /// what was sent, exactly as `on_key` does once the driver has taken it.
    fn press(shared: &mut Shared, usage: u8, pressed: bool) -> KeyOutcome {
        let outcome = decide_key(shared, usage, pressed, nothing_held);
        if let KeyOutcome::Send(report) = outcome {
            commit_key(shared, report, usage, pressed);
        }
        outcome
    }

    /// The virtual keyboard reporting back what it was just told to send.
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

    /// The stuck-key bug this rule exists for: a key held while a modifier is
    /// tapped used to have its release decided by the combo watcher, which by
    /// then believed the key had gone to Windows - so the virtual keyboard was
    /// never told to let go, and the key repeated forever.
    #[test]
    fn a_key_held_while_a_modifier_is_tapped_is_still_released_on_the_device() {
        let mut shared = session();

        press(&mut shared, KEY_A, true);
        echo_of(&mut shared, KEY_A, true);

        // A modifier goes down and up while the key stays held.
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

    /// A key the virtual keyboard never pressed has to have its release follow
    /// its press to Windows, or the application never sees the key come up.
    #[test]
    fn the_release_of_a_key_the_device_never_pressed_goes_to_windows() {
        let mut shared = session();

        assert_eq!(press(&mut shared, KEY_B, false), KeyOutcome::Pass);
    }

    /// An auto-repeat would produce the very report the device is already
    /// sending, and record an echo that never arrives - which the next real
    /// press of that key would then be spent on.
    #[test]
    fn a_repeat_of_a_held_key_sends_nothing_and_is_still_swallowed() {
        let mut shared = session();
        press(&mut shared, KEY_A, true);
        echo_of(&mut shared, KEY_A, true);

        assert_eq!(
            press(&mut shared, KEY_A, true),
            KeyOutcome::AlreadyReported,
            "the report has not changed, so there is nothing to send"
        );
    }

    /// A modifier is never rebuilt on the virtual keyboard, whichever it is.
    #[test]
    fn modifiers_are_always_left_to_windows() {
        let mut shared = session();

        assert_eq!(press(&mut shared, LEFT_CTRL, true), KeyOutcome::Pass);
        assert_eq!(press(&mut shared, LEFT_CTRL, false), KeyOutcome::Pass);
    }

    /// Six is all a boot keyboard report can hold; the seventh has to reach
    /// Windows, and so does its release.
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
        click(&mut shared, MouseButtons::LEFT, true); // the echo
        assert_eq!(
            click(&mut shared, MouseButtons::LEFT, false),
            Some(MouseButtons::empty())
        );
    }

    /// The mouse half of the stuck-key rule: a button pressed while the
    /// redirect was off reached the application, so its release has to reach
    /// the application too.
    #[test]
    fn the_release_of_a_button_the_device_never_pressed_goes_to_windows() {
        let mut shared = session();

        assert_eq!(click(&mut shared, MouseButtons::LEFT, false), None);
    }

    /// One button going up must not take the others down with it.
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

    /// Only presses are counted, and only the ones that were really sent.
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
}
