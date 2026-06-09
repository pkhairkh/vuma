//! # Intermediate Representation (IR)
//!
//! Defines the IR types used as the central representation between the SCG
//! (Semantic Computation Graph) front-end and the ARM64 code emitter.
//!
//! ## Hierarchy
//!
//! ```text
//! IRProgram
//!  ├── Vec<IRFunction>
//!  │    ├── name, params, results
//!  │    └── Vec<IRBlock>
//!  │         ├── label
//!  │         ├── Vec<IRInstr>
//!  │         └── IRTerminator
//!  └── Vec<DataSection>
//! ```
//!
//! The IR is intentionally low-level but target-independent.  It uses virtual
//! registers (`IRValue::Register(id)`) that are later mapped to physical
//! ARM64 registers by the register allocator.

use std::fmt;

// ---------------------------------------------------------------------------
// IRValue
// ---------------------------------------------------------------------------

/// A value that can appear as an operand in an IR instruction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum IRValue {
    /// A virtual register identified by a numeric ID.
    Register(u32),
    /// An immediate constant.
    Immediate(i64),
    /// A memory address (absolute).
    Address(u64),
    /// A named label (for branch targets).
    Label(String),
}

impl IRValue {
    /// Returns `true` if this is a virtual register.
    pub fn is_register(&self) -> bool {
        matches!(self, IRValue::Register(_))
    }

    /// Returns `true` if this is an immediate constant.
    pub fn is_immediate(&self) -> bool {
        matches!(self, IRValue::Immediate(_))
    }

    /// Extract the register ID, if this is a register value.
    pub fn as_register(&self) -> Option<u32> {
        match self {
            IRValue::Register(id) => Some(*id),
            _ => None,
        }
    }

    /// Extract the immediate value, if this is an immediate.
    pub fn as_immediate(&self) -> Option<i64> {
        match self {
            IRValue::Immediate(v) => Some(*v),
            _ => None,
        }
    }
}

impl fmt::Display for IRValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IRValue::Register(id) => write!(f, "%v{}", id),
            IRValue::Immediate(v) => write!(f, "{}", v),
            IRValue::Address(a) => write!(f, "0x{:016x}", a),
            IRValue::Label(name) => write!(f, "@{}", name),
        }
    }
}

// ---------------------------------------------------------------------------
// Binary / Unary operators
// ---------------------------------------------------------------------------

/// Binary operations supported by the IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BinOpKind {
    Add,
    Sub,
    Mul,
    SDiv,
    UDiv,
    SRem,
    URem,
    And,
    Or,
    Xor,
    Shl,
    ShrL,
    ShrA,
}

impl fmt::Display for BinOpKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BinOpKind::Add => "add",
            BinOpKind::Sub => "sub",
            BinOpKind::Mul => "mul",
            BinOpKind::SDiv => "sdiv",
            BinOpKind::UDiv => "udiv",
            BinOpKind::SRem => "srem",
            BinOpKind::URem => "urem",
            BinOpKind::And => "and",
            BinOpKind::Or => "or",
            BinOpKind::Xor => "xor",
            BinOpKind::Shl => "shl",
            BinOpKind::ShrL => "shr.l",
            BinOpKind::ShrA => "shr.a",
        })
    }
}

/// Unary operations supported by the IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum UnaryOpKind {
    Neg,
    Not,
    Clz,
    Ctz,
    Popcnt,
}

impl fmt::Display for UnaryOpKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UnaryOpKind::Neg => "neg",
            UnaryOpKind::Not => "not",
            UnaryOpKind::Clz => "clz",
            UnaryOpKind::Ctz => "ctz",
            UnaryOpKind::Popcnt => "popcnt",
        })
    }
}

/// Cast / reinterpretation kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CastKind {
    /// Zero-extend (e.g. u8 → u64).
    ZExt,
    /// Sign-extend (e.g. i8 → i64).
    SExt,
    /// Truncate (e.g. i64 → i32).
    Trunc,
    /// Reinterpret bits (no data change, just type change).
    BitCast,
}

impl fmt::Display for CastKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CastKind::ZExt => "zext",
            CastKind::SExt => "sext",
            CastKind::Trunc => "trunc",
            CastKind::BitCast => "bitcast",
        })
    }
}

// ---------------------------------------------------------------------------
// IR Instruction
// ---------------------------------------------------------------------------

/// A single IR instruction.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IRInstr {
    /// Load a value from memory: `dst = load addr`
    Load {
        dst: IRValue,
        addr: IRValue,
    },

    /// Store a value to memory: `store value, addr`
    Store {
        value: IRValue,
        addr: IRValue,
    },

    /// Binary operation: `dst = lhs op rhs`
    BinOp {
        op: BinOpKind,
        dst: IRValue,
        lhs: IRValue,
        rhs: IRValue,
    },

    /// Unary operation: `dst = op operand`
    UnaryOp {
        op: UnaryOpKind,
        dst: IRValue,
        operand: IRValue,
    },

    /// Function call: `dst = call func_name(args…)`
    Call {
        dst: Option<IRValue>,
        func: String,
        args: Vec<IRValue>,
    },

    /// Stack allocation: `dst = alloc size` — reserves `size` bytes on the
    /// stack and returns a pointer in `dst`.
    Alloc {
        dst: IRValue,
        size: u32,
    },

    /// Heap deallocation: `free ptr` — not directly emitted as an instruction;
    /// lowered to a runtime call.
    Free {
        ptr: IRValue,
    },

    /// Type cast / reinterpret: `dst = cast kind src`
    Cast {
        kind: CastKind,
        dst: IRValue,
        src: IRValue,
    },

    /// SSA phi node: `dst = phi [(val, block), …]`
    Phi {
        dst: IRValue,
        incoming: Vec<(IRValue, String)>,
    },

    /// Compute the address of a data symbol: `dst = getaddress name`
    GetAddress {
        dst: IRValue,
        name: String,
    },

    /// Compute `dst = base + offset` (pointer arithmetic).
    Offset {
        dst: IRValue,
        base: IRValue,
        offset: IRValue,
    },
}

impl IRInstr {
    /// Returns the set of virtual-register IDs that this instruction defines
    /// (writes to).
    pub fn defined_regs(&self) -> Vec<u32> {
        match self {
            IRInstr::Load { dst, .. }
            | IRInstr::BinOp { dst, .. }
            | IRInstr::UnaryOp { dst, .. }
            | IRInstr::Alloc { dst, .. }
            | IRInstr::Cast { dst, .. }
            | IRInstr::Phi { dst, .. }
            | IRInstr::GetAddress { dst, .. }
            | IRInstr::Offset { dst, .. } => dst.as_register().into_iter().collect(),
            IRInstr::Call { dst, .. } => dst.as_ref().and_then(|v| v.as_register()).into_iter().collect(),
            IRInstr::Store { .. } | IRInstr::Free { .. } => vec![],
        }
    }

    /// Returns the set of virtual-register IDs that this instruction uses
    /// (reads from).
    pub fn used_regs(&self) -> Vec<u32> {
        match self {
            IRInstr::Load { addr, .. } => addr.as_register().into_iter().collect(),
            IRInstr::Store { value, addr, .. } => {
                let mut r = value.as_register().into_iter().collect::<Vec<_>>();
                r.extend(addr.as_register());
                r
            }
            IRInstr::BinOp { lhs, rhs, .. } => {
                let mut r = lhs.as_register().into_iter().collect::<Vec<_>>();
                r.extend(rhs.as_register());
                r
            }
            IRInstr::UnaryOp { operand, .. } => operand.as_register().into_iter().collect(),
            IRInstr::Call { args, .. } => args.iter().filter_map(|v| v.as_register()).collect(),
            IRInstr::Alloc { .. } | IRInstr::GetAddress { .. } => vec![],
            IRInstr::Free { ptr } => ptr.as_register().into_iter().collect(),
            IRInstr::Cast { src, .. } => src.as_register().into_iter().collect(),
            IRInstr::Phi { incoming, .. } => incoming.iter().filter_map(|(v, _)| v.as_register()).collect(),
            IRInstr::Offset { base, offset, .. } => {
                let mut r = base.as_register().into_iter().collect::<Vec<_>>();
                r.extend(offset.as_register());
                r
            }
        }
    }
}

impl fmt::Display for IRInstr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IRInstr::Load { dst, addr } => write!(f, "{} = load {}", dst, addr),
            IRInstr::Store { value, addr } => write!(f, "store {}, {}", value, addr),
            IRInstr::BinOp { op, dst, lhs, rhs } => {
                write!(f, "{} = {} {}, {}", dst, op, lhs, rhs)
            }
            IRInstr::UnaryOp { op, dst, operand } => {
                write!(f, "{} = {} {}", dst, op, operand)
            }
            IRInstr::Call { dst, func, args } => {
                let args_str = args
                    .iter()
                    .map(|a| format!("{}", a))
                    .collect::<Vec<_>>()
                    .join(", ");
                match dst {
                    Some(d) => write!(f, "{} = call @{}({})", d, func, args_str),
                    None => write!(f, "call @{}({})", func, args_str),
                }
            }
            IRInstr::Alloc { dst, size } => write!(f, "{} = alloc {}", dst, size),
            IRInstr::Free { ptr } => write!(f, "free {}", ptr),
            IRInstr::Cast { kind, dst, src } => {
                write!(f, "{} = {} {}", dst, kind, src)
            }
            IRInstr::Phi { dst, incoming } => {
                let pairs = incoming
                    .iter()
                    .map(|(v, b)| format!("[{}, @{}]", v, b))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "{} = phi {}", dst, pairs)
            }
            IRInstr::GetAddress { dst, name } => {
                write!(f, "{} = getaddress @{}", dst, name)
            }
            IRInstr::Offset { dst, base, offset } => {
                write!(f, "{} = offset {}, {}", dst, base, offset)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// IR Terminator
// ---------------------------------------------------------------------------

/// A block terminator — the last "instruction" in an `IRBlock` that transfers
/// control flow.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IRTerminator {
    /// Unconditional jump to a label.
    Jump(String),
    /// Conditional branch: if `cond` is non-zero, go to `true_block`;
    /// otherwise go to `false_block`.
    Branch {
        cond: IRValue,
        true_block: String,
        false_block: String,
    },
    /// Return from the current function with optional values.
    Return(Vec<IRValue>),
    /// Unreachable code marker (e.g. after a diverging call).
    Unreachable,
}

impl fmt::Display for IRTerminator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IRTerminator::Jump(target) => write!(f, "jump @{}", target),
            IRTerminator::Branch {
                cond,
                true_block,
                false_block,
            } => {
                write!(f, "br {}, @{}, @{}", cond, true_block, false_block)
            }
            IRTerminator::Return(vals) => {
                let vals_str = vals
                    .iter()
                    .map(|v| format!("{}", v))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "ret {}", vals_str)
            }
            IRTerminator::Unreachable => write!(f, "unreachable"),
        }
    }
}

// ---------------------------------------------------------------------------
// IRBlock
// ---------------------------------------------------------------------------

/// A basic block within an IR function.
///
/// Execution enters at the top and falls through each instruction.  The block
/// always ends with exactly one terminator.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IRBlock {
    /// Block label (used as a branch target).
    pub label: String,
    /// Ordered instructions in this block.
    pub instructions: Vec<IRInstr>,
    /// The terminating control-flow instruction.
    pub terminator: IRTerminator,
}

impl IRBlock {
    /// Create a new empty block with the given label and an `Unreachable`
    /// terminator placeholder (callers should replace it).
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            instructions: Vec::new(),
            terminator: IRTerminator::Unreachable,
        }
    }

    /// Append an instruction to this block.
    pub fn push(&mut self, instr: IRInstr) {
        self.instructions.push(instr);
    }
}

impl fmt::Display for IRBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "@{}:", self.label)?;
        for instr in &self.instructions {
            writeln!(f, "  {}", instr)?;
        }
        writeln!(f, "  {}", self.terminator)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// IRFunction
// ---------------------------------------------------------------------------

/// A function in the IR.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IRFunction {
    /// Function name (used as a symbol in the emitted binary).
    pub name: String,
    /// Parameter virtual registers.
    pub params: Vec<IRValue>,
    /// Return-value virtual registers.
    pub results: Vec<IRValue>,
    /// Basic blocks, in layout order.  The first block is the entry block.
    pub blocks: Vec<IRBlock>,
}

impl IRFunction {
    /// Create a new function with the given name and an empty entry block.
    pub fn new(name: impl Into<String>) -> Self {
        let entry_label = "entry".to_string();
        Self {
            name: name.into(),
            params: Vec::new(),
            results: Vec::new(),
            blocks: vec![IRBlock::new(entry_label)],
        }
    }

    /// Returns a mutable reference to the current (last) block.
    pub fn current_block(&mut self) -> &mut IRBlock {
        self.blocks.last_mut().expect("IRFunction must have at least one block")
    }

    /// Append a new block and return its index.
    pub fn append_block(&mut self, label: impl Into<String>) -> usize {
        let idx = self.blocks.len();
        self.blocks.push(IRBlock::new(label));
        idx
    }
}

impl fmt::Display for IRFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let params = self
            .params
            .iter()
            .map(|p| format!("{}", p))
            .collect::<Vec<_>>()
            .join(", ");
        let results = self
            .results
            .iter()
            .map(|r| format!("{}", r))
            .collect::<Vec<_>>()
            .join(", ");
        if results.is_empty() {
            writeln!(f, "fn @{}({}) {{", self.name, params)?;
        } else {
            writeln!(f, "fn @{}({}) -> {} {{", self.name, params, results)?;
        }
        for block in &self.blocks {
            write!(f, "{}", block)?;
        }
        writeln!(f, "}}")
    }
}

// ---------------------------------------------------------------------------
// DataSection
// ---------------------------------------------------------------------------

/// A data section embedded in the emitted binary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DataSection {
    /// Section name (e.g. `"rodata"`, `"data"`, `"bss"`).
    pub name: String,
    /// Section kind determines placement and alignment.
    pub kind: DataSectionKind,
    /// Alignment in bytes (power of two).
    pub align: u32,
    /// Raw data bytes (empty for BSS sections).
    pub data: Vec<u8>,
}

/// Classification of a data section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DataSectionKind {
    /// Read-only data (`.rodata`).
    ReadOnly,
    /// Read-write initialized data (`.data`).
    Data,
    /// Zero-initialized data (`.bss`).
    Bss,
}

// ---------------------------------------------------------------------------
// IRProgram
// ---------------------------------------------------------------------------

/// A complete IR program — the top-level container.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IRProgram {
    /// Functions in the program.
    pub functions: Vec<IRFunction>,
    /// Data sections.
    pub data_sections: Vec<DataSection>,
}

impl IRProgram {
    /// Create an empty program.
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
            data_sections: Vec::new(),
        }
    }
}

impl Default for IRProgram {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for IRProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for func in &self.functions {
            write!(f, "{}", func)?;
        }
        for section in &self.data_sections {
            writeln!(f, "section {} ({:?}), align {}", section.name, section.kind, section.align)?;
            writeln!(f, "  {} bytes", section.data.len())?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ir_value_display() {
        assert_eq!(format!("{}", IRValue::Register(0)), "%v0");
        assert_eq!(format!("{}", IRValue::Immediate(42)), "42");
        assert_eq!(format!("{}", IRValue::Label("entry".into())), "@entry");
    }

    #[test]
    fn ir_function_build() {
        let mut func = IRFunction::new("main");
        func.params.push(IRValue::Register(0));
        func.results.push(IRValue::Register(1));

        let block = func.current_block();
        block.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(1),
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

        let text = format!("{}", func);
        assert!(text.contains("fn @main"));
        assert!(text.contains("add"));
        assert!(text.contains("ret"));
    }

    #[test]
    fn ir_instr_def_use() {
        let instr = IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(2),
            lhs: IRValue::Register(0),
            rhs: IRValue::Register(1),
        };
        assert_eq!(instr.defined_regs(), vec![2]);
        assert_eq!(instr.used_regs(), vec![0, 1]);
    }
}
