//! # fuzz_driver — integrated fuzzer driver for VUMA
//!
//! Wave 9 / Task 9-d. Combines the three fuzzer modules built in earlier
//! waves into a single tool that:
//!
//! 1. Generates a random VUMA program using one of:
//!    - `vuma_fuzzer` (Module 1: type system + arithmetic expressions)
//!    - `fuzzer_stmt_gen` (Module 2: statements / control flow / calls)
//!    - `fuzzer_mem_gen` (Module 3: memory ops + multi-function programs)
//! 2. Compiles each program on all 7 native backends
//!    (x86_64, aarch64, riscv64, arm32, mips64el, ppc64, loongarch64).
//! 3. Runs each binary and captures exit code + stdout + crash signal.
//! 4. Reports compile failures, crashes, timeouts, and differential
//!    disagreements (any backend producing a different exit code / stdout).
//! 5. Honours `--count N` (default 100) and `--seed S` (default 42).
//!
//! Usage:
//!     ./fuzz_driver [--count N] [--seed S] [--dump]
//!
//! Determinism: a given `--seed S` always produces the same sequence of
//! programs because each program `i` derives its sub-RNG state from a
//! single seeded `StdRng` that is advanced deterministically between
//! iterations. Reproducing a failing program is therefore as simple as
//! re-running `fuzz_driver --seed S --count N` and inspecting program
//! number `i` in the report (the offending source is also printed to
//! stderr inline).

#![allow(dead_code)] // submodules carry their own unused-but-intentional items

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::fs;
use std::process::Command;

use vuma::pipeline::{
    bridge_scg_to_codegen, run_scg_transforms, CompileConfig, CompileTarget, OptLevel,
    VerificationLevel,
};
use vuma_codegen::backend::{create_backend, AllocatedProgram, BackendKind};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{AstToScg, Parser};

// ===========================================================================
// Submodule imports — pull the already-written fuzzer modules in as
// `#[path]` mods. Their `fn main()` functions become regular private
// functions inside the submodule (not entry points), which is fine.
// ===========================================================================

#[path = "fuzzer_stmt_gen.rs"]
mod fuzzer_stmt_gen;

#[path = "fuzzer_mem_gen.rs"]
mod fuzzer_mem_gen;

// ===========================================================================
// Module 1 — type system + expression generator.
//
// The original `src/bin/vuma_fuzzer.rs` (Wave 2 / Task 2-a) keeps its
// items private, so we cannot `#[path]`-import it. Instead, we copy the
// Module 1 surface (VumaType + ExprGen + generate_program) verbatim into
// a private submodule here. This is the smallest self-contained copy
// that exposes `generate_program(rng)` for the driver to call.
// ===========================================================================

mod vuma_expr_fuzzer {
    //! Copied from `src/bin/vuma_fuzzer.rs` (Task 2-a). Generates a single
    //! `fn main() -> i32` that declares a few integer variables and
    //! computes arithmetic / boolean expressions over them before
    //! returning. All expressions are total (no DBZ, no UB shifts).

    use rand::rngs::StdRng;
    use rand::Rng;
    use std::fmt;

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    pub enum VumaType {
        U8,
        U16,
        U32,
        U64,
        I32,
        I64,
        Address,
        Bool,
    }

    impl VumaType {
        fn size(&self) -> u64 {
            match self {
                VumaType::U8 | VumaType::Bool => 1,
                VumaType::U16 => 2,
                VumaType::U32 | VumaType::I32 => 4,
                VumaType::U64 | VumaType::I64 | VumaType::Address => 8,
            }
        }
        fn is_unsigned(&self) -> bool {
            matches!(self, VumaType::U8 | VumaType::U16 | VumaType::U32 | VumaType::U64)
        }
        fn is_signed(&self) -> bool {
            matches!(self, VumaType::I32 | VumaType::I64)
        }
        fn is_integer(&self) -> bool {
            self.is_unsigned() || self.is_signed()
        }
        fn bit_width(&self) -> u32 {
            match self {
                VumaType::U8 => 8,
                VumaType::U16 => 16,
                VumaType::U32 | VumaType::I32 => 32,
                VumaType::U64 | VumaType::I64 | VumaType::Address => 64,
                VumaType::Bool => 0,
            }
        }
        fn random(rng: &mut impl Rng) -> Self {
            match rng.gen_range(0..8u32) {
                0 => VumaType::U8,
                1 => VumaType::U16,
                2 => VumaType::U32,
                3 => VumaType::U64,
                4 => VumaType::I32,
                5 => VumaType::I64,
                6 => VumaType::Address,
                _ => VumaType::Bool,
            }
        }
        fn random_integer(rng: &mut impl Rng) -> Self {
            match rng.gen_range(0..6u32) {
                0 => VumaType::U8,
                1 => VumaType::U16,
                2 => VumaType::U32,
                3 => VumaType::U64,
                4 => VumaType::I32,
                _ => VumaType::I64,
            }
        }
        fn to_vuma_str(&self) -> &'static str {
            match self {
                VumaType::U8 => "u8",
                VumaType::U16 => "u16",
                VumaType::U32 => "u32",
                VumaType::U64 => "u64",
                VumaType::I32 => "i32",
                VumaType::I64 => "i64",
                VumaType::Address => "Address",
                VumaType::Bool => "bool",
            }
        }
        #[allow(dead_code)]
        fn _silence_size(&self) -> u64 {
            self.size()
        }
    }

    impl fmt::Display for VumaType {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.to_vuma_str())
        }
    }

    /// Random VUMA expression generator with a typed variable scope.
    /// See `src/bin/vuma_fuzzer.rs` for the full design notes.
    pub struct ExprGen {
        rng: StdRng,
        scope: Vec<(String, VumaType)>,
        max_depth: u32,
        var_counter: u32,
    }

    impl ExprGen {
        pub fn new(rng: StdRng, max_depth: u32) -> Self {
            Self { rng, scope: Vec::new(), max_depth, var_counter: 0 }
        }
        fn fresh_name(&mut self) -> String {
            let name = format!("v{}", self.var_counter);
            self.var_counter += 1;
            name
        }
        fn declare(&mut self, name: String, ty: VumaType) {
            self.scope.push((name, ty));
        }
        fn vars_of_type(&self, ty: &VumaType) -> Vec<String> {
            self.scope
                .iter()
                .filter(|(_, t)| t == ty)
                .map(|(n, _)| n.clone())
                .collect()
        }
        pub fn gen_expr(&mut self, ty: VumaType, depth: u32) -> String {
            let leaf_p = if depth == 0 {
                1.0
            } else {
                let base = 0.30;
                let depth_bonus = (depth as f64) * 0.05;
                (base + depth_bonus).min(0.6)
            };
            if self.rng.gen_bool(leaf_p) {
                return self.gen_leaf(&ty);
            }
            match ty {
                VumaType::Bool => self.gen_bool_op(depth),
                VumaType::Address => self.gen_leaf(&ty),
                t @ (VumaType::U8
                | VumaType::U16
                | VumaType::U32
                | VumaType::U64
                | VumaType::I32
                | VumaType::I64) => self.gen_arith(&t, depth),
            }
        }
        fn gen_leaf(&mut self, ty: &VumaType) -> String {
            let vars = self.vars_of_type(ty);
            if !vars.is_empty() && self.rng.gen_bool(0.65) {
                let idx = self.rng.gen_range(0..vars.len());
                return vars[idx].clone();
            }
            if matches!(ty, VumaType::Address) {
                if let Some(name) = vars.first() {
                    return name.clone();
                }
                return "0".to_string();
            }
            self.gen_literal(ty.clone())
        }
        fn gen_literal(&mut self, ty: VumaType) -> String {
            match ty {
                VumaType::Bool => {
                    if self.rng.gen_bool(0.5) {
                        "true".to_string()
                    } else {
                        "false".to_string()
                    }
                }
                VumaType::Address => "0".to_string(),
                VumaType::U8 => self.rng.gen_range(0u32..=255).to_string(),
                VumaType::U16 => self.rng.gen_range(0u32..=65535).to_string(),
                VumaType::U32 => self.rng.gen_range(0u64..=4294967295).to_string(),
                VumaType::U64 => self.rng.gen_range(0i64..=i64::MAX).to_string(),
                VumaType::I32 => self.rng.gen_range(0i64..=i32::MAX as i64).to_string(),
                VumaType::I64 => self.rng.gen_range(0i64..=i64::MAX).to_string(),
            }
        }
        fn gen_arith(&mut self, ty: &VumaType, depth: u32) -> String {
            let ops: &[&str] = if ty.is_unsigned() {
                &["+", "-", "*", "/", "%", "&", "|", "^", "<<", ">>"]
            } else {
                &["+", "-", "*", "&", "|", "^", "<<", ">>"]
            };
            let op = ops[self.rng.gen_range(0..ops.len())];
            let lhs = self.gen_expr(ty.clone(), depth.saturating_sub(1));
            match op {
                "/" | "%" => {
                    let divisor = self.rng.gen_range(1..=16);
                    format!("({} {} {})", lhs, op, divisor)
                }
                "<<" | ">>" => {
                    let bw = ty.bit_width();
                    let max_shift = if bw > 0 { bw - 1 } else { 0 };
                    let shamt = if max_shift > 0 {
                        self.rng.gen_range(0..=max_shift)
                    } else {
                        0
                    };
                    format!("({} {} {})", lhs, op, shamt)
                }
                _ => {
                    let rhs = self.gen_expr(ty.clone(), depth.saturating_sub(1));
                    format!("({} {} {})", lhs, op, rhs)
                }
            }
        }
        fn gen_bool_op(&mut self, depth: u32) -> String {
            match self.rng.gen_range(0..3u32) {
                0 => self.gen_comparison(depth),
                1 => self.gen_logical(depth),
                _ => self.gen_leaf(&VumaType::Bool),
            }
        }
        fn gen_comparison(&mut self, depth: u32) -> String {
            let ty = VumaType::random_integer(&mut self.rng);
            let ops = ["<", ">", "<=", ">=", "==", "!="];
            let op = ops[self.rng.gen_range(0..ops.len())];
            let lhs = self.gen_expr(ty.clone(), depth.saturating_sub(1));
            let rhs = self.gen_expr(ty, depth.saturating_sub(1));
            format!("({} {} {})", lhs, op, rhs)
        }
        fn gen_logical(&mut self, depth: u32) -> String {
            let op = if self.rng.gen_bool(0.5) { "&&" } else { "||" };
            let lhs = self.gen_expr(VumaType::Bool, depth.saturating_sub(1));
            let rhs = self.gen_expr(VumaType::Bool, depth.saturating_sub(1));
            format!("({} {} {})", lhs, op, rhs)
        }
    }

    /// Generate one Module-1 program: `fn main() -> i32 { ... }` with
    /// several integer / boolean variables and a final `return <var>;`.
    pub fn generate_program(rng: &mut StdRng) -> String {
        let mut gen = ExprGen::new(rng.clone(), 3);
        let mut lines: Vec<String> = Vec::new();

        lines.push("// Auto-generated by fuzz_driver (Module 1: type system + expr gen)".into());
        lines.push("fn main() -> i32 {".into());

        let num_seeds = gen.rng.gen_range(2..=4);
        for _ in 0..num_seeds {
            let ty = VumaType::random_integer(&mut gen.rng);
            let name = gen.fresh_name();
            let val = gen.gen_literal(ty.clone());
            gen.declare(name.clone(), ty.clone());
            lines.push(format!("    {}: {} = {};", name, ty.to_vuma_str(), val));
        }

        let num_derived = gen.rng.gen_range(3..=6);
        for _ in 0..num_derived {
            let ty = if gen.rng.gen_bool(0.8) {
                VumaType::random_integer(&mut gen.rng)
            } else {
                VumaType::Bool
            };
            let name = gen.fresh_name();
            let depth = gen.rng.gen_range(1..=gen.max_depth);
            let expr = gen.gen_expr(ty.clone(), depth);
            gen.declare(name.clone(), ty.clone());
            lines.push(format!("    {}: {} = {};", name, ty.to_vuma_str(), expr));
        }

        let int_vars: Vec<String> = gen
            .scope
            .iter()
            .filter(|(_, t)| t.is_integer())
            .map(|(n, _)| n.clone())
            .collect();
        let ret_expr = if !int_vars.is_empty() {
            let idx = gen.rng.gen_range(0..int_vars.len());
            int_vars[idx].clone()
        } else {
            "0".to_string()
        };
        lines.push(format!("    return {};", ret_expr));
        lines.push("}".into());

        lines.join("\n")
    }
}

// ===========================================================================
// Backend specs + compile / run helpers.
//
// Copied from `src/bin/differential_test.rs` (Task 9 / 9-a). QEMU
// binaries live under `/tmp/qemu_bins/`. x86_64 runs natively.
// ===========================================================================

const QEMU_ARM: &str = "/tmp/qemu_bins/qemu-arm";
const QEMU_AARCH64: &str = "/tmp/qemu_bins/qemu-aarch64";
const QEMU_RISCV64: &str = "/tmp/qemu_bins/qemu-riscv64";
const QEMU_PPC64: &str = "/tmp/qemu_bins/qemu-ppc64";
const QEMU_LOONGARCH64: &str = "/tmp/qemu_bins/qemu-loongarch64";
const QEMU_MIPS64EL: &str = "/tmp/qemu_bins/qemu-mips64el";

/// One backend we will fuzz against.
struct BackendSpec {
    name: &'static str,
    kind: BackendKind,
    /// Absolute path to QEMU binary, or `None` to run natively (x86_64).
    qemu: Option<&'static str>,
}

fn backends() -> Vec<BackendSpec> {
    vec![
        BackendSpec { name: "x86_64",      kind: BackendKind::X86_64,      qemu: None },
        BackendSpec { name: "aarch64",     kind: BackendKind::AArch64,     qemu: Some(QEMU_AARCH64) },
        BackendSpec { name: "riscv64",     kind: BackendKind::RiscV64,     qemu: Some(QEMU_RISCV64) },
        BackendSpec { name: "arm32",       kind: BackendKind::Arm32,       qemu: Some(QEMU_ARM) },
        BackendSpec { name: "mips64el",    kind: BackendKind::Mips64,      qemu: Some(QEMU_MIPS64EL) },
        BackendSpec { name: "ppc64",       kind: BackendKind::PowerPC64,   qemu: Some(QEMU_PPC64) },
        BackendSpec { name: "loongarch64", kind: BackendKind::LoongArch64, qemu: Some(QEMU_LOONGARCH64) },
    ]
}

/// Per-(program, backend) compile outcome.
enum CompileOutcome {
    Ok(Vec<u8>),
    Fail(String),
}

/// Per-(program, backend) execution outcome.
#[derive(Clone)]
enum ExecOutcome {
    Done { code: i32, stdout: Vec<u8> },
    Crash { code: i32, signal: Option<i32>, detail: String },
    Timeout,
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
    let bin_path = std::env::temp_dir().join(format!(
        "vuma_fuzzdrv_{}_{}.bin",
        tag,
        std::process::id()
    ));
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

// ===========================================================================
// Program generators — wrap each fuzzer module behind a uniform
// `generate(seed) -> String` interface so the driver can pick one at
// random without caring about the underlying API shape.
// ===========================================================================

/// Generate a Module-1 program (expression-only `fn main`).
fn gen_module1(seed: u64) -> String {
    let mut rng = StdRng::seed_from_u64(seed);
    vuma_expr_fuzzer::generate_program(&mut rng)
}

/// Generate a Module-2 program (statements / control flow / function
/// calls). Reuses `fuzzer_stmt_gen::StmtGen` and the matching
/// `HELPERS` prelude verbatim from the original task 2-b binary.
fn gen_module2(seed: u64) -> String {
    use fuzzer_stmt_gen::{StmtGen, VumaType};
    let mut gen = StmtGen::new(seed);
    gen.set_max_depth(3);

    // Register the same helper functions defined in `HELPERS` so that
    // `gen_call` knows their signatures.
    gen.register_function("fuzz_add_u32", vec![VumaType::U32, VumaType::U32], VumaType::U32);
    gen.register_function("fuzz_mul_u64", vec![VumaType::U64, VumaType::U64], VumaType::U64);
    gen.register_function("fuzz_is_nonzero", vec![VumaType::U32], VumaType::Bool);
    gen.register_function("fuzz_id_u64", vec![VumaType::U64], VumaType::U64);
    gen.register_function("fuzz_neg_i32", vec![VumaType::I32], VumaType::I32);
    gen.register_function("fuzz_noop", vec![], VumaType::Void);
    gen.register_function(
        "fuzz_first_u64",
        vec![VumaType::U64, VumaType::U64],
        VumaType::U64,
    );

    gen.set_return_type(VumaType::I32);
    let body = gen.gen_block(10, 0);

    // Guarantee a final `return` at the end of `main` — if the body's
    // last line was already a `return`, skip this to avoid unreachable
    // code.
    let trimmed = body.trim_end();
    let last_line = trimmed.lines().last().unwrap_or("");
    let needs_final_return = !last_line.trim_start().starts_with("return");
    let final_return = if needs_final_return {
        "    return 0;\n".to_string()
    } else {
        String::new()
    };

    format!(
        "// Auto-generated by fuzz_driver (Module 2: stmt gen, seed={})\n\
         {}\n\
         fn main() -> i32 {{\n\
         {}{}\n\
         }}\n",
        seed,
        fuzzer_stmt_gen::HELPERS,
        body,
        final_return
    )
}

/// Generate a Module-3 program (memory ops + multi-function).
fn gen_module3(seed: u64) -> String {
    let mut gen = fuzzer_mem_gen::MemFuncGen::new(seed);
    gen.gen_program()
}

/// Pick one of the three generators at random, using `rng` to choose.
/// Returns the chosen module index alongside the program text so the
/// driver can report which generator produced a failing program.
fn generate_random_program(rng: &mut StdRng) -> (u8, String) {
    // Derive a stable sub-seed for the chosen generator so that each
    // program's source is reproducible from the top-level (seed, i).
    let sub_seed: u64 = rng.gen();
    let choice: u8 = rng.gen_range(0..3);
    let program = match choice {
        0 => gen_module1(sub_seed),
        1 => gen_module2(sub_seed),
        _ => gen_module3(sub_seed),
    };
    (choice, program)
}

// ===========================================================================
// Per-program driver — compile on every backend, run, capture results.
// ===========================================================================

struct BackendResult {
    name: String,
    compile: Result<(), String>,
    exec: Option<ExecOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
    if exit_codes.is_empty() {
        return ProgramCategory::Crash;
    }
    let all_same_code = exit_codes.iter().all(|c| *c == exit_codes[0]);
    let all_same_stdout = stdouts.iter().all(|s| s == &stdouts[0]);
    if all_same_code && all_same_stdout {
        ProgramCategory::Pass
    } else {
        ProgramCategory::DiffFailure
    }
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

/// Compile + run a single program on every backend. Returns the
/// per-backend result vector.
fn run_one_program(program: &str, backends: &[BackendSpec], timeout_secs: u64) -> Vec<BackendResult> {
    let mut results: Vec<BackendResult> = Vec::with_capacity(backends.len());
    for b in backends {
        let compiled = compile_for_backend(program, b.kind);
        let exec = match &compiled {
            CompileOutcome::Ok(bytes) => Some(execute_binary(bytes, b.qemu, timeout_secs, b.name)),
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
    results
}

// ===========================================================================
// CLI parsing + main loop
// ===========================================================================

fn parse_flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let count: usize = parse_flag(&args, "--count")
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);
    let seed: u64 = parse_flag(&args, "--seed")
        .and_then(|s| s.parse().ok())
        .unwrap_or(42);
    // Optional: dump each generated program to /tmp for debugging.
    let dump_programs: bool = args.iter().any(|a| a == "--dump");

    let mut rng = StdRng::seed_from_u64(seed);
    let backends = backends();

    eprintln!(
        "fuzz_driver: count={} seed={} backends={} ({} total runs)",
        count,
        seed,
        backends.len(),
        count.saturating_mul(backends.len())
    );

    let mut compile_fails = 0usize;
    let mut crashes = 0usize;
    let mut timeouts = 0usize;
    let mut differential_fails = 0usize;
    let mut passes = 0usize;

    // Detailed per-program records for the final report.
    struct ProgReport {
        idx: usize,
        module: u8,
        category: ProgramCategory,
        results: Vec<BackendResult>,
        #[allow(dead_code)]
        program: String,
    }
    let mut reports: Vec<ProgReport> = Vec::new();

    for i in 0..count {
        let (module, program) = generate_random_program(&mut rng);

        if dump_programs {
            let p = format!("/tmp/fuzz_driver_p{:04}_m{}.vuma", i, module);
            let _ = fs::write(&p, &program);
        }

        let results = run_one_program(&program, &backends, 3);
        let cat = categorise(&results);

        match cat {
            ProgramCategory::Pass => passes += 1,
            ProgramCategory::DiffFailure => differential_fails += 1,
            ProgramCategory::Crash => crashes += 1,
            ProgramCategory::Timeout => timeouts += 1,
            ProgramCategory::CompileFailAll | ProgramCategory::CompileFailSome => compile_fails += 1,
        }

        eprintln!("[{}/{}] module={} : {:?}", i + 1, count, module, cat);

        // For non-pass outcomes, print per-backend details immediately so
        // the user can correlate failures with the offending program.
        if cat != ProgramCategory::Pass {
            for r in &results {
                match (&r.compile, &r.exec) {
                    (Err(e), _) => {
                        eprintln!("    {} : COMPILE FAIL: {}", r.name, e);
                    }
                    (Ok(()), Some(ExecOutcome::Timeout)) => {
                        eprintln!("    {} : TIMEOUT", r.name);
                    }
                    (Ok(()), Some(ExecOutcome::Crash { code, signal, detail })) => {
                        eprintln!(
                            "    {} : CRASH (code={}, signal={:?}) {}",
                            r.name, code, signal, detail
                        );
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
            // Print the offending program source so the failure is
            // directly reproducible.
            eprintln!("    --- program source (module {}) ---", module);
            for line in program.lines() {
                eprintln!("    | {}", line);
            }
        }

        reports.push(ProgReport {
            idx: i,
            module,
            category: cat,
            results,
            program,
        });
    }

    // ---- Final report ----
    println!("\n========== FUZZ DRIVER REPORT ==========");
    println!("Programs generated : {}", count);
    println!("Seed               : {}", seed);
    println!(
        "Backends           : {}",
        backends.iter().map(|b| b.name).collect::<Vec<_>>().join(", ")
    );
    println!("Total runs         : {}", count.saturating_mul(backends.len()));
    println!();
    println!("  Pass                  : {:>4}", passes);
    println!("  Compile failures      : {:>4}", compile_fails);
    println!("  Crashes               : {:>4}", crashes);
    println!("  Timeouts              : {:>4}", timeouts);
    println!("  Differential failures : {:>4}", differential_fails);
    println!();

    // ---- Failure breakdown by module ----
    let mut mod_breakdown: std::collections::BTreeMap<(u8, ProgramCategory), usize> =
        std::collections::BTreeMap::new();
    for r in &reports {
        *mod_breakdown.entry((r.module, r.category.clone())).or_insert(0) += 1;
    }
    println!("--- Per-module outcome breakdown ---");
    println!("  {:<8} {:<20} {:>6}", "module", "category", "count");
    for ((m, c), n) in &mod_breakdown {
        let mname = match m {
            0 => "M1-expr",
            1 => "M2-stmt",
            _ => "M3-mem",
        };
        println!("  {:<8} {:<20} {:>6}", mname, format!("{:?}", c), n);
    }

    // ---- List differential failures explicitly ----
    let diffs: Vec<&ProgReport> = reports
        .iter()
        .filter(|r| r.category == ProgramCategory::DiffFailure)
        .collect();
    if !diffs.is_empty() {
        println!("\n--- Differential failures ({} total) ---", diffs.len());
        for r in diffs.iter() {
            println!("  program #{:04} (module {}):", r.idx, r.module);
            for br in &r.results {
                match (&br.compile, &br.exec) {
                    (Ok(()), Some(ExecOutcome::Done { code, stdout })) => {
                        println!(
                            "    {:<14} : exit={} stdout=\"{}\"",
                            br.name,
                            code,
                            fmt_stdout(stdout)
                        );
                    }
                    (Ok(()), Some(ExecOutcome::Crash { code, signal, detail })) => {
                        println!(
                            "    {:<14} : CRASH code={} signal={:?} {}",
                            br.name, code, signal, detail
                        );
                    }
                    (Ok(()), Some(ExecOutcome::Timeout)) => {
                        println!("    {:<14} : TIMEOUT", br.name);
                    }
                    (Ok(()), Some(ExecOutcome::SpawnFailed(e))) => {
                        println!("    {:<14} : SPAWN FAIL: {}", br.name, e);
                    }
                    (Err(e), _) => {
                        println!("    {:<14} : COMPILE FAIL: {}", br.name, e);
                    }
                    (Ok(()), None) => {
                        println!("    {:<14} : (not run)", br.name);
                    }
                }
            }
        }
    }

    // ---- List crashes explicitly ----
    let crashes_list: Vec<&ProgReport> = reports
        .iter()
        .filter(|r| r.category == ProgramCategory::Crash)
        .collect();
    if !crashes_list.is_empty() {
        println!("\n--- Crashes ({} total) ---", crashes_list.len());
        for r in crashes_list.iter() {
            println!("  program #{:04} (module {}):", r.idx, r.module);
            for br in &r.results {
                if let (Ok(()), Some(ExecOutcome::Crash { code, signal, detail })) =
                    (&br.compile, &br.exec)
                {
                    println!(
                        "    {:<14} : CRASH code={} signal={:?} {}",
                        br.name, code, signal, detail
                    );
                }
            }
        }
    }

    // ---- Final headline number ----
    println!(
        "\n=== {} / {} programs have all backends agreeing ===",
        passes, count
    );

    // Exit non-zero if any failure was observed — useful for CI.
    let total_fails = compile_fails + crashes + timeouts + differential_fails;
    if total_fails > 0 {
        std::process::exit(1);
    }
}
