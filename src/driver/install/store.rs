//! Finding our packages in the driver store without reading any text.

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, REG_SAM_FLAGS, RegCloseKey, RegOpenKeyExW,
    RegQueryValueExW,
};
use windows::core::PCWSTR;

use crate::driver::wide;

/// The driver database's index of published .inf files.
const DRIVER_INF_FILES: &str = r"SYSTEM\DriverDatabase\DriverInfFiles";

/// Names the copy of a package that Windows is actually using.
const ACTIVE: &str = "Active";

/// Every name `package` is published under in the driver store, such as
/// `oem49.inf`.
#[must_use]
pub fn published_names(package: &str) -> Vec<String> {
    let Some(key) = Key::open(&format!(r"{DRIVER_INF_FILES}\{package}")) else {
        return Vec::new();
    };
    let mut names = key.strings("");
    names.extend(key.strings(ACTIVE));
    names.retain(|name| !name.is_empty());
    names.sort_unstable();
    names.dedup();
    names
}

/// A registry key that closes itself.
struct Key(HKEY);

impl Key {
    fn open(path: &str) -> Option<Self> {
        let path = wide(path);
        let mut key = HKEY::default();
        // SAFETY: the path is null terminated and outlives the call, and the
        // handle Windows writes back is closed by `Drop`.
        let opened = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(path.as_ptr()),
                None,
                REG_SAM_FLAGS(KEY_READ.0 | KEY_WOW64_64KEY.0),
                &mut key,
            )
        };
        (opened == ERROR_SUCCESS).then_some(Self(key))
    }

    fn strings(&self, name: &str) -> Vec<String> {
        let Some(units) = self.value(name) else {
            return Vec::new();
        };
        units
            .split(|unit| *unit == 0)
            .filter(|string| !string.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }

    fn value(&self, name: &str) -> Option<Vec<u16>> {
        let name = wide(name);
        let mut size = 0u32;
        // SAFETY: passing no buffer is how the call is asked for the size
        // alone; the name is null terminated and outlives both calls.
        let measured = unsafe {
            RegQueryValueExW(
                self.0,
                PCWSTR(name.as_ptr()),
                None,
                None,
                None,
                Some(&mut size),
            )
        };
        if measured != ERROR_SUCCESS || size == 0 {
            return None;
        }
        let mut units = vec![0u16; size as usize / 2 + 1];
        let mut size = (units.len() * 2) as u32;
        // SAFETY: the buffer is described by its real size in bytes, and it is
        // at least as large as the size measured above.
        let read = unsafe {
            RegQueryValueExW(
                self.0,
                PCWSTR(name.as_ptr()),
                None,
                None,
                Some(units.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if read != ERROR_SUCCESS {
            return None;
        }
        units.truncate(size as usize / 2);
        Some(units)
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from `RegOpenKeyExW` above and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_package_that_was_never_installed_has_no_published_names() {
        assert!(published_names("input_redirect_no_such_package.inf").is_empty());
    }

    #[test]
    fn a_nonsense_package_name_finds_nothing_instead_of_wandering_off() {
        for package in ["", "..", r"..\..\SYSTEM", "logi_joy_bus_enum"] {
            assert!(published_names(package).is_empty(), "{package:?}");
        }
    }
}
