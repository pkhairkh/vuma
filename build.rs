//! Build script for VUMA — sets RUSTC_VERSION_* environment variables
//! so that `--version` can display the Rust compiler version.

use std::process::Command;

fn main() {
    // Get rustc version string
    let output = Command::new("rustc")
        .args(["--version"])
        .output()
        .ok();

    if let Some(output) = output {
        if output.status.success() {
            let version_str = String::from_utf8_lossy(&output.stdout);
            // Parse "rustc 1.xx.y (sha) date" format
            let parts: Vec<&str> = version_str.trim().split_whitespace().collect();
            if parts.len() >= 2 {
                let ver = parts[1]; // e.g., "1.77.0"
                let ver_parts: Vec<&str> = ver.split('.').collect();
                if ver_parts.len() >= 3 {
                    println!("cargo:rustc-env=RUSTC_VERSION_MAJOR={}", ver_parts[0]);
                    println!("cargo:rustc-env=RUSTC_VERSION_MINOR={}", ver_parts[1]);
                    println!("cargo:rustc-env=RUSTC_VERSION_PATCH={}", ver_parts[2]);
                }
            }
        }
    }

    // Fallback if parsing failed
    println!("cargo:rustc-env=RUSTC_VERSION_MAJOR={}", option_env!("RUSTC_VERSION_MAJOR").unwrap_or("1"));
    println!("cargo:rustc-env=RUSTC_VERSION_MINOR={}", option_env!("RUSTC_VERSION_MINOR").unwrap_or("?"));
    println!("cargo:rustc-env=RUSTC_VERSION_PATCH={}", option_env!("RUSTC_VERSION_PATCH").unwrap_or("?"));

    // Re-run if rustc changes (not really needed but good practice)
    println!("cargo:rerun-if-changed=build.rs");
}
