//! Everything that talks to the signed Logitech driver stack.

mod device;
mod ghub;
mod holders;
pub mod install;
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
use std::ffi::OsString;
use std::fmt;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError};
use std::thread::sleep;
use std::time::{Duration, Instant};

use self::device::{Devices, VirtualDeviceId};
use crate::error::{Error, Result};
use crate::hid::{KeyboardReport, MouseReport};

const REMOVAL_TIMEOUT: Duration = Duration::from_secs(3);
const REMOVAL_POLL: Duration = Duration::from_millis(50);
const REOPEN_TIMEOUT: Duration = Duration::from_secs(10);
const PLUG_ATTEMPTS: u32 = 4;
const RETRY_PAUSE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    PreparingFiles,
    ClosingLogitechSoftware,
    InstallingDriver,
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
                "Closing Logitech software that would block the virtual devices".into()
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

pub trait Report {
    fn step(&mut self, step: Step);
}

impl<F: FnMut(Step)> Report for F {
    fn step(&mut self, step: Step) {
        self(step);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub connected: bool,
    pub virtual_keyboard: bool,
    pub virtual_mouse: bool,
}

#[derive(Default)]
struct PluggedDevices {
    keyboard: Option<VirtualDeviceId>,
    mouse: Option<VirtualDeviceId>,
}

pub struct Driver {
    devices: Mutex<Devices>,
    plugged: Mutex<PluggedDevices>,
    payload: ExtractedDrivers,
    _watchdog: ghub::Watchdog,
}

impl Driver {
    pub fn connect(report: &mut dyn Report) -> Result<Self> {
        if !is_elevated() {
            return Err(Error::NotElevated);
        }
        report.step(Step::PreparingFiles);
        let payload = ExtractedDrivers::unpack()?;

        if ghub::blockers_running() {
            report.step(Step::ClosingLogitechSoftware);
        }
        let watchdog = ghub::Watchdog::start()?;

        service::stop_all();

        if service::driver_installed() {
            match version::mismatch(payload.directory()) {
                None => {
                    report.step(Step::DriverReady);
                    install::sanitize_root_devices(payload.directory())?;
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
        reboot::clear_restart_pending();

        if install::remove_leftover_devices() > 0 {
            report.step(Step::CleaningPreviousSession);
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

    pub fn recreate_virtual_devices(&self, report: &mut dyn Report) -> Result<()> {
        let refused = self.unplug_virtual_devices();
        if refused.is_empty() && wait_for_empty_bus() {
            sleep(Duration::from_millis(200));
            return self.create_virtual_devices(report);
        }

        report.step(Step::RebuildingBus);
        install::remove_leftover_devices();
        self.rebuild_bus()?;
        if !wait_for_empty_bus() {
            return Err(Error::Device(format!(
                "the virtual {refused} did not leave the bus even after rebuilding it; a restart \
                 of Windows will clear it"
            )));
        }
        sleep(Duration::from_millis(200));
        self.create_virtual_devices(report)
    }

    fn rebuild_bus(&self) -> Result<()> {
        self.connection().close();
        install::remove_leftover_devices();
        service::stop_all();

        // A re-bind asked while the LampArray service still holds the device
        // open is vetoed by Windows without a word, so the blockers are stopped
        // here, right before the re-bind, and a refusal is worth failing over
        // rather than carrying on next to a bus with its stale state intact.
        if !ghub::stop_blockers() {
            return Err(Error::Device(ghub::LAMP_STILL_HOLDING.to_owned()));
        }

        let _ = install::bind_root_device(self.payload.directory())?;
        service::start_all();
        wait_for_empty_bus();
        sleep(Duration::from_millis(400));
        self.reopen()
    }

    fn reopen(&self) -> Result<()> {
        *self.connection() = open_within_timeout()?;
        Ok(())
    }

    fn create_virtual_devices(&self, report: &mut dyn Report) -> Result<()> {
        report.step(Step::CreatingVirtualDevices);
        if self.plug_both_devices(report).is_ok() {
            return Ok(());
        }

        self.unplug_virtual_devices();
        report.step(Step::RebuildingBus);
        self.rebuild_bus()?;
        wait_for_empty_bus();
        sleep(Duration::from_millis(200));

        self.plug_both_devices(report)
    }

    fn plug_both_devices(&self, report: &mut dyn Report) -> Result<()> {
        self.plug_keyboard()?;
        report.step(Step::KeyboardReady);
        sleep(Duration::from_millis(100));

        if let Err(error) = self.plug_mouse() {
            self.unplug_virtual_devices();
            return Err(error);
        }
        report.step(Step::MouseReady);
        Ok(())
    }

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

    fn unplug_virtual_devices(&self) -> Refused {
        let mut names = Vec::new();
        let mut plugged = self.plugged_devices();

        if let Some(id) = plugged.keyboard {
            if self.connection().unplug(id).is_ok() {
                plugged.keyboard = None;
            } else {
                names.push("keyboard");
            }
        }

        // Give the driver and PnP manager time to tear down the first device
        // before requesting the second one.
        sleep(Duration::from_millis(350));

        if let Some(id) = plugged.mouse {
            if self.connection().unplug(id).is_ok() {
                plugged.mouse = None;
            } else {
                names.push("mouse");
            }
        }

        Refused { names }
    }

    fn plugged_devices(&self) -> MutexGuard<'_, PluggedDevices> {
        self.plugged.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn connection(&self) -> MutexGuard<'_, Devices> {
        self.devices.lock().unwrap_or_else(PoisonError::into_inner)
    }

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

    pub fn release_everything(&self) {
        let _ = self.send_keyboard(KeyboardReport::EMPTY);
        let _ = self.send_mouse(MouseReport::EMPTY);
    }

    pub fn release_keyboard_waiting(&self) -> Result<()> {
        self.connection().send_keyboard(KeyboardReport::EMPTY)
    }

    pub fn release_mouse_waiting(&self) -> Result<()> {
        self.connection().send_mouse(MouseReport::EMPTY)
    }

    pub fn park(&self) {
        self.release_everything();
        let _ = self.unplug_virtual_devices();
        install::remove_leftover_devices();
    }

    #[must_use]
    pub fn status(&self) -> Status {
        let plugged = self.plugged_devices();
        Status {
            connected: self.connection_now().is_ok_and(|devices| devices.is_open()),
            virtual_keyboard: plugged.keyboard.is_some(),
            virtual_mouse: plugged.mouse.is_some(),
        }
    }

    pub fn remove(self) -> Result<()> {
        self.release_everything();
        self.unplug_virtual_devices();
        self.connection().close();
        force_uninstall()
    }
}

pub fn force_uninstall() -> Result<()> {
    install::remove_leftover_devices();
    install::remove_root_device();
    install::uninstall()
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.park();
    }
}

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

static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

pub fn begin_shutdown() {
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
}

fn is_shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::SeqCst)
}

fn leaving() -> Error {
    Error::Device("the program is closing".to_owned())
}

#[must_use]
pub fn is_running() -> bool {
    service::driver_running()
}

#[must_use]
pub fn is_elevated() -> bool {
    use std::mem::size_of;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
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

pub(crate) fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub(crate) fn system32(program: &str) -> PathBuf {
    system_directory().join(program)
}

pub(crate) fn system_directory() -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::Foundation::MAX_PATH;
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;

    let mut buffer = [0u16; MAX_PATH as usize];
    // SAFETY: the call is given the real length of the buffer and writes no
    // more than that.
    let written = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    if written == 0 || written > buffer.len() {
        return PathBuf::from(r"C:\Windows\System32");
    }
    PathBuf::from(OsString::from_wide(&buffer[..written]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused() -> Error {
        Error::Device("busy".to_owned())
    }

    #[test]
    fn wide_strings_are_null_terminated() {
        assert_eq!(wide("ab"), vec![0x61, 0x62, 0x00]);
        assert_eq!(wide(""), vec![0x00]);
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
}
