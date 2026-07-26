//! Everything that talks to the signed Logitech driver stack.
//!
//! The rest of the program only sees [`Driver`]: connect to it, send reports,
//! drop it. Where it lives on disk, which services carry it and which IOCTL
//! moves a byte into the kernel stays inside this module.

mod device;
mod ghub;
mod holders;
mod install;
mod ioctl;
mod payload;
mod process;
mod reboot;
mod service;
mod version;

pub use payload::ExtractedDrivers;
pub use reboot::{
    clear_restart_pending, is_restart_pending, mark_restart_pending, request_restart,
};

use std::borrow::Cow;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::hid::{KeyboardReport, MouseReport};

use device::{Devices, VirtualDeviceId};

/// How long the bus is given to finish taking the child devices away, and how
/// often to look.
///
/// This is a wait for something that can be seen - the devices disappearing
/// from the bus - rather than a pause in the hope that it happened. A pause is
/// a guess about a machine we are not running on, and the request that follows
/// it is refused with an invalid parameter that explains nothing.
const REMOVAL_TIMEOUT: Duration = Duration::from_secs(3);
const REMOVAL_POLL: Duration = Duration::from_millis(50);

/// How long the driver is given to publish its device again after the bus has
/// been rebuilt. Plug and play has to build the whole stack anew, which takes
/// noticeably longer than a removal.
const REOPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// How many times a refused plug is worth repeating, and how long to wait in
/// between. A refusal is nearly always the bus still catching up.
const PLUG_ATTEMPTS: u32 = 4;
const RETRY_PAUSE: Duration = Duration::from_millis(300);

/// A step worth telling the user about. The driver layer decides what happened,
/// the interface decides how it looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    PreparingFiles,
    ClosingLogitechSoftware,
    InstallingDriver,
    /// Carries both builds, because replacing a driver behind someone's back is
    /// exactly the kind of thing they are entitled to read the details of.
    ReplacingDriver(version::Mismatch),
    DriverReady,
    CleaningPreviousSession,
    RebuildingBus,
    CreatingVirtualDevices,
    KeyboardReady,
    MouseReady,
    Connected,
}

impl fmt::Display for Step {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text: Cow<'static, str> = match self {
            Self::PreparingFiles => "Preparing the driver files".into(),
            Self::ClosingLogitechSoftware => {
                "Closing Logitech G HUB, which wants the same virtual devices".into()
            }
            Self::InstallingDriver => {
                "Installing the Logitech driver, this only happens once".into()
            }
            Self::ReplacingDriver(mismatch) => format!(
                "Replacing the installed Logitech driver, version {}, with the {} this program \
                 speaks to",
                mismatch.installed, mismatch.ours
            )
            .into(),
            Self::DriverReady => "Logitech driver is installed and up to date".into(),
            Self::CleaningPreviousSession => {
                "Cleaned up devices left behind by a previous session".into()
            }
            Self::RebuildingBus => "Rebuilding the virtual bus, this takes a few seconds".into(),
            Self::CreatingVirtualDevices => "Creating the virtual keyboard and mouse".into(),
            Self::KeyboardReady => "Virtual keyboard is in place".into(),
            Self::MouseReady => "Virtual mouse is in place".into(),
            Self::Connected => "Connected to the driver".into(),
        };

        formatter.write_str(&text)
    }
}

/// Where progress messages go while the driver is being brought up.
pub trait Report {
    fn step(&mut self, step: Step);
}

impl<F: FnMut(Step)> Report for F {
    fn step(&mut self, step: Step) {
        self(step);
    }
}

/// What the interface shows in its status line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub connected: bool,
    pub virtual_keyboard: bool,
    pub virtual_mouse: bool,
}

/// The two devices the driver created for this session.
#[derive(Default)]
struct PluggedDevices {
    keyboard: Option<VirtualDeviceId>,
    mouse: Option<VirtualDeviceId>,
}

/// An open connection to the driver, with the virtual devices it created.
pub struct Driver {
    /// Behind a lock because it is replaced while the program runs: rebuilding
    /// the bus tears down the very device this connection goes through.
    devices: Mutex<Devices>,
    plugged: Mutex<PluggedDevices>,
    // Kept alive on purpose: the extracted files are removed when this is
    // dropped, and the installer may still want to read them - both on the way
    // up and every time the bus is rebuilt.
    payload: ExtractedDrivers,
    // Also kept alive on purpose, and never read: it keeps G HUB closed for as
    // long as this connection exists, and stops the moment it is dropped, so
    // nothing of ours outlives the session.
    _watchdog: ghub::Watchdog,
}

impl Driver {
    /// Installs the driver if it is missing, then opens it and creates the two
    /// virtual devices.
    pub fn connect(report: &mut dyn Report) -> Result<Self> {
        if !is_elevated() {
            return Err(Error::NotElevated);
        }

        report.step(Step::PreparingFiles);
        let payload = ExtractedDrivers::unpack()?;

        // Before the driver is touched at all: G HUB plugs its own virtual
        // keyboard and mouse with the very product ids we ask for, and a
        // product id can only be taken once. Its agent starts itself again a
        // moment after it is closed, so the watchdog keeps looking for as long
        // as this connection lives. Nothing of the user's is lost - profiles,
        // macros and lighting live in the application's own files, and are
        // applied again the next time it runs.
        if ghub::is_running() {
            report.step(Step::ClosingLogitechSoftware);
            ghub::stop();
        }
        let watchdog = ghub::Watchdog::start();

        if service::driver_installed() {
            // A build we did not read the protocol out of answers our requests
            // with an invalid parameter and no reason, which is the same thing
            // it says when a device is merely busy - so it is not something to
            // find out about later, from a failure that names nothing. Ours
            // goes in whichever way the versions differ: a protocol is not a
            // version number to be compared for greatness.
            match version::mismatch(payload.directory()) {
                None => {
                    report.step(Step::DriverReady);

                    // Re-binding the bus driver on every start is not optional:
                    // without it the first plug after a service restart fails
                    // with an invalid parameter, because the bus keeps stale
                    // state from the last session. A fresh install does this on
                    // its way through, so it is only needed on the path that
                    // skips the installer - binding twice costs a second or two
                    // of plug and play for nothing.
                    install::bind_root_device(payload.directory())?;
                }
                Some(mismatch) => {
                    report.step(Step::ReplacingDriver(mismatch));
                    install::replace(payload.directory())?;
                    report.step(Step::DriverReady);
                }
            }
        } else {
            report.step(Step::InstallingDriver);
            install::install(payload.directory())?;
            report.step(Step::DriverReady);
        }

        service::start_all();

        // The driver is present and answering, so a restart owed by an earlier
        // removal either happened or no longer applies. The flag must not
        // outlive the condition it describes.
        reboot::clear_restart_pending();

        if install::remove_leftover_devices() > 0 {
            report.step(Step::CleaningPreviousSession);

            // Windows carries the removal out in the background, and the first
            // plug must not race it.
            wait_for_empty_bus();
        }

        let devices = open_within_timeout()?;
        report.step(Step::Connected);

        let driver = Self {
            devices: Mutex::new(devices),
            plugged: Mutex::new(PluggedDevices::default()),
            payload,
            _watchdog: watchdog,
        };
        driver.create_virtual_devices(report)?;

        Ok(driver)
    }

    /// Unplugs the virtual keyboard and mouse and creates them again.
    ///
    /// This takes `&self` because the redirect engine holds the driver through
    /// an `Arc`: everything that changes here sits behind its own lock.
    pub fn recreate_virtual_devices(&self, report: &mut dyn Report) -> Result<()> {
        let refused = self.unplug_virtual_devices();

        // The clean case: the driver let both devices go and Windows took them
        // off the bus, so the next pair can be created straight away.
        if refused.is_empty() && wait_for_empty_bus() {
            return self.create_virtual_devices(report);
        }

        // A refusal means the driver still believes one of the children exists.
        // Taking the device node away with the setup API does not change its
        // mind - the bus reports the same child again on its next enumeration -
        // and the product id stays taken, which is what the next plug is
        // refused for. Only handing the bus driver back to plug and play clears
        // it, and that is exactly what a restart of this program does.
        report.step(Step::RebuildingBus);
        install::remove_leftover_devices();
        self.rebuild_bus()?;

        if !wait_for_empty_bus() {
            return Err(Error::Device(format!(
                "the virtual {refused} did not leave the bus even after rebuilding it; a restart \
                 of Windows will clear it"
            )));
        }

        self.create_virtual_devices(report)
    }

    /// Hands the bus driver back to plug and play so that it forgets the
    /// children it still thinks it has.
    ///
    /// The connection goes through the same root device, so it is closed first
    /// and opened again afterwards: plug and play builds the stack anew and the
    /// handle from before does not survive that. While it is closed, a report
    /// on its way to the driver is turned down and the real key or button goes
    /// through instead, which is the right answer for those few seconds.
    fn rebuild_bus(&self) -> Result<()> {
        self.connection().close();

        install::bind_root_device(self.payload.directory())?;
        service::start_all();

        self.reopen()
    }

    /// Opens the driver again, giving plug and play time to publish it.
    fn reopen(&self) -> Result<()> {
        *self.connection() = open_within_timeout()?;
        Ok(())
    }

    fn create_virtual_devices(&self, report: &mut dyn Report) -> Result<()> {
        report.step(Step::CreatingVirtualDevices);

        self.plug_keyboard()?;
        report.step(Step::KeyboardReady);

        // Half a pair is worse than none: the keyboard that did come up would
        // keep its place on the bus, and the next attempt would be refused for
        // the same reason this one was.
        if let Err(error) = self.plug_mouse() {
            self.unplug_virtual_devices();
            return Err(error);
        }
        report.step(Step::MouseReady);

        Ok(())
    }

    /// Each device is remembered as soon as it exists, so a failure halfway
    /// through never leaves one behind that nothing owns.
    fn plug_keyboard(&self) -> Result<()> {
        let id = self.plug_unless_leaving(Devices::plug_keyboard)?;
        self.plugged_devices().keyboard = Some(id);

        Ok(())
    }

    fn plug_mouse(&self) -> Result<()> {
        let id = self.plug_unless_leaving(Devices::plug_mouse)?;
        self.plugged_devices().mouse = Some(id);

        Ok(())
    }

    /// Creates one device, unless the program is on its way out.
    ///
    /// Closing the window runs the cleanup on a thread Windows injects, and it
    /// can arrive while this one is waiting for the bus - creating a pair takes
    /// the better part of a second. The cleanup only knows about the devices
    /// that were on the list when it looked, so one finished afterwards would
    /// be left behind on the bus, showing up in Device Manager as a Logitech
    /// device that is not there. Asking twice - before starting and again once
    /// the bus answers - is what keeps that window down to a single request.
    fn plug_unless_leaving(
        &self,
        plug: impl Fn(&Devices) -> Result<VirtualDeviceId>,
    ) -> Result<VirtualDeviceId> {
        if is_shutting_down() {
            return Err(leaving());
        }

        let id = keep_trying(|| plug(&self.connection()))?;

        if is_shutting_down() {
            let _ = self.connection().unplug(id);
            return Err(leaving());
        }

        Ok(id)
    }

    /// Asks the driver to take both virtual devices away, and names the ones it
    /// would not.
    ///
    /// The refusals used to be discarded, and that is how a device could stay
    /// on the bus while the program believed it had gone: the plug that came
    /// next asked for a product id that was still taken, and was turned down
    /// with an invalid parameter that named no reason. The names are what tell
    /// the caller to stop believing, and what the user is shown when nothing
    /// helps.
    ///
    /// The lock over the ids is let go of before the driver is spoken to: they
    /// are ours the moment they are taken out, and a kernel call is not worth
    /// holding a lock across.
    fn unplug_virtual_devices(&self) -> Refused {
        let devices: Vec<(&'static str, VirtualDeviceId)> = {
            let mut plugged = self.plugged_devices();
            [
                ("keyboard", plugged.keyboard.take()),
                ("mouse", plugged.mouse.take()),
            ]
            .into_iter()
            .filter_map(|(name, id)| id.map(|id| (name, id)))
            .collect()
        };

        let names = devices
            .into_iter()
            .filter(|(_, id)| self.connection().unplug(*id).is_err())
            .map(|(name, _)| name)
            .collect();

        Refused { names }
    }

    /// A poisoned lock only means that some thread panicked while holding it.
    /// What is behind it is still what the driver handed us, so it is taken as
    /// it is rather than turning a panic into a second one.
    fn plugged_devices(&self) -> MutexGuard<'_, PluggedDevices> {
        self.plugged.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The connection, waited for. Used by the interface thread, which can
    /// afford to wait for a rebuild to finish.
    fn connection(&self) -> MutexGuard<'_, Devices> {
        self.devices.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The connection, or an error when it is busy being replaced.
    ///
    /// The hooks reach the driver from inside a low-level hook callback, where
    /// waiting is not free: Windows quietly drops a hook that takes too long,
    /// and the redirect then stops working with nothing to show for it. A
    /// report lost while the bus is being rebuilt is the cheaper outcome - the
    /// key or the button simply goes through as the real device's.
    fn connection_now(&self) -> Result<MutexGuard<'_, Devices>> {
        match self.devices.try_lock() {
            Ok(devices) => Ok(devices),
            Err(TryLockError::Poisoned(poisoned)) => Ok(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => Err(Error::Device(
                "the virtual devices are being re-created".to_owned(),
            )),
        }
    }

    pub fn send_keyboard(&self, report: KeyboardReport) -> Result<()> {
        self.connection_now()?.send_keyboard(report)
    }

    pub fn send_mouse(&self, report: MouseReport) -> Result<()> {
        self.connection_now()?.send_mouse(report)
    }

    /// Lets go of every key and button the virtual devices are holding.
    pub fn release_everything(&self) {
        let _ = self.send_keyboard(KeyboardReport::EMPTY);
        let _ = self.send_mouse(MouseReport::EMPTY);
    }

    /// Lets go of every key, waiting for the connection if it is busy.
    ///
    /// Only for callers that are not a hook callback. [`Self::send_keyboard`]
    /// gives up rather than wait, which is right for a hook - a lost keystroke
    /// costs less than a stalled desktop - but wrong for switching the redirect
    /// off, where the whole point of the report is that nothing stays held.
    /// Nothing sends another one afterwards, so losing this one leaves a key
    /// down for the rest of the session.
    pub fn release_keyboard_waiting(&self) -> Result<()> {
        self.connection().send_keyboard(KeyboardReport::EMPTY)
    }

    /// The mouse half of [`Self::release_keyboard_waiting`].
    pub fn release_mouse_waiting(&self) -> Result<()> {
        self.connection().send_mouse(MouseReport::EMPTY)
    }

    /// Leaves the machine as if this program had never run: nothing held down
    /// and no virtual device on the bus.
    ///
    /// Calling this twice is harmless, which matters because the ways out of
    /// the program overlap - a menu choice, a closed window and a dropped
    /// owner can all arrive at it.
    pub fn park(&self) {
        self.release_everything();

        // A device the driver would not let go of stays on the bus and shows up
        // in Device Manager as a Logitech mouse that is not there. Taking the
        // node off is the most that can be done on the way out: rebuilding the
        // bus takes seconds, and this also runs while the window is closing,
        // where there are none to spare. The next start rebuilds it anyway.
        if !self.unplug_virtual_devices().is_empty() {
            install::remove_leftover_devices();
        }
    }

    #[must_use]
    pub fn status(&self) -> Status {
        let plugged = self.plugged_devices();

        Status {
            // A connection that is busy being replaced is not one to report as
            // ready, and the status line must never be what waits for it.
            connected: self.connection_now().is_ok_and(|devices| devices.is_open()),
            virtual_keyboard: plugged.keyboard.is_some(),
            virtual_mouse: plugged.mouse.is_some(),
        }
    }

    /// Removes the driver package from the system. Consumes the connection,
    /// because there is nothing left to talk to afterwards.
    pub fn remove(self) -> Result<()> {
        self.release_everything();
        self.unplug_virtual_devices();
        self.connection().close();

        install::uninstall()
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        // Whatever happened - a clean exit, a closed console window, a panic -
        // no key may stay held down after we are gone.
        self.park();
    }
}

/// The virtual devices the driver would not take away, by the names a user
/// would call them.
struct Refused {
    names: Vec<&'static str>,
}

impl Refused {
    fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

impl fmt::Display for Refused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.names.as_slice() {
            [] => formatter.write_str("devices"),
            [only] => formatter.write_str(only),
            names => formatter.write_str(&names.join(" and the virtual ")),
        }
    }
}

/// Waits until no virtual device of an earlier session is left on the bus.
///
/// Answers `false` when the timeout runs out with something still there, which
/// is worth knowing rather than pressing on: the device that is still present
/// holds the product id the next one is about to ask for.
fn wait_for_empty_bus() -> bool {
    let deadline = Instant::now() + REMOVAL_TIMEOUT;

    loop {
        if install::leftover_devices() == 0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }

        sleep(REMOVAL_POLL);
    }
}

/// Opens the driver, giving plug and play time to publish its device.
///
/// The already-installed path re-binds the bus on every start, which restarts
/// the core child underneath it, and the first open can arrive before the
/// interface is back. A single attempt then fails a start that a moment's wait
/// would have carried, so the open is retried until the driver answers or the
/// deadline runs out - the same wait a rebuilt bus is already given.
fn open_within_timeout() -> Result<Devices> {
    let deadline = Instant::now() + REOPEN_TIMEOUT;

    loop {
        match Devices::open() {
            Ok(devices) => return Ok(devices),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => sleep(REMOVAL_POLL),
        }
    }
}

/// Repeats a plug that was turned down, pausing in between.
///
/// The bus answers a request that arrives too early with the same refusal it
/// uses for a malformed one, so there is nothing to tell the two apart by:
/// asking again a few times is what separates "too early" from "wrong".
fn keep_trying<T>(mut plug: impl FnMut() -> Result<T>) -> Result<T> {
    let mut attempt = 1;

    loop {
        match plug() {
            Ok(value) => return Ok(value),
            Err(error) if attempt == PLUG_ATTEMPTS => return Err(error),
            Err(_) => {
                sleep(RETRY_PAUSE);
                attempt += 1;
            }
        }
    }
}

/// Whether the program has begun leaving, from wherever it was asked to.
///
/// The window can be closed at any moment, and Windows then gives the process a
/// few seconds on a thread of its own before taking it away. Anything that
/// creates a device has to know, because a device created after the cleanup has
/// looked at the list is one nothing will take away.
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

/// Says that the program is on its way out. Nothing turns this back off: there
/// is no way back from it.
pub fn begin_shutdown() {
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
}

fn is_shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::SeqCst)
}

/// The refusal handed to whatever was still being set up.
fn leaving() -> Error {
    Error::Device("the program is closing".to_owned())
}

/// True when the driver package is installed and its services are running.
///
/// This is the one state in which a restart owed by an earlier removal cannot
/// still be pending: the driver is plainly here and answering.
#[must_use]
pub fn is_running() -> bool {
    service::driver_running()
}

/// True when the program runs with administrator rights.
///
/// The token knows this itself, which is both shorter and steadier than
/// assembling the administrators SID and asking whether we are a member of it.
#[must_use]
pub fn is_elevated() -> bool {
    use std::mem::size_of;

    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = HANDLE::default();

    // SAFETY: the token is closed on every path that opened it, and the
    // structure Windows writes into is described with its real size.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut written = 0u32;
        let queried = GetTokenInformation(
            token,
            TokenElevation,
            Some(std::ptr::addr_of_mut!(elevation).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut written,
        )
        .is_ok();
        let _ = CloseHandle(token);

        queried && elevation.TokenIsElevated != 0
    }
}

/// Win32 wants null-terminated UTF-16 almost everywhere.
pub(crate) fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused() -> Error {
        Error::Device("busy".to_owned())
    }

    /// A machine carrying a 2024 build, against the one this program speaks to.
    fn mismatch() -> version::Mismatch {
        version::Mismatch {
            installed: version::Version::from_words(0x07e8_0001, 0),
            ours: version::Version::from_words(0x07e5_0001, 0x0555_0000),
        }
    }

    #[test]
    fn wide_strings_are_null_terminated() {
        assert_eq!(wide("ab"), vec![0x61, 0x62, 0x00]);
        assert_eq!(wide(""), vec![0x00]);
    }

    #[test]
    fn a_plug_that_works_first_time_is_not_repeated() {
        let mut attempts = 0;

        let id = keep_trying(|| {
            attempts += 1;
            Ok(7)
        });

        assert_eq!(id.ok(), Some(7));
        assert_eq!(attempts, 1);
    }

    #[test]
    fn a_plug_refused_while_the_bus_catches_up_is_asked_again() {
        let mut attempts = 0;

        let id = keep_trying(|| {
            attempts += 1;
            if attempts < 3 {
                Err(refused())
            } else {
                Ok(attempts)
            }
        });

        assert_eq!(id.ok(), Some(3));
    }

    #[test]
    fn a_plug_that_is_always_refused_gives_up_and_says_so() {
        let mut attempts = 0;

        let result: Result<()> = keep_trying(|| {
            attempts += 1;
            Err(refused())
        });

        assert!(result.is_err());
        assert_eq!(attempts, PLUG_ATTEMPTS);
    }

    /// On a machine with no virtual device of ours there is nothing to wait
    /// for, and the wait has to notice that at once instead of sitting out its
    /// timeout - which is what a fixed pause did.
    #[test]
    fn an_empty_bus_is_noticed_without_waiting() {
        let started = Instant::now();

        assert!(wait_for_empty_bus());
        assert!(started.elapsed() < REMOVAL_TIMEOUT);
    }

    /// The message the user is left with when nothing worked has to read as a
    /// sentence, whichever device was the one that would not go.
    #[test]
    fn the_refused_devices_are_named_the_way_a_user_would_name_them() {
        assert_eq!(
            Refused {
                names: vec!["mouse"]
            }
            .to_string(),
            "mouse"
        );
        assert_eq!(
            Refused {
                names: vec!["keyboard", "mouse"]
            }
            .to_string(),
            "keyboard and the virtual mouse"
        );
        assert!(Refused { names: vec![] }.is_empty());
    }

    /// Replacing someone's driver is the one step they are most likely to want
    /// to ask about afterwards, so it has to say which build went and which
    /// came - in the form they would quote it in.
    #[test]
    fn the_step_that_replaces_the_driver_names_both_builds() {
        let text = Step::ReplacingDriver(mismatch()).to_string();

        assert!(
            text.contains("2024.1.0.0"),
            "{text:?} hides what is installed"
        );
        assert!(
            text.contains("2021.1.1365.0"),
            "{text:?} hides what we speak"
        );
    }

    #[test]
    fn every_step_reads_as_a_sentence_a_user_can_understand() {
        let steps = [
            Step::PreparingFiles,
            Step::ClosingLogitechSoftware,
            Step::InstallingDriver,
            Step::ReplacingDriver(mismatch()),
            Step::DriverReady,
            Step::CleaningPreviousSession,
            Step::RebuildingBus,
            Step::CreatingVirtualDevices,
            Step::KeyboardReady,
            Step::MouseReady,
            Step::Connected,
        ];

        for step in steps {
            let text = step.to_string();
            assert!(!text.is_empty());
            assert!(
                text.chars().next().is_some_and(char::is_uppercase),
                "{text:?} should start with a capital letter"
            );
            assert!(
                !text.contains("IOCTL") && !text.contains("pnputil") && !text.contains('_'),
                "{text:?} leaks an implementation detail"
            );
        }
    }
}
