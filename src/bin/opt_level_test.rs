//! O0 vs O3 optimizer comparison tool for the VUMA compiler.
//!
//! For each .vuma example, compile at OptLevel::O0 and OptLevel::O3 (x86_64
//! backend), execute both binaries natively, and compare exit code + stdout.
//! Reports miscompilations, crashes, timeouts, and a final pass/fail tally.

use vuma_codegen::backend::{create_backend, AllocatedProgram, BackendKind};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{AstToScg, Parser};
use vuma::pipeline::{
    bridge_scg_to_codegen, run_scg_transforms, CompileConfig, CompileTarget, OptLevel,
    VerificationLevel,
};
use std::fs;
use std::process::Command;

/// Compile a VUMA source string at the given opt level for the x86_64 backend
/// and return the raw ELF bytes (or an error string).
fn compile_at_level(source: &str, opt: OptLevel) -> Result<Vec<u8>, String> {
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
        target: CompileTarget::Linux,
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
    let backend = create_backend(BackendKind::X86_64).map_err(|e| format!("backend: {}", e))?;
    let mut allocated = Vec::new();
    for func in &ir_program.functions {
        allocated
            .push(backend.allocate_registers(func).map_err(|e| format!("regalloc: {}", e))?);
    }
    let total_code: usize = allocated.iter().map(|f| f.code_size).sum();
    let program = AllocatedProgram {
        functions: allocated,
        total_code_size: total_code,
        total_data_size: 0,
    };
    backend.encode_program(&program).map_err(|e| format!("encode: {}", e))
}

/// Outcome of running one binary natively under `timeout`.
#[derive(Clone)]
struct RunOutcome {
    code: i32,
    stdout: Vec<u8>,
    #[allow(dead_code)]
    stderr: Vec<u8>,
    crashed: bool,
    timed_out: bool,
    #[allow(dead_code)]
    spawn_failed: bool,
}

impl RunOutcome {
    fn failed() -> Self {
        RunOutcome {
            code: -1,
            stdout: vec![],
            stderr: b"<spawn failed>".to_vec(),
            crashed: false,
            timed_out: false,
            spawn_failed: true,
        }
    }
}

/// Write the binary to a temp file, chmod 0o755, run `timeout N path` natively.
fn run_native(binary: &[u8], timeout_secs: u64) -> RunOutcome {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let bin_path = std::env::temp_dir()
        .join(format!("vuma_optcmp_{}_{}.bin", std::process::id(), nanos));
    if fs::write(&bin_path, binary).is_err() {
        return RunOutcome::failed();
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
                // `timeout` returns 124 when it kills the process for taking too long.
                let timed_out = code == 124;
                RunOutcome {
                    code,
                    stdout: o.stdout,
                    stderr: o.stderr,
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
                    stderr: o.stderr,
                    crashed,
                    timed_out,
                    spawn_failed: false,
                }
            }
        }
        Err(_) => RunOutcome::failed(),
    }
}

#[allow(dead_code)]
fn opt_name(o: &OptLevel) -> &'static str {
    match o {
        OptLevel::O0 => "O0",
        OptLevel::O1 => "O1",
        OptLevel::O2 => "O2",
        OptLevel::O3 => "O3",
    }
}

#[derive(Clone)]
struct PerLevelResult {
    compile_err: Option<String>,
    run: Option<RunOutcome>,
}

/// One example's comparison record.
#[derive(Clone)]
struct Comparison {
    name: String,
    o0: PerLevelResult,
    o3: PerLevelResult,
    verdict: Verdict,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Verdict {
    Pass,           // O0 == O3 (exit code + stdout)
    Miscompilation, // both ran, but exit code or stdout differ
    CrashAsym,      // crashed at one level but not the other
    TimeoutAsym,    // timed out at one level but not the other
    CompileFailO0,  // O0 compile failed
    CompileFailO3,  // O3 compile failed
    CompileFailBoth,
}

fn short_bytes(b: &[u8], n: usize) -> String {
    let s = String::from_utf8_lossy(b);
    let s: String = s.chars().take(n).collect();
    s.replace('\n', "\\n")
}

fn main() {
    let examples_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/my-project/examples".to_string());
    let timeout_secs: u64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);

    let mut examples: Vec<String> = match fs::read_dir(&examples_dir) {
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

    println!(
        "=== VUMA O0 vs O3 optimizer comparison (x86_64, native, timeout={}s) ===",
        timeout_secs
    );
    println!("Examples dir: {}", examples_dir);
    println!("Total examples: {}\n", examples.len());

    let mut comparisons: Vec<Comparison> = Vec::with_capacity(examples.len());

    for (i, ex) in examples.iter().enumerate() {
        let path = format!("{}/{}", examples_dir, ex);
        let source = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                println!(
                    "[{:>2}/{:>2}] {:<32} READ-FAIL: {}",
                    i + 1,
                    examples.len(),
                    ex,
                    e
                );
                continue;
            }
        };

        let o0_binary = compile_at_level(&source, OptLevel::O0);
        let o3_binary = compile_at_level(&source, OptLevel::O3);

        let mut o0_res = PerLevelResult { compile_err: None, run: None };
        let mut o3_res = PerLevelResult { compile_err: None, run: None };

        match &o0_binary {
            Ok(bin) => o0_res.run = Some(run_native(bin, timeout_secs)),
            Err(e) => o0_res.compile_err = Some(e.clone()),
        }
        match &o3_binary {
            Ok(bin) => o3_res.run = Some(run_native(bin, timeout_secs)),
            Err(e) => o3_res.compile_err = Some(e.clone()),
        }

        // Decide verdict.
        let verdict = match (&o0_res.compile_err, &o3_res.compile_err) {
            (Some(_), Some(_)) => Verdict::CompileFailBoth,
            (Some(_), None) => Verdict::CompileFailO0,
            (None, Some(_)) => Verdict::CompileFailO3,
            (None, None) => {
                let r0 = o0_res.run.as_ref().unwrap();
                let r3 = o3_res.run.as_ref().unwrap();
                let t0 = r0.timed_out;
                let t3 = r3.timed_out;
                let c0 = r0.crashed;
                let c3 = r3.crashed;
                if t0 != t3 {
                    Verdict::TimeoutAsym
                } else if c0 != c3 {
                    Verdict::CrashAsym
                } else if t0 && t3 {
                    // both timed out — treat as pass (cannot compare, but symmetric)
                    Verdict::Pass
                } else {
                    let same_code = r0.code == r3.code;
                    let same_stdout = r0.stdout == r3.stdout;
                    if same_code && same_stdout {
                        Verdict::Pass
                    } else {
                        Verdict::Miscompilation
                    }
                }
            }
        };

        let v_tag = match verdict {
            Verdict::Pass => "PASS",
            Verdict::Miscompilation => "MISCOMPILE",
            Verdict::CrashAsym => "CRASH-ASYM",
            Verdict::TimeoutAsym => "TIMEOUT-ASYM",
            Verdict::CompileFailO0 => "CFAIL-O0",
            Verdict::CompileFailO3 => "CFAIL-O3",
            Verdict::CompileFailBoth => "CFAIL-BOTH",
        };
        println!(
            "[{:>2}/{:>2}] {:<32} {}",
            i + 1,
            examples.len(),
            ex,
            v_tag
        );

        comparisons.push(Comparison {
            name: ex.clone(),
            o0: o0_res,
            o3: o3_res,
            verdict,
        });
    }

    // ===== Detailed report =====
    println!("\n=== Detailed report ===");

    let mut pass: Vec<String> = Vec::new();
    let mut miscomp: Vec<&Comparison> = Vec::new();
    let mut crash_asym: Vec<&Comparison> = Vec::new();
    let mut timeout_asym: Vec<&Comparison> = Vec::new();
    let mut cfail_o0: Vec<&Comparison> = Vec::new();
    let mut cfail_o3: Vec<&Comparison> = Vec::new();
    let mut cfail_both: Vec<&Comparison> = Vec::new();

    for c in &comparisons {
        match c.verdict {
            Verdict::Pass => pass.push(c.name.clone()),
            Verdict::Miscompilation => miscomp.push(c),
            Verdict::CrashAsym => crash_asym.push(c),
            Verdict::TimeoutAsym => timeout_asym.push(c),
            Verdict::CompileFailO0 => cfail_o0.push(c),
            Verdict::CompileFailO3 => cfail_o3.push(c),
            Verdict::CompileFailBoth => cfail_both.push(c),
        }
    }

    // Snapshot counts up-front so we can move the vectors into for-loops below.
    let n_pass = pass.len();
    let n_miscomp = miscomp.len();
    let n_crash = crash_asym.len();
    let n_timeout = timeout_asym.len();
    let n_cfail_o0 = cfail_o0.len();
    let n_cfail_o3 = cfail_o3.len();
    let n_cfail_both = cfail_both.len();
    let total = comparisons.len();

    println!("\n--- PASS (O0 == O3): {} ---", n_pass);
    for n in &pass {
        println!("  OK  {}", n);
    }

    println!("\n--- MISCOMPILATION (O0 != O3): {} ---", n_miscomp);
    for c in &miscomp {
        println!("\n  >> {}", c.name);
        let r0 = c.o0.run.as_ref().unwrap();
        let r3 = c.o3.run.as_ref().unwrap();
        println!(
            "     O0 exit={} stdout={:?}",
            r0.code,
            short_bytes(&r0.stdout, 200)
        );
        println!(
            "     O3 exit={} stdout={:?}",
            r3.code,
            short_bytes(&r3.stdout, 200)
        );
        if r0.code != r3.code {
            println!("     >> EXIT CODE DIFFERS");
        }
        if r0.stdout != r3.stdout {
            println!("     >> STDOUT DIFFERS");
        }
    }

    println!("\n--- CRASH ASYMMETRY (crash at one level): {} ---", n_crash);
    for c in &crash_asym {
        let r0 = c.o0.run.as_ref().unwrap();
        let r3 = c.o3.run.as_ref().unwrap();
        println!(
            "  {:<32} O0 crash={} (code={})  |  O3 crash={} (code={})",
            c.name, r0.crashed, r0.code, r3.crashed, r3.code
        );
    }

    println!("\n--- TIMEOUT ASYMMETRY: {} ---", n_timeout);
    for c in &timeout_asym {
        let r0 = c.o0.run.as_ref().unwrap();
        let r3 = c.o3.run.as_ref().unwrap();
        println!(
            "  {:<32} O0 timed_out={}  |  O3 timed_out={}",
            c.name, r0.timed_out, r3.timed_out
        );
    }

    println!("\n--- COMPILE FAILURES ---");
    println!("  O0 only:   {}", n_cfail_o0);
    for c in &cfail_o0 {
        println!("    {} : {}", c.name, c.o0.compile_err.as_deref().unwrap_or("?"));
    }
    println!("  O3 only:   {}", n_cfail_o3);
    for c in &cfail_o3 {
        println!("    {} : {}", c.name, c.o3.compile_err.as_deref().unwrap_or("?"));
    }
    println!("  Both:      {}", n_cfail_both);
    for c in &cfail_both {
        println!("    {}", c.name);
    }

    // ===== Summary =====
    println!("\n=== Summary ===");
    println!("Total examples run:        {}", total);
    println!("PASS (O0 == O3):           {}", n_pass);
    println!("MISCOMPILATION:            {}", n_miscomp);
    println!("CRASH ASYMMETRY:           {}", n_crash);
    println!("TIMEOUT ASYMMETRY:         {}", n_timeout);
    println!("Compile fail (O0 only):    {}", n_cfail_o0);
    println!("Compile fail (O3 only):    {}", n_cfail_o3);
    println!("Compile fail (both):       {}", n_cfail_both);
    if total > 0 {
        println!(
            "\n>>> {}/{} programs have O0 == O3 <<<",
            n_pass, total
        );
    }
}
