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

use std::fs;
use std::io::{self, Write as IoWrite};
use std::path::PathBuf;
use std::process::Command;

use clap::{Parser, Subcommand, ValueEnum};

use vuma::pipeline::{
    compile, CompileConfig, CompileTarget, OptLevel, VerificationLevel, VumaError,
};
use vuma_codegen::backend::{BackendKind, create_backend};

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

    /// Verification level (overrides subcommand default)
    #[arg(long, global = true, value_enum, default_value = "normal")]
    verification: VerificationArg,

    /// Include debug info in output
    #[arg(long, global = true)]
    debug: bool,

    #[command(subcommand)]
    command: Commands,
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

#[derive(Subcommand, Debug)]
enum Commands {
    /// Parse + compile to ARM64 ELF (default), save to output file
    Build {
        /// Input VUMA source file
        file: PathBuf,

        /// Output file path (default: <input>.o)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Target platform
        #[arg(long, value_enum, default_value = "pi5-linux")]
        target: TargetArg,
    },

    /// Build + execute (via QEMU aarch64 or native)
    Run {
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
}

/// Target platform CLI argument for the `build` subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum TargetArg {
    #[value(name = "pi5-bare")]
    Pi5Bare,
    #[value(name = "pi5-linux")]
    Pi5Linux,
    #[value(name = "linux")]
    Linux,
}

impl From<TargetArg> for CompileTarget {
    fn from(val: TargetArg) -> Self {
        match val {
            TargetArg::Pi5Bare => CompileTarget::Pi5Bare,
            TargetArg::Pi5Linux => CompileTarget::Pi5Linux,
            TargetArg::Linux => CompileTarget::Linux,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Helper functions
// ═══════════════════════════════════════════════════════════════════════════

/// Read source file content with a human-readable error on failure.
fn read_source(path: &PathBuf) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| {
        format!(
            "error: cannot read source file '{}': {}",
            path.display(),
            e
        )
    })
}

/// Build a `CompileConfig` from the global CLI flags.
fn make_config(cli: &Cli, target: CompileTarget) -> CompileConfig {
    CompileConfig {
        target,
        opt_level: OptLevel::from(cli.opt_level),
        verification_level: VerificationLevel::from(cli.verification),
        debug_info: cli.debug,
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
fn default_output_path(input: &PathBuf) -> PathBuf {
    let stem = input
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let dir = input.parent().unwrap_or(std::path::Path::new("."));
    dir.join(format!("{}.o", stem))
}

// ═══════════════════════════════════════════════════════════════════════════
// Subcommand handlers
// ═══════════════════════════════════════════════════════════════════════════

/// `vuma build <file>` — Parse + compile to ELF, save to output file.
fn cmd_build(cli: &Cli, file: &PathBuf, output: &Option<PathBuf>, target: TargetArg) -> Result<(), String> {
    let source = read_source(file)?;
    let config = make_config(cli, CompileTarget::from(target));
    let result = compile(&source, &config).map_err(|errors| {
        print_errors(&errors);
        format!("compilation failed with {} error(s)", errors.len())
    })?;

    let out_path = output.as_ref().cloned().unwrap_or_else(|| default_output_path(file));
    fs::write(&out_path, &result.binary).map_err(|e| {
        format!("error: cannot write output file '{}': {}", out_path.display(), e)
    })?;

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

    Ok(())
}

/// `vuma run <file>` — Build + execute.
fn cmd_run(cli: &Cli, file: &PathBuf, args: &[String]) -> Result<(), String> {
    let source = read_source(file)?;
    let config = make_config(cli, CompileTarget::Pi5Linux);
    let result = compile(&source, &config).map_err(|errors| {
        print_errors(&errors);
        format!("compilation failed with {} error(s)", errors.len())
    })?;

    // Write binary to a temp file.
    let tmp_dir = std::env::temp_dir();
    let exe_path = tmp_dir.join(format!("vuma_run_{}", std::process::id()));
    fs::write(&exe_path, &result.binary).map_err(|e| {
        format!("error: cannot write temporary executable '{}': {}", exe_path.display(), e)
    })?;

    // Make the file executable on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&exe_path).map_err(|e| {
            format!("error: cannot stat '{}': {}", exe_path.display(), e)
        })?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&exe_path, perms).map_err(|e| {
            format!("error: cannot chmod '{}': {}", exe_path.display(), e)
        })?;
    }

    // Try to execute: first natively (if on aarch64), then via qemu-aarch64.
    let native_output = Command::new(&exe_path)
        .args(args)
        .output();

    let output = match native_output {
        Ok(out) => out,
        Err(_) => {
            // Native execution failed; try qemu-aarch64.
            let qemu_output = Command::new("qemu-aarch64")
                .arg(&exe_path)
                .args(args)
                .output()
                .map_err(|e| {
                    format!(
                        "error: failed to execute binary (neither native nor qemu-aarch64): {}",
                        e
                    )
                })?;
            qemu_output
        }
    };

    io::stdout().write_all(&output.stdout).map_err(|e| {
        format!("error: failed to write program output: {}", e)
    })?;
    io::stderr().write_all(&output.stderr).map_err(|e| {
        format!("error: failed to write program stderr: {}", e)
    })?;

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
    let mut config = make_config(cli, CompileTarget::Pi5Linux);
    // check mode: always run verification, don't skip
    if config.verification_level == VerificationLevel::None {
        config.verification_level = VerificationLevel::Normal;
    }

    // Run the full compile — the check command doesn't save the binary,
    // it just verifies the program compiles and passes verification.
    let result = compile(&source, &config).map_err(|errors| {
        print_errors(&errors);
        format!("check failed with {} error(s)", errors.len())
    })?;

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

    Ok(())
}

/// `vuma emit <isa> <file>` — Compile to a specific ISA target.
fn cmd_emit(cli: &Cli, isa: IsaArg, file: &PathBuf, output: &Option<PathBuf>) -> Result<(), String> {
    let source = read_source(file)?;
    let backend_kind = BackendKind::from(isa);
    let config = make_config(cli, CompileTarget::Pi5Linux);
    let result = compile(&source, &config).map_err(|errors| {
        print_errors(&errors);
        format!("compilation failed with {} error(s)", errors.len())
    })?;

    // Use the multi-arch backend to produce ISA-specific output.
    let backend = create_backend(backend_kind).map_err(|e| {
        format!("error: cannot create {} backend: {}", backend_kind.isa_name(), e)
    })?;

    // Allocate registers and encode using the target backend.
    let mut allocated_functions = Vec::new();
    if let Some(ref debug_info) = result.debug_info {
        if let Some(ref ir_program) = debug_info.ir_pre_regalloc {
            for func in &ir_program.functions {
                match backend.allocate_registers(func) {
                    Ok(allocated) => allocated_functions.push(allocated),
                    Err(e) => {
                        eprintln!("warning: register allocation failed for '{}': {}", func.name, e);
                    }
                }
            }
        }
    }

    let out_path = output.as_ref().cloned().unwrap_or_else(|| default_output_path(file));

    // If we have allocated functions, encode them; otherwise write the ARM64 binary.
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
                println!(
                    "Emitted {} -> {} ({} bytes, ISA: {})",
                    file.display(),
                    out_path.display(),
                    bytes.len(),
                    backend.name(),
                );
            }
            Err(e) => {
                // Backend encode failed; fall back to writing the ARM64 ELF.
                eprintln!("warning: {} encoding failed ({}), writing ARM64 ELF instead", backend.name(), e);
                fs::write(&out_path, &result.binary).map_err(|e2| {
                    format!("error: cannot write output file '{}': {}", out_path.display(), e2)
                })?;
                println!(
                    "Emitted {} -> {} ({} bytes, ARM64 ELF fallback)",
                    file.display(),
                    out_path.display(),
                    result.binary.len(),
                );
            }
        }
    } else {
        // No IR program available (debug info not captured), write the ARM64 binary.
        fs::write(&out_path, &result.binary).map_err(|e| {
            format!("error: cannot write output file '{}': {}", out_path.display(), e)
        })?;
        println!(
            "Emitted {} -> {} ({} bytes, ARM64 ELF)",
            file.display(),
            out_path.display(),
            result.binary.len(),
        );
    }

    Ok(())
}

/// `vuma disasm <file>` — Read binary and disassemble.
fn cmd_disasm(file: &PathBuf, isa: IsaArg, base_addr_str: &str) -> Result<(), String> {
    let bytes = fs::read(file).map_err(|e| {
        format!("error: cannot read binary file '{}': {}", file.display(), e)
    })?;

    let base_addr = u64::from_str_radix(base_addr_str.trim_start_matches("0x"), 16)
        .map_err(|e| format!("error: invalid base address '{}': {}", base_addr_str, e))?;

    let backend_kind = BackendKind::from(isa);
    let backend = create_backend(backend_kind).map_err(|e| {
        format!("error: cannot create {} backend: {}", backend_kind.isa_name(), e)
    })?;

    let instructions = backend.disassemble(&bytes, base_addr);

    println!("Disassembly of {} ({} bytes, ISA: {}):", file.display(), bytes.len(), backend.name());
    for line in &instructions {
        println!("{}", line);
    }

    Ok(())
}

/// `vuma verify <file>` — Run IVE 5-invariant verification.
fn cmd_verify(cli: &Cli, file: &PathBuf) -> Result<(), String> {
    let source = read_source(file)?;
    let mut config = make_config(cli, CompileTarget::Pi5Linux);
    // Force exhaustive verification for the verify subcommand.
    config.verification_level = VerificationLevel::Exhaustive;

    let result = compile(&source, &config).map_err(|errors| {
        print_errors(&errors);
        format!("compilation/verification failed with {} error(s)", errors.len())
    })?;

    match result.verification {
        Some(ref verification) => {
            println!("IVE Verification Results for {}", file.display());
            println!("  Overall verdict: {}", verification.overall);
            println!("  Summary: {} invariant(s) checked", verification.summary.total_checked);
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
                    status_str,
                    per_inv.kind,
                    per_inv.result.message,
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
        io::stdout().flush().map_err(|e| format!("flush error: {}", e))?;

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
        match parser.parse_program() {
            Ok(program) => {
                // Successfully parsed; display the AST.
                println!("{:#?}", program);
                continue;
            }
            Err(program_errors) => {
                // Program parse failed; try to parse as an expression via
                // wrapping it in a dummy program context.
                // Since parse_expr is private, we re-parse as a minimal program.
                let wrapped = format!("fn _repl_expr() {{ {} }}", trimmed);
                let mut expr_parser = vuma_parser::Parser::new(&wrapped);
                match expr_parser.parse_program() {
                    Ok(program) => {
                        // Print the function body AST.
                        if let Some(item) = program.items.first() {
                            println!("{:#?}", item);
                        } else {
                            println!("{:#?}", program);
                        }
                    }
                    Err(_) => {
                        // Show the original parse errors.
                        for err in &program_errors {
                            eprintln!("parse error: {}", err);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Main entry point
// ═══════════════════════════════════════════════════════════════════════════

fn main() {
    // Initialize logger.
    env_logger::init();

    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Build { ref file, ref output, target } => cmd_build(&cli, file, output, target),
        Commands::Run { ref file, ref args } => cmd_run(&cli, file, args),
        Commands::Check { ref file } => cmd_check(&cli, file),
        Commands::Emit { isa, ref file, ref output } => cmd_emit(&cli, isa, file, output),
        Commands::Disasm { ref file, isa, ref base_addr } => cmd_disasm(file, isa, base_addr),
        Commands::Verify { ref file } => cmd_verify(&cli, file),
        Commands::Repl => cmd_repl(),
    };

    if let Err(err) = result {
        eprintln!("{}", err);
        std::process::exit(1);
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
            Commands::Build { ref file, ref output, ref target } => {
                assert_eq!(file, &PathBuf::from("hello.vuma"));
                assert!(output.is_none());
                assert_eq!(target, &TargetArg::Pi5Linux);
            }
            _ => panic!("expected Build command"),
        }
    }

    /// Test 2: `vuma build hello.vuma -o out.o --target pi5-bare` parses correctly.
    #[test]
    fn test_build_with_options() {
        let cli = Cli::try_parse_from([
            "vuma", "build", "hello.vuma", "-o", "out.o", "--target", "pi5-bare",
        ])
        .unwrap();
        match cli.command {
            Commands::Build { ref file, ref output, ref target } => {
                assert_eq!(file, &PathBuf::from("hello.vuma"));
                assert_eq!(output.as_ref().unwrap(), &PathBuf::from("out.o"));
                assert_eq!(target, &TargetArg::Pi5Bare);
            }
            _ => panic!("expected Build command"),
        }
    }

    /// Test 3: `vuma run hello.vuma` parses correctly.
    #[test]
    fn test_run_basic() {
        let cli = Cli::try_parse_from(["vuma", "run", "hello.vuma"]).unwrap();
        match cli.command {
            Commands::Run { ref file, ref args } => {
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
            Commands::Run { ref file, ref args } => {
                assert_eq!(file, &PathBuf::from("hello.vuma"));
                assert_eq!(args, &vec!["arg1".to_string(), "arg2".to_string()]);
            }
            _ => panic!("expected Run command"),
        }
    }

    /// Test 5: `vuma check hello.vuma` parses correctly.
    #[test]
    fn test_check() {
        let cli = Cli::try_parse_from(["vuma", "check", "hello.vuma"]).unwrap();
        match cli.command {
            Commands::Check { ref file } => {
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
            Commands::Emit { isa, ref file, ref output } => {
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
        let cli = Cli::try_parse_from(["vuma", "emit", "x86_64", "hello.vuma", "-o", "out.o"]).unwrap();
        match cli.command {
            Commands::Emit { isa, ref file, ref output } => {
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
            "vuma", "disasm", "hello.o", "--isa", "riscv64", "--base-addr", "0x1000",
        ])
        .unwrap();
        match cli.command {
            Commands::Disasm { ref file, isa, ref base_addr } => {
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
            Commands::Verify { ref file } => {
                assert_eq!(file, &PathBuf::from("hello.vuma"));
            }
            _ => panic!("expected Verify command"),
        }
    }

    /// Test 10: `vuma repl` parses correctly.
    #[test]
    fn test_repl() {
        let cli = Cli::try_parse_from(["vuma", "repl"]).unwrap();
        match cli.command {
            Commands::Repl => {}
            _ => panic!("expected Repl command"),
        }
    }

    /// Test 11: Global --opt-level flag works.
    #[test]
    fn test_global_opt_level() {
        let cli = Cli::try_parse_from(["vuma", "--opt-level", "O0", "build", "hello.vuma"]).unwrap();
        assert_eq!(cli.opt_level, OptLevelArg::O0);
    }

    /// Test 12: Global --verification flag works.
    #[test]
    fn test_global_verification_level() {
        let cli = Cli::try_parse_from(["vuma", "--verification", "exhaustive", "build", "hello.vuma"]).unwrap();
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
        let isa_names = ["aarch64", "x86_64", "riscv64", "wasm32", "loongarch64", "arm32", "mips64", "ppc64"];
        for name in isa_names {
            let cli = Cli::try_parse_from(["vuma", "emit", name, "test.vuma"]).unwrap();
            match cli.command {
                Commands::Emit { isa, .. } => {
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
        assert_eq!(VerificationLevel::from(VerificationArg::None), VerificationLevel::None);
        assert_eq!(VerificationLevel::from(VerificationArg::Quick), VerificationLevel::Quick);
        assert_eq!(VerificationLevel::from(VerificationArg::Normal), VerificationLevel::Normal);
        assert_eq!(VerificationLevel::from(VerificationArg::Exhaustive), VerificationLevel::Exhaustive);
    }

    /// Test 18: TargetArg conversion to pipeline CompileTarget.
    #[test]
    fn test_target_conversion() {
        assert_eq!(CompileTarget::from(TargetArg::Pi5Bare), CompileTarget::Pi5Bare);
        assert_eq!(CompileTarget::from(TargetArg::Pi5Linux), CompileTarget::Pi5Linux);
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
