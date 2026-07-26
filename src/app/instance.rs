//! One copy of the program at a time.
//!
//! Two copies would each create their own pair of virtual devices and each
//! install their own hooks, and every key would then be swallowed twice.

use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

/// A name in the global namespace, so a second copy is noticed even when it
/// runs in another session.
const CLAIM: &str = r"Global\InputRedirect.SingleInstance";

/// Held for as long as this copy runs.
pub struct SingleInstance {
    claim: HANDLE,
}

impl SingleInstance {
    /// Claims the name, or reports that somebody else already holds it.
    pub fn claim() -> Option<Self> {
        // SAFETY: the handle is closed on both paths, and the name is a plain
        // string owned by this function for the duration of the call.
        unsafe {
            let claim = CreateMutexW(None, true, &HSTRING::from(CLAIM)).ok()?;

            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(claim);
                return None;
            }

            Some(Self { claim })
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: the handle was created by this type and is closed once.
        unsafe {
            let _ = CloseHandle(self.claim);
        }
    }
}
