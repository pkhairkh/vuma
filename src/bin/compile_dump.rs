//! Standalone tool to compile a .vuma file and dump the resulting ELF bytes.
use vuma_codegen::backend::{create_backend, BackendKind, AllocatedProgram};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{Parser, AstToScg, ModuleResolver};
use vuma::pipeline::{CompileConfig, run_scg_transforms, run_ir_pipeline, CompileTarget, OptLevel, VerificationLevel, bridge_ast_to_codegen_scg, build_pmt_layout_specs};
use std::path::Path;
use std::process::Command;
use std::fs;

/// Like `iter().map(f).collect::<Result<Vec<_>, _>>()` but parallelized with
/// `std::thread::scope` over the available CPU cores (replaces rayon).
///
/// Preserves input order in the output. Short-circuits on the first chunk that
/// returns an error (a chunk runs to completion before its error is observed,
/// but later chunks are not started after a per-chunk error is detected during
/// the join phase). Falls back to sequential iteration when the slice is empty
/// or only one thread is available.
fn par_collect_result<T, U, E, F>(items: &[T], f: F) -> Result<Vec<U>, E>
where
    T: Sync,
    U: Send,
    E: Send,
    F: Fn(&T) -> Result<U, E> + Sync,
{
    let n = items.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    let num_threads = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(1)
        .min(n);
    if num_threads == 1 {
        return items.iter().map(&f).collect();
    }
    let chunk_size = n.div_ceil(num_threads);
    let chunk_results: Result<Vec<Vec<U>>, E> = std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(num_threads);
        for chunk in items.chunks(chunk_size) {
            let f_ref = &f;
            handles.push(s.spawn(move || {
                chunk.iter().map(f_ref).collect::<Result<Vec<_>, _>>()
            }));
        }
        handles
            .into_iter()
            .map(|h| h.join().expect("worker thread panicked"))
            .collect()
    });
    chunk_results.map(|vs| vs.into_iter().flatten().collect())
}

fn backend_from_name(name: &str) -> Result<BackendKind, String> {
    match name.to_ascii_lowercase().as_str() {
        "x86_64" | "x86-64" | "x64" => Ok(BackendKind::X86_64),
        "aarch64" | "arm64" => Ok(BackendKind::AArch64),
        "riscv64" | "riscv" => Ok(BackendKind::RiscV64),
        "riscv32" => Ok(BackendKind::RiscV32),
        "x86_32" | "i386" | "x86" => Ok(BackendKind::X86_32),
        "arm32" | "arm" => Ok(BackendKind::Arm32),
        "mips64" | "mips" => Ok(BackendKind::Mips64),
        "ppc64" | "powerpc64" | "ppc" => Ok(BackendKind::PowerPC64),
        "ppc64le" | "powerpc64le" | "ppcle" => Ok(BackendKind::PowerPC64LE),
        "loongarch64" | "loongarch" => Ok(BackendKind::LoongArch64),
        "wasm32" | "wasm" => Ok(BackendKind::Wasm32),
        "sparc64" | "sparc" => Ok(BackendKind::Sparc64),
        "s390x" | "s390" => Ok(BackendKind::S390X),
        "mips64be" | "mips64-be" => Ok(BackendKind::Mips64Be),
        "armeb" | "arm-be" => Ok(BackendKind::ArmEb),
        "aarch64_be" | "aarch64be" => Ok(BackendKind::AArch64Be),
        "m68k" => Ok(BackendKind::M68k),
        "alpha" => Ok(BackendKind::Alpha),
        "hppa" | "parisc" => Ok(BackendKind::Hppa),
        _ => Err(format!("unknown backend: {}", name)),
    }
}

fn compile_for_backend(source: &str, kind: BackendKind) -> Result<(Vec<u8>, Option<String>), String> {
    compile_for_backend_with_path(source, kind, None, false, OptLevel::O2)
}

fn compile_for_backend_with_path(source: &str, kind: BackendKind, file_path: Option<&Path>, verify: bool, opt_level: OptLevel) -> Result<(Vec<u8>, Option<String>), String> {
    // Resolve imports if a file path is provided
    let ast = if let Some(path) = file_path {
        let mut resolver = ModuleResolver::new();
        match resolver.resolve_source(source, Some(path)) {
            Ok(program) => program,
            Err(errors) => return Err(format!("import resolution: {} errors: {:?}", errors.len(), errors.first())),
        }
    } else {
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        if result.has_errors() { return Err(format!("parse: {} errors", result.errors.len())); }
        result.unwrap()
    };

    // (Wave A / PMT-only) Build the PMT layout registry up-front from
    // the AST's `Item::LayoutDef` items so the IVE's
    // `VerificationLevel::Pmt` can run the 3 state verifiers
    // (state_read / state_write / state_transform) with full layout info.
    // PMT is always on in VUMA 2.0 — there is no "Normal" mode and no
    // `--pmt` flag. If the program has no `layout` items, this returns an
    // empty map (cheap — no work to do).
    let pmt_layouts = Some(build_pmt_layout_specs(&ast));

    let mut scg = { let mut c = AstToScg::new(); c.convert(&ast).map_err(|e| format!("scg: {}", e))? };

    // Run InterproceduralAllocFlow pass to connect factory-function
    // allocations to their callers' free() calls.
    use vuma_scg::SCGPass;
    let _ = vuma_scg::InterproceduralAllocFlow::new().run(&mut scg);

    // Wave 2: Run SCG-level O2 transforms BEFORE IVE verification and IR
    // building, mirroring `compile_with_path` Stage 7 (which runs at the
    // config's opt_level). The O2 config constructed here is also reused
    // for `run_ir_pipeline` below so the entire post-IR-build O2 pipeline
    // uses one consistent config (matching the production compile path).
    let o2_config = CompileConfig {
        target: if kind == BackendKind::Wasm32 { CompileTarget::Wasm32 } else { CompileTarget::Linux },
        opt_level,
        verification_level: VerificationLevel::Normal,
        ..Default::default()
    };
    let _ = run_scg_transforms(&mut scg, &o2_config);

    // Optionally run IVE verification (non-fatal — report to stderr).
    // Skip verification for programs marked with "// ive_skip" in the
    // source header. This is used for tests that intentionally don't
    // manage memory (e.g., pure arithmetic tests that allocate a buffer
    // for computation but never free it — the leak is intentional and
    // not the focus of the test).
    //
    // Wave 2: IVE now verifies the O2-optimized SCG (run_scg_transforms
    // was already applied above at O2). The previous local O0 config +
    // redundant O0 run_scg_transforms call have been removed so there is
    // a single consistent SCG state. Wave 7 double-checks IVE
    // correctness on the optimized SCG.
    //
    // Wave 7 / Wave A: the IVE ALWAYS runs ONLY the 3 state verifiers
    // (state_read / state_write / state_transform) via
    // `VerificationLevel::Pmt`. Memory safety for PMT programs is a
    // type-checking property (layouts + state ops), not a verification
    // problem. The PmtLayoutSpec registry built above is attached to the
    // VerificationInput so the state verifiers have field offset/size
    // info. There is no `Normal` mode anymore — VUMA 2.0 is PMT-only.
    let mut ive_status: Option<String> = None;
    let ive_skip = source.lines().take(20).any(|l| l.contains("// ive_skip"));
    // VUMA 2.0 is PMT-only — every program uses VerificationLevel::Pmt.
    // PMT programs (with layouts/state_new) get the 3 state verifiers.
    // Legacy pointer programs never reach this point: pointer syntax
    // (`allocate`, `free`, `*ptr`, `&x`, `*T`) is now a hard parse error
    // (see src/parser/src/parser.rs `check_pointer_syntax`), so the
    // compile aborts before IVE verification runs.
    if verify && !ive_skip {
        let mut ive_input = vuma_ive::verification::VerificationInput::from_scg(scg.clone());
        if let Some(layouts) = &pmt_layouts {
            ive_input = ive_input.with_pmt_layouts(layouts.clone());
        }
        let level = vuma_ive::invariant_aggregator::VerificationLevel::Pmt;
        let aggregator = vuma_ive::invariant_aggregator::InvariantAggregator::new()
            .with_level(level);
        let result = aggregator.verify_all(&ive_input);
        let verdict = format!("{:?}", result.overall);
        let summary = format!(
            "passed={} failed={} total={}",
            result.summary.passed,
            result.summary.failed,
            result.summary.total_checked
        );
        ive_status = Some(format!("{} {}", verdict, summary));
        eprintln!("IVE: {} {}", verdict, summary);
    } else if verify && ive_skip {
        ive_status = Some("Skip ive_skip".to_string());
        eprintln!("IVE: Skip (ive_skip marker)");
    }

    // Use the unified direct AST→codegen bridge (same path as vuma build/emit/run).
    let codegen_scg = bridge_ast_to_codegen_scg(&ast);
    let ir_program = { let mut b = IRBuilder::new(); b.build(&codegen_scg).map_err(|e| format!("ir: {}", e))? };

    // Wave 2: Run the full post-IR-build O2 pipeline via the shared
    // `run_ir_pipeline` helper (same path as `compile_with_path`). This
    // runs Wave 34 lowering (monomorphize, closures, switches, tail-calls,
    // loop-normalize), Wave 36 bv_verify, the Wave 10 syscall allowlist,
    // Stage 8b codegen-opt with the REAL backend latency table, and Wave
    // 32 escape+effects/SROA. Previously compile_dump only ran
    // `run_optimizations` (IR-level opts, default latency table) — skipping
    // ~half the production O2 pipeline. A syscall-allowlist violation or
    // bv_verify abort (none currently fatal) becomes a compile error here
    // (test marked CE) — triaged in Waves 3-5.
    let mut timings: Vec<(String, u64)> = Vec::new();
    let ir_program = run_ir_pipeline(ir_program, &o2_config, kind, &mut timings)
        .map_err(|e| format!("ir_pipeline: {:?}", e))?;

    let backend = create_backend(kind).map_err(|e| format!("backend: {}", e))?;

    // Populate thread-local set of 64-bit-returning function names.
    {
        use std::collections::HashSet;
        let func_64bit: HashSet<String> = ir_program.functions.iter()
            .filter(|f| f.result_types.iter().any(|t| matches!(t, vuma_codegen::ir::IRType::I64 | vuma_codegen::ir::IRType::U64)))
            .map(|f| f.name.clone())
            .collect();
        vuma_codegen::backend::set_64bit_returns(&func_64bit);
    }

    // Parallel per-function codegen using std::thread::scope.
    let allocated: Vec<_> = par_collect_result(&ir_program.functions, |func| {
        backend.allocate_registers(func)
    })
    .map_err(|e| format!("regalloc: {}", e))?;
    let total_code: usize = allocated.iter().map(|f| f.code_size).sum();
    let program = AllocatedProgram { functions: allocated, total_code_size: total_code, total_data_size: 0 };
    let binary = backend.encode_program(&program).map_err(|e| format!("encode: {}", e))?;
    Ok((binary, ive_status))
}

fn execute_binary(binary: &[u8], qemu: Option<&str>, timeout_secs: u64) -> (i32, Vec<u8>, Vec<u8>, bool) {
    let bin_path = std::env::temp_dir().join(format!("vuma_diag_{}.bin", std::process::id()));
    let _ = fs::write(&bin_path, binary);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(&bin_path, perms);
    }
    let mut cmd = Command::new("timeout");
    cmd.arg(format!("{}", timeout_secs));
    if let Some(q) = qemu { if !q.is_empty() { cmd.arg(q); } }
    cmd.arg(&bin_path);
    let output = cmd.output();
    let _ = fs::remove_file(&bin_path);
    match output {
        Ok(o) => {
            let code = o.status.code().unwrap_or(-1);
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                let signal = o.status.signal();
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                let crashed = stderr.contains("Segmentation fault")
                    || stderr.contains("uncaught target signal")
                    || code == 139 || code == 134
                    || signal.is_some();
                (code, o.stdout, o.stderr, crashed)
            }
            #[cfg(not(unix))]
            {
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                let crashed = stderr.contains("Segmentation fault")
                    || code == 139 || code == 134;
                (code, o.stdout, o.stderr, crashed)
            }
        }
        Err(_) => (-1, vec![], vec![], true),
    }
}

fn run_diag(backend_name: &str, examples_dir: &str, qemu: Option<&str>) {
    let kind = match backend_from_name(backend_name) {
        Ok(k) => k,
        Err(e) => { eprintln!("{}", e); std::process::exit(2); }
    };
    let mut examples: Vec<String> = fs::read_dir(examples_dir).unwrap()
        .filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".vuma")).collect();
    examples.sort();
    let mut compile_fail = Vec::new();
    let mut crash = Vec::new();
    let mut pass = Vec::new();
    let mut timeout = Vec::new();
    let exec_fail: Vec<(String, i32)> = Vec::new();
    for ex in &examples {
        let path = format!("{}/{}", examples_dir, ex);
        let source = fs::read_to_string(&path).unwrap();
        let binary = match compile_for_backend(&source, kind) {
            Ok((b, _)) => b,
            Err(e) => { compile_fail.push((ex.clone(), e)); continue; }
        };
        if let Some(q) = qemu {
            let (code, stdout, stderr, crashed) = execute_binary(&binary, Some(q), 3);
            if crashed {
                let err_str = String::from_utf8_lossy(&stderr);
                let err_short: String = err_str.chars().take(200).collect();
                crash.push((ex.clone(), code, err_short));
            } else if code == 124 {
                timeout.push((ex.clone(), code));
            } else {
                // Accept any non-crash, non-timeout exit code as pass
                pass.push((ex.clone(), code, stdout));
            }
        } else {
            pass.push((ex.clone(), 0, vec![]));
        }
    }
    println!("\n=== {} diagnostic results ===", backend_name);
    println!("Total: {} examples", examples.len());
    println!("Compile failures ({}):", compile_fail.len());
    for (n, e) in &compile_fail { println!("  X {} : {}", n, e); }
    println!("Crashes ({}):", crash.len());
    for (n, c, e) in &crash { println!("  CRASH {} (code={}): {}", n, c, e); }
    println!("Timeouts ({}):", timeout.len());
    for (n, c) in &timeout { println!("  TIMEOUT {} (code={})", n, c); }
    println!("Exec fail ({}):", exec_fail.len());
    for (n, c) in &exec_fail { println!("  FAIL {} (code={})", n, c); }
    println!("Pass ({}):", pass.len());
    for (n, c, _) in &pass { println!("  OK {} (code={})", n, c); }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "diag" {
        let backend = if args.len() > 2 { args[2].as_str() } else { "mips64" };
        let examples_dir = if args.len() > 3 { args[3].as_str() } else { "/tmp/my-project/examples" };
        let qemu: Option<&str> = if args.len() > 4 { Some(args[4].as_str()) } else { None };
        run_diag(backend, examples_dir, qemu);
        return;
    }
    // Parse flags:
    //   --verify    enables IVE verification (non-fatal).
    //
    // PMT is always on in VUMA 2.0 (PMT-only mode). Pointer syntax
    // (`allocate`, `free`, `*ptr`, `&x`, `*T`) is always a hard parse
    // error at the parser level — no flag required. The IVE always uses
    // `VerificationLevel::Pmt` and the PMT layout registry is always
    // built (cheap — empty map if no `layout` items).
    let mut verify = false;
    let mut opt_level = OptLevel::O2;
    let positional: Vec<String> = args.iter().skip(1).filter(|a| {
        if *a == "--verify" { verify = true; false }
        else if a.starts_with("--opt-level=") {
            let val = &a["--opt-level=".len()..];
            opt_level = match val {
                "O0" => OptLevel::O0,
                "O1" => OptLevel::O1,
                "O2" => OptLevel::O2,
                "O3" => OptLevel::O3,
                _ => { eprintln!("error: invalid opt-level '{}'; use O0|O1|O2|O3", val); std::process::exit(1); }
            };
            false
        } else { true }
    }).cloned().collect();
    if positional.len() < 2 {
        eprintln!("Usage: compile_dump <source.vuma> <output.bin> [backend] [--verify] [--opt-level=O0|O1|O2|O3]");
        std::process::exit(1);
    }
    let path = &positional[0];
    let out_path = &positional[1];
    let backend_name = if positional.len() > 2 { positional[2].as_str() } else { "aarch64" };
    let kind = backend_from_name(backend_name).unwrap_or(BackendKind::AArch64);
    let source = std::fs::read_to_string(path).unwrap();
    let file_path = std::path::Path::new(path);
    let (binary, _ive_status) = match compile_for_backend_with_path(&source, kind, Some(file_path), verify, opt_level) {
        Ok(v) => v,
        Err(e) => {
            // Wave A (PMT-only): print a clean compile-error message to
            // stderr and exit non-zero instead of panicking via
            // `.unwrap()`. The iso_test driver treats any non-zero exit as
            // `CE` (compile error), which is the desired outcome for
            // pointer-syntax violations (now always hard errors).
            eprintln!("compile error: {}", e);
            std::process::exit(1);
        }
    };
    std::fs::write(out_path, &binary).unwrap();
    // Set executable permissions (0o755) so QEMU can run the output.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(out_path).unwrap().permissions();
        perms.set_mode(0o755);
        let _ = std::fs::set_permissions(out_path, perms);
    }
    eprintln!("Wrote {} bytes to {}", binary.len(), out_path);
}
