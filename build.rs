//! Build script for VUMA.
//!
//! Always: detect the `rustc` version and expose it as
//! `RUSTC_VERSION_{MAJOR,MINOR,PATCH}` env vars so `--version` can print it.
//!
//! ## Wave 4-D — Lean FFI linkage (stub-fallback path)
//!
//! When the `pmt-runtime-check` feature is enabled, this script attempts to
//! wire the Lean-verified PMT checkers into the Rust binary via the pipeline
//! documented in `FFI_BRIDGE_PLAN.md §2`:
//!
//! ```text
//! lake build  →  lean --emit-c  →  cc::Build  →  cargo:rustc-link-lib
//! ```
//!
//! The full pipeline is only attempted when `lake` is on PATH **and**
//! `LEAN_HOME` is set. If either is missing, or any pipeline step fails,
//! the script prints a `cargo:warning` and falls back to compiling a STUB
//! C file (`proof/extracted/lean_stub.c`) that defines the 7 Lean export
//! symbols with hardcoded return values.
//!
//! The stub satisfies the linker so `cargo check --features pmt-runtime-check`
//! succeeds in Lean-free environments, but it does **not** emit the
//! `lean_ffi_linked` cfg, so `verification.rs` keeps using the hand-written
//! Rust verifiers (the parity-tested path). The stub is therefore SAFE and
//! HONEST: no unverified code is ever executed.
//!
//! When `pmt-runtime-check` is OFF, this script does nothing beyond rustc
//! version detection (the pre-4-D behavior).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // ── rustc version detection (always runs) ──────────────────────────
    detect_rustc_version();

    println!("cargo:rerun-if-changed=build.rs");

    // ── Lean FFI linkage (only when the feature is on) ─────────────────
    #[cfg(feature = "pmt-runtime-check")]
    {
        link_lean_ffi();
    }
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

// ─────────────────────────────────────────────────────────────────────
// Lean FFI linkage (entire module gated on the feature)
// ─────────────────────────────────────────────────────────────────────
#[cfg(feature = "pmt-runtime-check")]
fn link_lean_ffi() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let proof_dir = PathBuf::from(&manifest_dir).join("proof");
    let extracted_dir = proof_dir.join("extracted");
    let stub_c = extracted_dir.join("lean_stub.c");

    // Inputs that should retrigger the build.
    println!("cargo:rerun-if-changed={}", stub_c.display());
    println!(
        "cargo:rerun-if-changed={}",
        proof_dir.join("PMT").join("Extraction.lean").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        proof_dir.join("lakefile.toml").display()
    );

    // Step 1+2: attempt the real `lake build` → `lean --emit-c` pipeline
    // ONLY when `lake` is on PATH and `LEAN_HOME` is set. Best-effort: any
    // failure falls through to the stub.
    let lake_on_path = Command::new("lake")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let lean_home_set = env::var_os("LEAN_HOME").is_some();

    if lake_on_path && lean_home_set {
        match try_real_lean_pipeline(&proof_dir) {
            Ok(()) => {
                // Real linkage succeeded; `lean_ffi_linked` cfg + env were
                // emitted inside try_real_lean_pipeline. Done.
                return;
            }
            Err(e) => {
                println!(
                    "cargo:warning=Lean FFI linkage FAILED — falling back to stub ({})",
                    e
                );
            }
        }
    } else {
        println!(
            "cargo:warning=Lean FFI linkage skipped (lake={} LEAN_HOME={}) — using stub",
            if lake_on_path { "present" } else { "absent" },
            if lean_home_set { "set" } else { "unset" }
        );
    }

    // Step 3: ALWAYS compile the stub as the fallback/default. The stub
    // defines the 7 Lean export symbols with hardcoded return values. It
    // satisfies the linker but does NOT emit `lean_ffi_linked`, so the
    // hand-written Rust verifiers remain in effect at runtime.
    compile_stub(&stub_c);
}

/// Attempt the real `lake build` → `lean --emit-c` → `cc::Build` pipeline.
///
/// On success: compiles the emitted C into `liblean_extraction.a` and emits
/// `cargo:rustc-cfg=lean_ffi_linked` + `cargo:rustc-env=LEAN_FFI_LINKED=1`
/// so `verification.rs` routes the 3 state verifiers through the extracted
/// Lean functions.
///
/// Returns `Err(msg)` on any failure so the caller can fall back to the
/// stub. Never panics.
#[cfg(feature = "pmt-runtime-check")]
fn try_real_lean_pipeline(proof_dir: &Path) -> Result<(), String> {
    // Step 1: `lake build` in proof/
    let lake_build = Command::new("lake")
        .arg("build")
        .current_dir(proof_dir)
        .output()
        .map_err(|e| format!("spawn lake build: {e}"))?;
    if !lake_build.status.success() {
        return Err(format!(
            "lake build exited {}: {}",
            lake_build.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&lake_build.stderr)
        ));
    }

    // Step 2: `lean --emit-c` for Extraction.lean. The emitted C lands
    // under proof/.lake/build/lib/PMT/Extraction.c (FFI_BRIDGE_PLAN §2).
    let extraction_lean = proof_dir.join("PMT").join("Extraction.lean");
    let emit_c = Command::new("lean")
        .arg("--emit-c")
        .arg(&extraction_lean)
        .current_dir(proof_dir)
        .output()
        .map_err(|e| format!("spawn lean --emit-c: {e}"))?;
    if !emit_c.status.success() {
        return Err(format!(
            "lean --emit-c exited {}: {}",
            emit_c.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&emit_c.stderr)
        ));
    }

    let emitted_c = proof_dir
        .join(".lake")
        .join("build")
        .join("lib")
        .join("PMT")
        .join("Extraction.c");
    if !emitted_c.exists() {
        return Err(format!(
            "emitted C not found at {}",
            emitted_c.display()
        ));
    }

    // Step 3: cc::Build to compile the emitted C into a static archive.
    // NOTE: real extraction also requires linking `lean_runtime` objects
    // (FFI_BRIDGE_PLAN §2.3). That wiring is intentionally NOT done here
    // yet — when `lake`/`LEAN_HOME` are genuinely available this branch
    // is reached, but a fully-linked real archive needs the runtime
    // objects too. For now this returns Err on the runtime-object check
    // so the stub path is taken until Wave 5 completes the runtime link.
    let lean_runtime = proof_dir
        .join(".lake")
        .join("build")
        .join("lib")
        .join("lean_runtime");
    if !lean_runtime.exists() {
        return Err(format!(
            "lean_runtime objects not found at {} (Wave 5 runtime-link TODO)",
            lean_runtime.display()
        ));
    }

    cc::Build::new().file(&emitted_c).compile("lean_extraction");

    // Signal to the Rust code that real Lean FFI is linked.
    println!("cargo:rustc-cfg=lean_ffi_linked");
    println!("cargo:rustc-env=LEAN_FFI_LINKED=1");
    Ok(())
}

/// Compile the stub C file into the static archive `lean_extraction` and
/// emit the link directives. This is the SAFE default: it provides the 7
/// Lean export symbols so the binary links, but does NOT activate FFI
/// routing (no `lean_ffi_linked` cfg).
#[cfg(feature = "pmt-runtime-check")]
fn compile_stub(stub_c: &Path) {
    if !stub_c.exists() {
        println!(
            "cargo:warning=Lean FFI stub missing at {} — linkage will fail",
            stub_c.display()
        );
        return;
    }
    cc::Build::new().file(stub_c).compile("lean_extraction");

    // cc::Build::compile already emits `cargo:rustc-link-lib=static=...`
    // and `cargo:rustc-link-search=native=...` for OUT_DIR. Re-emit them
    // explicitly for visibility in build logs (duplicate directives are
    // harmless to the linker).
    println!("cargo:rustc-link-lib=static=lean_extraction");
    let out_dir = env::var("OUT_DIR").unwrap_or_else(|_| ".".to_string());
    println!("cargo:rustc-link-search=native={}", out_dir);
}
