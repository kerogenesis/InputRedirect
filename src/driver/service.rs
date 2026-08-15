//! The three kernel services the driver package installs.

use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::Win32::System::Services::{
    CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx,
    SC_HANDLE, SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_CONTROL_STOP,
    SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START, SERVICE_START_PENDING, SERVICE_STATUS,
    SERVICE_STATUS_PROCESS, SERVICE_STOP, SERVICE_STOP_PENDING, SERVICE_STOPPED, StartServiceW,
};
use windows::core::PCWSTR;

use super::wide;

/// The bus is the one that tells us whether the package is installed at all;
/// the other two are brought up by plug and play behind it.
pub const BUS: &str = "logi_joy_bus_enum";
pub const CORE: &str = "logi_joy_xlcore";
pub const VIRTUAL_HID: &str = "logi_joy_vir_hid";

/// Start order. Stopping walks it backwards.
const ALL: [&str; 3] = [BUS, CORE, VIRTUAL_HID];

/// How long a service is given to reach the state it was asked for, and how
/// often to look. The SCM accepts the request immediately; reaching the state
/// is what takes the time.
const TRANSITION_TIMEOUT: Duration = Duration::from_secs(3);
const TRANSITION_POLL: Duration = Duration::from_millis(50);

/// The SCM states a service can be in, as plain numbers the rest of this file
/// can match on.
const STOPPED: u32 = SERVICE_STOPPED.0;
const START_PENDING: u32 = SERVICE_START_PENDING.0;
const STOP_PENDING: u32 = SERVICE_STOP_PENDING.0;
const RUNNING: u32 = SERVICE_RUNNING.0;

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

/// The state a service is in, or none when there is nothing to ask.
fn state(name: &str) -> Option<u32> {
    let handle = ServiceHandle::open(name, SERVICE_QUERY_STATUS)?;
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
        return None;
    }
    // SAFETY: the call above filled the whole buffer with the structure it was
    // asked for; the read makes no assumption about the buffer's alignment.
    let status: SERVICE_STATUS_PROCESS =
        unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast()) };
    Some(status.dwCurrentState.0)
}

#[must_use]
pub fn is_running(name: &str) -> bool {
    state(name) == Some(RUNNING)
}

pub fn start(name: &str) -> bool {
    let Some(handle) = ServiceHandle::open(name, SERVICE_START) else {
        return false;
    };
    // SAFETY: the handle was opened with SERVICE_START.
    let result = unsafe { StartServiceW(handle.service, None) };
    drop(handle);
    result.is_ok()
}

pub fn stop(name: &str) -> bool {
    let Some(handle) = ServiceHandle::open(name, SERVICE_STOP) else {
        return false;
    };
    let mut status = SERVICE_STATUS::default();
    // SAFETY: the handle was opened with SERVICE_STOP and status is ours.
    let result = unsafe { ControlService(handle.service, SERVICE_CONTROL_STOP, &mut status) };
    drop(handle);
    result.is_ok()
}

/// Stops a service and waits until it is really stopped.
///
/// `true` also when there is nothing to stop: a service that is not present or
/// already stopped has already satisfied the request. A service that refuses
/// to stop answers `false`, and the caller has to decide what that blocks.
#[must_use]
pub fn stop_and_wait(name: &str) -> bool {
    match state(name) {
        None | Some(STOPPED) => return true,
        Some(STOP_PENDING) => {}
        Some(_) => {
            if !stop(name) {
                return false;
            }
        }
    }
    wait_for_state(STOPPED, || state(name))
}

/// Starts a service and waits until it is really running.
///
/// `true` also when it is already running. A service that is not there, or
/// that does not reach the running state in time, answers `false`.
#[must_use]
pub fn start_and_wait(name: &str) -> bool {
    match state(name) {
        Some(RUNNING) => return true,
        None => return false,
        Some(START_PENDING) => {}
        Some(_) => {
            if !start(name) {
                return false;
            }
        }
    }
    wait_for_state(RUNNING, || state(name))
}

/// Polls a service until it reaches `wanted` or the deadline passes.
fn wait_for_state(wanted: u32, state: impl FnMut() -> Option<u32>) -> bool {
    wait_until(Instant::now() + TRANSITION_TIMEOUT, wanted, state)
}

/// The loop itself, given its deadline outright so tests do not wait for the
/// real one.
fn wait_until(deadline: Instant, wanted: u32, mut state: impl FnMut() -> Option<u32>) -> bool {
    loop {
        if state() == Some(wanted) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(TRANSITION_POLL);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A service that walks through `states` and then stays where it ended.
    fn faked(states: &'static [u32]) -> impl FnMut() -> Option<u32> {
        let mut cursor = 0;
        move || {
            let state = states
                .get(cursor)
                .copied()
                .or_else(|| states.last().copied());
            cursor += 1;
            state
        }
    }

    #[test]
    fn a_state_that_is_already_there_is_waited_for_without_delay() {
        assert!(wait_until(
            Instant::now() + Duration::from_secs(1),
            STOPPED,
            faked(&[STOPPED]),
        ));
    }

    #[test]
    fn a_state_reached_after_a_pending_moment_is_recognised() {
        assert!(wait_until(
            Instant::now() + Duration::from_secs(1),
            STOPPED,
            faked(&[STOP_PENDING, STOPPED]),
        ));
    }

    #[test]
    fn a_state_that_never_arrives_answers_false() {
        let reached = wait_until(
            Instant::now() + Duration::from_millis(150),
            STOPPED,
            faked(&[RUNNING]),
        );

        assert!(!reached);
    }

    #[test]
    fn a_service_that_is_not_there_has_no_state() {
        assert_eq!(state(r"no\such\service\anywhere"), None);
    }
}
