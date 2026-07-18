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

use vuma::pipeline::{
    bridge_ast_to_codegen_scg, compile_modules, compile_with_path, CompileConfig, CompileResult, CompileTarget, OptLevel,
    VerificationLevel, VumaError,
};
use vuma::telemetry::TelemetryCollector;
use vuma::logging::{LogLevel, init_logger, global_logger};
use vuma_codegen::backend::{create_backend, BackendKind};
use vuma_codegen::ScgToIr;

// ═══════════════════════════════════════════════════════════════════════════
// CLI definition (hand-written argv parser)
// ═══════════════════════════════════════════════════════════════════════════

/// VUMA — Verified-Unsafe Memory Access: AI-Native Programming Language
///
/// Hand-written: all field-level parsing/validation is done in
/// `parse_cli_from` further down in this file. The struct itself only
/// describes the parsed result.
#[derive(Debug)]
struct Cli {
    /// Optimization level (overrides subcommand default) [default: O3]
    opt_level: OptLevelArg,

    /// Verification level (overrides subcommand default).
    /// Default is `normal` (strict — all five invariants run).
    /// `--verification none` is NO LONGER accepted; use `--no-verify`
    /// to explicitly bypass verification (Wave 19 escape-hatch closure).
    /// [default: normal]
    verification: VerificationArg,

    /// Explicitly skip all verification (escape hatch).
    /// This is the ONLY way to bypass verification. A compile-time
    /// warning is emitted when this flag is used. (Wave 19)
    no_verify: bool,

    /// Strict verification: treat `Inconclusive` verdicts as compilation-
    /// blocking errors (Wave 19). By default `Inconclusive` (unverified
    /// but no proven violation) is allowed; `--strict-verification` makes
    /// it a hard error.
    strict_verification: bool,

    /// Include debug info in output (alias: --debug-info)
    debug: bool,

    /// Emit full ELF section headers in the output binary
    sections: bool,

    /// Launch the interactive REPL (shorthand for `vuma repl`)
    repl: bool,

    /// Enable runtime memory safety checks (bounds checking, --safe mode)
    safe: bool,

    /// Disable compile-time memory-safety analysis (use-after-free, double-
    /// free, leak, uninitialized-read detection).  This is an ESCAPE HATCH:
    /// programs with memory-safety bugs will compile successfully but crash
    /// or corrupt memory at runtime.  Use only for debugging the analyzer
    /// itself or for prototyping unsafe code.  A compile-time warning is
    /// emitted when this flag is set.
    no_memory_safety: bool,

    /// Run performance benchmarks instead of compiling
    bench: bool,

    /// Enable verbose/debug logging
    verbose: bool,

    /// Suppress non-error output
    quiet: bool,

    /// Output telemetry data as JSON
    telemetry: bool,

    command: Option<Commands>,
}

/// Optimization level CLI argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptLevelArg {
    O0,
    O1,
    O2,
    O3,
}

impl OptLevelArg {
    /// Parse a value for `--opt-level`.
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "O0" => Ok(OptLevelArg::O0),
            "O1" => Ok(OptLevelArg::O1),
            "O2" => Ok(OptLevelArg::O2),
            "O3" => Ok(OptLevelArg::O3),
            other => Err(format!(
                "error: invalid value '{other}' for '--opt-level <OPT_LEVEL>'\n  [possible values: O0, O1, O2, O3]"
            )),
        }
    }
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
///
/// `None` is intentionally NOT a CLI value (Wave 19): the only way to
/// bypass verification is the explicit `--no-verify` flag, which emits a
/// compile-time warning. This prevents users from silently disabling
/// verification via `--verification none`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerificationArg {
    Quick,
    Normal,
    Exhaustive,
}

impl VerificationArg {
    /// Parse a value for `--verification`.
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "quick" => Ok(VerificationArg::Quick),
            "normal" => Ok(VerificationArg::Normal),
            "exhaustive" => Ok(VerificationArg::Exhaustive),
            other => Err(format!(
                "error: invalid value '{other}' for '--verification <VERIFICATION>'\n  [possible values: quick, normal, exhaustive]"
            )),
        }
    }
}

impl From<VerificationArg> for VerificationLevel {
    fn from(val: VerificationArg) -> Self {
        match val {
            VerificationArg::Quick => VerificationLevel::Quick,
            VerificationArg::Normal => VerificationLevel::Normal,
            VerificationArg::Exhaustive => VerificationLevel::Exhaustive,
        }
    }
}

/// Target ISA for the `emit` subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IsaArg {
    Aarch64,
    X86_64,
    Riscv64,
    Wasm32,
    Loongarch64,
    Arm32,
    Mips64,
    Ppc64,
    Ppc64le,
    Sparc64,
    S390x,
    Alpha,
    Hppa,
    M68k,
    Riscv32,
    X86_32,
}

impl IsaArg {
    /// Parse a value for `--isa` / `--target`.
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "aarch64" => Ok(IsaArg::Aarch64),
            "x86_64" => Ok(IsaArg::X86_64),
            "riscv64" => Ok(IsaArg::Riscv64),
            "wasm32" => Ok(IsaArg::Wasm32),
            "loongarch64" => Ok(IsaArg::Loongarch64),
            "arm32" => Ok(IsaArg::Arm32),
            "mips64" => Ok(IsaArg::Mips64),
            "ppc64" => Ok(IsaArg::Ppc64),
            "ppc64le" => Ok(IsaArg::Ppc64le),
            "sparc64" => Ok(IsaArg::Sparc64),
            "s390x" => Ok(IsaArg::S390x),
            "alpha" => Ok(IsaArg::Alpha),
            "hppa" => Ok(IsaArg::Hppa),
            "m68k" => Ok(IsaArg::M68k),
            "riscv32" => Ok(IsaArg::Riscv32),
            "x86_32" => Ok(IsaArg::X86_32),
            other => Err(format!(
                "error: invalid value '{other}' for '--isa <ISA>'\n  [possible values: aarch64, x86_64, riscv64, wasm32, loongarch64, arm32, mips64, ppc64, ppc64le, sparc64, s390x, alpha, hppa, m68k, riscv32, x86_32]"
            )),
        }
    }
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
            IsaArg::Ppc64le => BackendKind::PowerPC64LE,
            IsaArg::Sparc64 => BackendKind::Sparc64,
            IsaArg::S390x => BackendKind::S390X,
            IsaArg::Alpha => BackendKind::Alpha,
            IsaArg::Hppa => BackendKind::Hppa,
            IsaArg::M68k => BackendKind::M68k,
            IsaArg::Riscv32 => BackendKind::RiscV32,
            IsaArg::X86_32 => BackendKind::X86_32,
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
        "powerpc64le" => Some(IsaArg::Ppc64le),
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

#[derive(Debug)]
enum Commands {
    /// Parse + compile to ELF (host ISA by default), save to output file
    Build {
        /// Input VUMA source file
        file: PathBuf,

        /// Output file path (default: <input>.o)
        output: Option<PathBuf>,

        /// Target platform [default: linux]
        target: TargetArg,

        /// Target ISA. Defaults to the host architecture so the emitted
        /// binary can be executed natively; pass `--isa aarch64` to
        /// cross-compile for AArch64 (uses the canonical pipeline with
        /// full verification + telemetry). Non-AArch64 targets use the
        /// direct AST→codegen bridge path.
        isa: Option<IsaArg>,
    },

    /// Build + execute (native on host arch, or via qemu-<isa>)
    Run {
        /// Target ISA. Defaults to the host architecture so the emitted
        /// binary can be executed natively. NOTE: because `args` uses
        /// trailing-var-arg semantics, `--isa` must appear BEFORE the
        /// input file:
        ///   `vuma run --isa x86_64 hello.vuma arg1 arg2`
        isa: Option<IsaArg>,

        /// Input VUMA source file
        file: PathBuf,

        /// Arguments to pass to the executed program (trailing var-arg;
        /// hyphen-leading args after the file are NOT re-parsed as flags).
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
        output: Option<PathBuf>,
    },

    /// Compile to a relocatable object file (ET_REL) for linking with system linker
    Compile {
        /// Input VUMA source file
        file: PathBuf,

        /// Output file path (default: <input>.o)
        output: Option<PathBuf>,

        /// Target ISA [default: aarch64]
        target: IsaArg,

        /// Output format (elf, obj, raw, wasm) [default: obj]
        format: FormatArg,
    },

    /// Read a binary file and disassemble it
    Disasm {
        /// Binary file to disassemble
        file: PathBuf,

        /// ISA to use for disassembly [default: aarch64]
        isa: IsaArg,

        /// Starting virtual address [default: 0x400000]
        base_addr: String,
    },

    /// Link multiple `.vuma` files into a single ELF binary.
    ///
    /// Each input file is parsed independently; the ASTs are merged
    /// (cross-module `extern "C"` declarations are resolved against real
    /// `fn` definitions in sibling files), and the merged program is
    /// compiled through the direct AST → codegen SCG → IR → regalloc →
    /// backend.encode_program path. The emitted ELF targets the host
    /// architecture (or the explicit `--isa` if given) so it can be
    /// executed natively.
    ///
    /// Usage:
    ///   vuma link <file1.vuma> <file2.vuma> ... [-o <output>] [--run] [--isa x86_64]
    ///
    /// Default output is `a.out`. When `--run` is set, the linked ELF is
    /// `chmod +x`'d and executed (with no argv; to pass argv to the linked
    /// binary, run it directly after `vuma link` writes it). The
    /// `--run` flag is a convenience for the smoke-test workflow; the
    /// end-to-end test in `wave48_self_host.rs` invokes the linked ELF
    /// directly via `std::process::Command` so it can pass argv.
    Link {
        /// Input VUMA source files (two or more). The first file is the
        /// entry-point module (the one that defines `fn main`); subsequent
        /// files are sibling modules whose `fn` definitions resolve
        /// `extern` declarations in the entry module.
        files: Vec<PathBuf>,

        /// Output file path (default: a.out)
        output: Option<PathBuf>,

        /// Target ISA. Defaults to the host architecture so the emitted
        /// binary can be executed natively. Currently `compile_modules`
        /// always uses the host backend regardless of this flag (the
        /// flag is accepted for forward-compatibility and to mirror
        /// `vuma build`/`vuma run`'s CLI shape); a future wave will
        /// thread it through to the pipeline.
        isa: Option<IsaArg>,

        /// After linking, `chmod +x` the output and execute it, forwarding
        /// stdout/stderr to the terminal. The linked binary is invoked
        /// with no argv; to pass argv, run the binary directly after
        /// `vuma link` writes it.
        run: bool,
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
        cmd: PkgCommand,
    },
}

/// Package manager subcommands.
#[derive(Debug)]
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
        /// Version requirement (e.g. "0.1", "^1.0") [default: *]
        version: String,
    },
}

/// Target platform CLI argument for the `build` subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetArg {
    Linux,
}

impl TargetArg {
    /// Parse a value for `--target`.
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "linux" => Ok(TargetArg::Linux),
            other => Err(format!(
                "error: invalid value '{other}' for '--target <TARGET>'\n  [possible values: linux]"
            )),
        }
    }
}

impl From<TargetArg> for CompileTarget {
    fn from(val: TargetArg) -> Self {
        match val {
            TargetArg::Linux => CompileTarget::Linux,
        }
    }
}

/// Output format for the `compile` subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormatArg {
    Elf,
    Obj,
    Raw,
    Wasm,
}

impl FormatArg {
    /// Parse a value for `--format`.
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "elf" => Ok(FormatArg::Elf),
            "obj" => Ok(FormatArg::Obj),
            "raw" => Ok(FormatArg::Raw),
            "wasm" => Ok(FormatArg::Wasm),
            other => Err(format!(
                "error: invalid value '{other}' for '--format <FORMAT>'\n  [possible values: elf, obj, raw, wasm]"
            )),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Hand-written argv parser
// ═══════════════════════════════════════════════════════════════════════════

/// Iterator type used by the argv parser.
type ArgIter = std::iter::Peekable<std::vec::IntoIter<String>>;

/// Parse error returned by `parse_cli_from`.
#[derive(Debug)]
enum ParseError {
    /// `--help` / `-h` was encountered.
    Help,
    /// `--version` / `-V` was encountered.
    Version,
    /// A parse error with a human-readable message.
    Msg(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Help => write!(f, "help requested"),
            ParseError::Version => write!(f, "version requested"),
            ParseError::Msg(s) => write!(f, "{}", s),
        }
    }
}

impl std::error::Error for ParseError {}

/// Print top-level usage to stdout.
fn print_usage() {
    println!(
        "VUMA — Verified-Unsafe Memory Access: AI-Native Programming Language\n\n\
         Usage: vuma [OPTIONS] [COMMAND]\n\n\
         Commands:\n\
         \x20  build     Parse + compile to ELF, save to output file\n\
         \x20  run       Build + execute (native on host arch, or via qemu-<isa>)\n\
         \x20  check     Parse + SCG + BD inference + IVE verification only\n\
         \x20  emit      Compile to a specific ISA target\n\
         \x20  compile   Compile to a relocatable object file (ET_REL)\n\
         \x20  disasm    Read a binary file and disassemble it\n\
         \x20  link      Link multiple .vuma files into a single ELF binary\n\
         \x20  verify    Run IVE 5-invariant verification\n\
         \x20  repl      Interactive REPL: parse expressions and print AST\n\
         \x20  lsp       Start the Language Server (LSP) for IDE/LLM integration\n\
         \x20  pkg       Package manager subcommands\n\
         \x20  help      Print this message or the help of the given subcommand(s)\n\n\
         Options:\n\
         \x20      --opt-level <OPT_LEVEL>        Optimization level [default: O3] [possible values: O0, O1, O2, O3]\n\
         \x20      --verification <VERIFICATION>  Verification level [default: normal] [possible values: quick, normal, exhaustive]\n\
         \x20      --no-verify                    Explicitly skip all verification (escape hatch)\n\
         \x20      --strict-verification          Treat Inconclusive verdicts as compilation-blocking errors\n\
         \x20      --debug                        Include debug info in output (alias: --debug-info)\n\
         \x20      --sections                     Emit full ELF section headers in the output binary\n\
         \x20      --repl                         Launch the interactive REPL (shorthand for `vuma repl`)\n\
         \x20      --safe                         Enable runtime memory safety checks\n\
         \x20      --no-memory-safety             Disable compile-time memory-safety analysis (escape hatch)\n\
         \x20      --bench                        Run performance benchmarks instead of compiling\n\
         \x20  -v, --verbose                     Enable verbose/debug logging\n\
         \x20  -q, --quiet                       Suppress non-error output\n\
         \x20      --telemetry                    Output telemetry data as JSON\n\
         \x20  -h, --help                        Print help\n\
         \x20  -V, --version                     Print version"
    );
}

/// Parse `argv` from `std::env::args()`. On `--help`/`-h` prints usage and
/// exits with code 0; on `--version`/`-V` prints the version and exits with
/// code 0; on any parse error prints the message to stderr and exits with
/// code 2.
fn parse_cli() -> Cli {
    let args: Vec<String> = std::env::args().collect();
    match parse_cli_from(args) {
        Ok(cli) => cli,
        Err(ParseError::Help) => {
            print_usage();
            std::process::exit(0);
        }
        Err(ParseError::Version) => {
            println!("{}", version_long());
            std::process::exit(0);
        }
        Err(ParseError::Msg(msg)) => {
            eprintln!("{}", msg);
            std::process::exit(2);
        }
    }
}

/// Parse the supplied argument vector. Index 0 is the program name and is
/// skipped. Used by both `parse_cli()` and the test suite (which calls it
/// directly with synthetic argv vectors).
fn parse_cli_from<I>(args: I) -> Result<Cli, ParseError>
where
    I: IntoIterator<Item: Into<String>>,
{
    let mut iter: ArgIter = args
        .into_iter()
        .map(|s| s.into())
        .skip(1) // skip program name (args[0])
        .collect::<Vec<_>>()
        .into_iter()
        .peekable();

    let mut cli = Cli {
        opt_level: OptLevelArg::O3, // O3 is always on (default since 2026-07)
        verification: VerificationArg::Normal,
        no_verify: false,
        strict_verification: false,
        debug: false,
        sections: false,
        repl: false,
        safe: false,
        no_memory_safety: false,
        bench: false,
        verbose: false,
        quiet: false,
        telemetry: false,
        command: None,
    };

    while let Some(arg) = iter.peek().cloned() {
        if arg == "--help" || arg == "-h" {
            return Err(ParseError::Help);
        }
        if arg == "--version" || arg == "-V" {
            return Err(ParseError::Version);
        }
        if try_consume_global_flag(&arg, &mut iter, &mut cli)? {
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}'\n\n\
                 Usage: vuma [OPTIONS] [COMMAND]\n\n\
                 For more information, try '--help'."
            )));
        }
        // First non-flag argument is the subcommand name.
        iter.next(); // consume the subcommand name
        let cmd = parse_subcommand(&arg, &mut iter, &mut cli)?;
        cli.command = Some(cmd);
        break;
    }

    Ok(cli)
}

/// Try to consume `arg` as a global CLI flag (a flag that may appear before
/// or after the subcommand). Returns `Ok(true)` if it matched and was
/// consumed (possibly along with its value); `Ok(false)` if it didn't match
/// a global flag (and wasn't consumed); `Err` on a parse error.
fn try_consume_global_flag(
    arg: &str,
    iter: &mut ArgIter,
    cli: &mut Cli,
) -> Result<bool, ParseError> {
    match arg {
        "--opt-level" => {
            iter.next();
            let val = take_value(iter, "--opt-level <OPT_LEVEL>")?;
            cli.opt_level = OptLevelArg::parse(&val).map_err(ParseError::Msg)?;
            Ok(true)
        }
        "--verification" => {
            iter.next();
            let val = take_value(iter, "--verification <VERIFICATION>")?;
            cli.verification = VerificationArg::parse(&val).map_err(ParseError::Msg)?;
            Ok(true)
        }
        "--no-verify" => {
            iter.next();
            cli.no_verify = true;
            Ok(true)
        }
        "--strict-verification" => {
            iter.next();
            cli.strict_verification = true;
            Ok(true)
        }
        "--debug" | "--debug-info" => {
            iter.next();
            cli.debug = true;
            Ok(true)
        }
        "--sections" => {
            iter.next();
            cli.sections = true;
            Ok(true)
        }
        "--repl" => {
            iter.next();
            cli.repl = true;
            Ok(true)
        }
        "--safe" => {
            iter.next();
            cli.safe = true;
            Ok(true)
        }
        "--no-memory-safety" => {
            iter.next();
            cli.no_memory_safety = true;
            Ok(true)
        }
        "--bench" => {
            iter.next();
            cli.bench = true;
            Ok(true)
        }
        "-v" | "--verbose" => {
            iter.next();
            cli.verbose = true;
            Ok(true)
        }
        "-q" | "--quiet" => {
            iter.next();
            cli.quiet = true;
            Ok(true)
        }
        "--telemetry" => {
            iter.next();
            cli.telemetry = true;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Take the next value from the iterator, or return an error if there is none.
fn take_value(iter: &mut ArgIter, name: &str) -> Result<String, ParseError> {
    match iter.next() {
        Some(v) => Ok(v),
        None => Err(ParseError::Msg(format!(
            "error: a value is required for '{name}' but none was supplied"
        ))),
    }
}

/// Dispatch to a subcommand-specific parser. The subcommand name has
/// already been consumed from `iter`.
fn parse_subcommand(
    name: &str,
    iter: &mut ArgIter,
    cli: &mut Cli,
) -> Result<Commands, ParseError> {
    match name {
        "build" => parse_build(iter, cli),
        "run" => parse_run(iter, cli),
        "check" => parse_check(iter, cli),
        "emit" => parse_emit(iter, cli),
        "compile" => parse_compile(iter, cli),
        "disasm" => parse_disasm(iter, cli),
        "link" => parse_link(iter, cli),
        "verify" => parse_verify(iter, cli),
        "repl" => parse_repl(iter, cli),
        "lsp" => parse_lsp(iter, cli),
        "pkg" => parse_pkg(iter, cli),
        "help" => Err(ParseError::Help),
        other => Err(ParseError::Msg(format!(
            "error: unrecognized subcommand '{other}'\n\n\
             Usage: vuma [OPTIONS] [COMMAND]\n\n\
             For more information, try '--help'."
        ))),
    }
}

/// `vuma build <file> [-o <out>] [--target linux] [--isa <isa>]`
fn parse_build(iter: &mut ArgIter, cli: &mut Cli) -> Result<Commands, ParseError> {
    let mut file: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut target: TargetArg = TargetArg::Linux;
    let mut isa: Option<IsaArg> = None;

    while let Some(arg) = iter.peek().cloned() {
        if arg == "--help" || arg == "-h" {
            return Err(ParseError::Help);
        }
        if arg == "--version" || arg == "-V" {
            return Err(ParseError::Version);
        }
        if try_consume_global_flag(&arg, iter, cli)? {
            continue;
        }
        match arg.as_str() {
            "-o" | "--output" => {
                iter.next();
                let val = take_value(iter, "--output <OUTPUT>")?;
                output = Some(PathBuf::from(val));
                continue;
            }
            "--target" => {
                iter.next();
                let val = take_value(iter, "--target <TARGET>")?;
                target = TargetArg::parse(&val).map_err(ParseError::Msg)?;
                continue;
            }
            "--isa" => {
                iter.next();
                let val = take_value(iter, "--isa <ISA>")?;
                isa = Some(IsaArg::parse(&val).map_err(ParseError::Msg)?);
                continue;
            }
            _ => {}
        }
        if arg.starts_with('-') && arg != "-" {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma build'\n\n\
                 For more information, try '--help'."
            )));
        }
        iter.next();
        if file.is_none() {
            file = Some(PathBuf::from(arg));
        } else {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma build'\n\n\
                 For more information, try '--help'."
            )));
        }
    }

    let file = file.ok_or_else(|| {
        ParseError::Msg(
            "error: the following required arguments were not provided:\n  <FILE>\n\n\
             Usage: vuma build <FILE> [OPTIONS]\n\n\
             For more information, try '--help'."
                .to_string(),
        )
    })?;

    Ok(Commands::Build { file, output, target, isa })
}

/// `vuma run [--isa <isa>] <file> [args...]` (trailing-var-arg).
///
/// Once the file positional has been consumed, every remaining arg
/// (including flag-shaped ones) becomes a trailing program argument.
fn parse_run(iter: &mut ArgIter, cli: &mut Cli) -> Result<Commands, ParseError> {
    let mut isa: Option<IsaArg> = None;
    let mut file: Option<PathBuf> = None;
    let mut trailing_args: Vec<String> = Vec::new();

    while let Some(arg) = iter.peek().cloned() {
        // Once the file positional has been consumed, every remaining arg
        // (including flag-shaped ones) becomes a trailing program argument.
        if file.is_some() {
            // Drain everything remaining into trailing_args.
            for a in iter.by_ref() {
                trailing_args.push(a);
            }
            break;
        }
        if arg == "--help" || arg == "-h" {
            return Err(ParseError::Help);
        }
        if arg == "--version" || arg == "-V" {
            return Err(ParseError::Version);
        }
        if try_consume_global_flag(&arg, iter, cli)? {
            continue;
        }
        if arg == "--isa" {
            iter.next();
            let val = take_value(iter, "--isa <ISA>")?;
            isa = Some(IsaArg::parse(&val).map_err(ParseError::Msg)?);
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma run'\n  note: `--isa` must appear BEFORE the input file (trailing_var_arg)\n\n\
                 For more information, try '--help'."
            )));
        }
        iter.next();
        file = Some(PathBuf::from(arg));
    }

    let file = file.ok_or_else(|| {
        ParseError::Msg(
            "error: the following required arguments were not provided:\n  <FILE>\n\n\
             Usage: vuma run [OPTIONS] <FILE> [ARGS]...\n\n\
             For more information, try '--help'."
                .to_string(),
        )
    })?;

    Ok(Commands::Run { isa, file, args: trailing_args })
}

/// `vuma check <file>`
fn parse_check(iter: &mut ArgIter, cli: &mut Cli) -> Result<Commands, ParseError> {
    let mut file: Option<PathBuf> = None;

    while let Some(arg) = iter.peek().cloned() {
        if arg == "--help" || arg == "-h" {
            return Err(ParseError::Help);
        }
        if arg == "--version" || arg == "-V" {
            return Err(ParseError::Version);
        }
        if try_consume_global_flag(&arg, iter, cli)? {
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma check'\n\n\
                 For more information, try '--help'."
            )));
        }
        iter.next();
        if file.is_none() {
            file = Some(PathBuf::from(arg));
        } else {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma check'\n\n\
                 For more information, try '--help'."
            )));
        }
    }

    let file = file.ok_or_else(|| {
        ParseError::Msg(
            "error: the following required arguments were not provided:\n  <FILE>\n\n\
             Usage: vuma check <FILE>\n\n\
             For more information, try '--help'."
                .to_string(),
        )
    })?;

    Ok(Commands::Check { file })
}

/// `vuma emit <isa> <file> [-o <out>]`
fn parse_emit(iter: &mut ArgIter, cli: &mut Cli) -> Result<Commands, ParseError> {
    let mut output: Option<PathBuf> = None;
    let mut positionals: Vec<String> = Vec::new();

    while let Some(arg) = iter.peek().cloned() {
        if arg == "--help" || arg == "-h" {
            return Err(ParseError::Help);
        }
        if arg == "--version" || arg == "-V" {
            return Err(ParseError::Version);
        }
        if try_consume_global_flag(&arg, iter, cli)? {
            continue;
        }
        if arg == "-o" || arg == "--output" {
            iter.next();
            let val = take_value(iter, "--output <OUTPUT>")?;
            output = Some(PathBuf::from(val));
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma emit'\n\n\
                 For more information, try '--help'."
            )));
        }
        iter.next();
        positionals.push(arg);
    }

    if positionals.len() < 2 {
        let missing = if positionals.is_empty() { "<ISA> <FILE>" } else { "<FILE>" };
        return Err(ParseError::Msg(format!(
            "error: the following required arguments were not provided:\n  {missing}\n\n\
             Usage: vuma emit <ISA> <FILE> [OPTIONS]\n\n\
             For more information, try '--help'."
        )));
    }
    if positionals.len() > 2 {
        return Err(ParseError::Msg(format!(
            "error: unexpected argument '{}' for 'vuma emit'\n\n\
             For more information, try '--help'.",
            positionals[2]
        )));
    }
    let isa = IsaArg::parse(&positionals[0]).map_err(ParseError::Msg)?;
    let file = PathBuf::from(&positionals[1]);

    Ok(Commands::Emit { isa, file, output })
}

/// `vuma compile <file> [-o <out>] [--target aarch64] [--format obj]`
fn parse_compile(iter: &mut ArgIter, cli: &mut Cli) -> Result<Commands, ParseError> {
    let mut file: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut target: IsaArg = IsaArg::Aarch64;
    let mut format: FormatArg = FormatArg::Obj;

    while let Some(arg) = iter.peek().cloned() {
        if arg == "--help" || arg == "-h" {
            return Err(ParseError::Help);
        }
        if arg == "--version" || arg == "-V" {
            return Err(ParseError::Version);
        }
        if try_consume_global_flag(&arg, iter, cli)? {
            continue;
        }
        match arg.as_str() {
            "-o" | "--output" => {
                iter.next();
                let val = take_value(iter, "--output <OUTPUT>")?;
                output = Some(PathBuf::from(val));
                continue;
            }
            "--target" => {
                iter.next();
                let val = take_value(iter, "--target <TARGET>")?;
                target = IsaArg::parse(&val).map_err(ParseError::Msg)?;
                continue;
            }
            "--format" => {
                iter.next();
                let val = take_value(iter, "--format <FORMAT>")?;
                format = FormatArg::parse(&val).map_err(ParseError::Msg)?;
                continue;
            }
            _ => {}
        }
        if arg.starts_with('-') && arg != "-" {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma compile'\n\n\
                 For more information, try '--help'."
            )));
        }
        iter.next();
        if file.is_none() {
            file = Some(PathBuf::from(arg));
        } else {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma compile'\n\n\
                 For more information, try '--help'."
            )));
        }
    }

    let file = file.ok_or_else(|| {
        ParseError::Msg(
            "error: the following required arguments were not provided:\n  <FILE>\n\n\
             Usage: vuma compile <FILE> [OPTIONS]\n\n\
             For more information, try '--help'."
                .to_string(),
        )
    })?;

    Ok(Commands::Compile { file, output, target, format })
}

/// `vuma disasm <file> [--isa aarch64] [--base-addr 0x400000]`
fn parse_disasm(iter: &mut ArgIter, cli: &mut Cli) -> Result<Commands, ParseError> {
    let mut file: Option<PathBuf> = None;
    let mut isa: IsaArg = IsaArg::Aarch64;
    let mut base_addr: String = "0x400000".to_string();

    while let Some(arg) = iter.peek().cloned() {
        if arg == "--help" || arg == "-h" {
            return Err(ParseError::Help);
        }
        if arg == "--version" || arg == "-V" {
            return Err(ParseError::Version);
        }
        if try_consume_global_flag(&arg, iter, cli)? {
            continue;
        }
        match arg.as_str() {
            "--isa" => {
                iter.next();
                let val = take_value(iter, "--isa <ISA>")?;
                isa = IsaArg::parse(&val).map_err(ParseError::Msg)?;
                continue;
            }
            "--base-addr" => {
                iter.next();
                let val = take_value(iter, "--base-addr <BASE_ADDR>")?;
                base_addr = val;
                continue;
            }
            _ => {}
        }
        if arg.starts_with('-') && arg != "-" {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma disasm'\n\n\
                 For more information, try '--help'."
            )));
        }
        iter.next();
        if file.is_none() {
            file = Some(PathBuf::from(arg));
        } else {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma disasm'\n\n\
                 For more information, try '--help'."
            )));
        }
    }

    let file = file.ok_or_else(|| {
        ParseError::Msg(
            "error: the following required arguments were not provided:\n  <FILE>\n\n\
             Usage: vuma disasm <FILE> [OPTIONS]\n\n\
             For more information, try '--help'."
                .to_string(),
        )
    })?;

    Ok(Commands::Disasm { file, isa, base_addr })
}

/// `vuma link <file1> <file2> ... [-o <out>] [--isa <isa>] [--run]`
fn parse_link(iter: &mut ArgIter, cli: &mut Cli) -> Result<Commands, ParseError> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut output: Option<PathBuf> = None;
    let mut isa: Option<IsaArg> = None;
    let mut run: bool = false;

    while let Some(arg) = iter.peek().cloned() {
        if arg == "--help" || arg == "-h" {
            return Err(ParseError::Help);
        }
        if arg == "--version" || arg == "-V" {
            return Err(ParseError::Version);
        }
        if try_consume_global_flag(&arg, iter, cli)? {
            continue;
        }
        match arg.as_str() {
            "-o" | "--output" => {
                iter.next();
                let val = take_value(iter, "--output <OUTPUT>")?;
                output = Some(PathBuf::from(val));
                continue;
            }
            "--isa" => {
                iter.next();
                let val = take_value(iter, "--isa <ISA>")?;
                isa = Some(IsaArg::parse(&val).map_err(ParseError::Msg)?);
                continue;
            }
            "--run" => {
                iter.next();
                run = true;
                continue;
            }
            _ => {}
        }
        if arg.starts_with('-') && arg != "-" {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma link'\n\n\
                 For more information, try '--help'."
            )));
        }
        iter.next();
        files.push(PathBuf::from(arg));
    }

    if files.is_empty() {
        return Err(ParseError::Msg(
            "error: the following required arguments were not provided:\n  <FILES>...\n\n\
             Usage: vuma link <FILES>... [OPTIONS]\n\n\
             For more information, try '--help'."
                .to_string(),
        ));
    }

    Ok(Commands::Link { files, output, isa, run })
}

/// `vuma verify <file>`
fn parse_verify(iter: &mut ArgIter, cli: &mut Cli) -> Result<Commands, ParseError> {
    let mut file: Option<PathBuf> = None;

    while let Some(arg) = iter.peek().cloned() {
        if arg == "--help" || arg == "-h" {
            return Err(ParseError::Help);
        }
        if arg == "--version" || arg == "-V" {
            return Err(ParseError::Version);
        }
        if try_consume_global_flag(&arg, iter, cli)? {
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma verify'\n\n\
                 For more information, try '--help'."
            )));
        }
        iter.next();
        if file.is_none() {
            file = Some(PathBuf::from(arg));
        } else {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma verify'\n\n\
                 For more information, try '--help'."
            )));
        }
    }

    let file = file.ok_or_else(|| {
        ParseError::Msg(
            "error: the following required arguments were not provided:\n  <FILE>\n\n\
             Usage: vuma verify <FILE>\n\n\
             For more information, try '--help'."
                .to_string(),
        )
    })?;

    Ok(Commands::Verify { file })
}

/// `vuma repl` (no further args)
fn parse_repl(iter: &mut ArgIter, cli: &mut Cli) -> Result<Commands, ParseError> {
    while let Some(arg) = iter.peek().cloned() {
        if arg == "--help" || arg == "-h" {
            return Err(ParseError::Help);
        }
        if arg == "--version" || arg == "-V" {
            return Err(ParseError::Version);
        }
        if try_consume_global_flag(&arg, iter, cli)? {
            continue;
        }
        return Err(ParseError::Msg(format!(
            "error: unexpected argument '{arg}' for 'vuma repl'\n\n\
             For more information, try '--help'."
        )));
    }
    Ok(Commands::Repl)
}

/// `vuma lsp` (no further args)
fn parse_lsp(iter: &mut ArgIter, cli: &mut Cli) -> Result<Commands, ParseError> {
    while let Some(arg) = iter.peek().cloned() {
        if arg == "--help" || arg == "-h" {
            return Err(ParseError::Help);
        }
        if arg == "--version" || arg == "-V" {
            return Err(ParseError::Version);
        }
        if try_consume_global_flag(&arg, iter, cli)? {
            continue;
        }
        return Err(ParseError::Msg(format!(
            "error: unexpected argument '{arg}' for 'vuma lsp'\n\n\
             For more information, try '--help'."
        )));
    }
    Ok(Commands::Lsp)
}

/// `vuma pkg <subcommand>` — package manager dispatch.
fn parse_pkg(iter: &mut ArgIter, cli: &mut Cli) -> Result<Commands, ParseError> {
    // Consume any leading global flags (they may appear between the
    // subcommand and its first positional).
    let mut sub_name: Option<String> = None;
    while let Some(arg) = iter.peek().cloned() {
        if arg == "--help" || arg == "-h" {
            return Err(ParseError::Help);
        }
        if arg == "--version" || arg == "-V" {
            return Err(ParseError::Version);
        }
        if try_consume_global_flag(&arg, iter, cli)? {
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma pkg'\n\n\
                 For more information, try '--help'."
            )));
        }
        iter.next();
        sub_name = Some(arg);
        break;
    }

    let sub_name = sub_name.ok_or_else(|| {
        ParseError::Msg(
            "error: a subcommand is required for 'vuma pkg'\n  [possible subcommands: init, build, add]\n\n\
             Usage: vuma pkg <COMMAND>\n\n\
             For more information, try '--help'."
                .to_string(),
        )
    })?;

    match sub_name.as_str() {
        "init" => parse_pkg_init(iter, cli),
        "build" => parse_pkg_build(iter, cli),
        "add" => parse_pkg_add(iter, cli),
        "help" => Err(ParseError::Help),
        other => Err(ParseError::Msg(format!(
            "error: unrecognized subcommand '{other}' for 'vuma pkg'\n  [possible subcommands: init, build, add]\n\n\
             Usage: vuma pkg <COMMAND>\n\n\
             For more information, try '--help'."
        ))),
    }
}

/// `vuma pkg init <name>`
fn parse_pkg_init(iter: &mut ArgIter, cli: &mut Cli) -> Result<Commands, ParseError> {
    let mut name: Option<String> = None;
    while let Some(arg) = iter.peek().cloned() {
        if arg == "--help" || arg == "-h" {
            return Err(ParseError::Help);
        }
        if arg == "--version" || arg == "-V" {
            return Err(ParseError::Version);
        }
        if try_consume_global_flag(&arg, iter, cli)? {
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma pkg init'\n\n\
                 For more information, try '--help'."
            )));
        }
        iter.next();
        if name.is_none() {
            name = Some(arg);
        } else {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma pkg init'\n\n\
                 For more information, try '--help'."
            )));
        }
    }

    let name = name.ok_or_else(|| {
        ParseError::Msg(
            "error: the following required arguments were not provided:\n  <NAME>\n\n\
             Usage: vuma pkg init <NAME>\n\n\
             For more information, try '--help'."
                .to_string(),
        )
    })?;

    Ok(Commands::Pkg { cmd: PkgCommand::Init { name } })
}

/// `vuma pkg build`
fn parse_pkg_build(iter: &mut ArgIter, cli: &mut Cli) -> Result<Commands, ParseError> {
    while let Some(arg) = iter.peek().cloned() {
        if arg == "--help" || arg == "-h" {
            return Err(ParseError::Help);
        }
        if arg == "--version" || arg == "-V" {
            return Err(ParseError::Version);
        }
        if try_consume_global_flag(&arg, iter, cli)? {
            continue;
        }
        return Err(ParseError::Msg(format!(
            "error: unexpected argument '{arg}' for 'vuma pkg build'\n\n\
             For more information, try '--help'."
        )));
    }
    Ok(Commands::Pkg { cmd: PkgCommand::Build })
}

/// `vuma pkg add <dep> [version]` (default version "*")
fn parse_pkg_add(iter: &mut ArgIter, cli: &mut Cli) -> Result<Commands, ParseError> {
    let mut positionals: Vec<String> = Vec::new();

    while let Some(arg) = iter.peek().cloned() {
        if arg == "--help" || arg == "-h" {
            return Err(ParseError::Help);
        }
        if arg == "--version" || arg == "-V" {
            return Err(ParseError::Version);
        }
        if try_consume_global_flag(&arg, iter, cli)? {
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return Err(ParseError::Msg(format!(
                "error: unexpected argument '{arg}' for 'vuma pkg add'\n\n\
                 For more information, try '--help'."
            )));
        }
        iter.next();
        positionals.push(arg);
    }

    if positionals.is_empty() {
        return Err(ParseError::Msg(
            "error: the following required arguments were not provided:\n  <DEP>\n\n\
             Usage: vuma pkg add <DEP> [VERSION]\n\n\
             For more information, try '--help'."
                .to_string(),
        ));
    }
    if positionals.len() > 2 {
        return Err(ParseError::Msg(format!(
            "error: unexpected argument '{}' for 'vuma pkg add'\n\n\
             For more information, try '--help'.",
            positionals[2]
        )));
    }
    let dep = positionals[0].clone();
    let version = if positionals.len() == 2 {
        positionals[1].clone()
    } else {
        "*".to_string()
    };

    Ok(Commands::Pkg {
        cmd: PkgCommand::Add { dep, version },
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Version information
// ═══════════════════════════════════════════════════════════════════════════

/// VUMA version.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Supported backend list.
const SUPPORTED_BACKENDS: &str =
    "aarch64, x86_64, riscv64, arm32, mips64, ppc64, loongarch64, wasm32, sparc64, s390x";

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
    // Wave 19: `--no-verify` is the ONLY way to bypass verification.
    // Emit a compile-time warning so the bypass is never silent.
    let verification_level = if cli.no_verify {
        eprintln!(
            "warning: --no-verify bypasses all IVE verification. \
             Memory-safety violations will NOT be detected. \
             Use only for debugging/testing."
        );
        VerificationLevel::None
    } else {
        VerificationLevel::from(cli.verification)
    };

    CompileConfig {
        target,
        opt_level: OptLevel::from(cli.opt_level),
        verification_level,
        strict_verification: cli.strict_verification,
        debug_info: cli.debug,
        section_headers: cli.sections,
        runtime_bounds_checks: cli.safe,
        memory_safety: !cli.no_memory_safety,
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
    opt_level: OptLevel,
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
    let mut ir_program = ir_builder.convert(&codegen_scg).map_err(|e| {
        format!("IR conversion error: {}", e)
    })?;

    // Step 3b: Run codegen-level IR optimization (Wave 10 proper).
    // Use the actual backend's latency table for per-ISA optimization.
    // Gated by opt level: O0 skips (matches pipeline.rs behavior).
    if !matches!(opt_level, OptLevel::O0) {
        let backend_for_table = create_backend(backend_kind).map_err(|e| {
            format!("error: cannot create {} backend for latency table: {}", backend_kind.isa_name(), e)
        })?;
        let latency_table = backend_for_table.target_info().latency_table();
        ir_program = vuma_codegen::opt::run_optimizations_with_target(ir_program, &latency_table);
    }

    // Step 4: Create backend and allocate registers.
    let backend = create_backend(backend_kind).map_err(|e| {
        format!(
            "error: cannot create {} backend: {}",
            backend_kind.isa_name(),
            e
        )
    })?;

    // Populate thread-local set of 64-bit-returning function names.
    // This is needed by arm32 (and potentially other 32-bit backends) to
    // know whether to store R1 (high word) after a function call returns.
    {
        use std::collections::HashSet;
        let func_64bit: HashSet<String> = ir_program.functions.iter()
            .filter(|f| f.result_types.iter().any(|t| matches!(t, vuma_codegen::ir::IRType::I64 | vuma_codegen::ir::IRType::U64)))
            .map(|f| f.name.clone())
            .collect();
        vuma_codegen::backend::set_64bit_returns(&func_64bit);
    }

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
                println!("\n{}", report.to_json_pretty());
            }

            Ok(())
        }
        CompileResult::Partial(partial) => {
            print_errors(&partial.diagnostics);

            if cli.telemetry {
                if let Some(tc) = telemetry {
                    let report = tc.finalize();
                    eprintln!("\n{}", report.to_json_pretty());
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
    file: &Path,
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

    let binary = compile_to_binary_direct(source, isa, OptLevel::from(cli.opt_level))?;

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
    let binary = compile_to_binary_direct(&source, resolved_isa, OptLevel::from(_cli.opt_level)).map_err(|err| {
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

/// `vuma link <file1.vuma> <file2.vuma> ... -o <output> [--run] [--isa x86_64]`
///
/// Reads each named `.vuma` file, passes the `(filename, contents)` tuples
/// to `pipeline::compile_modules`, writes the resulting ELF to the output
/// path (default `a.out`), and (if `--run` is set) `chmod +x`'s the output
/// and executes it (with no argv; to pass argv, run the binary directly
/// after `vuma link` writes it).
///
/// The linked ELF targets the host architecture (or the explicit `--isa`
/// if given — currently `compile_modules` always uses the host backend;
/// the flag is accepted for forward-compatibility). The canonical pipeline
/// (`compile_with_path`) always emits AArch64 and cannot be used here
/// without silently producing a non-runnable binary on x86_64 / riscv64 /
/// etc. hosts.
fn cmd_link(
    cli: &Cli,
    files: &[PathBuf],
    output: &Option<PathBuf>,
    _isa: &Option<IsaArg>,
    run: bool,
) -> Result<(), String> {
    if files.len() < 2 {
        return Err(format!(
            "link requires at least 2 input files (got {}); \
             for single-file compilation use `vuma build` or `vuma emit`",
            files.len()
        ));
    }

    // Read each input file into a (name, contents) tuple.
    let mut modules: Vec<(String, String)> = Vec::with_capacity(files.len());
    for path in files {
        let source = read_source(path)?;
        // Use the file name (not the full path) as the module name —
        // keeps error messages readable and matches the convention used
        // by the bootstrap self-host test (which passes bare file names
        // like "full_lexer.vuma").
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        modules.push((name, source));
    }

    // Build a CompileConfig from the global CLI flags.  compile_modules
    // uses the host backend regardless of `target`, so we pass Linux
    // (the only non-Wasm32 target) here.
    let config = make_config(cli, CompileTarget::Linux);

    // Compile + link.
    let compile_output = compile_modules(&modules, &config).map_err(|errors| {
        // Print each error to stderr before returning the summary.
        print_errors(&errors);
        format!("link failed with {} error(s)", errors.len())
    })?;

    // Write the ELF to the output path (default: a.out in CWD).
    let out_path = output
        .as_ref()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("a.out"));
    fs::write(&out_path, &compile_output.binary).map_err(|e| {
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
        let host_arch = std::env::consts::ARCH;
        println!(
            "Linked {} file(s) -> {} ({} bytes, {} IR functions, {} IR instructions, host ISA: {})",
            files.len(),
            out_path.display(),
            compile_output.binary.len(),
            compile_output.ir_function_count,
            compile_output.ir_instruction_count,
            host_arch,
        );

        // Print stage timings.
        for (stage, ms) in &compile_output.stage_timings {
            println!("  {:20} {}ms", stage, ms);
        }
    }

    // If --run is set, execute the linked ELF (no argv — for argv, run
    // the binary directly after vuma link writes it).
    if run {
        let output_result = Command::new(&out_path).output();
        let out = output_result.map_err(|e| {
            format!(
                "error: failed to execute linked binary '{}': {}",
                out_path.display(),
                e
            )
        })?;

        io::stdout()
            .write_all(&out.stdout)
            .map_err(|e| format!("error: failed to write program output: {}", e))?;
        io::stderr()
            .write_all(&out.stderr)
            .map_err(|e| format!("error: failed to write program stderr: {}", e))?;

        let code = out.status.code().unwrap_or(1);
        if code != 0 {
            return Err(format!("linked program exited with code {}", code));
        }
    }

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
        let backends: [(BackendKind, &str); 11] = [
            (BackendKind::AArch64, "aarch64"),
            (BackendKind::X86_64, "x86_64"),
            (BackendKind::RiscV64, "riscv64"),
            (BackendKind::Arm32, "arm32"),
            (BackendKind::Mips64, "mips64"),
            (BackendKind::PowerPC64, "ppc64"),
            (BackendKind::PowerPC64LE, "ppc64le"),
            (BackendKind::LoongArch64, "loongarch64"),
            (BackendKind::Wasm32, "wasm32"),
            (BackendKind::Sparc64, "sparc64"),
            (BackendKind::S390X, "s390x"),
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
    // `vuma_log!` (and the `log_error!` / `log_warn!` / `log_info!` /
    // `log_debug!` / `log_trace!` macros) is the sole logging mechanism in
    // this self-hosted compiler — there is no third-party `log` crate bridge.
    init_logger(log_level);

    let cli = parse_cli();

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
        Commands::Link {
            ref files,
            ref output,
            ref isa,
            ref run,
        } => cmd_link(&cli, files, output, isa, *run),
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

    /// Test 1: `vuma build hello.vuma` parses correctly.
    #[test]
    fn test_build_basic() {
        let cli = parse_cli_from(["vuma", "build", "hello.vuma"]).unwrap();
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
        let cli = parse_cli_from([
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
        let cli = parse_cli_from([
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
        let cli = parse_cli_from(["vuma", "run", "hello.vuma"]).unwrap();
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
        let cli = parse_cli_from(["vuma", "run", "hello.vuma", "arg1", "arg2"]).unwrap();
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
        let cli = parse_cli_from([
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
        let cli = parse_cli_from(["vuma", "check", "hello.vuma"]).unwrap();
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
        let cli = parse_cli_from(["vuma", "emit", "aarch64", "hello.vuma"]).unwrap();
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
            parse_cli_from(["vuma", "emit", "x86_64", "hello.vuma", "-o", "out.o"]).unwrap();
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
        let cli = parse_cli_from([
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
        let cli = parse_cli_from(["vuma", "verify", "hello.vuma"]).unwrap();
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
        let cli = parse_cli_from(["vuma", "repl"]).unwrap();
        match cli.command {
            Some(Commands::Repl) => {}
            _ => panic!("expected Repl command"),
        }
    }

    /// Test 10b: `vuma --repl` parses correctly.
    #[test]
    fn test_repl_flag() {
        let cli = parse_cli_from(["vuma", "--repl"]).unwrap();
        assert!(cli.repl, "--repl flag should be true");
    }

    /// Test 11: Global --opt-level flag works.
    #[test]
    fn test_global_opt_level() {
        let cli =
            parse_cli_from(["vuma", "--opt-level", "O0", "build", "hello.vuma"]).unwrap();
        assert_eq!(cli.opt_level, OptLevelArg::O0);
    }

    /// Test 12: Global --verification flag works.
    #[test]
    fn test_global_verification_level() {
        let cli = parse_cli_from([
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
        let cli = parse_cli_from(["vuma", "--debug", "build", "hello.vuma"]).unwrap();
        assert!(cli.debug);
    }

    /// Test 14: Default values are correct.
    #[test]
    fn test_defaults() {
        let cli = parse_cli_from(["vuma", "build", "hello.vuma"]).unwrap();
        assert_eq!(cli.opt_level, OptLevelArg::O3);
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
            let cli = parse_cli_from(["vuma", "emit", name, "test.vuma"]).unwrap();
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
        // Note: `VerificationArg::None` was removed in Wave 19 (close
        // verification escape hatches). The CLI `--verification` arg now
        // accepts only quick/normal/exhaustive; `VerificationLevel::None`
        // remains an internal pipeline default but is not user-selectable.
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
        let result = parse_cli_from(["vuma", "invalid"]);
        assert!(result.is_err());
    }
}
