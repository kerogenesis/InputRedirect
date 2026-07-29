//! Embeds the icon and the manifest into the executable.
//!
//! The inputs live in `res/`: the folder holds resources the build reads, which
//! keeps `build/` from looking like a build output directory in the root.
//!
//! Both are committed files, so the build needs nothing beyond a resource
//! compiler - no shell, no scripts, and the same bytes on every machine.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// Where the manifest and the icon live, relative to the crate root.
const RESOURCES: &str = "res";

fn main() {
    println!("cargo::rerun-if-changed={RESOURCES}/app.manifest");
    println!("cargo::rerun-if-changed={RESOURCES}/app.ico");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is always set"));
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set"));

    let resources = manifest_dir.join(RESOURCES);
    let icon = resources.join("app.ico");
    let manifest = resources.join("app.manifest");

    let script = out_dir.join("app.rc");
    fs::write(&script, resource_script(&icon, &manifest)).expect("write app.rc");

    // The compilation result is ignored the way version 2's unit return was:
    // embedding resources is best effort here - it is skipped when cross
    // compiling without a resource compiler - and a build must not fail over it.
    let _ = embed_resource::compile(&script, embed_resource::NONE);
}

fn resource_script(icon: &Path, manifest: &Path) -> String {
    let mut script = String::new();

    // Writing into a String cannot fail, so the results are discarded.
    let _ = writeln!(script, "1 ICON \"{}\"", escape(icon));
    // 1 = CREATEPROCESS_MANIFEST_RESOURCE_ID, 24 = RT_MANIFEST.
    let _ = writeln!(script, "1 24 \"{}\"", escape(manifest));
    script
}

fn escape(path: &Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}
