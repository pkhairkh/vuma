//! Differential tester: compile each .vuma example on all 7 native backends,
//! run the resulting binary, and compare exit codes AND stdout across backends.
//!
//! Backends: x86_64 (native), AArch64, RISC-V 64, ARM32, MIPS64LE, PPC64,
//! LoongArch64 (all under QEMU). WASM32 is skipped (cannot run natively).
//!
//! Usage: ./differential_test [examples_dir]
//!
//! Default examples_dir = /tmp/my-project/examples

use vuma_codegen::backend::{create_backend, AllocatedProgram, BackendKind};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{AstToScg, Parser};
use vuma::pipeline::{
    bridge_scg_to_codegen, run_scg_transforms, CompileConfig, CompileTarget, OptLevel,
    VerificationLevel,
};
use std::fs;
use std::process::Command;

/// Description of one backend we will test.
struct BackendSpec {
    name: &'static str,
    kind: BackendKind,
    /// Absolute path to QEMU binary, or None to run natively (x86_64).
    qemu: Option<&'static str>,
}

const QEMU_ARM: &str = "/tmp/qemu_bins/qemu-arm";
const QEMU_AARCH64: &str = "/tmp/qemu_bins/qemu-aarch64";
const QEMU_RISCV64: &str = "/tmp/qemu_bins/qemu-riscv64";
const QEMU_PPC64: &str = "/tmp/qemu_bins/qemu-ppc64";
const QEMU_LOONGARCH64: &str = "/tmp/qemu_bins/qemu-loongarch64";
const QEMU_MIPS64EL: &str = "/tmp/qemu_bins/qemu-mips64el";

fn backends() -> Vec<BackendSpec> {
    vec![
        BackendSpec { name: "x86_64",       kind: BackendKind::X86_64,       qemu: None },
        BackendSpec { name: "aarch64",      kind: BackendKind::AArch64,      qemu: Some(QEMU_AARCH64) },
        BackendSpec { name: "riscv64",      kind: BackendKind::RiscV64,      qemu: Some(QEMU_RISCV64) },
        BackendSpec { name: "arm32",        kind: BackendKind::Arm32,        qemu: Some(QEMU_ARM) },
        BackendSpec { name: "mips64el",     kind: BackendKind::Mips64,       qemu: Some(QEMU_MIPS64EL) },
        BackendSpec { name: "ppc64",        kind: BackendKind::PowerPC64,    qemu: Some(QEMU_PPC64) },
        BackendSpec { name: "loongarch64",  kind: BackendKind::LoongArch64,  qemu: Some(QEMU_LOONGARCH64) },
    ]
}

/// One per-(program, backend) compile outcome.
enum CompileOutcome {
    Ok(Vec<u8>),
    Fail(String),
}

/// One per-(program, backend) execution outcome.
#[derive(Clone)]
enum ExecOutcome {
    /// Process exited normally with a code and stdout.
    Done { code: i32, stdout: Vec<u8> },
    /// Process was killed (signal / segfault / uncaught target signal).
    Crash { code: i32, signal: Option<i32>, detail: String },
    /// `timeout` killed it (exit code 124).
    Timeout,
    /// Could not spawn (e.g. QEMU missing).
    SpawnFailed(String),
}

fn compile_for_backend(source: &str, kind: BackendKind) -> CompileOutcome {
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    if result.has_errors() {
        return CompileOutcome::Fail(format!("parse: {} errors", result.errors.len()));
    }
    let ast = match result.into_result() {
        Ok(a) => a,
        Err(_) => return CompileOutcome::Fail("parse: unresolved errors".into()),
    };
    let mut scg = {
        let mut c = AstToScg::new();
        match c.convert(&ast) {
            Ok(s) => s,
            Err(e) => return CompileOutcome::Fail(format!("scg: {}", e)),
        }
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
        match b.build(&codegen_scg) {
            Ok(p) => p,
            Err(e) => return CompileOutcome::Fail(format!("ir: {}", e)),
        }
    };
    let backend = match create_backend(kind) {
        Ok(b) => b,
        Err(e) => return CompileOutcome::Fail(format!("backend: {}", e)),
    };
    let mut allocated = Vec::new();
    for func in &ir_program.functions {
        match backend.allocate_registers(func) {
            Ok(a) => allocated.push(a),
            Err(e) => return CompileOutcome::Fail(format!("regalloc: {}", e)),
        }
    }
    let total_code: usize = allocated.iter().map(|f| f.code_size).sum();
    let program = AllocatedProgram {
        functions: allocated,
        total_code_size: total_code,
        total_data_size: 0,
    };
    match backend.encode_program(&program) {
        Ok(bytes) => CompileOutcome::Ok(bytes),
        Err(e) => CompileOutcome::Fail(format!("encode: {}", e)),
    }
}

fn execute_binary(binary: &[u8], qemu: Option<&str>, timeout_secs: u64, tag: &str) -> ExecOutcome {
    let bin_path = std::env::temp_dir().join(format!("vuma_difftest_{}_{}.bin", tag, std::process::id()));
    if let Err(e) = fs::write(&bin_path, binary) {
        return ExecOutcome::SpawnFailed(format!("write: {}", e));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(&bin_path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = fs::set_permissions(&bin_path, perms);
        }
    }
    let mut cmd = Command::new("timeout");
    cmd.arg(format!("{}", timeout_secs));
    if let Some(q) = qemu {
        if !q.is_empty() {
            cmd.arg(q);
        }
    }
    cmd.arg(&bin_path);
    let output = cmd.output();
    let _ = fs::remove_file(&bin_path);
    match output {
        Ok(o) => {
            let code = o.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                let signal = o.status.signal();
                let crashed = stderr.contains("Segmentation fault")
                    || stderr.contains("uncaught target signal")
                    || code == 139
                    || code == 134
                    || signal.is_some();
                if code == 124 && signal.is_none() && !stderr.contains("uncaught target signal") {
                    // `timeout` returns 124 when it had to kill the process.
                    ExecOutcome::Timeout
                } else if crashed {
                    let detail: String = stderr.chars().take(200).collect();
                    ExecOutcome::Crash { code, signal, detail }
                } else {
                    ExecOutcome::Done { code, stdout: o.stdout }
                }
            }
            #[cfg(not(unix))]
            {
                let crashed = stderr.contains("Segmentation fault")
                    || stderr.contains("uncaught target signal")
                    || code == 139
                    || code == 134;
                if code == 124 && !crashed {
                    ExecOutcome::Timeout
                } else if crashed {
                    let detail: String = stderr.chars().take(200).collect();
                    ExecOutcome::Crash { code, signal: None, detail }
                } else {
                    ExecOutcome::Done { code, stdout: o.stdout }
                }
            }
        }
        Err(e) => ExecOutcome::SpawnFailed(format!("spawn: {}", e)),
    }
}

/// Per-backend result for a single example.
struct BackendResult {
    name: String,
    /// Stores Ok(()) on success or the error message on failure.
    compile: Result<(), String>,
    exec: Option<ExecOutcome>,
}

fn fmt_stdout(s: &[u8]) -> String {
    let mut out = String::new();
    for &b in s {
        if b == b'\n' {
            out.push_str("\\n");
        } else if b == b'\t' {
            out.push_str("\\t");
        } else if b == b'\r' {
            out.push_str("\\r");
        } else if (32..127).contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("\\x{:02x}", b));
        }
    }
    if out.len() > 80 {
        format!("{}...(truncated, {} bytes total)", &out[..80], out.len())
    } else {
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ProgramCategory {
    Pass,
    DiffFailure,
    Crash,
    Timeout,
    CompileFailAll,
    CompileFailSome,
}

fn categorise(results: &[BackendResult]) -> ProgramCategory {
    let compiled: Vec<&BackendResult> = results.iter().filter(|r| r.compile.is_ok()).collect();
    if compiled.is_empty() {
        return ProgramCategory::CompileFailAll;
    }
    if compiled.len() < results.len() {
        return ProgramCategory::CompileFailSome;
    }
    let mut any_timeout = false;
    let mut any_crash = false;
    for r in results.iter() {
        if let Some(e) = &r.exec {
            match e {
                ExecOutcome::Timeout => any_timeout = true,
                ExecOutcome::Crash { .. } => any_crash = true,
                _ => {}
            }
        }
    }
    if any_timeout {
        return ProgramCategory::Timeout;
    }
    if any_crash {
        return ProgramCategory::Crash;
    }
    // All ran cleanly. Compare exit codes and stdout.
    let mut exit_codes: Vec<i32> = Vec::new();
    let mut stdouts: Vec<Vec<u8>> = Vec::new();
    for r in results.iter() {
        if let Some(ExecOutcome::Done { code, stdout }) = &r.exec {
            exit_codes.push(*code);
            stdouts.push(stdout.clone());
        }
    }
    let all_same_code = exit_codes.iter().all(|c| *c == exit_codes[0]);
    let all_same_stdout = stdouts.iter().all(|s| s == &stdouts[0]);
    if all_same_code && all_same_stdout {
        ProgramCategory::Pass
    } else {
        ProgramCategory::DiffFailure
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let examples_dir = if args.len() > 1 { args[1].as_str() } else { "/tmp/my-project/examples" };

    // Discover examples.
    let mut examples: Vec<String> = match fs::read_dir(examples_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".vuma"))
            .collect(),
        Err(e) => {
            eprintln!("ERROR: cannot read examples dir '{}': {}", examples_dir, e);
            std::process::exit(2);
        }
    };
    examples.sort();
    let n_total = examples.len();

    let backends = backends();
    eprintln!("Differential tester: {} examples x {} backends ({} total runs)",
              n_total, backends.len(), n_total * backends.len());

    let mut counts = std::collections::HashMap::<ProgramCategory, usize>::new();
    // Detailed per-program results for reporting.
    struct ProgReport {
        name: String,
        category: ProgramCategory,
        results: Vec<BackendResult>,
    }
    let mut reports: Vec<ProgReport> = Vec::new();

    for (idx, ex) in examples.iter().enumerate() {
        let path = format!("{}/{}", examples_dir, ex);
        let source = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[{}/{}] {} : READ FAIL: {}", idx + 1, n_total, ex, e);
                let mut results = Vec::new();
                for b in &backends {
                    results.push(BackendResult {
                        name: b.name.to_string(),
                        compile: Err(format!("read: {}", e)),
                        exec: None,
                    });
                }
                let cat = ProgramCategory::CompileFailAll;
                *counts.entry(cat.clone()).or_insert(0) += 1;
                reports.push(ProgReport { name: ex.clone(), category: cat, results });
                continue;
            }
        };

        let mut results: Vec<BackendResult> = Vec::new();
        for b in &backends {
            let compiled = compile_for_backend(&source, b.kind);
            let exec = match &compiled {
                CompileOutcome::Ok(bytes) => Some(execute_binary(bytes, b.qemu, 3, b.name)),
                CompileOutcome::Fail(_) => None,
            };
            let compile_flag = match &compiled {
                CompileOutcome::Ok(_) => Ok(()),
                CompileOutcome::Fail(e) => Err(e.clone()),
            };
            results.push(BackendResult {
                name: b.name.to_string(),
                compile: compile_flag,
                exec,
            });
        }

        let cat = categorise(&results);
        *counts.entry(cat.clone()).or_insert(0) += 1;
        eprintln!("[{}/{}] {} : {:?}", idx + 1, n_total, ex, cat);

        // For failures / crashes / timeouts, print the per-backend details now.
        match cat {
            ProgramCategory::Pass => {}
            _ => {
                for r in &results {
                    match (&r.compile, &r.exec) {
                        (Err(e), _) => {
                            eprintln!("    {} : COMPILE FAIL: {}", r.name, e);
                        }
                        (Ok(()), Some(ExecOutcome::Timeout)) => {
                            eprintln!("    {} : TIMEOUT", r.name);
                        }
                        (Ok(()), Some(ExecOutcome::Crash { code, signal, detail })) => {
                            eprintln!("    {} : CRASH (code={}, signal={:?}) {}", r.name, code, signal, detail);
                        }
                        (Ok(()), Some(ExecOutcome::SpawnFailed(e))) => {
                            eprintln!("    {} : SPAWN FAIL: {}", r.name, e);
                        }
                        (Ok(()), Some(ExecOutcome::Done { code, stdout })) => {
                            eprintln!("    {} : exit={} stdout=\"{}\"", r.name, code, fmt_stdout(stdout));
                        }
                        (Ok(()), None) => {
                            eprintln!("    {} : (not run)", r.name);
                        }
                    }
                }
            }
        }

        reports.push(ProgReport { name: ex.clone(), category: cat, results });
    }

    // ---- Final report ----
    println!("\n========== DIFFERENTIAL TEST REPORT ==========");
    println!("Examples scanned: {}", n_total);
    println!("Backends: {}", backends.iter().map(|b| b.name).collect::<Vec<_>>().join(", "));

    let n_pass = *counts.get(&ProgramCategory::Pass).unwrap_or(&0);
    let n_diff = *counts.get(&ProgramCategory::DiffFailure).unwrap_or(&0);
    let n_crash = *counts.get(&ProgramCategory::Crash).unwrap_or(&0);
    let n_timeout = *counts.get(&ProgramCategory::Timeout).unwrap_or(&0);
    let n_cfail_all = *counts.get(&ProgramCategory::CompileFailAll).unwrap_or(&0);
    let n_cfail_some = *counts.get(&ProgramCategory::CompileFailSome).unwrap_or(&0);

    println!("\n--- Summary ---");
    println!("  PASS                  : {:>3} / {}", n_pass, n_total);
    println!("  DIFFERENTIAL FAILURE  : {:>3} / {}", n_diff, n_total);
    println!("  CRASH (some backend)  : {:>3} / {}", n_crash, n_total);
    println!("  TIMEOUT (some backend): {:>3} / {}", n_timeout, n_total);
    println!("  COMPILE FAIL (all)    : {:>3} / {}", n_cfail_all, n_total);
    println!("  COMPILE FAIL (some)   : {:>3} / {}", n_cfail_some, n_total);

    // List per-category.
    fn list(reports: &[ProgReport], cat: &ProgramCategory, title: &str) {
        let matching: Vec<&ProgReport> = reports.iter().filter(|r| &r.category == cat).collect();
        if matching.is_empty() {
            return;
        }
        println!("\n--- {} ({}) ---", title, matching.len());
        for r in matching.iter() {
            println!("  {}", r.name);
            for br in &r.results {
                match (&br.compile, &br.exec) {
                    (Err(e), _) => {
                        println!("      {:<14} : COMPILE FAIL: {}", br.name, e);
                    }
                    (Ok(()), Some(ExecOutcome::Timeout)) => {
                        println!("      {:<14} : TIMEOUT", br.name);
                    }
                    (Ok(()), Some(ExecOutcome::Crash { code, signal, detail })) => {
                        let sig = signal.map(|s| format!("{}", s)).unwrap_or_else(|| "-".into());
                        println!("      {:<14} : CRASH (code={}, sig={}) {}", br.name, code, sig, detail);
                    }
                    (Ok(()), Some(ExecOutcome::SpawnFailed(e))) => {
                        println!("      {:<14} : SPAWN FAIL: {}", br.name, e);
                    }
                    (Ok(()), Some(ExecOutcome::Done { code, stdout })) => {
                        println!("      {:<14} : exit={} stdout=\"{}\"", br.name, code, fmt_stdout(stdout));
                    }
                    (Ok(()), None) => {
                        println!("      {:<14} : (not run)", br.name);
                    }
                }
            }
        }
    }

    list(&reports, &ProgramCategory::DiffFailure, "DIFFERENTIAL FAILURES");
    list(&reports, &ProgramCategory::Crash, "CRASHES");
    list(&reports, &ProgramCategory::Timeout, "TIMEOUTS");
    list(&reports, &ProgramCategory::CompileFailAll, "COMPILE FAILURES (all backends)");
    list(&reports, &ProgramCategory::CompileFailSome, "COMPILE FAILURES (some backends)");

    // PASS list — short, one per line.
    let passes: Vec<&ProgReport> = reports.iter().filter(|r| r.category == ProgramCategory::Pass).collect();
    if !passes.is_empty() {
        println!("\n--- PASS ({}) ---", passes.len());
        for r in passes.iter() {
            // Show the agreed exit code and stdout for the first backend.
            if let Some(first) = r.results.first() {
                if let (Ok(()), Some(ExecOutcome::Done { code, stdout })) = (&first.compile, &first.exec) {
                    println!("  {:<30} exit={}  stdout=\"{}\"", r.name, code, fmt_stdout(stdout));
                    continue;
                }
            }
            println!("  {}", r.name);
        }
    }

    // Final headline number the task asked for.
    println!("\n=== {} / {} programs have all backends agreeing ===", n_pass, n_total);
}
