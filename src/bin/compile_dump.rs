//! Standalone tool to compile a .vuma file and dump the resulting ELF bytes.
use std::fs;
use std::path::Path;
use std::process::Command;
use vuma::pipeline::{
    bridge_ast_to_codegen_scg, build_alloc_sizes, build_pmt_layout_specs, collect_secret_vars,
    run_ir_pipeline, run_scg_transforms, CompileConfig, CompileTarget, OptLevel, VerificationLevel,
};
use vuma_codegen::backend::{create_backend, AllocatedProgram, BackendKind};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{AstToScg, ModuleResolver, Parser};

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
            handles.push(s.spawn(move || chunk.iter().map(f_ref).collect::<Result<Vec<_>, _>>()));
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

fn compile_for_backend(
    source: &str,
    kind: BackendKind,
) -> Result<(Vec<u8>, Option<String>), String> {
    // VUMA 2.0: verification is MANDATORY — `verify=true` always.
    // `--safe` is OFF by default here (the diagnostic `diag` path does not
    // expose it); callers that need bounds-check IR must use
    // `compile_for_backend_with_path` directly. `allow_inconclusive=false`
    // matches the Gap 1 "full flip" default (Inconclusive is a hard error).
    compile_for_backend_with_path(source, kind, None, true, OptLevel::O3, false, false)
    // O3 always on
}

fn compile_for_backend_with_path(
    source: &str,
    kind: BackendKind,
    file_path: Option<&Path>,
    verify: bool,
    opt_level: OptLevel,
    safe: bool,
    allow_inconclusive: bool,
) -> Result<(Vec<u8>, Option<String>), String> {
    // Resolve imports if a file path is provided
    let ast = if let Some(path) = file_path {
        let mut resolver = ModuleResolver::new();
        match resolver.resolve_source(source, Some(path)) {
            Ok(program) => program,
            Err(errors) => {
                return Err(format!(
                    "import resolution: {} errors: {:?}",
                    errors.len(),
                    errors.first()
                ))
            }
        }
    } else {
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        if result.has_errors() {
            return Err(format!("parse: {} errors", result.errors.len()));
        }
        result.unwrap()
    };

    // (MT-only) Build the PMT layout registry up-front from
    // the AST's `Item::LayoutDef` items so the IVE's
    // `VerificationLevel::Pmt` can run the 3 state verifiers
    // (state_read / state_write / state_transform) with full layout info.
    // PMT is always on in VUMA 2.0 — there is no "Normal" mode and no
    // `--pmt` flag. If the program has no `layout` items, this returns an
    // empty map (cheap — no work to do).
    let pmt_layouts = Some(build_pmt_layout_specs(&ast));

    let mut scg = {
        let mut c = AstToScg::new();
        c.convert(&ast).map_err(|e| format!("scg: {}", e))?
    };

    // Run InterproceduralAllocFlow pass to connect factory-function
    // allocations to their callers' free() calls.
    use vuma_scg::SCGPass;
    let _ = vuma_scg::InterproceduralAllocFlow::new().run(&mut scg);

    // Run SCG-level O3 transforms BEFORE IVE verification and IR
    // building, mirroring `compile_with_path` Stage 7. In VUMA 2.0 O3 is
    // mandatory, so `run_scg_transforms` always runs the full O3 SCG pass
    // set (DCE, const-fold, CSE, inlining, LICM, strength reduction,
    // tail-call detection, dead-region elimination). The config
    // constructed here is also reused for `run_ir_pipeline` below so the
    // entire post-IR-build O3 pipeline uses one consistent config
    // (matching the production compile path).
    let o3_config = CompileConfig {
        target: if kind == BackendKind::Wasm32 {
            CompileTarget::Wasm32
        } else {
            CompileTarget::Linux
        },
        opt_level,
        verification_level: VerificationLevel::Normal,
        // IMPL-1-safe-mandatory: wire `--safe` through so runtime bounds-check
        // IR (`__oob_trap` traps) is actually emitted on this path. Previously
        // `runtime_bounds_checks` defaulted to `false` and the codegen SCG was
        // never mutated by `inject_bounds_check_ir`, making `--safe` a no-op
        // on `compile_dump` (the canonical `vuma build` / `vuma compile` path
        // in `pipeline.rs` was the only one that honored it).
        runtime_bounds_checks: safe,
        // Disable inlining so functions referenced via GetAddress
        // (function pointers) survive the O3 pipeline. Without this, simple
        // functions like `fn my_handler(x: u64) -> u64 { return x + 1; }`
        // get inlined into their callers, and GetAddress can't find them.
        inline_threshold: 0,
        ..Default::default()
    };
    let _ = run_scg_transforms(&mut scg, &o3_config);

    // (VUMA 2.0) IVE PMT verification — MANDATORY and a HARD gate.
    // There is no `// ive_skip` source marker anymore and the `--verify`
    // flag defaults to `true`, so verification ALWAYS runs. A `Fail`
    // verdict is a compile error (the tool exits non-zero, which the
    // gold-standard test driver records as `CE`); `Inconclusive` and
    // `NoChecks` are non-blocking (matching `compile_with_path`'s default
    // non-strict behaviour).
    //
    // IVE now verifies the O3-optimized SCG (run_scg_transforms
    // was already applied above at O3). The previous local O0 config +
    // redundant O0 run_scg_transforms call have been removed so there is
    // a single consistent SCG state. double-checks IVE
    // correctness on the optimized SCG.
    //
    // the IVE ALWAYS runs ONLY the 3 state verifiers
    // (state_read / state_write / state_transform) via
    // `VerificationLevel::Pmt`. Memory safety for PMT programs is a
    // type-checking property (layouts + state ops), not a verification
    // problem. The PmtLayoutSpec registry built above is attached to the
    // VerificationInput so the state verifiers have field offset/size
    // info. There is no `Normal` mode anymore — VUMA 2.0 is PMT-only.
    let mut ive_status: Option<String> = None;
    // VUMA 2.0 is PMT-only — every program uses VerificationLevel::Pmt.
    // PMT programs (with layouts/state_new) get the 3 state verifiers.
    // Legacy pointer programs never reach this point: pointer syntax
    // (`allocate`, `free`, `*ptr`, `&x`, `*T`) is now a hard parse error
    // (see src/parser/src/parser.rs `check_pointer_syntax`), so the
    // compile aborts before IVE verification runs.
    if verify {
        let mut ive_input = vuma_ive::verification::VerificationInput::from_scg(scg.clone());
        if let Some(layouts) = &pmt_layouts {
            ive_input = ive_input.with_pmt_layouts(layouts.clone());
        }
        // [Gap 8 full fix] Populate secret_vars from #[secret] attributes
        // so the constant-time verifier uses explicit tainting instead of
        // the unsound substring heuristic on labels/filenames.
        let secret_vars = collect_secret_vars(&ast);
        ive_input = ive_input.with_secret_vars(secret_vars);
        let level = vuma_ive::invariant_aggregator::VerificationLevel::Pmt;
        let aggregator =
            vuma_ive::invariant_aggregator::InvariantAggregator::new().with_level(level);
        let result = aggregator.verify_all(&ive_input);
        let verdict = format!("{:?}", result.overall);
        let summary = format!(
            "passed={} failed={} total={}",
            result.summary.passed, result.summary.failed, result.summary.total_checked
        );
        ive_status = Some(format!("{} {}", verdict, summary));
        eprintln!("IVE: {} {}", verdict, summary);
        // HARD gate: refuse to emit a binary for a PMT-violating program.
        if result.overall == vuma_ive::invariant_aggregator::OverallVerdict::Fail {
            return Err(format!(
                "PMT verification FAILED: {} invariant(s) violated",
                result.summary.failed
            ));
        }
        // (Gap 1 full flip, -e relaxation) Inconclusive is a HARD
        // failure by default in `compile_dump`. The new `--allow-inconclusive`
        // flag (-e) downgrades it to a stderr warning so users no
        // longer have to drop down to `vuma compile --allow-inconclusive`
        // (canonical pipeline, `pipeline.rs:5186`) to soft-pass with a logged
        // soundness waiver. `Fail` remains a hard error regardless of the
        // flag — only `Inconclusive` is affected.
        if result.overall == vuma_ive::invariant_aggregator::OverallVerdict::Inconclusive {
            let unverified = result
                .summary
                .total_checked
                .saturating_sub(result.summary.passed)
                .saturating_sub(result.summary.failed);
            if allow_inconclusive {
                eprintln!(
                    "warning: PMT verification returned Inconclusive: {} \
                     invariant(s) unverified (passed={}, total_checked={}). \
                     Emitted under --allow-inconclusive soundness waiver.",
                    unverified, result.summary.passed, result.summary.total_checked
                );
            } else {
                return Err(format!(
                    "PMT verification returned Inconclusive: {} invariant(s) unverified \
                     (passed={}, total_checked={}). Inconclusive is a HARD failure \
                     by default (Gap 1 full flip). Re-run with --allow-inconclusive \
                     to soft-pass with a logged soundness waiver, or via `vuma compile \
                     --allow-inconclusive` (canonical pipeline).",
                    unverified, result.summary.passed, result.summary.total_checked
                ));
            }
        }
    }

    // Use the unified direct AST→codegen bridge (same path as vuma build/emit/run).
    let mut codegen_scg = bridge_ast_to_codegen_scg(&ast);

    // IMPL-1-safe-mandatory: when `--safe` is set, mutate the codegen SCG to
    // insert `__oob_trap` bounds-check IR before every statically-bounded
    // array access. Mirrors the canonical `pipeline.rs:5430-5495` flow:
    //   1. `build_alloc_sizes` collects stack-allocation sizes (PMT state
    //      buffers + raw `allocate` arrays).
    //   2. `find_bounds_check_sites_with_bounds` is computed (kept for
    //      logging parity with the canonical path; the injection itself
    //      re-walks the SCG).
    //   3. `inject_bounds_check_ir` emits `__oob_trap` IR nodes.
    //
    // The IRBuilder below consumes the mutated SCG, so the trap nodes lower
    // to backend-specific `__oob_trap` stubs (exit 134) on all 19 backends.
    // Arena-allocated buffers and raw-pointer arithmetic are NOT bounded
    // (see `memory_safety.rs:427` doc comment) — they remain Stage 3
    // (SoftBound fat pointers) territory.
    if safe {
        let alloc_sizes = build_alloc_sizes(&codegen_scg);
        let _sites = vuma_codegen::memory_safety::find_bounds_check_sites_with_bounds(
            &codegen_scg,
            &alloc_sizes,
        );
        vuma_codegen::memory_safety::inject_bounds_check_ir(&mut codegen_scg, &alloc_sizes);
        // IMPL-1-liveness: tombstone UAF detection — mirrors the canonical
        // `pipeline.rs:5481` flow. `state_new` allocations are grown by +1
        // byte at AST→SCG bridge time to hold a LIVE/DEAD flag; this pass
        // injects a Load+Eq+If sequence before each SEQ access that traps
        // via `__uaf_trap` (exit 135) when the flag is 0 (DEAD).
        vuma_codegen::memory_safety::inject_liveness_check_ir(&mut codegen_scg, &alloc_sizes);
    }
    let ir_program = {
        let mut b = IRBuilder::new();
        b.build(&codegen_scg).map_err(|e| format!("ir: {}", e))?
    };

    for _ds in &ir_program.data_sections {}

    // Run the full post-IR-build O3 pipeline via the shared
    // `run_ir_pipeline` helper (same path as `compile_with_path`). This
    // runs lowering (monomorphize, closures, switches, tail-calls,
    // loop-normalize), bv_verify, the syscall allowlist,
    // Stage 8b codegen-opt with the REAL backend latency table, and Wave
    // 32 escape+effects/SROA. Previously compile_dump only ran
    // `run_optimizations` (IR-level opts, default latency table) — skipping
    // ~half the production O3 pipeline. A syscall-allowlist violation or
    // bv_verify abort (none currently fatal) becomes a compile error here
    // (test marked CE) — triaged in Waves 3-5.
    let mut timings: Vec<(String, u64)> = Vec::new();
    // (Wave 2 / Task 3) Pass `secret_vars` so the IR-level information-flow
    // verifier can label `#[secret]`-annotated vregs `Secret` and flag
    // real `Secret → Public` flows.
    let secret_vars = collect_secret_vars(&ast);
    let ir_program = run_ir_pipeline(ir_program, &o3_config, kind, &secret_vars, &mut timings)
        .map_err(|e| format!("ir_pipeline: {:?}", e))?;

    let backend = create_backend(kind).map_err(|e| format!("backend: {}", e))?;

    // Populate thread-local set of 64-bit-returning function names.
    {
        use std::collections::HashSet;
        let func_64bit: HashSet<String> = ir_program
            .functions
            .iter()
            .filter(|f| {
                f.result_types.iter().any(|t| {
                    matches!(
                        t,
                        vuma_codegen::ir::IRType::I64
                            | vuma_codegen::ir::IRType::U64
                            | vuma_codegen::ir::IRType::F64
                    )
                })
            })
            .map(|f| f.name.clone())
            .collect();
        vuma_codegen::backend::set_64bit_returns(&func_64bit);
    }

    // K10G-wasm32-newton: populate the global function-name → param-IRTypes
    // map so the wasm32 backend's IRInstr::Call handler can push each call
    // argument with the correct wasm type (F64 vs I32). Without this,
    // `newton_sqrt(n_f, n_f, 8)` pushes all three args as I32, failing
    // wasmtime validation ("expected f64, found i32").
    {
        let func_params: std::collections::HashMap<String, Vec<vuma_codegen::ir::IRType>> =
            ir_program
                .functions
                .iter()
                .map(|f| (f.name.clone(), f.param_types.clone()))
                .collect();
        vuma_codegen::backend::set_func_param_types(&func_params);
    }

    // Pre-register ALL function param types BEFORE the parallel
    // `allocate_registers` loop.  The arm32/armeb Call handler looks up
    // the callee's param types (to decide 32-vs-64-bit arg passing) from
    // a shared global map that is populated inside `allocate_registers`.
    // When allocation runs in parallel (par_collect_result below), a Call
    // in function A may be lowered before function B (the callee) has
    // registered its param types — the lookup misses and the Call handler
    // falls back to `vec![true; num_args]` (all-64-bit), corrupting the
    // ABI for 32-bit params (callee reads arg0's high word as arg1).
    // Symptoms: fn_chained_calls=3 (expect 15), test_sha_round=54 (expect 77).
    // Pre-registering eliminates the race for arm32/armeb.  Other backends
    // (riscv32 has the same pattern but is not in scope here) are unaffected.
    if matches!(kind, BackendKind::Arm32 | BackendKind::ArmEb) {
        vuma_codegen::arm32::preregister_param_types(&ir_program.functions);
    }

    // F2a (Task 7-a): Central pre-lowering float-op verification.
    //
    // Reject bitwise/shift/remainder ops (`And`/`Or`/`Xor`/`Shl`/`ShrL`/
    // `ShrA`/`Ror`/`Rol`/`SRem`/`URem`) on `F32`/`F64` operands BEFORE
    // the parallel `allocate_registers` loop runs, so all 19 backends
    // (including the 4 thin wrappers) benefit without per-backend
    // wiring.  The previous AArch64-only call site in
    // `AArch64Backend::allocate_registers` has been removed — this
    // central call subsumes it.  See `verify_program_float_ops` in
    // `codegen/src/backend.rs` for the full rationale.
    if let Err(errs) = vuma_codegen::backend::verify_program_float_ops(&ir_program) {
        return Err(format!(
            "pre-lowering float-op verification failed: {}",
            errs.join("; ")
        ));
    }

    // Parallel per-function codegen using std::thread::scope.
    let allocated: Vec<_> = par_collect_result(&ir_program.functions, |func| {
        backend.allocate_registers(func)
    })
    .map_err(|e| format!("regalloc: {}", e))?;
    let total_code: usize = allocated.iter().map(|f| f.code_size).sum();
    // Collect all ReadOnly data sections (string literals) into a
    // single rodata byte vector for the backend to emit in .rodata.
    let rodata_data: Vec<u8> = ir_program
        .data_sections
        .iter()
        .filter(|ds| ds.kind == vuma_codegen::ir::DataSectionKind::ReadOnly)
        .flat_map(|ds| ds.data.iter().copied())
        .collect();
    // Collect all function names from the AST so the backend can
    // distinguish function symbols (text) from data symbols (bss).
    // Functions removed by the O3 optimizer but still referenced via
    // GetAddress must not be classified as data symbols.
    let function_names: std::collections::HashSet<String> = ast
        .items
        .iter()
        .filter_map(|item| match item {
            vuma_parser::ast::Item::FnDef(fd) => Some(fd.name.clone()),
            vuma_parser::ast::Item::TransformDef(td) => Some(td.name.clone()),
            _ => None,
        })
        .collect();
    let program = AllocatedProgram {
        functions: allocated,
        total_code_size: total_code,
        total_data_size: 0,
        rodata_data,
        function_names,
    };
    let binary = backend
        .encode_program(&program)
        .map_err(|e| format!("encode: {}", e))?;
    Ok((binary, ive_status))
}

fn execute_binary(
    binary: &[u8],
    qemu: Option<&str>,
    timeout_secs: u64,
) -> (i32, Vec<u8>, Vec<u8>, bool) {
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
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                let signal = o.status.signal();
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                let crashed = stderr.contains("Segmentation fault")
                    || stderr.contains("uncaught target signal")
                    || code == 139
                    || code == 134
                    || signal.is_some();
                (code, o.stdout, o.stderr, crashed)
            }
            #[cfg(not(unix))]
            {
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                let crashed = stderr.contains("Segmentation fault") || code == 139 || code == 134;
                (code, o.stdout, o.stderr, crashed)
            }
        }
        Err(_) => (-1, vec![], vec![], true),
    }
}

fn run_diag(backend_name: &str, examples_dir: &str, qemu: Option<&str>) {
    let kind = match backend_from_name(backend_name) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(2);
        }
    };
    let mut examples: Vec<String> = fs::read_dir(examples_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".vuma"))
        .collect();
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
            Err(e) => {
                compile_fail.push((ex.clone(), e));
                continue;
            }
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
    for (n, e) in &compile_fail {
        println!("  X {} : {}", n, e);
    }
    println!("Crashes ({}):", crash.len());
    for (n, c, e) in &crash {
        println!("  CRASH {} (code={}): {}", n, c, e);
    }
    println!("Timeouts ({}):", timeout.len());
    for (n, c) in &timeout {
        println!("  TIMEOUT {} (code={})", n, c);
    }
    println!("Exec fail ({}):", exec_fail.len());
    for (n, c) in &exec_fail {
        println!("  FAIL {} (code={})", n, c);
    }
    println!("Pass ({}):", pass.len());
    for (n, c, _) in &pass {
        println!("  OK {} (code={})", n, c);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "diag" {
        let backend = if args.len() > 2 {
            args[2].as_str()
        } else {
            "mips64"
        };
        let examples_dir = if args.len() > 3 {
            args[3].as_str()
        } else {
            "/tmp/my-project/examples"
        };
        let qemu: Option<&str> = if args.len() > 4 {
            Some(args[4].as_str())
        } else {
            None
        };
        run_diag(backend, examples_dir, qemu);
        return;
    }
    // Parse flags:
    //   --safe     accepted for backwards compatibility (no-op — runtime
    //              bounds-check IR emission is ALWAYS ON in VUMA 2.0;
    //              `__oob_trap` traps are injected before every
    //              statically-bounded array access). The flag is accepted
    //              but ignored, mirroring `--opt-level=O3`.
    //   --opt-level=O3   accepted for backwards compatibility (no-op —
    //              O3 is hardcoded and mandatory; any other value is
    //              rejected).
    //
    // PMT is always on in VUMA 2.0 (PMT-only mode). Pointer syntax
    // (`allocate`, `free`, `*ptr`, `&x`, `*T`) is always a hard parse
    // error at the parser level — no flag required. The IVE always uses
    // `VerificationLevel::Pmt` and the PMT layout registry is always
    // built (cheap — empty map if no `layout` items). There is no
    // `// ive_skip` source marker. IVE verification CAN now be skipped
    // via the `--no-verify` flag (see below) — added in -e as a
    // debugging escape hatch; not recommended for production builds.
    //
    // IVE verification flags (-e):
    //   --verify              explicitly enable IVE (the default; accepted
    //                          for backwards compat — previously a no-op
    //                          that silently fell through to the positional
    //                          vector and was mis-parsed as a backend name).
    //   --no-verify skip IVE entirely. NEW in -e — intended
    //                          for debugging the codegen pipeline when IVE
    //                          is known to fail spuriously. NOT recommended
    //                          for production builds: PMT soundness is
    //                          unverified when this is set.
    //   --allow-inconclusive  downgrade IVE `Inconclusive` verdict from a
    //                          HARD compile error to a stderr warning.
    //                          Parity with `vuma compile --allow-inconclusive`
    //                          (canonical pipeline, `pipeline.rs:5186`).
    //                          `Fail` remains a hard error regardless.
    //
    // Default behaviour is unchanged: `verify = true` and
    // `allow_inconclusive = false` (matching the previous Gap 1 "full flip"
    // where Inconclusive was a hard error).
    let mut verify = true;
    let mut allow_inconclusive = false;
    // IMPL-1-safe-mandatory: `--safe` is now ALWAYS ON. The flag is
    // accepted for backwards compatibility but is a pure no-op (mirrors
    // `--opt-level=O3`). Bounds-check + liveness-check IR injection is
    // unconditionally emitted below.
    let safe = true;
    // O3 is **mandatory** in VUMA 2.0 — the `--opt-level` flag is now a
    // pure no-op (the parser only validates that the value is `O3` and
    // otherwise ignores it; the pipeline always uses `OptLevel::O3`).
    let opt_level = OptLevel::O3;
    let positional: Vec<String> = args
        .iter()
        .skip(1)
        .filter(|a| {
            if *a == "--safe" {
                // No-op: `safe` is always `true`. Accepted for backwards compat.
                false
            } else if *a == "--verify" {
                // Explicit on-switch (default is already `true`). Accepted
                // for backwards compat and to make the IVE gate visible on
                // the command line.
                verify = true;
                false
            } else if *a == "--no-verify" {
                // NEW (-e): skip IVE entirely. Debugging escape hatch.
                verify = false;
                eprintln!(
                    "warning: --no-verify skips IVE PMT verification; \
                 output binary is NOT soundness-checked"
                );
                false
            } else if *a == "--allow-inconclusive" {
                // NEW (-e): downgrade Inconclusive from hard error to
                // warning. Parity with `vuma compile --allow-inconclusive`.
                allow_inconclusive = true;
                false
            } else if let Some(val) = a.strip_prefix("--opt-level=") {
                if val != "O3" {
                    eprintln!(
                        "error: invalid opt-level '{}'; VUMA 2.0 mandates O3 \
                     (only O3 is accepted; O0/O1/O2 are not supported)",
                        val
                    );
                    std::process::exit(1);
                }
                false
            } else {
                true
            }
        })
        .cloned()
        .collect();
    if positional.len() < 2 {
        eprintln!("Usage: compile_dump <source.vuma> <output.bin> [backend] [--opt-level=O3] [--safe (always on)] [--verify] [--no-verify] [--allow-inconclusive]");
        std::process::exit(1);
    }
    let path = &positional[0];
    let out_path = &positional[1];
    let backend_name = if positional.len() > 2 {
        positional[2].as_str()
    } else {
        "aarch64"
    };
    let kind = backend_from_name(backend_name).unwrap_or(BackendKind::AArch64);
    let source = std::fs::read_to_string(path).unwrap();
    let file_path = std::path::Path::new(path);
    let (binary, _ive_status) = match compile_for_backend_with_path(
        &source,
        kind,
        Some(file_path),
        verify,
        opt_level,
        safe,
        allow_inconclusive,
    ) {
        Ok(v) => v,
        Err(e) => {
            //  (PMT-only): print a clean compile-error message to
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
