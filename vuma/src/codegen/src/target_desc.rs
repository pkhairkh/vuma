//! # Target Description System
//!
//! Machine-readable ISA specifications that make adding new ISAs a data-driven
//! process. Each `TargetDesc` contains the complete register file, calling
//! convention details, and instruction category metadata for an ISA.

use crate::backend::{Endianness, OutputFormat, RegClass};

/// A complete machine-readable description of a target ISA.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TargetDesc {
    pub name: &'static str,
    pub triple: &'static str,
    pub elf_machine: u16,
    pub base_addr: u64,
    pub pointer_width: usize,
    pub endianness: Endianness,
    pub output_format: OutputFormat,
    pub registers: Vec<RegDesc>,
    pub calling_convention: CallingConventionDesc,
    pub instruction_categories: Vec<InstCategoryDesc>,
}

/// Description of a single register.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RegDesc {
    pub name: &'static str,
    pub class: RegClass,
    pub index: usize,
    pub is_allocatable: bool,
    pub is_hardwired_zero: bool,
    pub is_stack_pointer: bool,
    pub is_frame_pointer: bool,
    pub is_link_register: bool,
    pub is_toc_pointer: bool,
    pub is_callee_saved: bool,
    pub is_arg_reg: bool,
    pub arg_position: Option<usize>,
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

/// Description of a calling convention.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CallingConventionDesc {
    pub name: &'static str,
    pub int_arg_regs: Vec<usize>,
    pub fp_arg_regs: Vec<usize>,
    pub int_return_regs: Vec<usize>,
    pub fp_return_regs: Vec<usize>,
    pub callee_saved_gprs: Vec<usize>,
    pub callee_saved_fps: Vec<usize>,
    pub stack_alignment: usize,
    pub has_link_register: bool,
    pub has_branch_delay_slots: bool,
    pub has_toc_pointer: bool,
}

/// Description of an instruction category.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstCategoryDesc {
    pub name: &'static str,
    pub insts: Vec<&'static str>,
}

/// Registry of all target descriptions.
pub struct TargetDescRegistry {
    descs: std::collections::HashMap<&'static str, TargetDesc>,
}

impl TargetDescRegistry {
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

    pub fn get(&self, name: &str) -> Option<&TargetDesc> {
        self.descs.get(name)
    }

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
            insts: vec!["ADD", "SUB", "MUL", "SDIV", "UDIV", "AND", "ORR", "EOR", "LSL", "LSR"],
        },
        InstCategoryDesc {
            name: "branch",
            insts: vec!["B", "BL", "BR", "B.cond", "CBZ", "CBNZ", "TBZ", "TBNZ", "RET"],
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
            insts: vec!["ADD", "SUB", "MUL", "DIV", "AND", "OR", "XOR", "SLL", "SRL", "SRA", "ADDI", "SLT"],
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
            insts: vec!["ECALL", "EBREAK", "FENCE", "FENCE.I", "CSRRC", "CSRRS", "CSRRW"],
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
            insts: vec!["i32.add", "i32.sub", "i32.mul", "i64.add", "i64.sub", "i64.mul"],
        },
        InstCategoryDesc {
            name: "branch",
            insts: vec!["br", "br_if", "br_table", "return", "if", "else", "end", "loop", "block"],
        },
        InstCategoryDesc {
            name: "load_store",
            insts: vec!["i32.load", "i32.store", "i64.load", "i64.store", "i32.load8_s", "i32.store8"],
        },
        InstCategoryDesc {
            name: "fp_arithmetic",
            insts: vec!["f32.add", "f32.sub", "f64.add", "f64.sub", "f32.mul", "f64.mul"],
        },
        InstCategoryDesc {
            name: "system",
            insts: vec!["call", "call_indirect", "drop", "nop", "unreachable", "select"],
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
            insts: vec!["ADD.W", "SUB.W", "MUL.W", "DIV.W", "AND", "OR", "XOR", "SLL.W", "SRL.W", "SRA.W"],
        },
        InstCategoryDesc {
            name: "branch",
            insts: vec!["BEQ", "BNE", "BLT", "BGE", "BLTU", "BGEU", "B", "BL", "JIRL"],
        },
        InstCategoryDesc {
            name: "load_store",
            insts: vec!["LD.W", "ST.W", "LD.D", "ST.D", "LD.BU", "ST.B", "LD.HU", "ST.H"],
        },
        InstCategoryDesc {
            name: "fp_arithmetic",
            insts: vec!["FADD.D", "FSUB.D", "FMUL.D", "FDIV.D", "FMOV.D", "FCMP.D", "FCVT"],
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
        int_return_regs: vec![0],  // RAX
        fp_return_regs: vec![0, 1], // XMM0, XMM1
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
            insts: vec!["ADD", "SUB", "IMUL", "IDIV", "AND", "OR", "XOR", "SHL", "SHR", "SAR", "NEG", "NOT"],
        },
        InstCategoryDesc {
            name: "branch",
            insts: vec!["JMP", "JE", "JNE", "JL", "JG", "JLE", "JGE", "CALL", "RET", "LOOP"],
        },
        InstCategoryDesc {
            name: "load_store",
            insts: vec!["MOV", "LEA", "PUSH", "POP", "MOVZX", "MOVSX"],
        },
        InstCategoryDesc {
            name: "fp_arithmetic",
            insts: vec!["ADDSD", "SUBSD", "MULSD", "DIVSD", "CVTSI2SD", "CVTSD2SI", "UCOMISD"],
        },
        InstCategoryDesc {
            name: "system",
            insts: vec!["SYSCALL", "INT", "CPUID", "LFENCE", "MFENCE", "SFENCE", "NOP"],
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
            insts: vec!["ADD", "SUB", "MUL", "MLA", "AND", "ORR", "EOR", "LSL", "LSR", "ASR"],
        },
        InstCategoryDesc {
            name: "branch",
            insts: vec!["B", "BL", "BX", "BLX", "B.cond", "CBZ", "CBNZ"],
        },
        InstCategoryDesc {
            name: "load_store",
            insts: vec!["LDR", "STR", "LDM", "STM", "PUSH", "POP", "LDRB", "STRB", "LDRD", "STRD"],
        },
        InstCategoryDesc {
            name: "fp_arithmetic",
            insts: vec!["VADD.F64", "VSUB.F64", "VMUL.F64", "VDIV.F64", "VMOV", "VCMP", "VCVT"],
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
            insts: vec!["ADD", "ADDU", "SUB", "SUBU", "MULT", "DIV", "AND", "OR", "XOR", "SLL", "SRL", "SRA"],
        },
        InstCategoryDesc {
            name: "branch",
            insts: vec!["BEQ", "BNE", "BGTZ", "BLEZ", "BLTZ", "BGEZ", "J", "JAL", "JR", "JALR"],
        },
        InstCategoryDesc {
            name: "load_store",
            insts: vec!["LW", "LD", "SW", "SD", "LB", "SB", "LH", "SH", "LBU", "LHU"],
        },
        InstCategoryDesc {
            name: "fp_arithmetic",
            insts: vec!["ADD.D", "SUB.D", "MUL.D", "DIV.D", "MOV.D", "C.LE.D", "CVT.D.W"],
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
        callee_saved_gprs: vec![14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31],
        callee_saved_fps: vec![14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31],
        stack_alignment: 16,
        has_link_register: true,
        has_branch_delay_slots: false,
        has_toc_pointer: true,
    };

    let instruction_categories = vec![
        InstCategoryDesc {
            name: "arithmetic",
            insts: vec!["ADD", "SUBF", "MULLD", "DIVD", "AND", "OR", "XOR", "SLD", "SRD", "SRAD", "ADDI"],
        },
        InstCategoryDesc {
            name: "branch",
            insts: vec!["B", "BC", "BCLR", "BCCTR", "BL", "BCLR", "BCCTR"],
        },
        InstCategoryDesc {
            name: "load_store",
            insts: vec!["LD", "STD", "LWZ", "STW", "LBZ", "STB", "LHZ", "STH", "LMW", "STMW"],
        },
        InstCategoryDesc {
            name: "fp_arithmetic",
            insts: vec!["FADD", "FSUB", "FMUL", "FDIV", "FMOV", "FCMP", "FCVT", "FSQRT"],
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
            let allocatable = desc
                .registers
                .iter()
                .filter(|r| r.is_allocatable)
                .count();
            let non_allocatable = desc
                .registers
                .iter()
                .filter(|r| !r.is_allocatable)
                .count();
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
            let category_names: Vec<&str> = desc
                .instruction_categories
                .iter()
                .map(|c| c.name)
                .collect();
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
            let allocatable = desc
                .registers
                .iter()
                .filter(|r| r.is_allocatable)
                .count();
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
                    name, idx
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
                    name, idx
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
            let mut seen_special: std::collections::HashSet<usize> = std::collections::HashSet::new();
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
            aarch64
                .registers
                .iter()
                .any(|r| r.is_hardwired_zero),
            "AArch64 should have a hardwired zero register"
        );
        assert!(
            aarch64
                .registers
                .iter()
                .any(|r| r.is_link_register),
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
}
