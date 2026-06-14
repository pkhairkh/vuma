//! # Wasm32 Mnemonic Disassembler
//!
//! Decodes WebAssembly bytecode into `WasmInstr` instances and provides
//! `Display` for human-readable mnemonic output. Covers the core instruction
//! set needed by the VUMA ISel lowering.

use super::WasmInstr;
use super::WasmType;
use super::{decode_signed_leb128, decode_unsigned_leb128};

// ---------------------------------------------------------------------------
// Decode error
// ---------------------------------------------------------------------------

/// Error produced when wasm decoding fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The byte slice is too short.
    Truncated { needed: usize, available: usize },
    /// The byte sequence is not a recognised wasm opcode.
    UnknownOpcode { opcode: u8 },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Truncated { needed, available } => {
                write!(f, "truncated: need {needed} bytes, have {available}")
            }
            DecodeError::UnknownOpcode { opcode } => {
                write!(f, "unknown wasm opcode: 0x{opcode:02x}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

// ---------------------------------------------------------------------------
// Display for WasmInstr
// ---------------------------------------------------------------------------

impl std::fmt::Display for WasmInstr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Control
            WasmInstr::Block(ty) => write!(f, "block {}", ty.map_or("void".to_string(), |t| t.to_string())),
            WasmInstr::Loop(ty) => write!(f, "loop {}", ty.map_or("void".to_string(), |t| t.to_string())),
            WasmInstr::If(ty) => write!(f, "if {}", ty.map_or("void".to_string(), |t| t.to_string())),
            WasmInstr::Else => write!(f, "else"),
            WasmInstr::End => write!(f, "end"),
            WasmInstr::Br(idx) => write!(f, "br {idx}"),
            WasmInstr::BrIf(idx) => write!(f, "br_if {idx}"),
            WasmInstr::BrTable { labels, default } => {
                write!(f, "br_table")?;
                for lbl in labels {
                    write!(f, " {lbl}")?;
                }
                write!(f, " {default}")
            }
            WasmInstr::Return => write!(f, "return"),
            WasmInstr::Call(idx) => write!(f, "call {idx}"),
            WasmInstr::CallIndirect {
                type_idx,
                table_idx,
            } => {
                write!(f, "call_indirect {type_idx} {table_idx}")
            }

            // Parametric
            WasmInstr::Select => write!(f, "select"),
            WasmInstr::Drop => write!(f, "drop"),

            // Variable
            WasmInstr::LocalGet(idx) => write!(f, "local.get {idx}"),
            WasmInstr::LocalSet(idx) => write!(f, "local.set {idx}"),
            WasmInstr::LocalTee(idx) => write!(f, "local.tee {idx}"),
            WasmInstr::GlobalGet(idx) => write!(f, "global.get {idx}"),
            WasmInstr::GlobalSet(idx) => write!(f, "global.set {idx}"),

            // Memory loads
            WasmInstr::I32Load { align, offset } => {
                write!(f, "i32.load align={align} offset={offset}")
            }
            WasmInstr::I64Load { align, offset } => {
                write!(f, "i64.load align={align} offset={offset}")
            }
            WasmInstr::F32Load { align, offset } => {
                write!(f, "f32.load align={align} offset={offset}")
            }
            WasmInstr::F64Load { align, offset } => {
                write!(f, "f64.load align={align} offset={offset}")
            }
            WasmInstr::I32Load8S { align, offset } => {
                write!(f, "i32.load8_s align={align} offset={offset}")
            }
            WasmInstr::I32Load8U { align, offset } => {
                write!(f, "i32.load8_u align={align} offset={offset}")
            }
            WasmInstr::I32Load16S { align, offset } => {
                write!(f, "i32.load16_s align={align} offset={offset}")
            }
            WasmInstr::I32Load16U { align, offset } => {
                write!(f, "i32.load16_u align={align} offset={offset}")
            }
            WasmInstr::I64Load8S { align, offset } => {
                write!(f, "i64.load8_s align={align} offset={offset}")
            }
            WasmInstr::I64Load8U { align, offset } => {
                write!(f, "i64.load8_u align={align} offset={offset}")
            }
            WasmInstr::I64Load16S { align, offset } => {
                write!(f, "i64.load16_s align={align} offset={offset}")
            }
            WasmInstr::I64Load16U { align, offset } => {
                write!(f, "i64.load16_u align={align} offset={offset}")
            }
            WasmInstr::I64Load32S { align, offset } => {
                write!(f, "i64.load32_s align={align} offset={offset}")
            }
            WasmInstr::I64Load32U { align, offset } => {
                write!(f, "i64.load32_u align={align} offset={offset}")
            }

            // Memory stores
            WasmInstr::I32Store { align, offset } => {
                write!(f, "i32.store align={align} offset={offset}")
            }
            WasmInstr::I64Store { align, offset } => {
                write!(f, "i64.store align={align} offset={offset}")
            }
            WasmInstr::F32Store { align, offset } => {
                write!(f, "f32.store align={align} offset={offset}")
            }
            WasmInstr::F64Store { align, offset } => {
                write!(f, "f64.store align={align} offset={offset}")
            }
            WasmInstr::I32Store8 { align, offset } => {
                write!(f, "i32.store8 align={align} offset={offset}")
            }
            WasmInstr::I32Store16 { align, offset } => {
                write!(f, "i32.store16 align={align} offset={offset}")
            }
            WasmInstr::I64Store8 { align, offset } => {
                write!(f, "i64.store8 align={align} offset={offset}")
            }
            WasmInstr::I64Store16 { align, offset } => {
                write!(f, "i64.store16 align={align} offset={offset}")
            }
            WasmInstr::I64Store32 { align, offset } => {
                write!(f, "i64.store32 align={align} offset={offset}")
            }

            WasmInstr::MemorySize(idx) => write!(f, "memory.size {idx}"),
            WasmInstr::MemoryGrow(idx) => write!(f, "memory.grow {idx}"),

            // Numeric i32
            WasmInstr::I32Const(val) => write!(f, "i32.const {val}"),
            WasmInstr::I32Eqz => write!(f, "i32.eqz"),
            WasmInstr::I32Eq => write!(f, "i32.eq"),
            WasmInstr::I32Ne => write!(f, "i32.ne"),
            WasmInstr::I32LtS => write!(f, "i32.lt_s"),
            WasmInstr::I32LtU => write!(f, "i32.lt_u"),
            WasmInstr::I32GtS => write!(f, "i32.gt_s"),
            WasmInstr::I32GtU => write!(f, "i32.gt_u"),
            WasmInstr::I32LeS => write!(f, "i32.le_s"),
            WasmInstr::I32LeU => write!(f, "i32.le_u"),
            WasmInstr::I32GeS => write!(f, "i32.ge_s"),
            WasmInstr::I32GeU => write!(f, "i32.ge_u"),
            WasmInstr::I32Clz => write!(f, "i32.clz"),
            WasmInstr::I32Ctz => write!(f, "i32.ctz"),
            WasmInstr::I32Popcnt => write!(f, "i32.popcnt"),
            WasmInstr::I32Add => write!(f, "i32.add"),
            WasmInstr::I32Sub => write!(f, "i32.sub"),
            WasmInstr::I32Mul => write!(f, "i32.mul"),
            WasmInstr::I32DivS => write!(f, "i32.div_s"),
            WasmInstr::I32DivU => write!(f, "i32.div_u"),
            WasmInstr::I32RemS => write!(f, "i32.rem_s"),
            WasmInstr::I32RemU => write!(f, "i32.rem_u"),
            WasmInstr::I32And => write!(f, "i32.and"),
            WasmInstr::I32Or => write!(f, "i32.or"),
            WasmInstr::I32Xor => write!(f, "i32.xor"),
            WasmInstr::I32Shl => write!(f, "i32.shl"),
            WasmInstr::I32ShrS => write!(f, "i32.shr_s"),
            WasmInstr::I32ShrU => write!(f, "i32.shr_u"),
            WasmInstr::I32Rotl => write!(f, "i32.rotl"),
            WasmInstr::I32Rotr => write!(f, "i32.rotr"),

            // Numeric i64
            WasmInstr::I64Const(val) => write!(f, "i64.const {val}"),
            WasmInstr::I64Eqz => write!(f, "i64.eqz"),
            WasmInstr::I64Eq => write!(f, "i64.eq"),
            WasmInstr::I64Ne => write!(f, "i64.ne"),
            WasmInstr::I64LtS => write!(f, "i64.lt_s"),
            WasmInstr::I64LtU => write!(f, "i64.lt_u"),
            WasmInstr::I64GtS => write!(f, "i64.gt_s"),
            WasmInstr::I64GtU => write!(f, "i64.gt_u"),
            WasmInstr::I64LeS => write!(f, "i64.le_s"),
            WasmInstr::I64LeU => write!(f, "i64.le_u"),
            WasmInstr::I64GeS => write!(f, "i64.ge_s"),
            WasmInstr::I64GeU => write!(f, "i64.ge_u"),
            WasmInstr::I64Clz => write!(f, "i64.clz"),
            WasmInstr::I64Ctz => write!(f, "i64.ctz"),
            WasmInstr::I64Popcnt => write!(f, "i64.popcnt"),
            WasmInstr::I64Add => write!(f, "i64.add"),
            WasmInstr::I64Sub => write!(f, "i64.sub"),
            WasmInstr::I64Mul => write!(f, "i64.mul"),
            WasmInstr::I64DivS => write!(f, "i64.div_s"),
            WasmInstr::I64DivU => write!(f, "i64.div_u"),
            WasmInstr::I64RemS => write!(f, "i64.rem_s"),
            WasmInstr::I64RemU => write!(f, "i64.rem_u"),
            WasmInstr::I64And => write!(f, "i64.and"),
            WasmInstr::I64Or => write!(f, "i64.or"),
            WasmInstr::I64Xor => write!(f, "i64.xor"),
            WasmInstr::I64Shl => write!(f, "i64.shl"),
            WasmInstr::I64ShrS => write!(f, "i64.shr_s"),
            WasmInstr::I64ShrU => write!(f, "i64.shr_u"),
            WasmInstr::I64Rotl => write!(f, "i64.rotl"),
            WasmInstr::I64Rotr => write!(f, "i64.rotr"),

            // Numeric f32
            WasmInstr::F32Const(val) => write!(f, "f32.const {val}"),
            WasmInstr::F32Eq => write!(f, "f32.eq"),
            WasmInstr::F32Ne => write!(f, "f32.ne"),
            WasmInstr::F32Lt => write!(f, "f32.lt"),
            WasmInstr::F32Gt => write!(f, "f32.gt"),
            WasmInstr::F32Le => write!(f, "f32.le"),
            WasmInstr::F32Ge => write!(f, "f32.ge"),
            WasmInstr::F32Add => write!(f, "f32.add"),
            WasmInstr::F32Sub => write!(f, "f32.sub"),
            WasmInstr::F32Mul => write!(f, "f32.mul"),
            WasmInstr::F32Div => write!(f, "f32.div"),
            WasmInstr::F32Sqrt => write!(f, "f32.sqrt"),
            WasmInstr::F32Neg => write!(f, "f32.neg"),

            // Numeric f64
            WasmInstr::F64Const(val) => write!(f, "f64.const {val}"),
            WasmInstr::F64Eq => write!(f, "f64.eq"),
            WasmInstr::F64Ne => write!(f, "f64.ne"),
            WasmInstr::F64Lt => write!(f, "f64.lt"),
            WasmInstr::F64Gt => write!(f, "f64.gt"),
            WasmInstr::F64Le => write!(f, "f64.le"),
            WasmInstr::F64Ge => write!(f, "f64.ge"),
            WasmInstr::F64Add => write!(f, "f64.add"),
            WasmInstr::F64Sub => write!(f, "f64.sub"),
            WasmInstr::F64Mul => write!(f, "f64.mul"),
            WasmInstr::F64Div => write!(f, "f64.div"),
            WasmInstr::F64Sqrt => write!(f, "f64.sqrt"),
            WasmInstr::F64Neg => write!(f, "f64.neg"),

            // Conversions
            WasmInstr::I32WrapI64 => write!(f, "i32.wrap_i64"),
            WasmInstr::I64ExtendI32S => write!(f, "i64.extend_i32_s"),
            WasmInstr::I64ExtendI32U => write!(f, "i64.extend_i32_u"),
            WasmInstr::F32DemoteF64 => write!(f, "f32.demote_f64"),
            WasmInstr::F64PromoteF32 => write!(f, "f64.promote_f32"),
            WasmInstr::I32TruncF32S => write!(f, "i32.trunc_f32_s"),
            WasmInstr::I32TruncF64S => write!(f, "i32.trunc_f64_s"),
            WasmInstr::I32TruncF32U => write!(f, "i32.trunc_f32_u"),
            WasmInstr::I32TruncF64U => write!(f, "i32.trunc_f64_u"),
            WasmInstr::I64TruncF32S => write!(f, "i64.trunc_f32_s"),
            WasmInstr::I64TruncF64S => write!(f, "i64.trunc_f64_s"),
            WasmInstr::I64TruncF32U => write!(f, "i64.trunc_f32_u"),
            WasmInstr::I64TruncF64U => write!(f, "i64.trunc_f64_u"),
            WasmInstr::F32ConvertI32S => write!(f, "f32.convert_i32_s"),
            WasmInstr::F32ConvertI64S => write!(f, "f32.convert_i64_s"),
            WasmInstr::F32ConvertI32U => write!(f, "f32.convert_i32_u"),
            WasmInstr::F32ConvertI64U => write!(f, "f32.convert_i64_u"),
            WasmInstr::F64ConvertI32S => write!(f, "f64.convert_i32_s"),
            WasmInstr::F64ConvertI64S => write!(f, "f64.convert_i64_s"),
            WasmInstr::F64ConvertI32U => write!(f, "f64.convert_i32_u"),
            WasmInstr::F64ConvertI64U => write!(f, "f64.convert_i64_u"),
            WasmInstr::I32ReinterpretF32 => write!(f, "i32.reinterpret_f32"),
            WasmInstr::I64ReinterpretF64 => write!(f, "i64.reinterpret_f64"),
            WasmInstr::F32ReinterpretI32 => write!(f, "f32.reinterpret_i32"),
            WasmInstr::F64ReinterpretI64 => write!(f, "f64.reinterpret_i64"),

            // Pseudo
            WasmInstr::Nop => write!(f, "nop"),
            WasmInstr::Unreachable => write!(f, "unreachable"),

            // SIMD
            WasmInstr::V128Const(_) => write!(f, "v128.const"),
            WasmInstr::I32X4Add => write!(f, "i32x4.add"),
            WasmInstr::I32X4Mul => write!(f, "i32x4.mul"),
            WasmInstr::F32X4Add => write!(f, "f32x4.add"),
            WasmInstr::F32X4Mul => write!(f, "f32x4.mul"),

            // Bulk memory
            WasmInstr::MemoryCopy { src_mem, dst_mem } => {
                write!(f, "memory.copy {src_mem} {dst_mem}")
            }
            WasmInstr::MemoryFill { mem } => write!(f, "memory.fill {mem}"),
            WasmInstr::MemoryInit { data_idx, mem } => write!(f, "memory.init {data_idx} {mem}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Decode helpers
// ---------------------------------------------------------------------------

/// Read a memory-immediate (align + offset, both unsigned LEB128) from the
/// byte stream. Returns (align, offset, new_pos).
fn read_memarg(bytes: &[u8], pos: usize) -> Result<(u32, u32, usize), DecodeError> {
    if pos >= bytes.len() {
        return Err(DecodeError::Truncated {
            needed: pos + 1,
            available: bytes.len(),
        });
    }
    let (align, n1) = decode_unsigned_leb128(&bytes[pos..]);
    let pos2 = pos + n1;
    if pos2 >= bytes.len() {
        return Err(DecodeError::Truncated {
            needed: pos2 + 1,
            available: bytes.len(),
        });
    }
    let (offset, n2) = decode_unsigned_leb128(&bytes[pos2..]);
    Ok((align as u32, offset as u32, pos2 + n2))
}

/// Read a signed LEB128 i32 immediate.
fn read_i32(bytes: &[u8], pos: usize) -> Result<(i32, usize), DecodeError> {
    if pos >= bytes.len() {
        return Err(DecodeError::Truncated {
            needed: pos + 1,
            available: bytes.len(),
        });
    }
    let (val, n) = decode_signed_leb128(&bytes[pos..]);
    Ok((val as i32, pos + n))
}

/// Read a signed LEB128 i64 immediate.
fn read_i64(bytes: &[u8], pos: usize) -> Result<(i64, usize), DecodeError> {
    if pos >= bytes.len() {
        return Err(DecodeError::Truncated {
            needed: pos + 1,
            available: bytes.len(),
        });
    }
    let (val, n) = decode_signed_leb128(&bytes[pos..]);
    Ok((val, pos + n))
}

/// Read an unsigned LEB128 index.
fn read_idx(bytes: &[u8], pos: usize) -> Result<(u32, usize), DecodeError> {
    if pos >= bytes.len() {
        return Err(DecodeError::Truncated {
            needed: pos + 1,
            available: bytes.len(),
        });
    }
    let (val, n) = decode_unsigned_leb128(&bytes[pos..]);
    Ok((val as u32, pos + n))
}

/// Read a WasmType byte from the stream.
fn read_block_type(bytes: &[u8], pos: usize) -> Result<(Option<WasmType>, usize), DecodeError> {
    if pos >= bytes.len() {
        return Err(DecodeError::Truncated {
            needed: pos + 1,
            available: bytes.len(),
        });
    }
    let ty = match bytes[pos] {
        0x7F => Some(WasmType::I32),
        0x7E => Some(WasmType::I64),
        0x7D => Some(WasmType::F32),
        0x7C => Some(WasmType::F64),
        0x40 => None, // void block type
        other => {
            return Err(DecodeError::UnknownOpcode { opcode: other });
        }
    };
    Ok((ty, pos + 1))
}

// ---------------------------------------------------------------------------
// Main decode entry point
// ---------------------------------------------------------------------------

impl WasmInstr {
    /// Decode a single WebAssembly instruction from the byte stream.
    ///
    /// Returns the decoded instruction and the number of bytes consumed.
    pub fn decode(bytes: &[u8]) -> Result<(Self, usize), DecodeError> {
        if bytes.is_empty() {
            return Err(DecodeError::Truncated {
                needed: 1,
                available: 0,
            });
        }

        let opcode = bytes[0];
        let mut pos = 1usize;

        let instr = match opcode {
            // Control
            0x02 => {
                let (ty, np) = read_block_type(bytes, pos)?;
                pos = np;
                WasmInstr::Block(ty)
            }
            0x03 => {
                let (ty, np) = read_block_type(bytes, pos)?;
                pos = np;
                WasmInstr::Loop(ty)
            }
            0x04 => {
                let (ty, np) = read_block_type(bytes, pos)?;
                pos = np;
                WasmInstr::If(ty)
            }
            0x05 => WasmInstr::Else,
            0x0B => WasmInstr::End,
            0x0C => {
                let (idx, np) = read_idx(bytes, pos)?;
                pos = np;
                WasmInstr::Br(idx)
            }
            0x0D => {
                let (idx, np) = read_idx(bytes, pos)?;
                pos = np;
                WasmInstr::BrIf(idx)
            }
            0x0F => WasmInstr::Return,
            0x10 => {
                let (idx, np) = read_idx(bytes, pos)?;
                pos = np;
                WasmInstr::Call(idx)
            }

            // Parametric
            0x1B => WasmInstr::Select,
            0x1A => WasmInstr::Drop,

            // Variable
            0x20 => {
                let (idx, np) = read_idx(bytes, pos)?;
                pos = np;
                WasmInstr::LocalGet(idx)
            }
            0x21 => {
                let (idx, np) = read_idx(bytes, pos)?;
                pos = np;
                WasmInstr::LocalSet(idx)
            }
            0x22 => {
                let (idx, np) = read_idx(bytes, pos)?;
                pos = np;
                WasmInstr::LocalTee(idx)
            }
            0x23 => {
                let (idx, np) = read_idx(bytes, pos)?;
                pos = np;
                WasmInstr::GlobalGet(idx)
            }
            0x24 => {
                let (idx, np) = read_idx(bytes, pos)?;
                pos = np;
                WasmInstr::GlobalSet(idx)
            }

            // Memory loads
            0x28 => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I32Load {
                    align: a,
                    offset: o,
                }
            }
            0x29 => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I64Load {
                    align: a,
                    offset: o,
                }
            }
            0x2A => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::F32Load {
                    align: a,
                    offset: o,
                }
            }
            0x2B => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::F64Load {
                    align: a,
                    offset: o,
                }
            }
            0x2C => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I32Load8S {
                    align: a,
                    offset: o,
                }
            }
            0x2D => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I32Load8U {
                    align: a,
                    offset: o,
                }
            }
            0x2E => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I32Load16S {
                    align: a,
                    offset: o,
                }
            }
            0x2F => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I32Load16U {
                    align: a,
                    offset: o,
                }
            }
            0x30 => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I64Load8S {
                    align: a,
                    offset: o,
                }
            }
            0x31 => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I64Load8U {
                    align: a,
                    offset: o,
                }
            }
            0x32 => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I64Load16S {
                    align: a,
                    offset: o,
                }
            }
            0x33 => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I64Load16U {
                    align: a,
                    offset: o,
                }
            }
            0x34 => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I64Load32S {
                    align: a,
                    offset: o,
                }
            }
            0x35 => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I64Load32U {
                    align: a,
                    offset: o,
                }
            }

            // Memory stores
            0x36 => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I32Store {
                    align: a,
                    offset: o,
                }
            }
            0x37 => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I64Store {
                    align: a,
                    offset: o,
                }
            }
            0x38 => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::F32Store {
                    align: a,
                    offset: o,
                }
            }
            0x39 => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::F64Store {
                    align: a,
                    offset: o,
                }
            }
            0x3A => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I32Store8 {
                    align: a,
                    offset: o,
                }
            }
            0x3B => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I32Store16 {
                    align: a,
                    offset: o,
                }
            }
            0x3C => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I64Store8 {
                    align: a,
                    offset: o,
                }
            }
            0x3D => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I64Store16 {
                    align: a,
                    offset: o,
                }
            }
            0x3E => {
                let (a, o, np) = read_memarg(bytes, pos)?;
                pos = np;
                WasmInstr::I64Store32 {
                    align: a,
                    offset: o,
                }
            }
            0x3F => {
                let (idx, np) = read_idx(bytes, pos)?;
                pos = np;
                WasmInstr::MemorySize(idx)
            }
            0x40 => {
                let (idx, np) = read_idx(bytes, pos)?;
                pos = np;
                WasmInstr::MemoryGrow(idx)
            }

            // i32 constants and comparisons
            0x41 => {
                let (val, np) = read_i32(bytes, pos)?;
                pos = np;
                WasmInstr::I32Const(val)
            }
            0x45 => WasmInstr::I32Eqz,
            0x46 => WasmInstr::I32Eq,
            0x47 => WasmInstr::I32Ne,
            0x48 => WasmInstr::I32LtS,
            0x49 => WasmInstr::I32LtU,
            0x4A => WasmInstr::I32GtS,
            0x4B => WasmInstr::I32GtU,
            0x4C => WasmInstr::I32LeS,
            0x4D => WasmInstr::I32LeU,
            0x4E => WasmInstr::I32GeS,
            0x4F => WasmInstr::I32GeU,
            0x67 => WasmInstr::I32Clz,
            0x68 => WasmInstr::I32Ctz,
            0x69 => WasmInstr::I32Popcnt,
            0x6A => WasmInstr::I32Add,
            0x6B => WasmInstr::I32Sub,
            0x6C => WasmInstr::I32Mul,
            0x6D => WasmInstr::I32DivS,
            0x6E => WasmInstr::I32DivU,
            0x6F => WasmInstr::I32RemS,
            0x70 => WasmInstr::I32RemU,
            0x71 => WasmInstr::I32And,
            0x72 => WasmInstr::I32Or,
            0x73 => WasmInstr::I32Xor,
            0x74 => WasmInstr::I32Shl,
            0x75 => WasmInstr::I32ShrS,
            0x76 => WasmInstr::I32ShrU,
            0x77 => WasmInstr::I32Rotl,
            0x78 => WasmInstr::I32Rotr,

            // i64
            0x42 => {
                let (val, np) = read_i64(bytes, pos)?;
                pos = np;
                WasmInstr::I64Const(val)
            }
            0x50 => WasmInstr::I64Eqz,
            0x51 => WasmInstr::I64Eq,
            0x52 => WasmInstr::I64Ne,
            0x53 => WasmInstr::I64LtS,
            0x54 => WasmInstr::I64LtU,
            0x55 => WasmInstr::I64GtS,
            0x56 => WasmInstr::I64GtU,
            0x57 => WasmInstr::I64LeS,
            0x58 => WasmInstr::I64LeU,
            0x59 => WasmInstr::I64GeS,
            0x5A => WasmInstr::I64GeU,
            0x79 => WasmInstr::I64Clz,
            0x7A => WasmInstr::I64Ctz,
            0x7B => WasmInstr::I64Popcnt,
            0x7C => WasmInstr::I64Add,
            0x7D => WasmInstr::I64Sub,
            0x7E => WasmInstr::I64Mul,
            0x7F => WasmInstr::I64DivS,
            0x80 => WasmInstr::I64DivU,
            0x81 => WasmInstr::I64RemS,
            0x82 => WasmInstr::I64RemU,
            0x83 => WasmInstr::I64And,
            0x84 => WasmInstr::I64Or,
            0x85 => WasmInstr::I64Xor,
            0x86 => WasmInstr::I64Shl,
            0x87 => WasmInstr::I64ShrS,
            0x88 => WasmInstr::I64ShrU,
            0x89 => WasmInstr::I64Rotl,
            0x8A => WasmInstr::I64Rotr,

            // f32
            0x43 => {
                if pos + 4 > bytes.len() {
                    return Err(DecodeError::Truncated {
                        needed: pos + 4,
                        available: bytes.len(),
                    });
                }
                let val = f32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap_or([0; 4]));
                pos += 4;
                WasmInstr::F32Const(val)
            }
            0x5B => WasmInstr::F32Eq,
            0x5C => WasmInstr::F32Ne,
            0x5D => WasmInstr::F32Lt,
            0x5E => WasmInstr::F32Gt,
            0x5F => WasmInstr::F32Le,
            0x60 => WasmInstr::F32Ge,
            0x92 => WasmInstr::F32Add,
            0x93 => WasmInstr::F32Sub,
            0x94 => WasmInstr::F32Mul,
            0x95 => WasmInstr::F32Div,
            0x91 => WasmInstr::F32Sqrt,
            0x8C => WasmInstr::F32Neg,

            // f64
            0x44 => {
                if pos + 8 > bytes.len() {
                    return Err(DecodeError::Truncated {
                        needed: pos + 8,
                        available: bytes.len(),
                    });
                }
                let val = f64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap_or([0; 8]));
                pos += 8;
                WasmInstr::F64Const(val)
            }
            0x61 => WasmInstr::F64Eq,
            0x62 => WasmInstr::F64Ne,
            0x63 => WasmInstr::F64Lt,
            0x64 => WasmInstr::F64Gt,
            0x65 => WasmInstr::F64Le,
            0x66 => WasmInstr::F64Ge,
            0xA0 => WasmInstr::F64Add,
            0xA1 => WasmInstr::F64Sub,
            0xA2 => WasmInstr::F64Mul,
            0xA3 => WasmInstr::F64Div,
            0x9F => WasmInstr::F64Sqrt,
            0x9A => WasmInstr::F64Neg,

            // Conversions
            0xA7 => WasmInstr::I32WrapI64,
            0xAC => WasmInstr::I64ExtendI32S,
            0xAD => WasmInstr::I64ExtendI32U,
            0xB6 => WasmInstr::F32DemoteF64,
            0xBB => WasmInstr::F64PromoteF32,
            0xA8 => WasmInstr::I32TruncF32S,
            0xA9 => WasmInstr::I32TruncF64S,
            0xAE => WasmInstr::I64TruncF32S,
            0xAF => WasmInstr::I64TruncF64S,
            0xB2 => WasmInstr::F32ConvertI32S,
            0xB3 => WasmInstr::F32ConvertI64S,
            0xB7 => WasmInstr::F64ConvertI32S,
            0xB8 => WasmInstr::F64ConvertI64S,
            0xBC => WasmInstr::I32ReinterpretF32,
            0xBD => WasmInstr::I64ReinterpretF64,
            0xBE => WasmInstr::F32ReinterpretI32,
            0xBF => WasmInstr::F64ReinterpretI64,

            // Pseudo
            0x01 => WasmInstr::Nop,
            0x00 => WasmInstr::Unreachable,

            _ => {
                return Err(DecodeError::UnknownOpcode { opcode });
            }
        };

        Ok((instr, pos))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_i32_add() {
        let bytes = [0x6A];
        let (instr, n) = WasmInstr::decode(&bytes).unwrap();
        assert_eq!(n, 1);
        assert_eq!(instr, WasmInstr::I32Add);
        assert_eq!(format!("{instr}"), "i32.add");
    }

    #[test]
    fn test_decode_i32_const() {
        // i32.const 42 → opcode 0x41 + signed LEB128 of 42
        let mut bytes = vec![0x41];
        bytes.extend_from_slice(&super::super::encode_signed_leb128(42));
        let (instr, n) = WasmInstr::decode(&bytes).unwrap();
        assert_eq!(instr, WasmInstr::I32Const(42));
        assert_eq!(format!("{instr}"), "i32.const 42");
        assert!(n > 1);
    }

    #[test]
    fn test_decode_local_get() {
        let mut bytes = vec![0x20];
        bytes.extend_from_slice(&super::super::encode_unsigned_leb128(5));
        let (instr, _) = WasmInstr::decode(&bytes).unwrap();
        assert_eq!(instr, WasmInstr::LocalGet(5));
        assert_eq!(format!("{instr}"), "local.get 5");
    }

    #[test]
    fn test_roundtrip_i32_sub() {
        let original = WasmInstr::I32Sub;
        let mut encoded = Vec::new();
        original.encode(&mut encoded);
        let (decoded, _) = WasmInstr::decode(&encoded).unwrap();
        assert_eq!(original, decoded);
        assert_eq!(format!("{decoded}"), "i32.sub");
    }

    #[test]
    fn test_roundtrip_i64_mul() {
        let original = WasmInstr::I64Mul;
        let mut encoded = Vec::new();
        original.encode(&mut encoded);
        let (decoded, _) = WasmInstr::decode(&encoded).unwrap();
        assert_eq!(original, decoded);
        assert_eq!(format!("{decoded}"), "i64.mul");
    }

    #[test]
    fn test_roundtrip_i32_load() {
        let original = WasmInstr::I32Load {
            align: 2,
            offset: 8,
        };
        let mut encoded = Vec::new();
        original.encode(&mut encoded);
        let (decoded, _) = WasmInstr::decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_roundtrip_i32_store() {
        let original = WasmInstr::I32Store {
            align: 2,
            offset: 0,
        };
        let mut encoded = Vec::new();
        original.encode(&mut encoded);
        let (decoded, _) = WasmInstr::decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_roundtrip_i32_const_negative() {
        let original = WasmInstr::I32Const(-1);
        let mut encoded = Vec::new();
        original.encode(&mut encoded);
        let (decoded, _) = WasmInstr::decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_roundtrip_call() {
        let original = WasmInstr::Call(42);
        let mut encoded = Vec::new();
        original.encode(&mut encoded);
        let (decoded, _) = WasmInstr::decode(&encoded).unwrap();
        assert_eq!(original, decoded);
        assert_eq!(format!("{decoded}"), "call 42");
    }

    #[test]
    fn test_decode_truncated() {
        let result = WasmInstr::decode(&[]);
        assert!(matches!(result, Err(DecodeError::Truncated { .. })));
    }

    #[test]
    fn test_decode_unknown_opcode() {
        let result = WasmInstr::decode(&[0xFE]);
        assert!(matches!(result, Err(DecodeError::UnknownOpcode { .. })));
    }
}
