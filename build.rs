//! Embeds the icon and the manifest into the executable.
//!
//! The inputs live in `res/`: the folder holds resources the build reads, which
//! keeps `build/` from looking like a build output directory in the root.
//!
//! The icon is taken from the system shell library instead of being committed
//! as a binary blob, so the program looks native on whatever Windows build it
//! runs on. If the extraction fails the build still succeeds, only without an
//! icon - a missing icon is not worth breaking a build over.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the manifest and the icon script live, relative to the crate root.
const RESOURCES: &str = "res";

const SHELL_ICON_INDEX: &str = "120";

fn main() {
    println!("cargo::rerun-if-changed={RESOURCES}/app.manifest");
    println!("cargo::rerun-if-changed={RESOURCES}/extract_shell_icon.ps1");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is always set"));
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set"));

    let icon = out_dir.join("app.ico");
    let icon = extract_icon(&manifest_dir, &icon).then_some(icon);

    let script = out_dir.join("app.rc");
    let manifest = manifest_dir.join(RESOURCES).join("app.manifest");
    fs::write(&script, resource_script(icon.as_deref(), &manifest)).expect("write app.rc");

    // The compilation result is ignored the way version 2's unit return was:
    // embedding resources is best effort here - it is skipped when cross
    // compiling without a resource compiler - and a build must not fail over it.
    let _ = embed_resource::compile(&script, embed_resource::NONE);
}

fn extract_icon(manifest_dir: &Path, destination: &Path) -> bool {
    let script = manifest_dir.join(RESOURCES).join("extract_shell_icon.ps1");

    let status = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .arg("-Index")
        .arg(SHELL_ICON_INDEX)
        .arg("-Output")
        .arg(destination)
        .status();

    match status {
        Ok(status) if status.success() && destination.exists() => true,
        _ => {
            println!("cargo::warning=the shell icon could not be extracted, building without one");
            false
        }
    }
}

fn resource_script(icon: Option<&Path>, manifest: &Path) -> String {
    let mut script = String::new();

    if let Some(icon) = icon {
        // Writing into a String cannot fail, so the result is discarded.
        let _ = writeln!(script, "1 ICON \"{}\"", escape(icon));
    }
    // 1 = CREATEPROCESS_MANIFEST_RESOURCE_ID, 24 = RT_MANIFEST.
    let _ = writeln!(script, "1 24 \"{}\"", escape(manifest));
    script
}

fn escape(path: &Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}
