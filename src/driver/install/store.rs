//! Finding our packages in the driver store without reading any text.
//!
//! Removing a package needs the name Windows published it under, such as
//! `oem49.inf`. The obvious way to learn that name is `pnputil /enum-drivers`,
//! but the field labels of that listing are translated: on a Russian or
//! Ukrainian Windows they are not the words an English-language parser was
//! written against, so a program that matches them decides the driver is not
//! installed and refuses to remove it. The user is then left with a driver that
//! only the Device Manager can take out.
//!
//! Windows keeps the same information in its own driver database, indexed by the
//! .inf file name a package was published from. That name is ours -
//! `logi_joy_bus_enum.inf` is spelled the same on every machine in every
//! language - so nothing here depends on the display language. That is also
//! what makes it testable: there is no translated text left in the path, so an
//! English machine exercises exactly what a Russian one would.

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
    KEY_WOW64_64KEY, REG_SAM_FLAGS,
};

use super::wide;

/// The driver database's index of published .inf files.
const DRIVER_INF_FILES: &str = r"SYSTEM\DriverDatabase\DriverInfFiles";

/// Names the copy of a package that Windows is actually using.
const ACTIVE: &str = "Active";

/// Every name `package` is published under in the driver store, such as
/// `oem49.inf`.
///
/// Empty when the package is not installed, and also - deliberately - when the
/// database is not laid out the way this code expects. The caller keeps a
/// second way of finding out for that case rather than concluding from silence
/// here that there is nothing to remove.
#[must_use]
pub fn published_names(package: &str) -> Vec<String> {
    let Some(key) = Key::open(&format!(r"{DRIVER_INF_FILES}\{package}")) else {
        return Vec::new();
    };

    // The unnamed value lists every published copy as a `REG_MULTI_SZ`, and
    // `Active` names the one in use. Both are read and folded together, so a
    // machine that carries only one of them is still understood.
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
    /// Opens a key under `HKEY_LOCAL_MACHINE` for reading, or gives up quietly.
    ///
    /// The 64-bit view is asked for explicitly: the driver database lives there
    /// and nowhere else, and a 32-bit build reading its own redirected view
    /// would find an empty key rather than an error.
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

    /// Reads a value that holds text, whether it is one string or several.
    ///
    /// A `REG_SZ` and a `REG_MULTI_SZ` are both UTF-16 with a null after every
    /// string, so splitting on the nulls covers both and there is no reason to
    /// ask which one it is. An empty `name` means the unnamed value.
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

    /// Reads a value as the UTF-16 units it is stored as.
    ///
    /// The size is asked for first rather than guessed: the list of published
    /// names grows with every copy of the package Windows has kept.
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

        // One unit of slack: a value written without its final null is still
        // read as text rather than running off the end of the buffer.
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

    /// The lookup answers rather than panicking when the package is not there,
    /// which is the case on every machine without our driver. The value of this
    /// test is that it runs the whole registry path - opening a missing key,
    /// giving up, cleaning up - on any machine, in any language.
    #[test]
    fn a_package_that_was_never_installed_has_no_published_names() {
        assert!(published_names("input_redirect_no_such_package.inf").is_empty());
    }

    /// The path is built from the package name, so a name that cannot appear in
    /// the database must not be able to reach a different key either.
    #[test]
    fn a_nonsense_package_name_finds_nothing_instead_of_wandering_off() {
        for package in ["", "..", r"..\..\SYSTEM", "logi_joy_bus_enum"] {
            assert!(published_names(package).is_empty(), "{package:?}");
        }
    }
}
