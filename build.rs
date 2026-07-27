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

    // Declare the `lean_ffi_linked` cfg (set conditionally inside
    // `link_lean_ffi` when the real Lean→C pipeline succeeds) so rustc's
    // `unexpected_cfgs` lint accepts it in all builds. Added in Wave 5-C
    // because `tests/pmt_runtime_ffi_smoke.rs` references it via
    // `#[cfg(lean_ffi_linked)]` / `#[cfg_attr(not(lean_ffi_linked), ...)]`.
    println!("cargo::rustc-check-cfg=cfg(lean_ffi_linked)");

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
    // The real-vs-stub gate reads LEAN_HOME, so cargo MUST re-run this
    // script (and re-emit/clear `lean_ffi_linked`) whenever LEAN_HOME
    // changes — otherwise a stale cfg from a prior LEAN_HOME-set build
    // would persist into LEAN_HOME-unset builds (and vice versa), making
    // the stub-default guarantee unreliable.
    println!("cargo:rerun-if-env-changed=LEAN_HOME");

    // Step 1+2: attempt the real Lean → C → archive pipeline ONLY when
    // `LEAN_HOME` is explicitly set (PMT-1-G: the real archive activates
    // solely on LEAN_HOME so the stub remains the default everywhere
    // else). `lake` is NO LONGER required as a gate: try_real_lean_pipeline
    // consumes the C IR already emitted by a prior `lake build` under
    // proof/.lake/build/ir/, so the elan `lake` shim (which fails
    // `--version` without a default toolchain configured) no longer blocks
    // real linkage. Best-effort: any failure inside try_real_lean_pipeline
    // falls through to the stub.
    let lean_home_set = env::var_os("LEAN_HOME").is_some();

    if lean_home_set {
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
            "cargo:warning=Lean FFI linkage skipped (LEAN_HOME unset) — using stub"
        );
    }

    // Step 3: ALWAYS compile the stub as the fallback/default. The stub
    // defines the 7 Lean export symbols with hardcoded return values. It
    // satisfies the linker but does NOT emit `lean_ffi_linked`, so the
    // hand-written Rust verifiers remain in effect at runtime.
    compile_stub(&stub_c);
}

/// Attempt the REAL Lean → C → static-archive pipeline using the Lean
/// toolchain resolved from `LEAN_HOME` (or `lean` on PATH) and the C IR
/// already emitted by `lake build` under `proof/.lake/build/ir/`.
///
/// On success: compiles the emitted PMT C IR (plus the Lean runtime +
/// library objects, bundled into the SAME archive) and emits
/// `cargo:rustc-cfg=lean_ffi_linked` + `cargo:rustc-env=LEAN_FFI_LINKED=1`
/// so `verification.rs` routes the 3 state verifiers through the extracted
/// Lean functions.
///
/// Returns `Err(msg)` on any failure so the caller can fall back to the
/// stub (the caller wraps every `Err` in a `cargo:warning`). Never panics.
#[cfg(feature = "pmt-runtime-check")]
fn try_real_lean_pipeline(proof_dir: &Path) -> Result<(), String> {
    // ── Step (a): locate the Lean toolchain home ────────────────────
    // Prefer `LEAN_HOME` (elan toolchain dir, e.g.
    // ~/.elan/toolchains/leanprover--lean4---v4.21.0). Fall back to
    // `$(dirname $(which lean))/..` for non-elan prefix installs where
    // `lean` is at `<prefix>/bin/lean` and the runtime at
    // `<prefix>/lib/lean/`. (For elan the `which lean` heuristic resolves
    // to ~/.elan, which does NOT hold lib/lean/, so `LEAN_HOME` is the
    // reliable path there — the heuristic is best-effort.)
    let lean_home: PathBuf = match env::var_os("LEAN_HOME") {
        Some(v) => PathBuf::from(v),
        None => {
            let lean_exe = find_on_path("lean").ok_or_else(|| {
                "LEAN_HOME unset and `lean` not found on PATH".to_string()
            })?;
            lean_exe
                .parent()
                .and_then(|p| p.parent())
                .ok_or_else(|| {
                    "could not resolve lean_home from `lean` executable path".to_string()
                })?
                .to_path_buf()
        }
    };

    // ── Step (b)+(c): verify libleanrt.a and lean.h exist ───────────
    let lean_lib_dir = lean_home.join("lib").join("lean");
    let lean_inc_dir = lean_home.join("include").join("lean");
    let leanrt_a = lean_lib_dir.join("libleanrt.a");
    let lean_h = lean_inc_dir.join("lean.h");
    if !leanrt_a.is_file() {
        return Err(format!(
            "libleanrt.a not found at {} (LEAN_HOME={})",
            leanrt_a.display(),
            lean_home.display()
        ));
    }
    if !lean_h.is_file() {
        return Err(format!(
            "lean.h not found at {} (LEAN_HOME={})",
            lean_h.display(),
            lean_home.display()
        ));
    }

    // ── Verify the C IR emitted by `lake build` exists ──────────────
    // `lake build` emits each module's C under proof/.lake/build/ir/
    // <ModPath>.c (the IR directory, NOT .lake/build/lib/). We consume
    // the already-emitted IR directly rather than re-running `lean
    // --emit-c` here: re-emission is a `lake` concern, and re-running it
    // in build.rs is slow + fragile. If the IR is stale/absent we fall
    // back to the stub (Err) — re-run `lake build` in proof/ to refresh.
    let ir_dir = proof_dir.join(".lake").join("build").join("ir");
    let extraction_c = ir_dir.join("PMT").join("Extraction.c");
    if !extraction_c.is_file() {
        return Err(format!(
            "Extraction.c IR not found at {} — run `lake build` in proof/ first",
            extraction_c.display()
        ));
    }

    // ── Step (d): collect the PMT C IR files to compile ─────────────
    // Extraction.c transitively imports the whole PMT module graph
    // (Basic, Soundness, IVE/Soundness/*, Iris/*, …). Compile every PMT
    // module .c under ir/PMT/ so the archive defines the full set of
    // extracted Lean symbols the link may pull in. check_pmt.c (which
    // defines `main`) lives at the ir/ ROOT and is excluded automatically
    // by only descending into ir/PMT/. The root PMT.c (defines
    // initialize_PMT, no main) is added too in case the FFI later wants
    // to run module initializers.
    let mut c_files: Vec<PathBuf> = Vec::new();
    collect_c_files_recursive(&ir_dir.join("PMT"), &mut c_files)
        .map_err(|e| format!("collecting PMT .c IR: {e}"))?;
    if c_files.is_empty() {
        return Err("no PMT/*.c IR files found to compile".to_string());
    }
    let root_pmt_c = ir_dir.join("PMT.c");
    if root_pmt_c.is_file() {
        c_files.push(root_pmt_c);
    }
    for cf in &c_files {
        println!("cargo:rerun-if-changed={}", cf.display());
    }

    println!(
        "cargo:warning=Lean FFI real pipeline: compiling {} PMT .c file(s) with Lean include {}",
        c_files.len(),
        lean_inc_dir.display()
    );

    // cc::Build compiles the .c files into liblean_extraction.a in OUT_DIR.
    cc::Build::new()
        .files(&c_files)
        .include(lean_home.join("include"))
        .include(&lean_inc_dir)
        .warnings(false)
        // Lean-emitted C is not warning-clean; silence the noisy categories
        // the Lean header pragma already tries to suppress.
        .flag("-Wno-unused-parameter")
        .flag("-Wno-unused-but-set-variable")
        .flag("-Wno-unused-label")
        .flag("-Wno-unused-function")
        .flag("-Wno-unused-variable")
        .compile("lean_extraction");

    // ── Bundle the Lean runtime + library objects into the same archive ─
    // The integration test links ONLY `lean_extraction` (via a manual
    // `#[link(name = "lean_extraction", kind = "static")]` in
    // tests/pmt_runtime_ffi_smoke.rs): build-script `cargo:rustc-link-lib`
    // directives do NOT reliably propagate to integration-test link lines
    // (rlibs do not forward native link-libs to dependents — see the test's
    // `#[link]` comment). To make that single archive SELF-SUFFICIENT, we
    // extract the .o members of the Lean static libs and append them to
    // liblean_extraction.a so every `lean_*` / `l_*` runtime symbol the PMT
    // objects reference is satisfied from within the archive itself.
    //
    // libLeanc.a is intentionally SKIPPED: its single member (Leanc.o)
    // defines `main` / `_lean_main`, which would clash with the test
    // harness's own `main`. libInit/libStd are pure-C Lean library code;
    // libleanrt/libleancpp are the C++ runtime (verified disjoint: 0
    // overlapping defined symbols between leanrt and leancpp).
    let out_dir = env::var("OUT_DIR").map_err(|_| "OUT_DIR unset".to_string())?;
    let archive = PathBuf::from(&out_dir).join("liblean_extraction.a");
    // Merge the Lean static libs INTO liblean_extraction.a via an `ar -M`
    // MRI script (OPEN + ADDLIB + SAVE). ADDLIB copies every member
    // directly archive-to-archive, which — unlike `ar x` into a flat dir
    // then `ar rs` — does NOT lose objects that share a basename: libInit.a
    // alone has 119 duplicate member names (Basic.o, Array.o, … from
    // distinct modules), and a flat `ar x` lets later same-named members
    // overwrite earlier ones on disk, dropping their symbols (empirically
    // this left l_List_isEmpty___rarg / l_String_intercalate undefined at
    // link time). MRI ADDLIB preserves all members.
    //
    // libLeanc.a is intentionally skipped: its single member (Leanc.o)
    // defines `main` / `_lean_main`, which would clash with the test
    // harness's own `main`. libInit/libStd are pure-C Lean library code;
    // libleanrt/libleancpp are the C++ runtime (verified disjoint: 0
    // overlapping defined symbols between leanrt and leancpp).
    use std::io::Write as _;
    let lean_toolchain_lib_dir = lean_home.join("lib");
    let mut mri = String::new();
    mri.push_str(&format!("OPEN {}\n", archive.display()));
    // Lean library + runtime code (lib/lean): pure-C Lean module code
    // (Init/Std) + the C++ runtime (leanrt/leancpp).
    for lib_name in &["libInit.a", "libStd.a", "libleanrt.a", "libleancpp.a"] {
        let lib_path = lean_lib_dir.join(lib_name);
        if lib_path.is_file() {
            mri.push_str(&format!("ADDLIB {}\n", lib_path.display()));
        }
    }
    // C++ stdlib + GMP + unwind + libuv that libleanrt/libleancpp were
    // built against (lib/, the toolchain root — NOT lib/lean). Crucially
    // libleancpp was compiled with clang/libc++, so it references libc++ /
    // libc++abi symbols (std::__1::*, __cxa_*), NOT libstdc++; linking
    // -lstdc++ would NOT resolve them. Bundling the static archives makes
    // liblean_extraction.a self-sufficient for these deps too.
    for lib_name in &["libc++.a", "libc++abi.a", "libgmp.a", "libunwind.a", "libuv.a"] {
        let lib_path = lean_toolchain_lib_dir.join(lib_name);
        if lib_path.is_file() {
            mri.push_str(&format!("ADDLIB {}\n", lib_path.display()));
        }
    }
    mri.push_str("SAVE\nEND\n");
    let mut mri_child = Command::new("ar")
        .arg("-M")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn ar -M: {e}"))?;
    {
        let mut stdin = mri_child.stdin.take().ok_or_else(|| {
            "ar -M stdin pipe unavailable".to_string()
        })?;
        stdin
            .write_all(mri.as_bytes())
            .map_err(|e| format!("write ar -M stdin: {e}"))?;
        // Dropping `stdin` here signals EOF to `ar -M`.
    }
    let mri_out = mri_child
        .wait_with_output()
        .map_err(|e| format!("wait ar -M: {e}"))?;
    if !mri_out.status.success() {
        return Err(format!(
            "ar -M (merge Lean libs) exited {}: {}",
            mri_out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&mri_out.stderr)
        ));
    }

    // ── Step (e): link directives ───────────────────────────────────
    // Primary (per PMT-1-G spec): search <lean_home>/lib/lean and link
    // libleanrt.a. The bundle above already inlines leanrt's objects into
    // lean_extraction, so these are belt-and-suspenders — a backstop in
    // case any runtime symbol was missed by the bundle, and harmless when
    // the archive is already self-sufficient (the linker simply finds no
    // remaining undefined leanrt symbols to satisfy).
    println!("cargo:rustc-link-search=native={}", lean_lib_dir.display());
    println!("cargo:rustc-link-lib=static=leanrt");
    // No system-dylib directives are needed here: the C++ runtime
    // (libc++/libc++abi — libleancpp was built with clang/libc++, NOT
    // libstdc++), GMP, libunwind and libuv are all bundled INTO
    // liblean_extraction.a above, so the archive is self-sufficient.
    // POSIX libc/libpthread/libdl/libm are already on Rust's default link
    // line. (cargo:rustc-link-lib directives do not propagate to
    // integration-test link lines anyway — see tests/pmt_runtime_ffi_smoke.rs
    // #[link] comment — which is exactly why the bundle is required: the
    // test links ONLY lean_extraction, so it must carry every Lean-runtime
    // dependency internally.)

    // ── Step (f): signal real Lean FFI linkage to the Rust code ─────
    println!("cargo:rustc-cfg=lean_ffi_linked");
    println!("cargo:rustc-env=LEAN_FFI_LINKED=1");
    println!(
        "cargo:warning=Lean FFI real pipeline SUCCEEDED: {} PMT .c files + bundled Lean runtime -> liblean_extraction.a (lean_ffi_linked emitted)",
        c_files.len()
    );
    Ok(())
}

/// Search `$PATH` for an executable named `name`, returning its path.
/// Used by `try_real_lean_pipeline` to locate `lean` when `LEAN_HOME` is
/// unset (non-elan prefix installs).
#[cfg(feature = "pmt-runtime-check")]
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Recursively collect every `*.c` under `dir` into `out`. Used by
/// `try_real_lean_pipeline` to gather the PMT module C IR graph.
#[cfg(feature = "pmt-runtime-check")]
fn collect_c_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_c_files_recursive(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("c") {
            out.push(path);
        }
    }
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
