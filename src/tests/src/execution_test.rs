//! End-to-end execution tests for the VUMA pipeline.
//!
//! This test compiles every example under `examples/` for every one of the
//! 8 codegen backends and ATTEMPTS to execute the produced binary:
//!
//!  - x86_64      -> native execution (host is x86_64)
//!  - AArch64     -> via `qemu-aarch64` if available
//!  - RiscV64     -> via `qemu-riscv64` if available
//!  - LoongArch64 -> via `qemu-loongarch64` if available
//!  - Arm32       -> via `qemu-arm` if available
//!  - Mips64      -> via `qemu-mips64` if available
//!  - PowerPC64   -> via `qemu-ppc64` if available
//!  - Wasm32      -> via `node` (with experimental WASI) if available
//!
//! For backends without an available execution runner (no QEMU, no node)
//! the test still compiles the example, verifies that the binary is
//! non-empty, and emits a clear `SKIP` message. This way we get coverage
//! of every example x backend COMPILE path even where we can't execute.
//!
//! Note: This test always succeeds (no hard assertions). Its purpose is
//! to surface pass/fail/skip/crash counts for human inspection via the
//! captured stdout. A SIGSEGV (signal 11) is counted as a `crash`.

use vuma_codegen::backend::{create_backend, AllocatedProgram, BackendKind};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{AstToScg, Parser};
use vuma::pipeline::{
    bridge_scg_to_codegen, run_scg_transforms, CompileConfig, CompileTarget, OptLevel,
    VerificationLevel,
};
use std::process::Command;

// ---------------------------------------------------------------------------
// Compilation helper
// ---------------------------------------------------------------------------

fn compile_for_backend(source: &str, kind: BackendKind) -> Result<Vec<u8>, String> {
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    if result.has_errors() {
        return Err(format!("parse: {} errors", result.errors.len()));
    }
    let ast = result.unwrap();
    let mut scg = {
        let mut c = AstToScg::new();
        c.convert(&ast).map_err(|e| format!("scg: {}", e))?
    };
    let config = CompileConfig {
        target: if kind == BackendKind::Wasm32 {
            CompileTarget::Wasm32
        } else {
            CompileTarget::Linux
        },
        opt_level: OptLevel::O0,
        verification_level: VerificationLevel::None,
        ..Default::default()
    };
    let _ = run_scg_transforms(&mut scg, &config);
    let codegen_scg = bridge_scg_to_codegen(&scg);
    let ir_program = {
        let mut b = IRBuilder::new();
        b.build(&codegen_scg).map_err(|e| format!("ir: {}", e))?
    };
    let backend = create_backend(kind).map_err(|e| format!("backend: {}", e))?;
    let mut allocated = Vec::new();
    for func in &ir_program.functions {
        allocated.push(
            backend
                .allocate_registers(func)
                .map_err(|e| format!("regalloc: {}", e))?,
        );
    }
    let total_code: usize = allocated.iter().map(|f| f.code_size).sum();
    let program = AllocatedProgram {
        functions: allocated,
        total_code_size: total_code,
        total_data_size: 0,
    };
    backend
        .encode_program(&program)
        .map_err(|e| format!("encode: {}", e))
}

// ---------------------------------------------------------------------------
// QEMU / WASM runner discovery
// ---------------------------------------------------------------------------

/// Search PATH, /tmp/qemu_all/usr/bin, and /tmp/qemu_all2/usr/bin for a
/// working `qemu-<arch>` binary. Returns the path if `qemu-<arch>
/// --version` exits successfully.
fn find_qemu(arch: &str) -> Option<String> {
    // 1. PATH
    let qemu_name = format!("qemu-{}", arch);
    if let Ok(o) = Command::new(&qemu_name).arg("--version").output() {
        if o.status.success() {
            return Some(qemu_name);
        }
    }
    // 2. /tmp/qemu_all and /tmp/qemu_all2
    for dir in &["/tmp/qemu_all/usr/bin", "/tmp/qemu_all2/usr/bin"] {
        let path = format!("{}/qemu-{}", dir, arch);
        if std::path::Path::new(&path).exists() {
            if let Ok(o) = Command::new(&path).arg("--version").output() {
                if o.status.success() {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// Locate a runtime capable of executing a WASI `.wasm` module.
/// Prefers `wasmtime`, then `node` (with built-in experimental WASI).
fn find_wasm_runner() -> Option<String> {
    if let Ok(o) = Command::new("wasmtime").arg("--version").output() {
        if o.status.success() {
            return Some("wasmtime".to_string());
        }
    }
    if let Ok(o) = Command::new("node").arg("--version").output() {
        if o.status.success() {
            return Some("node".to_string());
        }
    }
    None
}

/// Maps a VUMA `BackendKind` to the `qemu-<arch>` binary name it needs
/// for execution (returns `None` for x86_64 native and Wasm32 which
/// doesn't use QEMU).
fn backend_qemu_name(kind: BackendKind) -> Option<&'static str> {
    match kind {
        BackendKind::X86_64 => None,            // native execution
        BackendKind::AArch64 => Some("aarch64"),
        BackendKind::RiscV64 => Some("riscv64"),
        BackendKind::LoongArch64 => Some("loongarch64"),
        BackendKind::Arm32 => Some("arm"),
        BackendKind::Mips64 => Some("mips64"),
        BackendKind::PowerPC64 => Some("ppc64"),
        BackendKind::Wasm32 => None,            // uses node/wasmtime
    }
}

// ---------------------------------------------------------------------------
// Execution helpers
// ---------------------------------------------------------------------------

/// Write `binary` to a temp file, chmod 0755, execute via QEMU (or
/// directly if `qemu` is None), and return `(exit_code, stdout,
/// stderr)`. Wrapped in `timeout 3` to prevent runaway processes.
fn execute_binary(binary: &[u8], qemu: Option<&str>) -> Result<(i32, Vec<u8>, Vec<u8>), String> {
    let bin_path = std::env::temp_dir().join(format!("vuma_exec_{}.bin", std::process::id()));
    std::fs::write(&bin_path, binary).map_err(|e| format!("write: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin_path, perms).unwrap();
    }

    let mut cmd = match qemu {
        Some(q) => {
            let mut c = Command::new("timeout");
            c.arg("3").arg(q).arg(&bin_path);
            c
        }
        None => {
            let mut c = Command::new("timeout");
            c.arg("3").arg(&bin_path);
            c
        }
    };
    let output = cmd.output().map_err(|e| format!("exec: {}", e))?;

    let _ = std::fs::remove_file(&bin_path);
    let exit_code = output.status.code().unwrap_or(-1);
    Ok((exit_code, output.stdout, output.stderr))
}

/// Execute a WASI `.wasm` module via `node` (using the experimental
/// `node:wasi` module) or `wasmtime`. Returns `(exit_code, stdout,
/// stderr)`.
fn execute_wasm(binary: &[u8], runner: &str) -> Result<(i32, Vec<u8>, Vec<u8>), String> {
    let wasm_path = std::env::temp_dir().join(format!("vuma_exec_{}.wasm", std::process::id()));
    std::fs::write(&wasm_path, binary).map_err(|e| format!("write: {}", e))?;

    let output = if runner == "wasmtime" {
        Command::new("timeout")
            .arg("3")
            .arg("wasmtime")
            .arg(&wasm_path)
            .output()
            .map_err(|e| format!("exec: {}", e))?
    } else {
        // node: use the built-in experimental WASI implementation.
        let script = format!(
            r#"
            const {{ WASI }} = require('node:wasi');
            const fs = require('node:fs');
            try {{
              const wasm = fs.readFileSync({:?});
              const wasi = new WASI({{ version: 'preview1', args: [], env: {{}}, preopens: {{}} }});
              WebAssembly.instantiate(wasm, wasi.getImportObject()).then(({{ instance }}) => {{
                try {{ wasi.start(instance); process.exit(0); }}
                catch (e) {{
                  if (e && (e.message || '').includes('proc_exit')) {{ process.exit(((e.code || 0) | 0)); }}
                  else {{ console.error('start:', (e && e.message) || String(e)); process.exit(1); }}
                }}
              }}).catch((e) => {{
                console.error('instantiate:', (e && e.message) || String(e));
                process.exit(1);
              }});
            }} catch (e) {{ console.error('outer:', (e && e.message) || String(e)); process.exit(1); }}
            "#,
            wasm_path.to_string_lossy()
        );
        Command::new("timeout")
            .arg("3")
            .arg("node")
            .arg("--no-warnings")
            .arg("-e")
            .arg(&script)
            .output()
            .map_err(|e| format!("exec: {}", e))?
    };

    let _ = std::fs::remove_file(&wasm_path);
    let exit_code = output.status.code().unwrap_or(-1);
    Ok((exit_code, output.stdout, output.stderr))
}

// ---------------------------------------------------------------------------
// Test: availability report (instant)
// ---------------------------------------------------------------------------

/// Quick report of which execution backends are available in the
/// current environment. Always passes; the value is in the printed
/// output, which makes it clear which backends will be SKIPped vs
/// EXECuted by the main test below.
#[test]
fn test_qemu_availability_report() {
    eprintln!("\n=== QEMU / WASM runner availability per backend ===");
    let all_backends = [
        BackendKind::X86_64,
        BackendKind::AArch64,
        BackendKind::RiscV64,
        BackendKind::LoongArch64,
        BackendKind::Arm32,
        BackendKind::Mips64,
        BackendKind::PowerPC64,
        BackendKind::Wasm32,
    ];
    let mut executable = 0;
    let mut compile_only = 0;
    for k in &all_backends {
        let qemu_name = backend_qemu_name(*k);
        let status: String = match qemu_name {
            None => {
                if *k == BackendKind::X86_64 {
                    executable += 1;
                    "native (host=x86_64)".to_string()
                } else if *k == BackendKind::Wasm32 {
                    match find_wasm_runner() {
                        Some(r) => {
                            executable += 1;
                            format!("wasm runner: {}", r)
                        }
                        None => {
                            compile_only += 1;
                            "NO WASM RUNNER (compile-only)".to_string()
                        }
                    }
                } else {
                    compile_only += 1;
                    "no runner (compile-only)".to_string()
                }
            }
            Some(name) => match find_qemu(name) {
                Some(p) => {
                    executable += 1;
                    format!("qemu-{} at {}", name, p)
                }
                None => {
                    compile_only += 1;
                    format!("qemu-{} NOT AVAILABLE (compile-only)", name)
                }
            },
        };
        eprintln!("  {:14} -> {}", k.isa_name(), status);
    }
    eprintln!(
        "=== Summary: {} executable backends, {} compile-only backends ===",
        executable, compile_only
    );
}

// ---------------------------------------------------------------------------
// Test: compile + attempt-execute matrix over ALL examples x ALL 8 backends
// ---------------------------------------------------------------------------

/// Compile every `.vuma` example under `examples/` for every one of the
/// 8 codegen backends and ATTEMPT execution. For backends without an
/// available execution runner, the test verifies that the example
/// COMPILES to a non-empty binary and records a `SKIP`. Always passes;
/// the result of interest is the printed matrix.
#[test]
fn test_execute_all_examples_all_executable_backends() {
    let examples_dir = format!("{}/../../examples", env!("CARGO_MANIFEST_DIR"));
    let mut examples: Vec<String> = std::fs::read_dir(&examples_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".vuma"))
        .collect();
    examples.sort();

    // Build the list of (backend_name, BackendKind, runner). For QEMU
    // backends, `runner` is `Some(qemu_path)`. For x86_64, `runner` is
    // `None` (native execution). For Wasm32, the test calls
    // `execute_wasm` separately (not via QEMU). For backends without an
    // available runner, the entry is omitted from `exec_backends` but
    // still appears in `compile_only_backends` so that we compile the
    // example to verify the codegen path works.
    let mut exec_backends: Vec<(&'static str, BackendKind, Option<String>)> = Vec::new();
    let mut compile_only_backends: Vec<(&'static str, BackendKind)> = Vec::new();

    // x86_64: always native
    exec_backends.push(("x86_64", BackendKind::X86_64, None));

    // QEMU-backed architectures
    for (name, kind, qemu_arch) in [
        ("aarch64", BackendKind::AArch64, "aarch64"),
        ("riscv64", BackendKind::RiscV64, "riscv64"),
        ("loongarch64", BackendKind::LoongArch64, "loongarch64"),
        ("arm32", BackendKind::Arm32, "arm"),
        ("mips64", BackendKind::Mips64, "mips64"),
        ("ppc64", BackendKind::PowerPC64, "ppc64"),
    ] {
        match find_qemu(qemu_arch) {
            Some(q) => exec_backends.push((name, kind, Some(q))),
            None => compile_only_backends.push((name, kind)),
        }
    }

    // Wasm32: separate handling (uses node/wasmtime, not QEMU)
    let wasm_runner = find_wasm_runner();
    if wasm_runner.is_none() {
        compile_only_backends.push(("wasm32", BackendKind::Wasm32));
    }

    eprintln!(
        "\n=== Execution Test: {} examples x {} backends ===",
        examples.len(),
        exec_backends.len() + compile_only_backends.len() + if wasm_runner.is_some() { 1 } else { 0 }
    );
    eprintln!(
        "Executable backends ({}): {:?}",
        exec_backends.len(),
        exec_backends
            .iter()
            .map(|(n, _, q)| (n, q.is_some()))
            .collect::<Vec<_>>()
    );
    eprintln!(
        "Compile-only backends ({}): {:?}",
        compile_only_backends.len(),
        compile_only_backends
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>()
    );
    if let Some(ref r) = wasm_runner {
        eprintln!("Wasm32 runner: {}", r);
    }

    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut crash = 0u32;
    let mut skip = 0u32;

    for ex in &examples {
        let path = format!("{}/{}", examples_dir, ex);
        let source = std::fs::read_to_string(&path).unwrap();

        // (1) Executable QEMU backends + native x86_64
        for (name, kind, qemu) in &exec_backends {
            match compile_for_backend(&source, *kind) {
                Err(e) => {
                    eprintln!("  X COMPILE {} {}: {}", ex, name, e);
                    fail += 1;
                }
                Ok(binary) => match execute_binary(&binary, qemu.as_deref()) {
                    Ok((code, stdout, stderr)) => {
                        let stderr_str = String::from_utf8_lossy(&stderr);
                        if stderr_str.contains("Segmentation fault") || code == -11 {
                            eprintln!("  ! CRASH  {} {}: signal 11 (SIGSEGV)", ex, name);
                            crash += 1;
                        } else if stderr_str.contains("uncaught target signal") {
                            eprintln!(
                                "  ! CRASH  {} {}: {}",
                                ex,
                                name,
                                stderr_str.lines().next().unwrap_or("")
                            );
                            crash += 1;
                        } else {
                            eprintln!(
                                "  OK EXEC  {} {}: exit={} stdout={}B",
                                ex,
                                name,
                                code,
                                stdout.len()
                            );
                            pass += 1;
                        }
                    }
                    Err(e) => {
                        eprintln!("  X EXEC   {} {}: {}", ex, name, e);
                        fail += 1;
                    }
                },
            }
        }

        // (2) Compile-only backends (no QEMU available): verify the
        // codegen path produces a non-empty binary, then SKIP.
        for (name, kind) in &compile_only_backends {
            match compile_for_backend(&source, *kind) {
                Err(e) => {
                    eprintln!("  X COMPILE {} {} (compile-only): {}", ex, name, e);
                    fail += 1;
                }
                Ok(binary) => {
                    if binary.is_empty() {
                        eprintln!("  X EMPTY  {} {} (compile-only produced 0 bytes)", ex, name);
                        fail += 1;
                    } else {
                        eprintln!(
                            "  ~ SKIP   {} {} (no QEMU; compiled {}B)",
                            ex,
                            name,
                            binary.len()
                        );
                        skip += 1;
                    }
                }
            }
        }

        // (3) Wasm32 execution via node/wasmtime (if available)
        if let Some(ref runner) = wasm_runner {
            match compile_for_backend(&source, BackendKind::Wasm32) {
                Err(e) => {
                    eprintln!("  X COMPILE {} wasm32: {}", ex, e);
                    fail += 1;
                }
                Ok(binary) => match execute_wasm(&binary, runner) {
                    Ok((code, stdout, stderr)) => {
                        let stderr_str = String::from_utf8_lossy(&stderr);
                        let stdout_str = String::from_utf8_lossy(&stdout);
                        // Wasm "crash" indicators: RuntimeError, unreachable,
                        // "wasm" + "Error" in stderr, or our own start:/
                        // instantiate:/ outer: error markers.
                        let crashed = stderr_str.contains("RuntimeError")
                            || stderr_str.contains("unreachable")
                            || (stderr_str.contains("wasm") && stderr_str.contains("Error"))
                            || stderr_str.contains("start:")
                            || stderr_str.contains("instantiate:")
                            || stderr_str.contains("outer:");
                        if crashed {
                            eprintln!(
                                "  ! CRASH  {} wasm32: {}",
                                ex,
                                stderr_str.lines().next().unwrap_or("")
                            );
                            crash += 1;
                        } else {
                            eprintln!(
                                "  OK EXEC  {} wasm32: exit={} stdout={}B",
                                ex,
                                code,
                                stdout_str.len()
                            );
                            pass += 1;
                        }
                    }
                    Err(e) => {
                        eprintln!("  X EXEC   {} wasm32: {}", ex, e);
                        fail += 1;
                    }
                },
            }
        }
    }

    eprintln!(
        "\n=== Results: {} pass, {} fail, {} crash, {} skip ===",
        pass, fail, crash, skip
    );
}
