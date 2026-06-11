//! build.rs — Custom build script for vuma-pi5 (bare-metal aarch64)
//!
//! When the target triple is `aarch64-unknown-none` this script:
//!   1. Instructs Cargo to re-run if the linker script changes.
//!   2. Adds the crate directory to the linker search path.
//!   3. Passes `-Tlink.ld` so the linker uses our custom script.
//!   4. Forces static, no-PIE, no-start-files linking suitable for
//!      bare-metal execution.
//!
//! For hosted targets (e.g. `aarch64-unknown-linux-gnu`) the script is
//! a no-op so that normal cross-compilation still works.

use std::env;
use std::path::PathBuf;

fn main() {
    let target = env::var("TARGET").unwrap_or_default();

    // Only configure bare-metal linker settings for the freestanding target.
    if target != "aarch64-unknown-none" {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let linker_script = manifest_dir.join("link.ld");

    // Re-run build script if the linker script changes.
    println!("cargo:rerun-if-changed={}", linker_script.display());

    // Add the manifest directory to the linker search path.
    println!("cargo:rustc-link-search={}", manifest_dir.display());

    // Use our custom linker script.
    println!("cargo:rustc-link-arg=-Tlink.ld");

    // Bare-metal: no standard startup files, fully static, no PIE.
    println!("cargo:rustc-link-arg=-nostartfiles");
    println!("cargo:rustc-link-arg=-static");
    println!("cargo:rustc-link-arg=-no-pie");

    // Prevent the linker from generating a GOT (makes position-independent
    // code impossible on bare metal).
    println!("cargo:rustc-link-arg=--no-gc-sections");

    // Keep all symbol information for debugging.
    println!(
        "cargo:rustc-link-arg=-Map={}/kernel8.map",
        manifest_dir.display()
    );
}
