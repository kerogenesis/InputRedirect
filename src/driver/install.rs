//! Installing and removing the driver package.
//!
//! Everything here is done the way Windows expects it: the package is handed to
//! the plug and play utility, and the root device it binds to is created with
//! the setup API rather than by writing to the registry.

mod store;

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread::{sleep, spawn};
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiCallClassInstaller, SetupDiCreateDeviceInfoList, SetupDiCreateDeviceInfoW,
    SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW, SetupDiRemoveDevice,
    SetupDiSetDeviceRegistryPropertyW, UpdateDriverForPlugAndPlayDevicesW, DICD_GENERATE_ID,
    DIF_REGISTERDEVICE, DIGCF_ALLCLASSES, DIGCF_PRESENT, INSTALLFLAG_FORCE, SPDRP_HARDWAREID,
    SP_DEVINFO_DATA,
};

use crate::error::{Error, Result};

use super::device::DeviceInfoSet;
use super::{holders, process, service, system32, version, wide, wide_path};

/// The device the bus driver binds to. It does not exist until we create it.
const ROOT_HARDWARE_ID: &str = r"root\LGHUBVirtualBus";
const SYSTEM_DEVICE_CLASS: windows::core::GUID =
    windows::core::GUID::from_u128(0x4d36_e97d_e325_11ce_bfc1_0800_2be1_0318);

const BUS_PACKAGE: &str = "logi_joy_bus_enum.inf";
const HID_PACKAGE: &str = "logi_joy_vir_hid.inf";

/// The plug and play utility, which is only ever the one in System32.
const INSTALLER: &str = "pnputil.exe";

/// What the plug and play utility answers when it has done the work but a
/// restart is needed to finish it.
///
/// The store is up to date either way; what is left over is the copy the kernel
/// already has open. That is a success when nothing needs the new files right
/// now, and not when the whole point was to have the new build answering -
/// which is why the answer is carried out of here rather than dropped.
const RESTART_TO_FINISH: i32 = 3010;

/// How long the plug and play utility is given to finish. It normally answers
/// in a second or two; the deadline is for the case where it never answers at
/// all, which used to hang the program on its way up.
const INSTALLER_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the services are given to come up, and how often to look.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(3);
const STARTUP_POLL: Duration = Duration::from_millis(50);

/// Whether Windows did the work now or only wrote it down for the next start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Finished {
    Now,
    AfterRestart,
}

/// Adds both packages to the driver store and binds them.
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

    bind_root_device(drivers)?;

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

/// Makes sure the build now answering is the one this program speaks to.
///
/// Windows says "done, after a restart" when it could not put a file in place
/// because the kernel still has the old one open. The services run either way,
/// but on the old images, which answer our requests with the same invalid
/// parameter they use for a device that is merely busy.
///
/// Reading the version back tells the two apart: Windows often lands the new
/// file at once and defers only the deletion of the old one.
fn is_the_build_we_speak_to(drivers: &Path, finished: Finished) -> Result<()> {
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

/// Puts our build of the package in place of whichever one is installed.
///
/// Both packages are removed first and ours added afterwards: they carry the
/// same file names whatever build they come from, and a package left behind can
/// be ranked above ours the next time plug and play looks for a driver.
///
/// Unlike a removal, a step that only finishes after a restart is not treated
/// as done here - the old images would go on answering, which is the situation
/// this function exists to get out of.
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
                )))
            }
        }
    }

    if add_packages(drivers)? == Finished::AfterRestart {
        finished = Finished::AfterRestart;
    }

    // Either half of the replacement can be put off to the next start, and
    // either one leaves the old build answering.
    is_the_build_we_speak_to(drivers, finished)
}

/// Looks for a running driver until the timeout runs out.
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

/// Creates the root device if it is missing and points the bus driver at it.
pub fn bind_root_device(drivers: &Path) -> Result<()> {
    if !root_device_exists() {
        create_root_device()?;
    }

    let inf = wide_path(&drivers.join(BUS_PACKAGE));
    let hardware_id = wide(ROOT_HARDWARE_ID);

    // SAFETY: both strings are null terminated and outlive the call. The
    // reboot-required flag is optional and was never read.
    unsafe {
        UpdateDriverForPlugAndPlayDevicesW(
            None,
            PCWSTR(hardware_id.as_ptr()),
            PCWSTR(inf.as_ptr()),
            INSTALLFLAG_FORCE,
            None,
        )
        .map_err(|error| Error::Install(format!("the driver could not be bound: {error}")))
    }
}

fn root_device_exists() -> bool {
    !find_devices(ROOT_HARDWARE_ID).is_empty()
}

fn create_root_device() -> Result<()> {
    let name = wide("LGHUBVirtualBus");
    let mut hardware_id = wide(ROOT_HARDWARE_ID);
    hardware_id.push(0); // the property is a list of strings

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

/// What an enumeration of the leftover child devices is for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Leftovers {
    Count,
    Remove,
}

/// How many child devices of a previous session are still on the bus.
///
/// A device that is still there holds its product id, and the next device asked
/// for with that id is turned down.
pub fn leftover_devices() -> i32 {
    visit_leftover_devices(Leftovers::Count)
}

/// Removes the child devices a previous session left behind, and reports how
/// many were actually taken away.
pub fn remove_leftover_devices() -> i32 {
    visit_leftover_devices(Leftovers::Remove)
}

fn visit_leftover_devices(action: Leftovers) -> i32 {
    let mut found = 0;

    // Counting asks only about devices really on the bus, because only those
    // hold a product id. Windows keeps the registry entry of a child long after
    // the bus stops reporting it, and counting those ghosts meant the wait for
    // an empty bus could never succeed.
    //
    // Removing deliberately asks about all of them, ghosts included.
    let scope = match action {
        Leftovers::Count => DIGCF_ALLCLASSES | DIGCF_PRESENT,
        Leftovers::Remove => DIGCF_ALLCLASSES,
    };

    // Named rather than passed inline: the pointer must outlive the call.
    let filter = wide("LGHUBDEVICE");

    // SAFETY: every device handed to SetupDiRemoveDevice came out of the same
    // enumeration, which outlives the loop.
    unsafe {
        let Ok(set) = SetupDiGetClassDevsW(None, PCWSTR(filter.as_ptr()), None, scope) else {
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
            index += 1;

            match action {
                Leftovers::Count => found += 1,
                Leftovers::Remove => {
                    if SetupDiRemoveDevice(set.handle(), &mut info).as_bool() {
                        found += 1;
                    }
                }
            }
        }
    }

    found
}

/// Removes both packages from the driver store.
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

        // A removal that only finishes after a restart still counts as done:
        // the copy left behind is the one the kernel already has open, which is
        // exactly what removing a loaded driver looks like.
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

/// Who is in the way, as a sentence to add to a failure - or nothing at all
/// when nobody can be named.
fn blamed() -> String {
    blame(&holders::of(&version::installed_binaries()))
}

/// The wording, kept apart from the asking so it can be tested without a driver
/// on the machine.
fn blame(holders: &[String]) -> String {
    if holders.is_empty() {
        String::new()
    } else {
        format!("; the driver files are held by {}", holders.join(", "))
    }
}

/// The names both our packages are published under, such as `oem49.inf`.
///
/// The driver database is asked first: it is indexed by our own .inf file names
/// and holds nothing that depends on the display language of Windows. The
/// printed listing is only read when the database has nothing to say.
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

/// Runs the plug and play utility and waits for it, but not forever.
///
/// The output is read on a thread of its own. A pipe that fills up stops the
/// program writing into it, so a wait that is not reading at the same time
/// would be waiting for a process that is waiting for us.
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
            // Nothing is owed to a utility that stopped answering, and leaving
            // it running would hold the driver store against the next attempt.
            // A minute has passed since it was started, so the id is only
            // closed if it still names the utility we started.
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

/// One entry of the driver store listing.
#[derive(Debug, PartialEq, Eq)]
struct Package {
    published_name: String,
    original_name: String,
    provider: String,
}

impl Package {
    /// Compared without regard to case: Windows file names are case
    /// insensitive, and mistaking our own package for someone else's would
    /// leave the user unable to remove the driver at all.
    fn is_ours(&self) -> bool {
        [BUS_PACKAGE, HID_PACKAGE]
            .into_iter()
            .any(|package| self.original_name.eq_ignore_ascii_case(package))
            && self.provider.contains("Logi")
    }
}

/// Parses the listing the plug and play utility prints. Records are separated
/// by blank lines and every field is `label: value`.
///
/// The labels are deliberately ignored: Windows translates them, and a parser
/// that matched them would report an empty driver store on a Russian or
/// Ukrainian machine. The order of the fields and the values read the same in
/// every language.
///
/// A record therefore starts at the first value that names an .inf file, which
/// is what separates the records from the heading printed above them.
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

/// The first three values of a record are the published name, the original name
/// and the provider, in that order. A record with fewer than three is not one.
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

fn find_devices(hardware_id: &str) -> Vec<String> {
    let mut found = Vec::new();
    let filter = wide(hardware_id);

    // SAFETY: the buffer is sized by us and the set outlives the loop.
    unsafe {
        let Ok(set) = SetupDiGetClassDevsW(
            Some(&SYSTEM_DEVICE_CLASS),
            PCWSTR(filter.as_ptr()),
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

    /// The same three records with Russian labels and a Russian heading.
    const RUSSIAN_LISTING: &str = "\u{41f}\u{440}\u{43e}\u{433}\u{440}\u{430}\u{43c}\u{43c}\u{430} Microsoft PnP Utility\n\n\u{41e}\u{43f}\u{443}\u{431}\u{43b}\u{438}\u{43a}\u{43e}\u{432}\u{430}\u{43d}\u{43d}\u{43e}\u{435} \u{438}\u{43c}\u{44f}:     oem49.inf\n\u{418}\u{441}\u{445}\u{43e}\u{434}\u{43d}\u{43e}\u{435} \u{438}\u{43c}\u{44f}:            logi_joy_bus_enum.inf\n\u{418}\u{43c}\u{44f} \u{43f}\u{43e}\u{441}\u{442}\u{430}\u{432}\u{449}\u{438}\u{43a}\u{430}:        Logitech\n\u{418}\u{43c}\u{44f} \u{43a}\u{43b}\u{430}\u{441}\u{441}\u{430}:           \u{421}\u{438}\u{441}\u{442}\u{435}\u{43c}\u{43d}\u{44b}\u{435} \u{443}\u{441}\u{442}\u{440}\u{43e}\u{439}\u{441}\u{442}\u{432}\u{430}\n\u{412}\u{435}\u{440}\u{441}\u{438}\u{44f} \u{434}\u{440}\u{430}\u{439}\u{432}\u{435}\u{440}\u{430}:      02.09.2022 14:30:00\n\n\u{41e}\u{43f}\u{443}\u{431}\u{43b}\u{438}\u{43a}\u{43e}\u{432}\u{430}\u{43d}\u{43d}\u{43e}\u{435} \u{438}\u{43c}\u{44f}:     oem50.inf\n\u{418}\u{441}\u{445}\u{43e}\u{434}\u{43d}\u{43e}\u{435} \u{438}\u{43c}\u{44f}:            logi_joy_vir_hid.inf\n\u{418}\u{43c}\u{44f} \u{43f}\u{43e}\u{441}\u{442}\u{430}\u{432}\u{449}\u{438}\u{43a}\u{430}:        Logitech\n\n\u{41e}\u{43f}\u{443}\u{431}\u{43b}\u{438}\u{43a}\u{43e}\u{432}\u{430}\u{43d}\u{43d}\u{43e}\u{435} \u{438}\u{43c}\u{44f}:     oem12.inf\n\u{418}\u{441}\u{445}\u{43e}\u{434}\u{43d}\u{43e}\u{435} \u{438}\u{43c}\u{44f}:            nvidia_display.inf\n\u{418}\u{43c}\u{44f} \u{43f}\u{43e}\u{441}\u{442}\u{430}\u{432}\u{449}\u{438}\u{43a}\u{430}:        NVIDIA\n";

    /// The bus package on a Ukrainian machine.
    const UKRAINIAN_LISTING: &str = "\u{41e}\u{43f}\u{443}\u{431}\u{43b}\u{456}\u{43a}\u{43e}\u{432}\u{430}\u{43d}\u{430} \u{43d}\u{430}\u{437}\u{432}\u{430}:     oem49.inf\n\u{41e}\u{440}\u{438}\u{433}\u{456}\u{43d}\u{430}\u{43b}\u{44c}\u{43d}\u{430} \u{43d}\u{430}\u{437}\u{432}\u{430}:    logi_joy_bus_enum.inf\n\u{41d}\u{430}\u{437}\u{432}\u{430} \u{43f}\u{43e}\u{441}\u{442}\u{430}\u{447}\u{430}\u{43b}\u{44c}\u{43d}\u{438}\u{43a}\u{430}: Logitech\n";

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
    fn a_listing_with_russian_labels_reads_exactly_like_the_english_one() {
        assert_eq!(parse_packages(RUSSIAN_LISTING), parse_packages(LISTING));
        assert_eq!(published_names(RUSSIAN_LISTING), ["oem49.inf", "oem50.inf"]);
    }

    #[test]
    fn a_listing_with_ukrainian_labels_is_read_as_well() {
        assert_eq!(published_names(UKRAINIAN_LISTING), ["oem49.inf"]);
    }

    #[test]
    fn the_heading_above_the_records_is_not_mistaken_for_one() {
        // Whatever the utility greets us with, it does not name an .inf file.
        let listing = format!("Some translated heading: with a colon in it\n\n{LISTING}");

        assert_eq!(parse_packages(&listing), parse_packages(LISTING));
    }

    #[test]
    fn a_time_in_a_later_field_does_not_disturb_the_record_it_belongs_to() {
        // A localised date field holds colons of its own.
        let packages = parse_packages(RUSSIAN_LISTING);

        assert_eq!(packages[0].published_name, "oem49.inf");
        assert_eq!(packages[0].provider, "Logitech");
    }

    #[test]
    fn a_package_from_another_vendor_with_our_file_name_is_not_ours() {
        let impostor = Package {
            published_name: "oem99.inf".to_owned(),
            original_name: BUS_PACKAGE.to_owned(),
            provider: "Some Other Vendor".to_owned(),
        };

        assert!(!impostor.is_ours());
    }

    #[test]
    fn the_case_the_listing_prints_the_file_name_in_does_not_matter() {
        let shouting = Package {
            published_name: "oem49.inf".to_owned(),
            original_name: BUS_PACKAGE.to_uppercase(),
            provider: "Logitech".to_owned(),
        };

        assert!(shouting.is_ours());
    }

    #[test]
    fn a_package_whose_name_only_starts_like_ours_is_not_ours() {
        let neighbour = Package {
            published_name: "oem51.inf".to_owned(),
            original_name: "logi_joy_bus_enum_v2.inf".to_owned(),
            provider: "Logitech".to_owned(),
        };

        assert!(!neighbour.is_ours());
    }

    #[test]
    fn a_listing_without_a_trailing_blank_line_still_yields_its_last_record() {
        let packages = parse_packages(
            "Published Name:     oem49.inf\nOriginal Name:      logi_joy_bus_enum.inf\nProvider Name:      Logitech",
        );

        assert_eq!(packages.len(), 1);
        assert!(packages[0].is_ours());
    }

    #[test]
    fn a_record_that_stops_before_the_provider_is_not_a_record() {
        let packages = parse_packages(
            "Published Name:     oem49.inf\nOriginal Name:      logi_joy_bus_enum.inf\n",
        );

        assert!(packages.is_empty());
    }

    #[test]
    fn an_empty_listing_yields_nothing_rather_than_an_empty_record() {
        assert!(parse_packages("").is_empty());
        assert!(parse_packages("Microsoft PnP Utility\n\n").is_empty());
    }

    /// Counting must not remove anything: on a machine without the driver both
    /// answer zero, and counting twice keeps answering the same.
    #[test]
    fn counting_the_leftover_devices_leaves_them_where_they_are() {
        assert_eq!(leftover_devices(), leftover_devices());
    }

    /// Looking for a hardware id nothing answers to must end quietly and give
    /// the set it opened back.
    #[test]
    fn a_hardware_id_nothing_answers_to_finds_no_devices() {
        assert!(find_devices(r"root\NoSuchDeviceOfOurs").is_empty());
    }

    /// Windows did the work there and then, so there is nothing to read.
    #[test]
    fn an_installation_that_finished_now_is_not_waiting_on_a_restart() {
        let answer = is_the_build_we_speak_to(Path::new(r"Z:\no\such\folder"), Finished::Now);

        assert!(answer.is_ok());
    }

    /// A deferred installation is only a reason to stop if the build that
    /// answers is still the wrong one. Where neither version can be read there
    /// is no evidence of that.
    #[test]
    fn a_deferred_installation_with_nothing_to_compare_carries_on() {
        let answer =
            is_the_build_we_speak_to(Path::new(r"Z:\no\such\folder"), Finished::AfterRestart);

        assert!(answer.is_ok());
    }

    /// "Restart the computer" without a reason is advice nobody acts on.
    #[test]
    fn a_restart_that_is_really_owed_says_which_build_is_in_the_way() {
        let error = Error::RestartRequired(format!(
            "Windows keeps version {} of the Logitech driver loaded until the computer restarts, \
             and the {} this program speaks to cannot take over until then",
            version::Version::from_words(0x07e8_0001, 0),
            version::Version::from_words(0x07e5_0001, 0x0555_0000)
        ));
        let text = error.to_string();

        assert!(text.contains("2024.1.0.0"), "{text:?} hides what is loaded");
        assert!(
            text.contains("2021.1.1365.0"),
            "{text:?} hides what we speak"
        );
        assert!(text.contains("restart"), "{text:?} does not say what to do");
    }

    #[test]
    fn a_failure_nobody_can_be_blamed_for_says_nothing_extra() {
        assert_eq!(blame(&[]), "");
    }

    #[test]
    fn the_holders_are_added_to_the_failure_as_a_readable_sentence() {
        let holders = vec!["System (process 4)".to_owned(), "lghub.exe".to_owned()];

        assert_eq!(
            blame(&holders),
            "; the driver files are held by System (process 4), lghub.exe"
        );
    }

    /// The whole message has to read as one sentence: a code, then who is in
    /// the way.
    #[test]
    fn a_failure_with_its_blame_reads_as_one_sentence() {
        let holders = vec!["System (process 4)".to_owned()];
        let message = format!(
            "oem49.inf could not be replaced (code 259){}",
            blame(&holders)
        );

        assert_eq!(
            message,
            "oem49.inf could not be replaced (code 259); the driver files are held by System \
             (process 4)"
        );
    }
}
