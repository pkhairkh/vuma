//! Task 10-d: Comprehensive differential testing sweep.
//!
//! For each .vuma program in a sample list (default: /tmp/sample100.txt),
//! runs two sweeps:
//!
//!   (A) Differential sweep: compile on all 7 native backends
//!       (x86_64, aarch64, riscv64, arm32, mips64, ppc64, loongarch64),
//!       execute under QEMU (or natively for x86_64), 2-second timeout,
//!       and compare exit code + stdout across backends.
//!
//!   (B) O0-vs-O3 sweep: compile at OptLevel::O0 and OptLevel::O3 on the
//!       x86_64 backend, execute both natively, 2-second timeout, and
//!       compare exit code + stdout.
//!
//! Writes the full report to:
//!   /tmp/my-project/tests/gold_standard/differential_results.txt
//!
//! Usage:
//!   ./task10d_runner [sample_list_path] [timeout_secs]

use std::fs;
use std::io::Write;
use std::process::Command;
use std::time::Instant;

use vuma::pipeline::{
    bridge_scg_to_codegen, run_scg_transforms, CompileConfig, CompileTarget, OptLevel,
    VerificationLevel,
};
use vuma_codegen::backend::{create_backend, AllocatedProgram, BackendKind};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{AstToScg, Parser};

// ---------- Backends ----------

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
        BackendSpec { name: "x86_64",      kind: BackendKind::X86_64,      qemu: None },
        BackendSpec { name: "arm32",       kind: BackendKind::Arm32,       qemu: Some(QEMU_ARM) },
        BackendSpec { name: "mips64",      kind: BackendKind::Mips64,      qemu: Some(QEMU_MIPS64EL) },
        BackendSpec { name: "aarch64",     kind: BackendKind::AArch64,     qemu: Some(QEMU_AARCH64) },
        BackendSpec { name: "riscv64",     kind: BackendKind::RiscV64,     qemu: Some(QEMU_RISCV64) },
        BackendSpec { name: "ppc64",       kind: BackendKind::PowerPC64,   qemu: Some(QEMU_PPC64) },
        BackendSpec { name: "loongarch64", kind: BackendKind::LoongArch64, qemu: Some(QEMU_LOONGARCH64) },
    ]
}

// ---------- Outcomes ----------

#[derive(Clone)]
enum ExecOutcome {
    Done { code: i32, stdout: Vec<u8> },
    Crash { code: i32, signal: Option<i32>, detail: String },
    Timeout,
    SpawnFailed(String),
}

#[derive(Clone)]
struct BackendResult {
    name: String,
    compile_err: Option<String>,
    exec: Option<ExecOutcome>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum DiffCategory {
    Pass,
    DiffFailure,
    Crash,
    Timeout,
    CompileFailAll,
    CompileFailSome,
}

impl DiffCategory {
    fn label(&self) -> &'static str {
        match self {
            DiffCategory::Pass => "Pass",
            DiffCategory::DiffFailure => "DiffFailure",
            DiffCategory::Crash => "Crash",
            DiffCategory::Timeout => "Timeout",
            DiffCategory::CompileFailAll => "CompileFailAll",
            DiffCategory::CompileFailSome => "CompileFailSome",
        }
    }
}

fn categorise_diff(results: &[BackendResult]) -> DiffCategory {
    let compiled: Vec<&BackendResult> = results.iter().filter(|r| r.compile_err.is_none()).collect();
    if compiled.is_empty() {
        return DiffCategory::CompileFailAll;
    }
    if compiled.len() < results.len() {
        return DiffCategory::CompileFailSome;
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
        return DiffCategory::Timeout;
    }
    if any_crash {
        return DiffCategory::Crash;
    }
    let mut exit_codes: Vec<i32> = Vec::new();
    let mut stdouts: Vec<Vec<u8>> = Vec::new();
    for r in results.iter() {
        if let Some(ExecOutcome::Done { code, stdout }) = &r.exec {
            exit_codes.push(*code);
            stdouts.push(stdout.clone());
        }
    }
    if exit_codes.is_empty() {
        return DiffCategory::Crash;
    }
    let all_same_code = exit_codes.iter().all(|c| *c == exit_codes[0]);
    let all_same_stdout = stdouts.iter().all(|s| s == &stdouts[0]);
    if all_same_code && all_same_stdout {
        DiffCategory::Pass
    } else {
        DiffCategory::DiffFailure
    }
}

// ---------- O0/O3 outcomes ----------

#[derive(Clone)]
struct RunOutcome {
    code: i32,
    stdout: Vec<u8>,
    crashed: bool,
    timed_out: bool,
    spawn_failed: bool,
}

#[derive(Clone)]
struct PerLevelResult {
    compile_err: Option<String>,
    run: Option<RunOutcome>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum OptVerdict {
    Pass,
    Miscompilation,
    CrashAsym,
    TimeoutAsym,
    CompileFailO0,
    CompileFailO3,
    CompileFailBoth,
}

impl OptVerdict {
    fn label(&self) -> &'static str {
        match self {
            OptVerdict::Pass => "Pass",
            OptVerdict::Miscompilation => "Miscompilation",
            OptVerdict::CrashAsym => "CrashAsym",
            OptVerdict::TimeoutAsym => "TimeoutAsym",
            OptVerdict::CompileFailO0 => "CompileFailO0",
            OptVerdict::CompileFailO3 => "CompileFailO3",
            OptVerdict::CompileFailBoth => "CompileFailBoth",
        }
    }
}

// ---------- Compile / Execute ----------

fn compile(source: &str, kind: BackendKind, opt: OptLevel) -> Result<Vec<u8>, String> {
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    if result.has_errors() {
        return Err(format!("parse: {} errors", result.errors.len()));
    }
    let ast = match result.into_result() {
        Ok(a) => a,
        Err(_) => return Err("parse: unresolved errors".into()),
    };
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
        opt_level: opt,
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
        allocated.push(backend.allocate_registers(func).map_err(|e| format!("regalloc: {}", e))?);
    }
    let total_code: usize = allocated.iter().map(|f| f.code_size).sum();
    let program = AllocatedProgram {
        functions: allocated,
        total_code_size: total_code,
        total_data_size: 0,
    };
    backend.encode_program(&program).map_err(|e| format!("encode: {}", e))
}

fn execute(binary: &[u8], qemu: Option<&str>, timeout_secs: u64, tag: &str) -> ExecOutcome {
    let bin_path = std::env::temp_dir().join(format!("vuma_t10d_{}_{}.bin", tag, std::process::id()));
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
                    ExecOutcome::Timeout
                } else if crashed {
                    let detail: String = stderr.chars().take(180).collect();
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
                    let detail: String = stderr.chars().take(180).collect();
                    ExecOutcome::Crash { code, signal: None, detail }
                } else {
                    ExecOutcome::Done { code, stdout: o.stdout }
                }
            }
        }
        Err(e) => ExecOutcome::SpawnFailed(format!("spawn: {}", e)),
    }
}

fn run_native(binary: &[u8], timeout_secs: u64, tag: &str) -> RunOutcome {
    let bin_path = std::env::temp_dir()
        .join(format!("vuma_t10d_opt_{}_{}.bin", tag, std::process::id()));
    if fs::write(&bin_path, binary).is_err() {
        return RunOutcome {
            code: -1,
            stdout: vec![],
            crashed: false,
            timed_out: false,
            spawn_failed: true,
        };
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
    let output = Command::new("timeout")
        .arg(format!("{}", timeout_secs))
        .arg(&bin_path)
        .output();
    let _ = fs::remove_file(&bin_path);
    match output {
        Ok(o) => {
            let code = o.status.code().unwrap_or(-1);
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                let signal = o.status.signal();
                let stderr_str = String::from_utf8_lossy(&o.stderr).to_string();
                let crashed = stderr_str.contains("Segmentation fault")
                    || stderr_str.contains("uncaught target signal")
                    || code == 139
                    || code == 134
                    || signal.is_some();
                let timed_out = code == 124;
                RunOutcome {
                    code,
                    stdout: o.stdout,
                    crashed,
                    timed_out,
                    spawn_failed: false,
                }
            }
            #[cfg(not(unix))]
            {
                let stderr_str = String::from_utf8_lossy(&o.stderr).to_string();
                let crashed = stderr_str.contains("Segmentation fault") || code == 139 || code == 134;
                let timed_out = code == 124;
                RunOutcome {
                    code,
                    stdout: o.stdout,
                    crashed,
                    timed_out,
                    spawn_failed: false,
                }
            }
        }
        Err(_) => RunOutcome {
            code: -1,
            stdout: vec![],
            crashed: false,
            timed_out: false,
            spawn_failed: true,
        },
    }
}

// ---------- Formatting helpers ----------

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
    if out.len() > 60 {
        format!("{}..", &out[..60])
    } else {
        out
    }
}

fn fmt_exec(e: &ExecOutcome) -> String {
    match e {
        ExecOutcome::Done { code, stdout } => format!("exit={} out=\"{}\"", code, fmt_stdout(stdout)),
        ExecOutcome::Crash { code, signal, detail } => format!(
            "CRASH code={} sig={:?} {}",
            code,
            signal,
            detail.replace('\n', " ").chars().take(120).collect::<String>()
        ),
        ExecOutcome::Timeout => "TIMEOUT".to_string(),
        ExecOutcome::SpawnFailed(s) => format!("SPAWN-FAIL {}", s),
    }
}

fn fmt_run(r: &RunOutcome) -> String {
    if r.spawn_failed {
        "SPAWN-FAIL".to_string()
    } else if r.timed_out {
        "TIMEOUT".to_string()
    } else if r.crashed {
        format!("CRASH code={}", r.code)
    } else {
        format!("exit={} out=\"{}\"", r.code, fmt_stdout(&r.stdout))
    }
}

// ---------- Main ----------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let sample_path = if args.len() > 1 {
        args[1].as_str()
    } else {
        "/tmp/sample100.txt"
    };
    let timeout_secs: u64 = if args.len() > 2 {
        args[2].parse().unwrap_or(2)
    } else {
        2
    };

    // Read sample file list
    let sample_text = match fs::read_to_string(sample_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("FATAL: cannot read sample list '{}': {}", sample_path, e);
            std::process::exit(2);
        }
    };
    let files: Vec<String> = sample_text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let n_total = files.len();
    eprintln!(
        "Task 10-d: differential sweep over {} programs x 7 backends (+ O0/O3 on x86_64), timeout={}s",
        n_total, timeout_secs
    );

    let backends = backends();
    let result_path = "/tmp/my-project/tests/gold_standard/differential_results.txt";
    let raw_path = "/tmp/my-project/tests/gold_standard/differential_raw.tsv";

    // Open the raw TSV for streaming writes (so partial progress is preserved).
    let mut raw_file = match fs::File::create(raw_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("FATAL: cannot create raw file '{}': {}", raw_path, e);
            std::process::exit(2);
        }
    };
    let _ = writeln!(
        raw_file,
        "program\tcategory\t{}\to0\to3\topt_verdict",
        backends.iter().map(|b| b.name).collect::<Vec<_>>().join("\t")
    );

    // Per-program reports for the final detailed report
    struct ProgReport {
        name: String,
        category: DiffCategory,
        diff: Vec<BackendResult>,
        o0: PerLevelResult,
        o3: PerLevelResult,
        opt_verdict: OptVerdict,
    }
    let mut reports: Vec<ProgReport> = Vec::with_capacity(n_total);

    let start = Instant::now();
    let mut diff_counts: std::collections::BTreeMap<DiffCategory, usize> = std::collections::BTreeMap::new();
    let mut opt_counts: std::collections::BTreeMap<OptVerdict, usize> = std::collections::BTreeMap::new();
    // Per-backend crash/timeout tally for differential
    let mut backend_crash: std::collections::BTreeMap<&'static str, usize> = std::collections::BTreeMap::new();
    let mut backend_timeout: std::collections::BTreeMap<&'static str, usize> = std::collections::BTreeMap::new();
    let mut backend_compile_fail: std::collections::BTreeMap<&'static str, usize> = std::collections::BTreeMap::new();
    let mut backend_disagree: std::collections::BTreeMap<&'static str, usize> = std::collections::BTreeMap::new();

    for (idx, f) in files.iter().enumerate() {
        let name = basename(f);
        let source = match fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[{}/{}] {} READ-FAIL: {}", idx + 1, n_total, name, e);
                let diff: Vec<BackendResult> = backends
                    .iter()
                    .map(|b| BackendResult {
                        name: b.name.to_string(),
                        compile_err: Some(format!("read: {}", e)),
                        exec: None,
                    })
                    .collect();
                let o0 = PerLevelResult { compile_err: Some(format!("read: {}", e)), run: None };
                let o3 = PerLevelResult { compile_err: Some(format!("read: {}", e)), run: None };
                let category = categorise_diff(&diff);
                let opt_verdict = OptVerdict::CompileFailBoth;
                *diff_counts.entry(category).or_insert(0) += 1;
                *opt_counts.entry(opt_verdict).or_insert(0) += 1;
                let _ = writeln!(
                    raw_file,
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    name,
                    category.label(),
                    backends.iter().map(|_| "READ-FAIL").collect::<Vec<_>>().join("\t"),
                    "READ-FAIL",
                    "READ-FAIL",
                    opt_verdict.label()
                );
                let _ = raw_file.flush();
                reports.push(ProgReport { name, category, diff, o0, o3, opt_verdict });
                continue;
            }
        };

        // (A) Differential: compile + run on each backend
        let mut diff: Vec<BackendResult> = Vec::with_capacity(backends.len());
        for b in &backends {
            let compile_result = compile(&source, b.kind, OptLevel::O0);
            match compile_result {
                Ok(bin) => {
                    let exec = execute(&bin, b.qemu, timeout_secs, &format!("d_{}_{}", idx, b.name));
                    diff.push(BackendResult { name: b.name.to_string(), compile_err: None, exec: Some(exec) });
                }
                Err(e) => {
                    *backend_compile_fail.entry(b.name).or_insert(0) += 1;
                    diff.push(BackendResult { name: b.name.to_string(), compile_err: Some(e), exec: None });
                }
            }
        }
        let category = categorise_diff(&diff);
        *diff_counts.entry(category).or_insert(0) += 1;

        // Per-backend crash / timeout / disagreement tally.
        // Iterate by index so we can use the backend spec's 'static name
        // (the BackendResult.name is a borrowed String, which would
        // otherwise fail the 'static bound on the BTreeMap key).
        let mut exit_codes_seen: std::collections::BTreeMap<i32, usize> = std::collections::BTreeMap::new();
        for (i, r) in diff.iter().enumerate() {
            let bname: &'static str = backends[i].name;
            match &r.exec {
                Some(ExecOutcome::Crash { .. }) => {
                    *backend_crash.entry(bname).or_insert(0) += 1;
                }
                Some(ExecOutcome::Timeout) => {
                    *backend_timeout.entry(bname).or_insert(0) += 1;
                }
                Some(ExecOutcome::Done { code, .. }) => {
                    *exit_codes_seen.entry(*code).or_insert(0) += 1;
                }
                _ => {}
            }
        }
        let consensus = exit_codes_seen.iter().max_by_key(|(_, c)| **c).map(|(k, _)| *k);
        if let Some(consensus_code) = consensus {
            for (i, r) in diff.iter().enumerate() {
                let bname: &'static str = backends[i].name;
                if let Some(ExecOutcome::Done { code, .. }) = &r.exec {
                    if *code != consensus_code {
                        *backend_disagree.entry(bname).or_insert(0) += 1;
                    }
                }
            }
        }

        // (B) O0 vs O3 on x86_64
        let o0_binary = compile(&source, BackendKind::X86_64, OptLevel::O0);
        let o3_binary = compile(&source, BackendKind::X86_64, OptLevel::O3);
        let mut o0 = PerLevelResult { compile_err: None, run: None };
        let mut o3 = PerLevelResult { compile_err: None, run: None };
        match &o0_binary {
            Ok(bin) => o0.run = Some(run_native(bin, timeout_secs, &format!("o0_{}", idx))),
            Err(e) => o0.compile_err = Some(e.clone()),
        }
        match &o3_binary {
            Ok(bin) => o3.run = Some(run_native(bin, timeout_secs, &format!("o3_{}", idx))),
            Err(e) => o3.compile_err = Some(e.clone()),
        }
        let opt_verdict = match (&o0.compile_err, &o3.compile_err) {
            (Some(_), Some(_)) => OptVerdict::CompileFailBoth,
            (Some(_), None) => OptVerdict::CompileFailO0,
            (None, Some(_)) => OptVerdict::CompileFailO3,
            (None, None) => {
                let r0 = o0.run.as_ref().unwrap();
                let r3 = o3.run.as_ref().unwrap();
                if r0.timed_out != r3.timed_out {
                    OptVerdict::TimeoutAsym
                } else if r0.crashed != r3.crashed {
                    OptVerdict::CrashAsym
                } else if r0.timed_out && r3.timed_out {
                    OptVerdict::Pass
                } else if r0.code == r3.code && r0.stdout == r3.stdout {
                    OptVerdict::Pass
                } else {
                    OptVerdict::Miscompilation
                }
            }
        };
        *opt_counts.entry(opt_verdict).or_insert(0) += 1;

        // Stream one TSV row to the raw file
        let diff_cells: Vec<String> = diff
            .iter()
            .map(|r| match (&r.compile_err, &r.exec) {
                (Some(e), _) => format!("CFAIL:{}", e.chars().take(40).collect::<String>()),
                (None, Some(e)) => fmt_exec(e),
                (None, None) => "NOEXEC".to_string(),
            })
            .collect();
        let o0_cell = match (&o0.compile_err, &o0.run) {
            (Some(e), _) => format!("CFAIL:{}", e.chars().take(40).collect::<String>()),
            (None, Some(r)) => fmt_run(r),
            _ => "NOEXEC".to_string(),
        };
        let o3_cell = match (&o3.compile_err, &o3.run) {
            (Some(e), _) => format!("CFAIL:{}", e.chars().take(40).collect::<String>()),
            (None, Some(r)) => fmt_run(r),
            _ => "NOEXEC".to_string(),
        };
        let _ = writeln!(
            raw_file,
            "{}\t{}\t{}\t{}\t{}\t{}",
            name,
            category.label(),
            diff_cells.join("\t"),
            o0_cell,
            o3_cell,
            opt_verdict.label()
        );
        let _ = raw_file.flush();

        reports.push(ProgReport {
            name,
            category,
            diff,
            o0,
            o3,
            opt_verdict,
        });

        if (idx + 1) % 5 == 0 || idx + 1 == n_total {
            eprintln!(
                "[{}/{}] {} cat={} opt={} elapsed={:.1}s",
                idx + 1,
                n_total,
                basename(f),
                category.label(),
                opt_verdict.label(),
                start.elapsed().as_secs_f32()
            );
        }
    }

    // ---------- Write final report ----------
    let mut out = String::new();
    out.push_str("VUMA Differential Testing Results\n");
    out.push_str(&format!("Date: {}\n", chrono_rfc2822_or_fallback()));
    out.push_str(&format!("Sample: {} programs x 7 backends = {} differential runs\n", n_total, n_total * 7));
    out.push_str(&format!("Plus: {} programs x 2 opt levels = {} O0-vs-O3 runs (x86_64)\n", n_total, n_total * 2));
    out.push_str(&format!("Total runs: {}\n", n_total * 7 + n_total * 2));
    out.push_str(&format!("Per-execution timeout: {}s\n", timeout_secs));
    out.push_str("Backends: x86_64 (native), arm32, mips64, aarch64, riscv64, ppc64, loongarch64 (all under QEMU)\n");
    out.push_str("\nNote on mips64: the mips64 backend emits big-endian MIPS64 ELF, but the only\n");
    out.push_str("  available QEMU binary is qemu-mips64el (little-endian). QEMU will refuse\n");
    out.push_str("  the resulting binaries with an 'Invalid ELF' error. MIPS numbers in this\n");
    out.push_str("  report are therefore not real execution results — they reflect QEMU's ELF\n");
    out.push_str("  rejection (typically exit code 1). See Task 9-b in worklog.md for context.\n");
    out.push_str("Note on loongarch64: documented broken call/return path (Task 8-a); tests\n");
    out.push_str("  involving function calls frequently crash or time out on this backend.\n");
    out.push_str("Note on ppc64: documented Address-typed-return calling-convention bug\n");
    out.push_str("  (Task 9-b); *_func_load / struct2_func_load / *_address_return tests\n");
    out.push_str("  typically exit 0 on ppc64.\n");
    out.push('\n');

    // ----- Section 1: Per-program differential table -----
    out.push_str("=== Section 1: Per-program differential sweep (7 backends) ===\n");
    out.push_str(&format!(
        "{:<3} {:<28} {:<14} {:<10} {:<10} {:<10} {:<10} {:<10} {:<10} {:<10}\n",
        "#", "program", "category",
        "x86_64", "arm32", "mips64", "aarch64", "riscv64", "ppc64", "loong"
    ));
    out.push_str(&"-".repeat(140));
    out.push('\n');
    for (i, r) in reports.iter().enumerate() {
        let short = |r: &BackendResult| -> String {
            match (&r.compile_err, &r.exec) {
                (Some(_), _) => "CFAIL".to_string(),
                (None, Some(ExecOutcome::Done { code, .. })) => format!("e{}", code),
                (None, Some(ExecOutcome::Crash { code, .. })) => format!("CRASH({})", code),
                (None, Some(ExecOutcome::Timeout)) => "TMO".to_string(),
                (None, Some(ExecOutcome::SpawnFailed(_))) => "SPWN".to_string(),
                (None, None) => "?".to_string(),
            }
        };
        let row: Vec<String> = r.diff.iter().map(short).collect();
        out.push_str(&format!(
            "{:<3} {:<28} {:<14} {:<10} {:<10} {:<10} {:<10} {:<10} {:<10} {:<10}\n",
            i + 1,
            truncate(&r.name, 28),
            r.category.label(),
            row[0], row[1], row[2], row[3], row[4], row[5], row[6]
        ));
    }
    out.push('\n');

    // ----- Section 2: Differential summary -----
    let n_pass = *diff_counts.get(&DiffCategory::Pass).unwrap_or(&0);
    let n_diff = *diff_counts.get(&DiffCategory::DiffFailure).unwrap_or(&0);
    let n_crash = *diff_counts.get(&DiffCategory::Crash).unwrap_or(&0);
    let n_tmo = *diff_counts.get(&DiffCategory::Timeout).unwrap_or(&0);
    let n_cfa = *diff_counts.get(&DiffCategory::CompileFailAll).unwrap_or(&0);
    let n_cfs = *diff_counts.get(&DiffCategory::CompileFailSome).unwrap_or(&0);

    out.push_str("=== Section 2: Differential summary ===\n");
    out.push_str(&format!("Total programs           : {}\n", n_total));
    out.push_str(&format!("All backends agree (Pass): {} ({:.1}%)\n", n_pass, pct(n_pass, n_total)));
    out.push_str(&format!("Differential failure     : {} ({:.1}%)  -- exit code or stdout differ\n", n_diff, pct(n_diff, n_total)));
    out.push_str(&format!("Crash on at least 1 backend: {} ({:.1}%)\n", n_crash, pct(n_crash, n_total)));
    out.push_str(&format!("Timeout on at least 1 backend: {} ({:.1}%)\n", n_tmo, pct(n_tmo, n_total)));
    out.push_str(&format!("Compile-fail on ALL backends: {} ({:.1}%)\n", n_cfa, pct(n_cfa, n_total)));
    out.push_str(&format!("Compile-fail on SOME (not all): {} ({:.1}%)\n", n_cfs, pct(n_cfs, n_total)));
    out.push('\n');
    out.push_str("Per-backend tally (across all programs):\n");
    out.push_str(&format!(
        "  {:<14} {:>10} {:>10} {:>10} {:>12}\n",
        "backend", "crashes", "timeouts", "cfails", "disagreements"
    ));
    for b in &backends {
        out.push_str(&format!(
            "  {:<14} {:>10} {:>10} {:>10} {:>12}\n",
            b.name,
            backend_crash.get(b.name).copied().unwrap_or(0),
            backend_timeout.get(b.name).copied().unwrap_or(0),
            backend_compile_fail.get(b.name).copied().unwrap_or(0),
            backend_disagree.get(b.name).copied().unwrap_or(0)
        ));
    }
    out.push('\n');

    // ----- Section 3: Differential failures (detailed) -----
    out.push_str("=== Section 3: Detailed differential failures (exit code or stdout differ) ===\n");
    let mut diff_failures_shown = 0;
    for r in &reports {
        if r.category != DiffCategory::DiffFailure {
            continue;
        }
        diff_failures_shown += 1;
        out.push_str(&format!("\n>> {}:\n", r.name));
        for br in &r.diff {
            let cell = match (&br.compile_err, &br.exec) {
                (Some(e), _) => format!("COMPILE-FAIL: {}", e),
                (None, Some(e)) => fmt_exec(e),
                _ => "NO-EXEC".to_string(),
            };
            out.push_str(&format!("  {:<14} {}\n", br.name, cell));
        }
    }
    if diff_failures_shown == 0 {
        out.push_str("  (none)\n");
    }
    out.push('\n');

    // ----- Section 4: Crashes (detailed) -----
    out.push_str("=== Section 4: Detailed crashes (at least one backend crashed) ===\n");
    let mut crashes_shown = 0;
    for r in &reports {
        if r.category != DiffCategory::Crash {
            continue;
        }
        crashes_shown += 1;
        out.push_str(&format!("\n>> {}:\n", r.name));
        for br in &r.diff {
            if let Some(ExecOutcome::Crash { code, signal, detail }) = &br.exec {
                out.push_str(&format!(
                    "  {:<14} CRASH code={} sig={:?} {}\n",
                    br.name,
                    code,
                    signal,
                    detail.replace('\n', " ").chars().take(160).collect::<String>()
                ));
            } else {
                let cell = match (&br.compile_err, &br.exec) {
                    (Some(e), _) => format!("COMPILE-FAIL: {}", e),
                    (None, Some(e)) => fmt_exec(e),
                    _ => "NO-EXEC".to_string(),
                };
                out.push_str(&format!("  {:<14} {}\n", br.name, cell));
            }
        }
    }
    if crashes_shown == 0 {
        out.push_str("  (none)\n");
    }
    out.push('\n');

    // ----- Section 5: O0 vs O3 sweep -----
    out.push_str("=== Section 5: O0 vs O3 optimizer sweep (x86_64 native) ===\n");
    out.push_str(&format!(
        "{:<3} {:<28} {:<14} {:<30} {:<30}\n",
        "#", "program", "verdict", "O0", "O3"
    ));
    out.push_str(&"-".repeat(110));
    out.push('\n');
    for (i, r) in reports.iter().enumerate() {
        let o0_cell = match (&r.o0.compile_err, &r.o0.run) {
            (Some(e), _) => format!("CFAIL:{}", truncate(e, 26)),
            (None, Some(r0)) => fmt_run(r0),
            _ => "?".to_string(),
        };
        let o3_cell = match (&r.o3.compile_err, &r.o3.run) {
            (Some(e), _) => format!("CFAIL:{}", truncate(e, 26)),
            (None, Some(r3)) => fmt_run(r3),
            _ => "?".to_string(),
        };
        out.push_str(&format!(
            "{:<3} {:<28} {:<14} {:<30} {:<30}\n",
            i + 1,
            truncate(&r.name, 28),
            r.opt_verdict.label(),
            truncate(&o0_cell, 30),
            truncate(&o3_cell, 30)
        ));
    }
    out.push('\n');

    // ----- Section 6: O0/O3 summary -----
    let o_pass = *opt_counts.get(&OptVerdict::Pass).unwrap_or(&0);
    let o_mis = *opt_counts.get(&OptVerdict::Miscompilation).unwrap_or(&0);
    let o_ca = *opt_counts.get(&OptVerdict::CrashAsym).unwrap_or(&0);
    let o_ta = *opt_counts.get(&OptVerdict::TimeoutAsym).unwrap_or(&0);
    let o_fo0 = *opt_counts.get(&OptVerdict::CompileFailO0).unwrap_or(&0);
    let o_fo3 = *opt_counts.get(&OptVerdict::CompileFailO3).unwrap_or(&0);
    let o_fb = *opt_counts.get(&OptVerdict::CompileFailBoth).unwrap_or(&0);

    out.push_str("=== Section 6: O0 vs O3 summary ===\n");
    out.push_str(&format!("Total programs         : {}\n", n_total));
    out.push_str(&format!("PASS (O0 == O3)        : {} ({:.1}%)\n", o_pass, pct(o_pass, n_total)));
    out.push_str(&format!("Miscompilation (O0!=O3): {} ({:.1}%)\n", o_mis, pct(o_mis, n_total)));
    out.push_str(&format!("Crash asymmetry        : {} ({:.1}%)\n", o_ca, pct(o_ca, n_total)));
    out.push_str(&format!("Timeout asymmetry      : {} ({:.1}%)\n", o_ta, pct(o_ta, n_total)));
    out.push_str(&format!("Compile-fail O0 only   : {} ({:.1}%)\n", o_fo0, pct(o_fo0, n_total)));
    out.push_str(&format!("Compile-fail O3 only   : {} ({:.1}%)\n", o_fo3, pct(o_fo3, n_total)));
    out.push_str(&format!("Compile-fail both      : {} ({:.1}%)\n", o_fb, pct(o_fb, n_total)));
    out.push('\n');

    // ----- Section 7: Detailed O0/O3 miscompilations -----
    out.push_str("=== Section 7: Detailed O0/O3 miscompilations ===\n");
    let mut mis_shown = 0;
    for r in &reports {
        if r.opt_verdict != OptVerdict::Miscompilation {
            continue;
        }
        mis_shown += 1;
        let r0 = r.o0.run.as_ref().unwrap();
        let r3 = r.o3.run.as_ref().unwrap();
        out.push_str(&format!("\n>> {}:\n", r.name));
        out.push_str(&format!("  O0: {}\n", fmt_run(r0)));
        out.push_str(&format!("  O3: {}\n", fmt_run(r3)));
        if r0.code != r3.code {
            out.push_str("  >> EXIT CODE DIFFERS\n");
        }
        if r0.stdout != r3.stdout {
            out.push_str("  >> STDOUT DIFFERS\n");
        }
    }
    if mis_shown == 0 {
        out.push_str("  (none)\n");
    }
    out.push('\n');

    // ----- Section 8: Headline summary -----
    out.push_str("=== Section 8: Headline summary ===\n");
    out.push_str(&format!(
        "Differential agreement rate: {}/{} = {:.1}%\n",
        n_pass, n_total, pct(n_pass, n_total)
    ));
    out.push_str(&format!(
        "O0/O3 agreement rate       : {}/{} = {:.1}%\n",
        o_pass, n_total, pct(o_pass, n_total)
    ));
    out.push_str(&format!(
        "Total wall-clock elapsed   : {:.1}s\n",
        start.elapsed().as_secs_f32()
    ));
    out.push_str(&format!("\nRaw per-program TSV: {}\n", raw_path));

    match fs::write(result_path, out) {
        Ok(_) => {
            eprintln!("Report written to: {}", result_path);
            eprintln!("Raw TSV written to: {}", raw_path);
        }
        Err(e) => {
            eprintln!("FATAL: cannot write report '{}': {}", result_path, e);
            std::process::exit(3);
        }
    }

    // Print final headline summary to stderr too
    eprintln!("--- DONE ---");
    eprintln!(
        "Differential: Pass={}/{}, DiffFail={}, Crash={}, Timeout={}, CFailAll={}, CFailSome={}",
        n_pass, n_total, n_diff, n_crash, n_tmo, n_cfa, n_cfs
    );
    eprintln!(
        "O0/O3:        Pass={}/{}, Miscomp={}, CrashAsym={}, TimeoutAsym={}, CFailO0={}, CFailO3={}, CFailBoth={}",
        o_pass, n_total, o_mis, o_ca, o_ta, o_fo0, o_fo3, o_fb
    );
    eprintln!("Elapsed: {:.1}s", start.elapsed().as_secs_f32());
}

// ---------- Helpers ----------

fn basename(p: &str) -> String {
    let trimmed = p.trim_end_matches(".vuma");
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    last.to_string()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(2)).collect();
        out.push_str("..");
        out
    }
}

fn pct(num: usize, den: usize) -> f64 {
    if den == 0 {
        0.0
    } else {
        (num as f64) * 100.0 / (den as f64)
    }
}

fn chrono_rfc2822_or_fallback() -> String {
    match std::process::Command::new("date").output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(_) => format!(
            "(unix epoch {})",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        ),
    }
}
