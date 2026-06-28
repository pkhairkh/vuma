//! # Foreign Function Interface (FFI) for VUMA
//!
//! Provides the FFI infrastructure for calling external C functions and
//! Linux syscalls from VUMA programs.
//!
//! # Overview
//!
//! The FFI module handles:
//!
//! - **`extern "C"` block syntax** — declares external functions in VUMA source
//! - **Syscall bindings** — Linux kernel interfaces: `write`, `read`, `exit`,
//!   `mmap`, `munmap`, `brk`
//! - **C library bindings** — libc functions: `memcpy`, `memset`, `malloc`,
//!   `free`
//! - **Codegen support** — extern function calls emit relocations instead of
//!   local `BL` instructions
//!
//! # Example VUMA Source
//!
//! ```vuma
//! extern "C" {
//!     fn write(fd: i64, buf: Address, count: i64) -> i64;
//!     fn read(fd: i64, buf: Address, count: i64) -> i64;
//!     fn exit(code: i64);
//! }
//!
//! fn main() {
//!     write(1, 0x400000, 13);
//!     exit(0);
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;

// ---------------------------------------------------------------------------
// ExternBlock — AST representation
// ---------------------------------------------------------------------------

/// An `extern "C" { ... }` block declaring external functions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExternBlock {
    /// The calling convention (e.g. "C", "system").
    pub convention: CallingConvention,
    /// Functions declared in this block.
    pub functions: Vec<ExternFn>,
}

/// A function declared inside an `extern` block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExternFn {
    /// Function name (as it appears in VUMA source and in the linker symbol table).
    pub name: String,
    /// Parameter types.
    pub param_types: Vec<ExternType>,
    /// Return type (None = void).
    pub return_type: Option<ExternType>,
}

/// Calling convention for extern blocks.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CallingConvention {
    /// C calling convention (System V ABI on Linux, Microsoft on Windows).
    C,
    /// Platform default calling convention.
    System,
    /// VUMA internal calling convention.
    Vuma,
}

impl fmt::Display for CallingConvention {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallingConvention::C => write!(f, "C"),
            CallingConvention::System => write!(f, "system"),
            CallingConvention::Vuma => write!(f, "vuma"),
        }
    }
}

/// Types used in extern function declarations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExternType {
    /// `i8`
    I8,
    /// `i16`
    I16,
    /// `i32`
    I32,
    /// `i64`
    I64,
    /// `u8`
    U8,
    /// `u16`
    U16,
    /// `u32`
    U32,
    /// `u64`
    U64,
    /// `f32`
    F32,
    /// `f64`
    F64,
    /// Raw pointer (pointer-sized).
    Ptr,
    /// Void (only valid as return type).
    Void,
}

impl ExternType {
    /// Returns the size in bytes for this type on a 64-bit platform.
    pub fn size_64bit(&self) -> usize {
        match self {
            ExternType::I8 | ExternType::U8 => 1,
            ExternType::I16 | ExternType::U16 => 2,
            ExternType::I32 | ExternType::U32 | ExternType::F32 => 4,
            ExternType::I64 | ExternType::U64 | ExternType::F64 | ExternType::Ptr => 8,
            ExternType::Void => 0,
        }
    }
}

impl fmt::Display for ExternType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExternType::I8 => write!(f, "i8"),
            ExternType::I16 => write!(f, "i16"),
            ExternType::I32 => write!(f, "i32"),
            ExternType::I64 => write!(f, "i64"),
            ExternType::U8 => write!(f, "u8"),
            ExternType::U16 => write!(f, "u16"),
            ExternType::U32 => write!(f, "u32"),
            ExternType::U64 => write!(f, "u64"),
            ExternType::F32 => write!(f, "f32"),
            ExternType::F64 => write!(f, "f64"),
            ExternType::Ptr => write!(f, "ptr"),
            ExternType::Void => write!(f, "void"),
        }
    }
}

// ---------------------------------------------------------------------------
// Pre-defined binding tables
// ---------------------------------------------------------------------------

/// Returns the standard Linux syscall bindings as an `ExternBlock`.
pub fn linux_syscall_bindings() -> ExternBlock {
    ExternBlock {
        convention: CallingConvention::C,
        functions: vec![
            ExternFn {
                name: "write".to_string(),
                param_types: vec![ExternType::I64, ExternType::Ptr, ExternType::I64],
                return_type: Some(ExternType::I64),
            },
            ExternFn {
                name: "read".to_string(),
                param_types: vec![ExternType::I64, ExternType::Ptr, ExternType::I64],
                return_type: Some(ExternType::I64),
            },
            ExternFn {
                name: "exit".to_string(),
                param_types: vec![ExternType::I64],
                return_type: None,
            },
            ExternFn {
                name: "mmap".to_string(),
                param_types: vec![
                    ExternType::Ptr,  // addr
                    ExternType::I64,  // length
                    ExternType::I64,  // prot
                    ExternType::I64,  // flags
                    ExternType::I64,  // fd
                    ExternType::I64,  // offset
                ],
                return_type: Some(ExternType::Ptr),
            },
            ExternFn {
                name: "munmap".to_string(),
                param_types: vec![ExternType::Ptr, ExternType::I64],
                return_type: Some(ExternType::I64),
            },
            ExternFn {
                name: "brk".to_string(),
                param_types: vec![ExternType::Ptr],
                return_type: Some(ExternType::Ptr),
            },
        ],
    }
}

/// Returns the standard C library bindings as an `ExternBlock`.
pub fn c_library_bindings() -> ExternBlock {
    ExternBlock {
        convention: CallingConvention::C,
        functions: vec![
            ExternFn {
                name: "memcpy".to_string(),
                param_types: vec![ExternType::Ptr, ExternType::Ptr, ExternType::I64],
                return_type: Some(ExternType::Ptr),
            },
            ExternFn {
                name: "memset".to_string(),
                param_types: vec![ExternType::Ptr, ExternType::I64, ExternType::I64],
                return_type: Some(ExternType::Ptr),
            },
            ExternFn {
                name: "malloc".to_string(),
                param_types: vec![ExternType::I64],
                return_type: Some(ExternType::Ptr),
            },
            ExternFn {
                name: "free".to_string(),
                param_types: vec![ExternType::Ptr],
                return_type: None,
            },
        ],
    }
}

// ---------------------------------------------------------------------------
// ExternRegistry — tracks all known extern functions
// ---------------------------------------------------------------------------

/// Registry of all known extern functions, built from `extern` blocks
/// in the source code and the built-in binding tables.
#[derive(Debug, Clone, Default)]
pub struct ExternRegistry {
    /// Map from function name to its extern declaration.
    functions: HashMap<String, ExternFn>,
    /// Map from function name to its calling convention.
    conventions: HashMap<String, CallingConvention>,
}

impl ExternRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a registry pre-populated with Linux syscall and C library bindings.
    pub fn with_default_bindings() -> Self {
        let mut registry = Self::new();
        registry.register_block(&linux_syscall_bindings());
        registry.register_block(&c_library_bindings());
        registry
    }

    /// Register all functions from an `extern` block.
    pub fn register_block(&mut self, block: &ExternBlock) {
        for func in &block.functions {
            self.functions.insert(func.name.clone(), func.clone());
            self.conventions.insert(func.name.clone(), block.convention);
        }
    }

    /// Look up an extern function by name.
    pub fn get(&self, name: &str) -> Option<&ExternFn> {
        self.functions.get(name)
    }

    /// Check if a function name is a known extern function.
    pub fn is_extern(&self, name: &str) -> bool {
        self.functions.contains_key(name)
    }

    /// Get the calling convention for a function.
    pub fn convention(&self, name: &str) -> Option<CallingConvention> {
        self.conventions.get(name).copied()
    }

    /// Returns true if a call to this function should be emitted as a
    /// relocation (external symbol) rather than a local branch.
    pub fn needs_relocation(&self, name: &str) -> bool {
        self.is_extern(name)
    }

    /// List all registered extern function names.
    pub fn function_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.functions.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    /// Returns the set of all registered extern function names.
    /// Useful for passing to the codegen bridge so that calls to
    /// extern functions are marked with `is_extern: true`.
    pub fn function_name_set(&self) -> HashSet<String> {
        self.functions.keys().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// ExternBlock → ExternRegistry conversion
// ---------------------------------------------------------------------------

impl ExternBlock {
    /// Convert this block into an `ExternRegistry`.
    pub fn to_registry(&self) -> ExternRegistry {
        let mut registry = ExternRegistry::new();
        registry.register_block(self);
        registry
    }

    /// Returns the set of function names declared in this block.
    pub fn function_names_set(&self) -> HashSet<String> {
        self.functions.iter().map(|f| f.name.clone()).collect()
    }
}

// ---------------------------------------------------------------------------
// Relocation types for the codegen
// ---------------------------------------------------------------------------

/// A relocation entry emitted for an extern function call.
///
/// Instead of emitting a local `BL <symbol>` instruction, the codegen
/// emits a relocation that the linker will resolve at link time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Relocation {
    /// Offset in the code section where the relocation applies.
    pub offset: u64,
    /// The kind of relocation.
    pub kind: RelocationKind,
    /// The external symbol name to resolve.
    pub symbol: String,
    /// Addend (typically 0 for function calls).
    pub addend: i64,
}

/// The kind of relocation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RelocationKind {
    /// AArch64: R_AARCH64_CALL26 — 26-bit relative call.
    AArch64Call26,
    /// AArch64: R_AARCH64_ADR_PREL_PG_HI21 + R_AARCH64_LDST64_ABS_LO12_NC
    /// for loading a 64-bit address.
    AArch64AdrpPage,
    /// x86_64: R_X86_64_PLT32 — 32-bit PC-relative PLT call.
    X86_64Plt32,
    /// x86_64: R_X86_64_64 — absolute 64-bit address.
    X86_64Abs64,
    /// RISC-V: R_RISCV_CALL — auipc + jalr pair.
    RiscvCall,
    /// ARM32: R_ARM_CALL / R_ARM_JUMP24 — 24-bit relative branch.
    Arm32Call,
    /// ARM32: R_ARM_V4BX — branch exchange (for Thumb interworking).
    Arm32V4bx,
    /// MIPS64: R_MIPS_26 — 26-bit jump target.
    Mips26,
    /// MIPS64: R_MIPS_GOT_CALL — GOT-based call.
    MipsGotCall,
    /// PPC64: R_PPC64_REL24 — 24-bit relative branch.
    Ppc64Rel24,
    /// PPC64: R_PPC64_REL64 — 64-bit relative address.
    Ppc64Rel64,
    /// LoongArch64: R_LARCH_B26 — 26-bit relative call.
    LoongArchB26,
    /// LoongArch64: R_LARCH_CALL36 — 36-bit call sequence.
    LoongArchCall36,
    /// x86_32: R_386_PLT32 — 32-bit PC-relative PLT call.
    X86_32Plt32,
    /// Generic: 32-bit PC-relative call (fallback).
    GenericCall32,
    /// Generic: 64-bit absolute address (fallback).
    GenericAbs64,
}

impl fmt::Display for RelocationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RelocationKind::AArch64Call26 => write!(f, "R_AARCH64_CALL26"),
            RelocationKind::AArch64AdrpPage => write!(f, "R_AARCH64_ADR_PREL_PG_HI21"),
            RelocationKind::X86_64Plt32 => write!(f, "R_X86_64_PLT32"),
            RelocationKind::X86_64Abs64 => write!(f, "R_X86_64_64"),
            RelocationKind::RiscvCall => write!(f, "R_RISCV_CALL"),
            RelocationKind::Arm32Call => write!(f, "R_ARM_CALL"),
            RelocationKind::Arm32V4bx => write!(f, "R_ARM_V4BX"),
            RelocationKind::Mips26 => write!(f, "R_MIPS_26"),
            RelocationKind::MipsGotCall => write!(f, "R_MIPS_GOT_CALL"),
            RelocationKind::Ppc64Rel24 => write!(f, "R_PPC64_REL24"),
            RelocationKind::Ppc64Rel64 => write!(f, "R_PPC64_REL64"),
            RelocationKind::LoongArchB26 => write!(f, "R_LARCH_B26"),
            RelocationKind::LoongArchCall36 => write!(f, "R_LARCH_CALL36"),
            RelocationKind::X86_32Plt32 => write!(f, "R_386_PLT32"),
            RelocationKind::GenericCall32 => write!(f, "GENERIC_CALL32"),
            RelocationKind::GenericAbs64 => write!(f, "GENERIC_ABS64"),
        }
    }
}

impl RelocationKind {
    /// Returns the appropriate relocation kind for a function call on the
    /// given target architecture.
    pub fn for_arch(arch: &str) -> Self {
        match arch {
            "aarch64" => RelocationKind::AArch64Call26,
            "x86_64" => RelocationKind::X86_64Plt32,
            "riscv64" | "riscv32" => RelocationKind::RiscvCall,
            "arm32" => RelocationKind::Arm32Call,
            "mips64" => RelocationKind::Mips26,
            "ppc64" => RelocationKind::Ppc64Rel24,
            "loongarch64" => RelocationKind::LoongArchB26,
            "x86_32" | "i386" => RelocationKind::X86_32Plt32,
            _ => RelocationKind::GenericCall32,
        }
    }
}

// ---------------------------------------------------------------------------
// SyscallTable — Linux syscall numbers per architecture
// ---------------------------------------------------------------------------

/// The target architecture for syscall number lookup.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Arch {
    /// x86_64 (AMD64)
    X86_64,
    /// AArch64 (ARM64)
    AArch64,
    /// RISC-V 64-bit
    RiscV64,
    /// ARM 32-bit (AARCH32)
    Arm32,
    /// MIPS64
    Mips64,
    /// PowerPC 64-bit (little-endian)
    PPC64,
    /// LoongArch64
    LoongArch64,
    /// x86 32-bit (i386)
    X86_32,
    /// RISC-V 32-bit
    RiscV32,
    /// Wasm32 (no native syscalls; uses wasi or external bindings)
    Wasm32,
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Arch::X86_64 => write!(f, "x86_64"),
            Arch::AArch64 => write!(f, "aarch64"),
            Arch::RiscV64 => write!(f, "riscv64"),
            Arch::Arm32 => write!(f, "arm32"),
            Arch::Mips64 => write!(f, "mips64"),
            Arch::PPC64 => write!(f, "ppc64"),
            Arch::LoongArch64 => write!(f, "loongarch64"),
            Arch::X86_32 => write!(f, "x86_32"),
            Arch::RiscV32 => write!(f, "riscv32"),
            Arch::Wasm32 => write!(f, "wasm32"),
        }
    }
}

impl Arch {
    /// Parse an architecture name string into an `Arch` value.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "x86_64" | "amd64" => Some(Arch::X86_64),
            "aarch64" | "arm64" => Some(Arch::AArch64),
            "riscv64" => Some(Arch::RiscV64),
            "arm32" | "arm" | "armv7" => Some(Arch::Arm32),
            "mips64" | "mips64el" => Some(Arch::Mips64),
            "ppc64" | "ppc64le" | "powerpc64" => Some(Arch::PPC64),
            "loongarch64" | "la64" => Some(Arch::LoongArch64),
            "x86_32" | "i386" | "x86" => Some(Arch::X86_32),
            "riscv32" | "rv32" => Some(Arch::RiscV32),
            "wasm32" => Some(Arch::Wasm32),
            _ => None,
        }
    }
}

/// Well-known Linux syscall names used by VUMA programs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SyscallName {
    /// `read` — read from a file descriptor.
    Read,
    /// `write` — write to a file descriptor.
    Write,
    /// `open` — open a file.
    Open,
    /// `close` — close a file descriptor.
    Close,
    /// `exit` — terminate the process.
    Exit,
    /// `exit_group` — exit all threads in the process.
    ExitGroup,
    /// `mmap` — map memory.
    Mmap,
    /// `munmap` — unmap memory.
    Munmap,
    /// `brk` — change data segment size.
    Brk,
    /// `ioctl` — device control.
    Ioctl,
    /// `fcntl` — file control.
    Fcntl,
    /// `getpid` — get process ID.
    Getpid,
    /// `kill` — send signal.
    Kill,
    /// `mprotect` — set memory protection.
    Mprotect,
    /// `clock_gettime` — get time.
    ClockGettime,
    /// `sched_yield` — yield the CPU.
    SchedYield,
    /// `clone` — create a new thread/process.
    Clone,
    /// `futex` — fast userspace mutex.
    Futex,
    /// `set_tid_address` — set thread ID pointer.
    SetTidAddress,
}

impl fmt::Display for SyscallName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyscallName::Read => write!(f, "read"),
            SyscallName::Write => write!(f, "write"),
            SyscallName::Open => write!(f, "open"),
            SyscallName::Close => write!(f, "close"),
            SyscallName::Exit => write!(f, "exit"),
            SyscallName::ExitGroup => write!(f, "exit_group"),
            SyscallName::Mmap => write!(f, "mmap"),
            SyscallName::Munmap => write!(f, "munmap"),
            SyscallName::Brk => write!(f, "brk"),
            SyscallName::Ioctl => write!(f, "ioctl"),
            SyscallName::Fcntl => write!(f, "fcntl"),
            SyscallName::Getpid => write!(f, "getpid"),
            SyscallName::Kill => write!(f, "kill"),
            SyscallName::Mprotect => write!(f, "mprotect"),
            SyscallName::ClockGettime => write!(f, "clock_gettime"),
            SyscallName::SchedYield => write!(f, "sched_yield"),
            SyscallName::Clone => write!(f, "clone"),
            SyscallName::Futex => write!(f, "futex"),
            SyscallName::SetTidAddress => write!(f, "set_tid_address"),
        }
    }
}

/// Per-architecture syscall number table for Linux.
///
/// Linux assigns different syscall numbers to each architecture. This struct
/// provides a unified lookup interface so that VUMA codegen can emit the
/// correct immediate for `svc #0` (AArch64), `syscall` (x86_64), `ecall`
/// (RISC-V), etc.
#[derive(Debug, Clone)]
pub struct SyscallTable {
    /// Architecture this table is for.
    pub arch: Arch,
    /// Map from syscall name to its number.
    numbers: HashMap<SyscallName, u64>,
}

impl SyscallTable {
    /// Create the syscall table for the given architecture.
    pub fn for_arch(arch: Arch) -> Self {
        let numbers = match arch {
            Arch::X86_64 => x86_64_syscalls(),
            Arch::AArch64 => aarch64_syscalls(),
            Arch::RiscV64 => riscv64_syscalls(),
            Arch::Arm32 => arm32_syscalls(),
            Arch::Mips64 => mips64_syscalls(),
            Arch::PPC64 => ppc64_syscalls(),
            Arch::LoongArch64 => loongarch64_syscalls(),
            Arch::X86_32 => x86_32_syscalls(),
            Arch::RiscV32 => riscv32_syscalls(),
            Arch::Wasm32 => HashMap::new(), // no native syscalls
        };
        Self { arch, numbers }
    }

    /// Look up the syscall number for a named syscall.
    pub fn get(&self, name: SyscallName) -> Option<u64> {
        self.numbers.get(&name).copied()
    }

    /// Returns the number of syscalls in this table.
    pub fn len(&self) -> usize {
        self.numbers.len()
    }

    /// Returns true if the table is empty.
    pub fn is_empty(&self) -> bool {
        self.numbers.is_empty()
    }

    /// Iterate over all (name, number) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (SyscallName, u64)> + '_ {
        self.numbers.iter().map(|(&k, &v)| (k, v))
    }
}

// ---------------------------------------------------------------------------
// Architecture-specific syscall tables
// ---------------------------------------------------------------------------

fn x86_64_syscalls() -> HashMap<SyscallName, u64> {
    use SyscallName::*;
    [
        (Read, 0),
        (Write, 1),
        (Open, 2),
        (Close, 3),
        (Exit, 60),
        (ExitGroup, 231),
        (Mmap, 9),
        (Munmap, 11),
        (Brk, 12),
        (Ioctl, 16),
        (Fcntl, 72),
        (Getpid, 39),
        (Kill, 62),
        (Mprotect, 10),
        (ClockGettime, 228),
        (SchedYield, 24),
        (Clone, 56),
        (Futex, 202),
        (SetTidAddress, 218),
    ]
    .into_iter()
    .collect()
}

fn aarch64_syscalls() -> HashMap<SyscallName, u64> {
    use SyscallName::*;
    [
        (Read, 63),
        (Write, 64),
        (Open, 35),    // openat
        (Close, 57),
        (Exit, 93),
        (ExitGroup, 94),
        (Mmap, 222),
        (Munmap, 215),
        (Brk, 214),
        (Ioctl, 29),
        (Fcntl, 25),
        (Getpid, 172),
        (Kill, 129),
        (Mprotect, 226),
        (ClockGettime, 115),
        (SchedYield, 124),
        (Clone, 220),
        (Futex, 98),
        (SetTidAddress, 96),
    ]
    .into_iter()
    .collect()
}

fn riscv64_syscalls() -> HashMap<SyscallName, u64> {
    use SyscallName::*;
    [
        (Read, 63),
        (Write, 64),
        (Open, 35),    // openat
        (Close, 57),
        (Exit, 93),
        (ExitGroup, 94),
        (Mmap, 222),
        (Munmap, 215),
        (Brk, 214),
        (Ioctl, 29),
        (Fcntl, 25),
        (Getpid, 172),
        (Kill, 129),
        (Mprotect, 226),
        (ClockGettime, 115),
        (SchedYield, 124),
        (Clone, 220),
        (Futex, 98),
        (SetTidAddress, 96),
    ]
    .into_iter()
    .collect()
}

fn arm32_syscalls() -> HashMap<SyscallName, u64> {
    use SyscallName::*;
    [
        (Read, 3),
        (Write, 4),
        (Open, 5),
        (Close, 6),
        (Exit, 1),
        (ExitGroup, 248),
        (Mmap, 192),   // mmap2 on ARM
        (Munmap, 91),
        (Brk, 45),
        (Ioctl, 54),
        (Fcntl, 55),
        (Getpid, 20),
        (Kill, 37),
        (Mprotect, 125),
        (ClockGettime, 263),
        (SchedYield, 158),
        (Clone, 120),
        (Futex, 240),
        (SetTidAddress, 256),
    ]
    .into_iter()
    .collect()
}

fn mips64_syscalls() -> HashMap<SyscallName, u64> {
    use SyscallName::*;
    [
        (Read, 5000),
        (Write, 5001),
        (Open, 5002),
        (Close, 5003),
        (Exit, 5058),
        (ExitGroup, 5206),
        (Mmap, 5009),
        (Munmap, 5011),
        (Brk, 5012),
        (Ioctl, 5015),
        (Fcntl, 5070),
        (Getpid, 5038),
        (Kill, 5060),
        (Mprotect, 5010),
        (ClockGettime, 5223),
        (SchedYield, 5023),
        (Clone, 5055),
        (Futex, 5194),
        (SetTidAddress, 5210),
    ]
    .into_iter()
    .collect()
}

fn ppc64_syscalls() -> HashMap<SyscallName, u64> {
    use SyscallName::*;
    [
        (Read, 3),
        (Write, 4),
        (Open, 5),
        (Close, 6),
        (Exit, 1),
        (ExitGroup, 234),
        (Mmap, 90),
        (Munmap, 91),
        (Brk, 45),
        (Ioctl, 54),
        (Fcntl, 55),
        (Getpid, 20),
        (Kill, 37),
        (Mprotect, 125),
        (ClockGettime, 246),
        (SchedYield, 158),
        (Clone, 120),
        (Futex, 221),
        (SetTidAddress, 232),
    ]
    .into_iter()
    .collect()
}

fn loongarch64_syscalls() -> HashMap<SyscallName, u64> {
    use SyscallName::*;
    [
        (Read, 63),
        (Write, 64),
        (Open, 35),    // openat
        (Close, 57),
        (Exit, 93),
        (ExitGroup, 94),
        (Mmap, 222),
        (Munmap, 215),
        (Brk, 214),
        (Ioctl, 29),
        (Fcntl, 25),
        (Getpid, 172),
        (Kill, 129),
        (Mprotect, 226),
        (ClockGettime, 115),
        (SchedYield, 124),
        (Clone, 220),
        (Futex, 98),
        (SetTidAddress, 96),
    ]
    .into_iter()
    .collect()
}

/// x86_32 (i386) Linux syscall numbers.
///
/// i386 uses `int 0x80` with the syscall number in EAX and args in
/// EBX, ECX, EDX, ESI, EDI, EBP (max 6). Note that `mmap` on i386 uses
/// `mmap2` (syscall 192) which takes the offset in pages (4096-byte units),
/// not bytes. For zero-offset anonymous mappings this is equivalent.
fn x86_32_syscalls() -> HashMap<SyscallName, u64> {
    use SyscallName::*;
    [
        (Read, 3),
        (Write, 4),
        (Open, 5),
        (Close, 6),
        (Exit, 1),
        (ExitGroup, 252),
        (Mmap, 192),   // mmap2 on i386 (offset in pages, not bytes)
        (Munmap, 91),
        (Brk, 45),
        (Ioctl, 54),
        (Fcntl, 55),
        (Getpid, 20),
        (Kill, 37),
        (Mprotect, 125),
        (ClockGettime, 265),
        (SchedYield, 158),
        (Clone, 120),
        (Futex, 240),
        (SetTidAddress, 258),
    ]
    .into_iter()
    .collect()
}

/// RISC-V 32-bit Linux syscall numbers.
///
/// RV32 uses `ecall` with the syscall number in a7 and args in
/// a0-a5 (max 6). The generic RISC-V syscall table (newer arch-generic
/// numbers) is shared between RV32 and RV64 — the numbers are identical.
fn riscv32_syscalls() -> HashMap<SyscallName, u64> {
    use SyscallName::*;
    [
        (Read, 63),
        (Write, 64),
        (Open, 35),    // openat
        (Close, 57),
        (Exit, 93),
        (ExitGroup, 94),
        (Mmap, 222),
        (Munmap, 215),
        (Brk, 214),
        (Ioctl, 29),
        (Fcntl, 25),
        (Getpid, 172),
        (Kill, 129),
        (Mprotect, 226),
        (ClockGettime, 115),
        (SchedYield, 124),
        (Clone, 220),
        (Futex, 98),
        (SetTidAddress, 96),
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linux_syscall_bindings() {
        let bindings = linux_syscall_bindings();
        assert_eq!(bindings.convention, CallingConvention::C);
        assert!(bindings.functions.iter().any(|f| f.name == "write"));
        assert!(bindings.functions.iter().any(|f| f.name == "read"));
        assert!(bindings.functions.iter().any(|f| f.name == "exit"));
        assert!(bindings.functions.iter().any(|f| f.name == "mmap"));
        assert!(bindings.functions.iter().any(|f| f.name == "munmap"));
        assert!(bindings.functions.iter().any(|f| f.name == "brk"));
    }

    #[test]
    fn test_c_library_bindings() {
        let bindings = c_library_bindings();
        assert_eq!(bindings.convention, CallingConvention::C);
        assert!(bindings.functions.iter().any(|f| f.name == "memcpy"));
        assert!(bindings.functions.iter().any(|f| f.name == "memset"));
        assert!(bindings.functions.iter().any(|f| f.name == "malloc"));
        assert!(bindings.functions.iter().any(|f| f.name == "free"));
    }

    #[test]
    fn test_extern_registry() {
        let registry = ExternRegistry::with_default_bindings();
        assert!(registry.is_extern("write"));
        assert!(registry.is_extern("malloc"));
        assert!(!registry.is_extern("my_local_fn"));
        assert!(registry.needs_relocation("write"));
        assert_eq!(registry.convention("write"), Some(CallingConvention::C));
    }

    #[test]
    fn test_register_custom_extern_block() {
        let mut registry = ExternRegistry::new();
        let block = ExternBlock {
            convention: CallingConvention::C,
            functions: vec![ExternFn {
                name: "custom_fn".to_string(),
                param_types: vec![ExternType::I64],
                return_type: Some(ExternType::I64),
            }],
        };
        registry.register_block(&block);
        assert!(registry.is_extern("custom_fn"));
        assert!(registry.needs_relocation("custom_fn"));
    }

    // ---- SyscallTable tests ----

    #[test]
    fn test_syscall_table_x86_64() {
        let table = SyscallTable::for_arch(Arch::X86_64);
        assert_eq!(table.get(SyscallName::Read), Some(0));
        assert_eq!(table.get(SyscallName::Write), Some(1));
        assert_eq!(table.get(SyscallName::Exit), Some(60));
        assert_eq!(table.get(SyscallName::Mmap), Some(9));
        assert_eq!(table.get(SyscallName::Brk), Some(12));
    }

    #[test]
    fn test_syscall_table_aarch64() {
        let table = SyscallTable::for_arch(Arch::AArch64);
        assert_eq!(table.get(SyscallName::Write), Some(64));
        assert_eq!(table.get(SyscallName::Exit), Some(93));
        assert_eq!(table.get(SyscallName::Mmap), Some(222));
    }

    #[test]
    fn test_syscall_table_riscv64() {
        let table = SyscallTable::for_arch(Arch::RiscV64);
        assert_eq!(table.get(SyscallName::Write), Some(64));
        assert_eq!(table.get(SyscallName::Exit), Some(93));
    }

    #[test]
    fn test_syscall_table_arm32() {
        let table = SyscallTable::for_arch(Arch::Arm32);
        assert_eq!(table.get(SyscallName::Exit), Some(1));
        assert_eq!(table.get(SyscallName::Write), Some(4));
        assert_eq!(table.get(SyscallName::Mmap), Some(192)); // mmap2
    }

    #[test]
    fn test_syscall_table_mips64() {
        let table = SyscallTable::for_arch(Arch::Mips64);
        assert_eq!(table.get(SyscallName::Read), Some(5000));
        assert_eq!(table.get(SyscallName::Write), Some(5001));
        assert_eq!(table.get(SyscallName::Exit), Some(5058));
    }

    #[test]
    fn test_syscall_table_ppc64() {
        let table = SyscallTable::for_arch(Arch::PPC64);
        assert_eq!(table.get(SyscallName::Exit), Some(1));
        assert_eq!(table.get(SyscallName::Write), Some(4));
        assert_eq!(table.get(SyscallName::Mmap), Some(90));
    }

    #[test]
    fn test_syscall_table_loongarch64() {
        let table = SyscallTable::for_arch(Arch::LoongArch64);
        assert_eq!(table.get(SyscallName::Write), Some(64));
        assert_eq!(table.get(SyscallName::Exit), Some(93));
    }

    #[test]
    fn test_syscall_table_x86_32() {
        let table = SyscallTable::for_arch(Arch::X86_32);
        assert_eq!(table.get(SyscallName::Read), Some(3));
        assert_eq!(table.get(SyscallName::Write), Some(4));
        assert_eq!(table.get(SyscallName::Exit), Some(1));
        assert_eq!(table.get(SyscallName::ExitGroup), Some(252));
        assert_eq!(table.get(SyscallName::Mmap), Some(192)); // mmap2
        assert_eq!(table.get(SyscallName::Munmap), Some(91));
        assert_eq!(table.get(SyscallName::Futex), Some(240));
        assert_eq!(table.get(SyscallName::SetTidAddress), Some(258));
    }

    #[test]
    fn test_syscall_table_riscv32() {
        let table = SyscallTable::for_arch(Arch::RiscV32);
        assert_eq!(table.get(SyscallName::Read), Some(63));
        assert_eq!(table.get(SyscallName::Write), Some(64));
        assert_eq!(table.get(SyscallName::Exit), Some(93));
        assert_eq!(table.get(SyscallName::Mmap), Some(222));
        assert_eq!(table.get(SyscallName::Futex), Some(98));
        // RV32 shares the generic RISC-V syscall table with RV64
        let rv64 = SyscallTable::for_arch(Arch::RiscV64);
        assert_eq!(table.get(SyscallName::Write), rv64.get(SyscallName::Write));
        assert_eq!(table.get(SyscallName::Exit), rv64.get(SyscallName::Exit));
    }

    #[test]
    fn test_syscall_table_wasm32_empty() {
        let table = SyscallTable::for_arch(Arch::Wasm32);
        assert!(table.is_empty());
        assert_eq!(table.get(SyscallName::Exit), None);
    }

    #[test]
    fn test_arch_from_name() {
        assert_eq!(Arch::from_name("x86_64"), Some(Arch::X86_64));
        assert_eq!(Arch::from_name("amd64"), Some(Arch::X86_64));
        assert_eq!(Arch::from_name("aarch64"), Some(Arch::AArch64));
        assert_eq!(Arch::from_name("arm64"), Some(Arch::AArch64));
        assert_eq!(Arch::from_name("riscv64"), Some(Arch::RiscV64));
        assert_eq!(Arch::from_name("arm32"), Some(Arch::Arm32));
        assert_eq!(Arch::from_name("mips64"), Some(Arch::Mips64));
        assert_eq!(Arch::from_name("ppc64"), Some(Arch::PPC64));
        assert_eq!(Arch::from_name("loongarch64"), Some(Arch::LoongArch64));
        assert_eq!(Arch::from_name("x86_32"), Some(Arch::X86_32));
        assert_eq!(Arch::from_name("i386"), Some(Arch::X86_32));
        assert_eq!(Arch::from_name("riscv32"), Some(Arch::RiscV32));
        assert_eq!(Arch::from_name("rv32"), Some(Arch::RiscV32));
        assert_eq!(Arch::from_name("unknown"), None);
    }

    #[test]
    fn test_relocation_kind_for_arch() {
        assert_eq!(RelocationKind::for_arch("aarch64"), RelocationKind::AArch64Call26);
        assert_eq!(RelocationKind::for_arch("x86_64"), RelocationKind::X86_64Plt32);
        assert_eq!(RelocationKind::for_arch("riscv64"), RelocationKind::RiscvCall);
        assert_eq!(RelocationKind::for_arch("riscv32"), RelocationKind::RiscvCall);
        assert_eq!(RelocationKind::for_arch("arm32"), RelocationKind::Arm32Call);
        assert_eq!(RelocationKind::for_arch("mips64"), RelocationKind::Mips26);
        assert_eq!(RelocationKind::for_arch("ppc64"), RelocationKind::Ppc64Rel24);
        assert_eq!(RelocationKind::for_arch("loongarch64"), RelocationKind::LoongArchB26);
        assert_eq!(RelocationKind::for_arch("x86_32"), RelocationKind::X86_32Plt32);
        assert_eq!(RelocationKind::for_arch("i386"), RelocationKind::X86_32Plt32);
        assert_eq!(RelocationKind::for_arch("unknown"), RelocationKind::GenericCall32);
    }
}
