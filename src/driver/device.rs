//! Finding the driver on the system and holding the handles to it.

use std::thread::sleep;
use std::time::Duration;

use windows::core::{GUID, PCWSTR};
use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::Storage::FileSystem::{
    NtCreateFile, NTCREATEFILE_CREATE_DISPOSITION, NTCREATEFILE_CREATE_OPTIONS,
};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE, UNICODE_STRING};
use windows::Win32::Storage::FileSystem::{
    FILE_ACCESS_RIGHTS, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE,
};
use windows::Win32::System::IO::IO_STATUS_BLOCK;

use crate::error::{Error, Result};
use crate::hid::{KeyboardReport, MouseReport};

use super::ioctl::{self, DeviceKind};
use super::wide;

/// Published by the core driver: the endpoint every request goes through.
const CORE_INTERFACE: GUID = GUID::from_u128(0x1abc_05c0_c378_41b9_9cef_df1a_ba82_b015);

/// How long plug and play is given to enumerate a new child device.
///
/// This one stays a pause rather than becoming a wait for something: the driver
/// answers the request before Windows has finished building the device, and
/// there is no reliable thing to look at in between.
const PLUG_SETTLE: Duration = Duration::from_millis(200);

/// A device information set that Windows takes back when it goes out of scope.
///
/// The set holds enumeration state on the kernel side, and every user of one is
/// a loop with early exits in it, so the destroy call belongs to the scope
/// rather than to a line at the end of it.
pub(super) struct DeviceInfoSet(HDEVINFO);

impl DeviceInfoSet {
    pub(super) fn new(set: HDEVINFO) -> Self {
        Self(set)
    }

    pub(super) fn handle(&self) -> HDEVINFO {
        self.0
    }
}

impl Drop for DeviceInfoSet {
    fn drop(&mut self) {
        // SAFETY: the set came from SetupDi and is destroyed exactly once.
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

/// The id the driver hands back for a device it created.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualDeviceId(u32);

/// The open connection to the driver.
pub struct Devices {
    core: Option<HANDLE>,
}

// SAFETY: the handle is an opaque kernel handle rather than a pointer into this
// process, so nothing in this type is tied to the thread that opened it.
unsafe impl Send for Devices {}

// SAFETY: the driver serialises the requests it receives, and every method here
// takes the connection by shared reference and mutates nothing behind it. The
// hook thread and the interface thread may therefore share one connection.
unsafe impl Sync for Devices {}

impl Devices {
    pub fn open() -> Result<Self> {
        let mut candidates = interface_paths(&CORE_INTERFACE);

        // The interface is normally discoverable, but on a machine where plug
        // and play has not caught up yet the two well known slots still work.
        candidates.push(r"\??\ROOT#SYSTEM#0001#{1abc05c0-c378-41b9-9cef-df1aba82b015}".to_owned());
        candidates.push(r"\??\ROOT#SYSTEM#0002#{1abc05c0-c378-41b9-9cef-df1aba82b015}".to_owned());

        candidates
            .iter()
            .find_map(|path| open_native(path))
            .map(|core| Self { core: Some(core) })
            .ok_or_else(|| {
                Error::Device(
                    "the driver is installed but its device could not be opened".to_owned(),
                )
            })
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.core.is_some()
    }

    pub fn close(&mut self) {
        if let Some(handle) = self.core.take() {
            // SAFETY: the handle came from NtCreateFile and is closed once.
            unsafe {
                let _ = CloseHandle(handle);
            }
        }
    }

    fn handle(&self) -> Result<HANDLE> {
        self.core
            .ok_or_else(|| Error::Device("the connection to the driver is closed".to_owned()))
    }

    pub fn plug_keyboard(&self) -> Result<VirtualDeviceId> {
        self.plug(DeviceKind::Keyboard)
    }

    pub fn plug_mouse(&self) -> Result<VirtualDeviceId> {
        self.plug(DeviceKind::Mouse)
    }

    /// Creates one virtual device and reads back the id the driver gave it.
    ///
    /// The id can come back as zero, meaning the driver created the device but
    /// did not say which one it is. The device works, but it cannot be asked
    /// for by id afterwards, so [`Self::unplug`] turns that into a refusal and
    /// the caller takes the device off the bus the hard way.
    fn plug(&self, kind: DeviceKind) -> Result<VirtualDeviceId> {
        let handle = self.handle()?;
        let mut payload = ioctl::plug_payload(kind);

        ioctl::send(handle, ioctl::PLUG_DEVICE, &mut payload, true).map_err(|error| {
            Error::VirtualDevice {
                device: kind.label(),
                reason: error.to_string(),
            }
        })?;

        // Plug and play needs a moment to enumerate the new child device.
        sleep(PLUG_SETTLE);

        Ok(VirtualDeviceId(ioctl::plugged_device_id(&payload)))
    }

    /// Asks the driver to take a virtual device away.
    ///
    /// The answer decides what happens next: a device the driver still owns
    /// keeps its product id, and the next plug of the same device is refused
    /// with an invalid parameter that says nothing about the real reason.
    pub fn unplug(&self, id: VirtualDeviceId) -> Result<()> {
        // Zero is not an id the driver ever handed out - it is what is read
        // back when the driver named nothing - so there is nothing to ask for.
        if id.0 == 0 {
            return Err(Error::Device(
                "the driver never said which device it created".to_owned(),
            ));
        }

        let handle = self.handle()?;
        let mut request = ioctl::UnplugRequest::new(id.0).as_bytes();

        ioctl::send(handle, ioctl::UNPLUG_DEVICE, &mut request, true)
    }

    pub fn send_keyboard(&self, report: KeyboardReport) -> Result<()> {
        let mut bytes = report.to_bytes();
        ioctl::send(self.handle()?, ioctl::SEND_KEYBOARD, &mut bytes, false)
    }

    pub fn send_mouse(&self, report: MouseReport) -> Result<()> {
        let mut bytes = report.to_bytes();
        ioctl::send(self.handle()?, ioctl::SEND_MOUSE, &mut bytes, false)
    }
}

impl Drop for Devices {
    fn drop(&mut self) {
        self.close();
    }
}

/// Generic write and synchronize, the only rights a report needs.
const DEVICE_ACCESS: FILE_ACCESS_RIGHTS = FILE_ACCESS_RIGHTS(0x4010_0000);
const NORMAL_ATTRIBUTES: FILE_FLAGS_AND_ATTRIBUTES = FILE_FLAGS_AND_ATTRIBUTES(0x80);
const SHARE_READ_WRITE: FILE_SHARE_MODE = FILE_SHARE_MODE(3);
/// Open an existing device, never create one.
const OPEN_EXISTING: NTCREATEFILE_CREATE_DISPOSITION = NTCREATEFILE_CREATE_DISPOSITION(1);
/// Synchronous, and not a directory.
const SYNCHRONOUS_DEVICE: NTCREATEFILE_CREATE_OPTIONS = NTCREATEFILE_CREATE_OPTIONS(0x60);

/// Opens a device by its native object path.
fn open_native(path: &str) -> Option<HANDLE> {
    let mut path = wide(path);
    let length = ((path.len() - 1) * 2) as u16;

    let mut name = UNICODE_STRING {
        Length: length,
        MaximumLength: length + 2,
        Buffer: windows::core::PWSTR(path.as_mut_ptr()),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        ObjectName: &mut name,
        Attributes: windows::Win32::Foundation::OBJECT_ATTRIBUTE_FLAGS(0x40), // case insensitive
        ..Default::default()
    };

    let mut handle = HANDLE::default();
    let mut status_block = IO_STATUS_BLOCK::default();

    // SAFETY: every pointer handed over lives until the call returns, and the
    // handle is only used when the call reports success.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            DEVICE_ACCESS,
            &attributes,
            &mut status_block,
            None,
            NORMAL_ATTRIBUTES,
            SHARE_READ_WRITE,
            OPEN_EXISTING,
            SYNCHRONOUS_DEVICE,
            None,
            0,
        )
    };

    status.is_ok().then_some(handle)
}

/// Every path currently published for a device interface class.
fn interface_paths(interface: &GUID) -> Vec<String> {
    let mut paths = Vec::new();

    // SAFETY: every buffer handed to SetupDi is sized from the size it asked
    // for, and the set outlives the calls that read from it.
    unsafe {
        let Ok(set) = SetupDiGetClassDevsW(
            Some(interface),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        ) else {
            return paths;
        };
        let set = DeviceInfoSet::new(set);

        let mut index = 0;
        loop {
            let mut data = SP_DEVICE_INTERFACE_DATA {
                cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInterfaces(set.handle(), None, interface, index, &mut data).is_err()
            {
                break;
            }
            index += 1;

            let mut needed = 0;
            let _ = SetupDiGetDeviceInterfaceDetailW(
                set.handle(),
                &data,
                None,
                0,
                Some(&mut needed),
                None,
            );
            if needed == 0 {
                continue;
            }

            // The detail struct is read and written through a typed pointer, so
            // the buffer has to carry its alignment. A `Vec<u8>` only promises
            // one byte, so a `Vec<u32>` sized to hold `needed` bytes is used
            // instead: its four-byte alignment matches the struct's.
            let mut buffer = vec![0u32; (needed as usize).div_ceil(4)];
            let detail = buffer
                .as_mut_ptr()
                .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
            (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;

            if SetupDiGetDeviceInterfaceDetailW(
                set.handle(),
                &data,
                Some(detail),
                needed,
                None,
                None,
            )
            .is_ok()
            {
                let text = PCWSTR((*detail).DevicePath.as_ptr()).to_string();
                if let Ok(path) = text {
                    paths.push(to_native_path(&path));
                }
            }
        }
    }

    paths
}

/// Setup reports `\\?\...`, the native open call wants `\??\...`.
fn to_native_path(path: &str) -> String {
    match path.strip_prefix(r"\\?\") {
        Some(rest) => format!(r"\??\{rest}"),
        None => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_paths_are_rewritten_into_native_ones() {
        assert_eq!(
            to_native_path(r"\\?\ROOT#SYSTEM#0001#{1abc05c0}"),
            r"\??\ROOT#SYSTEM#0001#{1abc05c0}"
        );
    }

    #[test]
    fn a_path_that_is_already_native_is_left_alone() {
        let native = r"\??\ROOT#SYSTEM#0002#{1abc05c0}";

        assert_eq!(to_native_path(native), native);
    }

    /// Enumerating an interface nobody publishes has to end quietly, and the
    /// set it opened along the way has to be given back.
    #[test]
    fn an_interface_nobody_publishes_yields_no_paths() {
        let nobody = GUID::from_u128(0x0000_0000_dead_4000_8000_0000_0000_0001);

        assert!(interface_paths(&nobody).is_empty());
    }

    /// The refusal is what tells the caller to take the device off the bus
    /// itself, so it has to arrive even when there is no driver to ask.
    #[test]
    fn a_device_the_driver_never_named_cannot_be_unplugged_by_id() {
        let devices = Devices { core: None };

        assert!(devices.unplug(VirtualDeviceId(0)).is_err());
    }
}
