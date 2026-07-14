//! # Target Description System
//!
//! Machine-readable ISA specifications that make adding new ISAs a data-driven
//! process. Each `TargetDesc` contains the complete register file, calling
//! convention details, and instruction category metadata for an ISA.

use crate::backend::{Endianness, OutputFormat, RegClass};

/// A complete machine-readable description of a target ISA.
#[derive(Debug, Clone)]
pub struct TargetDesc {
    /// ISA name in lowercase (e.g., `"aarch64"`, `"riscv64"`).
    pub name: &'static str,
    /// LLVM-style target triple (e.g., `"aarch64-unknown-linux-gnu"`).
    pub triple: &'static str,
    /// ELF `e_machine` value (0 for non-ELF targets like Wasm).
    pub elf_machine: u16,
    /// Default base address for the `.text` section.
    pub base_addr: u64,
    /// Pointer width in bytes (4 for 32-bit, 8 for 64-bit).
    pub pointer_width: usize,
    /// Byte order of the target.
    pub endianness: Endianness,
    /// Binary output format produced by the backend.
    pub output_format: OutputFormat,
    /// Register file description.
    pub registers: Vec<RegDesc>,
    /// Calling convention details.
    pub calling_convention: CallingConventionDesc,
    /// Instruction category metadata.
    pub instruction_categories: Vec<InstCategoryDesc>,
    /// Instruction latency table for the scheduler (Wave 5).
    /// Maps instruction category → (latency_cycles, throughput_per_cycle).
    /// If empty, the scheduler assumes uniform latency 1.
    pub latency_table: LatencyTable,
}

/// Latency table for instruction scheduling (Wave 5).
///
/// Models pipeline hazards for list-scheduling. Each entry maps an
/// instruction category to its latency (cycles until result is available)
/// and throughput (instructions per cycle on that functional unit).
#[derive(Debug, Clone, Default)]
pub struct LatencyTable {
    /// Map from instruction category name to latency info.
    pub entries: Vec<LatencyEntry>,
}

/// A single latency entry for one instruction category.
#[derive(Debug, Clone)]
pub struct LatencyEntry {
    /// Category name (matches `InstCategoryDesc::name`).
    pub category: String,
    /// Latency in cycles (time from issue to result availability).
    pub latency: u8,
    /// Throughput: 1/throughput = cycles between consecutive issues.
    /// 1 = fully pipelined, 2 = half-throughput, etc.
    pub throughput: u8,
    /// Functional unit this instruction uses (for hazard detection).
    pub functional_unit: FunctionalUnit,
}

/// Functional units for scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FunctionalUnit {
    /// Integer ALU.
    Alu,
    /// Load/store unit.
    Memory,
    /// Branch unit.
    Branch,
    /// Floating-point / SIMD.
    FpSimd,
    /// Multiply unit (may have higher latency than ALU).
    Multiply,
    /// Divide unit (typically very high latency, not pipelined).
    Divide,
}

impl LatencyTable {
    /// Creates a new empty latency table.
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Creates a latency table with default modern OoO core values.
    /// These are conservative estimates suitable for list-scheduling.
    pub fn default_ooo() -> Self {
        Self {
            entries: vec![
                LatencyEntry { category: "arithmetic".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "logical".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "shift".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "load".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "store".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "branch".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Branch },
                LatencyEntry { category: "multiply".to_string(), latency: 3, throughput: 1, functional_unit: FunctionalUnit::Multiply },
                LatencyEntry { category: "divide".to_string(), latency: 20, throughput: 0, functional_unit: FunctionalUnit::Divide },
                LatencyEntry { category: "fp_simd".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::FpSimd },
            ],
        }
    }

    /// AArch64 (Cortex-A78 / Neoverse-class) latency table.
    /// Sources: ARM Cortex-A78 Software Optimization Guide, ARM ARM.
    /// Integer ALU 1-cycle, mul 3-cycle, divide 20-35 cycle non-pipelined.
    pub fn aarch64() -> Self {
        Self {
            entries: vec![
                LatencyEntry { category: "arithmetic".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "logical".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "shift".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "load".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "store".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "branch".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Branch },
                LatencyEntry { category: "multiply".to_string(), latency: 3, throughput: 1, functional_unit: FunctionalUnit::Multiply },
                LatencyEntry { category: "divide".to_string(), latency: 20, throughput: 0, functional_unit: FunctionalUnit::Divide },
                LatencyEntry { category: "fp_simd".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::FpSimd },
            ],
        }
    }

    /// x86-64 (Intel Golden Cove / AMD Zen 4-class) latency table.
    /// Sources: Intel SDM Vol 1, AMD PPR for Zen 4. LEA is 1-cycle, MUL 3-cycle,
    /// IDIV 20-40 cycle non-pipelined. Loads ~5-cycle to L1.
    pub fn x86_64() -> Self {
        Self {
            entries: vec![
                LatencyEntry { category: "arithmetic".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "logical".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "shift".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "load".to_string(), latency: 5, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "store".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "branch".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Branch },
                LatencyEntry { category: "multiply".to_string(), latency: 3, throughput: 1, functional_unit: FunctionalUnit::Multiply },
                LatencyEntry { category: "divide".to_string(), latency: 30, throughput: 0, functional_unit: FunctionalUnit::Divide },
                LatencyEntry { category: "fp_simd".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::FpSimd },
            ],
        }
    }

    /// RISC-V 64 (SiFive U74 / generic RV64GC) latency table.
    /// Sources: SiFive U74 Manual. MUL 3-cycle, DIV 20-40 cycle.
    pub fn riscv64() -> Self {
        Self {
            entries: vec![
                LatencyEntry { category: "arithmetic".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "logical".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "shift".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "load".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "store".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "branch".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Branch },
                LatencyEntry { category: "multiply".to_string(), latency: 3, throughput: 1, functional_unit: FunctionalUnit::Multiply },
                LatencyEntry { category: "divide".to_string(), latency: 40, throughput: 0, functional_unit: FunctionalUnit::Divide },
                LatencyEntry { category: "fp_simd".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::FpSimd },
            ],
        }
    }

    /// ARM32 (Cortex-A55 / ARMv7-A class) latency table.
    pub fn arm32() -> Self {
        Self {
            entries: vec![
                LatencyEntry { category: "arithmetic".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "logical".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "shift".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "load".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "store".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "branch".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Branch },
                LatencyEntry { category: "multiply".to_string(), latency: 3, throughput: 1, functional_unit: FunctionalUnit::Multiply },
                LatencyEntry { category: "divide".to_string(), latency: 25, throughput: 0, functional_unit: FunctionalUnit::Divide },
                LatencyEntry { category: "fp_simd".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::FpSimd },
            ],
        }
    }

    /// MIPS64 (MIPS64r6 / I6400-class) latency table.
    pub fn mips64() -> Self {
        Self {
            entries: vec![
                LatencyEntry { category: "arithmetic".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "logical".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "shift".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "load".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "store".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "branch".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Branch },
                LatencyEntry { category: "multiply".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::Multiply },
                LatencyEntry { category: "divide".to_string(), latency: 30, throughput: 0, functional_unit: FunctionalUnit::Divide },
                LatencyEntry { category: "fp_simd".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::FpSimd },
            ],
        }
    }

    /// PowerPC 64 (POWER9-class) latency table.
    /// Sources: POWER9 User Manual. MUL 4-5 cycle, DIV 24-40 cycle.
    pub fn ppc64() -> Self {
        Self {
            entries: vec![
                LatencyEntry { category: "arithmetic".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "logical".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "shift".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "load".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "store".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "branch".to_string(), latency: 2, throughput: 1, functional_unit: FunctionalUnit::Branch },
                LatencyEntry { category: "multiply".to_string(), latency: 5, throughput: 1, functional_unit: FunctionalUnit::Multiply },
                LatencyEntry { category: "divide".to_string(), latency: 40, throughput: 0, functional_unit: FunctionalUnit::Divide },
                LatencyEntry { category: "fp_simd".to_string(), latency: 6, throughput: 1, functional_unit: FunctionalUnit::FpSimd },
            ],
        }
    }

    /// LoongArch 64 (LA464 / 3A5000-class) latency table.
    pub fn loongarch64() -> Self {
        Self {
            entries: vec![
                LatencyEntry { category: "arithmetic".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "logical".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "shift".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "load".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "store".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "branch".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Branch },
                LatencyEntry { category: "multiply".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::Multiply },
                LatencyEntry { category: "divide".to_string(), latency: 35, throughput: 0, functional_unit: FunctionalUnit::Divide },
                LatencyEntry { category: "fp_simd".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::FpSimd },
            ],
        }
    }

    /// Wasm32 latency table. Wasm is a virtual ISA; these values model a
    /// typical JIT-compiled execution on a modern host (V8/SpiderMonkey).
    pub fn wasm32() -> Self {
        Self {
            entries: vec![
                LatencyEntry { category: "arithmetic".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "logical".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "shift".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "load".to_string(), latency: 3, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "store".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "branch".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Branch },
                LatencyEntry { category: "multiply".to_string(), latency: 3, throughput: 1, functional_unit: FunctionalUnit::Multiply },
                LatencyEntry { category: "divide".to_string(), latency: 20, throughput: 0, functional_unit: FunctionalUnit::Divide },
                LatencyEntry { category: "fp_simd".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::FpSimd },
            ],
        }
    }

    // ============================================================
    // Remaining ISAs (11) — covers all 19 BackendKind variants.
    // The big-endian / little-endian variants reuse their parent
    // ISA's table (endianness doesn't change instruction latency).
    // ============================================================

    /// RISC-V 32 (RV32IMAC/RV32GC). Same pipeline as RV64; 32-bit regs.
    pub fn riscv32() -> Self {
        // RV32 has the same M-extension latencies as RV64 on SiFive U-class.
        Self::riscv64()
    }

    /// x86-32 (i386/i686). Same integer pipeline as x86-64; 32-bit regs.
    pub fn x86_32() -> Self {
        Self::x86_64()
    }

    /// PowerPC 64 little-endian (ppc64le, ELFv2). Same pipeline as ppc64 BE.
    pub fn ppc64le() -> Self {
        Self::ppc64()
    }

    /// MIPS64 big-endian. Same pipeline as mips64 LE; only data endianness differs.
    pub fn mips64be() -> Self {
        Self::mips64()
    }

    /// ARM32 big-endian (armeb). Same pipeline as arm32 LE.
    pub fn armeb() -> Self {
        Self::arm32()
    }

    /// AArch64 big-endian (aarch64_be). Instructions are always LE-fetched
    /// per ARM ARM D6.1.3; only data endianness differs. Same latencies.
    pub fn aarch64_be() -> Self {
        Self::aarch64()
    }

    /// SPARC V9 (UltraSPARC T1-class). In-order core, 1-cycle ALU,
    /// 4-cycle load, non-pipelined divide.
    pub fn sparc64() -> Self {
        Self {
            entries: vec![
                LatencyEntry { category: "arithmetic".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "logical".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "shift".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "load".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "store".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "branch".to_string(), latency: 2, throughput: 1, functional_unit: FunctionalUnit::Branch },
                LatencyEntry { category: "multiply".to_string(), latency: 5, throughput: 1, functional_unit: FunctionalUnit::Multiply },
                LatencyEntry { category: "divide".to_string(), latency: 40, throughput: 0, functional_unit: FunctionalUnit::Divide },
                LatencyEntry { category: "fp_simd".to_string(), latency: 5, throughput: 1, functional_unit: FunctionalUnit::FpSimd },
            ],
        }
    }

    /// IBM System Z (z14/z15-class). CISC, 1-cycle ALU, 6-cycle load,
    /// variable-latency divide.
    pub fn s390x() -> Self {
        Self {
            entries: vec![
                LatencyEntry { category: "arithmetic".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "logical".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "shift".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "load".to_string(), latency: 6, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "store".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "branch".to_string(), latency: 2, throughput: 1, functional_unit: FunctionalUnit::Branch },
                LatencyEntry { category: "multiply".to_string(), latency: 6, throughput: 1, functional_unit: FunctionalUnit::Multiply },
                LatencyEntry { category: "divide".to_string(), latency: 40, throughput: 0, functional_unit: FunctionalUnit::Divide },
                LatencyEntry { category: "fp_simd".to_string(), latency: 6, throughput: 1, functional_unit: FunctionalUnit::FpSimd },
            ],
        }
    }

    /// Motorola 68000 (68040-class). CISC, 1-cycle ALU, 3-cycle load,
    /// 40-cycle divide (not pipelined).
    pub fn m68k() -> Self {
        Self {
            entries: vec![
                LatencyEntry { category: "arithmetic".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "logical".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "shift".to_string(), latency: 2, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "load".to_string(), latency: 3, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "store".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "branch".to_string(), latency: 2, throughput: 1, functional_unit: FunctionalUnit::Branch },
                LatencyEntry { category: "multiply".to_string(), latency: 20, throughput: 0, functional_unit: FunctionalUnit::Multiply },
                LatencyEntry { category: "divide".to_string(), latency: 40, throughput: 0, functional_unit: FunctionalUnit::Divide },
                LatencyEntry { category: "fp_simd".to_string(), latency: 6, throughput: 1, functional_unit: FunctionalUnit::FpSimd },
            ],
        }
    }

    /// DEC Alpha 21264. In-order, 1-cycle ALU, 3-cycle load, 12-cycle divide.
    pub fn alpha() -> Self {
        Self {
            entries: vec![
                LatencyEntry { category: "arithmetic".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "logical".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "shift".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "load".to_string(), latency: 3, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "store".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "branch".to_string(), latency: 2, throughput: 1, functional_unit: FunctionalUnit::Branch },
                LatencyEntry { category: "multiply".to_string(), latency: 7, throughput: 1, functional_unit: FunctionalUnit::Multiply },
                LatencyEntry { category: "divide".to_string(), latency: 12, throughput: 0, functional_unit: FunctionalUnit::Divide },
                LatencyEntry { category: "fp_simd".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::FpSimd },
            ],
        }
    }

    /// HP PA-RISC (PA-8900-class). 1-cycle ALU, 3-cycle load, 20-cycle divide.
    pub fn hppa() -> Self {
        Self {
            entries: vec![
                LatencyEntry { category: "arithmetic".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "logical".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "shift".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Alu },
                LatencyEntry { category: "load".to_string(), latency: 3, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "store".to_string(), latency: 1, throughput: 1, functional_unit: FunctionalUnit::Memory },
                LatencyEntry { category: "branch".to_string(), latency: 2, throughput: 1, functional_unit: FunctionalUnit::Branch },
                LatencyEntry { category: "multiply".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::Multiply },
                LatencyEntry { category: "divide".to_string(), latency: 20, throughput: 0, functional_unit: FunctionalUnit::Divide },
                LatencyEntry { category: "fp_simd".to_string(), latency: 4, throughput: 1, functional_unit: FunctionalUnit::FpSimd },
            ],
        }
    }

    /// Looks up the latency for a given instruction category.
    /// Returns (latency, throughput, functional_unit).
    /// Defaults to (1, 1, Alu) if not found.
    pub fn lookup(&self, category: &str) -> (u8, u8, FunctionalUnit) {
        for entry in &self.entries {
            if entry.category == category {
                return (entry.latency, entry.throughput, entry.functional_unit);
            }
        }
        (1, 1, FunctionalUnit::Alu)
    }
}

/// Description of a single register.
#[derive(Debug, Clone)]
pub struct RegDesc {
    /// Register name (e.g., `"X0"`, `"RAX"`, `"x10"`).
    pub name: &'static str,
    /// Register class (GPR, SIMD/FP, Condition, Special).
    pub class: RegClass,
    /// Register index within its class (0-based).
    pub index: usize,
    /// Whether this register is available for the register allocator.
    pub is_allocatable: bool,
    /// Whether this register always reads as zero (e.g., RISC-V `x0`).
    pub is_hardwired_zero: bool,
    /// Whether this register serves as the stack pointer.
    pub is_stack_pointer: bool,
    /// Whether this register serves as the frame pointer.
    pub is_frame_pointer: bool,
    /// Whether this register holds the return address (link register).
    pub is_link_register: bool,
    /// Whether this register holds the TOC (table of contents) pointer.
    pub is_toc_pointer: bool,
    /// Whether this register must be preserved across function calls.
    pub is_callee_saved: bool,
    /// Whether this register is used for passing arguments.
    pub is_arg_reg: bool,
    /// For argument registers, the position in the argument list (0-based).
    pub arg_position: Option<usize>,
    /// Whether this register is used for returning values.
    pub is_return_reg: bool,
}

impl RegDesc {
    /// Create a new GPR descriptor (allocatable by default).
    fn gpr(name: &'static str, index: usize) -> Self {
        Self {
            name,
            class: RegClass::Gpr,
            index,
            is_allocatable: true,
            is_hardwired_zero: false,
            is_stack_pointer: false,
            is_frame_pointer: false,
            is_link_register: false,
            is_toc_pointer: false,
            is_callee_saved: false,
            is_arg_reg: false,
            arg_position: None,
            is_return_reg: false,
        }
    }

    /// Create a new SIMD/FP register descriptor (allocatable by default).
    fn fpr(name: &'static str, index: usize) -> Self {
        Self {
            name,
            class: RegClass::SimdFp,
            index,
            is_allocatable: true,
            is_hardwired_zero: false,
            is_stack_pointer: false,
            is_frame_pointer: false,
            is_link_register: false,
            is_toc_pointer: false,
            is_callee_saved: false,
            is_arg_reg: false,
            arg_position: None,
            is_return_reg: false,
        }
    }

    /// Create a new special-purpose register descriptor (not allocatable).
    fn special_reg(name: &'static str, index: usize) -> Self {
        Self {
            name,
            class: RegClass::Special,
            index,
            is_allocatable: false,
            is_hardwired_zero: false,
            is_stack_pointer: false,
            is_frame_pointer: false,
            is_link_register: false,
            is_toc_pointer: false,
            is_callee_saved: false,
            is_arg_reg: false,
            arg_position: None,
            is_return_reg: false,
        }
    }

    /// Create a new condition register descriptor (not allocatable).
    fn cond_reg(name: &'static str, index: usize) -> Self {
        Self {
            name,
            class: RegClass::Condition,
            index,
            is_allocatable: false,
            is_hardwired_zero: false,
            is_stack_pointer: false,
            is_frame_pointer: false,
            is_link_register: false,
            is_toc_pointer: false,
            is_callee_saved: false,
            is_arg_reg: false,
            arg_position: None,
            is_return_reg: false,
        }
    }

    // Builder-style modifiers

    fn hardwired_zero(mut self) -> Self {
        self.is_hardwired_zero = true;
        self.is_allocatable = false;
        self
    }

    fn stack_pointer(mut self) -> Self {
        self.is_stack_pointer = true;
        self.is_allocatable = false;
        self
    }

    fn frame_pointer(mut self) -> Self {
        self.is_frame_pointer = true;
        self
    }

    fn link_register(mut self) -> Self {
        self.is_link_register = true;
        self.is_allocatable = false;
        self
    }

    fn toc_pointer(mut self) -> Self {
        self.is_toc_pointer = true;
        self.is_allocatable = false;
        self
    }

    fn callee_saved(mut self) -> Self {
        self.is_callee_saved = true;
        self
    }

    fn arg(mut self, pos: usize) -> Self {
        self.is_arg_reg = true;
        self.arg_position = Some(pos);
        self
    }

    fn return_reg(mut self) -> Self {
        self.is_return_reg = true;
        self
    }

    fn not_allocatable(mut self) -> Self {
        self.is_allocatable = false;
        self
    }
}

// ---------------------------------------------------------------------------
// RegisterClass (Wave 24)
// ---------------------------------------------------------------------------

/// A summary view of one register class on a target, extracted from a
/// [`TargetDesc`].
///
/// (Wave 24) The target-agnostic register allocator
/// ([`crate::regalloc::TargetAgnosticRegAlloc`]) needs a per-class view of
/// the register file that exposes:
/// - which registers are allocatable,
/// - how many are caller-saved vs callee-saved,
/// - the register width (for spill-slot sizing), and
/// - a per-class move cost (for the coalescing heuristic).
///
/// `RegisterClass` is that view.  It is derived from `TargetDesc::registers`
/// by [`TargetDesc::register_classes`].
///
/// Note: this is distinct from the [`RegClass`] enum, which merely *tags*
/// a register with its class (GPR / SIMD-FP / Condition / Special).
/// `RegisterClass` aggregates information about *all* registers sharing
/// that tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterClass {
    /// Human-readable class name (e.g. `"GPR"`, `"FPR"`, `"Vec"`, `"Condition"`).
    pub name: &'static str,
    /// The register-class enum value this summary describes.
    pub class: RegClass,
    /// Width of each register in this class, in bytes.
    /// GPR width is the target's `pointer_width`; SIMD/FP width is 16
    /// (128-bit) for all currently-modeled ISAs; Condition/Special default
    /// to 4 (32-bit condition register / TOC slot).
    pub width_bytes: usize,
    /// Total number of registers in this class (allocatable + reserved).
    pub total_count: usize,
    /// Number of allocatable registers in this class
    /// (`RegDesc::is_allocatable == true`).
    pub allocatable_count: usize,
    /// Number of allocatable callee-saved registers in this class.
    pub callee_saved_count: usize,
    /// Number of allocatable caller-saved registers in this class
    /// (allocatable registers that are not callee-saved).
    pub caller_saved_count: usize,
    /// Cost of a register-to-register move within this class, in arbitrary
    /// "latency units" (1 = cheapest).  Used by the coalescing heuristic
    /// to decide whether eliminating a copy is worthwhile: cheaper moves
    /// justify more aggressive coalescing.
    pub move_cost: u32,
}

impl RegisterClass {
    /// Returns `true` if this class has at least one allocatable register.
    pub fn has_allocatable(&self) -> bool {
        self.allocatable_count > 0
    }
}

impl TargetDesc {
    /// Returns a [`RegisterClass`] summary for each register class present
    /// in this target's register file (one entry per distinct `RegClass`
    /// value actually used by some `RegDesc`).
    ///
    /// (Wave 24) The summaries are computed by scanning
    /// `self.registers` and grouping by `RegDesc::class`.  The `move_cost`
    /// for each class is taken from [`TargetDesc::move_cost`].
    pub fn register_classes(&self) -> Vec<RegisterClass> {
        // Collect the set of classes actually present, in a deterministic
        // order (the order RegClass variants are declared in backend.rs).
        let mut classes_present: Vec<RegClass> = Vec::new();
        for reg in &self.registers {
            if !classes_present.contains(&reg.class) {
                classes_present.push(reg.class);
            }
        }

        classes_present
            .into_iter()
            .map(|class| {
                let in_class: Vec<&RegDesc> =
                    self.registers.iter().filter(|r| r.class == class).collect();
                let total_count = in_class.len();
                let allocatable_count =
                    in_class.iter().filter(|r| r.is_allocatable).count();
                let callee_saved_count = in_class
                    .iter()
                    .filter(|r| r.is_allocatable && r.is_callee_saved)
                    .count();
                let caller_saved_count = in_class
                    .iter()
                    .filter(|r| r.is_allocatable && !r.is_callee_saved)
                    .count();
                let width_bytes = Self::class_width(self.pointer_width, class);
                RegisterClass {
                    name: Self::class_name(class),
                    class,
                    width_bytes,
                    total_count,
                    allocatable_count,
                    callee_saved_count,
                    caller_saved_count,
                    move_cost: self.move_cost(class),
                }
            })
            .collect()
    }

    /// Returns the allocatable register descriptors for a given class.
    ///
    /// (Wave 24) Convenience accessor used by the target-agnostic
    /// allocator's pool-construction code.
    pub fn allocatable_regs(&self, class: RegClass) -> Vec<&RegDesc> {
        self.registers
            .iter()
            .filter(|r| r.class == class && r.is_allocatable)
            .collect()
    }

    /// Returns the move cost (in arbitrary "latency units") for a
    /// register-to-register move within the given class on this target.
    ///
    /// (Wave 24) Defaults are conservative and ISA-agnostic:
    /// - `Gpr` → 1 (single-cycle `mov` / `orr` on all modeled ISAs)
    /// - `SimdFp` → 1 (single-cycle `vmov` / `movaps` / `fmv`)
    /// - `Condition` → 2 (CR-field ops on PPC are 2-cycle)
    /// - `Special` → 4 (TOC ops are heavier)
    ///
    /// Individual targets can override this in the future by storing a
    /// per-class cost table in `TargetDesc`; for now, the defaults are
    /// computed from the class.
    pub fn move_cost(&self, class: RegClass) -> u32 {
        match class {
            RegClass::Gpr => 1,
            RegClass::SimdFp => 1,
            RegClass::Condition => 2,
            RegClass::Special => 4,
        }
    }

    /// Human-readable name for a `RegClass` variant.
    fn class_name(class: RegClass) -> &'static str {
        match class {
            RegClass::Gpr => "GPR",
            RegClass::SimdFp => "FPR",
            RegClass::Condition => "Condition",
            RegClass::Special => "Special",
        }
    }

    /// Default register width (in bytes) for a class on a target with the
    /// given pointer width.
    fn class_width(pointer_width: usize, class: RegClass) -> usize {
        match class {
            // GPRs are pointer-width on every modeled ISA.
            RegClass::Gpr => pointer_width,
            // SIMD/FP registers are 128-bit (16 bytes) on every modeled
            // ISA with SIMD (AArch64 V regs, x86_64 XMM, RISC-V V, etc.).
            RegClass::SimdFp => 16,
            // Condition registers (PPC CR fields) are 32-bit.
            RegClass::Condition => 4,
            // Special registers (TOC, etc.) are pointer-width.
            RegClass::Special => pointer_width,
        }
    }
}

/// Description of a calling convention.
#[derive(Debug, Clone)]
pub struct CallingConventionDesc {
    /// Calling convention name (e.g., `"aapcs64"`, `"lp64d"`, `"systemv"`).
    pub name: &'static str,
    /// GPR indices used for integer arguments, in order.
    pub int_arg_regs: Vec<usize>,
    /// FPR indices used for floating-point arguments, in order.
    pub fp_arg_regs: Vec<usize>,
    /// GPR indices used for integer return values, in order.
    pub int_return_regs: Vec<usize>,
    /// FPR indices used for floating-point return values, in order.
    pub fp_return_regs: Vec<usize>,
    /// GPR indices that are callee-saved.
    pub callee_saved_gprs: Vec<usize>,
    /// FPR indices that are callee-saved.
    pub callee_saved_fps: Vec<usize>,
    /// Required stack alignment in bytes.
    pub stack_alignment: usize,
    /// Whether the ISA uses a link register (vs pushing return address on stack).
    pub has_link_register: bool,
    /// Whether branches have delay slots (MIPS only).
    pub has_branch_delay_slots: bool,
    /// Whether the ISA uses a TOC (table of contents) pointer (PPC64).
    pub has_toc_pointer: bool,
}

/// Description of an instruction category.
#[derive(Debug, Clone)]
pub struct InstCategoryDesc {
    /// Category name (e.g., `"arithmetic"`, `"branch"`, `"load_store"`).
    pub name: &'static str,
    /// Instruction mnemonics belonging to this category.
    pub insts: Vec<&'static str>,
}

/// Registry of all target descriptions.
pub struct TargetDescRegistry {
    descs: std::collections::HashMap<&'static str, TargetDesc>,
}

impl TargetDescRegistry {
    /// Creates a new registry pre-populated with all supported target descriptions.
    pub fn new() -> Self {
        let mut descs = std::collections::HashMap::new();
        descs.insert("aarch64", aarch64_target_desc());
        descs.insert("riscv64", riscv64_target_desc());
        descs.insert("wasm32", wasm32_target_desc());
        descs.insert("loongarch64", loongarch64_target_desc());
        descs.insert("x86_64", x86_64_target_desc());
        descs.insert("arm32", arm32_target_desc());
        descs.insert("mips64", mips64_target_desc());
        descs.insert("ppc64", ppc64_target_desc());
        Self { descs }
    }

    /// Looks up a target description by ISA name (e.g., `"aarch64"`).
    pub fn get(&self, name: &str) -> Option<&TargetDesc> {
        self.descs.get(name)
    }

    /// Returns the list of all registered ISA names.
    pub fn isa_names(&self) -> Vec<&'static str> {
        self.descs.keys().copied().collect()
    }
}

impl Default for TargetDescRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// AArch64 (AAPCS64)
// ===========================================================================

fn aarch64_target_desc() -> TargetDesc {
    let registers = vec![
        // X0-X7: argument/return registers (caller-saved)
        RegDesc::gpr("X0", 0).arg(0).return_reg(),
        RegDesc::gpr("X1", 1).arg(1).return_reg(),
        RegDesc::gpr("X2", 2).arg(2),
        RegDesc::gpr("X3", 3).arg(3),
        RegDesc::gpr("X4", 4).arg(4),
        RegDesc::gpr("X5", 5).arg(5),
        RegDesc::gpr("X6", 6).arg(6),
        RegDesc::gpr("X7", 7).arg(7),
        // X8: indirect result location register (caller-saved)
        RegDesc::gpr("X8", 8),
        // X9-X15: caller-saved temporaries
        RegDesc::gpr("X9", 9),
        RegDesc::gpr("X10", 10),
        RegDesc::gpr("X11", 11),
        RegDesc::gpr("X12", 12),
        RegDesc::gpr("X13", 13),
        RegDesc::gpr("X14", 14),
        RegDesc::gpr("X15", 15),
        // X16-X17: intra-procedure call scratch (IP0/IP1), not allocatable
        RegDesc::gpr("X16", 16).not_allocatable(),
        RegDesc::gpr("X17", 17).not_allocatable(),
        // X18: platform register, not allocatable
        RegDesc::gpr("X18", 18).not_allocatable(),
        // X19-X28: callee-saved
        RegDesc::gpr("X19", 19).callee_saved(),
        RegDesc::gpr("X20", 20).callee_saved(),
        RegDesc::gpr("X21", 21).callee_saved(),
        RegDesc::gpr("X22", 22).callee_saved(),
        RegDesc::gpr("X23", 23).callee_saved(),
        RegDesc::gpr("X24", 24).callee_saved(),
        RegDesc::gpr("X25", 25).callee_saved(),
        RegDesc::gpr("X26", 26).callee_saved(),
        RegDesc::gpr("X27", 27).callee_saved(),
        RegDesc::gpr("X28", 28).callee_saved(),
        // X29: frame pointer (callee-saved)
        RegDesc::gpr("X29", 29).frame_pointer().callee_saved(),
        // X30: link register, not allocatable
        RegDesc::gpr("X30", 30).link_register(),
        // SP: stack pointer, not allocatable
        RegDesc::gpr("SP", 31).stack_pointer(),
        // XZR: zero register, not allocatable
        RegDesc::gpr("XZR", 32).hardwired_zero(),
        // V0-V7: FP argument/return registers (caller-saved)
        RegDesc::fpr("V0", 0).arg(0).return_reg(),
        RegDesc::fpr("V1", 1).arg(1).return_reg(),
        RegDesc::fpr("V2", 2).arg(2).return_reg(),
        RegDesc::fpr("V3", 3).arg(3).return_reg(),
        RegDesc::fpr("V4", 4).arg(4),
        RegDesc::fpr("V5", 5).arg(5),
        RegDesc::fpr("V6", 6).arg(6),
        RegDesc::fpr("V7", 7).arg(7),
        // V8-V15: callee-saved FP registers
        RegDesc::fpr("V8", 8).callee_saved(),
        RegDesc::fpr("V9", 9).callee_saved(),
        RegDesc::fpr("V10", 10).callee_saved(),
        RegDesc::fpr("V11", 11).callee_saved(),
        RegDesc::fpr("V12", 12).callee_saved(),
        RegDesc::fpr("V13", 13).callee_saved(),
        RegDesc::fpr("V14", 14).callee_saved(),
        RegDesc::fpr("V15", 15).callee_saved(),
        // V16-V31: caller-saved FP temporaries
        RegDesc::fpr("V16", 16),
        RegDesc::fpr("V17", 17),
        RegDesc::fpr("V18", 18),
        RegDesc::fpr("V19", 19),
        RegDesc::fpr("V20", 20),
        RegDesc::fpr("V21", 21),
        RegDesc::fpr("V22", 22),
        RegDesc::fpr("V23", 23),
        RegDesc::fpr("V24", 24),
        RegDesc::fpr("V25", 25),
        RegDesc::fpr("V26", 26),
        RegDesc::fpr("V27", 27),
        RegDesc::fpr("V28", 28),
        RegDesc::fpr("V29", 29),
        RegDesc::fpr("V30", 30),
        RegDesc::fpr("V31", 31),
    ];

    let calling_convention = CallingConventionDesc {
        name: "aapcs64",
        int_arg_regs: vec![0, 1, 2, 3, 4, 5, 6, 7],
        fp_arg_regs: vec![0, 1, 2, 3, 4, 5, 6, 7],
        int_return_regs: vec![0, 1],
        fp_return_regs: vec![0, 1, 2, 3],
        callee_saved_gprs: vec![19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29],
        callee_saved_fps: vec![8, 9, 10, 11, 12, 13, 14, 15],
        stack_alignment: 16,
        has_link_register: true,
        has_branch_delay_slots: false,
        has_toc_pointer: false,
    };

    let instruction_categories = vec![
        InstCategoryDesc {
            name: "arithmetic",
            insts: vec![
                "ADD", "SUB", "MUL", "SDIV", "UDIV", "AND", "ORR", "EOR", "LSL", "LSR",
            ],
        },
        InstCategoryDesc {
            name: "branch",
            insts: vec![
                "B", "BL", "BR", "B.cond", "CBZ", "CBNZ", "TBZ", "TBNZ", "RET",
            ],
        },
        InstCategoryDesc {
            name: "load_store",
            insts: vec!["LDR", "STR", "LDP", "STP", "LDUR", "STUR", "LDRB", "STRB"],
        },
        InstCategoryDesc {
            name: "fp_arithmetic",
            insts: vec!["FADD", "FSUB", "FMUL", "FDIV", "FMOV", "FCMP", "FCVT"],
        },
        InstCategoryDesc {
            name: "system",
            insts: vec!["SVC", "MRS", "MSR", "DMB", "DSB", "ISB", "NOP"],
        },
    ];

    TargetDesc {
        name: "aarch64",
        triple: "aarch64-unknown-linux-gnu",
        elf_machine: 183,
        base_addr: 0x400000,
        pointer_width: 8,
        endianness: Endianness::Little,
        output_format: OutputFormat::Elf64,
        registers,
        calling_convention,
        instruction_categories,
        latency_table: LatencyTable::aarch64(),
    }
}

// ===========================================================================
// RISC-V64 (LP64D)
// ===========================================================================

fn riscv64_target_desc() -> TargetDesc {
    let registers = vec![
        // x0: hardwired zero
        RegDesc::gpr("x0", 0).hardwired_zero(),
        // x1: return address (link register)
        RegDesc::gpr("x1", 1).link_register(),
        // x2: stack pointer
        RegDesc::gpr("x2", 2).stack_pointer(),
        // x3: global pointer
        RegDesc::gpr("x3", 3).not_allocatable(),
        // x4: thread pointer
        RegDesc::gpr("x4", 4).not_allocatable(),
        // x5-x7: temporaries t0-t2 (caller-saved)
        RegDesc::gpr("x5", 5),
        RegDesc::gpr("x6", 6),
        RegDesc::gpr("x7", 7),
        // x8: s0/fp (callee-saved, frame pointer)
        RegDesc::gpr("x8", 8).frame_pointer().callee_saved(),
        // x9: s1 (callee-saved)
        RegDesc::gpr("x9", 9).callee_saved(),
        // x10-x17: arguments a0-a7 (caller-saved)
        RegDesc::gpr("x10", 10).arg(0).return_reg(),
        RegDesc::gpr("x11", 11).arg(1).return_reg(),
        RegDesc::gpr("x12", 12).arg(2),
        RegDesc::gpr("x13", 13).arg(3),
        RegDesc::gpr("x14", 14).arg(4),
        RegDesc::gpr("x15", 15).arg(5),
        RegDesc::gpr("x16", 16).arg(6),
        RegDesc::gpr("x17", 17).arg(7),
        // x18-x27: saved s2-s11 (callee-saved)
        RegDesc::gpr("x18", 18).callee_saved(),
        RegDesc::gpr("x19", 19).callee_saved(),
        RegDesc::gpr("x20", 20).callee_saved(),
        RegDesc::gpr("x21", 21).callee_saved(),
        RegDesc::gpr("x22", 22).callee_saved(),
        RegDesc::gpr("x23", 23).callee_saved(),
        RegDesc::gpr("x24", 24).callee_saved(),
        RegDesc::gpr("x25", 25).callee_saved(),
        RegDesc::gpr("x26", 26).callee_saved(),
        RegDesc::gpr("x27", 27).callee_saved(),
        // x28-x31: temporaries t3-t6 (caller-saved)
        RegDesc::gpr("x28", 28),
        RegDesc::gpr("x29", 29),
        RegDesc::gpr("x30", 30),
        RegDesc::gpr("x31", 31),
        // f0-f7: temporaries ft0-ft7 (caller-saved)
        RegDesc::fpr("f0", 0),
        RegDesc::fpr("f1", 1),
        RegDesc::fpr("f2", 2),
        RegDesc::fpr("f3", 3),
        RegDesc::fpr("f4", 4),
        RegDesc::fpr("f5", 5),
        RegDesc::fpr("f6", 6),
        RegDesc::fpr("f7", 7),
        // f8-f9: saved fs0-fs1 (callee-saved)
        RegDesc::fpr("f8", 8).callee_saved(),
        RegDesc::fpr("f9", 9).callee_saved(),
        // f10-f17: arguments fa0-fa7 (caller-saved)
        RegDesc::fpr("f10", 10).arg(0).return_reg(),
        RegDesc::fpr("f11", 11).arg(1).return_reg(),
        RegDesc::fpr("f12", 12).arg(2),
        RegDesc::fpr("f13", 13).arg(3),
        RegDesc::fpr("f14", 14).arg(4),
        RegDesc::fpr("f15", 15).arg(5),
        RegDesc::fpr("f16", 16).arg(6),
        RegDesc::fpr("f17", 17).arg(7),
        // f18-f27: saved fs2-fs11 (callee-saved)
        RegDesc::fpr("f18", 18).callee_saved(),
        RegDesc::fpr("f19", 19).callee_saved(),
        RegDesc::fpr("f20", 20).callee_saved(),
        RegDesc::fpr("f21", 21).callee_saved(),
        RegDesc::fpr("f22", 22).callee_saved(),
        RegDesc::fpr("f23", 23).callee_saved(),
        RegDesc::fpr("f24", 24).callee_saved(),
        RegDesc::fpr("f25", 25).callee_saved(),
        RegDesc::fpr("f26", 26).callee_saved(),
        RegDesc::fpr("f27", 27).callee_saved(),
        // f28-f31: temporaries ft8-ft11 (caller-saved)
        RegDesc::fpr("f28", 28),
        RegDesc::fpr("f29", 29),
        RegDesc::fpr("f30", 30),
        RegDesc::fpr("f31", 31),
    ];

    let calling_convention = CallingConventionDesc {
        name: "lp64d",
        int_arg_regs: vec![10, 11, 12, 13, 14, 15, 16, 17],
        fp_arg_regs: vec![10, 11, 12, 13, 14, 15, 16, 17],
        int_return_regs: vec![10, 11],
        fp_return_regs: vec![10, 11],
        callee_saved_gprs: vec![8, 9, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27],
        callee_saved_fps: vec![8, 9, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27],
        stack_alignment: 16,
        has_link_register: true,
        has_branch_delay_slots: false,
        has_toc_pointer: false,
    };

    let instruction_categories = vec![
        InstCategoryDesc {
            name: "arithmetic",
            insts: vec![
                "ADD", "SUB", "MUL", "DIV", "AND", "OR", "XOR", "SLL", "SRL", "SRA", "ADDI", "SLT",
            ],
        },
        InstCategoryDesc {
            name: "branch",
            insts: vec!["BEQ", "BNE", "BLT", "BGE", "BLTU", "BGEU", "JAL", "JALR"],
        },
        InstCategoryDesc {
            name: "load_store",
            insts: vec!["LW", "LD", "SW", "SD", "LB", "SB", "LH", "SH", "LBU", "LHU"],
        },
        InstCategoryDesc {
            name: "fp_arithmetic",
            insts: vec!["FADD.D", "FSUB.D", "FMUL.D", "FDIV.D", "FMV.D", "FCVT.D.W"],
        },
        InstCategoryDesc {
            name: "system",
            insts: vec![
                "ECALL", "EBREAK", "FENCE", "FENCE.I", "CSRRC", "CSRRS", "CSRRW",
            ],
        },
    ];

    TargetDesc {
        name: "riscv64",
        triple: "riscv64-unknown-linux-gnu",
        elf_machine: 243,
        base_addr: 0x10000,
        pointer_width: 8,
        endianness: Endianness::Little,
        output_format: OutputFormat::Elf64,
        registers,
        calling_convention,
        instruction_categories,
        latency_table: LatencyTable::riscv64(),
    }
}

// ===========================================================================
// Wasm32 (stack machine)
// ===========================================================================

fn wasm32_target_desc() -> TargetDesc {
    let registers = vec![
        // Single pseudo-register representing the operand stack
        RegDesc::special_reg("stack", 0),
    ];

    let calling_convention = CallingConventionDesc {
        name: "wasm-stack",
        int_arg_regs: vec![],
        fp_arg_regs: vec![],
        int_return_regs: vec![],
        fp_return_regs: vec![],
        callee_saved_gprs: vec![],
        callee_saved_fps: vec![],
        stack_alignment: 8,
        has_link_register: false,
        has_branch_delay_slots: false,
        has_toc_pointer: false,
    };

    let instruction_categories = vec![
        InstCategoryDesc {
            name: "arithmetic",
            insts: vec![
                "i32.add", "i32.sub", "i32.mul", "i64.add", "i64.sub", "i64.mul",
            ],
        },
        InstCategoryDesc {
            name: "branch",
            insts: vec![
                "br", "br_if", "br_table", "return", "if", "else", "end", "loop", "block",
            ],
        },
        InstCategoryDesc {
            name: "load_store",
            insts: vec![
                "i32.load",
                "i32.store",
                "i64.load",
                "i64.store",
                "i32.load8_s",
                "i32.store8",
            ],
        },
        InstCategoryDesc {
            name: "fp_arithmetic",
            insts: vec![
                "f32.add", "f32.sub", "f64.add", "f64.sub", "f32.mul", "f64.mul",
            ],
        },
        InstCategoryDesc {
            name: "system",
            insts: vec![
                "call",
                "call_indirect",
                "drop",
                "nop",
                "unreachable",
                "select",
            ],
        },
    ];

    TargetDesc {
        name: "wasm32",
        triple: "wasm32-unknown-unknown",
        elf_machine: 0,
        base_addr: 0,
        pointer_width: 4,
        endianness: Endianness::Little,
        output_format: OutputFormat::WasmBinary,
        registers,
        calling_convention,
        instruction_categories,
        latency_table: LatencyTable::wasm32(),
    }
}

// ===========================================================================
// LoongArch64 (LP64)
// ===========================================================================

fn loongarch64_target_desc() -> TargetDesc {
    let registers = vec![
        // r0: hardwired zero
        RegDesc::gpr("r0", 0).hardwired_zero(),
        // r1: return address (link register)
        RegDesc::gpr("r1", 1).link_register(),
        // r2: thread pointer
        RegDesc::gpr("r2", 2).not_allocatable(),
        // r3: stack pointer
        RegDesc::gpr("r3", 3).stack_pointer(),
        // r4-r11: arguments a0-a7 (caller-saved)
        RegDesc::gpr("r4", 4).arg(0).return_reg(),
        RegDesc::gpr("r5", 5).arg(1).return_reg(),
        RegDesc::gpr("r6", 6).arg(2),
        RegDesc::gpr("r7", 7).arg(3),
        RegDesc::gpr("r8", 8).arg(4),
        RegDesc::gpr("r9", 9).arg(5),
        RegDesc::gpr("r10", 10).arg(6),
        RegDesc::gpr("r11", 11).arg(7),
        // r12-r20: temporaries t0-t8 (caller-saved)
        RegDesc::gpr("r12", 12),
        RegDesc::gpr("r13", 13),
        RegDesc::gpr("r14", 14),
        RegDesc::gpr("r15", 15),
        RegDesc::gpr("r16", 16),
        RegDesc::gpr("r17", 17),
        RegDesc::gpr("r18", 18),
        RegDesc::gpr("r19", 19),
        RegDesc::gpr("r20", 20),
        // r21: temp / PIC register (caller-saved)
        RegDesc::gpr("r21", 21),
        // r22: frame pointer (callee-saved)
        RegDesc::gpr("r22", 22).frame_pointer().callee_saved(),
        // r23-r31: saved s0-s8 (callee-saved)
        RegDesc::gpr("r23", 23).callee_saved(),
        RegDesc::gpr("r24", 24).callee_saved(),
        RegDesc::gpr("r25", 25).callee_saved(),
        RegDesc::gpr("r26", 26).callee_saved(),
        RegDesc::gpr("r27", 27).callee_saved(),
        RegDesc::gpr("r28", 28).callee_saved(),
        RegDesc::gpr("r29", 29).callee_saved(),
        RegDesc::gpr("r30", 30).callee_saved(),
        RegDesc::gpr("r31", 31).callee_saved(),
        // f0-f7: arguments fa0-fa7 (caller-saved)
        RegDesc::fpr("f0", 0).arg(0).return_reg(),
        RegDesc::fpr("f1", 1).arg(1).return_reg(),
        RegDesc::fpr("f2", 2).arg(2),
        RegDesc::fpr("f3", 3).arg(3),
        RegDesc::fpr("f4", 4).arg(4),
        RegDesc::fpr("f5", 5).arg(5),
        RegDesc::fpr("f6", 6).arg(6),
        RegDesc::fpr("f7", 7).arg(7),
        // f8-f23: temporaries ft0-ft15 (caller-saved)
        RegDesc::fpr("f8", 8),
        RegDesc::fpr("f9", 9),
        RegDesc::fpr("f10", 10),
        RegDesc::fpr("f11", 11),
        RegDesc::fpr("f12", 12),
        RegDesc::fpr("f13", 13),
        RegDesc::fpr("f14", 14),
        RegDesc::fpr("f15", 15),
        RegDesc::fpr("f16", 16),
        RegDesc::fpr("f17", 17),
        RegDesc::fpr("f18", 18),
        RegDesc::fpr("f19", 19),
        RegDesc::fpr("f20", 20),
        RegDesc::fpr("f21", 21),
        RegDesc::fpr("f22", 22),
        RegDesc::fpr("f23", 23),
        // f24-f31: saved fs0-fs7 (callee-saved)
        RegDesc::fpr("f24", 24).callee_saved(),
        RegDesc::fpr("f25", 25).callee_saved(),
        RegDesc::fpr("f26", 26).callee_saved(),
        RegDesc::fpr("f27", 27).callee_saved(),
        RegDesc::fpr("f28", 28).callee_saved(),
        RegDesc::fpr("f29", 29).callee_saved(),
        RegDesc::fpr("f30", 30).callee_saved(),
        RegDesc::fpr("f31", 31).callee_saved(),
    ];

    let calling_convention = CallingConventionDesc {
        name: "lp64",
        int_arg_regs: vec![4, 5, 6, 7, 8, 9, 10, 11],
        fp_arg_regs: vec![0, 1, 2, 3, 4, 5, 6, 7],
        int_return_regs: vec![4, 5],
        fp_return_regs: vec![0, 1],
        callee_saved_gprs: vec![22, 23, 24, 25, 26, 27, 28, 29, 30, 31],
        callee_saved_fps: vec![24, 25, 26, 27, 28, 29, 30, 31],
        stack_alignment: 16,
        has_link_register: true,
        has_branch_delay_slots: false,
        has_toc_pointer: false,
    };

    let instruction_categories = vec![
        InstCategoryDesc {
            name: "arithmetic",
            insts: vec![
                "ADD.W", "SUB.W", "MUL.W", "DIV.W", "AND", "OR", "XOR", "SLL.W", "SRL.W", "SRA.W",
            ],
        },
        InstCategoryDesc {
            name: "branch",
            insts: vec![
                "BEQ", "BNE", "BLT", "BGE", "BLTU", "BGEU", "B", "BL", "JIRL",
            ],
        },
        InstCategoryDesc {
            name: "load_store",
            insts: vec![
                "LD.W", "ST.W", "LD.D", "ST.D", "LD.BU", "ST.B", "LD.HU", "ST.H",
            ],
        },
        InstCategoryDesc {
            name: "fp_arithmetic",
            insts: vec![
                "FADD.D", "FSUB.D", "FMUL.D", "FDIV.D", "FMOV.D", "FCMP.D", "FCVT",
            ],
        },
        InstCategoryDesc {
            name: "system",
            insts: vec!["SYSCALL", "DBAR", "IBAR", "CSRRD", "CSRWR", "CSRXCHG"],
        },
    ];

    TargetDesc {
        name: "loongarch64",
        triple: "loongarch64-unknown-linux-gnu",
        elf_machine: 258,
        base_addr: 0x120000000,
        pointer_width: 8,
        endianness: Endianness::Little,
        output_format: OutputFormat::Elf64,
        registers,
        calling_convention,
        instruction_categories,
        latency_table: LatencyTable::loongarch64(),
    }
}

// ===========================================================================
// x86_64 (SystemV)
// ===========================================================================

fn x86_64_target_desc() -> TargetDesc {
    let registers = vec![
        // RAX: return value (caller-saved)
        RegDesc::gpr("RAX", 0).return_reg(),
        // RCX: arg4 (caller-saved)
        RegDesc::gpr("RCX", 1).arg(3),
        // RDX: arg3 (caller-saved)
        RegDesc::gpr("RDX", 2).arg(2),
        // RBX: callee-saved
        RegDesc::gpr("RBX", 3).callee_saved(),
        // RSP: stack pointer
        RegDesc::gpr("RSP", 4).stack_pointer(),
        // RBP: frame pointer (callee-saved)
        RegDesc::gpr("RBP", 5).frame_pointer().callee_saved(),
        // RSI: arg2 (caller-saved)
        RegDesc::gpr("RSI", 6).arg(1),
        // RDI: arg1 (caller-saved)
        RegDesc::gpr("RDI", 7).arg(0),
        // R8: arg5 (caller-saved)
        RegDesc::gpr("R8", 8).arg(4),
        // R9: arg6 (caller-saved)
        RegDesc::gpr("R9", 9).arg(5),
        // R10-R11: caller-saved temporaries
        RegDesc::gpr("R10", 10),
        RegDesc::gpr("R11", 11),
        // R12-R15: callee-saved
        RegDesc::gpr("R12", 12).callee_saved(),
        RegDesc::gpr("R13", 13).callee_saved(),
        RegDesc::gpr("R14", 14).callee_saved(),
        RegDesc::gpr("R15", 15).callee_saved(),
        // XMM0-XMM7: FP arguments/return (caller-saved)
        RegDesc::fpr("XMM0", 0).arg(0).return_reg(),
        RegDesc::fpr("XMM1", 1).arg(1).return_reg(),
        RegDesc::fpr("XMM2", 2).arg(2),
        RegDesc::fpr("XMM3", 3).arg(3),
        RegDesc::fpr("XMM4", 4).arg(4),
        RegDesc::fpr("XMM5", 5).arg(5),
        RegDesc::fpr("XMM6", 6).arg(6),
        RegDesc::fpr("XMM7", 7).arg(7),
        // XMM8-XMM15: caller-saved temporaries
        RegDesc::fpr("XMM8", 8),
        RegDesc::fpr("XMM9", 9),
        RegDesc::fpr("XMM10", 10),
        RegDesc::fpr("XMM11", 11),
        RegDesc::fpr("XMM12", 12),
        RegDesc::fpr("XMM13", 13),
        RegDesc::fpr("XMM14", 14),
        RegDesc::fpr("XMM15", 15),
    ];

    let calling_convention = CallingConventionDesc {
        name: "systemv",
        int_arg_regs: vec![7, 6, 2, 1, 8, 9], // RDI, RSI, RDX, RCX, R8, R9
        fp_arg_regs: vec![0, 1, 2, 3, 4, 5, 6, 7],
        int_return_regs: vec![0],                      // RAX
        fp_return_regs: vec![0, 1],                    // XMM0, XMM1
        callee_saved_gprs: vec![3, 5, 12, 13, 14, 15], // RBX, RBP, R12-R15
        callee_saved_fps: vec![],
        stack_alignment: 16,
        has_link_register: false,
        has_branch_delay_slots: false,
        has_toc_pointer: false,
    };

    let instruction_categories = vec![
        InstCategoryDesc {
            name: "arithmetic",
            insts: vec![
                "ADD", "SUB", "IMUL", "IDIV", "AND", "OR", "XOR", "SHL", "SHR", "SAR", "NEG", "NOT",
            ],
        },
        InstCategoryDesc {
            name: "branch",
            insts: vec![
                "JMP", "JE", "JNE", "JL", "JG", "JLE", "JGE", "CALL", "RET", "LOOP",
            ],
        },
        InstCategoryDesc {
            name: "load_store",
            insts: vec!["MOV", "LEA", "PUSH", "POP", "MOVZX", "MOVSX"],
        },
        InstCategoryDesc {
            name: "fp_arithmetic",
            insts: vec![
                "ADDSD", "SUBSD", "MULSD", "DIVSD", "CVTSI2SD", "CVTSD2SI", "UCOMISD",
            ],
        },
        InstCategoryDesc {
            name: "system",
            insts: vec![
                "SYSCALL", "INT", "CPUID", "LFENCE", "MFENCE", "SFENCE", "NOP",
            ],
        },
    ];

    TargetDesc {
        name: "x86_64",
        triple: "x86_64-unknown-linux-gnu",
        elf_machine: 62,
        base_addr: 0x400000,
        pointer_width: 8,
        endianness: Endianness::Little,
        output_format: OutputFormat::Elf64,
        registers,
        calling_convention,
        instruction_categories,
        latency_table: LatencyTable::x86_64(),
    }
}

// ===========================================================================
// ARM32 (AAPCS)
// ===========================================================================

fn arm32_target_desc() -> TargetDesc {
    let registers = vec![
        // R0-R3: argument/return registers (caller-saved)
        RegDesc::gpr("R0", 0).arg(0).return_reg(),
        RegDesc::gpr("R1", 1).arg(1).return_reg(),
        RegDesc::gpr("R2", 2).arg(2),
        RegDesc::gpr("R3", 3).arg(3),
        // R4-R11: callee-saved
        RegDesc::gpr("R4", 4).callee_saved(),
        RegDesc::gpr("R5", 5).callee_saved(),
        RegDesc::gpr("R6", 6).callee_saved(),
        RegDesc::gpr("R7", 7).callee_saved(),
        RegDesc::gpr("R8", 8).callee_saved(),
        RegDesc::gpr("R9", 9).callee_saved(),
        RegDesc::gpr("R10", 10).callee_saved(),
        // R11: frame pointer (callee-saved)
        RegDesc::gpr("R11", 11).frame_pointer().callee_saved(),
        // R12: intra-procedure scratch (IP, caller-saved)
        RegDesc::gpr("R12", 12),
        // R13: stack pointer
        RegDesc::gpr("R13", 13).stack_pointer(),
        // R14: link register
        RegDesc::gpr("R14", 14).link_register(),
        // R15: program counter
        RegDesc::gpr("R15", 15).not_allocatable(),
        // D0-D7: FP argument/return (VFP, caller-saved)
        RegDesc::fpr("D0", 0).arg(0).return_reg(),
        RegDesc::fpr("D1", 1).arg(1).return_reg(),
        RegDesc::fpr("D2", 2).arg(2).return_reg(),
        RegDesc::fpr("D3", 3).arg(3).return_reg(),
        RegDesc::fpr("D4", 4).arg(4),
        RegDesc::fpr("D5", 5).arg(5),
        RegDesc::fpr("D6", 6).arg(6),
        RegDesc::fpr("D7", 7).arg(7),
        // D8-D15: callee-saved
        RegDesc::fpr("D8", 8).callee_saved(),
        RegDesc::fpr("D9", 9).callee_saved(),
        RegDesc::fpr("D10", 10).callee_saved(),
        RegDesc::fpr("D11", 11).callee_saved(),
        RegDesc::fpr("D12", 12).callee_saved(),
        RegDesc::fpr("D13", 13).callee_saved(),
        RegDesc::fpr("D14", 14).callee_saved(),
        RegDesc::fpr("D15", 15).callee_saved(),
        // D16-D31: caller-saved (VFPv3 extension)
        RegDesc::fpr("D16", 16),
        RegDesc::fpr("D17", 17),
        RegDesc::fpr("D18", 18),
        RegDesc::fpr("D19", 19),
        RegDesc::fpr("D20", 20),
        RegDesc::fpr("D21", 21),
        RegDesc::fpr("D22", 22),
        RegDesc::fpr("D23", 23),
        RegDesc::fpr("D24", 24),
        RegDesc::fpr("D25", 25),
        RegDesc::fpr("D26", 26),
        RegDesc::fpr("D27", 27),
        RegDesc::fpr("D28", 28),
        RegDesc::fpr("D29", 29),
        RegDesc::fpr("D30", 30),
        RegDesc::fpr("D31", 31),
    ];

    let calling_convention = CallingConventionDesc {
        name: "aapcs",
        int_arg_regs: vec![0, 1, 2, 3],
        fp_arg_regs: vec![0, 1, 2, 3, 4, 5, 6, 7],
        int_return_regs: vec![0, 1],
        fp_return_regs: vec![0, 1, 2, 3],
        callee_saved_gprs: vec![4, 5, 6, 7, 8, 9, 10, 11],
        callee_saved_fps: vec![8, 9, 10, 11, 12, 13, 14, 15],
        stack_alignment: 8,
        has_link_register: true,
        has_branch_delay_slots: false,
        has_toc_pointer: false,
    };

    let instruction_categories = vec![
        InstCategoryDesc {
            name: "arithmetic",
            insts: vec![
                "ADD", "SUB", "MUL", "MLA", "AND", "ORR", "EOR", "LSL", "LSR", "ASR",
            ],
        },
        InstCategoryDesc {
            name: "branch",
            insts: vec!["B", "BL", "BX", "BLX", "B.cond", "CBZ", "CBNZ"],
        },
        InstCategoryDesc {
            name: "load_store",
            insts: vec![
                "LDR", "STR", "LDM", "STM", "PUSH", "POP", "LDRB", "STRB", "LDRD", "STRD",
            ],
        },
        InstCategoryDesc {
            name: "fp_arithmetic",
            insts: vec![
                "VADD.F64", "VSUB.F64", "VMUL.F64", "VDIV.F64", "VMOV", "VCMP", "VCVT",
            ],
        },
        InstCategoryDesc {
            name: "system",
            insts: vec!["SVC", "MRS", "MSR", "DMB", "DSB", "ISB", "NOP"],
        },
    ];

    TargetDesc {
        name: "arm32",
        triple: "arm-unknown-linux-gnueabihf",
        elf_machine: 40,
        base_addr: 0x10000,
        pointer_width: 4,
        endianness: Endianness::Little,
        output_format: OutputFormat::Elf32,
        registers,
        calling_convention,
        instruction_categories,
        latency_table: LatencyTable::arm32(),
    }
}

// ===========================================================================
// MIPS64 (N64)
// ===========================================================================

fn mips64_target_desc() -> TargetDesc {
    let registers = vec![
        // $0: hardwired zero
        RegDesc::gpr("$0", 0).hardwired_zero(),
        // $1: assembler temporary (at)
        RegDesc::gpr("$1", 1).not_allocatable(),
        // $2-$3: return values v0-v1 (caller-saved)
        RegDesc::gpr("$2", 2).return_reg(),
        RegDesc::gpr("$3", 3).return_reg(),
        // $4-$7: arguments a0-a3 (caller-saved)
        RegDesc::gpr("$4", 4).arg(0),
        RegDesc::gpr("$5", 5).arg(1),
        RegDesc::gpr("$6", 6).arg(2),
        RegDesc::gpr("$7", 7).arg(3),
        // $8-$15: temporaries t0-t7 (caller-saved)
        RegDesc::gpr("$8", 8),
        RegDesc::gpr("$9", 9),
        RegDesc::gpr("$10", 10),
        RegDesc::gpr("$11", 11),
        RegDesc::gpr("$12", 12),
        RegDesc::gpr("$13", 13),
        RegDesc::gpr("$14", 14),
        RegDesc::gpr("$15", 15),
        // $16-$23: saved s0-s7 (callee-saved)
        RegDesc::gpr("$16", 16).callee_saved(),
        RegDesc::gpr("$17", 17).callee_saved(),
        RegDesc::gpr("$18", 18).callee_saved(),
        RegDesc::gpr("$19", 19).callee_saved(),
        RegDesc::gpr("$20", 20).callee_saved(),
        RegDesc::gpr("$21", 21).callee_saved(),
        RegDesc::gpr("$22", 22).callee_saved(),
        RegDesc::gpr("$23", 23).callee_saved(),
        // $24-$25: temporaries t8-t9 (caller-saved)
        RegDesc::gpr("$24", 24),
        RegDesc::gpr("$25", 25),
        // $26-$27: kernel registers k0-k1 (not allocatable)
        RegDesc::gpr("$26", 26).not_allocatable(),
        RegDesc::gpr("$27", 27).not_allocatable(),
        // $28: global pointer (not allocatable)
        RegDesc::gpr("$28", 28).not_allocatable(),
        // $29: stack pointer
        RegDesc::gpr("$29", 29).stack_pointer(),
        // $30: frame pointer (callee-saved)
        RegDesc::gpr("$30", 30).frame_pointer().callee_saved(),
        // $31: return address (link register)
        RegDesc::gpr("$31", 31).link_register(),
        // $f0-$f1: FP return values (caller-saved)
        RegDesc::fpr("$f0", 0).return_reg(),
        RegDesc::fpr("$f1", 1).return_reg(),
        // $f2-$f11: temporaries (caller-saved)
        RegDesc::fpr("$f2", 2),
        RegDesc::fpr("$f3", 3),
        RegDesc::fpr("$f4", 4),
        RegDesc::fpr("$f5", 5),
        RegDesc::fpr("$f6", 6),
        RegDesc::fpr("$f7", 7),
        RegDesc::fpr("$f8", 8),
        RegDesc::fpr("$f9", 9),
        RegDesc::fpr("$f10", 10),
        RegDesc::fpr("$f11", 11),
        // $f12-$f19: FP arguments (caller-saved)
        RegDesc::fpr("$f12", 12).arg(0),
        RegDesc::fpr("$f13", 13).arg(1),
        RegDesc::fpr("$f14", 14).arg(2),
        RegDesc::fpr("$f15", 15).arg(3),
        RegDesc::fpr("$f16", 16).arg(4),
        RegDesc::fpr("$f17", 17).arg(5),
        RegDesc::fpr("$f18", 18).arg(6),
        RegDesc::fpr("$f19", 19).arg(7),
        // $f20-$f31: callee-saved
        RegDesc::fpr("$f20", 20).callee_saved(),
        RegDesc::fpr("$f21", 21).callee_saved(),
        RegDesc::fpr("$f22", 22).callee_saved(),
        RegDesc::fpr("$f23", 23).callee_saved(),
        RegDesc::fpr("$f24", 24).callee_saved(),
        RegDesc::fpr("$f25", 25).callee_saved(),
        RegDesc::fpr("$f26", 26).callee_saved(),
        RegDesc::fpr("$f27", 27).callee_saved(),
        RegDesc::fpr("$f28", 28).callee_saved(),
        RegDesc::fpr("$f29", 29).callee_saved(),
        RegDesc::fpr("$f30", 30).callee_saved(),
        RegDesc::fpr("$f31", 31).callee_saved(),
        // Special: HI, LO multiply/divide registers
        RegDesc::special_reg("HI", 0),
        RegDesc::special_reg("LO", 1),
    ];

    let calling_convention = CallingConventionDesc {
        name: "n64",
        int_arg_regs: vec![4, 5, 6, 7],
        fp_arg_regs: vec![12, 13, 14, 15, 16, 17, 18, 19],
        int_return_regs: vec![2, 3],
        fp_return_regs: vec![0, 1],
        callee_saved_gprs: vec![16, 17, 18, 19, 20, 21, 22, 23, 30],
        callee_saved_fps: vec![20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31],
        stack_alignment: 16,
        has_link_register: true,
        has_branch_delay_slots: true,
        has_toc_pointer: false,
    };

    let instruction_categories = vec![
        InstCategoryDesc {
            name: "arithmetic",
            insts: vec![
                "ADD", "ADDU", "SUB", "SUBU", "MULT", "DIV", "AND", "OR", "XOR", "SLL", "SRL",
                "SRA",
            ],
        },
        InstCategoryDesc {
            name: "branch",
            insts: vec![
                "BEQ", "BNE", "BGTZ", "BLEZ", "BLTZ", "BGEZ", "J", "JAL", "JR", "JALR",
            ],
        },
        InstCategoryDesc {
            name: "load_store",
            insts: vec!["LW", "LD", "SW", "SD", "LB", "SB", "LH", "SH", "LBU", "LHU"],
        },
        InstCategoryDesc {
            name: "fp_arithmetic",
            insts: vec![
                "ADD.D", "SUB.D", "MUL.D", "DIV.D", "MOV.D", "C.LE.D", "CVT.D.W",
            ],
        },
        InstCategoryDesc {
            name: "system",
            insts: vec!["SYSCALL", "BREAK", "ERET", "MFC0", "MTC0", "SYNC"],
        },
    ];

    TargetDesc {
        name: "mips64",
        triple: "mips64-unknown-linux-gnuabi64",
        elf_machine: 8,
        base_addr: 0x120000000,
        pointer_width: 8,
        endianness: Endianness::Big,
        output_format: OutputFormat::Elf64,
        registers,
        calling_convention,
        instruction_categories,
        latency_table: LatencyTable::mips64(),
    }
}

// ===========================================================================
// PowerPC64 (ELFv2)
// ===========================================================================

fn ppc64_target_desc() -> TargetDesc {
    let registers = vec![
        // R0: volatile / scratch (allocatable but has special meaning in some insns)
        RegDesc::gpr("R0", 0),
        // R1: stack pointer
        RegDesc::gpr("R1", 1).stack_pointer(),
        // R2: TOC pointer
        RegDesc::gpr("R2", 2).toc_pointer(),
        // R3-R10: arguments/return (caller-saved)
        RegDesc::gpr("R3", 3).arg(0).return_reg(),
        RegDesc::gpr("R4", 4).arg(1),
        RegDesc::gpr("R5", 5).arg(2),
        RegDesc::gpr("R6", 6).arg(3),
        RegDesc::gpr("R7", 7).arg(4),
        RegDesc::gpr("R8", 8).arg(5),
        RegDesc::gpr("R9", 9).arg(6),
        RegDesc::gpr("R10", 10).arg(7),
        // R11-R12: volatile (caller-saved)
        RegDesc::gpr("R11", 11),
        RegDesc::gpr("R12", 12),
        // R13: thread pointer (not allocatable)
        RegDesc::gpr("R13", 13).not_allocatable(),
        // R14-R31: callee-saved
        RegDesc::gpr("R14", 14).callee_saved(),
        RegDesc::gpr("R15", 15).callee_saved(),
        RegDesc::gpr("R16", 16).callee_saved(),
        RegDesc::gpr("R17", 17).callee_saved(),
        RegDesc::gpr("R18", 18).callee_saved(),
        RegDesc::gpr("R19", 19).callee_saved(),
        RegDesc::gpr("R20", 20).callee_saved(),
        RegDesc::gpr("R21", 21).callee_saved(),
        RegDesc::gpr("R22", 22).callee_saved(),
        RegDesc::gpr("R23", 23).callee_saved(),
        RegDesc::gpr("R24", 24).callee_saved(),
        RegDesc::gpr("R25", 25).callee_saved(),
        RegDesc::gpr("R26", 26).callee_saved(),
        RegDesc::gpr("R27", 27).callee_saved(),
        RegDesc::gpr("R28", 28).callee_saved(),
        RegDesc::gpr("R29", 29).callee_saved(),
        RegDesc::gpr("R30", 30).callee_saved(),
        // R31: callee-saved, traditionally used as frame pointer
        RegDesc::gpr("R31", 31).frame_pointer().callee_saved(),
        // F0: FP return (caller-saved)
        RegDesc::fpr("F0", 0).return_reg(),
        // F1-F13: FP arguments/return (caller-saved)
        RegDesc::fpr("F1", 1).arg(0).return_reg(),
        RegDesc::fpr("F2", 2).arg(1),
        RegDesc::fpr("F3", 3).arg(2),
        RegDesc::fpr("F4", 4).arg(3),
        RegDesc::fpr("F5", 5).arg(4),
        RegDesc::fpr("F6", 6).arg(5),
        RegDesc::fpr("F7", 7).arg(6),
        RegDesc::fpr("F8", 8).arg(7),
        RegDesc::fpr("F9", 9).arg(8),
        RegDesc::fpr("F10", 10).arg(9),
        RegDesc::fpr("F11", 11).arg(10),
        RegDesc::fpr("F12", 12).arg(11),
        RegDesc::fpr("F13", 13).arg(12),
        // F14-F31: callee-saved
        RegDesc::fpr("F14", 14).callee_saved(),
        RegDesc::fpr("F15", 15).callee_saved(),
        RegDesc::fpr("F16", 16).callee_saved(),
        RegDesc::fpr("F17", 17).callee_saved(),
        RegDesc::fpr("F18", 18).callee_saved(),
        RegDesc::fpr("F19", 19).callee_saved(),
        RegDesc::fpr("F20", 20).callee_saved(),
        RegDesc::fpr("F21", 21).callee_saved(),
        RegDesc::fpr("F22", 22).callee_saved(),
        RegDesc::fpr("F23", 23).callee_saved(),
        RegDesc::fpr("F24", 24).callee_saved(),
        RegDesc::fpr("F25", 25).callee_saved(),
        RegDesc::fpr("F26", 26).callee_saved(),
        RegDesc::fpr("F27", 27).callee_saved(),
        RegDesc::fpr("F28", 28).callee_saved(),
        RegDesc::fpr("F29", 29).callee_saved(),
        RegDesc::fpr("F30", 30).callee_saved(),
        RegDesc::fpr("F31", 31).callee_saved(),
        // VS32-VS63: VMX/Altivec vector registers (VSX upper half)
        // VS32-VS33: volatile
        RegDesc::fpr("VS32", 32),
        RegDesc::fpr("VS33", 33),
        // VS34-VS45: vector argument/return registers (V2-V13)
        RegDesc::fpr("VS34", 34),
        RegDesc::fpr("VS35", 35),
        RegDesc::fpr("VS36", 36),
        RegDesc::fpr("VS37", 37),
        RegDesc::fpr("VS38", 38),
        RegDesc::fpr("VS39", 39),
        RegDesc::fpr("VS40", 40),
        RegDesc::fpr("VS41", 41),
        RegDesc::fpr("VS42", 42),
        RegDesc::fpr("VS43", 43),
        RegDesc::fpr("VS44", 44),
        RegDesc::fpr("VS45", 45),
        // VS46-VS51: volatile
        RegDesc::fpr("VS46", 46),
        RegDesc::fpr("VS47", 47),
        RegDesc::fpr("VS48", 48),
        RegDesc::fpr("VS49", 49),
        RegDesc::fpr("VS50", 50),
        RegDesc::fpr("VS51", 51),
        // VS52-VS63: callee-saved (V20-V31)
        RegDesc::fpr("VS52", 52).callee_saved(),
        RegDesc::fpr("VS53", 53).callee_saved(),
        RegDesc::fpr("VS54", 54).callee_saved(),
        RegDesc::fpr("VS55", 55).callee_saved(),
        RegDesc::fpr("VS56", 56).callee_saved(),
        RegDesc::fpr("VS57", 57).callee_saved(),
        RegDesc::fpr("VS58", 58).callee_saved(),
        RegDesc::fpr("VS59", 59).callee_saved(),
        RegDesc::fpr("VS60", 60).callee_saved(),
        RegDesc::fpr("VS61", 61).callee_saved(),
        RegDesc::fpr("VS62", 62).callee_saved(),
        RegDesc::fpr("VS63", 63).callee_saved(),
        // CR0-CR7: condition register fields
        RegDesc::cond_reg("CR0", 0),
        RegDesc::cond_reg("CR1", 1),
        RegDesc::cond_reg("CR2", 2),
        RegDesc::cond_reg("CR3", 3),
        RegDesc::cond_reg("CR4", 4),
        RegDesc::cond_reg("CR5", 5),
        RegDesc::cond_reg("CR6", 6),
        RegDesc::cond_reg("CR7", 7),
        // Special: LR, CTR
        RegDesc::special_reg("LR", 0),
        RegDesc::special_reg("CTR", 1),
    ];

    let calling_convention = CallingConventionDesc {
        name: "elfv2",
        int_arg_regs: vec![3, 4, 5, 6, 7, 8, 9, 10],
        fp_arg_regs: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13],
        int_return_regs: vec![3],
        fp_return_regs: vec![0, 1],
        callee_saved_gprs: vec![
            14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
        ],
        callee_saved_fps: vec![
            14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
        ],
        stack_alignment: 16,
        has_link_register: true,
        has_branch_delay_slots: false,
        has_toc_pointer: true,
    };

    let instruction_categories = vec![
        InstCategoryDesc {
            name: "arithmetic",
            insts: vec![
                "ADD", "SUBF", "MULLD", "DIVD", "AND", "OR", "XOR", "SLD", "SRD", "SRAD", "ADDI",
            ],
        },
        InstCategoryDesc {
            name: "branch",
            insts: vec!["B", "BC", "BCLR", "BCCTR", "BL", "BCLR", "BCCTR"],
        },
        InstCategoryDesc {
            name: "load_store",
            insts: vec![
                "LD", "STD", "LWZ", "STW", "LBZ", "STB", "LHZ", "STH", "LMW", "STMW",
            ],
        },
        InstCategoryDesc {
            name: "fp_arithmetic",
            insts: vec![
                "FADD", "FSUB", "FMUL", "FDIV", "FMOV", "FCMP", "FCVT", "FSQRT",
            ],
        },
        InstCategoryDesc {
            name: "system",
            insts: vec!["SC", "RFI", "MFSPR", "MTSPR", "SYNC", "ISYNC", "NOP"],
        },
    ];

    TargetDesc {
        name: "ppc64",
        triple: "powerpc64le-unknown-linux-gnu",
        elf_machine: 21,
        base_addr: 0x10000000,
        pointer_width: 8,
        endianness: Endianness::Bi,
        output_format: OutputFormat::Elf64,
        registers,
        calling_convention,
        instruction_categories,
        latency_table: LatencyTable::ppc64(),
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the registry contains all 8 ISAs.
    #[test]
    fn test_registry_contains_all_isas() {
        let registry = TargetDescRegistry::new();
        let expected = [
            "aarch64",
            "riscv64",
            "wasm32",
            "loongarch64",
            "x86_64",
            "arm32",
            "mips64",
            "ppc64",
        ];
        for name in &expected {
            assert!(
                registry.get(name).is_some(),
                "Registry missing ISA: {}",
                name
            );
        }
        let names = registry.isa_names();
        assert_eq!(names.len(), 8, "Expected 8 ISAs, got {}", names.len());
    }

    /// Verify no register is both an argument register and callee-saved
    /// within the same register class.
    #[test]
    fn test_no_arg_and_callee_saved_overlap() {
        let registry = TargetDescRegistry::new();
        for name in registry.isa_names() {
            let desc = registry.get(name).unwrap();
            for reg in &desc.registers {
                assert!(
                    !(reg.is_arg_reg && reg.is_callee_saved),
                    "[{}] register {} (class={:?}) is both arg and callee-saved",
                    name,
                    reg.name,
                    reg.class,
                );
            }
        }
    }

    /// Verify allocatable + non-allocatable = total register count.
    #[test]
    fn test_allocatable_plus_non_allocatable_equals_total() {
        let registry = TargetDescRegistry::new();
        for name in registry.isa_names() {
            let desc = registry.get(name).unwrap();
            let total = desc.registers.len();
            let allocatable = desc.registers.iter().filter(|r| r.is_allocatable).count();
            let non_allocatable = desc.registers.iter().filter(|r| !r.is_allocatable).count();
            assert_eq!(
                allocatable + non_allocatable,
                total,
                "[{}] allocatable ({}) + non-allocatable ({}) != total ({})",
                name,
                allocatable,
                non_allocatable,
                total
            );
        }
    }

    /// Verify arg positions are sequential starting from 0 for each
    /// register class within each ISA.
    #[test]
    fn test_arg_positions_sequential_from_zero() {
        let registry = TargetDescRegistry::new();
        for name in registry.isa_names() {
            let desc = registry.get(name).unwrap();

            // Check GPR arg positions
            let mut gpr_args: Vec<usize> = desc
                .registers
                .iter()
                .filter(|r| r.class == RegClass::Gpr && r.is_arg_reg)
                .filter_map(|r| r.arg_position)
                .collect();
            gpr_args.sort();
            let expected: Vec<usize> = (0..gpr_args.len()).collect();
            assert_eq!(
                gpr_args, expected,
                "[{}] GPR arg positions not sequential from 0: got {:?}, expected {:?}",
                name, gpr_args, expected
            );

            // Check SimdFp arg positions
            let mut fp_args: Vec<usize> = desc
                .registers
                .iter()
                .filter(|r| r.class == RegClass::SimdFp && r.is_arg_reg)
                .filter_map(|r| r.arg_position)
                .collect();
            fp_args.sort();
            let expected: Vec<usize> = (0..fp_args.len()).collect();
            assert_eq!(
                fp_args, expected,
                "[{}] SimdFp arg positions not sequential from 0: got {:?}, expected {:?}",
                name, fp_args, expected
            );
        }
    }

    /// Verify each ISA has at least the "arithmetic" and "branch"
    /// instruction categories.
    #[test]
    fn test_required_instruction_categories() {
        let registry = TargetDescRegistry::new();
        for name in registry.isa_names() {
            let desc = registry.get(name).unwrap();
            let category_names: Vec<&str> =
                desc.instruction_categories.iter().map(|c| c.name).collect();
            assert!(
                category_names.contains(&"arithmetic"),
                "[{}] missing 'arithmetic' instruction category",
                name
            );
            assert!(
                category_names.contains(&"branch"),
                "[{}] missing 'branch' instruction category",
                name
            );
        }
    }

    /// Verify each ISA has at least one allocatable register (except wasm32).
    #[test]
    fn test_allocatable_registers_exist() {
        let registry = TargetDescRegistry::new();
        for name in registry.isa_names() {
            let desc = registry.get(name).unwrap();
            let allocatable = desc.registers.iter().filter(|r| r.is_allocatable).count();
            if name == "wasm32" {
                assert_eq!(
                    allocatable, 0,
                    "[{}] wasm32 should have no allocatable registers",
                    name
                );
            } else {
                assert!(
                    allocatable > 0,
                    "[{}] should have at least one allocatable register",
                    name
                );
            }
        }
    }

    /// Verify the calling convention descriptor matches the register
    /// descriptions for each ISA.
    #[test]
    fn test_calling_convention_matches_registers() {
        let registry = TargetDescRegistry::new();
        for name in registry.isa_names() {
            let desc = registry.get(name).unwrap();
            let cc = &desc.calling_convention;

            // Check that int arg reg indices correspond to actual arg registers
            for &idx in &cc.int_arg_regs {
                let reg = desc
                    .registers
                    .iter()
                    .find(|r| r.class == RegClass::Gpr && r.index == idx);
                assert!(
                    reg.is_some(),
                    "[{}] calling convention references GPR index {} but no such register",
                    name,
                    idx
                );
                let reg = reg.unwrap();
                assert!(
                    reg.is_arg_reg,
                    "[{}] calling convention int_arg_regs includes {} ({}) but it's not marked as arg_reg",
                    name, idx, reg.name
                );
            }

            // Check that FP arg reg indices correspond to actual FP arg registers
            for &idx in &cc.fp_arg_regs {
                let reg = desc
                    .registers
                    .iter()
                    .find(|r| r.class == RegClass::SimdFp && r.index == idx);
                assert!(
                    reg.is_some(),
                    "[{}] calling convention references SimdFp index {} but no such register",
                    name,
                    idx
                );
                let reg = reg.unwrap();
                assert!(
                    reg.is_arg_reg,
                    "[{}] calling convention fp_arg_regs includes {} ({}) but it's not marked as arg_reg",
                    name, idx, reg.name
                );
            }

            // Check that callee-saved GPRs are actually marked callee-saved
            for &idx in &cc.callee_saved_gprs {
                let reg = desc
                    .registers
                    .iter()
                    .find(|r| r.class == RegClass::Gpr && r.index == idx);
                assert!(
                    reg.is_some(),
                    "[{}] calling convention callee_saved_gprs references index {} but no such register",
                    name, idx
                );
                let reg = reg.unwrap();
                assert!(
                    reg.is_callee_saved,
                    "[{}] calling convention callee_saved_gprs includes {} ({}) but it's not marked callee-saved",
                    name, idx, reg.name
                );
            }

            // Check that callee-saved FPRs are actually marked callee-saved
            for &idx in &cc.callee_saved_fps {
                let reg = desc
                    .registers
                    .iter()
                    .find(|r| r.class == RegClass::SimdFp && r.index == idx);
                assert!(
                    reg.is_some(),
                    "[{}] calling convention callee_saved_fps references index {} but no such register",
                    name, idx
                );
                let reg = reg.unwrap();
                assert!(
                    reg.is_callee_saved,
                    "[{}] calling convention callee_saved_fps includes {} ({}) but it's not marked callee-saved",
                    name, idx, reg.name
                );
            }
        }
    }

    /// Verify unique register indices within each class for each ISA.
    #[test]
    fn test_unique_register_indices() {
        let registry = TargetDescRegistry::new();
        for name in registry.isa_names() {
            let desc = registry.get(name).unwrap();

            let mut seen_gpr: std::collections::HashSet<usize> = std::collections::HashSet::new();
            let mut seen_fpr: std::collections::HashSet<usize> = std::collections::HashSet::new();
            let mut seen_special: std::collections::HashSet<usize> =
                std::collections::HashSet::new();
            let mut seen_cond: std::collections::HashSet<usize> = std::collections::HashSet::new();

            for reg in &desc.registers {
                let set = match reg.class {
                    RegClass::Gpr => &mut seen_gpr,
                    RegClass::SimdFp => &mut seen_fpr,
                    RegClass::Special => &mut seen_special,
                    RegClass::Condition => &mut seen_cond,
                };
                assert!(
                    set.insert(reg.index),
                    "[{}] duplicate {} index {} for register {}",
                    name,
                    match reg.class {
                        RegClass::Gpr => "GPR",
                        RegClass::SimdFp => "SimdFp",
                        RegClass::Special => "Special",
                        RegClass::Condition => "Condition",
                    },
                    reg.index,
                    reg.name
                );
            }
        }
    }

    /// Verify specific ISA properties that should hold.
    #[test]
    fn test_isa_specific_properties() {
        let registry = TargetDescRegistry::new();

        // AArch64 should have a hardwired zero register
        let aarch64 = registry.get("aarch64").unwrap();
        assert!(
            aarch64.registers.iter().any(|r| r.is_hardwired_zero),
            "AArch64 should have a hardwired zero register"
        );
        assert!(
            aarch64.registers.iter().any(|r| r.is_link_register),
            "AArch64 should have a link register"
        );

        // RISC-V should have a hardwired zero register
        let riscv = registry.get("riscv64").unwrap();
        assert!(
            riscv.registers.iter().any(|r| r.is_hardwired_zero),
            "RISC-V should have a hardwired zero register"
        );

        // MIPS should have branch delay slots
        let mips = registry.get("mips64").unwrap();
        assert!(
            mips.calling_convention.has_branch_delay_slots,
            "MIPS64 should have branch delay slots"
        );

        // PPC should have a TOC pointer
        let ppc = registry.get("ppc64").unwrap();
        assert!(
            ppc.calling_convention.has_toc_pointer,
            "PPC64 should have a TOC pointer"
        );
        assert!(
            ppc.registers.iter().any(|r| r.is_toc_pointer),
            "PPC64 should have a register marked as TOC pointer"
        );

        // x86_64 should NOT have a link register
        let x86 = registry.get("x86_64").unwrap();
        assert!(
            !x86.calling_convention.has_link_register,
            "x86_64 should not have a link register"
        );
        assert!(
            !x86.registers.iter().any(|r| r.is_link_register),
            "x86_64 should not have any register marked as link register"
        );

        // Wasm32 should have no allocatable registers
        let wasm = registry.get("wasm32").unwrap();
        assert_eq!(
            wasm.registers.len(),
            1,
            "Wasm32 should have exactly one pseudo-register"
        );
        assert_eq!(wasm.registers[0].name, "stack");
    }

    // ====================================================================
    // Wave 24 tests — RegisterClass + TargetDesc modeling
    // ====================================================================

    /// (Wave 24, sub-task 3) `TargetDesc::register_classes()` returns a
    /// `RegisterClass` summary for each class present in the target's
    /// register file.  Verify the counts and metadata for x86_64.
    #[test]
    fn wave24_x86_64_register_class_summary() {
        let registry = TargetDescRegistry::new();
        let x86 = registry.get("x86_64").expect("x86_64 in registry");

        let classes = x86.register_classes();
        // x86_64 has GPR and SimdFp classes (no Condition/Special).
        assert!(
            classes.iter().any(|c| c.class == RegClass::Gpr),
            "x86_64 should have a GPR class"
        );
        assert!(
            classes.iter().any(|c| c.class == RegClass::SimdFp),
            "x86_64 should have a SimdFp class"
        );

        let gpr = classes
            .iter()
            .find(|c| c.class == RegClass::Gpr)
            .expect("GPR class present");
        assert_eq!(gpr.name, "GPR");
        // x86_64 GPRs: RAX, RCX, RDX, RBX, RSP, RBP, RSI, RDI, R8-R15 = 16
        assert_eq!(gpr.total_count, 16, "x86_64 should have 16 GPRs total");
        // RSP is non-allocatable; the other 15 are allocatable.
        assert_eq!(
            gpr.allocatable_count, 15,
            "x86_64 should have 15 allocatable GPRs"
        );
        // Callee-saved allocatable: RBX, RBP, R12-R15 = 6.
        assert_eq!(
            gpr.callee_saved_count, 6,
            "x86_64 should have 6 callee-saved allocatable GPRs (RBX, RBP, R12-R15)"
        );
        // Caller-saved allocatable: RAX, RCX, RDX, RSI, RDI, R8-R11 = 9.
        assert_eq!(
            gpr.caller_saved_count, 9,
            "x86_64 should have 9 caller-saved allocatable GPRs"
        );
        // GPR width = pointer width = 8 bytes on x86_64.
        assert_eq!(gpr.width_bytes, 8, "x86_64 GPR width should be 8 bytes");
        // GPR move cost = 1 (single-cycle mov).
        assert_eq!(gpr.move_cost, 1, "x86_64 GPR move cost should be 1");
        assert!(gpr.has_allocatable());

        let fpr = classes
            .iter()
            .find(|c| c.class == RegClass::SimdFp)
            .expect("SimdFp class present");
        assert_eq!(fpr.name, "FPR");
        // x86_64 XMM0-XMM15 = 16 SIMD regs, all allocatable, all caller-saved.
        assert_eq!(fpr.total_count, 16, "x86_64 should have 16 SIMD regs");
        assert_eq!(fpr.allocatable_count, 16);
        assert_eq!(fpr.callee_saved_count, 0, "x86_64 has no callee-saved SIMD");
        assert_eq!(fpr.caller_saved_count, 16);
        // SIMD/FP width = 16 bytes (128-bit XMM).
        assert_eq!(fpr.width_bytes, 16, "x86_64 FPR width should be 16 bytes");
        assert_eq!(fpr.move_cost, 1);
    }

    /// (Wave 24, sub-task 3) `TargetDesc::allocatable_regs(class)` filters
    /// the register file to allocatable regs of the given class.
    #[test]
    fn wave24_allocatable_regs_filter() {
        let registry = TargetDescRegistry::new();
        let x86 = registry.get("x86_64").unwrap();

        let gprs = x86.allocatable_regs(RegClass::Gpr);
        // 15 allocatable GPRs (all except RSP).
        assert_eq!(gprs.len(), 15);
        // RSP should NOT be in the allocatable list.
        assert!(
            !gprs.iter().any(|r| r.is_stack_pointer),
            "RSP should not be allocatable"
        );
        // RBX should be present and callee-saved.
        assert!(
            gprs.iter().any(|r| r.name == "RBX" && r.is_callee_saved),
            "RBX should be an allocatable callee-saved GPR"
        );

        let fprs = x86.allocatable_regs(RegClass::SimdFp);
        assert_eq!(fprs.len(), 16, "all 16 XMM regs should be allocatable");

        // Condition and Special classes are empty for x86_64.
        assert!(x86.allocatable_regs(RegClass::Condition).is_empty());
        assert!(x86.allocatable_regs(RegClass::Special).is_empty());
    }

    /// (Wave 24, sub-task 3) `TargetDesc::move_cost(class)` returns sane
    /// per-class move costs.  Verify the defaults and that they're
    /// consistent with what `register_classes()` reports.
    #[test]
    fn wave24_move_cost_defaults() {
        let registry = TargetDescRegistry::new();
        let x86 = registry.get("x86_64").unwrap();

        // Default move costs: GPR=1, SIMD=1, Condition=2, Special=4.
        assert_eq!(x86.move_cost(RegClass::Gpr), 1);
        assert_eq!(x86.move_cost(RegClass::SimdFp), 1);
        assert_eq!(x86.move_cost(RegClass::Condition), 2);
        assert_eq!(x86.move_cost(RegClass::Special), 4);

        // The move cost returned by `register_classes()` must match
        // `move_cost()` for each class.
        for rc in x86.register_classes() {
            assert_eq!(
                rc.move_cost,
                x86.move_cost(rc.class),
                "register_classes move_cost mismatch for {:?}",
                rc.class
            );
        }
    }

    /// (Wave 24, sub-task 3) Every ISA in the registry should produce a
    /// non-empty `register_classes()` list, and the counts should be
    /// self-consistent (allocatable == caller + callee; total >= allocatable).
    #[test]
    fn wave24_register_class_consistency_all_isas() {
        let registry = TargetDescRegistry::new();
        for name in registry.isa_names() {
            let desc = registry.get(name).unwrap();
            let classes = desc.register_classes();
            assert!(
                !classes.is_empty(),
                "[{}] register_classes() should be non-empty",
                name
            );
            for rc in &classes {
                assert_eq!(
                    rc.allocatable_count,
                    rc.caller_saved_count + rc.callee_saved_count,
                    "[{}] {:?}: allocatable != caller + callee",
                    name,
                    rc.class
                );
                assert!(
                    rc.total_count >= rc.allocatable_count,
                    "[{}] {:?}: total < allocatable",
                    name,
                    rc.class
                );
                // wasm32 has zero allocatable regs — `has_allocatable()`
                // should return false for it; all others should have at
                // least one allocatable class.
                if name == "wasm32" {
                    assert!(
                        !rc.has_allocatable(),
                        "[{}] wasm32 should have no allocatable regs",
                        name
                    );
                }
            }
        }
    }
}
