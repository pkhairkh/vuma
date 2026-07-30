//! Build script for VUMA.
//!
//! Always: detect the `rustc` version and expose it as
//! `RUSTC_VERSION_{MAJOR,MINOR,PATCH}` env vars so `--version` can print it.
//!
//! ## Lean FFI bridge removed
//!
//! Lean FFI bridge removed. Z3-based contract discharge replaces it.
//! PMT state verification uses hand-written Rust verifiers.
//!
//! History (kept for reference): Wave 4-D wired a Lean-verified PMT FFI
//! surface into the build via the pipeline documented in
//! `FFI_BRIDGE_PLAN.md §2`:
//!
//! ```text
//! lake build  →  lean --emit-c  →  cc::Build  →  cargo:rustc-link-lib
//! ```
//!
//! When `lake` / `LEAN_HOME` were unavailable, the build fell back to
//! compiling a STUB C file (`proof/extracted/lean_stub.c`) that defined
//! the 7 Lean `@[export]` symbols with hardcoded return values. The
//! stub satisfied the linker but did NOT emit the `lean_ffi_linked`
//! cfg, so `verification.rs` was supposed to keep using the
//! hand-written Rust verifiers.
//!
//! In practice, when the `pmt-runtime-check` feature was ON, the IVE
//! pipeline routed the 3 PMT state verifiers through
//! `verify_pmt_via_lean`, which on the stub sub-path returned
//! all-empty (all-pass) Vecs — every program's PMT state verification
//! "passed" regardless of actual safety. This was the most dangerous
//! false positive in the system, so the entire Lean FFI bridge has
//! been deleted (see `src/ive/src/verification.rs` for the matching
//! removal).
//!
//! The `pmt-runtime-check` Cargo feature is RETAINED as a no-op so
//! existing CI commands (`cargo build --features pmt-runtime-check`)
//! continue to work; it no longer triggers any Lean linkage here. The
//! feature still activates the independent pure-Rust `pmt_check`
//! module in `vuma-codegen` (a real, parity-tested hand-translation
//! that does NOT depend on the stub).
//!
//! `proof/extracted/lean_stub.c` and `proof/extracted/pmt_check.rs`
//! are kept on disk for reference but are no longer compiled or
//! linked by this script.
//!
//! The `lean_ffi_linked` cfg is still declared via
//! `cargo::rustc-check-cfg` below so that `tests/*` files referencing
//! it via `#[cfg(lean_ffi_linked)]` / `#[cfg_attr(not(lean_ffi_linked),
//! ...)]` do not trigger `unexpected_cfgs` lint warnings. The cfg is
//! NEVER emitted by this script (it is always unset), so those test
//! branches consistently take the `not(lean_ffi_linked)` path.

use std::process::Command;

fn main() {
    // ── rustc version detection (always runs) ──────────────────────────
    detect_rustc_version();

    // Declare the `lean_ffi_linked` cfg so rustc's `unexpected_cfgs` lint
    // accepts it in all builds. The cfg is NEVER emitted by this script
    // (the Lean FFI bridge has been removed — see the file-level doc
    // above), but several `tests/*.rs` files still reference it via
    // `#[cfg(lean_ffi_linked)]` / `#[cfg_attr(not(lean_ffi_linked), ...)]`
    // for historical branching; declaring it here keeps those references
    // lint-clean without changing their (always `not(...)`) evaluation.
    println!("cargo::rustc-check-cfg=cfg(lean_ffi_linked)");

    println!("cargo:rerun-if-changed=build.rs");
}

fn detect_rustc_version() {
    let output = Command::new("rustc").args(["--version"]).output().ok();

    if let Some(output) = output {
        if output.status.success() {
            let version_str = String::from_utf8_lossy(&output.stdout);
            // Parse "rustc 1.xx.y (sha) date" format
            let parts: Vec<&str> = version_str.split_whitespace().collect();
            if parts.len() >= 2 {
                let ver = parts[1]; // e.g., "1.77.0"
                let ver_parts: Vec<&str> = ver.split('.').collect();
                if ver_parts.len() >= 3 {
                    println!("cargo:rustc-env=RUSTC_VERSION_MAJOR={}", ver_parts[0]);
                    println!("cargo:rustc-env=RUSTC_VERSION_MINOR={}", ver_parts[1]);
                    println!("cargo:rustc-env=RUSTC_VERSION_PATCH={}", ver_parts[2]);
                    return;
                }
            }
        }
    }

    // Fallback if parsing failed
    println!(
        "cargo:rustc-env=RUSTC_VERSION_MAJOR={}",
        option_env!("RUSTC_VERSION_MAJOR").unwrap_or("1")
    );
    println!(
        "cargo:rustc-env=RUSTC_VERSION_MINOR={}",
        option_env!("RUSTC_VERSION_MINOR").unwrap_or("?")
    );
    println!(
        "cargo:rustc-env=RUSTC_VERSION_PATCH={}",
        option_env!("RUSTC_VERSION_PATCH").unwrap_or("?")
    );
}
