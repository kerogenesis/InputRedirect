//! The signed driver files live inside the executable and are written out to a
//! temporary folder when they are needed.
//!
//! `include_bytes!` copies them in verbatim at compile time, and they are
//! written back out verbatim, so the hashes covered by the signed catalogues
//! still match and Windows accepts the package.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::error::{Error, Result};

struct BundledFile {
    name: &'static str,
    bytes: &'static [u8],
}

const BUNDLED_FILES: [BundledFile; 7] = [
    BundledFile {
        name: "logi_joy_bus_enum.sys",
        bytes: include_bytes!("../../drivers/logi_joy_bus_enum.sys"),
    },
    BundledFile {
        name: "logi_joy_bus_enum.inf",
        bytes: include_bytes!("../../drivers/logi_joy_bus_enum.inf"),
    },
    BundledFile {
        name: "logi_joy_bus_enum.cat",
        bytes: include_bytes!("../../drivers/logi_joy_bus_enum.cat"),
    },
    BundledFile {
        name: "logi_joy_vir_hid.sys",
        bytes: include_bytes!("../../drivers/logi_joy_vir_hid.sys"),
    },
    BundledFile {
        name: "logi_joy_vir_hid.inf",
        bytes: include_bytes!("../../drivers/logi_joy_vir_hid.inf"),
    },
    BundledFile {
        name: "logi_joy_vir_hid.cat",
        bytes: include_bytes!("../../drivers/logi_joy_vir_hid.cat"),
    },
    BundledFile {
        name: "logi_joy_xlcore.sys",
        bytes: include_bytes!("../../drivers/logi_joy_xlcore.sys"),
    },
];

/// The unpacked driver files. The folder is deleted when this value is dropped.
pub struct ExtractedDrivers {
    folder: TempDir,
}

impl ExtractedDrivers {
    pub fn unpack() -> Result<Self> {
        let folder = tempfile::Builder::new()
            .prefix("InputRedirect-")
            .tempdir()
            .map_err(|error| Error::Payload(error.to_string()))?;

        for file in &BUNDLED_FILES {
            fs::write(folder.path().join(file.name), file.bytes)
                .map_err(|error| Error::Payload(format!("{}: {error}", file.name)))?;
        }

        Ok(Self { folder })
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        self.folder.path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_carries_both_inf_packages_and_all_three_binaries() {
        let names: Vec<_> = BUNDLED_FILES.iter().map(|file| file.name).collect();

        assert!(names.contains(&"logi_joy_bus_enum.inf"));
        assert!(names.contains(&"logi_joy_vir_hid.inf"));
        assert_eq!(
            names.iter().filter(|name| name.ends_with(".sys")).count(),
            3
        );
        assert_eq!(
            names.iter().filter(|name| name.ends_with(".cat")).count(),
            2
        );
    }

    #[test]
    fn no_bundled_file_is_empty() {
        for file in &BUNDLED_FILES {
            assert!(!file.bytes.is_empty(), "{} is empty", file.name);
        }
    }

    #[test]
    fn the_catalogues_still_look_like_signed_catalogues() {
        // A PKCS#7 catalogue starts with an ASN.1 SEQUENCE. If line endings had
        // been translated somewhere along the way, this is the first thing that
        // would break.
        for file in BUNDLED_FILES
            .iter()
            .filter(|file| file.name.ends_with(".cat"))
        {
            assert_eq!(file.bytes[0], 0x30, "{} is not a DER sequence", file.name);
        }
    }

    #[test]
    fn the_drivers_are_portable_executables() {
        for file in BUNDLED_FILES
            .iter()
            .filter(|file| file.name.ends_with(".sys"))
        {
            assert_eq!(&file.bytes[..2], b"MZ", "{} is not a PE image", file.name);
        }
    }

    #[test]
    fn unpacking_writes_every_file_and_cleans_up_after_itself() {
        let path = {
            let extracted = ExtractedDrivers::unpack().expect("unpack");
            for file in &BUNDLED_FILES {
                let written = std::fs::read(extracted.directory().join(file.name)).expect("read");
                assert_eq!(
                    written, file.bytes,
                    "{} was not written verbatim",
                    file.name
                );
            }
            extracted.directory().to_path_buf()
        };

        assert!(!path.exists(), "the temporary folder outlived the value");
    }
}
