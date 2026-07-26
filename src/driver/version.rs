//! Which build of the Logitech driver is on the machine.
//!
//! The requests this program sends were read out of one particular build: the
//! layout of the plug request, the codes that carry it and the place the new
//! device's number appears in the answer all belong to that build. A different
//! build may lay them out differently, and the only thing it says about a
//! request it does not understand is that a parameter was invalid - which is
//! what it also says when a product id is merely taken. There is no way to tell
//! those two apart from the outside, so the version is compared beforehand and
//! the build this program was written against is the one that runs.

use std::ffi::{c_void, OsString};
use std::fmt;
use std::mem::size_of;
use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
};

use super::wide;

/// The three binaries the package installs.
///
/// `logi_joy_xlcore.sys` is on the list on purpose: it is the one every request
/// goes through, and a comparison that only looked at the bus and the HID
/// driver would miss the file that decides whether the protocol is understood.
const BINARIES: [&str; 3] = [
    "logi_joy_bus_enum.sys",
    "logi_joy_xlcore.sys",
    "logi_joy_vir_hid.sys",
];

/// A driver on the machine that is not the build this program speaks to.
///
/// Both versions are carried, not just the fact that they differ, because that
/// is what the user is shown: "replacing 2024.1.0.0 with 2021.1.1365.0" is a
/// sentence they can quote, while "replacing the driver" is one they can only
/// wonder about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mismatch {
    pub installed: Version,
    pub ours: Version,
}

/// The first installed binary whose version is not the one this program carries,
/// with both versions.
///
/// One is enough to decide: the packages are replaced together, because they
/// carry the same file names whatever build they come from. A binary that is not
/// installed at all is not a mismatch - there is nothing to replace, and the
/// installer is about to put ours there anyway. Neither is one whose version
/// cannot be read: a file that will not answer is not evidence of anything, and
/// reinstalling the driver on a guess would be worse than leaving it alone.
pub fn mismatch(bundled: &Path) -> Option<Mismatch> {
    BINARIES.into_iter().find_map(|name| {
        let installed = Version::of(&installed_path(name))?;
        let ours = Version::of(&bundled.join(name))?;

        (installed != ours).then_some(Mismatch { installed, ours })
    })
}

/// Where the loaded copies of the three binaries are, whether or not they are
/// there at the moment.
///
/// These are the files a failed removal is about, which is why the paths are
/// published rather than kept: whoever reports such a failure has to ask who is
/// holding exactly these three open.
pub fn installed_binaries() -> Vec<PathBuf> {
    BINARIES.into_iter().map(installed_path).collect()
}

/// Where Windows keeps the copy of a driver binary it loads.
fn installed_path(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));

    Path::new(&root).join("System32").join("drivers").join(name)
}

/// The four numbers Windows shows as the file version of a driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    parts: [u16; 4],
}

impl Version {
    /// Reads the version out of a file, or nothing if the file is missing or
    /// carries no version at all.
    fn of(file: &Path) -> Option<Self> {
        let path = wide(&file.display().to_string());
        let root = wide(r"\");

        // The length Windows reports back is in bytes, and it is taken in bytes
        // here as well: what is behind the pointer is only a version block if
        // there is a whole one of them there.
        let expected = size_of::<VS_FIXEDFILEINFO>() as u32;

        // SAFETY: the block is sized by the call above and outlives every read
        // from it; the pointer the query hands back points inside that block,
        // and its length is checked before it is read through.
        unsafe {
            let size = GetFileVersionInfoSizeW(PCWSTR(path.as_ptr()), None);
            if size == 0 {
                return None;
            }

            let mut block = vec![0u8; size as usize];
            GetFileVersionInfoW(PCWSTR(path.as_ptr()), None, size, block.as_mut_ptr().cast())
                .ok()?;

            let mut fixed: *mut c_void = std::ptr::null_mut();
            let mut length = 0u32;
            let found = VerQueryValueW(
                block.as_ptr().cast(),
                PCWSTR(root.as_ptr()),
                &mut fixed,
                &mut length,
            );

            if !found.as_bool() || fixed.is_null() || length < expected {
                return None;
            }

            let info: VS_FIXEDFILEINFO = std::ptr::read_unaligned(fixed.cast());

            Some(Self::from_words(info.dwFileVersionMS, info.dwFileVersionLS))
        }
    }

    /// The version arrives as two words, each holding two of the four numbers.
    pub fn from_words(most: u32, least: u32) -> Self {
        Self {
            parts: [
                (most >> 16) as u16,
                (most & 0xffff) as u16,
                (least >> 16) as u16,
                (least & 0xffff) as u16,
            ],
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [major, minor, build, revision] = self.parts;

        write!(formatter, "{major}.{minor}.{build}.{revision}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The build both .inf files in this repository declare.
    const OURS: (u32, u32) = (0x07e5_0001, 0x0555_0000);

    #[test]
    fn a_version_reads_the_way_the_driver_package_writes_it() {
        let version = Version::from_words(OURS.0, OURS.1);

        assert_eq!(version.to_string(), "2021.1.1365.0");
    }

    #[test]
    fn two_builds_that_differ_in_any_one_number_are_not_the_same_build() {
        let ours = Version::from_words(OURS.0, OURS.1);

        assert_eq!(ours, Version::from_words(OURS.0, OURS.1));
        assert_ne!(ours, Version::from_words(OURS.0, 0x0555_0001));
        assert_ne!(ours, Version::from_words(0x07e8_0001, OURS.1));
    }

    #[test]
    fn a_mismatch_carries_the_build_on_the_machine_and_the_one_we_speak_to() {
        let mismatch = Mismatch {
            installed: Version::from_words(0x07e8_0001, 0),
            ours: Version::from_words(OURS.0, OURS.1),
        };

        assert_eq!(mismatch.installed.to_string(), "2024.1.0.0");
        assert_eq!(mismatch.ours.to_string(), "2021.1.1365.0");
    }

    #[test]
    fn the_loaded_copy_is_looked_for_where_windows_keeps_it() {
        let path = installed_path("logi_joy_xlcore.sys");

        assert!(path.is_absolute());
        assert!(path.ends_with(Path::new("drivers").join("logi_joy_xlcore.sys")));
    }

    #[test]
    fn every_binary_of_the_package_is_looked_for() {
        let installed = installed_binaries();

        assert_eq!(installed.len(), BINARIES.len());
        assert!(installed.iter().all(|path| path.is_absolute()));
    }

    #[test]
    fn a_version_that_cannot_be_read_is_not_a_reason_to_reinstall() {
        // Neither side of the comparison exists here, and a file that will not
        // answer must not be reported as a different build.
        assert!(mismatch(Path::new(r"Z:\no\such\folder")).is_none());
    }
}
