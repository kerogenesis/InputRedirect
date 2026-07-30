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

use windows_registry::LOCAL_MACHINE;

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
    let path = format!(r"{DRIVER_INF_FILES}\{package}");

    let Ok(key) = LOCAL_MACHINE.options().read().open(&path) else {
        return Vec::new();
    };

    // The unnamed value lists every published copy as a `REG_MULTI_SZ`, and
    // `Active` names the one in use. Both are read and folded together, so a
    // machine that carries only one of them is still understood.
    //
    // `REG_MULTI_SZ` and `REG_SZ` share the same encoding: UTF-16 strings
    // separated by null terminators. Reading the raw bytes and splitting on
    // nulls covers both without consulting the type field.
    let mut names: Vec<String> = Vec::new();
    if let Ok(bytes) = key.get_bytes("") {
        names.extend(strings_from_utf16_bytes(&bytes));
    }
    if let Ok(bytes) = key.get_bytes(ACTIVE) {
        names.extend(strings_from_utf16_bytes(&bytes));
    }

    names.retain(|name| !name.is_empty());
    names.sort_unstable();
    names.dedup();

    names
}

/// Splits raw UTF-16 LE registry bytes into strings.
///
/// Both `REG_SZ` and `REG_MULTI_SZ` are sequences of null-terminated UTF-16
/// strings: splitting on `0u16` covers both. A byte count that is not even
/// cannot be a valid UTF-16 sequence and is returned as empty.
fn strings_from_utf16_bytes(bytes: &[u8]) -> Vec<String> {
    if bytes.len() % 2 != 0 {
        return Vec::new();
    }

    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();

    units
        .split(|&unit| unit == 0)
        .filter(|string| !string.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
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

    #[test]
    fn a_single_string_is_returned_as_one_item() {
        let bytes: Vec<u8> = "oem49.inf"
            .encode_utf16()
            .chain(std::iter::once(0u16))
            .flat_map(|unit| unit.to_le_bytes())
            .collect();

        assert_eq!(strings_from_utf16_bytes(&bytes), ["oem49.inf"]);
    }

    #[test]
    fn two_strings_separated_by_a_null_are_both_returned() {
        let units: Vec<u16> = "oem49.inf"
            .encode_utf16()
            .chain([0, /* second string */ ])
            .chain("oem50.inf".encode_utf16())
            .chain([0])
            .collect();
        let bytes: Vec<u8> = units
            .iter()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();

        let mut names = strings_from_utf16_bytes(&bytes);
        names.sort_unstable();
        assert_eq!(names, ["oem49.inf", "oem50.inf"]);
    }

    #[test]
    fn an_odd_byte_count_yields_nothing() {
        assert!(strings_from_utf16_bytes(&[0x41]).is_empty());
    }

    #[test]
    fn an_empty_slice_yields_nothing() {
        assert!(strings_from_utf16_bytes(&[]).is_empty());
    }
}
