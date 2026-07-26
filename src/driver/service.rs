//! The three kernel services the driver package installs.

use windows::core::PCWSTR;
use windows::Win32::System::Services::{
    CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx,
    StartServiceW, SC_HANDLE, SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_CONTROL_STOP,
    SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START, SERVICE_STATUS, SERVICE_STATUS_PROCESS,
    SERVICE_STOP,
};

use super::wide;

/// The bus is the one that tells us whether the package is installed at all;
/// the other two are brought up by plug and play behind it.
pub const BUS: &str = "logi_joy_bus_enum";
pub const CORE: &str = "logi_joy_xlcore";
pub const VIRTUAL_HID: &str = "logi_joy_vir_hid";

/// Start order. Stopping walks it backwards.
const ALL: [&str; 3] = [BUS, CORE, VIRTUAL_HID];

struct ServiceHandle {
    manager: SC_HANDLE,
    service: SC_HANDLE,
}

impl ServiceHandle {
    fn open(name: &str, access: u32) -> Option<Self> {
        let name = wide(name);

        // SAFETY: both handles are closed in Drop, and the name outlives the call.
        unsafe {
            let manager =
                OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT).ok()?;
            match OpenServiceW(manager, PCWSTR(name.as_ptr()), access) {
                Ok(service) => Some(Self { manager, service }),
                Err(_) => {
                    let _ = CloseServiceHandle(manager);
                    None
                }
            }
        }
    }
}

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        // SAFETY: both handles came from a successful open above.
        unsafe {
            let _ = CloseServiceHandle(self.service);
            let _ = CloseServiceHandle(self.manager);
        }
    }
}

#[must_use]
pub fn is_present(name: &str) -> bool {
    ServiceHandle::open(name, SERVICE_QUERY_STATUS).is_some()
}

#[must_use]
pub fn is_running(name: &str) -> bool {
    let Some(handle) = ServiceHandle::open(name, SERVICE_QUERY_STATUS) else {
        return false;
    };

    // Windows wants a byte buffer, so it gets one. Handing it a slice made out
    // of a live `SERVICE_STATUS_PROCESS` would leave two paths to the same
    // bytes at once, one of them typed, which is exactly what the compiler is
    // allowed to assume never happens.
    let mut buffer = [0u8; std::mem::size_of::<SERVICE_STATUS_PROCESS>()];
    let mut needed = 0u32;

    // SAFETY: the buffer is exactly the size the API is told it is.
    let queried = unsafe {
        QueryServiceStatusEx(
            handle.service,
            SC_STATUS_PROCESS_INFO,
            Some(&mut buffer),
            &mut needed,
        )
    };
    if queried.is_err() {
        return false;
    }

    // SAFETY: the call above filled the whole buffer with the structure it was
    // asked for; the read makes no assumption about the buffer's alignment.
    let status: SERVICE_STATUS_PROCESS =
        unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast()) };

    status.dwCurrentState == SERVICE_RUNNING
}

pub fn start(name: &str) -> bool {
    let Some(handle) = ServiceHandle::open(name, SERVICE_START) else {
        return false;
    };

    // SAFETY: the handle was opened with SERVICE_START.
    unsafe { StartServiceW(handle.service, None).is_ok() }
}

pub fn stop(name: &str) -> bool {
    let Some(handle) = ServiceHandle::open(name, SERVICE_STOP) else {
        return false;
    };

    let mut status = SERVICE_STATUS::default();

    // SAFETY: the handle was opened with SERVICE_STOP and status is ours.
    unsafe { ControlService(handle.service, SERVICE_CONTROL_STOP, &mut status).is_ok() }
}

pub fn start_all() {
    for name in ALL {
        let _ = start(name);
    }
}

/// Children before their parent, or the bus refuses to go down.
pub fn stop_all() {
    for name in ALL.iter().rev() {
        let _ = stop(name);
    }
}

#[must_use]
pub fn driver_installed() -> bool {
    is_present(BUS)
}

#[must_use]
pub fn driver_running() -> bool {
    is_running(BUS) && is_running(CORE)
}
