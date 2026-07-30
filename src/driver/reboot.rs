//! Remembering, across a reboot, that a reboot is still owed.
//!
//! Removing the driver leaves the .sys images loaded in the kernel until the
//! machine restarts. A volatile registry value survives the program exiting and
//! disappears when Windows starts again.
//!
//! `REG_OPTION_VOLATILE` is honoured only when the key is *created*, so a key
//! left behind by an older build would keep the flag forever and the program
//! would refuse to start for good. The flag is therefore also cleared
//! explicitly, the moment the driver answers again.

use std::process::Command;

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY,
    KEY_WRITE, REG_DWORD, REG_OPTION_VOLATILE, REG_SAM_FLAGS,
};
use windows_registry::LOCAL_MACHINE;

use super::{system32, wide};

const KEY_PATH: &str = r"SOFTWARE\InputRedirect";
const VALUE_NAME: &str = "RestartPending";

/// Records that the machine has to restart before the driver is really gone.
///
/// Uses raw Win32 APIs directly because `REG_OPTION_VOLATILE` is not exposed
/// by `windows-registry`, and the delete-then-create pattern that ensures the
/// key is always volatile has no safe equivalent in that crate.
pub fn mark_restart_pending() {
    let path = wide(KEY_PATH);
    let name = wide(VALUE_NAME);
    let mut key = HKEY::default();

    // SAFETY: the key is closed below; every pointer outlives the call.
    unsafe {
        // Deleted first so the create below always makes a fresh - and
        // therefore volatile - key. The key is ours and holds only this flag,
        // and a key that was not there is no error.
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
            REG_DWORD,
            Some(&one),
        );
        let _ = RegCloseKey(key);
    }
}

/// Forgets the flag. Called when the driver is up and answering, which is the
/// one situation in which a restart cannot still be owed.
pub fn clear_restart_pending() {
    if let Ok(key) = LOCAL_MACHINE.options().write().open(KEY_PATH) {
        let _ = key.remove_value(VALUE_NAME);
    }
}

/// True when the driver was removed and the machine has not restarted yet.
#[must_use]
pub fn is_restart_pending() -> bool {
    LOCAL_MACHINE
        .options()
        .read()
        .open(KEY_PATH)
        .and_then(|key| key.get_u32(VALUE_NAME))
        .is_ok_and(|value| value == 1)
}

/// Asks Windows to restart, giving the user a few seconds to read why.
pub fn request_restart() -> bool {
    Command::new(system32("shutdown.exe"))
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
