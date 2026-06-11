//! # WebAssembly 32-bit Backend
//!
//! Implements the `Backend` trait for the Wasm32 target.  WebAssembly is
//! fundamentally different from register-based ISAs: it is a stack machine
//! with structured control flow and a binary format.  This module provides:
//!
//! - `WasmType` — value types (i32, i64, f32, f64)
//! - `WasmInstr` — exhaustive instruction enum covering all ops needed for VUMA IR
//! - LEB128 encoding/decoding (unsigned and signed)
//! - Wasm binary-format encoder that emits complete `.wasm` modules
//! - `Wasm32Backend` — `Backend` implementation that lowers IR to Wasm bytecode

use crate::backend::{
    AllocatedBlock, AllocatedFunction, AllocatedInstruction, AllocatedProgram, Backend,
    BackendError, Wasm32TargetInfo,
};
use crate::ir::{
    BinOpKind, CastKind, IRFunction, IRInstr, IRType, IRValue, IRTerminator, UnaryOpKind,
};
use std::collections::HashMap;

// ===========================================================================
// Wasm value types
// ===========================================================================

/// WebAssembly value types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum WasmType {
    I32,
    I64,
    F32,
    F64,
}

impl WasmType {
    /// Binary encoding of this type in the Wasm format.
    pub fn to_byte(&self) -> u8 {
        match self {
            WasmType::I32 => 0x7F,
            WasmType::I64 => 0x7E,
            WasmType::F32 => 0x7D,
            WasmType::F64 => 0x7C,
        }
    }

    /// Size in bytes of this Wasm type on the stack.
    pub fn byte_size(&self) -> usize {
        match self {
            WasmType::I32 | WasmType::F32 => 4,
            WasmType::I64 | WasmType::F64 => 8,
        }
    }

    /// Whether this is an integer type.
    pub fn is_integer(&self) -> bool {
        matches!(self, WasmType::I32 | WasmType::I64)
    }

    /// Whether this is a floating-point type.
    pub fn is_float(&self) -> bool {
        matches!(self, WasmType::F32 | WasmType::F64)
    }

    /// Map an IRType to the corresponding WasmType.
    pub fn from_ir_type(ty: &IRType) -> Option<WasmType> {
        match ty {
            IRType::I8 | IRType::I16 | IRType::I32 | IRType::U8 | IRType::U16 | IRType::U32
            | IRType::Ptr | IRType::Func => Some(WasmType::I32),
            IRType::I64 | IRType::U64 => Some(WasmType::I64),
            IRType::F32 => Some(WasmType::F32),
            IRType::F64 => Some(WasmType::F64),
            IRType::Void | IRType::Struct { .. } | IRType::Array { .. } => None,
        }
    }
}

impl std::fmt::Display for WasmType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WasmType::I32 => write!(f, "i32"),
            WasmType::I64 => write!(f, "i64"),
            WasmType::F32 => write!(f, "f32"),
            WasmType::F64 => write!(f, "f64"),
        }
    }
}

// ===========================================================================
// Wasm instructions
// ===========================================================================

/// WebAssembly instructions needed for VUMA IR lowering.
///
/// Each variant corresponds to one Wasm opcode.  Variants that take
/// immediates carry them as fields.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum WasmInstr {
    // ── Control ───────────────────────────────────────────────────────
    Block(WasmType),
    Loop(WasmType),
    If(WasmType),
    Else,
    End,
    Br(u32),
    BrIf(u32),
    BrTable { labels: Vec<u32>, default: u32 },
    Return,
    Call(u32),
    CallIndirect { type_idx: u32, table_idx: u32 },

    // ── Parametric ────────────────────────────────────────────────────
    Select,
    Drop,

    // ── Variable ──────────────────────────────────────────────────────
    LocalGet(u32),
    LocalSet(u32),
    LocalTee(u32),
    GlobalGet(u32),
    GlobalSet(u32),

    // ── Memory ────────────────────────────────────────────────────────
    I32Load { align: u32, offset: u32 },
    I64Load { align: u32, offset: u32 },
    F32Load { align: u32, offset: u32 },
    F64Load { align: u32, offset: u32 },
    I32Store { align: u32, offset: u32 },
    I64Store { align: u32, offset: u32 },
    F32Store { align: u32, offset: u32 },
    F64Store { align: u32, offset: u32 },
    I32Load8S { align: u32, offset: u32 },
    I32Load8U { align: u32, offset: u32 },
    I32Load16S { align: u32, offset: u32 },
    I32Load16U { align: u32, offset: u32 },
    I64Load8S { align: u32, offset: u32 },
    I64Load8U { align: u32, offset: u32 },
    I64Load16S { align: u32, offset: u32 },
    I64Load16U { align: u32, offset: u32 },
    I64Load32S { align: u32, offset: u32 },
    I64Load32U { align: u32, offset: u32 },
    I32Store8 { align: u32, offset: u32 },
    I32Store16 { align: u32, offset: u32 },
    I64Store8 { align: u32, offset: u32 },
    I64Store16 { align: u32, offset: u32 },
    I64Store32 { align: u32, offset: u32 },
    MemorySize(u32),
    MemoryGrow(u32),

    // ── Numeric i32 ──────────────────────────────────────────────────
    I32Const(i32),
    I32Eqz,
    I32Eq,
    I32Ne,
    I32LtS,
    I32LtU,
    I32GtS,
    I32GtU,
    I32LeS,
    I32LeU,
    I32GeS,
    I32GeU,
    I32Clz,
    I32Ctz,
    I32Popcnt,
    I32Add,
    I32Sub,
    I32Mul,
    I32DivS,
    I32DivU,
    I32RemS,
    I32RemU,
    I32And,
    I32Or,
    I32Xor,
    I32Shl,
    I32ShrS,
    I32ShrU,
    I32Rotl,
    I32Rotr,

    // ── Numeric i64 ──────────────────────────────────────────────────
    I64Const(i64),
    I64Eqz,
    I64Eq,
    I64Ne,
    I64LtS,
    I64LtU,
    I64GtS,
    I64GtU,
    I64LeS,
    I64LeU,
    I64GeS,
    I64GeU,
    I64Clz,
    I64Ctz,
    I64Popcnt,
    I64Add,
    I64Sub,
    I64Mul,
    I64DivS,
    I64DivU,
    I64RemS,
    I64RemU,
    I64And,
    I64Or,
    I64Xor,
    I64Shl,
    I64ShrS,
    I64ShrU,
    I64Rotl,
    I64Rotr,

    // ── Numeric f32 ──────────────────────────────────────────────────
    F32Const(f32),
    F32Eq,
    F32Ne,
    F32Lt,
    F32Gt,
    F32Le,
    F32Ge,
    F32Add,
    F32Sub,
    F32Mul,
    F32Div,
    F32Sqrt,
    F32Neg,

    // ── Numeric f64 ──────────────────────────────────────────────────
    F64Const(f64),
    F64Eq,
    F64Ne,
    F64Lt,
    F64Gt,
    F64Le,
    F64Ge,
    F64Add,
    F64Sub,
    F64Mul,
    F64Div,
    F64Sqrt,
    F64Neg,

    // ── Conversions ──────────────────────────────────────────────────
    I32WrapI64,
    I64ExtendI32S,
    I64ExtendI32U,
    F32DemoteF64,
    F64PromoteF32,
    I32TruncF32S,
    I32TruncF64S,
    I64TruncF32S,
    I64TruncF64S,
    F32ConvertI32S,
    F32ConvertI64S,
    F64ConvertI32S,
    F64ConvertI64S,
    I32ReinterpretF32,
    I64ReinterpretF64,
    F32ReinterpretI32,
    F64ReinterpretI64,

    // ── Pseudo-instruction: no-op (used for IR ops that lower to nothing) ──
    Nop,

    // ── Unreachable ───────────────────────────────────────────────────
    /// unreachable: traps unconditionally (opcode 0x00)
    Unreachable,

    // ── SIMD v128 instructions ────────────────────────────────────────
    /// v128.const: push a 128-bit vector constant
    V128Const([u8; 16]),
    /// i32x4.add: lane-wise addition of two i32x4 vectors
    I32X4Add,
    /// i32x4.mul: lane-wise multiplication of two i32x4 vectors
    I32X4Mul,
    /// f32x4.add: lane-wise addition of two f32x4 vectors
    F32X4Add,
    /// f32x4.mul: lane-wise multiplication of two f32x4 vectors
    F32X4Mul,

    // ── Bulk Memory Operations ────────────────────────────────────────
    /// memory.copy: copy from one memory region to another
    MemoryCopy { src_mem: u32, dst_mem: u32 },
    /// memory.fill: fill a memory region with a byte value
    MemoryFill { mem: u32 },
    /// memory.init: initialize memory from a data segment
    MemoryInit { data_idx: u32, mem: u32 },
}

impl WasmInstr {
    /// Encode this instruction into Wasm bytecode, appending bytes to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            // ── Control ──────────────────────────────────────────────
            WasmInstr::Block(ty) => {
                out.push(0x02);
                out.push(ty.to_byte());
            }
            WasmInstr::Loop(ty) => {
                out.push(0x03);
                out.push(ty.to_byte());
            }
            WasmInstr::If(ty) => {
                out.push(0x04);
                out.push(ty.to_byte());
            }
            WasmInstr::Else => out.push(0x05),
            WasmInstr::End => out.push(0x0B),
            WasmInstr::Br(idx) => {
                out.push(0x0C);
                out.extend_from_slice(&encode_unsigned_leb128(*idx as u64));
            }
            WasmInstr::BrIf(idx) => {
                out.push(0x0D);
                out.extend_from_slice(&encode_unsigned_leb128(*idx as u64));
            }
            WasmInstr::BrTable { labels, default } => {
                out.push(0x0E);
                out.extend_from_slice(&encode_unsigned_leb128(labels.len() as u64));
                for &lbl in labels {
                    out.extend_from_slice(&encode_unsigned_leb128(lbl as u64));
                }
                out.extend_from_slice(&encode_unsigned_leb128(*default as u64));
            }
            WasmInstr::Return => out.push(0x0F),
            WasmInstr::Call(idx) => {
                out.push(0x10);
                out.extend_from_slice(&encode_unsigned_leb128(*idx as u64));
            }
            WasmInstr::CallIndirect { type_idx, table_idx } => {
                out.push(0x11);
                out.extend_from_slice(&encode_unsigned_leb128(*type_idx as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*table_idx as u64));
            }

            // ── Parametric ──────────────────────────────────────────
            WasmInstr::Select => out.push(0x1B),
            WasmInstr::Drop => out.push(0x1A),

            // ── Variable ────────────────────────────────────────────
            WasmInstr::LocalGet(idx) => {
                out.push(0x20);
                out.extend_from_slice(&encode_unsigned_leb128(*idx as u64));
            }
            WasmInstr::LocalSet(idx) => {
                out.push(0x21);
                out.extend_from_slice(&encode_unsigned_leb128(*idx as u64));
            }
            WasmInstr::LocalTee(idx) => {
                out.push(0x22);
                out.extend_from_slice(&encode_unsigned_leb128(*idx as u64));
            }
            WasmInstr::GlobalGet(idx) => {
                out.push(0x23);
                out.extend_from_slice(&encode_unsigned_leb128(*idx as u64));
            }
            WasmInstr::GlobalSet(idx) => {
                out.push(0x24);
                out.extend_from_slice(&encode_unsigned_leb128(*idx as u64));
            }

            // ── Memory ──────────────────────────────────────────────
            WasmInstr::I32Load { align, offset } => {
                out.push(0x28);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I64Load { align, offset } => {
                out.push(0x29);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::F32Load { align, offset } => {
                out.push(0x2A);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::F64Load { align, offset } => {
                out.push(0x2B);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I32Store { align, offset } => {
                out.push(0x36);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I64Store { align, offset } => {
                out.push(0x37);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::F32Store { align, offset } => {
                out.push(0x38);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::F64Store { align, offset } => {
                out.push(0x39);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I32Load8S { align, offset } => {
                out.push(0x2C);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I32Load8U { align, offset } => {
                out.push(0x2D);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I32Load16S { align, offset } => {
                out.push(0x2E);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I32Load16U { align, offset } => {
                out.push(0x2F);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I64Load8S { align, offset } => {
                out.push(0x30);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I64Load8U { align, offset } => {
                out.push(0x31);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I64Load16S { align, offset } => {
                out.push(0x32);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I64Load16U { align, offset } => {
                out.push(0x33);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I64Load32S { align, offset } => {
                out.push(0x34);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I64Load32U { align, offset } => {
                out.push(0x35);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I32Store8 { align, offset } => {
                out.push(0x3A);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I32Store16 { align, offset } => {
                out.push(0x3B);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I64Store8 { align, offset } => {
                out.push(0x3C);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I64Store16 { align, offset } => {
                out.push(0x3D);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::I64Store32 { align, offset } => {
                out.push(0x3E);
                out.extend_from_slice(&encode_unsigned_leb128(*align as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*offset as u64));
            }
            WasmInstr::MemorySize(idx) => {
                out.push(0x3F);
                out.extend_from_slice(&encode_unsigned_leb128(*idx as u64));
            }
            WasmInstr::MemoryGrow(idx) => {
                out.push(0x40);
                out.extend_from_slice(&encode_unsigned_leb128(*idx as u64));
            }

            // ── Numeric i32 ─────────────────────────────────────────
            WasmInstr::I32Const(val) => {
                out.push(0x41);
                out.extend_from_slice(&encode_signed_leb128(*val as i64));
            }
            WasmInstr::I32Eqz => out.push(0x45),
            WasmInstr::I32Eq => out.push(0x46),
            WasmInstr::I32Ne => out.push(0x47),
            WasmInstr::I32LtS => out.push(0x48),
            WasmInstr::I32LtU => out.push(0x49),
            WasmInstr::I32GtS => out.push(0x4A),
            WasmInstr::I32GtU => out.push(0x4B),
            WasmInstr::I32LeS => out.push(0x4C),
            WasmInstr::I32LeU => out.push(0x4D),
            WasmInstr::I32GeS => out.push(0x4E),
            WasmInstr::I32GeU => out.push(0x4F),
            WasmInstr::I32Clz => out.push(0x67),
            WasmInstr::I32Ctz => out.push(0x68),
            WasmInstr::I32Popcnt => out.push(0x69),
            WasmInstr::I32Add => out.push(0x6A),
            WasmInstr::I32Sub => out.push(0x6B),
            WasmInstr::I32Mul => out.push(0x6C),
            WasmInstr::I32DivS => out.push(0x6D),
            WasmInstr::I32DivU => out.push(0x6E),
            WasmInstr::I32RemS => out.push(0x6F),
            WasmInstr::I32RemU => out.push(0x70),
            WasmInstr::I32And => out.push(0x71),
            WasmInstr::I32Or => out.push(0x72),
            WasmInstr::I32Xor => out.push(0x73),
            WasmInstr::I32Shl => out.push(0x74),
            WasmInstr::I32ShrS => out.push(0x75),
            WasmInstr::I32ShrU => out.push(0x76),
            WasmInstr::I32Rotl => out.push(0x77),
            WasmInstr::I32Rotr => out.push(0x78),

            // ── Numeric i64 ─────────────────────────────────────────
            WasmInstr::I64Const(val) => {
                out.push(0x42);
                out.extend_from_slice(&encode_signed_leb128(*val));
            }
            WasmInstr::I64Eqz => out.push(0x50),
            WasmInstr::I64Eq => out.push(0x51),
            WasmInstr::I64Ne => out.push(0x52),
            WasmInstr::I64LtS => out.push(0x53),
            WasmInstr::I64LtU => out.push(0x54),
            WasmInstr::I64GtS => out.push(0x55),
            WasmInstr::I64GtU => out.push(0x56),
            WasmInstr::I64LeS => out.push(0x57),
            WasmInstr::I64LeU => out.push(0x58),
            WasmInstr::I64GeS => out.push(0x59),
            WasmInstr::I64GeU => out.push(0x5A),
            WasmInstr::I64Clz => out.push(0x79),
            WasmInstr::I64Ctz => out.push(0x7A),
            WasmInstr::I64Popcnt => out.push(0x7B),
            WasmInstr::I64Add => out.push(0x7C),
            WasmInstr::I64Sub => out.push(0x7D),
            WasmInstr::I64Mul => out.push(0x7E),
            WasmInstr::I64DivS => out.push(0x7F),
            WasmInstr::I64DivU => out.push(0x80),
            WasmInstr::I64RemS => out.push(0x81),
            WasmInstr::I64RemU => out.push(0x82),
            WasmInstr::I64And => out.push(0x83),
            WasmInstr::I64Or => out.push(0x84),
            WasmInstr::I64Xor => out.push(0x85),
            WasmInstr::I64Shl => out.push(0x86),
            WasmInstr::I64ShrS => out.push(0x87),
            WasmInstr::I64ShrU => out.push(0x88),
            WasmInstr::I64Rotl => out.push(0x89),
            WasmInstr::I64Rotr => out.push(0x8A),

            // ── Numeric f32 ─────────────────────────────────────────
            WasmInstr::F32Const(val) => {
                out.push(0x43);
                out.extend_from_slice(&val.to_le_bytes());
            }
            WasmInstr::F32Eq => out.push(0x5B),
            WasmInstr::F32Ne => out.push(0x5C),
            WasmInstr::F32Lt => out.push(0x5D),
            WasmInstr::F32Gt => out.push(0x5E),
            WasmInstr::F32Le => out.push(0x5F),
            WasmInstr::F32Ge => out.push(0x60),
            WasmInstr::F32Add => out.push(0x92),
            WasmInstr::F32Sub => out.push(0x93),
            WasmInstr::F32Mul => out.push(0x94),
            WasmInstr::F32Div => out.push(0x95),
            WasmInstr::F32Sqrt => out.push(0x91),
            WasmInstr::F32Neg => out.push(0x8C),

            // ── Numeric f64 ─────────────────────────────────────────
            WasmInstr::F64Const(val) => {
                out.push(0x44);
                out.extend_from_slice(&val.to_le_bytes());
            }
            WasmInstr::F64Eq => out.push(0x61),
            WasmInstr::F64Ne => out.push(0x62),
            WasmInstr::F64Lt => out.push(0x63),
            WasmInstr::F64Gt => out.push(0x64),
            WasmInstr::F64Le => out.push(0x65),
            WasmInstr::F64Ge => out.push(0x66),
            WasmInstr::F64Add => out.push(0xA0),
            WasmInstr::F64Sub => out.push(0xA1),
            WasmInstr::F64Mul => out.push(0xA2),
            WasmInstr::F64Div => out.push(0xA3),
            WasmInstr::F64Sqrt => out.push(0x9F),
            WasmInstr::F64Neg => out.push(0x9A),

            // ── Conversions ─────────────────────────────────────────
            WasmInstr::I32WrapI64 => out.push(0xA7),
            WasmInstr::I64ExtendI32S => out.push(0xAC),
            WasmInstr::I64ExtendI32U => out.push(0xAD),
            WasmInstr::F32DemoteF64 => out.push(0xB6),
            WasmInstr::F64PromoteF32 => out.push(0xBB),
            WasmInstr::I32TruncF32S => out.push(0xA8),
            WasmInstr::I32TruncF64S => out.push(0xA9),
            WasmInstr::I64TruncF32S => out.push(0xAE),
            WasmInstr::I64TruncF64S => out.push(0xAF),
            WasmInstr::F32ConvertI32S => out.push(0xB2),
            WasmInstr::F32ConvertI64S => out.push(0xB3),
            WasmInstr::F64ConvertI32S => out.push(0xB7),
            WasmInstr::F64ConvertI64S => out.push(0xB8),
            WasmInstr::I32ReinterpretF32 => out.push(0xBC),
            WasmInstr::I64ReinterpretF64 => out.push(0xBD),
            WasmInstr::F32ReinterpretI32 => out.push(0xBE),
            WasmInstr::F64ReinterpretI64 => out.push(0xBF),

            // ── Nop ─────────────────────────────────────────────────
            WasmInstr::Nop => out.push(0x01),

            // ── Unreachable ──────────────────────────────────────────
            WasmInstr::Unreachable => out.push(0x00),

            // ── SIMD v128 ──────────────────────────────────────────
            WasmInstr::V128Const(bytes) => {
                out.push(0xFD); // SIMD prefix
                out.extend_from_slice(&encode_unsigned_leb128(0x0C)); // v128.const opcode
                out.extend_from_slice(bytes);
            }
            WasmInstr::I32X4Add => {
                out.push(0xFD);
                out.extend_from_slice(&encode_unsigned_leb128(0x0E)); // i32x4.add
            }
            WasmInstr::I32X4Mul => {
                out.push(0xFD);
                out.extend_from_slice(&encode_unsigned_leb128(0x15)); // i32x4.mul
            }
            WasmInstr::F32X4Add => {
                out.push(0xFD);
                out.extend_from_slice(&encode_unsigned_leb128(0x2C)); // f32x4.add
            }
            WasmInstr::F32X4Mul => {
                out.push(0xFD);
                out.extend_from_slice(&encode_unsigned_leb128(0x35)); // f32x4.mul
            }

            // ── Bulk Memory Operations ────────────────────────────
            WasmInstr::MemoryCopy { src_mem, dst_mem } => {
                out.push(0xFC); // multi-byte op prefix
                out.extend_from_slice(&encode_unsigned_leb128(0x0A)); // memory.copy
                out.extend_from_slice(&encode_unsigned_leb128(*src_mem as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*dst_mem as u64));
            }
            WasmInstr::MemoryFill { mem } => {
                out.push(0xFC);
                out.extend_from_slice(&encode_unsigned_leb128(0x0B)); // memory.fill
                out.extend_from_slice(&encode_unsigned_leb128(*mem as u64));
            }
            WasmInstr::MemoryInit { data_idx, mem } => {
                out.push(0xFC);
                out.extend_from_slice(&encode_unsigned_leb128(0x08)); // memory.init
                out.extend_from_slice(&encode_unsigned_leb128(*data_idx as u64));
                out.extend_from_slice(&encode_unsigned_leb128(*mem as u64));
            }
        }
    }

    /// Encode this instruction and return the bytes as a new Vec.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }
}

// ===========================================================================
// LEB128 encoding / decoding
// ===========================================================================

/// Encode an unsigned 64-bit value as unsigned LEB128.
pub fn encode_unsigned_leb128(value: u64) -> Vec<u8> {
    let mut result = Vec::new();
    let mut val = value;
    loop {
        let mut byte = (val & 0x7F) as u8;
        val >>= 7;
        if val != 0 {
            byte |= 0x80;
        }
        result.push(byte);
        if val == 0 {
            break;
        }
    }
    result
}

/// Encode a signed 64-bit value as signed LEB128.
pub fn encode_signed_leb128(value: i64) -> Vec<u8> {
    let mut result = Vec::new();
    let mut val = value;
    loop {
        let mut byte = (val & 0x7F) as u8;
        val >>= 7;
        // Sign bit of byte is bit 6
        let sign_bit = (byte & 0x40) != 0;
        // If val is 0 and sign bit is clear, or val is -1 and sign bit is set, we're done
        if (val == 0 && !sign_bit) || (val == -1 && sign_bit) {
            result.push(byte);
            break;
        }
        byte |= 0x80;
        result.push(byte);
    }
    result
}

/// Decode an unsigned LEB128 value from a byte slice.
/// Returns (decoded_value, number_of_bytes_consumed).
pub fn decode_unsigned_leb128(bytes: &[u8]) -> (u64, usize) {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    let mut count = 0;
    for &byte in bytes {
        count += 1;
        result |= ((byte & 0x7F) as u64) << shift;
        if (byte & 0x80) == 0 {
            break;
        }
        shift += 7;
    }
    (result, count)
}

/// Decode a signed LEB128 value from a byte slice.
/// Returns (decoded_value, number_of_bytes_consumed).
pub fn decode_signed_leb128(bytes: &[u8]) -> (i64, usize) {
    let mut result: i64 = 0;
    let mut shift: u32 = 0;
    let mut count = 0;
    let mut byte: u8;
    loop {
        byte = bytes[count];
        count += 1;
        result |= ((byte & 0x7F) as i64) << shift;
        shift += 7;
        if (byte & 0x80) == 0 {
            break;
        }
    }
    // Sign-extend if the sign bit is set
    if shift < 64 && (byte & 0x40) != 0 {
        result |= !0i64 << shift;
    }
    (result, count)
}

// ===========================================================================
// Wasm binary format encoder
// ===========================================================================

/// Wasm section IDs.
const SECTION_TYPE: u8 = 1;
const SECTION_IMPORT: u8 = 2;
const SECTION_FUNCTION: u8 = 3;
const SECTION_TABLE: u8 = 4;
const SECTION_MEMORY: u8 = 5;
const SECTION_GLOBAL: u8 = 6;
const SECTION_EXPORT: u8 = 7;
const SECTION_START: u8 = 8;
const SECTION_ELEMENT: u8 = 9;
const SECTION_CODE: u8 = 10;
const SECTION_DATA: u8 = 11;

/// Wasm magic number and version.
const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D]; // "\0asm"
const WASM_VERSION: [u8; 4] = [0x01, 0x00, 0x00, 0x00]; // version 1

/// A Wasm function type signature: (params) -> (results).
///
/// Supports multi-value returns (Wasm 2.0 feature): the `results` vector
/// may contain more than one type.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WasmFuncType {
    pub params: Vec<WasmType>,
    pub results: Vec<WasmType>,
}

impl WasmFuncType {
    /// Encode this function type in Wasm binary format.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(0x60); // func type tag
        out.extend_from_slice(&encode_unsigned_leb128(self.params.len() as u64));
        for p in &self.params {
            out.push(p.to_byte());
        }
        out.extend_from_slice(&encode_unsigned_leb128(self.results.len() as u64));
        for r in &self.results {
            out.push(r.to_byte());
        }
        out
    }
}

/// A Wasm import entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WasmImport {
    pub module: String,
    pub name: String,
    pub kind: WasmImportKind,
}

impl WasmImport {
    /// Create a WASI `fd_write` import (wasip1).
    ///
    /// Signature: (fd: i32, iov_ptr: i32, iov_cnt: i32, nwritten_ptr: i32) -> i32
    pub fn wasi_fd_write(type_idx: u32) -> Self {
        WasmImport {
            module: "wasi_snapshot_preview1".to_string(),
            name: "fd_write".to_string(),
            kind: WasmImportKind::Function { type_idx },
        }
    }

    /// Create a WASI `proc_exit` import (wasip1).
    ///
    /// Signature: (exit_code: i32) -> ()
    pub fn wasi_proc_exit(type_idx: u32) -> Self {
        WasmImport {
            module: "wasi_snapshot_preview1".to_string(),
            name: "proc_exit".to_string(),
            kind: WasmImportKind::Function { type_idx },
        }
    }
}

/// Kind of import.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WasmImportKind {
    Function { type_idx: u32 },
    Table { elem_type: u8, limits: WasmLimits },
    Memory { limits: WasmLimits },
    Global { val_type: WasmType, mutable: bool },
}

/// Wasm limits (for tables and memories).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WasmLimits {
    pub min: u32,
    pub max: Option<u32>,
}

impl WasmLimits {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self.max {
            Some(max) => {
                out.push(0x01);
                out.extend_from_slice(&encode_unsigned_leb128(self.min as u64));
                out.extend_from_slice(&encode_unsigned_leb128(max as u64));
            }
            None => {
                out.push(0x00);
                out.extend_from_slice(&encode_unsigned_leb128(self.min as u64));
            }
        }
        out
    }
}

/// A Wasm export entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WasmExport {
    pub name: String,
    pub kind: WasmExportKind,
    pub index: u32,
}

/// Kind of export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WasmExportKind {
    Function = 0,
    Table = 1,
    Memory = 2,
    Global = 3,
}

/// A Wasm global variable.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WasmGlobal {
    pub val_type: WasmType,
    pub mutable: bool,
    pub init_expr: Vec<u8>, // init expr bytes (e.g. i32.const + end)
}

/// A Wasm data segment.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WasmDataSegment {
    pub memory_index: u32,
    pub offset_expr: Vec<u8>, // init expr for offset
    pub data: Vec<u8>,
}

/// A Wasm element segment (for indirect calls).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WasmElementSegment {
    pub table_index: u32,
    pub offset_expr: Vec<u8>,
    pub func_indices: Vec<u32>,
}

/// Builder for a complete Wasm module.
#[derive(Debug, Clone, Default)]
pub struct WasmModuleBuilder {
    pub types: Vec<WasmFuncType>,
    pub imports: Vec<WasmImport>,
    pub functions: Vec<u32>,       // type indices for each function
    pub tables: Vec<(u8, WasmLimits)>, // (elem_type, limits)
    pub memories: Vec<WasmLimits>,
    pub globals: Vec<WasmGlobal>,
    pub exports: Vec<WasmExport>,
    pub start_func: Option<u32>,
    pub elements: Vec<WasmElementSegment>,
    pub code: Vec<WasmFuncBody>,   // one per function (after imports)
    pub data: Vec<WasmDataSegment>,
    pub num_imported_functions: u32,
}

/// A Wasm function body.
///
/// Supports local declarations (count + type pairs) as per the Wasm binary format.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WasmFuncBody {
    pub locals: Vec<(u32, WasmType)>, // (count, type) pairs
    pub body: Vec<u8>,                // bytecode of the function body
}

impl WasmFuncBody {
    /// Create a new function body with given locals and body bytecode.
    pub fn new(locals: Vec<(u32, WasmType)>, body: Vec<u8>) -> Self {
        Self { locals, body }
    }

    /// Create a function body with no extra locals.
    pub fn from_body(body: Vec<u8>) -> Self {
        Self { locals: vec![], body }
    }
}

impl WasmModuleBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a function type and return its index.
    pub fn add_type(&mut self, ft: WasmFuncType) -> u32 {
        let idx = self.types.len() as u32;
        self.types.push(ft);
        idx
    }

    /// Add a function (type index only; body added separately).
    pub fn add_function(&mut self, type_idx: u32) -> u32 {
        let idx = self.num_imported_functions + (self.functions.len() as u32);
        self.functions.push(type_idx);
        idx
    }

    /// Add a function body (must match the function index).
    pub fn add_code(&mut self, body: WasmFuncBody) {
        self.code.push(body);
    }

    /// Add a memory definition.
    pub fn add_memory(&mut self, limits: WasmLimits) -> u32 {
        let idx = self.memories.len() as u32;
        self.memories.push(limits);
        idx
    }

    /// Add an import.
    pub fn add_import(&mut self, import: WasmImport) {
        if let WasmImportKind::Function { .. } = import.kind {
            self.num_imported_functions += 1;
        }
        self.imports.push(import);
    }

    /// Add an export.
    pub fn add_export(&mut self, export: WasmExport) {
        self.exports.push(export);
    }

    /// Add a data segment.
    pub fn add_data(&mut self, segment: WasmDataSegment) {
        self.data.push(segment);
    }

    /// Set the start function.
    pub fn set_start(&mut self, func_idx: u32) {
        self.start_func = Some(func_idx);
    }

    /// Encode the complete module into bytes.
    pub fn encode(self) -> Vec<u8> {
        let mut module = Vec::new();

        // Magic + version
        module.extend_from_slice(&WASM_MAGIC);
        module.extend_from_slice(&WASM_VERSION);

        // Type section
        if !self.types.is_empty() {
            let mut section = Vec::new();
            section.extend_from_slice(&encode_unsigned_leb128(self.types.len() as u64));
            for ft in &self.types {
                section.extend_from_slice(&ft.encode());
            }
            emit_section(&mut module, SECTION_TYPE, &section);
        }

        // Import section
        if !self.imports.is_empty() {
            let mut section = Vec::new();
            section.extend_from_slice(&encode_unsigned_leb128(self.imports.len() as u64));
            for imp in &self.imports {
                section.extend_from_slice(&encode_unsigned_leb128(imp.module.len() as u64));
                section.extend_from_slice(imp.module.as_bytes());
                section.extend_from_slice(&encode_unsigned_leb128(imp.name.len() as u64));
                section.extend_from_slice(imp.name.as_bytes());
                match &imp.kind {
                    WasmImportKind::Function { type_idx } => {
                        section.push(0x00);
                        section.extend_from_slice(&encode_unsigned_leb128(*type_idx as u64));
                    }
                    WasmImportKind::Table { elem_type, limits } => {
                        section.push(0x01);
                        section.push(*elem_type);
                        section.extend_from_slice(&limits.encode());
                    }
                    WasmImportKind::Memory { limits } => {
                        section.push(0x02);
                        section.extend_from_slice(&limits.encode());
                    }
                    WasmImportKind::Global { val_type, mutable } => {
                        section.push(0x03);
                        section.push(val_type.to_byte());
                        section.push(if *mutable { 0x01 } else { 0x00 });
                    }
                }
            }
            emit_section(&mut module, SECTION_IMPORT, &section);
        }

        // Function section
        if !self.functions.is_empty() {
            let mut section = Vec::new();
            section.extend_from_slice(&encode_unsigned_leb128(self.functions.len() as u64));
            for &type_idx in &self.functions {
                section.extend_from_slice(&encode_unsigned_leb128(type_idx as u64));
            }
            emit_section(&mut module, SECTION_FUNCTION, &section);
        }

        // Table section
        if !self.tables.is_empty() {
            let mut section = Vec::new();
            section.extend_from_slice(&encode_unsigned_leb128(self.tables.len() as u64));
            for (elem_type, limits) in &self.tables {
                section.push(*elem_type);
                section.extend_from_slice(&limits.encode());
            }
            emit_section(&mut module, SECTION_TABLE, &section);
        }

        // Memory section
        if !self.memories.is_empty() {
            let mut section = Vec::new();
            section.extend_from_slice(&encode_unsigned_leb128(self.memories.len() as u64));
            for limits in &self.memories {
                section.extend_from_slice(&limits.encode());
            }
            emit_section(&mut module, SECTION_MEMORY, &section);
        }

        // Global section
        if !self.globals.is_empty() {
            let mut section = Vec::new();
            section.extend_from_slice(&encode_unsigned_leb128(self.globals.len() as u64));
            for g in &self.globals {
                section.push(g.val_type.to_byte());
                section.push(if g.mutable { 0x01 } else { 0x00 });
                section.extend_from_slice(&g.init_expr);
            }
            emit_section(&mut module, SECTION_GLOBAL, &section);
        }

        // Export section
        if !self.exports.is_empty() {
            let mut section = Vec::new();
            section.extend_from_slice(&encode_unsigned_leb128(self.exports.len() as u64));
            for exp in &self.exports {
                section.extend_from_slice(&encode_unsigned_leb128(exp.name.len() as u64));
                section.extend_from_slice(exp.name.as_bytes());
                section.push(exp.kind as u8);
                section.extend_from_slice(&encode_unsigned_leb128(exp.index as u64));
            }
            emit_section(&mut module, SECTION_EXPORT, &section);
        }

        // Start section
        if let Some(func_idx) = self.start_func {
            let section = encode_unsigned_leb128(func_idx as u64);
            emit_section(&mut module, SECTION_START, &section);
        }

        // Element section
        if !self.elements.is_empty() {
            let mut section = Vec::new();
            section.extend_from_slice(&encode_unsigned_leb128(self.elements.len() as u64));
            for elem in &self.elements {
                section.extend_from_slice(&encode_unsigned_leb128(elem.table_index as u64));
                section.extend_from_slice(&elem.offset_expr);
                section.extend_from_slice(&encode_unsigned_leb128(elem.func_indices.len() as u64));
                for &idx in &elem.func_indices {
                    section.extend_from_slice(&encode_unsigned_leb128(idx as u64));
                }
            }
            emit_section(&mut module, SECTION_ELEMENT, &section);
        }

        // Code section
        if !self.code.is_empty() {
            let mut section = Vec::new();
            section.extend_from_slice(&encode_unsigned_leb128(self.code.len() as u64));
            for func_body in &self.code {
                // Encode the function body: locals + code + end byte
                let mut body_bytes = Vec::new();
                body_bytes.extend_from_slice(&encode_unsigned_leb128(func_body.locals.len() as u64));
                for (count, ty) in &func_body.locals {
                    body_bytes.extend_from_slice(&encode_unsigned_leb128(*count as u64));
                    body_bytes.push(ty.to_byte());
                }
                body_bytes.extend_from_slice(&func_body.body);
                // Function body is size-prefixed
                let body_size = body_bytes.len() as u64;
                section.extend_from_slice(&encode_unsigned_leb128(body_size));
                section.extend_from_slice(&body_bytes);
            }
            emit_section(&mut module, SECTION_CODE, &section);
        }

        // Data section
        if !self.data.is_empty() {
            let mut section = Vec::new();
            section.extend_from_slice(&encode_unsigned_leb128(self.data.len() as u64));
            for seg in &self.data {
                section.extend_from_slice(&encode_unsigned_leb128(seg.memory_index as u64));
                section.extend_from_slice(&seg.offset_expr);
                section.extend_from_slice(&encode_unsigned_leb128(seg.data.len() as u64));
                section.extend_from_slice(&seg.data);
            }
            emit_section(&mut module, SECTION_DATA, &section);
        }

        module
    }
}

/// Emit a Wasm section: section ID + LEB128 size + content.
fn emit_section(module: &mut Vec<u8>, section_id: u8, content: &[u8]) {
    module.push(section_id);
    module.extend_from_slice(&encode_unsigned_leb128(content.len() as u64));
    module.extend_from_slice(content);
}

/// Skip `count` unsigned LEB128 values in the byte slice, advancing `offset`.
fn skip_leb128(bytes: &[u8], offset: &mut usize, count: usize) {
    for _ in 0..count {
        while *offset < bytes.len() && (bytes[*offset] & 0x80) != 0 {
            *offset += 1;
        }
        if *offset < bytes.len() {
            *offset += 1;
        }
    }
}

/// Skip `count` signed LEB128 values in the byte slice, advancing `offset`.
fn skip_signed_leb128(bytes: &[u8], offset: &mut usize, count: usize) {
    // Signed and unsigned LEB128 have the same encoding structure for skipping
    skip_leb128(bytes, offset, count);
}

/// Skip a single Wasm instruction starting at `offset`, advancing past it.
///
/// This is used by `allocate_registers` to split the body bytecode into
/// per-instruction `AllocatedInstruction` entries with real opcode names.
fn skip_one_instruction(bytes: &[u8], offset: &mut usize) {
    if *offset >= bytes.len() {
        return;
    }
    let byte = bytes[*offset];
    *offset += 1;

    match byte {
        // ── Multi-byte opcodes ────────────────────────────────────
        0xFC => {
            // Bulk memory / saturated arithmetic prefix
            skip_leb128(bytes, offset, 1); // sub-opcode
            let (subop, _) = decode_unsigned_leb128(&bytes[*offset - 1..]);
            match subop {
                0x08 => skip_leb128(bytes, offset, 2), // memory.init
                0x0A => skip_leb128(bytes, offset, 2), // memory.copy
                0x0B => skip_leb128(bytes, offset, 1), // memory.fill
                _ => {} // other sub-ops may have 0 or more operands
            }
        }
        0xFD => {
            // SIMD prefix
            skip_leb128(bytes, offset, 1); // sub-opcode
            let (subop, _) = decode_unsigned_leb128(&bytes[*offset - 1..]);
            if subop == 0x0C {
                // v128.const: skip 16 bytes of payload
                *offset += 16;
            }
        }

        // ── Control flow ─────────────────────────────────────────
        0x00 => {} // unreachable
        0x01 => {} // nop
        0x02..=0x04 => { *offset += 1; } // block/loop/if + blocktype
        0x05 | 0x0B => {} // else, end
        0x0C | 0x0D => { skip_leb128(bytes, offset, 1); } // br, br_if
        0x0E => {
            // br_table: count + labels + default
            let (count, size) = decode_unsigned_leb128(&bytes[*offset..]);
            *offset += size;
            skip_leb128(bytes, offset, count as usize + 1);
        }
        0x0F => {} // return
        0x10 => { skip_leb128(bytes, offset, 1); } // call
        0x11 => { skip_leb128(bytes, offset, 2); } // call_indirect

        // ── Parametric ───────────────────────────────────────────
        0x1A | 0x1B => {} // drop, select

        // ── Variable ─────────────────────────────────────────────
        0x20..=0x24 => { skip_leb128(bytes, offset, 1); } // local.get/set/tee, global.get/set

        // ── Memory ───────────────────────────────────────────────
        0x28..=0x3E => { skip_leb128(bytes, offset, 2); } // loads/stores: align + offset
        0x3F | 0x40 => { skip_leb128(bytes, offset, 1); } // memory.size, memory.grow

        // ── Numeric ──────────────────────────────────────────────
        0x41 => { skip_signed_leb128(bytes, offset, 1); } // i32.const
        0x42 => { skip_signed_leb128(bytes, offset, 1); } // i64.const
        0x43 => { *offset += 4; } // f32.const
        0x44 => { *offset += 8; } // f64.const

        // All other single-byte opcodes (comparisons, arithmetic, conversions)
        0x45..=0x4F | 0x50..=0x5A | 0x5B..=0x66 | 0x67..=0x78
        | 0x79..=0x8A | 0x8C | 0x91..=0x95 | 0x9A | 0x9F..=0xA3
        | 0xA7..=0xBF => {}

        _ => {} // Unknown opcode; nothing more to skip
    }
}

// ===========================================================================
// IR → Wasm lowering
// ===========================================================================

/// Context for lowering an IR function to Wasm bytecode.
struct LoweringContext {
    /// Map from virtual register ID to local index.
    vreg_to_local: HashMap<u32, u32>,
    /// Number of locals (including parameters).
    num_locals: u32,
    /// Local declarations (for the function body).
    locals: Vec<(u32, WasmType)>,
    /// Map from block label to its label depth index for branch targets.
    block_labels: HashMap<String, u32>,
    /// Accumulated Wasm instructions.
    instrs: Vec<WasmInstr>,
}

impl LoweringContext {
    fn new() -> Self {
        Self {
            vreg_to_local: HashMap::new(),
            num_locals: 0,
            locals: Vec::new(),
            block_labels: HashMap::new(),
            instrs: Vec::new(),
        }
    }

    /// Allocate a local for a virtual register, returning the local index.
    fn alloc_local(&mut self, vreg_id: u32, ty: WasmType) -> u32 {
        let idx = self.num_locals;
        self.vreg_to_local.insert(vreg_id, idx);
        self.num_locals += 1;
        // Try to merge with an existing (count, type) entry
        if let Some(last) = self.locals.last_mut() {
            if last.1 == ty {
                last.0 += 1;
                return idx;
            }
        }
        self.locals.push((1, ty));
        idx
    }

    /// Get the local index for a virtual register.
    fn get_local(&self, vreg_id: u32) -> Option<u32> {
        self.vreg_to_local.get(&vreg_id).copied()
    }

    /// Emit an instruction.
    fn emit(&mut self, instr: WasmInstr) {
        self.instrs.push(instr);
    }

    /// Push a value onto the Wasm stack from an IRValue.
    fn push_value(&mut self, val: &IRValue, type_hint: Option<&IRType>) {
        match val {
            IRValue::Immediate(v) => {
                let ty = type_hint.and_then(WasmType::from_ir_type).unwrap_or(WasmType::I32);
                match ty {
                    WasmType::I32 => self.emit(WasmInstr::I32Const(*v as i32)),
                    WasmType::I64 => self.emit(WasmInstr::I64Const(*v)),
                    WasmType::F32 => self.emit(WasmInstr::F32Const(*v as f32)),
                    WasmType::F64 => self.emit(WasmInstr::F64Const(*v as f64)),
                }
            }
            IRValue::Register(id) => {
                if let Some(local_idx) = self.get_local(*id) {
                    self.emit(WasmInstr::LocalGet(local_idx));
                }
            }
            IRValue::Address(addr) => {
                self.emit(WasmInstr::I32Const(*addr as i32)); // wasm32: pointers are i32
            }
            IRValue::Label(_) => {
                // Labels are handled via block structure; not pushed as values
            }
        }
    }

    /// Store the top of the Wasm stack into a virtual register's local.
    fn pop_to_vreg(&mut self, vreg_id: u32, ty: WasmType) {
        if !self.vreg_to_local.contains_key(&vreg_id) {
            self.alloc_local(vreg_id, ty);
        }
        if let Some(local_idx) = self.get_local(vreg_id) {
            self.emit(WasmInstr::LocalSet(local_idx));
        }
    }
}

/// Determine the Wasm type for an IR BinOp based on the operand types.
/// Since IR doesn't carry per-instruction types, we infer from context:
/// 64-bit integer ops use i64, 32-bit use i32, float ops use f32/f64.
fn wasm_type_for_binop(_op: &BinOpKind, lhs: &IRValue, rhs: &IRValue) -> WasmType {
    // If either operand is an i64 immediate, use i64
    if let IRValue::Immediate(v) = lhs {
        if *v != (*v as i32 as i64) {
            return WasmType::I64;
        }
    }
    if let IRValue::Immediate(v) = rhs {
        if *v != (*v as i32 as i64) {
            return WasmType::I64;
        }
    }
    // Default to i32 for integer ops
    WasmType::I32
}

/// Infer the Wasm type of an IR value based on its representation.
///
/// For immediates, we check if the value fits in i32; for registers and
/// addresses, we default to i32 (the Wasm32 pointer type).  This is a
/// heuristic — the IR does not carry per-value type information — but it
/// works for the common cases needed during instruction selection.
fn infer_wasm_type(val: &IRValue) -> WasmType {
    match val {
        IRValue::Immediate(v) => {
            if *v != (*v as i32 as i64) {
                WasmType::I64
            } else {
                WasmType::I32
            }
        }
        IRValue::Register(_) | IRValue::Address(_) | IRValue::Label(_) => WasmType::I32,
    }
}

/// Lower an IR function to Wasm bytecode, returning the function body bytes
/// and local declarations.
fn lower_function(func: &IRFunction) -> Result<(WasmFuncBody, WasmFuncType), BackendError> {
    let mut ctx = LoweringContext::new();

    // Assign locals for parameters
    for (i, param) in func.params.iter().enumerate() {
        let ty = func.param_types.get(i)
            .and_then(WasmType::from_ir_type)
            .unwrap_or(WasmType::I32);
        if let IRValue::Register(id) = param {
            let idx = ctx.num_locals;
            ctx.vreg_to_local.insert(*id, idx);
            ctx.num_locals += 1;
            if let Some(last) = ctx.locals.last_mut() {
                if last.1 == ty {
                    last.0 += 1;
                } else {
                    ctx.locals.push((1, ty));
                }
            } else {
                ctx.locals.push((1, ty));
            }
        }
    }

    // Lower each block
    for (block_idx, block) in func.blocks.iter().enumerate() {
        // Emit a block for structured control flow (except the entry block)
        if block_idx > 0 {
            ctx.emit(WasmInstr::Block(WasmType::I32)); // placeholder block type
            ctx.block_labels.insert(block.label.clone(), block_idx as u32);
        } else {
            ctx.block_labels.insert(block.label.clone(), 0);
        }

        // Lower instructions
        for instr in &block.instructions {
            lower_instruction(instr, &mut ctx)?;
        }

        // Lower terminator
        lower_terminator(&block.terminator, &mut ctx, block_idx)?;

        // End the block
        if block_idx > 0 {
            ctx.emit(WasmInstr::End);
        }
    }

    // Build the function type
    let param_types: Vec<WasmType> = func.param_types.iter()
        .filter_map(WasmType::from_ir_type)
        .collect();
    let result_types: Vec<WasmType> = func.result_types.iter()
        .filter_map(WasmType::from_ir_type)
        .collect();

    let func_type = WasmFuncType {
        params: param_types,
        results: result_types,
    };

    // Encode all instructions to bytecode
    let mut body_bytes = Vec::new();
    for instr in &ctx.instrs {
        instr.encode(&mut body_bytes);
    }
    // Append the implicit end byte for the function body
    body_bytes.push(0x0B);

    let func_body = WasmFuncBody {
        locals: ctx.locals,
        body: body_bytes,
    };

    Ok((func_body, func_type))
}

/// Lower a single IR instruction.
fn lower_instruction(instr: &IRInstr, ctx: &mut LoweringContext) -> Result<(), BackendError> {
    match instr {
        IRInstr::BinOp { op, dst, lhs, rhs } => {
            let ty = wasm_type_for_binop(op, lhs, rhs);
            ctx.push_value(lhs, None);
            ctx.push_value(rhs, None);
            let wasm_op = match ty {
                WasmType::I32 => match op {
                    BinOpKind::Add => WasmInstr::I32Add,
                    BinOpKind::Sub => WasmInstr::I32Sub,
                    BinOpKind::Mul => WasmInstr::I32Mul,
                    BinOpKind::SDiv => WasmInstr::I32DivS,
                    BinOpKind::UDiv => WasmInstr::I32DivU,
                    BinOpKind::SRem => WasmInstr::I32RemS,
                    BinOpKind::URem => WasmInstr::I32RemU,
                    BinOpKind::And => WasmInstr::I32And,
                    BinOpKind::Or => WasmInstr::I32Or,
                    BinOpKind::Xor => WasmInstr::I32Xor,
                    BinOpKind::Shl => WasmInstr::I32Shl,
                    BinOpKind::ShrL => WasmInstr::I32ShrU,
                    BinOpKind::ShrA => WasmInstr::I32ShrS,
                    BinOpKind::SLt => WasmInstr::I32LtS,
                    BinOpKind::SLe => WasmInstr::I32LeS,
                    BinOpKind::SGt => WasmInstr::I32GtS,
                    BinOpKind::SGe => WasmInstr::I32GeS,
                    BinOpKind::ULt => WasmInstr::I32LtU,
                    BinOpKind::ULe => WasmInstr::I32LeU,
                    BinOpKind::UGt => WasmInstr::I32GtU,
                    BinOpKind::UGe => WasmInstr::I32GeU,
                    BinOpKind::Eq => WasmInstr::I32Eq,
                    BinOpKind::Ne => WasmInstr::I32Ne,
                },
                WasmType::I64 => match op {
                    BinOpKind::Add => WasmInstr::I64Add,
                    BinOpKind::Sub => WasmInstr::I64Sub,
                    BinOpKind::Mul => WasmInstr::I64Mul,
                    BinOpKind::SDiv => WasmInstr::I64DivS,
                    BinOpKind::UDiv => WasmInstr::I64DivU,
                    BinOpKind::SRem => WasmInstr::I64RemS,
                    BinOpKind::URem => WasmInstr::I64RemU,
                    BinOpKind::And => WasmInstr::I64And,
                    BinOpKind::Or => WasmInstr::I64Or,
                    BinOpKind::Xor => WasmInstr::I64Xor,
                    BinOpKind::Shl => WasmInstr::I64Shl,
                    BinOpKind::ShrL => WasmInstr::I64ShrU,
                    BinOpKind::ShrA => WasmInstr::I64ShrS,
                    BinOpKind::SLt => WasmInstr::I64LtS,
                    BinOpKind::SLe => WasmInstr::I64LeS,
                    BinOpKind::SGt => WasmInstr::I64GtS,
                    BinOpKind::SGe => WasmInstr::I64GeS,
                    BinOpKind::ULt => WasmInstr::I64LtU,
                    BinOpKind::ULe => WasmInstr::I64LeU,
                    BinOpKind::UGt => WasmInstr::I64GtU,
                    BinOpKind::UGe => WasmInstr::I64GeU,
                    BinOpKind::Eq => WasmInstr::I64Eq,
                    BinOpKind::Ne => WasmInstr::I64Ne,
                },
                WasmType::F32 => match op {
                    BinOpKind::Add => WasmInstr::F32Add,
                    BinOpKind::Sub => WasmInstr::F32Sub,
                    BinOpKind::Mul => WasmInstr::F32Mul,
                    BinOpKind::SDiv | BinOpKind::UDiv => WasmInstr::F32Div,
                    BinOpKind::Eq => WasmInstr::F32Eq,
                    BinOpKind::Ne => WasmInstr::F32Ne,
                    BinOpKind::SLt | BinOpKind::ULt => WasmInstr::F32Lt,
                    BinOpKind::SGt | BinOpKind::UGt => WasmInstr::F32Gt,
                    BinOpKind::SLe | BinOpKind::ULe => WasmInstr::F32Le,
                    BinOpKind::SGe | BinOpKind::UGe => WasmInstr::F32Ge,
                    _ => WasmInstr::I32Add, // fallback for unsupported float ops
                },
                WasmType::F64 => match op {
                    BinOpKind::Add => WasmInstr::F64Add,
                    BinOpKind::Sub => WasmInstr::F64Sub,
                    BinOpKind::Mul => WasmInstr::F64Mul,
                    BinOpKind::SDiv | BinOpKind::UDiv => WasmInstr::F64Div,
                    BinOpKind::Eq => WasmInstr::F64Eq,
                    BinOpKind::Ne => WasmInstr::F64Ne,
                    BinOpKind::SLt | BinOpKind::ULt => WasmInstr::F64Lt,
                    BinOpKind::SGt | BinOpKind::UGt => WasmInstr::F64Gt,
                    BinOpKind::SLe | BinOpKind::ULe => WasmInstr::F64Le,
                    BinOpKind::SGe | BinOpKind::UGe => WasmInstr::F64Ge,
                    _ => WasmInstr::I32Add, // fallback for unsupported float ops
                },
            };
            ctx.emit(wasm_op);
            if let IRValue::Register(id) = dst {
                ctx.pop_to_vreg(*id, ty);
            } else {
                ctx.emit(WasmInstr::Drop);
            }
        }

        IRInstr::UnaryOp { op, dst, operand } => {
            // Determine the Wasm type of the operand for type-aware lowering.
            // For register operands, we infer the type from context; for
            // immediates, we check if the value fits in i32.
            let ty = infer_wasm_type(operand);

            match op {
                UnaryOpKind::Neg => {
                    // Float negation uses dedicated Wasm instructions;
                    // integer negation is lowered as (0 - x).
                    match ty {
                        WasmType::F32 => {
                            ctx.push_value(operand, Some(&IRType::F32));
                            ctx.emit(WasmInstr::F32Neg);
                        }
                        WasmType::F64 => {
                            ctx.push_value(operand, Some(&IRType::F64));
                            ctx.emit(WasmInstr::F64Neg);
                        }
                        _ => {
                            // Integer negation: 0 - x
                            ctx.push_value(operand, None);
                            // Store operand to a temp local, then compute 0 - operand
                            if let IRValue::Register(id) = dst {
                                let temp_local = if let Some(idx) = ctx.get_local(*id) {
                                    idx
                                } else {
                                    ctx.alloc_local(*id, ty);
                                    ctx.get_local(*id).unwrap()
                                };
                                ctx.emit(WasmInstr::LocalSet(temp_local));
                                ctx.emit(WasmInstr::I32Const(0));
                                ctx.emit(WasmInstr::LocalGet(temp_local));
                                ctx.emit(WasmInstr::I32Sub);
                                ctx.pop_to_vreg(*id, ty);
                            } else {
                                ctx.emit(WasmInstr::Drop);
                                ctx.emit(WasmInstr::I32Const(0));
                            }
                            return Ok(());
                        }
                    }
                    if let IRValue::Register(id) = dst {
                        ctx.pop_to_vreg(*id, ty);
                    } else {
                        ctx.emit(WasmInstr::Drop);
                    }
                }
                UnaryOpKind::Not => {
                    // Logical not: i32.eqz (produces 1 if 0, 0 otherwise)
                    ctx.push_value(operand, None);
                    ctx.emit(WasmInstr::I32Eqz);
                    if let IRValue::Register(id) = dst {
                        ctx.pop_to_vreg(*id, WasmType::I32);
                    } else {
                        ctx.emit(WasmInstr::Drop);
                    }
                }
                UnaryOpKind::Clz | UnaryOpKind::Ctz | UnaryOpKind::Popcnt => {
                    ctx.push_value(operand, None);
                    let wasm_op = match op {
                        UnaryOpKind::Clz => WasmInstr::I32Clz,
                        UnaryOpKind::Ctz => WasmInstr::I32Ctz,
                        UnaryOpKind::Popcnt => WasmInstr::I32Popcnt,
                        _ => unreachable!(),
                    };
                    ctx.emit(wasm_op);
                    if let IRValue::Register(id) = dst {
                        ctx.pop_to_vreg(*id, WasmType::I32);
                    } else {
                        ctx.emit(WasmInstr::Drop);
                    }
                }
            }
        }

        IRInstr::Load { dst, addr } => {
            ctx.push_value(addr, Some(&IRType::I32));
            ctx.emit(WasmInstr::I32Load { align: 2, offset: 0 });
            if let IRValue::Register(id) = dst {
                ctx.pop_to_vreg(*id, WasmType::I32);
            } else {
                ctx.emit(WasmInstr::Drop);
            }
        }

        IRInstr::Store { value, addr } => {
            ctx.push_value(addr, Some(&IRType::I32));
            ctx.push_value(value, None);
            ctx.emit(WasmInstr::I32Store { align: 2, offset: 0 });
        }

        IRInstr::Call { dst, func: _, args } => {
            for arg in args {
                ctx.push_value(arg, None);
            }
            // The function index is resolved during module linking;
            // use a placeholder index 0 here.
            ctx.emit(WasmInstr::Call(0));
            if let Some(IRValue::Register(id)) = dst {
                ctx.pop_to_vreg(*id, WasmType::I32);
            }
            // If the function returns void, nothing to drop
        }

        IRInstr::Alloc { dst, size } => {
            // In Wasm, Alloc maps to a local variable.
            // We allocate `size` bytes worth of locals (as i32 slots).
            let num_slots = (*size).div_ceil(4); // round up to i32 slots
            if let IRValue::Register(id) = dst {
                // Use the first slot as the "pointer" (which is just the local index)
                // For Wasm, locals ARE the storage; we just allocate them
                let _local_idx = ctx.alloc_local(*id, WasmType::I32);
                // For multi-slot allocs, allocate additional locals
                for extra_idx in 1..num_slots {
                    ctx.alloc_local(u32::MAX - *id - extra_idx, WasmType::I32); // dummy vreg IDs for extra slots
                }
                // Push the local index as the "address" (pointer)
                if let Some(idx) = ctx.get_local(*id) {
                    ctx.emit(WasmInstr::I32Const(idx as i32));
                    ctx.emit(WasmInstr::LocalSet(idx));
                }
            }
        }

        IRInstr::Free { ptr: _ } => {
            // Wasm has no free; memory management is handled by the runtime.
            // This is a no-op in Wasm.
        }

        IRInstr::Cast { kind, dst, src } => {
            // Infer source and destination types for proper bitcast lowering.
            let src_ty = infer_wasm_type(src);
            ctx.push_value(src, None);
            let (wasm_op, result_ty) = match kind {
                CastKind::Trunc => (WasmInstr::I32WrapI64, WasmType::I32),
                CastKind::SExt => (WasmInstr::I64ExtendI32S, WasmType::I64),
                CastKind::ZExt => (WasmInstr::I64ExtendI32U, WasmType::I64),
                CastKind::BitCast => {
                    // BitCast reinterprets the bits of a value as a different
                    // type of the same size.  Use the proper Wasm reinterpret
                    // instructions; for same-type casts, no instruction needed.
                    match src_ty {
                        WasmType::I32 => (WasmInstr::F32ReinterpretI32, WasmType::F32),
                        WasmType::I64 => (WasmInstr::F64ReinterpretI64, WasmType::F64),
                        WasmType::F32 => (WasmInstr::I32ReinterpretF32, WasmType::I32),
                        WasmType::F64 => (WasmInstr::I64ReinterpretF64, WasmType::I64),
                    }
                }
            };
            ctx.emit(wasm_op);
            if let IRValue::Register(id) = dst {
                ctx.pop_to_vreg(*id, result_ty);
            } else {
                ctx.emit(WasmInstr::Drop);
            }
        }

        IRInstr::Phi { .. } => {
            // Phi nodes are resolved during SSA destruction before lowering.
            // They should not appear here, but we treat them as no-ops.
        }

        IRInstr::GetAddress { dst, name: _ } => {
            // In Wasm, addresses are offsets in linear memory.
            // We use i32.const with a placeholder; the linker resolves it.
            ctx.emit(WasmInstr::I32Const(0)); // placeholder
            if let IRValue::Register(id) = dst {
                ctx.pop_to_vreg(*id, WasmType::I32);
            } else {
                ctx.emit(WasmInstr::Drop);
            }
        }

        IRInstr::Offset { dst, base, offset } => {
            ctx.push_value(base, Some(&IRType::I32));
            ctx.push_value(offset, Some(&IRType::I32));
            ctx.emit(WasmInstr::I32Add);
            if let IRValue::Register(id) = dst {
                ctx.pop_to_vreg(*id, WasmType::I32);
            } else {
                ctx.emit(WasmInstr::Drop);
            }
        }

        IRInstr::Select { dst, cond, true_val, false_val } => {
            ctx.push_value(cond, None);
            ctx.push_value(true_val, None);
            ctx.push_value(false_val, None);
            ctx.emit(WasmInstr::Select);
            if let IRValue::Register(id) = dst {
                ctx.pop_to_vreg(*id, WasmType::I32);
            } else {
                ctx.emit(WasmInstr::Drop);
            }
        }

        IRInstr::Add { dst, lhs, rhs } => {
            ctx.push_value(lhs, None);
            ctx.push_value(rhs, None);
            ctx.emit(WasmInstr::I32Add);
            if let IRValue::Register(id) = dst {
                ctx.pop_to_vreg(*id, WasmType::I32);
            } else {
                ctx.emit(WasmInstr::Drop);
            }
        }

        IRInstr::Sub { dst, lhs, rhs } => {
            ctx.push_value(lhs, None);
            ctx.push_value(rhs, None);
            ctx.emit(WasmInstr::I32Sub);
            if let IRValue::Register(id) = dst {
                ctx.pop_to_vreg(*id, WasmType::I32);
            } else {
                ctx.emit(WasmInstr::Drop);
            }
        }

        IRInstr::Mul { dst, lhs, rhs } => {
            ctx.push_value(lhs, None);
            ctx.push_value(rhs, None);
            ctx.emit(WasmInstr::I32Mul);
            if let IRValue::Register(id) = dst {
                ctx.pop_to_vreg(*id, WasmType::I32);
            } else {
                ctx.emit(WasmInstr::Drop);
            }
        }

        IRInstr::Div { dst, lhs, rhs } => {
            ctx.push_value(lhs, None);
            ctx.push_value(rhs, None);
            ctx.emit(WasmInstr::I32DivS);
            if let IRValue::Register(id) = dst {
                ctx.pop_to_vreg(*id, WasmType::I32);
            } else {
                ctx.emit(WasmInstr::Drop);
            }
        }

        IRInstr::Cmp { kind, dst, lhs, rhs } => {
            ctx.push_value(lhs, None);
            ctx.push_value(rhs, None);
            let wasm_op = match kind {
                crate::ir::CmpKind::Eq => WasmInstr::I32Eq,
                crate::ir::CmpKind::Ne => WasmInstr::I32Ne,
                crate::ir::CmpKind::SLt => WasmInstr::I32LtS,
                crate::ir::CmpKind::SLe => WasmInstr::I32LeS,
                crate::ir::CmpKind::SGt => WasmInstr::I32GtS,
                crate::ir::CmpKind::SGe => WasmInstr::I32GeS,
                crate::ir::CmpKind::ULt => WasmInstr::I32LtU,
                crate::ir::CmpKind::ULe => WasmInstr::I32LeU,
                crate::ir::CmpKind::UGt => WasmInstr::I32GtU,
                crate::ir::CmpKind::UGe => WasmInstr::I32GeU,
            };
            ctx.emit(wasm_op);
            if let IRValue::Register(id) = dst {
                ctx.pop_to_vreg(*id, WasmType::I32);
            } else {
                ctx.emit(WasmInstr::Drop);
            }
        }

        IRInstr::Ret { values } => {
            for val in values {
                ctx.push_value(val, None);
            }
            ctx.emit(WasmInstr::Return);
        }

        IRInstr::Branch { target } => {
            if let Some(&depth) = ctx.block_labels.get(target) {
                ctx.emit(WasmInstr::Br(depth));
            }
        }

        IRInstr::CondBranch { cond, true_target, false_target } => {
            ctx.push_value(cond, None);
            if let Some(&true_depth) = ctx.block_labels.get(true_target) {
                ctx.emit(WasmInstr::BrIf(true_depth));
            }
            if let Some(&false_depth) = ctx.block_labels.get(false_target) {
                ctx.emit(WasmInstr::Br(false_depth));
            }
        }
    }
    Ok(())
}

/// Lower an IR terminator to Wasm control flow.
fn lower_terminator(
    term: &IRTerminator,
    ctx: &mut LoweringContext,
    _block_idx: usize,
) -> Result<(), BackendError> {
    match term {
        IRTerminator::Return(values) => {
            for val in values {
                ctx.push_value(val, None);
            }
            ctx.emit(WasmInstr::Return);
        }
        IRTerminator::Jump(target) => {
            // br to the target block label depth
            if let Some(&depth) = ctx.block_labels.get(target) {
                ctx.emit(WasmInstr::Br(depth));
            }
        }
        IRTerminator::Branch { cond, true_block, false_block } => {
            ctx.push_value(cond, None);
            // In Wasm, we use br_if for conditional branches
            // The structured control flow requires blocks for each branch target
            if let Some(&true_depth) = ctx.block_labels.get(true_block) {
                ctx.emit(WasmInstr::BrIf(true_depth));
            }
            if let Some(&false_depth) = ctx.block_labels.get(false_block) {
                ctx.emit(WasmInstr::Br(false_depth));
            }
        }
        IRTerminator::Unreachable => {
            ctx.emit(WasmInstr::Unreachable);
        }
        IRTerminator::Switch { discr, targets, default } => {
            ctx.push_value(discr, None);
            let labels: Vec<u32> = targets.iter()
                .filter_map(|(_, lbl)| ctx.block_labels.get(lbl).copied())
                .collect();
            let default_label = ctx.block_labels.get(default).copied().unwrap_or(0);
            ctx.emit(WasmInstr::BrTable { labels, default: default_label });
        }
        IRTerminator::Invoke { .. } | IRTerminator::TailCall { .. } | IRTerminator::Resume { .. } => {
            // These are lowered to Call instructions; terminators are handled at a higher level
        }
    }
    Ok(())
}

// ===========================================================================
// Wasm32Backend
// ===========================================================================

/// WebAssembly 32-bit code generation backend.
///
/// Wasm is a stack machine with no registers.  Virtual registers from the IR
/// are mapped to Wasm local variables.  The binary format is a structured
/// module with sections for types, functions, memory, etc.
pub struct Wasm32Backend {
    target_info: Wasm32TargetInfo,
}

impl Wasm32Backend {
    /// Create a new Wasm32 backend.
    pub fn new() -> Self {
        Self {
            target_info: Wasm32TargetInfo,
        }
    }
}

impl Default for Wasm32Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for Wasm32Backend {
    fn target_info(&self) -> &dyn crate::backend::TargetInfo {
        &self.target_info
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        // Wasm has no registers — map virtual regs to locals.
        // We lower the IR function to Wasm bytecode here.
        let (func_body, _func_type) = lower_function(func)
            .map_err(|e| BackendError::RegisterAllocFailed {
                isa: "wasm32",
                reason: e.to_string(),
            })?;

        // Build an AllocatedFunction from the lowered bytecode.
        // Each Wasm instruction is represented as an AllocatedInstruction
        // with its mnemonic and encoded bytes.
        let mut instructions = Vec::new();

        // Emit local declarations as pseudo-instructions
        for (count, ty) in &func_body.locals {
            let mut encoded = Vec::new();
            encoded.extend_from_slice(&encode_unsigned_leb128(*count as u64));
            encoded.push(ty.to_byte());
            instructions.push(AllocatedInstruction {
                opcode: format!("local_decl_{}", ty),
                reads: vec![],
                writes: vec![],
                encoded,
            });
        }

        // Disassemble the body bytes into per-instruction AllocatedInstructions
        // by re-parsing the bytecode we just emitted.  This gives us real
        // opcode mnemonics instead of a single opaque "wasm_body" blob.
        let disasm = self.disassemble(&func_body.body, 0);
        let mut offset = 0usize;
        for mnemonic in &disasm {
            // Determine how many bytes this instruction occupies by decoding
            // forward from the current offset.
            let start = offset;
            skip_one_instruction(&func_body.body, &mut offset);
            let instr_bytes = func_body.body[start..offset].to_vec();
            instructions.push(AllocatedInstruction {
                opcode: mnemonic.clone(),
                reads: vec![],
                writes: vec![],
                encoded: instr_bytes,
            });
        }

        let code_size: usize = instructions.iter().map(|i| i.encoded.len()).sum();

        Ok(AllocatedFunction {
            name: func.name.clone(),
            blocks: vec![AllocatedBlock {
                label: "entry".to_string(),
                instructions,
                code_offset: 0,
            }],
            frame_size: 0, // Wasm has no explicit frame
            callee_saved: vec![],
            spill_slots: 0,
            code_size,
            relocations: Vec::new(),
        })
    }

    fn encode_function(&self, func: &AllocatedFunction) -> Result<Vec<u8>, BackendError> {
        let mut bytes = Vec::new();
        for block in &func.blocks {
            for instr in &block.instructions {
                bytes.extend_from_slice(&instr.encoded);
            }
        }
        Ok(bytes)
    }

    fn encode_program(&self, program: &AllocatedProgram) -> Result<Vec<u8>, BackendError> {
        // Build a complete .wasm module from the allocated program.
        let mut module = WasmModuleBuilder::new();

        // Add a default memory (1 page minimum)
        module.add_memory(WasmLimits { min: 1, max: Some(256) });

        // For each function, add a type and function entry
        for func in &program.functions {
            // Create a default function type: () -> ()
            let type_idx = module.add_type(WasmFuncType {
                params: vec![],
                results: vec![],
            });
            let _func_idx = module.add_function(type_idx);

            // Encode the function body
            let mut body_bytes = Vec::new();
            for block in &func.blocks {
                for instr in &block.instructions {
                    body_bytes.extend_from_slice(&instr.encoded);
                }
            }
            // The body should already end with 0x0B (end), but ensure it
            if body_bytes.last() != Some(&0x0B) {
                body_bytes.push(0x0B);
            }

            module.add_code(WasmFuncBody {
                locals: vec![],
                body: body_bytes,
            });

            // Export the function
            module.add_export(WasmExport {
                name: func.name.clone(),
                kind: crate::wasm32::WasmExportKind::Function,
                index: _func_idx,
            });
        }

        // Add data sections as Wasm data segments
        // (This is handled at a higher level; for now, we skip it.)

        Ok(module.encode())
    }

    fn return_stub(&self) -> Vec<u8> {
        // Wasm end byte = 0x0B
        vec![0x0B]
    }

    fn trampoline(&self, _entry_addr: u64) -> Vec<u8> {
        // Wasm doesn't use trampolines — return empty
        vec![]
    }

    fn disassemble(&self, bytes: &[u8], addr: u64) -> Vec<String> {
        let mut lines = Vec::new();
        let mut offset = 0usize;
        let mut pc = addr;

        while offset < bytes.len() {
            let byte = bytes[offset];
            let start_offset = offset;

            // Handle multi-byte opcodes (0xFC = bulk memory, 0xFD = SIMD)
            let mnemonic = if byte == 0xFC && offset + 1 < bytes.len() {
                // Bulk memory / saturated arithmetic prefix
                let (subop, subop_size) = decode_unsigned_leb128(&bytes[offset + 1..]);
                offset += 1 + subop_size;
                let name = match subop {
                    0x08 => "memory.init".to_string(),
                    0x09 => "data.drop".to_string(),
                    0x0A => "memory.copy".to_string(),
                    0x0B => "memory.fill".to_string(),
                    0x0C => "memory.grow".to_string(),
                    0x0D => "memory.size".to_string(),
                    _ => format!("fc_subop_{:#04x}", subop),
                };
                // Skip remaining LEB128 operands for the sub-opcode
                match subop {
                    0x08 => { skip_leb128(bytes, &mut offset, 2); }  // data_idx, mem
                    0x0A => { skip_leb128(bytes, &mut offset, 2); }  // src, dst
                    0x0B => { skip_leb128(bytes, &mut offset, 1); }  // mem
                    _ => {}
                }
                name
            } else if byte == 0xFD && offset + 1 < bytes.len() {
                // SIMD prefix
                let (subop, subop_size) = decode_unsigned_leb128(&bytes[offset + 1..]);
                offset += 1 + subop_size;
                let name = match subop {
                    0x0C => "v128.const".to_string(),
                    0x0E => "i32x4.add".to_string(),
                    0x15 => "i32x4.mul".to_string(),
                    0x2C => "f32x4.add".to_string(),
                    0x35 => "f32x4.mul".to_string(),
                    _ => format!("simd_subop_{:#04x}", subop),
                };
                // Skip v128.const payload (16 bytes)
                if subop == 0x0C {
                    offset += 16;
                }
                name
            } else {
                let m = match byte {
                    0x00 => "unreachable".to_string(),
                    0x01 => "nop".to_string(),
                    0x02 => "block".to_string(),
                    0x03 => "loop".to_string(),
                    0x04 => "if".to_string(),
                    0x05 => "else".to_string(),
                    0x0B => "end".to_string(),
                    0x0C => "br".to_string(),
                    0x0D => "br_if".to_string(),
                    0x0E => "br_table".to_string(),
                    0x0F => "return".to_string(),
                    0x10 => "call".to_string(),
                    0x11 => "call_indirect".to_string(),
                    0x1A => "drop".to_string(),
                    0x1B => "select".to_string(),
                    0x20 => "local.get".to_string(),
                    0x21 => "local.set".to_string(),
                    0x22 => "local.tee".to_string(),
                    0x23 => "global.get".to_string(),
                    0x24 => "global.set".to_string(),
                    0x28 => "i32.load".to_string(),
                    0x29 => "i64.load".to_string(),
                    0x2A => "f32.load".to_string(),
                    0x2B => "f64.load".to_string(),
                    0x36 => "i32.store".to_string(),
                    0x37 => "i64.store".to_string(),
                    0x38 => "f32.store".to_string(),
                    0x39 => "f64.store".to_string(),
                    0x41 => "i32.const".to_string(),
                    0x42 => "i64.const".to_string(),
                    0x43 => "f32.const".to_string(),
                    0x44 => "f64.const".to_string(),
                    0x45 => "i32.eqz".to_string(),
                    0x46 => "i32.eq".to_string(),
                    0x47 => "i32.ne".to_string(),
                    0x48 => "i32.lt_s".to_string(),
                    0x49 => "i32.lt_u".to_string(),
                    0x4A => "i32.gt_s".to_string(),
                    0x4B => "i32.gt_u".to_string(),
                    0x4C => "i32.le_s".to_string(),
                    0x4D => "i32.le_u".to_string(),
                    0x4E => "i32.ge_s".to_string(),
                    0x4F => "i32.ge_u".to_string(),
                    0x6A => "i32.add".to_string(),
                    0x6B => "i32.sub".to_string(),
                    0x6C => "i32.mul".to_string(),
                    0x6D => "i32.div_s".to_string(),
                    0x6E => "i32.div_u".to_string(),
                    0x6F => "i32.rem_s".to_string(),
                    0x70 => "i32.rem_u".to_string(),
                    0x71 => "i32.and".to_string(),
                    0x72 => "i32.or".to_string(),
                    0x73 => "i32.xor".to_string(),
                    0x74 => "i32.shl".to_string(),
                    0x75 => "i32.shr_s".to_string(),
                    0x76 => "i32.shr_u".to_string(),
                    0x77 => "i32.rotl".to_string(),
                    0x78 => "i32.rotr".to_string(),
                    0x67 => "i32.clz".to_string(),
                    0x68 => "i32.ctz".to_string(),
                    0x69 => "i32.popcnt".to_string(),
                    0x50 => "i64.eqz".to_string(),
                    0x51 => "i64.eq".to_string(),
                    0x52 => "i64.ne".to_string(),
                    0x53 => "i64.lt_s".to_string(),
                    0x54 => "i64.lt_u".to_string(),
                    0x55 => "i64.gt_s".to_string(),
                    0x56 => "i64.gt_u".to_string(),
                    0x57 => "i64.le_s".to_string(),
                    0x58 => "i64.le_u".to_string(),
                    0x59 => "i64.ge_s".to_string(),
                    0x5A => "i64.ge_u".to_string(),
                    0x79 => "i64.clz".to_string(),
                    0x7A => "i64.ctz".to_string(),
                    0x7B => "i64.popcnt".to_string(),
                    0x7C => "i64.add".to_string(),
                    0x7D => "i64.sub".to_string(),
                    0x7E => "i64.mul".to_string(),
                    0x7F => "i64.div_s".to_string(),
                    0x80 => "i64.div_u".to_string(),
                    0x81 => "i64.rem_s".to_string(),
                    0x82 => "i64.rem_u".to_string(),
                    0x83 => "i64.and".to_string(),
                    0x84 => "i64.or".to_string(),
                    0x85 => "i64.xor".to_string(),
                    0x86 => "i64.shl".to_string(),
                    0x87 => "i64.shr_s".to_string(),
                    0x88 => "i64.shr_u".to_string(),
                    0x89 => "i64.rotl".to_string(),
                    0x8A => "i64.rotr".to_string(),
                    0x8C => "f32.neg".to_string(),
                    0x91 => "f32.sqrt".to_string(),
                    0x92 => "f32.add".to_string(),
                    0x93 => "f32.sub".to_string(),
                    0x94 => "f32.mul".to_string(),
                    0x95 => "f32.div".to_string(),
                    0x9A => "f64.neg".to_string(),
                    0x9F => "f64.sqrt".to_string(),
                    0xA0 => "f64.add".to_string(),
                    0xA1 => "f64.sub".to_string(),
                    0xA2 => "f64.mul".to_string(),
                    0xA3 => "f64.div".to_string(),
                    0xA7 => "i32.wrap_i64".to_string(),
                    0xAC => "i64.extend_i32_s".to_string(),
                    0xAD => "i64.extend_i32_u".to_string(),
                    0xB6 => "f32.demote_f64".to_string(),
                    0xBB => "f64.promote_f32".to_string(),
                    0xBC => "i32.reinterpret_f32".to_string(),
                    0xBD => "i64.reinterpret_f64".to_string(),
                    0xBE => "f32.reinterpret_i32".to_string(),
                    0xBF => "f64.reinterpret_i64".to_string(),
                    _ => format!("op_{:#04x}", byte),
                };
                offset += 1;

                // Skip LEB128 immediates for common patterns
                match byte {
                    0x20..=0x24 | 0x0C..=0x0D | 0x10 => {
                        skip_leb128(bytes, &mut offset, 1);
                    }
                    0x11 => {
                        skip_leb128(bytes, &mut offset, 2);
                    }
                    0x41 | 0x42 => {
                        skip_signed_leb128(bytes, &mut offset, 1);
                    }
                    0x28..=0x3E => {
                        skip_leb128(bytes, &mut offset, 2);
                    }
                    0x3F..=0x40 => {
                        skip_leb128(bytes, &mut offset, 1);
                    }
                    0x43 => { offset += 4; }  // f32.const: 4 bytes
                    0x44 => { offset += 8; }  // f64.const: 8 bytes
                    _ => {}
                }
                m
            };

            let consumed = offset - start_offset;
            let hex_bytes: Vec<String> = bytes[start_offset..offset]
                .iter().map(|b| format!("{:02x}", b)).collect();
            lines.push(format!("{:#010x}:  {:20}  {}", pc, hex_bytes.join(" "), mnemonic));
            pc += consumed as u64;
        }
        lines
    }

    fn name(&self) -> &'static str {
        "wasm32"
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ── LEB128 tests ──────────────────────────────────────────────

    #[test]
    fn test_unsigned_leb128_zero() {
        let encoded = encode_unsigned_leb128(0);
        assert_eq!(encoded, vec![0x00]);
        let (decoded, size) = decode_unsigned_leb128(&encoded);
        assert_eq!(decoded, 0);
        assert_eq!(size, 1);
    }

    #[test]
    fn test_unsigned_leb128_small() {
        let encoded = encode_unsigned_leb128(42);
        assert_eq!(encoded, vec![42]);
        let (decoded, size) = decode_unsigned_leb128(&encoded);
        assert_eq!(decoded, 42);
        assert_eq!(size, 1);
    }

    #[test]
    fn test_unsigned_leb128_max_single_byte() {
        let encoded = encode_unsigned_leb128(127);
        assert_eq!(encoded, vec![0x7F]);
        let (decoded, size) = decode_unsigned_leb128(&encoded);
        assert_eq!(decoded, 127);
        assert_eq!(size, 1);
    }

    #[test]
    fn test_unsigned_leb128_two_bytes() {
        let encoded = encode_unsigned_leb128(128);
        assert_eq!(encoded, vec![0x80, 0x01]);
        let (decoded, size) = decode_unsigned_leb128(&encoded);
        assert_eq!(decoded, 128);
        assert_eq!(size, 2);
    }

    #[test]
    fn test_unsigned_leb128_roundtrip_large() {
        let values = vec![0, 1, 127, 128, 255, 256, 16383, 16384, 624485, u32::MAX as u64, u64::MAX];
        for val in values {
            let encoded = encode_unsigned_leb128(val);
            let (decoded, size) = decode_unsigned_leb128(&encoded);
            assert_eq!(decoded, val, "Round-trip failed for {}", val);
            assert_eq!(size, encoded.len(), "Size mismatch for {}", val);
        }
    }

    #[test]
    fn test_signed_leb128_zero() {
        let encoded = encode_signed_leb128(0);
        assert_eq!(encoded, vec![0x00]);
        let (decoded, size) = decode_signed_leb128(&encoded);
        assert_eq!(decoded, 0);
        assert_eq!(size, 1);
    }

    #[test]
    fn test_signed_leb128_positive() {
        let encoded = encode_signed_leb128(42);
        assert_eq!(encoded, vec![42]);
        let (decoded, size) = decode_signed_leb128(&encoded);
        assert_eq!(decoded, 42);
        assert_eq!(size, 1);
    }

    #[test]
    fn test_signed_leb128_negative() {
        let encoded = encode_signed_leb128(-1);
        assert_eq!(encoded, vec![0x7F]);
        let (decoded, size) = decode_signed_leb128(&encoded);
        assert_eq!(decoded, -1);
        assert_eq!(size, 1);
    }

    #[test]
    fn test_signed_leb128_roundtrip() {
        let values = vec![0i64, 1, -1, 63, -64, 64, -65, 127, -128, 8192, -8193, i32::MAX as i64, i32::MIN as i64, i64::MIN, i64::MAX];
        for val in values {
            let encoded = encode_signed_leb128(val);
            let (decoded, size) = decode_signed_leb128(&encoded);
            assert_eq!(decoded, val, "Signed round-trip failed for {}", val);
            assert_eq!(size, encoded.len(), "Signed size mismatch for {}", val);
        }
    }

    // ── WasmType tests ────────────────────────────────────────────

    #[test]
    fn test_wasm_type_bytes() {
        assert_eq!(WasmType::I32.to_byte(), 0x7F);
        assert_eq!(WasmType::I64.to_byte(), 0x7E);
        assert_eq!(WasmType::F32.to_byte(), 0x7D);
        assert_eq!(WasmType::F64.to_byte(), 0x7C);
    }

    #[test]
    fn test_wasm_type_from_ir() {
        assert_eq!(WasmType::from_ir_type(&IRType::I32), Some(WasmType::I32));
        assert_eq!(WasmType::from_ir_type(&IRType::I64), Some(WasmType::I64));
        assert_eq!(WasmType::from_ir_type(&IRType::F32), Some(WasmType::F32));
        assert_eq!(WasmType::from_ir_type(&IRType::F64), Some(WasmType::F64));
        assert_eq!(WasmType::from_ir_type(&IRType::Ptr), Some(WasmType::I32)); // wasm32
        assert_eq!(WasmType::from_ir_type(&IRType::Void), None);
    }

    // ── Wasm instruction encoding tests ───────────────────────────

    #[test]
    fn test_i32_const_encoding() {
        let instr = WasmInstr::I32Const(42);
        let bytes = instr.to_bytes();
        assert_eq!(bytes[0], 0x41); // i32.const opcode
        assert_eq!(bytes[1], 42);   // LEB128 of 42
    }

    #[test]
    fn test_i32_add_encoding() {
        let instr = WasmInstr::I32Add;
        let bytes = instr.to_bytes();
        assert_eq!(bytes, vec![0x6A]);
    }

    #[test]
    fn test_local_get_encoding() {
        let instr = WasmInstr::LocalGet(3);
        let bytes = instr.to_bytes();
        assert_eq!(bytes[0], 0x20);
        assert_eq!(bytes[1], 3);
    }

    #[test]
    fn test_i32_load_encoding() {
        let instr = WasmInstr::I32Load { align: 2, offset: 0 };
        let bytes = instr.to_bytes();
        assert_eq!(bytes[0], 0x28);
        assert_eq!(bytes[1], 2); // align
        assert_eq!(bytes[2], 0); // offset
    }

    #[test]
    fn test_br_encoding() {
        let instr = WasmInstr::Br(1);
        let bytes = instr.to_bytes();
        assert_eq!(bytes[0], 0x0C);
        assert_eq!(bytes[1], 1);
    }

    #[test]
    fn test_call_encoding() {
        let instr = WasmInstr::Call(5);
        let bytes = instr.to_bytes();
        assert_eq!(bytes[0], 0x10);
        assert_eq!(bytes[1], 5);
    }

    #[test]
    fn test_conversion_instructions() {
        // Verify all conversion instructions encode to their correct opcodes
        assert_eq!(WasmInstr::I32WrapI64.to_bytes(), vec![0xA7]);
        assert_eq!(WasmInstr::I64ExtendI32S.to_bytes(), vec![0xAC]);
        assert_eq!(WasmInstr::I64ExtendI32U.to_bytes(), vec![0xAD]);
        assert_eq!(WasmInstr::F32DemoteF64.to_bytes(), vec![0xB6]);
        assert_eq!(WasmInstr::F64PromoteF32.to_bytes(), vec![0xBB]);
        assert_eq!(WasmInstr::I32TruncF32S.to_bytes(), vec![0xA8]);
        assert_eq!(WasmInstr::I32TruncF64S.to_bytes(), vec![0xA9]);
        assert_eq!(WasmInstr::I64TruncF32S.to_bytes(), vec![0xAE]);
        assert_eq!(WasmInstr::I64TruncF64S.to_bytes(), vec![0xAF]);
        assert_eq!(WasmInstr::F32ConvertI32S.to_bytes(), vec![0xB2]);
        assert_eq!(WasmInstr::F32ConvertI64S.to_bytes(), vec![0xB3]);
        assert_eq!(WasmInstr::F64ConvertI32S.to_bytes(), vec![0xB7]);
        assert_eq!(WasmInstr::F64ConvertI64S.to_bytes(), vec![0xB8]);
        assert_eq!(WasmInstr::I32ReinterpretF32.to_bytes(), vec![0xBC]);
        assert_eq!(WasmInstr::I64ReinterpretF64.to_bytes(), vec![0xBD]);
        assert_eq!(WasmInstr::F32ReinterpretI32.to_bytes(), vec![0xBE]);
        assert_eq!(WasmInstr::F64ReinterpretI64.to_bytes(), vec![0xBF]);
    }

    // ── Wasm section structure tests ──────────────────────────────

    #[test]
    fn test_wasm_module_magic_and_version() {
        let module = WasmModuleBuilder::new().encode();
        assert_eq!(&module[0..4], &WASM_MAGIC);
        assert_eq!(&module[4..8], &WASM_VERSION);
    }

    #[test]
    fn test_wasm_type_section() {
        let mut builder = WasmModuleBuilder::new();
        builder.add_type(WasmFuncType {
            params: vec![WasmType::I32],
            results: vec![WasmType::I32],
        });
        let module = builder.encode();
        // After magic (4) + version (4), the first section is the type section
        assert_eq!(module[8], SECTION_TYPE);
    }

    #[test]
    fn test_wasm_memory_section() {
        let mut builder = WasmModuleBuilder::new();
        builder.add_memory(WasmLimits { min: 1, max: Some(256) });
        let module = builder.encode();
        // Verify the memory section is present
        let mut offset = 8; // skip magic + version
        while offset < module.len() {
            let section_id = module[offset];
            offset += 1;
            let (size, size_len) = decode_unsigned_leb128(&module[offset..]);
            offset += size_len;
            if section_id == SECTION_MEMORY {
                return; // Found memory section
            }
            offset += size as usize;
        }
        panic!("Memory section not found");
    }

    // ── Complete .wasm module generation test ──────────────────────

    #[test]
    fn test_complete_wasm_module() {
        let mut builder = WasmModuleBuilder::new();

        // Add a function type: () -> i32
        let type_idx = builder.add_type(WasmFuncType {
            params: vec![],
            results: vec![WasmType::I32],
        });

        // Add memory
        builder.add_memory(WasmLimits { min: 1, max: Some(256) });

        // Add a function
        let func_idx = builder.add_function(type_idx);

        // Add function body: i32.const 42, end
        let mut body_bytes = Vec::new();
        WasmInstr::I32Const(42).encode(&mut body_bytes);
        body_bytes.push(0x0B); // end
        builder.add_code(WasmFuncBody {
            locals: vec![],
            body: body_bytes,
        });

        // Export the function
        builder.add_export(WasmExport {
            name: "main".to_string(),
            kind: WasmExportKind::Function,
            index: func_idx,
        });

        let module = builder.encode();

        // Verify magic + version
        assert_eq!(&module[0..4], &WASM_MAGIC);
        assert_eq!(&module[4..8], &WASM_VERSION);
        assert!(module.len() > 8);
    }

    // ── Stack discipline verification ──────────────────────────────

    #[test]
    fn test_stack_discipline_i32_add() {
        // i32.add pops 2 values, pushes 1
        let instr = WasmInstr::I32Add;
        let bytes = instr.to_bytes();
        assert_eq!(bytes.len(), 1);
        assert_eq!(bytes[0], 0x6A);
    }

    #[test]
    fn test_stack_discipline_select() {
        // select pops 3 (val1, val2, cond), pushes 1
        let instr = WasmInstr::Select;
        let bytes = instr.to_bytes();
        assert_eq!(bytes, vec![0x1B]);
    }

    #[test]
    fn test_stack_discipline_drop() {
        // drop pops 1, pushes 0
        let instr = WasmInstr::Drop;
        let bytes = instr.to_bytes();
        assert_eq!(bytes, vec![0x1A]);
    }

    // ── Backend trait dispatch test ────────────────────────────────

    #[test]
    fn test_wasm32_backend_trait_dispatch() {
        let backend: Box<dyn Backend> = Box::new(Wasm32Backend::new());
        assert_eq!(backend.name(), "wasm32");
        assert_eq!(backend.target_info().isa_name(), "wasm32");
        assert!(!backend.target_info().has_registers());
        assert_eq!(backend.target_info().pointer_width(), 4);
    }

    // ── return_stub test ──────────────────────────────────────────

    #[test]
    fn test_return_stub() {
        let backend = Wasm32Backend::new();
        let stub = backend.return_stub();
        assert_eq!(stub, vec![0x0B]); // Wasm end byte
    }

    // ── Trampoline test ───────────────────────────────────────────

    #[test]
    fn test_trampoline_empty() {
        let backend = Wasm32Backend::new();
        let tramp = backend.trampoline(0x1000);
        assert!(tramp.is_empty());
    }

    // ── WasmFuncType encoding test ────────────────────────────────

    #[test]
    fn test_func_type_encoding() {
        let ft = WasmFuncType {
            params: vec![WasmType::I32, WasmType::F64],
            results: vec![WasmType::I32],
        };
        let encoded = ft.encode();
        assert_eq!(encoded[0], 0x60); // func type tag
        // 2 params
        let (count, size) = decode_unsigned_leb128(&encoded[1..]);
        assert_eq!(count, 2);
        assert_eq!(encoded[1 + size], WasmType::I32.to_byte());
        assert_eq!(encoded[2 + size], WasmType::F64.to_byte());
    }

    // ── Disassembler test ─────────────────────────────────────────

    #[test]
    fn test_disassemble_i32_const() {
        let backend = Wasm32Backend::new();
        let mut bytes = Vec::new();
        WasmInstr::I32Const(42).encode(&mut bytes);
        let lines = backend.disassemble(&bytes, 0);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("i32.const"));
    }

    #[test]
    fn test_disassemble_i32_add() {
        let backend = Wasm32Backend::new();
        let bytes = vec![0x6A]; // i32.add
        let lines = backend.disassemble(&bytes, 0);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("i32.add"));
    }

    // ── All i32 opcodes test ──────────────────────────────────────

    #[test]
    fn test_all_i32_comparison_opcodes() {
        let opcodes = vec![
            (WasmInstr::I32Eqz, 0x45),
            (WasmInstr::I32Eq, 0x46),
            (WasmInstr::I32Ne, 0x47),
            (WasmInstr::I32LtS, 0x48),
            (WasmInstr::I32LtU, 0x49),
            (WasmInstr::I32GtS, 0x4A),
            (WasmInstr::I32GtU, 0x4B),
            (WasmInstr::I32LeS, 0x4C),
            (WasmInstr::I32LeU, 0x4D),
            (WasmInstr::I32GeS, 0x4E),
            (WasmInstr::I32GeU, 0x4F),
        ];
        for (instr, expected_opcode) in opcodes {
            assert_eq!(instr.to_bytes(), vec![expected_opcode],
                "Opcode mismatch for {:?}", instr);
        }
    }

    // ── Encode program test ───────────────────────────────────────

    #[test]
    fn test_encode_program() {
        let backend = Wasm32Backend::new();
        let program = AllocatedProgram {
            functions: vec![AllocatedFunction {
                name: "test_func".to_string(),
                blocks: vec![AllocatedBlock {
                    label: "entry".to_string(),
                    instructions: vec![AllocatedInstruction {
                        opcode: "i32.const_42".to_string(),
                        reads: vec![],
                        writes: vec![],
                        encoded: {
                            let mut b = Vec::new();
                            WasmInstr::I32Const(42).encode(&mut b);
                            b.push(0x0B); // end
                            b
                        },
                    }],
                    code_offset: 0,
                }],
                frame_size: 0,
                callee_saved: vec![],
                spill_slots: 0,
                code_size: 3,
                relocations: Vec::new(),
            }],
            total_code_size: 3,
            total_data_size: 0,
        };
        let result = backend.encode_program(&program);
        assert!(result.is_ok());
        let wasm_bytes = result.unwrap();
        // Must start with Wasm magic
        assert_eq!(&wasm_bytes[0..4], &[0x00, 0x61, 0x73, 0x6D]);
    }

    // ── SIMD v128 Tests ─────────────────────────────────────────────

    #[test]
    fn test_v128_const_encoding() {
        let val = [0x01u8; 16];
        let instr = WasmInstr::V128Const(val);
        let bytes = instr.to_bytes();
        assert_eq!(bytes[0], 0xFD); // SIMD prefix
        // The sub-opcode is LEB128(0x0C) = 0x0C
        assert_eq!(bytes[1], 0x0C);
        // 16 bytes of data follow
        assert_eq!(&bytes[2..18], &[0x01u8; 16]);
    }

    #[test]
    fn test_i32x4_add_encoding() {
        let bytes = WasmInstr::I32X4Add.to_bytes();
        assert_eq!(bytes[0], 0xFD); // SIMD prefix
        // LEB128(0x0E)
        assert_eq!(bytes[1], 0x0E);
    }

    #[test]
    fn test_f32x4_mul_encoding() {
        let bytes = WasmInstr::F32X4Mul.to_bytes();
        assert_eq!(bytes[0], 0xFD); // SIMD prefix
        // LEB128(0x35)
        assert_eq!(bytes[1], 0x35);
    }

    // ── Bulk Memory Tests ────────────────────────────────────────────

    #[test]
    fn test_memory_copy_encoding() {
        let bytes = WasmInstr::MemoryCopy { src_mem: 0, dst_mem: 0 }.to_bytes();
        assert_eq!(bytes[0], 0xFC); // multi-byte op prefix
        // LEB128(0x0A) for memory.copy
        assert_eq!(bytes[1], 0x0A);
    }

    #[test]
    fn test_memory_fill_encoding() {
        let bytes = WasmInstr::MemoryFill { mem: 0 }.to_bytes();
        assert_eq!(bytes[0], 0xFC);
        assert_eq!(bytes[1], 0x0B); // memory.fill
    }

    #[test]
    fn test_memory_init_encoding() {
        let bytes = WasmInstr::MemoryInit { data_idx: 0, mem: 0 }.to_bytes();
        assert_eq!(bytes[0], 0xFC);
        assert_eq!(bytes[1], 0x08); // memory.init
    }

    // ── WASI Import Tests ────────────────────────────────────────────

    #[test]
    fn test_wasi_fd_write_import() {
        let imp = WasmImport::wasi_fd_write(0);
        assert_eq!(imp.module, "wasi_snapshot_preview1");
        assert_eq!(imp.name, "fd_write");
    }

    #[test]
    fn test_wasi_proc_exit_import() {
        let imp = WasmImport::wasi_proc_exit(1);
        assert_eq!(imp.module, "wasi_snapshot_preview1");
        assert_eq!(imp.name, "proc_exit");
    }

    // ── Multi-value Return / WasmFuncType Tests ─────────────────────

    #[test]
    fn test_multi_value_func_type() {
        let ft = WasmFuncType {
            params: vec![WasmType::I32, WasmType::I64],
            results: vec![WasmType::I32, WasmType::I64], // multi-value return
        };
        let encoded = ft.encode();
        assert_eq!(encoded[0], 0x60); // func type tag
        // 2 params + 2 results should encode correctly
        let decoded_params = decode_unsigned_leb128(&encoded[1..]);
        assert_eq!(decoded_params.0, 2);
    }

    // ── Disassemble SIMD and Bulk Memory ────────────────────────────

    #[test]
    fn test_disassemble_simd_i32x4_add() {
        let backend = Wasm32Backend::new();
        let bytes = WasmInstr::I32X4Add.to_bytes();
        let lines = backend.disassemble(&bytes, 0);
        assert!(lines[0].contains("i32x4.add"));
    }

    #[test]
    fn test_disassemble_memory_copy() {
        let backend = Wasm32Backend::new();
        let bytes = WasmInstr::MemoryCopy { src_mem: 0, dst_mem: 0 }.to_bytes();
        let lines = backend.disassemble(&bytes, 0);
        assert!(lines[0].contains("memory.copy"));
    }

    // ── Real Instruction Selection (ISel) Tests ───────────────────────

    /// Helper: build a minimal IRFunction with a single block that contains
    /// the given instructions and a Return terminator.
    fn make_simple_func(name: &str, instrs: Vec<IRInstr>) -> IRFunction {
        let mut func = IRFunction::new(name);
        func.blocks[0].instructions = instrs;
        func.blocks[0].terminator = IRTerminator::Return(vec![]);
        func
    }

    #[test]
    fn test_isel_i32_add_produces_real_opcode() {
        // IR: %1 = add %0, 42
        let mut func = make_simple_func("add_test", vec![
            IRInstr::Add {
                dst: IRValue::Register(1),
                lhs: IRValue::Register(0),
                rhs: IRValue::Immediate(42),
            },
        ]);
        func.param_types.push(IRType::I32);
        func.params.push(IRValue::Register(0));

        let backend = Wasm32Backend::new();
        let result = backend.allocate_registers(&func);
        assert!(result.is_ok(), "allocate_registers failed: {:?}", result.err());
        let alloc = result.unwrap();

        // The allocated instructions must contain i32.add (opcode 0x6A),
        // not a NOP (0x01) or a generic "wasm_body" opcode.
        let opcodes: Vec<&str> = alloc.blocks[0].instructions.iter()
            .map(|i| i.opcode.as_str())
            .collect();
        assert!(opcodes.iter().any(|o| o.contains("i32.add")),
            "Expected i32.add in opcodes, got: {:?}", opcodes);
        // Verify the encoded bytes for the add instruction contain 0x6A
        let add_instr = alloc.blocks[0].instructions.iter()
            .find(|i| i.opcode.contains("i32.add"))
            .expect("should find i32.add instruction");
        assert!(add_instr.encoded.contains(&0x6A),
            "i32.add encoded bytes should contain 0x6A, got: {:02x?}", add_instr.encoded);
    }

    #[test]
    fn test_isel_i32_sub_produces_real_opcode() {
        // IR: %1 = sub %0, 10
        let mut func = make_simple_func("sub_test", vec![
            IRInstr::Sub {
                dst: IRValue::Register(1),
                lhs: IRValue::Register(0),
                rhs: IRValue::Immediate(10),
            },
        ]);
        func.param_types.push(IRType::I32);
        func.params.push(IRValue::Register(0));

        let backend = Wasm32Backend::new();
        let alloc = backend.allocate_registers(&func).unwrap();
        let opcodes: Vec<&str> = alloc.blocks[0].instructions.iter()
            .map(|i| i.opcode.as_str())
            .collect();
        assert!(opcodes.iter().any(|o| o.contains("i32.sub")),
            "Expected i32.sub in opcodes, got: {:?}", opcodes);
    }

    #[test]
    fn test_isel_cmp_eq_produces_real_opcode() {
        // IR: %1 = cmp.eq %0, %0
        let mut func = make_simple_func("cmp_test", vec![
            IRInstr::Cmp {
                kind: crate::ir::CmpKind::Eq,
                dst: IRValue::Register(1),
                lhs: IRValue::Register(0),
                rhs: IRValue::Register(0),
            },
        ]);
        func.param_types.push(IRType::I32);
        func.params.push(IRValue::Register(0));

        let backend = Wasm32Backend::new();
        let alloc = backend.allocate_registers(&func).unwrap();
        let opcodes: Vec<&str> = alloc.blocks[0].instructions.iter()
            .map(|i| i.opcode.as_str())
            .collect();
        assert!(opcodes.iter().any(|o| o.contains("i32.eq")),
            "Expected i32.eq in opcodes, got: {:?}", opcodes);
    }

    #[test]
    fn test_isel_load_store_produces_real_opcodes() {
        // IR: %1 = load %0; store %1, %0
        let mut func = make_simple_func("ldst_test", vec![
            IRInstr::Load {
                dst: IRValue::Register(1),
                addr: IRValue::Register(0),
            },
            IRInstr::Store {
                value: IRValue::Register(1),
                addr: IRValue::Register(0),
            },
        ]);
        func.param_types.push(IRType::I32);
        func.params.push(IRValue::Register(0));

        let backend = Wasm32Backend::new();
        let alloc = backend.allocate_registers(&func).unwrap();
        let opcodes: Vec<&str> = alloc.blocks[0].instructions.iter()
            .map(|i| i.opcode.as_str())
            .collect();
        assert!(opcodes.iter().any(|o| o.contains("i32.load")),
            "Expected i32.load in opcodes, got: {:?}", opcodes);
        assert!(opcodes.iter().any(|o| o.contains("i32.store")),
            "Expected i32.store in opcodes, got: {:?}", opcodes);
    }

    #[test]
    fn test_isel_return_produces_real_opcode() {
        // IR: just return
        let func = make_simple_func("ret_test", vec![]);

        let backend = Wasm32Backend::new();
        let alloc = backend.allocate_registers(&func).unwrap();
        let opcodes: Vec<&str> = alloc.blocks[0].instructions.iter()
            .map(|i| i.opcode.as_str())
            .collect();
        assert!(opcodes.iter().any(|o| o.contains("return")),
            "Expected return in opcodes, got: {:?}", opcodes);
    }

    #[test]
    fn test_isel_binop_i32_mul_produces_real_opcode() {
        // IR: %1 = BinOp(Mul) %0, 3
        let mut func = make_simple_func("mul_test", vec![
            IRInstr::BinOp {
                op: BinOpKind::Mul,
                dst: IRValue::Register(1),
                lhs: IRValue::Register(0),
                rhs: IRValue::Immediate(3),
            },
        ]);
        func.param_types.push(IRType::I32);
        func.params.push(IRValue::Register(0));

        let backend = Wasm32Backend::new();
        let alloc = backend.allocate_registers(&func).unwrap();
        let mul_instr = alloc.blocks[0].instructions.iter()
            .find(|i| i.opcode.contains("i32.mul"));
        assert!(mul_instr.is_some(), "Expected i32.mul instruction");
        let instr = mul_instr.unwrap();
        assert!(instr.encoded.contains(&0x6C),
            "i32.mul encoded bytes should contain 0x6C, got: {:02x?}", instr.encoded);
    }

    #[test]
    fn test_isel_binop_sdiv_produces_i32_div_s() {
        // IR: %1 = BinOp(SDiv) %0, 2
        let mut func = make_simple_func("sdiv_test", vec![
            IRInstr::BinOp {
                op: BinOpKind::SDiv,
                dst: IRValue::Register(1),
                lhs: IRValue::Register(0),
                rhs: IRValue::Immediate(2),
            },
        ]);
        func.param_types.push(IRType::I32);
        func.params.push(IRValue::Register(0));

        let backend = Wasm32Backend::new();
        let alloc = backend.allocate_registers(&func).unwrap();
        let opcodes: Vec<&str> = alloc.blocks[0].instructions.iter()
            .map(|i| i.opcode.as_str())
            .collect();
        assert!(opcodes.iter().any(|o| o.contains("i32.div_s")),
            "Expected i32.div_s in opcodes, got: {:?}", opcodes);
    }

    #[test]
    fn test_isel_unreachable_produces_unreachable_not_nop() {
        // IR: unreachable terminator
        let mut func = IRFunction::new("unreachable_test");
        func.blocks[0].terminator = IRTerminator::Unreachable;

        let backend = Wasm32Backend::new();
        let alloc = backend.allocate_registers(&func).unwrap();

        // Must contain "unreachable" (0x00), NOT "nop" (0x01)
        let opcodes: Vec<&str> = alloc.blocks[0].instructions.iter()
            .map(|i| i.opcode.as_str())
            .collect();
        assert!(opcodes.iter().any(|o| o.contains("unreachable")),
            "Expected unreachable in opcodes, got: {:?}", opcodes);
        assert!(!opcodes.iter().any(|o| *o == "nop"),
            "Should not have nop opcode for unreachable terminator, got: {:?}", opcodes);
    }

    #[test]
    fn test_isel_f32_neg_encoding() {
        // Verify the f32.neg instruction encodes to opcode 0x8C
        let bytes = WasmInstr::F32Neg.to_bytes();
        assert_eq!(bytes, vec![0x8C]);
    }

    #[test]
    fn test_isel_f64_neg_encoding() {
        // Verify the f64.neg instruction encodes to opcode 0x9A
        let bytes = WasmInstr::F64Neg.to_bytes();
        assert_eq!(bytes, vec![0x9A]);
    }

    #[test]
    fn test_isel_unreachable_encoding() {
        // Verify the unreachable instruction encodes to opcode 0x00
        let bytes = WasmInstr::Unreachable.to_bytes();
        assert_eq!(bytes, vec![0x00]);
    }

    #[test]
    fn test_isel_cast_bitcast_uses_reinterpret() {
        // BitCast of i32 → f32 should use f32.reinterpret_i32 (0xBE), not Nop (0x01)
        let mut func = make_simple_func("bitcast_test", vec![
            IRInstr::Cast {
                kind: CastKind::BitCast,
                dst: IRValue::Register(1),
                src: IRValue::Register(0),
            },
        ]);
        func.param_types.push(IRType::I32);
        func.params.push(IRValue::Register(0));

        let backend = Wasm32Backend::new();
        let alloc = backend.allocate_registers(&func).unwrap();
        let opcodes: Vec<&str> = alloc.blocks[0].instructions.iter()
            .map(|i| i.opcode.as_str())
            .collect();
        assert!(opcodes.iter().any(|o| o.contains("reinterpret")),
            "Expected reinterpret for BitCast, got: {:?}", opcodes);
        assert!(!opcodes.iter().any(|o| *o == "nop"),
            "BitCast should not produce nop, got: {:?}", opcodes);
    }

    #[test]
    fn test_isel_allocate_registers_per_instruction_opcodes() {
        // Verify that allocate_registers produces per-instruction entries
        // with real opcode names (not a single "wasm_body" blob).
        let func = make_simple_func("isel_test", vec![
            IRInstr::Add {
                dst: IRValue::Register(1),
                lhs: IRValue::Immediate(10),
                rhs: IRValue::Immediate(20),
            },
        ]);

        let backend = Wasm32Backend::new();
        let alloc = backend.allocate_registers(&func).unwrap();

        // Should NOT have a "wasm_body" opcode
        let opcodes: Vec<&str> = alloc.blocks[0].instructions.iter()
            .map(|i| i.opcode.as_str())
            .collect();
        assert!(!opcodes.iter().any(|o| *o == "wasm_body"),
            "Should not have wasm_body opcode, got: {:?}", opcodes);
        // Should have specific instruction opcodes
        assert!(opcodes.iter().any(|o| o.contains("i32.const")),
            "Expected i32.const in opcodes, got: {:?}", opcodes);
        assert!(opcodes.iter().any(|o| o.contains("i32.add")),
            "Expected i32.add in opcodes, got: {:?}", opcodes);
    }

    // ── Disassembler Tests ──────────────────────────────────────────────

    #[test]
    fn test_wasm32_disassemble_nop() {
        let backend = Wasm32Backend::new();
        let bytes = WasmInstr::Nop.to_bytes();
        let lines = backend.disassemble(&bytes, 0);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("nop"), "Expected nop, got: {}", lines[0]);
    }

    #[test]
    fn test_wasm32_disassemble_i32_const() {
        let backend = Wasm32Backend::new();
        let bytes = WasmInstr::I32Const(42).to_bytes();
        let lines = backend.disassemble(&bytes, 0);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("i32.const"), "Expected i32.const, got: {}", lines[0]);
    }

    #[test]
    fn test_wasm32_disassemble_add_sub() {
        let backend = Wasm32Backend::new();
        let mut bytes = Vec::new();
        WasmInstr::I32Add.encode(&mut bytes);
        WasmInstr::I32Sub.encode(&mut bytes);
        let lines = backend.disassemble(&bytes, 0);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("i32.add"), "Expected i32.add, got: {}", lines[0]);
        assert!(lines[1].contains("i32.sub"), "Expected i32.sub, got: {}", lines[1]);
    }
}
pub mod disasm;
