//! Installing and removing the driver package.

mod store;

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread::{sleep, spawn};
use std::time::{Duration, Instant};

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DICD_GENERATE_ID, DIF_REGISTERDEVICE, DIGCF_ALLCLASSES, DIGCF_PRESENT, HDEVINFO,
    INSTALLFLAG_FORCE, SP_DEVINFO_DATA, SPDRP_HARDWAREID, SetupDiCallClassInstaller,
    SetupDiCreateDeviceInfoList, SetupDiCreateDeviceInfoW, SetupDiEnumDeviceInfo,
    SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceRegistryPropertyW,
    SetupDiRemoveDevice, SetupDiSetDeviceRegistryPropertyW, UpdateDriverForPlugAndPlayDevicesW,
};
use windows::core::PCWSTR;

use crate::error::{Error, Result};

use super::device::DeviceInfoSet;
use super::{holders, process, service, system32, version, wide, wide_path};

const ROOT_HARDWARE_ID: &str = r"root\LGHUBVirtualBus";
const SYSTEM_DEVICE_CLASS: windows::core::GUID =
    windows::core::GUID::from_u128(0x4d36_e97d_e325_11ce_bfc1_0800_2be1_0318);

const BUS_PACKAGE: &str = "logi_joy_bus_enum.inf";
const HID_PACKAGE: &str = "logi_joy_vir_hid.inf";

const INSTALLER: &str = "pnputil.exe";
const RESTART_TO_FINISH: i32 = 3010;
const INSTALLER_TIMEOUT: Duration = Duration::from_secs(60);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(3);
const STARTUP_POLL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finished {
    Now,
    AfterRestart,
}

pub fn install(drivers: &Path) -> Result<()> {
    let finished = add_packages(drivers)?;
    is_the_build_we_speak_to(drivers, finished)
}

fn add_packages(drivers: &Path) -> Result<Finished> {
    let mut finished = Finished::Now;
    for package in [BUS_PACKAGE, HID_PACKAGE] {
        let path = drivers.join(package);
        if !path.exists() {
            return Err(Error::Install(format!("{package} is missing")));
        }
        if add_package(&path)? == Finished::AfterRestart {
            finished = Finished::AfterRestart;
        }
    }
    if bind_root_device(drivers)? == Finished::AfterRestart {
        finished = Finished::AfterRestart;
    }
    if wait_for_driver() {
        return Ok(finished);
    }
    service::start_all();
    if wait_for_driver() {
        Ok(finished)
    } else {
        Err(Error::Install(
            "the driver was installed but did not start".to_owned(),
        ))
    }
}

pub fn is_the_build_we_speak_to(drivers: &Path, finished: Finished) -> Result<()> {
    if finished == Finished::Now {
        return Ok(());
    }
    match version::mismatch(drivers) {
        None => Ok(()),
        Some(mismatch) => Err(Error::RestartRequired(format!(
            "Windows keeps version {} of the Logitech driver loaded until the computer restarts, \
             and the {} this program speaks to cannot take over until then",
            mismatch.installed, mismatch.ours
        ))),
    }
}

pub fn replace(drivers: &Path) -> Result<()> {
    service::stop_all();
    let mut finished = Finished::Now;
    for published_name in our_published_names()? {
        let output = pnputil(["/delete-driver", &published_name, "/uninstall", "/force"])?;
        let code = output.status.code().unwrap_or(-1);
        match code {
            0 => {}
            RESTART_TO_FINISH => finished = Finished::AfterRestart,
            _ => {
                return Err(Error::Install(format!(
                    "{published_name} could not be replaced (code {code}){}",
                    blamed()
                )));
            }
        }
    }
    if add_packages(drivers)? == Finished::AfterRestart {
        finished = Finished::AfterRestart;
    }
    is_the_build_we_speak_to(drivers, finished)
}

fn wait_for_driver() -> bool {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if service::driver_running() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(STARTUP_POLL);
    }
}

pub fn sanitize_root_devices(drivers: &Path) -> Result<()> {
    let roots = find_devices(ROOT_HARDWARE_ID);
    if roots.len() > 1 {
        remove_root_device();
    }
    let _ = bind_root_device(drivers)?;
    Ok(())
}

pub fn bind_root_device(drivers: &Path) -> Result<Finished> {
    if !root_device_exists() {
        create_root_device()?;
    }
    let inf = wide_path(&drivers.join(BUS_PACKAGE));
    let hardware_id = wide(ROOT_HARDWARE_ID);
    let mut reboot_required = windows::core::BOOL::from(false);
    // SAFETY: both strings are null terminated and outlive the call.
    unsafe {
        UpdateDriverForPlugAndPlayDevicesW(
            None,
            PCWSTR(hardware_id.as_ptr()),
            PCWSTR(inf.as_ptr()),
            INSTALLFLAG_FORCE,
            Some(&mut reboot_required),
        )
        .map_err(|error| Error::Install(format!("the driver could not be bound: {error}")))?;
    }
    if reboot_required.as_bool() {
        Ok(Finished::AfterRestart)
    } else {
        Ok(Finished::Now)
    }
}

pub fn root_device_exists() -> bool {
    !find_devices(ROOT_HARDWARE_ID).is_empty()
}

pub fn remove_root_device() -> i32 {
    let enumerator = wide("ROOT");
    let mut removed = 0;
    // SAFETY: the buffer is sized by us and the set outlives the loop.
    unsafe {
        let Ok(set) = SetupDiGetClassDevsW(
            Some(&SYSTEM_DEVICE_CLASS),
            PCWSTR(enumerator.as_ptr()),
            None,
            DIGCF_ALLCLASSES,
        ) else {
            return 0;
        };
        let set = DeviceInfoSet::new(set);
        let mut index = 0;
        loop {
            let mut info = SP_DEVINFO_DATA {
                cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInfo(set.handle(), index, &mut info).is_err() {
                break;
            }
            if !has_hardware_id(set.handle(), &info, ROOT_HARDWARE_ID) {
                index += 1;
                continue;
            }
            if SetupDiRemoveDevice(set.handle(), &mut info).as_bool() {
                removed += 1;
            } else {
                index += 1;
            }
        }
    }
    removed
}

fn create_root_device() -> Result<()> {
    let name = wide("LGHUBVirtualBus");
    let mut hardware_id = wide(ROOT_HARDWARE_ID);
    hardware_id.push(0);
    // SAFETY: the property buffer is described with its real length in bytes,
    // and the set is destroyed wherever this scope ends.
    unsafe {
        let set = DeviceInfoSet::new(
            SetupDiCreateDeviceInfoList(Some(&SYSTEM_DEVICE_CLASS), None)
                .map_err(|error| Error::Install(format!("device list: {error}")))?,
        );
        let mut info = SP_DEVINFO_DATA {
            cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        SetupDiCreateDeviceInfoW(
            set.handle(),
            PCWSTR(name.as_ptr()),
            &SYSTEM_DEVICE_CLASS,
            PCWSTR::null(),
            None,
            DICD_GENERATE_ID,
            Some(&mut info),
        )
        .map_err(|error| Error::Install(format!("device node: {error}")))?;
        let bytes =
            std::slice::from_raw_parts(hardware_id.as_ptr().cast::<u8>(), hardware_id.len() * 2);
        SetupDiSetDeviceRegistryPropertyW(set.handle(), &mut info, SPDRP_HARDWAREID, Some(bytes))
            .map_err(|error| Error::Install(format!("hardware id: {error}")))?;
        SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set.handle(), Some(&info))
            .map_err(|error| Error::Install(format!("device registration: {error}")))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Leftovers {
    Count,
    Remove,
}

pub fn leftover_devices() -> i32 {
    visit_leftover_devices(Leftovers::Count)
}

pub fn remove_leftover_devices() -> i32 {
    visit_leftover_devices(Leftovers::Remove)
}

const MOUSE_HARDWARE_ID: &str = r"HID\VID_046D&PID_C231";
const KEYBOARD_HARDWARE_ID: &str = r"HID\VID_046D&PID_C232";
const VIRTUAL_MOUSE_HWID: &str = r"LGHUBDevice\VID_046D&PID_C231";
const VIRTUAL_KEYBOARD_HWID: &str = r"LGHUBDevice\VID_046D&PID_C232";

fn is_our_virtual_device(set: HDEVINFO, info: &SP_DEVINFO_DATA) -> bool {
    has_hardware_id(set, info, MOUSE_HARDWARE_ID)
        || has_hardware_id(set, info, KEYBOARD_HARDWARE_ID)
        || has_hardware_id(set, info, VIRTUAL_MOUSE_HWID)
        || has_hardware_id(set, info, VIRTUAL_KEYBOARD_HWID)
}

fn visit_leftover_devices(action: Leftovers) -> i32 {
    let mut found = 0;
    let scope = match action {
        Leftovers::Count => DIGCF_ALLCLASSES | DIGCF_PRESENT,
        Leftovers::Remove => DIGCF_ALLCLASSES,
    };
    let enumerators = [wide("HID"), wide("LGHUBDEVICE")];
    // SAFETY: every device handed to SetupDiRemoveDevice came out of the same
    // enumeration, which outlives the loop.
    unsafe {
        for enumerator in &enumerators {
            let Ok(set) = SetupDiGetClassDevsW(None, PCWSTR(enumerator.as_ptr()), None, scope)
            else {
                continue;
            };
            let set = DeviceInfoSet::new(set);
            let mut index = 0;
            loop {
                let mut info = SP_DEVINFO_DATA {
                    cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                    ..Default::default()
                };
                if SetupDiEnumDeviceInfo(set.handle(), index, &mut info).is_err() {
                    break;
                }
                if !is_our_virtual_device(set.handle(), &info) {
                    index += 1;
                    continue;
                }
                match action {
                    Leftovers::Count => {
                        found += 1;
                        index += 1;
                    }
                    Leftovers::Remove => {
                        if SetupDiRemoveDevice(set.handle(), &mut info).as_bool() {
                            found += 1;
                        } else {
                            index += 1;
                        }
                    }
                }
            }
        }
    }
    found
}

pub fn uninstall() -> Result<()> {
    service::stop_all();
    let ours = our_published_names()?;
    if ours.is_empty() {
        return Err(Error::Uninstall(
            "the driver is not present in the driver store".to_owned(),
        ));
    }
    for published_name in ours {
        let output = pnputil(["/delete-driver", &published_name, "/uninstall", "/force"])?;
        let code = output.status.code().unwrap_or(-1);
        if code != 0 && code != RESTART_TO_FINISH {
            return Err(Error::Uninstall(format!(
                "{published_name} could not be removed (code {code}){}",
                blamed()
            )));
        }
    }
    Ok(())
}

fn blamed() -> String {
    blame(&holders::of(&version::installed_binaries()))
}

fn blame(holders: &[String]) -> String {
    if holders.is_empty() {
        String::new()
    } else {
        format!("; the driver files are held by {}", holders.join(", "))
    }
}

fn our_published_names() -> Result<Vec<String>> {
    let from_database: Vec<String> = [BUS_PACKAGE, HID_PACKAGE]
        .into_iter()
        .flat_map(store::published_names)
        .collect();
    if !from_database.is_empty() {
        return Ok(from_database);
    }
    Ok(installed_packages()?
        .into_iter()
        .filter(Package::is_ours)
        .map(|package| package.published_name)
        .collect())
}

fn add_package(path: &Path) -> Result<Finished> {
    let path = path.display().to_string();
    let output = pnputil(["/add-driver", &path, "/install"])?;
    match output.status.code().unwrap_or(-1) {
        0 => Ok(Finished::Now),
        RESTART_TO_FINISH => Ok(Finished::AfterRestart),
        code => Err(Error::Install(format!(
            "the package could not be added to the system (code {code})"
        ))),
    }
}

fn pnputil<const N: usize>(arguments: [&str; N]) -> Result<Output> {
    let child = Command::new(system32(INSTALLER))
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            Error::Install(format!("the system installer could not be run: {error}"))
        })?;
    let id = child.id();
    let (sender, receiver) = mpsc::channel();
    let _reader = spawn(move || {
        let _ = sender.send(child.wait_with_output());
    });
    match receiver.recv_timeout(INSTALLER_TIMEOUT) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(Error::Install(format!(
            "the system installer could not be run: {error}"
        ))),
        Err(_) => {
            process::terminate(id, |name| name.eq_ignore_ascii_case(INSTALLER));
            Err(Error::Install(format!(
                "the system installer did not finish within {} seconds",
                INSTALLER_TIMEOUT.as_secs()
            )))
        }
    }
}

fn installed_packages() -> Result<Vec<Package>> {
    let output = pnputil(["/enum-drivers"])?;
    Ok(parse_packages(&String::from_utf8_lossy(&output.stdout)))
}

#[derive(Debug, PartialEq, Eq)]
struct Package {
    published_name: String,
    original_name: String,
    provider: String,
}

impl Package {
    fn is_ours(&self) -> bool {
        [BUS_PACKAGE, HID_PACKAGE]
            .into_iter()
            .any(|package| self.original_name.eq_ignore_ascii_case(package))
            && self.provider.contains("Logi")
    }
}

fn parse_packages(listing: &str) -> Vec<Package> {
    let mut packages = Vec::new();
    let mut values: Vec<&str> = Vec::new();
    for line in listing.lines() {
        let line = line.trim();
        if line.is_empty() {
            packages.extend(package_from(&values));
            values.clear();
            continue;
        }
        let Some((_, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if values.is_empty() && !value.ends_with(".inf") {
            continue;
        }
        values.push(value);
    }
    packages.extend(package_from(&values));
    packages
}

fn package_from(values: &[&str]) -> Option<Package> {
    let [published_name, original_name, provider, ..] = values else {
        return None;
    };
    Some(Package {
        published_name: (*published_name).to_owned(),
        original_name: (*original_name).to_owned(),
        provider: (*provider).to_owned(),
    })
}

fn has_hardware_id(set: HDEVINFO, info: &SP_DEVINFO_DATA, target: &str) -> bool {
    let mut buffer = [0u8; 1024];
    let mut size = 0u32;
    // SAFETY: the buffer is owned by us, and info is a valid pointer from
    // SetupDiEnumDeviceInfo.
    let ok = unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set,
            info,
            SPDRP_HARDWAREID,
            None,
            Some(&mut buffer),
            Some(&mut size),
        )
    };
    if ok.is_err() || size == 0 {
        return false;
    }
    let units: Vec<u16> = buffer[..size as usize]
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    units
        .split(|&unit| unit == 0)
        .filter(|slice| !slice.is_empty())
        .any(|slice| {
            String::from_utf16(slice)
                .is_ok_and(|hardware_id| hardware_id.eq_ignore_ascii_case(target))
        })
}

fn find_devices(hardware_id: &str) -> Vec<String> {
    let mut found = Vec::new();
    let enumerator = wide("ROOT");
    // SAFETY: the buffer is sized by us and the set outlives the loop.
    unsafe {
        let Ok(set) = SetupDiGetClassDevsW(
            Some(&SYSTEM_DEVICE_CLASS),
            PCWSTR(enumerator.as_ptr()),
            None,
            DIGCF_PRESENT,
        ) else {
            return found;
        };
        let set = DeviceInfoSet::new(set);
        let mut index = 0;
        loop {
            let mut info = SP_DEVINFO_DATA {
                cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInfo(set.handle(), index, &mut info).is_err() {
                break;
            }
            index += 1;
            if !has_hardware_id(set.handle(), &info, hardware_id) {
                continue;
            }
            let mut buffer = [0u16; 512];
            if SetupDiGetDeviceInstanceIdW(set.handle(), &info, Some(&mut buffer), None).is_ok() {
                if let Ok(id) = PCWSTR(buffer.as_ptr()).to_string() {
                    found.push(id);
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = "Microsoft PnP Utility\n\nPublished Name:     oem49.inf\nOriginal Name:      logi_joy_bus_enum.inf\nProvider Name:      Logitech\nClass Name:         System devices\nDriver Version:     09/02/2022 2022.3.0.2\n\nPublished Name:     oem50.inf\nOriginal Name:      logi_joy_vir_hid.inf\nProvider Name:      Logitech\nClass Name:         HIDClass\n\nPublished Name:     oem12.inf\nOriginal Name:      nvidia_display.inf\nProvider Name:      NVIDIA\nClass Name:         Display\n";

    fn published_names(listing: &str) -> Vec<String> {
        parse_packages(listing)
            .into_iter()
            .filter(Package::is_ours)
            .map(|package| package.published_name)
            .collect()
    }

    #[test]
    fn every_record_in_the_listing_is_read() {
        let packages = parse_packages(LISTING);
        assert_eq!(packages.len(), 3);
        assert_eq!(packages[0].published_name, "oem49.inf");
        assert_eq!(packages[1].original_name, "logi_joy_vir_hid.inf");
        assert_eq!(packages[2].provider, "NVIDIA");
    }

    #[test]
    fn only_our_two_packages_are_claimed_as_ours() {
        assert_eq!(published_names(LISTING), ["oem49.inf", "oem50.inf"]);
    }

    #[test]
    fn an_installation_that_finished_now_is_not_waiting_on_a_restart() {
        let answer = is_the_build_we_speak_to(Path::new(r"Z:\no\such\folder"), Finished::Now);
        assert!(answer.is_ok());
    }

    #[test]
    fn a_deferred_installation_with_nothing_to_compare_carries_on() {
        let answer =
            is_the_build_we_speak_to(Path::new(r"Z:\no\such\folder"), Finished::AfterRestart);
        assert!(answer.is_ok());
    }
}
