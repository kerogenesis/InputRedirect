//! Remembering, across a reboot, that a reboot is still owed.
//!
//! Removing the driver leaves the .sys images loaded in the kernel until the
//! machine restarts. A volatile registry value is nearly the whole answer: it
//! survives the program exiting and disappears when Windows starts again.
//!
//! "Nearly", because the value is only volatile while the key that holds it is,
//! and `REG_OPTION_VOLATILE` is honoured when the key is *created*. A key left
//! behind by an older build - or created by anything else under the same name -
//! would keep the flag forever, and the program would refuse to start for good.
//! So the flag is also cleared explicitly, the moment the driver answers again.

use std::process::Command;

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyExW, RegDeleteValueW, RegOpenKeyExW,
    RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, KEY_WRITE,
    REG_OPTION_VOLATILE, REG_SAM_FLAGS,
};

use super::wide;

const KEY_PATH: &str = r"SOFTWARE\InputRedirect";
const VALUE_NAME: &str = "RestartPending";

/// Records that the machine has to restart before the driver is really gone.
pub fn mark_restart_pending() {
    let path = wide(KEY_PATH);
    let name = wide(VALUE_NAME);
    let mut key = HKEY::default();

    // SAFETY: the key is closed below; every pointer outlives the call.
    unsafe {
        // Delete any existing key first, so the create below always makes a
        // fresh one - and `REG_OPTION_VOLATILE` is honoured only for a key that
        // is created. A persistent key left by an older build, or by anything
        // else under this name, would otherwise keep the flag across the very
        // restart meant to clear it and strand the program on the "please
        // restart" screen for good. The key is ours and holds only this flag, so
        // deleting it loses nothing, and a key that was not there is no error.
        let _ = RegDeleteKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            KEY_WOW64_64KEY.0,
            None,
        );

        let created = RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_VOLATILE,
            REG_SAM_FLAGS(KEY_WRITE.0 | KEY_WOW64_64KEY.0),
            None,
            &mut key,
            None,
        );
        if created != ERROR_SUCCESS {
            return;
        }

        let one = 1u32.to_ne_bytes();
        let _ = windows::Win32::System::Registry::RegSetValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            windows::Win32::System::Registry::REG_DWORD,
            Some(&one),
        );
        let _ = RegCloseKey(key);
    }
}

/// Forgets the flag. Called when the driver is up and answering, which is the
/// one situation in which a restart cannot still be owed - whether the machine
/// was restarted or the driver was simply installed again.
pub fn clear_restart_pending() {
    let path = wide(KEY_PATH);
    let name = wide(VALUE_NAME);
    let mut key = HKEY::default();

    // SAFETY: the key is closed on every path that opened it, and both strings
    // are null terminated and outlive the call.
    unsafe {
        if RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            None,
            REG_SAM_FLAGS(KEY_WRITE.0 | KEY_WOW64_64KEY.0),
            &mut key,
        ) != ERROR_SUCCESS
        {
            return;
        }

        let _ = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
        let _ = RegCloseKey(key);
    }
}

/// True when the driver was removed and the machine has not restarted yet.
#[must_use]
pub fn is_restart_pending() -> bool {
    let path = wide(KEY_PATH);
    let name = wide(VALUE_NAME);
    let mut key = HKEY::default();

    // SAFETY: the key is closed on every path that opened it.
    unsafe {
        if RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            None,
            REG_SAM_FLAGS(KEY_READ.0 | KEY_WOW64_64KEY.0),
            &mut key,
        ) != ERROR_SUCCESS
        {
            return false;
        }

        let mut value = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let read = RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            None,
            Some(std::ptr::addr_of_mut!(value).cast()),
            Some(&mut size),
        );
        let _ = RegCloseKey(key);

        read == ERROR_SUCCESS && value == 1
    }
}

/// Asks Windows to restart, giving the user a few seconds to read why.
pub fn request_restart() -> bool {
    Command::new("shutdown.exe")
        .args([
            "/r",
            "/t",
            "5",
            "/c",
            "InputRedirect: finishing the driver removal",
        ])
        .spawn()
        .is_ok()
}
