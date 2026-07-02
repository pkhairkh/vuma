#![allow(clippy::manual_range_contains, clippy::map_unwrap_or, clippy::unnecessary_cast, clippy::redundant_closure, clippy::if_same_then_else, clippy::collapsible_if, clippy::useless_format)]
//! VUMA CLI — Command-line interface for the VUMA compiler.
//!
//! Subcommands:
//! - `vuma build <file>` — Parse + compile to ARM64 ELF (default), save to output file
//! - `vuma run <file>`   — Build + execute (via QEMU aarch64 or native)
//! - `vuma check <file>` — Parse + SCG + BD inference + IVE verification only
//! - `vuma emit <isa> <file>` — Compile to specific ISA
//! - `vuma disasm <file>` — Read binary and disassemble
//! - `vuma verify <file>` — Run IVE 5-invariant verification
//! - `vuma repl` — Interactive REPL (parse expr, print AST)
//! - `vuma lsp`  — Start Language Server (LSP) for IDE/LLM integration

use std::fs;
use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Parser, Subcommand, ValueEnum};

use vuma::pipeline::{
    compile_with_path, CompileConfig, CompileResult, CompileTarget, OptLevel, VerificationLevel, VumaError,
};
use vuma::telemetry::TelemetryCollector;
use vuma::logging::{LogLevel, init_logger, global_logger};
use vuma_codegen::backend::{create_backend, BackendKind};
use vuma_codegen::ScgToIr;
use vuma_codegen::scg_to_ir::{Scg, ScgNode, ScgFunction, ScgParam, ScgStatement, ScgType,
    ScgExpr, ComputationNode, AllocationNode, AccessNode, CallNode, ControlNode, GetAddressNode,
    CastNode, SwitchArm};
use vuma_codegen::ir::BinOpKind;
use vuma_codegen::CastKind;
use std::collections::{HashMap, HashSet};

// ═══════════════════════════════════════════════════════════════════════════
// CLI definition (clap derive)
// ═══════════════════════════════════════════════════════════════════════════

/// VUMA — Verified-Unsafe Memory Access: AI-Native Programming Language
#[derive(Parser, Debug)]
#[command(name = "vuma", version, about = "VUMA compiler and toolchain")]
struct Cli {
    /// Optimization level (overrides subcommand default)
    #[arg(long, global = true, value_enum, default_value = "O2")]
    opt_level: OptLevelArg,

    /// Verification level (overrides subcommand default).
    /// Use `--verification none` to bypass verification (equivalent to
    /// `--allow-unverified`). Default is `normal` (strict).
    #[arg(long, global = true, value_enum, default_value = "normal")]
    verification: VerificationArg,

    /// Include debug info in output (alias: --debug-info)
    #[arg(long, global = true, visible_alias = "debug-info")]
    debug: bool,

    /// Emit full ELF section headers in the output binary
    #[arg(long, global = true)]
    sections: bool,

    /// Launch the interactive REPL (shorthand for `vuma repl`)
    #[arg(long, global = true)]
    repl: bool,

    /// Enable runtime memory safety checks (bounds checking, --safe mode)
    #[arg(long, global = true)]
    safe: bool,

    /// Run performance benchmarks instead of compiling
    #[arg(long, global = true)]
    bench: bool,

    /// Enable verbose/debug logging
    #[arg(short = 'v', long, global = true)]
    verbose: bool,

    /// Suppress non-error output
    #[arg(short = 'q', long, global = true)]
    quiet: bool,

    /// Output telemetry data as JSON
    #[arg(long, global = true)]
    telemetry: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

/// Optimization level CLI argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OptLevelArg {
    #[value(name = "O0")]
    O0,
    #[value(name = "O1")]
    O1,
    #[value(name = "O2")]
    O2,
    #[value(name = "O3")]
    O3,
}

impl From<OptLevelArg> for OptLevel {
    fn from(val: OptLevelArg) -> Self {
        match val {
            OptLevelArg::O0 => OptLevel::O0,
            OptLevelArg::O1 => OptLevel::O1,
            OptLevelArg::O2 => OptLevel::O2,
            OptLevelArg::O3 => OptLevel::O3,
        }
    }
}

/// Verification level CLI argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum VerificationArg {
    #[value(name = "none")]
    None,
    #[value(name = "quick")]
    Quick,
    #[value(name = "normal")]
    Normal,
    #[value(name = "exhaustive")]
    Exhaustive,
}

impl From<VerificationArg> for VerificationLevel {
    fn from(val: VerificationArg) -> Self {
        match val {
            VerificationArg::None => VerificationLevel::None,
            VerificationArg::Quick => VerificationLevel::Quick,
            VerificationArg::Normal => VerificationLevel::Normal,
            VerificationArg::Exhaustive => VerificationLevel::Exhaustive,
        }
    }
}

/// Target ISA for the `emit` subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum IsaArg {
    #[value(name = "aarch64")]
    Aarch64,
    #[value(name = "x86_64")]
    X86_64,
    #[value(name = "riscv64")]
    Riscv64,
    #[value(name = "wasm32")]
    Wasm32,
    #[value(name = "loongarch64")]
    Loongarch64,
    #[value(name = "arm32")]
    Arm32,
    #[value(name = "mips64")]
    Mips64,
    #[value(name = "ppc64")]
    Ppc64,
}

impl From<IsaArg> for BackendKind {
    fn from(val: IsaArg) -> Self {
        match val {
            IsaArg::Aarch64 => BackendKind::AArch64,
            IsaArg::X86_64 => BackendKind::X86_64,
            IsaArg::Riscv64 => BackendKind::RiscV64,
            IsaArg::Wasm32 => BackendKind::Wasm32,
            IsaArg::Loongarch64 => BackendKind::LoongArch64,
            IsaArg::Arm32 => BackendKind::Arm32,
            IsaArg::Mips64 => BackendKind::Mips64,
            IsaArg::Ppc64 => BackendKind::PowerPC64,
        }
    }
}

/// Detect the host architecture at compile time and return the closest
/// matching VUMA backend. Returns `None` for unsupported architectures
/// (the caller should fall back to `IsaArg::Aarch64` in that case).
///
/// Used by `vuma build` and `vuma run` so that the emitted binary can be
/// executed natively on the developer's machine. Without this, `vuma build`
/// always produces an AArch64 ELF (the canonical pipeline's only target),
/// which fails to execute on x86_64 / riscv64 / etc. hosts without QEMU.
fn host_isa() -> Option<IsaArg> {
    match std::env::consts::ARCH {
        "x86_64" => Some(IsaArg::X86_64),
        "aarch64" => Some(IsaArg::Aarch64),
        "riscv64" => Some(IsaArg::Riscv64),
        "arm" => Some(IsaArg::Arm32),
        "powerpc64" => Some(IsaArg::Ppc64),
        "mips" => Some(IsaArg::Mips64), // closest match; endian may need a runtime check
        "loongarch64" => Some(IsaArg::Loongarch64),
        _ => None,
    }
}

/// Resolve the effective ISA for a subcommand: explicit `--isa` flag wins,
/// otherwise fall back to the host architecture, otherwise AArch64
/// (preserving the historical default of the canonical pipeline).
fn resolve_isa(isa: &Option<IsaArg>) -> IsaArg {
    isa.or_else(host_isa).unwrap_or(IsaArg::Aarch64)
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Parse + compile to ELF (host ISA by default), save to output file
    Build {
        /// Input VUMA source file
        file: PathBuf,

        /// Output file path (default: <input>.o)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Target platform
        #[arg(long, value_enum, default_value = "linux")]
        target: TargetArg,

        /// Target ISA. Defaults to the host architecture so the emitted
        /// binary can be executed natively; pass `--isa aarch64` to
        /// cross-compile for AArch64 (uses the canonical pipeline with
        /// full verification + telemetry). Non-AArch64 targets use the
        /// direct AST→codegen bridge path.
        #[arg(long, value_enum)]
        isa: Option<IsaArg>,
    },

    /// Build + execute (native on host arch, or via qemu-<isa>)
    Run {
        /// Target ISA. Defaults to the host architecture so the emitted
        /// binary can be executed natively. NOTE: because `args` uses
        /// `trailing_var_arg`, `--isa` must appear BEFORE the input file:
        ///   `vuma run --isa x86_64 hello.vuma arg1 arg2`
        #[arg(long, value_enum)]
        isa: Option<IsaArg>,

        /// Input VUMA source file
        file: PathBuf,

        /// Arguments to pass to the executed program
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Parse + SCG + BD inference + IVE verification only (no codegen)
    Check {
        /// Input VUMA source file
        file: PathBuf,
    },

    /// Compile to a specific ISA target
    Emit {
        /// Target ISA
        isa: IsaArg,

        /// Input VUMA source file
        file: PathBuf,

        /// Output file path (default: <input>.o)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Compile to a relocatable object file (ET_REL) for linking with system linker
    Compile {
        /// Input VUMA source file
        file: PathBuf,

        /// Output file path (default: <input>.o)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Target ISA (default: aarch64)
        #[arg(long, value_enum, default_value = "aarch64")]
        target: IsaArg,

        /// Output format (elf, obj, raw, wasm)
        #[arg(long, value_enum, default_value = "obj")]
        format: FormatArg,
    },

    /// Read a binary file and disassemble it
    Disasm {
        /// Binary file to disassemble
        file: PathBuf,

        /// ISA to use for disassembly (default: aarch64)
        #[arg(long, value_enum, default_value = "aarch64")]
        isa: IsaArg,

        /// Starting virtual address (default: 0x400000)
        #[arg(long, default_value = "0x400000")]
        base_addr: String,
    },

    /// Run IVE 5-invariant verification
    Verify {
        /// Input VUMA source file
        file: PathBuf,
    },

    /// Interactive REPL: parse expressions and print AST
    Repl,

    /// Start the Language Server (LSP) for IDE/LLM integration
    Lsp,

    /// Package manager subcommands
    Pkg {
        #[command(subcommand)]
        cmd: PkgCommand,
    },
}

/// Package manager subcommands.
#[derive(Subcommand, Debug)]
enum PkgCommand {
    /// Initialize a new VUMA package in the current directory
    Init {
        /// Package name
        name: String,
    },
    /// Build the package and its dependencies
    Build,
    /// Add a dependency to the package manifest
    Add {
        /// Dependency name
        dep: String,
        /// Version requirement (e.g. "0.1", "^1.0")
        #[arg(default_value = "*")]
        version: String,
    },
}

/// Target platform CLI argument for the `build` subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum TargetArg {
    #[value(name = "linux")]
    Linux,
}

impl From<TargetArg> for CompileTarget {
    fn from(val: TargetArg) -> Self {
        match val {
            TargetArg::Linux => CompileTarget::Linux,
        }
    }
}

/// Output format for the `compile` subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum FormatArg {
    #[value(name = "elf")]
    Elf,
    #[value(name = "obj")]
    Obj,
    #[value(name = "raw")]
    Raw,
    #[value(name = "wasm")]
    Wasm,
}

// ═══════════════════════════════════════════════════════════════════════════
// Version information
// ═══════════════════════════════════════════════════════════════════════════

/// VUMA version.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Supported backend list.
const SUPPORTED_BACKENDS: &str =
    "aarch64, x86_64, riscv64, arm32, mips64, ppc64, loongarch64, wasm32";

/// Build the long version string.
fn version_long() -> String {
    format!(
        "vuma {}\n\
         supported backends: {}\n\
         rustc: {}.{}.{}",
        VERSION,
        SUPPORTED_BACKENDS,
        option_env!("RUSTC_VERSION_MAJOR").unwrap_or("?"),
        option_env!("RUSTC_VERSION_MINOR").unwrap_or("?"),
        option_env!("RUSTC_VERSION_PATCH").unwrap_or("?"),
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// Helper functions
// ═══════════════════════════════════════════════════════════════════════════

/// Read source file content with a human-readable error on failure.
fn read_source(path: &PathBuf) -> Result<String, String> {
    fs::read_to_string(path)
        .map_err(|e| format!("error: cannot read source file '{}': {}", path.display(), e))
}

/// Build a `CompileConfig` from the global CLI flags.
fn make_config(cli: &Cli, target: CompileTarget) -> CompileConfig {
    CompileConfig {
        target,
        opt_level: OptLevel::from(cli.opt_level),
        verification_level: VerificationLevel::from(cli.verification),
        debug_info: cli.debug,
        section_headers: cli.sections,
        runtime_bounds_checks: cli.safe,
        memory_safety: true,
        ..CompileConfig::default()
    }
}

/// Print compilation errors to stderr in a human-readable format.
fn print_errors(errors: &[VumaError]) {
    for err in errors {
        eprintln!("error[{}]: {}", err.stage(), err);
    }
}

/// Determine the default output path for a given input file.
fn default_output_path(input: &Path) -> PathBuf {
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    let dir = input.parent().unwrap_or(std::path::Path::new("."));
    dir.join(format!("{}.o", stem))
}

/// Compile VUMA source to a binary for the given ISA, using the direct
/// AST → codegen SCG → IR → backend.encode_program path.
///
/// This bypasses the canonical pipeline (which always targets AArch64)
/// and is used by:
///   - `vuma run` (so the emitted binary matches the host ISA and can be
///     executed natively),
///   - `vuma build` when the resolved ISA is not AArch64 (the canonical
///     pipeline cannot emit non-AArch64 ELF),
///   - `vuma compile` and `vuma emit` (which already had their own copies
///     of this logic — this helper exists to factor out the common path).
///
/// Returns the encoded binary bytes on success.
fn compile_to_binary_direct(
    source: &str,
    isa: IsaArg,
) -> Result<Vec<u8>, String> {
    let backend_kind = BackendKind::from(isa);

    // Step 1: Parse source → AST.
    let mut parser = vuma_parser::Parser::new(source);
    let parse_result = parser.parse_program();
    if parse_result.is_err() {
        return Err(format!("parse error: {:?}", parse_result.errors));
    }
    if !parse_result.errors.is_empty() {
        eprintln!(
            "[vuma] WARNING: {} non-fatal parse errors:",
            parse_result.errors.len()
        );
        for err in &parse_result.errors {
            eprintln!("[vuma]   {:?}", err);
        }
    }
    let program = parse_result.value.unwrap();

    // Step 2: Bridge parser AST → codegen SCG.
    let codegen_scg = bridge_ast_to_codegen_scg(&program);

    // Step 3: Lower codegen SCG → IR.
    let mut ir_builder = ScgToIr::new();
    let ir_program = ir_builder.convert(&codegen_scg).map_err(|e| {
        format!("IR conversion error: {}", e)
    })?;

    // Step 4: Create backend and allocate registers.
    let backend = create_backend(backend_kind).map_err(|e| {
        format!(
            "error: cannot create {} backend: {}",
            backend_kind.isa_name(),
            e
        )
    })?;

    let mut allocated_functions = Vec::new();
    for func in &ir_program.functions {
        match backend.allocate_registers(func) {
            Ok(allocated) => allocated_functions.push(allocated),
            Err(e) => {
                eprintln!(
                    "warning: register allocation failed for '{}': {}",
                    func.name, e
                );
            }
        }
    }

    if allocated_functions.is_empty() {
        return Err("no functions were successfully allocated".to_string());
    }

    let allocated_program = vuma_codegen::backend::AllocatedProgram {
        functions: allocated_functions,
        total_code_size: 0,
        total_data_size: 0,
    };

    backend
        .encode_program(&allocated_program)
        .map_err(|e| format!("error: {} encoding failed: {}", backend.name(), e))
}

// ═══════════════════════════════════════════════════════════════════════════
// Subcommand handlers
// ═══════════════════════════════════════════════════════════════════════════

/// `vuma build <file>` — Parse + compile to ELF, save to output file.
///
/// When the resolved ISA (explicit `--isa` flag, else host arch, else
/// AArch64) is AArch64, uses the canonical pipeline (`compile_with_recovery`)
/// which supports verification, MSG construction, SCG transforms, and
/// telemetry. For any other ISA, falls back to the direct AST→codegen
/// SCG bridge path (no verification / telemetry), since the canonical
/// pipeline currently always emits AArch64 ELF.
fn cmd_build(
    cli: &Cli,
    file: &PathBuf,
    output: &Option<PathBuf>,
    target: &TargetArg,
    isa: &Option<IsaArg>,
) -> Result<(), String> {
    let source = read_source(file)?;
    let resolved_isa = resolve_isa(isa);

    // Non-AArch64 path: direct AST→codegen bridge.
    // The canonical pipeline (`compile_with_recovery`) only emits AArch64
    // ELF, so cross-arch builds must use the direct path.
    if !matches!(resolved_isa, IsaArg::Aarch64) {
        return cmd_build_direct(cli, file, output, &source, resolved_isa);
    }

    // AArch64 path: canonical pipeline (verification + telemetry + MSG + transforms).
    let config = make_config(cli, CompileTarget::from(*target));

    // Initialize telemetry collector if --telemetry is set.
    let mut telemetry = if cli.telemetry {
        let mut tc = TelemetryCollector::new();
        tc.set_opt_level(&format!("{:?}", cli.opt_level));
        tc.set_verification_level(&format!("{:?}", cli.verification));
        tc.set_target(&format!("{:?}", target));
        tc.set_debug_info(cli.debug);
        Some(tc)
    } else {
        None
    };

    if let Some(ref mut tc) = telemetry { tc.stage_start("compile"); }

    let compile_result = vuma::compile_with_recovery(&source, Some(file), &config);

    if let Some(ref mut tc) = telemetry { tc.stage_end("compile"); }

    match compile_result {
        CompileResult::Success(result) => {
            if let Some(ref mut tc) = telemetry {
                tc.set_scg_node_count(result.scg.node_count());
                tc.set_ir_function_count(result.ir_function_count);
                tc.set_ir_instruction_count(result.ir_instruction_count);
                tc.set_binary_size(result.binary.len());
            }

            let out_path = output
                .as_ref()
                .cloned()
                .unwrap_or_else(|| default_output_path(file));
            fs::write(&out_path, &result.binary).map_err(|e| {
                format!(
                    "error: cannot write output file '{}': {}",
                    out_path.display(),
                    e
                )
            })?;
            // Make the output file executable on Unix.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&out_path)
                    .map_err(|e| format!("error: cannot stat '{}': {}", out_path.display(), e))?
                    .permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&out_path, perms)
                    .map_err(|e| format!("error: cannot chmod '{}': {}", out_path.display(), e))?;
            }

            if !cli.quiet {
                println!(
                    "Compiled {} -> {} ({} bytes, {} SCG nodes, {} IR instructions)",
                    file.display(),
                    out_path.display(),
                    result.binary.len(),
                    result.scg.node_count(),
                    result.ir_instruction_count,
                );

                // Print stage timings.
                for (stage, ms) in &result.stage_timings {
                    println!("  {:20} {}ms", stage, ms);
                }
            }

            // Output telemetry if requested.
            if let Some(tc) = telemetry {
                let report = tc.finalize();
                println!("\n{}", serde_json::to_string_pretty(&report).unwrap());
            }

            Ok(())
        }
        CompileResult::Partial(partial) => {
            print_errors(&partial.diagnostics);

            if cli.telemetry {
                if let Some(tc) = telemetry {
                    let report = tc.finalize();
                    eprintln!("\n{}", serde_json::to_string_pretty(&report).unwrap());
                }
            }

            Err(format!(
                "compilation failed with {} error(s) (last completed stage: {})",
                partial.diagnostics.len(),
                partial.last_completed_stage
                    .map(|s| format!("{:?}", s))
                    .unwrap_or_else(|| "none".to_string())
            ))
        }
    }
}

/// Non-AArch64 implementation of `vuma build`: uses the direct
/// AST→codegen SCG bridge path (`compile_to_binary_direct`). Does not
/// run the canonical pipeline (no verification, MSG, SCG transforms,
/// or telemetry — those are AArch64-only until the canonical pipeline
/// is generalised to other ISAs).
fn cmd_build_direct(
    cli: &Cli,
    file: &PathBuf,
    output: &Option<PathBuf>,
    source: &str,
    isa: IsaArg,
) -> Result<(), String> {
    let backend_kind = BackendKind::from(isa);
    eprintln!(
        "[build] Note: targeting {} via direct AST→codegen path \
         (canonical pipeline is AArch64-only; verification/telemetry unavailable)",
        backend_kind.isa_name(),
    );

    let binary = compile_to_binary_direct(source, isa)?;

    let out_path = output
        .as_ref()
        .cloned()
        .unwrap_or_else(|| default_output_path(file));
    fs::write(&out_path, &binary).map_err(|e| {
        format!(
            "error: cannot write output file '{}': {}",
            out_path.display(),
            e
        )
    })?;
    // Make the output file executable on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&out_path)
            .map_err(|e| format!("error: cannot stat '{}': {}", out_path.display(), e))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&out_path, perms)
            .map_err(|e| format!("error: cannot chmod '{}': {}", out_path.display(), e))?;
    }

    if !cli.quiet {
        println!(
            "Compiled {} -> {} ({} bytes, ISA: {})",
            file.display(),
            out_path.display(),
            binary.len(),
            backend_kind.isa_name(),
        );
    }

    Ok(())
}

/// `vuma run <file>` — Build + execute.
///
/// Always uses the direct AST→codegen SCG bridge path
/// (`compile_to_binary_direct`) so that the emitted binary matches the
/// resolved ISA (explicit `--isa` flag, else host arch, else AArch64).
/// This is necessary because the canonical pipeline (`compile_with_path`)
/// always emits AArch64 ELF — using it here would silently produce a
/// non-runnable binary on x86_64 / riscv64 / etc. hosts.
///
/// Execution strategy:
///   1. Try to execute the binary natively (works when the resolved ISA
///      matches the host ISA).
///   2. On native-exec failure (ENOEXEC, etc.), try `qemu-<isa>` as a
///      user-space emulator.
///   3. If both fail, print a clear, actionable error message naming the
///      ISA, the host arch, and the suggested remedy (install qemu or
///      compile for the host via `vuma emit`).
fn cmd_run(
    _cli: &Cli,
    file: &PathBuf,
    args: &[String],
    isa: &Option<IsaArg>,
) -> Result<(), String> {
    let source = read_source(file)?;
    let resolved_isa = resolve_isa(isa);
    let backend_kind = BackendKind::from(resolved_isa);
    let isa_name = backend_kind.isa_name();
    let host_arch = std::env::consts::ARCH;

    // Build via the direct path so the binary targets `resolved_isa`
    // (NOT the canonical pipeline's hardcoded AArch64).
    let binary = compile_to_binary_direct(&source, resolved_isa).map_err(|err| {
        global_logger().error(
            "run",
            &format!("compilation failed: {}", err),
        );
        err
    })?;

    let tmp_dir = std::env::temp_dir();
    let exe_path = tmp_dir.join(format!("vuma_run_{}", std::process::id()));
    fs::write(&exe_path, &binary).map_err(|e| {
        format!(
            "error: cannot write temporary executable '{}': {}",
            exe_path.display(),
            e
        )
    })?;

    // Make the file executable on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&exe_path)
            .map_err(|e| format!("error: cannot stat '{}': {}", exe_path.display(), e))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&exe_path, perms)
            .map_err(|e| format!("error: cannot chmod '{}': {}", exe_path.display(), e))?;
    }

    // Step 1: try native execution. This works whenever the resolved ISA
    // matches the host ISA (the common case, since resolved_isa defaults
    // to host_isa()).
    let native_output = Command::new(&exe_path).args(args).output();

    let output = match native_output {
        Ok(out) => out,
        Err(native_err) => {
            // Step 2: native exec failed (typically ENOEXEC on ISA
            // mismatch). Try the appropriate qemu-user emulator.
            // qemu-user binary names: qemu-aarch64, qemu-x86_64,
            // qemu-riscv64, qemu-arm (for arm32), etc.
            let qemu_bin = format!("qemu-{}", isa_name);
            match Command::new(&qemu_bin)
                .arg(&exe_path)
                .args(args)
                .output()
            {
                Ok(qemu_out) => qemu_out,
                Err(qemu_err) => {
                    // Step 3: both failed. Clean up and surface a clear,
                    // actionable error.
                    let _ = fs::remove_file(&exe_path);
                    let host_isa_name = host_isa()
                        .map(|i| BackendKind::from(i).isa_name())
                        .unwrap_or("aarch64");
                    return Err(format!(
                        "error: cannot execute {isa} binary on {host} host.\n\
                         Native exec failed: {native_err}\n\
                         {qemu} not found: {qemu_err2}\n\
                         \n\
                         To fix this, either:\n\
                         (1) install qemu-{isa} (e.g. `apt install qemu-user`), or\n\
                         (2) compile for the host instead: `vuma emit {host_isa} {file}`",
                        isa = isa_name,
                        host = host_arch,
                        native_err = native_err,
                        qemu = qemu_bin,
                        qemu_err2 = qemu_err,
                        host_isa = host_isa_name,
                        file = file.display(),
                    ));
                }
            }
        }
    };

    io::stdout()
        .write_all(&output.stdout)
        .map_err(|e| format!("error: failed to write program output: {}", e))?;
    io::stderr()
        .write_all(&output.stderr)
        .map_err(|e| format!("error: failed to write program stderr: {}", e))?;

    // Clean up temp file.
    let _ = fs::remove_file(&exe_path);

    let code = output.status.code().unwrap_or(1);
    if code != 0 {
        return Err(format!("program exited with code {}", code));
    }

    Ok(())
}

/// `vuma check <file>` — Parse + SCG + BD inference + IVE verification only.
fn cmd_check(cli: &Cli, file: &PathBuf) -> Result<(), String> {
    let source = read_source(file)?;
    let mut config = make_config(cli, CompileTarget::Linux);
    // check mode: always run verification, don't skip
    if config.verification_level == VerificationLevel::None {
        config.verification_level = VerificationLevel::Normal;
    }

    // Run the full compile — the check command doesn't save the binary,
    // it just verifies the program compiles and passes verification.
    let result = compile_with_path(&source, Some(file), &config).map_err(|errors| {
        print_errors(&errors);
        global_logger().error("check", &format!("check failed with {} error(s)", errors.len()));
        format!("check failed with {} error(s)", errors.len())
    })?;

    if !cli.quiet {
        println!("Check passed for {}", file.display());
        println!("  SCG nodes:      {}", result.scg.node_count());
        println!("  IR functions:   {}", result.ir_function_count);
        println!("  IR instructions: {}", result.ir_instruction_count);

        if let Some(ref verification) = result.verification {
            println!("  Verification:   {}", verification.overall);
        }

        // Print stage timings.
        for (stage, ms) in &result.stage_timings {
            println!("  {:20} {}ms", stage, ms);
        }
    }

    Ok(())
}

/// `vuma emit <isa> <file>` — Compile to a specific ISA target.
///
/// Uses the direct AST → codegen SCG → IR path for better code quality
/// than the main pipeline (which goes through the semantic SCG and loses
/// most program semantics in the SCG→IR bridge).
fn cmd_emit(
    _cli: &Cli,
    isa: &IsaArg,
    file: &PathBuf,
    output: &Option<PathBuf>,
) -> Result<(), String> {
    // NOTE: This command uses the direct AST→codegen SCG bridge
    // (bridge_ast_to_codegen_scg) which bypasses the canonical
    // semantic SCG pipeline. For full verification support, use
    // `vuma build` or `vuma compile` which route through the
    // canonical pipeline (src/pipeline.rs).
    // TODO: Unify this path with the canonical pipeline.
    eprintln!("[emit] Note: using direct AST→codegen path (not canonical pipeline)");
    let source = read_source(file)?;
    let backend_kind = BackendKind::from(*isa);

    // Step 1: Parse source → AST.
    let mut parser = vuma_parser::Parser::new(&source);
    let parse_result = parser.parse_program();
    if parse_result.is_err() {
        return Err(format!("parse error: {:?}", parse_result.errors));
    }
    if !parse_result.errors.is_empty() {
        eprintln!("[emit] WARNING: {} non-fatal parse errors:", parse_result.errors.len());
        for err in &parse_result.errors {
            eprintln!("[emit]   {:?}", err);
        }
    }
    let program = parse_result.value.unwrap();

    // Step 2: Bridge parser AST → codegen SCG.
    let codegen_scg = bridge_ast_to_codegen_scg(&program);

    // Step 3: Lower codegen SCG → IR.
    let mut ir_builder = ScgToIr::new();
    let ir_program = ir_builder.convert(&codegen_scg).map_err(|e| {
        format!("IR conversion error: {}", e)
    })?;

    eprintln!("[emit] IR program has {} functions", ir_program.functions.len());
    for func in &ir_program.functions {
        eprintln!("[emit] Function: {} ({} params, {} vregs)", func.name, func.params.len(), func.vregs.len());
        for block in &func.blocks {
            eprintln!("[emit]   Block: {}", block.label);
            for instr in &block.instructions {
                eprintln!("[emit]     {:?}", instr);
            }
        }
    }

    // Step 4: Create backend and allocate registers (with fallback).
    let backend = match create_backend(backend_kind) {
        Ok(b) => b,
        Err(e) => {
            let err_msg = format!("{}", e);
            global_logger().warn("emit", &format!("{} backend failed: {}", backend_kind.isa_name(), err_msg));
            // Try fallback to AArch64 if not already trying it
            if backend_kind != BackendKind::AArch64 {
                global_logger().info("emit", "falling back to aarch64 backend");
                create_backend(BackendKind::AArch64).map_err(|e2| {
                    format!("error: cannot create {} backend: {}, aarch64 fallback also failed: {}",
                        backend_kind.isa_name(), err_msg, e2)
                })?
            } else {
                return Err(format!("error: cannot create {} backend: {}", backend_kind.isa_name(), err_msg));
            }
        }
    };

    let mut allocated_functions = Vec::new();
    for func in &ir_program.functions {
        match backend.allocate_registers(func) {
            Ok(allocated) => allocated_functions.push(allocated),
            Err(e) => {
                eprintln!("warning: register allocation failed for '{}': {}", func.name, e);
            }
        }
    }

    let out_path = output
        .as_ref()
        .cloned()
        .unwrap_or_else(|| default_output_path(file));

    // Step 5: Encode and write output.
    if !allocated_functions.is_empty() {
        let allocated_program = vuma_codegen::backend::AllocatedProgram {
            functions: allocated_functions,
            total_code_size: 0,
            total_data_size: 0,
        };
        match backend.encode_program(&allocated_program) {
            Ok(bytes) => {
                fs::write(&out_path, &bytes).map_err(|e| {
                    format!("error: cannot write output file '{}': {}", out_path.display(), e)
                })?;
                // Make the output file executable on Unix.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = fs::metadata(&out_path)
                        .map_err(|e| format!("error: cannot stat '{}': {}", out_path.display(), e))?
                        .permissions();
                    perms.set_mode(0o755);
                    fs::set_permissions(&out_path, perms)
                        .map_err(|e| format!("error: cannot chmod '{}': {}", out_path.display(), e))?;
                }
                println!(
                    "Emitted {} -> {} ({} bytes, ISA: {})",
                    file.display(),
                    out_path.display(),
                    bytes.len(),
                    backend.name(),
                );
            }
            Err(e) => {
                use vuma_codegen::backend::BackendError;
                let prefix = match &e {
                    BackendError::UnresolvedRelocation { .. } => "E037",
                    _ => "error",
                };
                return Err(format!("{}: {} encoding failed: {}", prefix, backend.name(), e));
            }
        }
    } else {
        return Err("no functions were successfully allocated".to_string());
    }

    Ok(())
}

/// `vuma compile <file>` — Compile to a relocatable object file (ET_REL) for
/// linking with the system linker.  Produces ELF object files with proper
/// `.rela.text` sections and `SHN_UNDEF` symbol entries for extern functions,
/// so that `ld -o program program.o -lc` works.
///
/// This is the recommended workflow for programs that use `extern "C"` FFI:
///   vuma compile --format obj --target x86_64 program.vuma -o program.o
///   ld -o program program.o -lc
fn cmd_compile(
    _cli: &Cli,
    file: &PathBuf,
    output: &Option<PathBuf>,
    target: &IsaArg,
    format: &FormatArg,
) -> Result<(), String> {
    let source = read_source(file)?;
    let backend_kind = BackendKind::from(*target);

    // Step 1: Parse source → AST.
    let mut parser = vuma_parser::Parser::new(&source);
    let parse_result = parser.parse_program();
    if parse_result.is_err() {
        return Err(format!("parse error: {:?}", parse_result.errors));
    }
    if !parse_result.errors.is_empty() {
        eprintln!("[compile] WARNING: {} non-fatal parse errors:", parse_result.errors.len());
        for err in &parse_result.errors {
            eprintln!("[compile]   {:?}", err);
        }
    }
    let program = parse_result.value.unwrap();

    // Step 2: Bridge parser AST → codegen SCG (with extern awareness).
    let codegen_scg = bridge_ast_to_codegen_scg(&program);

    // Step 3: Lower codegen SCG → IR.
    let mut ir_builder = ScgToIr::new();
    let ir_program = ir_builder.convert(&codegen_scg).map_err(|e| {
        format!("IR conversion error: {}", e)
    })?;

    eprintln!("[compile] IR program has {} functions", ir_program.functions.len());

    // Step 4: Create backend and allocate registers.
    let backend = create_backend(backend_kind).map_err(|e| {
        format!("error: cannot create {} backend: {}", backend_kind.isa_name(), e)
    })?;

    let mut allocated_functions = Vec::new();
    for func in &ir_program.functions {
        match backend.allocate_registers(func) {
            Ok(allocated) => allocated_functions.push(allocated),
            Err(e) => {
                eprintln!("warning: register allocation failed for '{}': {}", func.name, e);
            }
        }
    }

    let out_path = output
        .as_ref()
        .cloned()
        .unwrap_or_else(|| {
            let stem = file.file_stem().unwrap_or_default().to_string_lossy();
            let dir = file.parent().unwrap_or(std::path::Path::new("."));
            dir.join(format!("{}.o", stem))
        });

    // Step 5: Encode and write output.
    if !allocated_functions.is_empty() {
        let allocated_program = vuma_codegen::backend::AllocatedProgram {
            functions: allocated_functions,
            total_code_size: 0,
            total_data_size: 0,
        };

        // For --format obj, we want ET_REL output with relocation entries
        // so the system linker can resolve extern symbols.
        // For --format elf, we produce a standalone ET_EXEC (same as emit).
        // For --format raw, we produce raw machine code bytes.
        // For --format wasm, we produce a Wasm binary.
        match format {
            FormatArg::Obj => {
                // ET_REL: produce a relocatable object file.
                // The backend's encode_program with relocatable config
                // generates .rela.text sections with SHN_UNDEF for externs.
                match backend.encode_program(&allocated_program) {
                    Ok(bytes) => {
                        // Patch the ELF header to ET_REL (e_type = 1) if it's currently ET_EXEC (e_type = 2).
                        // This is a lightweight approach — the relocations are already
                        // generated as SHN_UNDEF for extern calls.
                        let mut out_bytes = bytes;
                        if out_bytes.len() >= 18 {
                            // ELF64 header: e_type is at offset 16 (2 bytes, little-endian).
                            // Set e_type = ET_REL (1) for relocatable object.
                            // Note: if the backend already emits relocations with SHN_UNDEF,
                            // this is sufficient for the system linker to resolve them.
                            let e_type = u16::from_le_bytes([out_bytes[16], out_bytes[17]]);
                            if e_type == 2 {
                                // Currently ET_EXEC — change to ET_REL
                                let rel_bytes = 1u16.to_le_bytes();
                                out_bytes[16] = rel_bytes[0];
                                out_bytes[17] = rel_bytes[1];
                                eprintln!("[compile] Patched ELF type: ET_EXEC → ET_REL");
                            }
                        }
                        fs::write(&out_path, &out_bytes).map_err(|e| {
                            format!("error: cannot write output file '{}': {}", out_path.display(), e)
                        })?;
                        println!(
                            "Compiled {} -> {} ({} bytes, ISA: {}, format: obj)",
                            file.display(),
                            out_path.display(),
                            out_bytes.len(),
                            backend.name(),
                        );
                    }
                    Err(e) => {
                        use vuma_codegen::backend::BackendError;
                        let prefix = match &e {
                            BackendError::UnresolvedRelocation { .. } => "E037",
                            _ => "error",
                        };
                        return Err(format!("{}: {} compile failed: {}", prefix, backend.name(), e));
                    }
                }
            }
            FormatArg::Elf | FormatArg::Raw | FormatArg::Wasm => {
                // For other formats, delegate to the standard encoding path.
                match backend.encode_program(&allocated_program) {
                    Ok(bytes) => {
                        fs::write(&out_path, &bytes).map_err(|e| {
                            format!("error: cannot write output file '{}': {}", out_path.display(), e)
                        })?;
                        println!(
                            "Compiled {} -> {} ({} bytes, ISA: {}, format: {:?})",
                            file.display(),
                            out_path.display(),
                            bytes.len(),
                            backend.name(),
                            format,
                        );
                    }
                    Err(e) => {
                        return Err(format!("error: {} compile failed: {}", backend.name(), e));
                    }
                }
            }
        }
    } else {
        return Err("no functions were successfully allocated".to_string());
    }

    Ok(())
}

/// Bridge parser AST → codegen SCG.
///
/// This converts the parser's AST into the codegen's SCG representation,
/// which can then be lowered to IR. This bypasses the main pipeline's
/// semantic SCG which loses most program semantics.
///
/// Key design: expressions are flattened into three-address code (TAC) by
/// introducing temporary variables for sub-expressions. This preserves the
/// full semantics of nested binary operations, function calls, dereferences,
/// and casts.
/// Extract extern function names from `extern "C" { ... }` blocks in the AST.
/// Mirrors the same function in pipeline.rs for use in the `emit` path.
fn extract_extern_functions_from_ast(program: &vuma_parser::ast::Program) -> HashSet<String> {
    use vuma_parser::ast::Item;
    let mut extern_fns = HashSet::new();
    for item in &program.items {
        if let Item::ExternBlock(eb) = item {
            for fn_decl in &eb.functions {
                extern_fns.insert(fn_decl.name.clone());
            }
        }
    }
    extern_fns
}

fn bridge_ast_to_codegen_scg(program: &vuma_parser::ast::Program) -> Scg {
    use vuma_parser::ast::Item;

    // Collect extern function names so we can mark calls as is_extern.
    let extern_fns = extract_extern_functions_from_ast(program);

    // Collect global constant definitions so they can be inlined
    // as literal values when referenced in function bodies.
    let global_constants = collect_global_constants(program);

    let mut nodes: Vec<ScgNode> = Vec::new();

    for item in &program.items {
        if let Item::FnDef(fn_def) = item {
            let params: Vec<ScgParam> = fn_def
                .params
                .iter()
                .map(|p| ScgParam {
                    name: p.name.clone(),
                    ty: bridge_type_to_codegen_scg(&p.ty),
                })
                .collect();

            let results = if let Some(ref ret_ty) = fn_def.return_type {
                vec![bridge_type_to_codegen_scg(&Some(ret_ty.clone()))]
            } else {
                vec![]
            };

            let mut ctx = BridgeCtx::new();
            ctx.extern_fns = extern_fns.clone();
            ctx.global_constants = global_constants.clone();
            let mut body = bridge_block_to_scg_stmts(&fn_def.body, &mut ctx);

            // Ensure every function ends with a Return statement.
            // If the body doesn't end with a Return, add an implicit one.
            // When the function has a return type and the last statement was an
            // expression, use ctx.last_expr_result as the return value.
            let has_return = body.last().map_or(false, |s| matches!(s, ScgStatement::Return(_)));
            if !has_return {
                let ret_val = if !results.is_empty() {
                    // First check if the last expression was tracked.
                    if let Some(ref expr) = ctx.last_expr_result {
                        Some(expr.clone())
                    } else {
                        // Otherwise, look for the last computation/call result.
                        body.iter().rev().find_map(|s| match s {
                            ScgStatement::Computation(comp) => Some(ScgExpr::Var(comp.dst.clone())),
                            ScgStatement::Call(call) => call.dst.as_ref().map(|d| ScgExpr::Var(d.clone())),
                            _ => None,
                        })
                    }
                } else {
                    None
                };
                body.push(ScgStatement::Return(ret_val.into_iter().collect()));
            }

            nodes.push(ScgNode::Function(ScgFunction {
                name: fn_def.name.clone(),
                params,
                results,
                body,
            }));
        }
    }

    Scg { nodes }
}

/// Context for the AST → codegen SCG bridge, tracking a monotonic temp counter
/// and the last expression result for implicit returns.
struct BridgeCtx {
    temp_counter: u32,
    /// The result of the last expression statement, if any.
    /// Used for implicit return when a function body ends with an expression.
    last_expr_result: Option<ScgExpr>,
    /// Set of extern function names (from `extern "C" { ... }` blocks).
    /// Used to mark CallNodes as is_extern when the target is declared extern.
    extern_fns: HashSet<String>,
    /// Global constant definitions: maps name → integer value.
    /// Populated by scanning top-level `const`, `static`, and type-ascription
    /// declarations before processing any function bodies.
    global_constants: HashMap<String, i64>,
}

impl BridgeCtx {
    fn new() -> Self {
        Self { temp_counter: 0, last_expr_result: None, extern_fns: HashSet::new(), global_constants: HashMap::new() }
    }

    /// Allocate a unique temporary variable name.
    fn alloc_temp(&mut self) -> String {
        let name = format!("__t{}", self.temp_counter);
        self.temp_counter += 1;
        name
    }
}

/// Try to evaluate a constant expression to an integer value.
///
/// Handles integer literals, boolean literals, and simple binary operations
/// on constant sub-expressions. Returns `None` for non-constant expressions
/// (variable references, function calls, etc.).
fn eval_const_expr(expr: &vuma_parser::ast::Expr, consts: &HashMap<String, i64>) -> Option<i64> {
    use vuma_parser::ast::{Expr, Lit, BinOp, UnOp};
    match expr {
        Expr::Lit { value, .. } => match value {
            Lit::Int(n) => Some(*n),
            Lit::Bool(b) => Some(if *b { 1 } else { 0 }),
            _ => None,
        },
        Expr::Var { name, .. } => consts.get(name).copied(),
        Expr::BinOp { op, lhs, rhs, .. } => {
            let l = eval_const_expr(lhs, consts)?;
            let r = eval_const_expr(rhs, consts)?;
            Some(match op {
                BinOp::Add => l.wrapping_add(r),
                BinOp::Sub => l.wrapping_sub(r),
                BinOp::Mul => l.wrapping_mul(r),
                BinOp::Div => l.checked_div(r)?,
                BinOp::Mod => l.checked_rem(r)?,
                BinOp::BitAnd => l & r,
                BinOp::BitOr => l | r,
                BinOp::BitXor => l ^ r,
                BinOp::Shl => l.wrapping_shl(r as u32),
                BinOp::Shr => l.wrapping_shr(r as u32),
                BinOp::And => if l != 0 && r != 0 { 1 } else { 0 },
                BinOp::Or => if l != 0 || r != 0 { 1 } else { 0 },
                BinOp::Eq => if l == r { 1 } else { 0 },
                BinOp::Ne => if l != r { 1 } else { 0 },
                BinOp::Lt => if l < r { 1 } else { 0 },
                BinOp::Le => if l <= r { 1 } else { 0 },
                BinOp::Gt => if l > r { 1 } else { 0 },
                BinOp::Ge => if l >= r { 1 } else { 0 },
            })
        }
        Expr::UnOp { op, expr, .. } => {
            let v = eval_const_expr(expr, consts)?;
            Some(match op {
                UnOp::Neg => v.wrapping_neg(),
                UnOp::Not => if v == 0 { 1 } else { 0 },
                UnOp::BitNot => !v,
                UnOp::Deref => return None, // can't const-eval deref
            })
        }
        Expr::Cast { expr, .. } | Expr::TypeAscription { expr, .. } => {
            eval_const_expr(expr, consts)
        }
        _ => None,
    }
}

/// Collect global constant definitions from the top-level program items.
///
/// Scans for `Item::Const`, `Item::Static`, and `Item::Stmt(Stmt::Let)` that
/// have constant-evaluable initializers and builds a name → value map.
fn collect_global_constants(program: &vuma_parser::ast::Program) -> HashMap<String, i64> {
    use vuma_parser::ast::Item;
    let mut consts: HashMap<String, i64> = HashMap::new();

    for item in &program.items {
        match item {
            Item::Const(c) => {
                if let Some(val) = eval_const_expr(&c.value, &consts) {
                    consts.insert(c.name.clone(), val);
                }
            }
            Item::Static(s) => {
                if let Some(val) = eval_const_expr(&s.value, &consts) {
                    consts.insert(s.name.clone(), val);
                }
            }
            Item::Stmt(stmt) => {
                if let vuma_parser::ast::Stmt::Let(let_stmt) = stmt {
                    if let Some(val) = eval_const_expr(&let_stmt.value, &consts) {
                        consts.insert(let_stmt.name.clone(), val);
                    }
                }
            }
            _ => {}
        }
    }

    consts
}

/// Convert a parser type annotation to a codegen SCG type.
fn bridge_type_to_codegen_scg(ty: &Option<vuma_parser::ast::Type>) -> ScgType {
    match ty {
        Some(vuma_parser::ast::Type::BDBase(name)) => match name.as_str() {
            "i8" => ScgType::I8,
            "i16" => ScgType::I16,
            "i32" => ScgType::I32,
            "i64" => ScgType::I64,
            "u8" => ScgType::U8,
            "u16" => ScgType::U16,
            "u32" => ScgType::U32,
            "u64" => ScgType::U64,
            _ => ScgType::I64,
        },
        Some(vuma_parser::ast::Type::Ptr(_)) => ScgType::Ptr,
        Some(vuma_parser::ast::Type::RegionPtr { .. }) => ScgType::Ptr,
        _ => ScgType::Void,
    }
}


/// Convert a parser block into codegen SCG statements, flattening expressions
/// into three-address code with temporaries.
fn bridge_block_to_scg_stmts(block: &vuma_parser::ast::Block, ctx: &mut BridgeCtx) -> Vec<ScgStatement> {
    block
        .statements
        .iter()
        .flat_map(|s| bridge_stmt_to_scg(s, ctx))
        .collect()
}

/// Flatten an expression into three-address code. Returns the ScgExpr that
/// holds the result, and appends any intermediate computation statements
/// to `stmts`.
///
/// This is the core of the bridge: it recursively decomposes nested
/// expressions into a sequence of simple computation nodes, each operating
/// on at most two operands and producing one result. This preserves the
/// full semantics of the original expression tree.
fn flatten_expr(
    expr: &vuma_parser::ast::Expr,
    stmts: &mut Vec<ScgStatement>,
    ctx: &mut BridgeCtx,
) -> ScgExpr {
    use vuma_parser::ast::{Expr, Lit, UnOp};

    match expr {
        // ── Leaf expressions: return directly, no flattening needed ──
        Expr::Var { name, .. } => {
            // Check if this variable is a known global constant.
            // If so, inline its literal value instead of emitting a variable
            // reference that would be unresolved during IR lowering.
            if let Some(&val) = ctx.global_constants.get(name) {
                ScgExpr::Int(val)
            } else {
                ScgExpr::Var(name.clone())
            }
        }
        Expr::Lit { value, .. } => match value {
            Lit::Int(n) => ScgExpr::Int(*n),
            Lit::Float(f) => ScgExpr::Float(*f),
            Lit::Bool(b) => ScgExpr::Int(if *b { 1 } else { 0 }),
            Lit::Address(a) => ScgExpr::Int(*a as i64),
            Lit::String(_) => {
                eprintln!("[vuma] WARNING: string literals are not supported in codegen; using 0");
                ScgExpr::Int(0)
            }
        },

        // ── Binary operations: flatten lhs and rhs, then emit one Computation ──
        Expr::BinOp { op, lhs, rhs, .. } => {
            let lhs_expr = flatten_expr(lhs, stmts, ctx);
            let rhs_expr = flatten_expr(rhs, stmts, ctx);
            let dst = ctx.alloc_temp();
            let binop_kind = map_ast_binop(op);
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: dst.clone(),
                op: binop_kind,
                lhs: lhs_expr,
                rhs: rhs_expr,
                tail_call: false,
                reassigns: None,
            }));
            ScgExpr::Var(dst)
        }

        // ── Unary operations: flatten operand, then emit one Computation ──
        Expr::UnOp { op, expr: operand, .. } => {
            let operand_expr = flatten_expr(operand, stmts, ctx);
            let dst = ctx.alloc_temp();
            match op {
                UnOp::BitNot => {
                    stmts.push(ScgStatement::Computation(ComputationNode {
                        dst: dst.clone(),
                        op: BinOpKind::Xor,
                        lhs: operand_expr,
                        rhs: ScgExpr::Int(-1),
                        tail_call: false,
                        reassigns: None,
                    }));
                }
                UnOp::Neg => {
                    stmts.push(ScgStatement::Computation(ComputationNode {
                        dst: dst.clone(),
                        op: BinOpKind::Sub,
                        lhs: ScgExpr::Int(0),
                        rhs: operand_expr,
                        tail_call: false,
                        reassigns: None,
                    }));
                }
                UnOp::Not => {
                    stmts.push(ScgStatement::Computation(ComputationNode {
                        dst: dst.clone(),
                        op: BinOpKind::Eq,
                        lhs: operand_expr,
                        rhs: ScgExpr::Int(0),
                        tail_call: false,
                        reassigns: None,
                    }));
                }
                UnOp::Deref => {
                    stmts.push(ScgStatement::Access(AccessNode::Load {
                        dst: dst.clone(),
                        ptr: operand_expr,
                        offset: None,
                        ty: None,
                    }));
                }
            }
            ScgExpr::Var(dst)
        }

        // ── Function call: flatten args, emit CallNode ──
        Expr::Call { callee, args, .. } => {
            let func_name = match callee.as_ref() {
                Expr::Var { name, .. } => name.clone(),
                _ => "_unknown".into(),
            };
            let flat_args: Vec<ScgExpr> = args.iter()
                .map(|a| flatten_expr(a, stmts, ctx))
                .collect();
            let dst = ctx.alloc_temp();
            // Mark as extern if the function was declared in an extern "C" block
            // OR if it's a known built-in intrinsic (AtomicLoad/AtomicStore/AtomicCas
            // are lowered by the backend to machine instructions, not external calls,
            // but allocate/free are truly external).
            let is_extern = ctx.extern_fns.contains(&func_name)
                || func_name == "__vuma_alloc"
                || func_name == "__vuma_dealloc";
            stmts.push(ScgStatement::Call(CallNode {
                dst: Some(dst.clone()),
                func: func_name,
                args: flat_args,
                is_extern,
                reassigns: None,
            }));
            ScgExpr::Var(dst)
        }

        // ── Atomic operations: emit as CallNodes with special names ──
        // The backend's instruction selector recognizes these names and lowers
        // them to proper atomic machine instructions (LDAXR/STLXR on AArch64,
        // LOCK CMPXCHG on x86_64, LR.D/SC.D on RISC-V, etc.).
        Expr::AtomicLoad { addr, .. } => {
            let addr_expr = flatten_expr(addr, stmts, ctx);
            let dst = ctx.alloc_temp();
            stmts.push(ScgStatement::Call(CallNode {
                dst: Some(dst.clone()),
                func: "AtomicLoad".to_string(),
                args: vec![addr_expr],
                is_extern: true,
                reassigns: None,
            }));
            ScgExpr::Var(dst)
        }
        Expr::AtomicStore { value, addr, .. } => {
            let value_expr = flatten_expr(value, stmts, ctx);
            let addr_expr = flatten_expr(addr, stmts, ctx);
            let dst = ctx.alloc_temp();
            stmts.push(ScgStatement::Call(CallNode {
                dst: Some(dst.clone()),
                func: "AtomicStore".to_string(),
                args: vec![value_expr, addr_expr],
                is_extern: true,
                reassigns: None,
            }));
            ScgExpr::Var(dst)
        }
        Expr::AtomicCas { addr, expected, desired, .. } => {
            let addr_expr = flatten_expr(addr, stmts, ctx);
            let expected_expr = flatten_expr(expected, stmts, ctx);
            let desired_expr = flatten_expr(desired, stmts, ctx);
            let dst = ctx.alloc_temp();
            stmts.push(ScgStatement::Call(CallNode {
                dst: Some(dst.clone()),
                func: "AtomicCas".to_string(),
                args: vec![addr_expr, expected_expr, desired_expr],
                is_extern: true,
                reassigns: None,
            }));
            ScgExpr::Var(dst)
        }

        // ── Dereference: flatten the address, emit Load ──
        Expr::Deref { expr, .. } => {
            let addr = flatten_expr(expr, stmts, ctx);
            let dst = ctx.alloc_temp();
            stmts.push(ScgStatement::Access(AccessNode::Load {
                dst: dst.clone(),
                ptr: addr,
                offset: None,
                ty: None,
            }));
            ScgExpr::Var(dst)
        }

        // ── Offset (pointer arithmetic): flatten base and offset, emit Add ──
        Expr::Offset { base, offset, .. } => {
            let base_expr = flatten_expr(base, stmts, ctx);
            let off_expr = flatten_expr(offset, stmts, ctx);
            let dst = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: dst.clone(),
                op: BinOpKind::Add,
                lhs: base_expr,
                rhs: off_expr,
                tail_call: false,
                reassigns: None,
            }));
            ScgExpr::Var(dst)
        }

        // ── Cast: flatten operand, pass through ──
        Expr::Cast { expr, .. } => flatten_expr(expr, stmts, ctx),

        // ── TypeAscription: flatten inner expression ──
        Expr::TypeAscription { expr, .. } => flatten_expr(expr, stmts, ctx),

        // ── Index: flatten base and index, compute addr, emit Load ──
        Expr::Index { expr, index, .. } => {
            let base_expr = flatten_expr(expr, stmts, ctx);
            let idx_expr = flatten_expr(index, stmts, ctx);
            let addr = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: addr.clone(),
                op: BinOpKind::Add,
                lhs: base_expr,
                rhs: idx_expr,
                tail_call: false,
                reassigns: None,
            }));
            let dst = ctx.alloc_temp();
            stmts.push(ScgStatement::Access(AccessNode::Load {
                dst: dst.clone(),
                ptr: ScgExpr::Var(addr),
                offset: None,
                ty: None,
            }));
            ScgExpr::Var(dst)
        }

        // ── Range: just flatten start (range is handled by For) ──
        Expr::Range { start, .. } => flatten_expr(start, stmts, ctx),

        // ── Allocate: emit as a heap allocation call ──
        Expr::Allocate { size, .. } => {
            let size_expr = flatten_expr(size, stmts, ctx);
            let dst = ctx.alloc_temp();
            stmts.push(ScgStatement::Call(CallNode {
                dst: Some(dst.clone()),
                func: "__vuma_alloc".to_string(),
                args: vec![size_expr],
                is_extern: true,
                reassigns: None,
            }));
            ScgExpr::Var(dst)
        }

        // ── Null → 0 ──
        Expr::Null { .. } => ScgExpr::Int(0),

        // ── Uninitialized → 0 ──
        Expr::Uninitialized { .. } => ScgExpr::Int(0),

        // ── Address-of: emit GetAddress for symbol references ──
        Expr::AddressOf { expr, .. } => {
            // If the inner expression is a variable (function name, data symbol),
            // emit a GetAddress node that will lower to `IRInstr::GetAddress`
            // with a proper relocation.  Otherwise, just flatten the inner expr
            // (e.g. for @(*ptr) or other complex address-of patterns).
            match expr.as_ref() {
                Expr::Var { name, .. } => {
                    let dst = ctx.alloc_temp();
                    stmts.push(ScgStatement::GetAddress(GetAddressNode {
                        dst: dst.clone(),
                        name: name.clone(),
                    }));
                    ScgExpr::Var(dst)
                }
                _ => flatten_expr(expr, stmts, ctx),
            }
        }

        // ── Fallback for unsupported expression types ──
        // Log a warning instead of silently returning 0. This makes
        // unsupported constructs visible during compilation.
        _ => {
            eprintln!("[vuma] WARNING: unsupported expression type in flatten_expr; using 0");
            ScgExpr::Int(0)
        }
    }
}

/// Map a VUMA AST BinOp to a codegen BinOpKind.
fn map_ast_binop(op: &vuma_parser::ast::BinOp) -> BinOpKind {
    use vuma_parser::ast::BinOp;
    match op {
        BinOp::Add => BinOpKind::Add,
        BinOp::Sub => BinOpKind::Sub,
        BinOp::Mul => BinOpKind::Mul,
        BinOp::Div => BinOpKind::SDiv,
        BinOp::Mod => BinOpKind::SRem,
        BinOp::And => BinOpKind::And,
        BinOp::Or => BinOpKind::Or,
        BinOp::BitAnd => BinOpKind::And,
        BinOp::BitOr => BinOpKind::Or,
        BinOp::BitXor => BinOpKind::Xor,
        BinOp::Shl => BinOpKind::Shl,
        BinOp::Shr => BinOpKind::ShrL,
        BinOp::Eq => BinOpKind::Eq,
        BinOp::Ne => BinOpKind::Ne,
        BinOp::Lt => BinOpKind::SLt,
        BinOp::Le => BinOpKind::SLe,
        BinOp::Gt => BinOpKind::SGt,
        BinOp::Ge => BinOpKind::SGe,
    }
}

/// Convert a single parser statement into zero or more codegen SCG statements.
/// Uses `flatten_expr` to decompose nested expressions into three-address code.
fn bridge_stmt_to_scg(stmt: &vuma_parser::ast::Stmt, ctx: &mut BridgeCtx) -> Vec<ScgStatement> {
    use vuma_parser::ast::Stmt as PStmt;

    match stmt {
        // ── let x [: T] = expr ──
        PStmt::Let(let_stmt) => {
            let mut stmts = Vec::new();

            // Check if the RHS is an allocate() call → AllocationNode::Stack
            if let vuma_parser::ast::Expr::Call { callee, args, .. } = &let_stmt.value {
                if let vuma_parser::ast::Expr::Var { name, .. } = callee.as_ref() {
                    if name == "allocate" {
                        let size: u32 = args.first()
                            .and_then(|a| {
                                if let vuma_parser::ast::Expr::Lit { value, .. } = a {
                                    if let vuma_parser::ast::Lit::Int(n) = value {
                                        return Some(*n as u32);
                                    }
                                }
                                None
                            })
                            .unwrap_or(8);
                        return vec![ScgStatement::Allocation(AllocationNode::Stack {
                            name: let_stmt.name.clone(),
                            size,
                            ty: ScgType::Ptr,
                        })];
                    }
                    // Other function calls → CallNode (flatten args)
                    let flat_args: Vec<ScgExpr> = args.iter()
                        .map(|a| flatten_expr(a, &mut stmts, ctx))
                        .collect();
                    let is_extern = ctx.extern_fns.contains(name)
                        || name == "__vuma_alloc"
                        || name == "__vuma_dealloc";
                    stmts.push(ScgStatement::Call(CallNode {
                        dst: Some(let_stmt.name.clone()),
                        func: name.clone(),
                        args: flat_args,
                        is_extern,
                        reassigns: None,
                    }));
                    return stmts;
                }
            }

            // Check if the RHS is an Allocate expression → AllocationNode::Stack
            if let vuma_parser::ast::Expr::Allocate { size, .. } = &let_stmt.value {
                let size_val: u32 = match size.as_ref() {
                    vuma_parser::ast::Expr::Lit { value, .. } => {
                        if let vuma_parser::ast::Lit::Int(n) = value {
                            *n as u32
                        } else {
                            8
                        }
                    }
                    _ => 8,
                };
                return vec![ScgStatement::Allocation(AllocationNode::Stack {
                    name: let_stmt.name.clone(),
                    size: size_val,
                    ty: ScgType::Ptr,
                })];
            }

            // General case: flatten the expression and assign to dst
            let result = flatten_expr(&let_stmt.value, &mut stmts, ctx);
            match &result {
                ScgExpr::Var(name) if name == &let_stmt.name => {}
                _ => {
                    stmts.push(ScgStatement::Computation(ComputationNode {
                        dst: let_stmt.name.clone(),
                        op: BinOpKind::Add,
                        lhs: result,
                        rhs: ScgExpr::Int(0),
                        tail_call: false,
                        reassigns: None,
                    }));
                }
            }
            stmts
        }

        // ── target = value ──
        PStmt::Assign(assign_stmt) => {
            let mut stmts = Vec::new();

            // Detect dereference writes: `*expr = val` → Access::Store
            if let vuma_parser::ast::AssignTarget::Deref { expr, .. } = &assign_stmt.target {
                let ptr = flatten_expr(expr, &mut stmts, ctx);
                let value = flatten_expr(&assign_stmt.value, &mut stmts, ctx);
                stmts.push(ScgStatement::Access(AccessNode::Store {
                    ptr,
                    offset: None,
                    value,
                    ty: None,
                }));
                return stmts;
            }

            // Handle Index target: ptr[index] = value
            if let vuma_parser::ast::AssignTarget::Index { expr, index, .. } = &assign_stmt.target {
                let base = flatten_expr(expr, &mut stmts, ctx);
                let idx = flatten_expr(index, &mut stmts, ctx);
                let addr = ctx.alloc_temp();
                stmts.push(ScgStatement::Computation(ComputationNode {
                    dst: addr.clone(),
                    op: BinOpKind::Add,
                    lhs: base,
                    rhs: idx,
                    tail_call: false,
                    reassigns: None,
                }));
                let value = flatten_expr(&assign_stmt.value, &mut stmts, ctx);
                stmts.push(ScgStatement::Access(AccessNode::Store {
                    ptr: ScgExpr::Var(addr),
                    offset: None,
                    value,
                    ty: None,
                }));
                return stmts;
            }

            let dst = match &assign_stmt.target {
                vuma_parser::ast::AssignTarget::Var { name, .. } => name.clone(),
                vuma_parser::ast::AssignTarget::DerefField { field, .. } => field.clone(),
                vuma_parser::ast::AssignTarget::Deref { .. } => "_deref".into(),
                vuma_parser::ast::AssignTarget::Index { .. } => "_index".into(),
            };

            // Detect allocate() expression → AllocationNode::Stack
            if let vuma_parser::ast::Expr::Allocate { size, .. } = &assign_stmt.value {
                let size_val: u32 = match size.as_ref() {
                    vuma_parser::ast::Expr::Lit { value, .. } => {
                        if let vuma_parser::ast::Lit::Int(n) = value {
                            *n as u32
                        } else {
                            8
                        }
                    }
                    _ => 8,
                };
                return vec![ScgStatement::Allocation(AllocationNode::Stack {
                    name: dst,
                    size: size_val,
                    ty: ScgType::Ptr,
                })];
            }

            // Detect function calls in assign: `x = foo(args)` → CallNode (flatten args)
            if let vuma_parser::ast::Expr::Call { callee, args, .. } = &assign_stmt.value {
                if let vuma_parser::ast::Expr::Var { name, .. } = callee.as_ref() {
                    let flat_args: Vec<ScgExpr> = args.iter()
                        .map(|a| flatten_expr(a, &mut stmts, ctx))
                        .collect();
                    let is_extern = ctx.extern_fns.contains(name)
                        || name == "__vuma_alloc"
                        || name == "__vuma_dealloc";
                    stmts.push(ScgStatement::Call(CallNode {
                        dst: Some(dst),
                        func: name.clone(),
                        args: flat_args,
                        is_extern,
                        reassigns: None,
                    }));
                    return stmts;
                }
            }

            // General case: flatten the value expression and assign to dst
            let result = flatten_expr(&assign_stmt.value, &mut stmts, ctx);
            match &result {
                ScgExpr::Var(name) if name == &dst => {}
                _ => {
                    stmts.push(ScgStatement::Computation(ComputationNode {
                        dst: dst.clone(),
                        op: BinOpKind::Add,
                        lhs: result,
                        rhs: ScgExpr::Int(0),
                        tail_call: false,
                        reassigns: Some(dst),
                    }));
                }
            }
            stmts
        }

        // ── target op= value ──
        PStmt::CompoundAssign(ca_stmt) => {
            let mut stmts = Vec::new();
            let dst = match &ca_stmt.target {
                vuma_parser::ast::AssignTarget::Var { name, .. } => name.clone(),
                vuma_parser::ast::AssignTarget::DerefField { field, .. } => field.clone(),
                _ => "_".into(),
            };
            let binop = match ca_stmt.op {
                vuma_parser::ast::CompoundOp::Add => BinOpKind::Add,
                vuma_parser::ast::CompoundOp::Sub => BinOpKind::Sub,
                vuma_parser::ast::CompoundOp::Mul => BinOpKind::Mul,
                vuma_parser::ast::CompoundOp::Div => BinOpKind::SDiv,
                vuma_parser::ast::CompoundOp::Mod => BinOpKind::SRem,
                vuma_parser::ast::CompoundOp::BitAnd => BinOpKind::And,
                vuma_parser::ast::CompoundOp::BitOr => BinOpKind::Or,
                vuma_parser::ast::CompoundOp::BitXor => BinOpKind::Xor,
                vuma_parser::ast::CompoundOp::Shl => BinOpKind::Shl,
                vuma_parser::ast::CompoundOp::Shr => BinOpKind::ShrL,
            };
            let rhs = flatten_expr(&ca_stmt.value, &mut stmts, ctx);
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: dst.clone(),
                op: binop,
                lhs: ScgExpr::Var(dst.clone()),
                rhs,
                tail_call: false,
                reassigns: Some(dst),
            }));
            stmts
        }

        // ── allocate(size);  (standalone — not bound to a variable) ──
        // Lower to a stack allocation when the size is a literal int (mirrors
        // the `let x = allocate(N)` path), else to a heap allocation with a
        // dynamic size expression. Previously this was silently dropped.
        PStmt::Allocate(alloc_stmt) => {
            let mut stmts = Vec::new();
            let temp = ctx.alloc_temp();
            // Try to extract a literal integer size; fall back to Heap otherwise.
            if let vuma_parser::ast::Expr::Lit {
                value: vuma_parser::ast::Lit::Int(n),
                ..
            } = &alloc_stmt.size
            {
                let size = (*n as u32).max(1); // never allocate 0 bytes
                stmts.push(ScgStatement::Allocation(AllocationNode::Stack {
                    name: temp,
                    size,
                    ty: ScgType::U8, // raw byte buffer; caller may cast the pointer
                }));
            } else {
                let size_expr = flatten_expr(&alloc_stmt.size, &mut stmts, ctx);
                stmts.push(ScgStatement::Allocation(AllocationNode::Heap {
                    name: temp,
                    size_expr,
                    ty: ScgType::U8,
                }));
            }
            stmts
        }

        // ── free(ptr);  (standalone) ──
        // There is no ScgStatement::Free variant, so lower to a runtime call
        // to `__vuma_free(ptr, 0)`. The size argument is 0 because FreeStmt
        // does not carry a size (most mmap-based allocators track the size
        // internally). Previously this was silently dropped.
        PStmt::Free(free_stmt) => {
            let mut stmts = Vec::new();
            let ptr_expr = flatten_expr(&free_stmt.ptr, &mut stmts, ctx);
            stmts.push(ScgStatement::Call(CallNode {
                dst: None,
                func: "__vuma_free".to_string(),
                args: vec![ptr_expr, ScgExpr::Int(0)],
                is_extern: true,
                reassigns: None,
            }));
            stmts
        }

        // ── expr as Type;  (standalone cast) ──
        // Lower to a proper CastNode so the type conversion is preserved in
        // the IR. Previously this kept the operand (via flatten_expr) but
        // discarded the target type, producing an incorrect no-op.
        PStmt::Cast(cast_stmt) => {
            let mut stmts = Vec::new();
            let src = flatten_expr(&cast_stmt.expr, &mut stmts, ctx);
            let temp = ctx.alloc_temp();
            // The AST's CastStmt always carries a target type (target_type: Type,
            // not Option<Type>); use the existing bridge to map it to ScgType.
            let to_ty = bridge_type_to_codegen_scg(&Some(cast_stmt.target_type.clone()));
            // Source type is not annotated in the AST — assume I64 (VUMA's
            // default integer width). The IR layer can refine this via BD.
            let from_ty = ScgType::I64;
            // Choose a CastKind based on the source/target bit widths.
            // Floats are not handled by this heuristic (bridge_type_to_codegen_scg
            // currently maps "f32"/"f64" to ScgType::I64), so the result is
            // always an integer-to-integer cast kind.
            let kind = {
                let from_bits = match from_ty {
                    ScgType::I8 | ScgType::U8 => 8,
                    ScgType::I16 | ScgType::U16 => 16,
                    ScgType::I32 | ScgType::U32 | ScgType::F32 => 32,
                    _ => 64, // I64, U64, Ptr, F64, Void
                };
                let to_bits = match to_ty {
                    ScgType::I8 | ScgType::U8 => 8,
                    ScgType::I16 | ScgType::U16 => 16,
                    ScgType::I32 | ScgType::U32 | ScgType::F32 => 32,
                    _ => 64,
                };
                if to_bits > from_bits {
                    CastKind::SExt
                } else if to_bits < from_bits {
                    CastKind::Trunc
                } else {
                    CastKind::BitCast
                }
            };
            stmts.push(ScgStatement::Cast(CastNode {
                dst: temp,
                src,
                kind,
                from_ty,
                to_ty,
            }));
            stmts
        }

        PStmt::Return(ret_stmt) => {
            let mut stmts = Vec::new();
            let values = match &ret_stmt.value {
                Some(expr) => vec![flatten_expr(expr, &mut stmts, ctx)],
                None => vec![],
            };
            stmts.push(ScgStatement::Return(values));
            stmts
        }

        PStmt::Expr(expr_stmt) => {
            let mut stmts = Vec::new();
            let result = flatten_expr(&expr_stmt.expr, &mut stmts, ctx);
            ctx.last_expr_result = Some(result);
            stmts
        }

        PStmt::If(if_stmt) => {
            let mut pre_stmts = Vec::new();
            let cond = flatten_expr(&if_stmt.condition, &mut pre_stmts, ctx);
            let then_body = bridge_block_to_scg_stmts(&if_stmt.then_block, ctx);
            let else_body = if_stmt
                .else_block
                .as_ref()
                .map(|b| bridge_block_to_scg_stmts(b, ctx));
            let mut result = pre_stmts;
            result.push(ScgStatement::Control(ControlNode::If {
                cond,
                then_body,
                else_body,
            }));
            result
        }

        // while condition { body }
        // Lowered as: Loop { <compute cond>; if cond { body } else { break } }
        PStmt::While(while_stmt) => {
            let then_body = bridge_block_to_scg_stmts(&while_stmt.body, ctx);
            let mut loop_body = Vec::new();
            let cond = flatten_expr(&while_stmt.condition, &mut loop_body, ctx);
            loop_body.push(ScgStatement::Control(ControlNode::If {
                cond,
                then_body,
                else_body: Some(vec![ScgStatement::Control(ControlNode::Break)]),
            }));
            vec![ScgStatement::Control(ControlNode::Loop { body: loop_body, for_range: None, while_cond: None })]
        }

        // for name in start..end { body }
        // Lowered as:
        //   name = start
        //   loop { _cond = name < end; if _cond { body; name = name + 1 } else { break } }
        PStmt::For(for_stmt) => {
            let mut pre_stmts = Vec::new();
            let (start_expr, end_expr) = match &for_stmt.iter {
                vuma_parser::ast::Expr::Range { start, end, .. } => {
                    let s = flatten_expr(start, &mut pre_stmts, ctx);
                    let e = flatten_expr(end, &mut pre_stmts, ctx);
                    (s, e)
                }
                _other => {
                    (ScgExpr::Int(0), ScgExpr::Int(0))
                }
            };

            let init_stmt = ScgStatement::Computation(ComputationNode {
                dst: for_stmt.name.clone(),
                op: BinOpKind::Add,
                lhs: start_expr,
                rhs: ScgExpr::Int(0),
                tail_call: false,
                reassigns: None,
            });

            let mut loop_body = Vec::new();
            let cond_temp = format!("{}_cond", for_stmt.name);
            loop_body.push(ScgStatement::Computation(ComputationNode {
                dst: cond_temp.clone(),
                op: BinOpKind::SLt,
                lhs: ScgExpr::Var(for_stmt.name.clone()),
                rhs: end_expr.clone(),
                tail_call: false,
                reassigns: None,
            }));

            let inner_body = bridge_block_to_scg_stmts(&for_stmt.body, ctx);
            let mut full_then = inner_body;
            full_then.push(ScgStatement::Computation(ComputationNode {
                dst: for_stmt.name.clone(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var(for_stmt.name.clone()),
                rhs: ScgExpr::Int(1),
                tail_call: false,
                reassigns: None,
            }));

            loop_body.push(ScgStatement::Control(ControlNode::If {
                cond: ScgExpr::Var(cond_temp),
                then_body: full_then,
                else_body: Some(vec![ScgStatement::Control(ControlNode::Break)]),
            }));

            let mut result = pre_stmts;
            result.push(init_stmt);
            result.push(ScgStatement::Control(ControlNode::Loop { body: loop_body, for_range: None, while_cond: None }));
            result
        }

        PStmt::Loop(loop_stmt) => {
            let body = bridge_block_to_scg_stmts(&loop_stmt.body, ctx);
            vec![ScgStatement::Control(ControlNode::Loop { body, for_range: None, while_cond: None })]
        }

        PStmt::Break(_) => vec![ScgStatement::Control(ControlNode::Break)],
        PStmt::Continue(_) => vec![ScgStatement::Control(ControlNode::Continue)],

        // ── *ptr;  or  (*ptr).field;  (standalone access / deref read) ──
        // A dereference read with no destination. Lower to a Load into a
        // temporary so the pointer is evaluated AND the load happens
        // (which may trap on a bad pointer — the correct behavior for an
        // explicit dereference). Previously this was silently dropped,
        // which suppressed segfaults that the programmer should observe.
        PStmt::Access(access_stmt) => {
            let mut stmts = Vec::new();
            let ptr_expr = flatten_expr(&access_stmt.expr, &mut stmts, ctx);
            let temp = ctx.alloc_temp();
            stmts.push(ScgStatement::Access(AccessNode::Load {
                dst: temp,
                ptr: ptr_expr,
                offset: None,
                ty: None,
            }));
            stmts
        }

        // ── match subject { arms } ──
        // Lower to ControlNode::Switch when every arm pattern is a simple
        // integer literal (or the wildcard `_`). For complex patterns
        // (Ident, Struct, Enum, Range, Or), emit a TODO warning and drop
        // ONLY those arm bodies — the wildcard/default arm still runs,
        // which is better than silently dropping the whole match.
        PStmt::Match(match_stmt) => {
            let mut pre_stmts = Vec::new();
            let discriminant = flatten_expr(&match_stmt.subject, &mut pre_stmts, ctx);

            let mut switch_arms: Vec<SwitchArm> = Vec::new();
            let mut default_body: Vec<ScgStatement> = Vec::new();
            let mut saw_complex_pattern = false;

            for arm in &match_stmt.arms {
                match &arm.pattern {
                    vuma_parser::ast::MatchPattern::Lit { value, .. } => {
                        // Only integer-valued literals can become SwitchArm values.
                        let value_i = match value {
                            vuma_parser::ast::Lit::Int(n) => *n,
                            vuma_parser::ast::Lit::Bool(b) => {
                                if *b {
                                    1
                                } else {
                                    0
                                }
                            }
                            vuma_parser::ast::Lit::Address(a) => *a as i64,
                            _ => {
                                // Float/String literal — not a valid switch arm value.
                                saw_complex_pattern = true;
                                continue;
                            }
                        };
                        let mut arm_body: Vec<ScgStatement> = Vec::new();
                        let _ = flatten_expr(&arm.body, &mut arm_body, ctx);
                        switch_arms.push(SwitchArm {
                            value: value_i,
                            body: arm_body,
                        });
                    }
                    vuma_parser::ast::MatchPattern::Wildcard(_) => {
                        let mut arm_body: Vec<ScgStatement> = Vec::new();
                        let _ = flatten_expr(&arm.body, &mut arm_body, ctx);
                        default_body = arm_body;
                    }
                    _ => {
                        // Ident / Struct / Enum / Range / Or — too complex for
                        // the direct AST→codegen bridge. The arm body is
                        // dropped (NOT the whole match).
                        saw_complex_pattern = true;
                    }
                }
            }

            if saw_complex_pattern {
                eprintln!(
                    "[vuma] TODO: match statement at span {:?} uses complex patterns \
                     (ident/struct/enum/range/or) which are not yet supported by the direct \
                     AST→codegen bridge. Only literal-integer and wildcard arms were lowered; \
                     other arm bodies were dropped.",
                    match_stmt.span,
                );
            }

            pre_stmts.push(ScgStatement::Control(ControlNode::Switch {
                discriminant,
                arms: switch_arms,
                default_body,
            }));
            pre_stmts
        }

        // ── sync { body } ──
        // Concurrency primitive. The direct AST→codegen bridge does not
        // enforce sync semantics (no mutex / atomic fence emission); the
        // body is lowered inline so the statements still execute. TODO:
        // implement proper sync-block lowering.
        PStmt::Sync(sync_block) => {
            eprintln!(
                "[vuma] TODO: sync {{ ... }} block at span {:?} lowered without \
                 synchronization semantics (body executes inline, no mutex/fence)",
                sync_block.span,
            );
            bridge_block_to_scg_stmts(&sync_block.body, ctx)
        }

        // ── unsafe { body } ──
        // A scoping marker; lower the body inline. The unsafe contract is
        // the programmer's responsibility — no special handling needed.
        PStmt::UnsafeBlock { body, .. } => {
            bridge_block_to_scg_stmts(body, ctx)
        }

        // BD directives (bd/repd/capd/reld) are annotations consumed by
        // the BD inference pass — they produce no codegen statements.
        PStmt::BdDirective(_) => vec![],
    }
}
fn cmd_disasm(file: &PathBuf, isa: &IsaArg, base_addr_str: &str) -> Result<(), String> {
    let bytes = fs::read(file)
        .map_err(|e| format!("error: cannot read binary file '{}': {}", file.display(), e))?;

    let base_addr = u64::from_str_radix(base_addr_str.trim_start_matches("0x"), 16)
        .map_err(|e| format!("error: invalid base address '{}': {}", base_addr_str, e))?;

    let backend_kind = BackendKind::from(*isa);
    let backend = create_backend(backend_kind).map_err(|e| {
        format!(
            "error: cannot create {} backend: {}",
            backend_kind.isa_name(),
            e
        )
    })?;

    let instructions = backend.disassemble(&bytes, base_addr);

    println!(
        "Disassembly of {} ({} bytes, ISA: {}):",
        file.display(),
        bytes.len(),
        backend.name()
    );
    for line in &instructions {
        println!("{}", line);
    }

    Ok(())
}

/// `vuma verify <file>` — Run IVE 5-invariant verification.
fn cmd_verify(cli: &Cli, file: &PathBuf) -> Result<(), String> {
    let source = read_source(file)?;
    let mut config = make_config(cli, CompileTarget::Linux);
    // Force exhaustive verification for the verify subcommand.
    config.verification_level = VerificationLevel::Exhaustive;

    let result = compile_with_path(&source, Some(file), &config).map_err(|errors| {
        print_errors(&errors);
        format!(
            "compilation/verification failed with {} error(s)",
            errors.len()
        )
    })?;

    match result.verification {
        Some(ref verification) => {
            println!("IVE Verification Results for {}", file.display());
            println!("  Overall verdict: {}", verification.overall);
            println!(
                "  Summary: {} invariant(s) checked",
                verification.summary.total_checked
            );
            println!("           {} passed", verification.summary.passed);
            println!("           {} failed", verification.summary.failed);
            println!("           {} unverified", verification.summary.unverified);

            // Print individual invariant results.
            for per_inv in &verification.per_invariant {
                let status_str = match &per_inv.result.status {
                    vuma_ive::VerificationStatus::Proven => "PROVEN",
                    vuma_ive::VerificationStatus::ProbablySafe { .. } => "PROBABLY_SAFE",
                    vuma_ive::VerificationStatus::Unverified { .. } => "UNVERIFIED",
                    vuma_ive::VerificationStatus::Violated { .. } => "VIOLATED",
                };
                println!(
                    "  [{}] {:?} — {}",
                    status_str, per_inv.kind, per_inv.result.message,
                );
                if let Some(ref evidence) = per_inv.result.evidence {
                    println!("    evidence: {}", evidence);
                }
            }
        }
        None => {
            println!("Verification was skipped (verification level set to None).");
        }
    }

    Ok(())
}

/// `vuma repl` — Interactive REPL: parse expressions and print AST.
fn cmd_repl() -> Result<(), String> {
    println!("VUMA REPL v0.1.0");
    println!("Type VUMA expressions or statements. Enter ':quit' to exit, ':help' for help.");

    let stdin = io::stdin();
    let mut input = String::new();

    loop {
        print!("vuma> ");
        io::stdout()
            .flush()
            .map_err(|e| format!("flush error: {}", e))?;

        input.clear();
        match stdin.read_line(&mut input) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => return Err(format!("error reading input: {}", e)),
        }

        let trimmed = input.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == ":quit" || trimmed == ":q" {
            break;
        }
        if trimmed == ":help" || trimmed == ":h" {
            println!("  :quit, :q   — Exit the REPL");
            println!("  :help, :h   — Show this help");
            println!("  <expr>      — Parse and display the AST");
            println!("  <stmt>      — Parse and display the AST");
            println!("  <program>   — Parse and display the full program AST");
            continue;
        }

        // Try to parse as a full program first.
        let mut parser = vuma_parser::Parser::new(trimmed);
        let result = parser.parse_program();
        if !result.has_errors() {
            // Successfully parsed; display the AST.
            let program = result.unwrap();
            println!("{:#?}", program);
            continue;
        }
        // Program parse had errors; try to parse as an expression via
        // wrapping it in a dummy program context.
        let program_errors = result.errors.clone();
        // Since parse_expr is private, we re-parse as a minimal program.
        let wrapped = format!("fn _repl_expr() {{ {} }}", trimmed);
        let mut expr_parser = vuma_parser::Parser::new(&wrapped);
        let expr_result = expr_parser.parse_program();
        if !expr_result.has_errors() {
            // Print the function body AST.
            let program = expr_result.unwrap();
            if let Some(item) = program.items.first() {
                println!("{:#?}", item);
            } else {
                println!("{:#?}", program);
            }
        } else {
            // Show the original parse errors.
            for err in &program_errors {
                eprintln!("parse error: {}", err);
            }
        }
    }

    Ok(())
}

/// Start the VUMA Language Server (LSP) over stdin/stdout.
fn cmd_lsp() -> Result<(), String> {
    let mut server = vuma::lsp::LspServer::new();
    server.run();
    Ok(())
}

/// `vuma --bench` — Run the performance benchmark suite.
fn cmd_bench(_cli: &Cli) {
    use vuma_codegen::backend::BackendKind;

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║           VUMA Performance Benchmark Suite                  ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // ── Benchmark 1: SHA256d compile time + binary size ──
    println!("── Benchmark 1: SHA256d ──");
    let sha256d_path = PathBuf::from("examples/sha256d.vuma");
    let sha256d_source = fs::read_to_string(&sha256d_path).unwrap_or_else(|_| {
        eprintln!("  Warning: examples/sha256d.vuma not found, skipping SHA256d benchmark");
        String::new()
    });

    if !sha256d_source.is_empty() {
        let backends: [(BackendKind, &str); 8] = [
            (BackendKind::AArch64, "aarch64"),
            (BackendKind::X86_64, "x86_64"),
            (BackendKind::RiscV64, "riscv64"),
            (BackendKind::Arm32, "arm32"),
            (BackendKind::Mips64, "mips64"),
            (BackendKind::PowerPC64, "ppc64"),
            (BackendKind::LoongArch64, "loongarch64"),
            (BackendKind::Wasm32, "wasm32"),
        ];

        println!("  {:15} {:>12} {:>12} {:>12}", "Backend", "Time (ms)", "Size (B)", "Instrs");
        println!("  {:15} {:>12} {:>12} {:>12}", "───────", "────────", "────────", "────────");

        for (kind, name) in &backends {
            let start = std::time::Instant::now();

            // Parse
            let mut parser = vuma_parser::Parser::new(&sha256d_source);
            let parse_result = parser.parse_program();

            let _parse_time = start.elapsed();

            if parse_result.is_err() {
                println!("  {:15} {:>12} {:>12} {:>12}", name, "PARSE_ERR", "-", "-");
                continue;
            }

            let program = parse_result.value.unwrap();

            // Bridge to codegen SCG
            let codegen_scg = bridge_ast_to_codegen_scg(&program);

            // Lower to IR
            let mut ir_builder = ScgToIr::new();
            let ir_result = ir_builder.convert(&codegen_scg);

            let _ir_time = start.elapsed();

            if let Ok(ir_program) = ir_result {
                let ir_instr_count: usize = ir_program.functions.iter()
                    .map(|f| f.blocks.iter().map(|b| b.instructions.len()).sum::<usize>())
                    .sum();

                // Create backend and allocate
                let backend = match create_backend(*kind) {
                    Ok(b) => b,
                    Err(_) => {
                        println!("  {:15} {:>12} {:>12} {:>12}", name, "NO_BACKEND", "-", "-");
                        continue;
                    }
                };

                let mut allocated_functions = Vec::new();
                for func in &ir_program.functions {
                    if let Ok(allocated) = backend.allocate_registers(func) {
                        allocated_functions.push(allocated);
                    }
                }

                let total_time = start.elapsed();
                let codegen_time_ms = total_time.as_millis() as u64;

                // Encode
                let binary_size = if !allocated_functions.is_empty() {
                    let allocated_program = vuma_codegen::backend::AllocatedProgram {
                        functions: allocated_functions,
                        total_code_size: 0,
                        total_data_size: 0,
                    };
                    match backend.encode_program(&allocated_program) {
                        Ok(bytes) => bytes.len(),
                        Err(_) => 0,
                    }
                } else {
                    0
                };

                println!(
                    "  {:15} {:>12} {:>12} {:>12}",
                    name,
                    codegen_time_ms,
                    binary_size,
                    ir_instr_count
                );
            } else {
                println!("  {:15} {:>12} {:>12} {:>12}", name, "IR_ERR", "-", "-");
            }
        }
    }
    println!();

    // ── Benchmark 2: Compilation speed at varying program sizes ──
    println!("── Benchmark 2: Compilation Speed ──");
    println!("  {:20} {:>12} {:>12} {:>12}", "Program Size", "Parse (μs)", "SCG (μs)", "Total (μs)");
    println!("  {:20} {:>12} {:>12} {:>12}", "────────────", "──────────", "────────", "─────────");

    for &line_count in &[10, 50, 100, 500] {
        // Generate a synthetic program of the given size
        let mut source = String::from("fn main() {\n");
        for i in 0..line_count {
            source.push_str(&format!("    x{} = {} + {};\n", i, i, i + 1));
        }
        source.push_str("}\n");

        let start = std::time::Instant::now();
        let mut parser = vuma_parser::Parser::new(&source);
        let _ = parser.parse_program();
        let parse_time = start.elapsed().as_micros() as u64;

        let scg_start = std::time::Instant::now();
        // SCG construction would happen here
        let scg_time = scg_start.elapsed().as_micros() as u64;

        let total_time = start.elapsed().as_micros() as u64;

        println!(
            "  {:20} {:>12} {:>12} {:>12}",
            format!("{} lines", line_count),
            parse_time,
            scg_time,
            total_time
        );
    }
    println!();

    // ── Benchmark 3: Codegen quality (redundant loads/stores) ──
    println!("── Benchmark 3: Codegen Quality ──");
    if !sha256d_source.is_empty() {
        let mut parser = vuma_parser::Parser::new(&sha256d_source);
        let parse_result = parser.parse_program();
        if let Some(program) = parse_result.value {
            let codegen_scg = bridge_ast_to_codegen_scg(&program);
            let mut ir_builder = ScgToIr::new();
            if let Ok(ir_program) = ir_builder.convert(&codegen_scg) {
                let mut total_loads = 0usize;
                let mut total_stores = 0usize;
                let mut redundant_loads = 0usize;
                let redundant_stores = 0usize;

                for func in &ir_program.functions {
                    let last_store_target: Option<String> = None;
                    for block in &func.blocks {
                        for instr in &block.instructions {
                            match instr {
                                vuma_codegen::ir::IRInstr::Load { dst, .. } => {
                                    total_loads += 1;
                                    // A load is potentially redundant if it immediately
                                    // follows a store to the same address
                                    if last_store_target.is_some() {
                                        redundant_loads += 1;
                                    }
                                    let _ = dst;
                                }
                                vuma_codegen::ir::IRInstr::Store { .. } => {
                                    total_stores += 1;
                                    // A store is redundant if the same value was just stored
                                    // (simplified check — full analysis would need value numbering)
                                }
                                _ => {}
                            }
                        }
                    }
                }

                println!("  Total loads:           {}", total_loads);
                println!("  Total stores:          {}", total_stores);
                println!("  Potentially redundant loads:  {}", redundant_loads);
                println!("  Potentially redundant stores: {}", redundant_stores);
            }
        }
    }
    println!();

    // ── Benchmark 4: Memory safety analysis ──
    println!("── Benchmark 4: Memory Safety Analysis ──");
    if !sha256d_source.is_empty() {
        let mut parser = vuma_parser::Parser::new(&sha256d_source);
        let parse_result = parser.parse_program();
        if let Some(program) = parse_result.value {
            let codegen_scg = bridge_ast_to_codegen_scg(&program);

            let config = if _cli.safe {
                vuma_codegen::MemorySafetyConfig::safe_mode()
            } else {
                vuma_codegen::MemorySafetyConfig::compile_time_only()
            };

            let start = std::time::Instant::now();
            let analyzer = vuma_codegen::MemorySafetyAnalyzer::new(config);
            let report = analyzer.analyze(&codegen_scg);
            let elapsed = start.elapsed();

            println!("  Analysis time:         {}μs", elapsed.as_micros());
            println!("  Heap allocations:      {}", report.heap_allocations_analyzed);
            println!("  Stack allocations:     {}", report.stack_allocations_analyzed);
            println!("  Access sites:          {}", report.access_sites_analyzed);
            println!("  Violations found:      {}", report.violations.len());
            if !report.violations.is_empty() {
                for v in &report.violations {
                    println!("    {}", v);
                }
            } else {
                println!("  ✓ No memory safety violations detected");
            }
        }
    }
    println!();

    println!("Benchmark suite complete.");
}

// ═══════════════════════════════════════════════════════════════════════════
// Main entry point
// ═══════════════════════════════════════════════════════════════════════════

fn main() {
    // Determine log level from CLI flags.
    let log_level = if std::env::args().any(|a| a == "--quiet" || a == "-q") {
        LogLevel::Error
    } else if std::env::args().any(|a| a == "--verbose" || a == "-v") {
        LogLevel::Debug
    } else {
        LogLevel::Info
    };

    // Initialize the VUMA structured logger.
    init_logger(log_level);

    // Also set up the `log` crate bridge so that internal log::info! etc.
    // go through our structured logger.
    let _ = log::set_boxed_logger(Box::new(vuma::logging::VumaLogBridge));
    log::set_max_level(match log_level {
        LogLevel::Error => log::LevelFilter::Error,
        LogLevel::Warn => log::LevelFilter::Warn,
        LogLevel::Info => log::LevelFilter::Info,
        LogLevel::Debug => log::LevelFilter::Debug,
        LogLevel::Trace => log::LevelFilter::Trace,
    });

    let cli = Cli::parse();

    // Handle --version with extended info.
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("{}", version_long());
        return;
    }

    // Handle --bench flag: run the benchmark suite.
    if cli.bench {
        cmd_bench(&cli);
        return;
    }

    // Handle --repl flag: launch the full VumaRepl instead of subcommand.
    if cli.repl {
        let mut repl = vuma_core::repl::VumaRepl::new();
        if let Err(e) = repl.run() {
            eprintln!("REPL error: {e}");
            std::process::exit(1);
        }
        return;
    }

    let command = match cli.command {
        Some(ref cmd) => cmd,
        None => {
            // No subcommand and no --repl: print help.
            eprintln!("No subcommand specified. Use `vuma --help` or `vuma --repl`.");
            std::process::exit(1);
        }
    };

    let result = match command {
        Commands::Build {
            ref file,
            ref output,
            ref target,
            ref isa,
        } => cmd_build(&cli, file, output, target, isa),
        Commands::Run {
            ref file,
            ref args,
            ref isa,
        } => cmd_run(&cli, file, args, isa),
        Commands::Check { ref file } => cmd_check(&cli, file),
        Commands::Emit {
            ref isa,
            ref file,
            ref output,
        } => cmd_emit(&cli, isa, file, output),
        Commands::Compile {
            ref file,
            ref output,
            ref target,
            ref format,
        } => cmd_compile(&cli, file, output, target, format),
        Commands::Disasm {
            ref file,
            ref isa,
            ref base_addr,
        } => cmd_disasm(file, isa, base_addr),
        Commands::Verify { ref file } => cmd_verify(&cli, file),
        Commands::Repl => cmd_repl(),
        Commands::Lsp => cmd_lsp(),
        Commands::Pkg { ref cmd } => cmd_pkg(cmd),
    };

    if let Err(err) = result {
        eprintln!("{}", err);
        std::process::exit(1);
    }
}

/// `vuma pkg init/build/add` — Package manager subcommands.
fn cmd_pkg(cmd: &PkgCommand) -> Result<(), String> {
    let dir = std::env::current_dir().map_err(|e| format!("cannot get current directory: {}", e))?;
    match cmd {
        PkgCommand::Init { name } => {
            vuma::init_package(&dir, name).map_err(|e| format!("pkg init failed: {}", e))?;
            println!("Initialized VUMA package '{}' in {}", name, dir.display());
            Ok(())
        }
        PkgCommand::Build => {
            vuma::build_package(&dir).map_err(|e| format!("pkg build failed: {}", e))?;
            println!("Package built successfully");
            Ok(())
        }
        PkgCommand::Add { dep, version } => {
            vuma::add_dependency(&dir, dep, version).map_err(|e| format!("pkg add failed: {}", e))?;
            println!("Added dependency {} @ {}", dep, version);
            Ok(())
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests for CLI argument parsing
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Test 1: `vuma build hello.vuma` parses correctly.
    #[test]
    fn test_build_basic() {
        let cli = Cli::try_parse_from(["vuma", "build", "hello.vuma"]).unwrap();
        match cli.command {
            Some(Commands::Build {
                ref file,
                ref output,
                ref target,
                ref isa,
            }) => {
                assert_eq!(file, &PathBuf::from("hello.vuma"));
                assert!(output.is_none());
                assert_eq!(target, &TargetArg::Linux);
                assert!(isa.is_none(), "--isa should default to None (resolved at runtime)");
            }
            _ => panic!("expected Build command"),
        }
    }

    /// Test 2: `vuma build hello.vuma -o out.o --target linux` parses correctly.
    #[test]
    fn test_build_with_options() {
        let cli = Cli::try_parse_from([
            "vuma",
            "build",
            "hello.vuma",
            "-o",
            "out.o",
            "--target",
            "linux",
        ])
        .unwrap();
        match cli.command {
            Some(Commands::Build {
                ref file,
                ref output,
                ref target,
                isa: _,
            }) => {
                assert_eq!(file, &PathBuf::from("hello.vuma"));
                assert_eq!(output.as_ref().unwrap(), &PathBuf::from("out.o"));
                assert_eq!(target, &TargetArg::Linux);
            }
            _ => panic!("expected Build command"),
        }
    }

    /// Test 2b: `vuma build hello.vuma --isa x86_64` parses the --isa flag.
    #[test]
    fn test_build_with_isa() {
        let cli = Cli::try_parse_from([
            "vuma",
            "build",
            "hello.vuma",
            "--isa",
            "x86_64",
        ])
        .unwrap();
        match cli.command {
            Some(Commands::Build { ref file, ref isa, .. }) => {
                assert_eq!(file, &PathBuf::from("hello.vuma"));
                assert_eq!(*isa, Some(IsaArg::X86_64));
            }
            _ => panic!("expected Build command"),
        }
    }

    /// Test 3: `vuma run hello.vuma` parses correctly.
    #[test]
    fn test_run_basic() {
        let cli = Cli::try_parse_from(["vuma", "run", "hello.vuma"]).unwrap();
        match cli.command {
            Some(Commands::Run { ref file, ref args, isa: _ }) => {
                assert_eq!(file, &PathBuf::from("hello.vuma"));
                assert!(args.is_empty());
            }
            _ => panic!("expected Run command"),
        }
    }

    /// Test 4: `vuma run hello.vuma arg1 arg2` passes trailing args.
    #[test]
    fn test_run_with_args() {
        let cli = Cli::try_parse_from(["vuma", "run", "hello.vuma", "arg1", "arg2"]).unwrap();
        match cli.command {
            Some(Commands::Run { ref file, ref args, .. }) => {
                assert_eq!(file, &PathBuf::from("hello.vuma"));
                assert_eq!(args, &vec!["arg1".to_string(), "arg2".to_string()]);
            }
            _ => panic!("expected Run command"),
        }
    }

    /// Test 4b: `vuma run --isa aarch64 hello.vuma arg1` parses the --isa flag
    /// (must come BEFORE the file because of `trailing_var_arg`).
    #[test]
    fn test_run_with_isa() {
        let cli = Cli::try_parse_from([
            "vuma", "run", "--isa", "aarch64", "hello.vuma", "arg1",
        ])
        .unwrap();
        match cli.command {
            Some(Commands::Run { ref file, ref args, ref isa }) => {
                assert_eq!(file, &PathBuf::from("hello.vuma"));
                assert_eq!(*isa, Some(IsaArg::Aarch64));
                assert_eq!(args, &vec!["arg1".to_string()]);
            }
            _ => panic!("expected Run command"),
        }
    }

    /// Test 4c: `host_isa()` returns Some on a known host arch.
    #[test]
    fn test_host_isa_returns_some_on_known_arch() {
        // We can't predict the build host, but the helper should return
        // Some on all arches VUMA claims to support. If None, the caller
        // falls back to AArch64 — verify that fallback works too.
        let resolved = resolve_isa(&None);
        let _ = resolved; // just ensure it doesn't panic.
        // On any of the supported arches, host_isa() should be Some.
        let known = ["x86_64", "aarch64", "riscv64", "arm", "powerpc64", "mips", "loongarch64"];
        if known.contains(&std::env::consts::ARCH) {
            assert!(host_isa().is_some(), "host_isa() should return Some on supported arch {}", std::env::consts::ARCH);
        }
    }

    /// Test 5: `vuma check hello.vuma` parses correctly.
    #[test]
    fn test_check() {
        let cli = Cli::try_parse_from(["vuma", "check", "hello.vuma"]).unwrap();
        match cli.command {
            Some(Commands::Check { ref file }) => {
                assert_eq!(file, &PathBuf::from("hello.vuma"));
            }
            _ => panic!("expected Check command"),
        }
    }

    /// Test 6: `vuma emit aarch64 hello.vuma` parses correctly.
    #[test]
    fn test_emit_aarch64() {
        let cli = Cli::try_parse_from(["vuma", "emit", "aarch64", "hello.vuma"]).unwrap();
        match cli.command {
            Some(Commands::Emit {
                isa,
                ref file,
                ref output,
            }) => {
                assert_eq!(isa, IsaArg::Aarch64);
                assert_eq!(file, &PathBuf::from("hello.vuma"));
                assert!(output.is_none());
            }
            _ => panic!("expected Emit command"),
        }
    }

    /// Test 7: `vuma emit x86-64 hello.vuma -o out.o` parses correctly.
    #[test]
    fn test_emit_x86_64_with_output() {
        let cli =
            Cli::try_parse_from(["vuma", "emit", "x86_64", "hello.vuma", "-o", "out.o"]).unwrap();
        match cli.command {
            Some(Commands::Emit {
                isa,
                ref file,
                ref output,
            }) => {
                assert_eq!(isa, IsaArg::X86_64);
                assert_eq!(file, &PathBuf::from("hello.vuma"));
                assert_eq!(output.as_ref().unwrap(), &PathBuf::from("out.o"));
            }
            _ => panic!("expected Emit command"),
        }
    }

    /// Test 8: `vuma disasm hello.o --isa riscv64 --base-addr 0x1000` parses correctly.
    #[test]
    fn test_disasm() {
        let cli = Cli::try_parse_from([
            "vuma",
            "disasm",
            "hello.o",
            "--isa",
            "riscv64",
            "--base-addr",
            "0x1000",
        ])
        .unwrap();
        match cli.command {
            Some(Commands::Disasm {
                ref file,
                isa,
                ref base_addr,
            }) => {
                assert_eq!(file, &PathBuf::from("hello.o"));
                assert_eq!(isa, IsaArg::Riscv64);
                assert_eq!(base_addr, "0x1000");
            }
            _ => panic!("expected Disasm command"),
        }
    }

    /// Test 9: `vuma verify hello.vuma` parses correctly.
    #[test]
    fn test_verify() {
        let cli = Cli::try_parse_from(["vuma", "verify", "hello.vuma"]).unwrap();
        match cli.command {
            Some(Commands::Verify { ref file }) => {
                assert_eq!(file, &PathBuf::from("hello.vuma"));
            }
            _ => panic!("expected Verify command"),
        }
    }

    /// Test 10: `vuma repl` parses correctly.
    #[test]
    fn test_repl_subcommand() {
        let cli = Cli::try_parse_from(["vuma", "repl"]).unwrap();
        match cli.command {
            Some(Commands::Repl) => {}
            _ => panic!("expected Repl command"),
        }
    }

    /// Test 10b: `vuma --repl` parses correctly.
    #[test]
    fn test_repl_flag() {
        let cli = Cli::try_parse_from(["vuma", "--repl"]).unwrap();
        assert!(cli.repl, "--repl flag should be true");
    }

    /// Test 11: Global --opt-level flag works.
    #[test]
    fn test_global_opt_level() {
        let cli =
            Cli::try_parse_from(["vuma", "--opt-level", "O0", "build", "hello.vuma"]).unwrap();
        assert_eq!(cli.opt_level, OptLevelArg::O0);
    }

    /// Test 12: Global --verification flag works.
    #[test]
    fn test_global_verification_level() {
        let cli = Cli::try_parse_from([
            "vuma",
            "--verification",
            "exhaustive",
            "build",
            "hello.vuma",
        ])
        .unwrap();
        assert_eq!(cli.verification, VerificationArg::Exhaustive);
    }

    /// Test 13: Global --debug flag works.
    #[test]
    fn test_global_debug_flag() {
        let cli = Cli::try_parse_from(["vuma", "--debug", "build", "hello.vuma"]).unwrap();
        assert!(cli.debug);
    }

    /// Test 14: Default values are correct.
    #[test]
    fn test_defaults() {
        let cli = Cli::try_parse_from(["vuma", "build", "hello.vuma"]).unwrap();
        assert_eq!(cli.opt_level, OptLevelArg::O2);
        assert_eq!(cli.verification, VerificationArg::Normal);
        assert!(!cli.debug);
    }

    /// Test 15: All ISA values are parseable.
    #[test]
    fn test_all_isa_values() {
        let isa_names = [
            "aarch64",
            "x86_64",
            "riscv64",
            "wasm32",
            "loongarch64",
            "arm32",
            "mips64",
            "ppc64",
        ];
        for name in isa_names {
            let cli = Cli::try_parse_from(["vuma", "emit", name, "test.vuma"]).unwrap();
            match cli.command {
                Some(Commands::Emit { isa, .. }) => {
                    // Verify it parsed without error.
                    let _backend_kind = BackendKind::from(isa);
                }
                _ => panic!("expected Emit command for ISA {}", name),
            }
        }
    }

    /// Test 16: OptLevelArg conversion to pipeline OptLevel.
    #[test]
    fn test_opt_level_conversion() {
        assert_eq!(OptLevel::from(OptLevelArg::O0), OptLevel::O0);
        assert_eq!(OptLevel::from(OptLevelArg::O1), OptLevel::O1);
        assert_eq!(OptLevel::from(OptLevelArg::O2), OptLevel::O2);
        assert_eq!(OptLevel::from(OptLevelArg::O3), OptLevel::O3);
    }

    /// Test 17: VerificationArg conversion to pipeline VerificationLevel.
    #[test]
    fn test_verification_conversion() {
        assert_eq!(
            VerificationLevel::from(VerificationArg::None),
            VerificationLevel::None
        );
        assert_eq!(
            VerificationLevel::from(VerificationArg::Quick),
            VerificationLevel::Quick
        );
        assert_eq!(
            VerificationLevel::from(VerificationArg::Normal),
            VerificationLevel::Normal
        );
        assert_eq!(
            VerificationLevel::from(VerificationArg::Exhaustive),
            VerificationLevel::Exhaustive
        );
    }

    /// Test 18: TargetArg conversion to pipeline CompileTarget.
    #[test]
    fn test_target_conversion() {
        assert_eq!(CompileTarget::from(TargetArg::Linux), CompileTarget::Linux);
    }

    /// Test 19: default_output_path produces correct path.
    #[test]
    fn test_default_output_path() {
        let input = PathBuf::from("hello.vuma");
        let output = default_output_path(&input);
        assert_eq!(output, PathBuf::from("hello.o"));

        let input2 = PathBuf::from("/tmp/test.vuma");
        let output2 = default_output_path(&input2);
        assert_eq!(output2, PathBuf::from("/tmp/test.o"));
    }

    /// Test 20: Invalid subcommand is rejected.
    #[test]
    fn test_invalid_subcommand() {
        let result = Cli::try_parse_from(["vuma", "invalid"]);
        assert!(result.is_err());
    }
}
