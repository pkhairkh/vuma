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
//!  │    ├── name, params, results, param_types, result_types
//!  │    └── Vec<IRBlock>
//!  │         ├── label
//!  │         ├── Vec<IRInstr>
//!  │         └── IRTerminator
//!  └── Vec<DataSection>
//! ```
//!
//! ## Type System
//!
//! The IR includes a type system (`IRType`) that models the ARM64 LP64 data
//! model, with functions for computing sizes, alignments, AAPCS64 argument
//! classification, calling-convention layout, and stack-frame layout.
//!
//! The IR is intentionally low-level but target-independent.  It uses virtual
//! registers (`IRValue::Register(id)`) that are later mapped to physical
//! ARM64 registers by the register allocator.

use std::collections::{HashMap, HashSet};
use std::fmt;

// ---------------------------------------------------------------------------
// IRType
// ---------------------------------------------------------------------------

/// A type in the IR type system.
///
/// Models the ARM64 LP64 data model where `int` is 32 bits and pointers are
/// 64 bits.  Supports primitive integer/float types, pointers, void,
/// function pointers, structs, and arrays.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum IRType {
    /// Signed 8-bit integer.
    I8,
    /// Signed 16-bit integer.
    I16,
    /// Signed 32-bit integer.
    I32,
    /// Signed 64-bit integer.
    I64,
    /// Unsigned 8-bit integer.
    U8,
    /// Unsigned 16-bit integer.
    U16,
    /// Unsigned 32-bit integer.
    U32,
    /// Unsigned 64-bit integer.
    U64,
    /// 32-bit IEEE 754 floating-point.
    F32,
    /// 64-bit IEEE 754 floating-point.
    F64,
    /// Opaque pointer (64-bit on ARM64 LP64).
    Ptr,
    /// Void type (size 0, only valid as a function return type).
    Void,
    /// Function pointer (pointer-sized).
    Func,
    /// Struct type with a name and ordered fields.
    Struct {
        /// Struct name (may be empty for anonymous structs).
        name: String,
        /// Fields in declaration order.
        fields: Vec<IRType>,
    },
    /// Array type with an element type and element count.
    Array {
        /// Element type.
        element: Box<IRType>,
        /// Number of elements.
        count: usize,
    },
}

impl IRType {
    /// Returns `true` if this is an integer type (signed or unsigned).
    pub fn is_integer(&self) -> bool {
        matches!(
            self,
            IRType::I8
                | IRType::I16
                | IRType::I32
                | IRType::I64
                | IRType::U8
                | IRType::U16
                | IRType::U32
                | IRType::U64
        )
    }

    /// Returns `true` if this is a floating-point type.
    pub fn is_float(&self) -> bool {
        matches!(self, IRType::F32 | IRType::F64)
    }

    /// Returns `true` if this type is passed by value in registers under
    /// AAPCS64 (integer or FP primitive, pointer, func, or small struct/array).
    pub fn is_register_passable(&self) -> bool {
        match self {
            IRType::I8
            | IRType::I16
            | IRType::I32
            | IRType::I64
            | IRType::U8
            | IRType::U16
            | IRType::U32
            | IRType::U64
            | IRType::F32
            | IRType::F64
            | IRType::Ptr
            | IRType::Func => true,
            IRType::Void => false,
            IRType::Struct { fields, .. } => size_of(self) <= 16 && !fields.is_empty(),
            IRType::Array { .. } => size_of(self) <= 16,
        }
    }

    /// Returns `true` if this is a Homogeneous Floating-point Aggregate (HFA)
    /// — a struct or array of 1–4 identical floating-point members.
    pub fn is_hfa(&self) -> bool {
        match self {
            IRType::Struct { fields, .. } => {
                if fields.is_empty() || fields.len() > 4 {
                    return false;
                }
                let first = &fields[0];
                if !first.is_float() {
                    return false;
                }
                fields.iter().all(|f| f == first)
            }
            IRType::Array { element, count } => {
                if *count == 0 || *count > 4 {
                    return false;
                }
                element.is_float()
            }
            _ => false,
        }
    }

    /// If this is an HFA, returns the element type and count.
    pub fn hfa_info(&self) -> Option<(&IRType, usize)> {
        if !self.is_hfa() {
            return None;
        }
        match self {
            IRType::Struct { fields, .. } => Some((&fields[0], fields.len())),
            IRType::Array { element, count } => Some((element, *count)),
            _ => None,
        }
    }
}

impl fmt::Display for IRType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IRType::I8 => write!(f, "i8"),
            IRType::I16 => write!(f, "i16"),
            IRType::I32 => write!(f, "i32"),
            IRType::I64 => write!(f, "i64"),
            IRType::U8 => write!(f, "u8"),
            IRType::U16 => write!(f, "u16"),
            IRType::U32 => write!(f, "u32"),
            IRType::U64 => write!(f, "u64"),
            IRType::F32 => write!(f, "f32"),
            IRType::F64 => write!(f, "f64"),
            IRType::Ptr => write!(f, "ptr"),
            IRType::Void => write!(f, "void"),
            IRType::Func => write!(f, "func"),
            IRType::Struct { name, fields } => {
                let fields_str = fields
                    .iter()
                    .map(|t| format!("{}", t))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "struct {} {{ {} }}", name, fields_str)
            }
            IRType::Array { element, count } => {
                write!(f, "[{}; {}]", element, count)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// size_of / alignment_of  (ARM64 LP64)
// ---------------------------------------------------------------------------

/// Returns the byte size of `t` on ARM64 under the LP64 data model.
///
/// - Integers: their bit-width / 8
/// - Floats: 4 (f32) or 8 (f64)
/// - Pointers / function pointers: 8
/// - Void: 0
/// - Structs: sum of field sizes with inter-field padding for alignment,
///   rounded up to the struct's own alignment
/// - Arrays: `size_of(element) * count`
pub fn size_of(t: &IRType) -> usize {
    match t {
        IRType::I8 | IRType::U8 => 1,
        IRType::I16 | IRType::U16 => 2,
        IRType::I32 | IRType::U32 => 4,
        IRType::I64 | IRType::U64 | IRType::Ptr | IRType::Func => 8,
        IRType::F32 => 4,
        IRType::F64 => 8,
        IRType::Void => 0,
        IRType::Struct { fields, .. } => {
            let mut offset = 0usize;
            for field in fields {
                let field_align = alignment_of(field);
                // Align the current offset to the field alignment.
                offset = (offset + field_align - 1) & !(field_align - 1);
                offset += size_of(field);
            }
            // Round up to struct alignment.
            let struct_align = alignment_of(t);
            if struct_align > 0 {
                (offset + struct_align - 1) & !(struct_align - 1)
            } else {
                0
            }
        }
        IRType::Array { element, count } => size_of(element) * count,
    }
}

/// Returns the natural alignment of `t` on ARM64 under the LP64 data model.
///
/// - Primitives: their size
/// - Void: 1 (by convention, so that `size_of` math works for empty structs)
/// - Structs: maximum alignment of any field
/// - Arrays: alignment of the element type
pub fn alignment_of(t: &IRType) -> usize {
    match t {
        IRType::I8 | IRType::U8 => 1,
        IRType::I16 | IRType::U16 => 2,
        IRType::I32 | IRType::U32 => 4,
        IRType::I64 | IRType::U64 | IRType::Ptr | IRType::Func => 8,
        IRType::F32 => 4,
        IRType::F64 => 8,
        IRType::Void => 1,
        IRType::Struct { fields, .. } => {
            if fields.is_empty() {
                return 1;
            }
            fields.iter().map(alignment_of).max().unwrap_or(1)
        }
        IRType::Array { element, .. } => alignment_of(element),
    }
}

// ---------------------------------------------------------------------------
// AAPCS64 Argument Classification
// ---------------------------------------------------------------------------

/// Classification of how an argument or return value is passed under AAPCS64.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ArgClass {
    /// Passed in a general-purpose (X) register.
    Integer,
    /// Passed in a SIMD/FP (V) register.
    FP,
    /// Passed on the stack.
    Stack,
    /// Passed indirectly — the caller passes a pointer to the value.
    /// For returns, the caller provides the address in X8.
    Indirect,
}

impl fmt::Display for ArgClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ArgClass::Integer => "integer",
            ArgClass::FP => "fp",
            ArgClass::Stack => "stack",
            ArgClass::Indirect => "indirect",
        })
    }
}

/// Classifies a single argument or return-value type according to AAPCS64.
///
/// ## Rules (simplified)
///
/// - Integer types, pointers, and function pointers → `Integer`
/// - Floating-point types → `FP`
/// - Void → `Integer` (only valid for return; caller should check)
/// - Homogeneous FP aggregates (1–4 same-type FP members) → `FP`
/// - Structs / arrays ≤ 16 bytes (not HFA) → `Integer`
/// - Structs / arrays > 16 bytes → `Indirect`
pub fn classify_arg(t: &IRType) -> ArgClass {
    match t {
        // Integer types → Integer class
        IRType::I8
        | IRType::I16
        | IRType::I32
        | IRType::I64
        | IRType::U8
        | IRType::U16
        | IRType::U32
        | IRType::U64
        | IRType::Ptr
        | IRType::Func => ArgClass::Integer,

        // Floating-point → FP class
        IRType::F32 | IRType::F64 => ArgClass::FP,

        // Void — only meaningful as a return type; classify as Integer
        // (the caller should treat a void return as "no return value").
        IRType::Void => ArgClass::Integer,

        // Struct / Array: check HFA first, then size.
        IRType::Struct { .. } | IRType::Array { .. } => {
            if t.is_hfa() {
                ArgClass::FP
            } else {
                let sz = size_of(t);
                if sz <= 16 {
                    ArgClass::Integer
                } else {
                    ArgClass::Indirect
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Calling Convention
// ---------------------------------------------------------------------------

/// Which kind of register an argument or return value occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum RegisterClass {
    /// General-purpose X register (X0–X30, SP, XZR).
    X,
    /// SIMD/FP V register (V0–V31).
    V,
}

impl fmt::Display for RegisterClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RegisterClass::X => "x",
            RegisterClass::V => "v",
        })
    }
}

/// Describes where a single argument or return value is placed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArgLocation {
    /// Index of the argument (0-based) in the argument list.
    pub index: usize,
    /// AAPCS64 classification of this argument.
    pub class: ArgClass,
    /// Register class and index, if passed in a register.
    /// For `ArgClass::Integer` → `(RegisterClass::X, n)` where n is 0–7.
    /// For `ArgClass::FP` → `(RegisterClass::V, n)` where n is 0–7.
    /// `None` for stack or indirect arguments.
    pub register: Option<(RegisterClass, u32)>,
    /// Byte offset from SP (for stack arguments) or from the indirect
    /// pointer, if applicable.  `None` for register arguments.
    pub stack_offset: Option<i32>,
}

/// Describes the location of the return value.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RetLocation {
    /// AAPCS64 classification of the return.
    pub class: ArgClass,
    /// Register(s) used for the return value.
    /// - Void: empty
    /// - Integer ≤ 8 bytes: `[(X, 0)]`
    /// - Integer 9–16 bytes (struct): `[(X, 0), (X, 1)]`
    /// - FP (HFA 1 member): `[(V, 0)]`, etc.
    /// - Indirect: `[(X, 8)]` — X8 holds the caller-allocated address
    pub registers: Vec<(RegisterClass, u32)>,
}

/// Complete calling-convention information for a function signature.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CallingConvInfo {
    /// Location for each argument.
    pub arg_locations: Vec<ArgLocation>,
    /// Location for the return value.
    pub ret_location: RetLocation,
    /// Total bytes of stack argument space that the *caller* must reserve
    /// for arguments that don't fit in registers.
    pub stack_args_size: usize,
}

/// Computes calling-convention information for a function signature.
///
/// Walks the argument types, classifies each, and assigns registers (X0–X7
/// for integer/pointer, V0–V7 for FP) or stack slots.  Also classifies the
/// return value.
///
/// ## Return-value handling
///
/// - `void` → no registers
/// - Integer/pointer ≤ 8 bytes → X0
/// - Small struct (≤16 bytes, integer-like) → X0, X1
/// - FP primitive → V0
/// - HFA with N members → V0..V(N-1)
/// - Large type (>16 bytes, not HFA) → indirect via X8
pub fn compute_calling_conv(args: &[IRType], ret: &IRType) -> CallingConvInfo {
    let mut x_reg_idx: u32 = 0;
    let mut v_reg_idx: u32 = 0;
    let mut stack_offset: i32 = 0;
    let mut arg_locations = Vec::with_capacity(args.len());

    // If the return value is indirect, X8 is used for the return-area
    // pointer.  Per AAPCS64, this is passed as a hidden first argument in X8.
    // We model it by setting x_reg_idx = 1 so that the first explicit
    // argument starts at X0 (if not indirect) or X1 (since X8 is reserved).
    // Actually, AAPCS64 says X8 is *separate* — it does not consume an
    // argument register slot.  The integer argument registers X0–X7 are
    // independent of X8.  So we don't need to adjust x_reg_idx.
    let indirect_ret = classify_arg(ret) == ArgClass::Indirect;

    for (i, arg_type) in args.iter().enumerate() {
        let class = classify_arg(arg_type);
        match class {
            ArgClass::Integer => {
                if x_reg_idx < 8 {
                    arg_locations.push(ArgLocation {
                        index: i,
                        class,
                        register: Some((RegisterClass::X, x_reg_idx)),
                        stack_offset: None,
                    });
                    x_reg_idx += 1;
                } else {
                    // Spill to stack — each stack arg is 8-byte aligned.
                    arg_locations.push(ArgLocation {
                        index: i,
                        class: ArgClass::Stack,
                        register: None,
                        stack_offset: Some(stack_offset),
                    });
                    stack_offset += 8;
                }
            }
            ArgClass::FP => {
                // HFA: may consume multiple V registers.
                if let Some((elem_ty, count)) = arg_type.hfa_info() {
                    let elem_size = size_of(elem_ty);
                    if v_reg_idx + count as u32 <= 8 {
                        // HFA fits in V registers — store the first register;
                        // the remaining are consecutive.
                        arg_locations.push(ArgLocation {
                            index: i,
                            class,
                            register: Some((RegisterClass::V, v_reg_idx)),
                            stack_offset: None,
                        });
                        v_reg_idx += count as u32;
                    } else {
                        // HFA spills to stack — each member is naturally aligned.
                        let mut off = stack_offset;
                        for _ in 0..count {
                            let align = alignment_of(elem_ty) as i32;
                            off = (off + align - 1) & !(align - 1);
                            off += elem_size as i32;
                        }
                        arg_locations.push(ArgLocation {
                            index: i,
                            class: ArgClass::Stack,
                            register: None,
                            stack_offset: Some(stack_offset),
                        });
                        stack_offset = off;
                    }
                } else {
                    // Single FP value.
                    if v_reg_idx < 8 {
                        arg_locations.push(ArgLocation {
                            index: i,
                            class,
                            register: Some((RegisterClass::V, v_reg_idx)),
                            stack_offset: None,
                        });
                        v_reg_idx += 1;
                    } else {
                        let align = alignment_of(arg_type) as i32;
                        stack_offset = (stack_offset + align - 1) & !(align - 1);
                        arg_locations.push(ArgLocation {
                            index: i,
                            class: ArgClass::Stack,
                            register: None,
                            stack_offset: Some(stack_offset),
                        });
                        stack_offset += size_of(arg_type) as i32;
                    }
                }
            }
            ArgClass::Indirect => {
                // Pass a pointer to the value in an X register (or on stack).
                if x_reg_idx < 8 {
                    arg_locations.push(ArgLocation {
                        index: i,
                        class,
                        register: Some((RegisterClass::X, x_reg_idx)),
                        stack_offset: None,
                    });
                    x_reg_idx += 1;
                } else {
                    arg_locations.push(ArgLocation {
                        index: i,
                        class: ArgClass::Stack,
                        register: None,
                        stack_offset: Some(stack_offset),
                    });
                    stack_offset += 8;
                }
            }
            ArgClass::Stack => {
                let align = alignment_of(arg_type) as i32;
                stack_offset = (stack_offset + align - 1) & !(align - 1);
                arg_locations.push(ArgLocation {
                    index: i,
                    class,
                    register: None,
                    stack_offset: Some(stack_offset),
                });
                stack_offset += size_of(arg_type) as i32;
            }
        }
    }

    // Align stack args size to 16 bytes (AAPCS64 requirement).
    let stack_args_size = ((stack_offset as usize) + 15) & !15;

    // Compute return-value location.
    let ret_class = classify_arg(ret);
    let ret_location = match ret_class {
        ArgClass::Integer => {
            if *ret == IRType::Void {
                RetLocation {
                    class: ret_class,
                    registers: vec![],
                }
            } else {
                let sz = size_of(ret);
                if sz <= 8 {
                    RetLocation {
                        class: ret_class,
                        registers: vec![(RegisterClass::X, 0)],
                    }
                } else {
                    // Struct ≤ 16 bytes returned in X0 + X1.
                    RetLocation {
                        class: ret_class,
                        registers: vec![(RegisterClass::X, 0), (RegisterClass::X, 1)],
                    }
                }
            }
        }
        ArgClass::FP => {
            if let Some((_, count)) = ret.hfa_info() {
                let regs: Vec<(RegisterClass, u32)> =
                    (0..count as u32).map(|i| (RegisterClass::V, i)).collect();
                RetLocation {
                    class: ret_class,
                    registers: regs,
                }
            } else {
                RetLocation {
                    class: ret_class,
                    registers: vec![(RegisterClass::V, 0)],
                }
            }
        }
        ArgClass::Indirect => RetLocation {
            class: ret_class,
            registers: vec![(RegisterClass::X, 8)],
        },
        ArgClass::Stack => {
            // Stack return is unusual; model as indirect for now.
            RetLocation {
                class: ArgClass::Indirect,
                registers: vec![(RegisterClass::X, 8)],
            }
        }
    };

    let _ = indirect_ret; // X8 reservation is implicit via ret_location.

    CallingConvInfo {
        arg_locations,
        ret_location,
        stack_args_size,
    }
}

// ---------------------------------------------------------------------------
// Stack Layout
// ---------------------------------------------------------------------------

/// A named slot in the stack frame.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StackSlot {
    /// Human-readable name for this slot.
    pub name: String,
    /// Offset from the frame pointer (FP / X29).  Negative values are in the
    /// callee's frame (locals, saved regs); positive values point into the
    /// caller's stack-argument area.
    pub offset: i32,
    /// Size of this slot in bytes.
    pub size: usize,
    /// Alignment of this slot in bytes.
    pub alignment: usize,
}

/// Complete stack-frame layout for a function.
///
/// The stack frame layout follows the AAPCS64 convention:
///
/// ```text
/// Higher addresses
///   ┌─────────────────────┐
///   │ Incoming stack args  │  FP+16, FP+24, …
///   ├─────────────────────┤
///   │ Saved FP (X29)       │  FP+0
///   │ Saved LR (X30)       │  FP+8
///   ├─────────────────────┤
///   │ Callee-saved regs    │  FP-8, FP-16, …
///   │ (X19..X28)           │
///   ├─────────────────────┤
///   │ Local variables      │  from Alloc instructions
///   │ (aligned)            │
///   ├─────────────────────┤
///   │ Outgoing stack args  │  bottom of frame (for calls)
///   │ (for nested calls)   │
///   └─────────────────────┘  ← SP
/// Lower addresses
/// ```
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StackLayout {
    /// Total stack frame size in bytes (always 16-byte aligned).
    pub total_size: usize,
    /// Slots for callee-saved registers.
    pub callee_save_slots: Vec<StackSlot>,
    /// Slots for local variables (from `Alloc` instructions).
    pub local_slots: Vec<StackSlot>,
    /// Slot for outgoing stack arguments to callees (single contiguous area).
    pub outgoing_args_slot: Option<StackSlot>,
    /// Slot for the saved FP (X29) — always at FP+0.
    pub fp_slot: StackSlot,
    /// Slot for the saved LR (X30) — always at FP+8.
    pub lr_slot: StackSlot,
    /// Number of callee-saved registers that are spilled.
    pub callee_saves_count: usize,
}

/// Computes the stack-frame layout for an IR function.
///
/// This scans the function body for `Alloc` instructions (local variables) and
/// `Call` instructions (outgoing stack arguments), and lays out the frame
/// according to AAPCS64 conventions.
///
/// ## Parameters
///
/// - `func`: The IR function to compute the layout for.
/// - `callee_saves_count`: How many callee-saved registers (X19–X28) the
///   function uses.  Set by the register allocator.  Each consumes 8 bytes.
/// - `call_arg_types`: For each `Call` instruction in the function, the
///   argument types.  This is used to compute the outgoing stack-argument
///   area size.  If empty or if calls have no stack args, no outgoing area
///   is needed.
pub fn compute_stack_layout(func: &IRFunction) -> StackLayout {
    compute_stack_layout_with_info(func, 0, &[])
}

/// Computes the stack-frame layout with additional information from the
/// register allocator and call-site type information.
///
/// See [`compute_stack_layout`] for the simplified version.
pub fn compute_stack_layout_with_info(
    func: &IRFunction,
    callee_saves_count: usize,
    call_arg_types: &[Vec<IRType>],
) -> StackLayout {
    // Fixed slots: saved FP at FP+0, saved LR at FP+8.
    let fp_slot = StackSlot {
        name: "saved_fp".to_string(),
        offset: 0,
        size: 8,
        alignment: 8,
    };
    let lr_slot = StackSlot {
        name: "saved_lr".to_string(),
        offset: 8,
        size: 8,
        alignment: 8,
    };

    // Current negative offset from FP, growing downward.
    let mut offset: i32 = 0;

    // --- Callee-saved registers ---
    let mut callee_save_slots = Vec::new();
    for i in 0..callee_saves_count {
        offset -= 8;
        callee_save_slots.push(StackSlot {
            name: format!("callee_save_{}", i),
            offset,
            size: 8,
            alignment: 8,
        });
    }

    // --- Local variables (from Alloc instructions) ---
    let mut local_slots = Vec::new();
    let mut alloc_index = 0;
    for block in &func.blocks {
        for instr in &block.instructions {
            if let IRInstr::Alloc { dst, size } = instr {
                let size = *size as usize;
                let align = if size >= 8 { 8usize } else { size.next_power_of_two() };
                // Align offset.
                offset = (offset - align as i32) & !(align as i32 - 1);
                offset -= size as i32;
                // Re-align offset to the start of the slot.
                offset &= !(align as i32 - 1);
                let name = match dst {
                    IRValue::Register(id) => format!("local_%v{}", id),
                    _ => format!("local_{}", alloc_index),
                };
                local_slots.push(StackSlot {
                    name,
                    offset,
                    size,
                    alignment: align,
                });
                alloc_index += 1;
            }
        }
    }

    // --- Outgoing stack arguments ---
    // Compute the maximum stack-args size across all call sites.
    let mut max_outgoing_stack = 0usize;
    for arg_types in call_arg_types {
        let cc = compute_calling_conv(arg_types, &IRType::Void);
        max_outgoing_stack = max_outgoing_stack.max(cc.stack_args_size);
    }

    let outgoing_args_slot = if max_outgoing_stack > 0 {
        // Align to 16 bytes.
        let size = (max_outgoing_stack + 15) & !15;
        offset -= size as i32;
        Some(StackSlot {
            name: "outgoing_args".to_string(),
            offset,
            size,
            alignment: 16,
        })
    } else {
        None
    };

    // Total frame size: absolute value of the lowest offset, plus 16 for
    // FP/LR pair.  The frame pointer is at the saved-FP position; everything
    // below is at negative offsets from FP.
    let total_raw = ((-offset) as usize) + 16; // 16 = FP + LR
    let total_size = (total_raw + 15) & !15; // 16-byte align.

    StackLayout {
        total_size,
        callee_save_slots,
        local_slots,
        outgoing_args_slot,
        fp_slot,
        lr_slot,
        callee_saves_count,
    }
}

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
    /// Signed less-than comparison.
    SLt,
    /// Signed less-than-or-equal comparison.
    SLe,
    /// Signed greater-than comparison.
    SGt,
    /// Signed greater-than-or-equal comparison.
    SGe,
    /// Unsigned less-than comparison.
    ULt,
    /// Unsigned less-than-or-equal comparison.
    ULe,
    /// Unsigned greater-than comparison.
    UGt,
    /// Unsigned greater-than-or-equal comparison.
    UGe,
    /// Equality comparison.
    Eq,
    /// Inequality comparison.
    Ne,
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
            BinOpKind::SLt => "slt",
            BinOpKind::SLe => "sle",
            BinOpKind::SGt => "sgt",
            BinOpKind::SGe => "sge",
            BinOpKind::ULt => "ult",
            BinOpKind::ULe => "ule",
            BinOpKind::UGt => "ugt",
            BinOpKind::UGe => "uge",
            BinOpKind::Eq => "eq",
            BinOpKind::Ne => "ne",
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

// ---------------------------------------------------------------------------
// VirtualRegister
// ---------------------------------------------------------------------------

/// A named virtual register in SSA form.
///
/// Each virtual register has a unique numeric ID and an optional human-readable
/// name derived from the original source variable.  The ID guarantees uniqueness
/// even when the same source name is reused in different scopes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct VirtualRegister {
    /// Unique numeric identifier.
    pub id: u32,
    /// Optional human-readable name (e.g. `"x"`, `"loop_idx"`).
    pub name: Option<String>,
}

impl VirtualRegister {
    /// Create a new virtual register with the given ID and optional name.
    pub fn new(id: u32, name: Option<String>) -> Self {
        Self { id, name }
    }

    /// Create an anonymous virtual register (no name).
    pub fn anonymous(id: u32) -> Self {
        Self::new(id, None)
    }

    /// Create a named virtual register.
    pub fn named(id: u32, name: impl Into<String>) -> Self {
        Self::new(id, Some(name.into()))
    }

    /// Returns the register ID.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Returns the name, if any.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

impl fmt::Display for VirtualRegister {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            Some(n) => write!(f, "%{}#{}", n, self.id),
            None => write!(f, "%v{}", self.id),
        }
    }
}

// ---------------------------------------------------------------------------
// CmpKind
// ---------------------------------------------------------------------------

/// Comparison operations supported by the IR.
///
/// Each comparison produces a boolean result (1 or 0) stored in the
/// destination register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CmpKind {
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
    /// Signed less-than.
    SLt,
    /// Signed less-than-or-equal.
    SLe,
    /// Signed greater-than.
    SGt,
    /// Signed greater-than-or-equal.
    SGe,
    /// Unsigned less-than.
    ULt,
    /// Unsigned less-than-or-equal.
    ULe,
    /// Unsigned greater-than.
    UGt,
    /// Unsigned greater-than-or-equal.
    UGe,
}

impl fmt::Display for CmpKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CmpKind::Eq => "cmp.eq",
            CmpKind::Ne => "cmp.ne",
            CmpKind::SLt => "cmp.slt",
            CmpKind::SLe => "cmp.sle",
            CmpKind::SGt => "cmp.sgt",
            CmpKind::SGe => "cmp.sge",
            CmpKind::ULt => "cmp.ult",
            CmpKind::ULe => "cmp.ule",
            CmpKind::UGt => "cmp.ugt",
            CmpKind::UGe => "cmp.uge",
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

    /// Conditional select: `dst = if cond != 0 { true_val } else { false_val }`
    Select {
        dst: IRValue,
        cond: IRValue,
        true_val: IRValue,
        false_val: IRValue,
    },

    // ── Dedicated arithmetic instructions ────────────────────────────

    /// Add: `dst = lhs + rhs`
    Add {
        dst: IRValue,
        lhs: IRValue,
        rhs: IRValue,
    },
    /// Subtract: `dst = lhs - rhs`
    Sub {
        dst: IRValue,
        lhs: IRValue,
        rhs: IRValue,
    },
    /// Multiply: `dst = lhs * rhs`
    Mul {
        dst: IRValue,
        lhs: IRValue,
        rhs: IRValue,
    },
    /// Divide: `dst = lhs / rhs`
    Div {
        dst: IRValue,
        lhs: IRValue,
        rhs: IRValue,
    },

    // ── Comparison ──────────────────────────────────────────────────

    /// Comparison: `dst = cmp kind lhs rhs` — produces 1 or 0.
    Cmp {
        kind: CmpKind,
        dst: IRValue,
        lhs: IRValue,
        rhs: IRValue,
    },

    // ── Instruction-level control flow ───────────────────────────────

    /// Return from the current function with optional values.
    Ret {
        values: Vec<IRValue>,
    },
    /// Unconditional branch to a label.
    Branch {
        target: String,
    },
    /// Conditional branch: if `cond` is non-zero, go to `true_target`;
    /// otherwise go to `false_target`.
    CondBranch {
        cond: IRValue,
        true_target: String,
        false_target: String,
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
            | IRInstr::Offset { dst, .. }
            | IRInstr::Select { dst, .. } => dst.as_register().into_iter().collect(),
            IRInstr::Call { dst, .. } => dst.as_ref().and_then(|v| v.as_register()).into_iter().collect(),
            IRInstr::Add { dst, .. }
            | IRInstr::Sub { dst, .. }
            | IRInstr::Mul { dst, .. }
            | IRInstr::Div { dst, .. }
            | IRInstr::Cmp { dst, .. } => dst.as_register().into_iter().collect(),
            IRInstr::Store { .. }
            | IRInstr::Free { .. }
            | IRInstr::Ret { .. }
            | IRInstr::Branch { .. }
            | IRInstr::CondBranch { .. } => vec![],
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
            IRInstr::BinOp { lhs, rhs, .. }
            | IRInstr::Add { lhs, rhs, .. }
            | IRInstr::Sub { lhs, rhs, .. }
            | IRInstr::Mul { lhs, rhs, .. }
            | IRInstr::Div { lhs, rhs, .. }
            | IRInstr::Cmp { lhs, rhs, .. } => {
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
            IRInstr::Select { cond, true_val, false_val, .. } => {
                let mut r = cond.as_register().into_iter().collect::<Vec<_>>();
                r.extend(true_val.as_register());
                r.extend(false_val.as_register());
                r
            }
            IRInstr::Ret { values } => values.iter().filter_map(|v| v.as_register()).collect(),
            IRInstr::Branch { .. } => vec![],
            IRInstr::CondBranch { cond, .. } => cond.as_register().into_iter().collect(),
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
            IRInstr::Select { dst, cond, true_val, false_val } => {
                write!(f, "{} = select {}, {}, {}", dst, cond, true_val, false_val)
            }
            IRInstr::Add { dst, lhs, rhs } => write!(f, "{} = add {}, {}", dst, lhs, rhs),
            IRInstr::Sub { dst, lhs, rhs } => write!(f, "{} = sub {}, {}", dst, lhs, rhs),
            IRInstr::Mul { dst, lhs, rhs } => write!(f, "{} = mul {}, {}", dst, lhs, rhs),
            IRInstr::Div { dst, lhs, rhs } => write!(f, "{} = div {}, {}", dst, lhs, rhs),
            IRInstr::Cmp { kind, dst, lhs, rhs } => {
                write!(f, "{} = {} {}, {}", dst, kind, lhs, rhs)
            }
            IRInstr::Ret { values } => {
                let vals_str = values
                    .iter()
                    .map(|v| format!("{}", v))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "ret {}", vals_str)
            }
            IRInstr::Branch { target } => write!(f, "br @{}", target),
            IRInstr::CondBranch { cond, true_target, false_target } => {
                write!(f, "br {}, @{}, @{}", cond, true_target, false_target)
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
    /// Switch dispatch: branch to one of several targets based on the
    /// discriminator value, or fall through to `default`.
    Switch {
        /// Discriminator value.
        discr: IRValue,
        /// (value, target_label) pairs.
        targets: Vec<(i64, String)>,
        /// Default target if no value matches.
        default: String,
    },
    /// Invoke: call a function that may throw, with separate normal and
    /// unwind continuations.
    Invoke {
        /// Destination register for the return value.
        dst: Option<IRValue>,
        /// Function name.
        func: String,
        /// Arguments.
        args: Vec<IRValue>,
        /// Normal continuation label.
        normal: String,
        /// Unwind (exception) continuation label.
        unwind: String,
    },
    /// Tail call: jump to the callee, reusing the current stack frame.
    TailCall {
        /// Function name.
        func: String,
        /// Arguments.
        args: Vec<IRValue>,
    },
    /// Resume unwinding with the given exception value.
    Resume {
        /// Exception value to resume with.
        value: IRValue,
    },
}

impl IRTerminator {
    /// Returns the labels of all successor blocks referenced by this terminator.
    pub fn successor_labels(&self) -> Vec<&str> {
        match self {
            IRTerminator::Jump(target) => vec![target],
            IRTerminator::Branch {
                true_block,
                false_block,
                ..
            } => vec![true_block, false_block],
            IRTerminator::Return(_) | IRTerminator::Unreachable => vec![],
            IRTerminator::Switch { targets, default, .. } => {
                let mut labels: Vec<&str> = targets.iter().map(|(_, l)| l.as_str()).collect();
                labels.push(default);
                labels
            }
            IRTerminator::Invoke { normal, unwind, .. } => vec![normal, unwind],
            IRTerminator::TailCall { .. } | IRTerminator::Resume { .. } => vec![],
        }
    }
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
            IRTerminator::Switch { discr, targets, default } => {
                let pairs = targets
                    .iter()
                    .map(|(v, l)| format!("{}: @{}", v, l))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "switch {}, [{}] default @{}", discr, pairs, default)
            }
            IRTerminator::Invoke { dst, func, args, normal, unwind } => {
                let args_str = args
                    .iter()
                    .map(|a| format!("{}", a))
                    .collect::<Vec<_>>()
                    .join(", ");
                match dst {
                    Some(d) => write!(f, "invoke {} = @{}({}) normal @{} unwind @{}", d, func, args_str, normal, unwind),
                    None => write!(f, "invoke @{}({}) normal @{} unwind @{}", func, args_str, normal, unwind),
                }
            }
            IRTerminator::TailCall { func, args } => {
                let args_str = args
                    .iter()
                    .map(|a| format!("{}", a))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "tailcall @{}({})", func, args_str)
            }
            IRTerminator::Resume { value } => write!(f, "resume {}", value),
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
    /// Labels of predecessor blocks (populated after CFG construction).
    pub predecessors: HashSet<String>,
    /// Labels of successor blocks (populated after CFG construction).
    pub successors: HashSet<String>,
}

impl IRBlock {
    /// Create a new empty block with the given label and an `Unreachable`
    /// terminator placeholder (callers should replace it).
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            instructions: Vec::new(),
            terminator: IRTerminator::Unreachable,
            predecessors: HashSet::new(),
            successors: HashSet::new(),
        }
    }

    /// Append an instruction to this block.
    pub fn push(&mut self, instr: IRInstr) {
        self.instructions.push(instr);
    }

    /// Returns the number of instructions in this block.
    pub fn len(&self) -> usize {
        self.instructions.len()
    }

    /// Returns `true` if this block has no instructions.
    pub fn is_empty(&self) -> bool {
        self.instructions.is_empty()
    }

    /// Update the successor set from the current terminator.
    pub fn update_successors_from_terminator(&mut self) {
        self.successors.clear();
        for label in self.terminator.successor_labels() {
            self.successors.insert(label.to_string());
        }
    }
}

/// Backward-compatible alias.
pub type BasicBlock = IRBlock;

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
    /// Type of each parameter (parallel to `params`).
    pub param_types: Vec<IRType>,
    /// Type of each return value (parallel to `results`).
    pub result_types: Vec<IRType>,
    /// Named virtual registers used in this function.
    pub vregs: HashMap<u32, VirtualRegister>,
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
            param_types: Vec::new(),
            result_types: Vec::new(),
            vregs: HashMap::new(),
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

    /// Register a named virtual register.
    pub fn register_vreg(&mut self, vreg: VirtualRegister) {
        self.vregs.insert(vreg.id, vreg);
    }

    /// Look up a virtual register by ID.
    pub fn get_vreg(&self, id: u32) -> Option<&VirtualRegister> {
        self.vregs.get(&id)
    }

    /// Rebuild predecessor/successor sets for all blocks from terminators.
    pub fn rebuild_cfg(&mut self) {
        let label_to_idx: HashMap<String, usize> = self
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (b.label.clone(), i))
            .collect();

        for block in &mut self.blocks {
            block.predecessors.clear();
            block.successors.clear();
        }

        // Collect edge data first to avoid borrow conflicts.
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for i in 0..self.blocks.len() {
            self.blocks[i].update_successors_from_terminator();
            let succ_labels: Vec<String> =
                self.blocks[i].successors.iter().cloned().collect();
            for succ_label in succ_labels {
                if let Some(&succ_idx) = label_to_idx.get(&succ_label) {
                    edges.push((i, succ_idx));
                }
            }
        }

        // Now apply predecessor edges.
        let src_labels: Vec<(usize, String)> = self
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (i, b.label.clone()))
            .collect();
        for (src_idx, tgt_idx) in edges {
            self.blocks[tgt_idx]
                .predecessors
                .insert(src_labels[src_idx].1.clone());
        }
    }

    /// Find a block by label, returning its index.
    pub fn find_block_by_label(&self, label: &str) -> Option<usize> {
        self.blocks.iter().position(|b| b.label == label)
    }

    /// Returns the total number of blocks.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Returns the total number of instructions across all blocks.
    pub fn instruction_count(&self) -> usize {
        self.blocks.iter().map(|b| b.instructions.len()).sum()
    }
}

impl fmt::Display for IRFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let params = self
            .params
            .iter()
            .zip(self.param_types.iter())
            .map(|(p, t)| format!("{}: {}", p, t))
            .collect::<Vec<_>>()
            .join(", ");
        let results = self
            .results
            .iter()
            .zip(self.result_types.iter())
            .map(|(r, t)| format!("{}: {}", r, t))
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

    // --- Original tests (preserved) ---

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
        func.param_types.push(IRType::I64);
        func.results.push(IRValue::Register(1));
        func.result_types.push(IRType::I64);

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
        assert!(text.contains("i64"));
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

    // --- Type system tests ---

    #[test]
    fn size_of_primitive_types() {
        assert_eq!(size_of(&IRType::I8), 1);
        assert_eq!(size_of(&IRType::U8), 1);
        assert_eq!(size_of(&IRType::I16), 2);
        assert_eq!(size_of(&IRType::U16), 2);
        assert_eq!(size_of(&IRType::I32), 4);
        assert_eq!(size_of(&IRType::U32), 4);
        assert_eq!(size_of(&IRType::I64), 8);
        assert_eq!(size_of(&IRType::U64), 8);
        assert_eq!(size_of(&IRType::F32), 4);
        assert_eq!(size_of(&IRType::F64), 8);
        assert_eq!(size_of(&IRType::Ptr), 8);
        assert_eq!(size_of(&IRType::Func), 8);
        assert_eq!(size_of(&IRType::Void), 0);
    }

    #[test]
    fn alignment_of_primitive_types() {
        assert_eq!(alignment_of(&IRType::I8), 1);
        assert_eq!(alignment_of(&IRType::I32), 4);
        assert_eq!(alignment_of(&IRType::I64), 8);
        assert_eq!(alignment_of(&IRType::F64), 8);
        assert_eq!(alignment_of(&IRType::Ptr), 8);
        assert_eq!(alignment_of(&IRType::Void), 1);
    }

    #[test]
    fn size_of_struct_with_padding() {
        // struct { i8, i64 } → 1 byte + 7 padding + 8 bytes = 16 bytes
        let s = IRType::Struct {
            name: "Padded".to_string(),
            fields: vec![IRType::I8, IRType::I64],
        };
        assert_eq!(size_of(&s), 16);
        assert_eq!(alignment_of(&s), 8);

        // struct { i64, i8 } → 8 + 1 + 7 padding = 16 bytes
        let s2 = IRType::Struct {
            name: "Padded2".to_string(),
            fields: vec![IRType::I64, IRType::I8],
        };
        assert_eq!(size_of(&s2), 16);

        // struct { i32, i32 } → 4 + 4 = 8 bytes (no padding)
        let s3 = IRType::Struct {
            name: "Compact".to_string(),
            fields: vec![IRType::I32, IRType::I32],
        };
        assert_eq!(size_of(&s3), 8);
    }

    #[test]
    fn size_of_array() {
        // [i32; 4] → 16 bytes
        let a = IRType::Array {
            element: Box::new(IRType::I32),
            count: 4,
        };
        assert_eq!(size_of(&a), 16);
        assert_eq!(alignment_of(&a), 4);

        // [f64; 3] → 24 bytes
        let a2 = IRType::Array {
            element: Box::new(IRType::F64),
            count: 3,
        };
        assert_eq!(size_of(&a2), 24);
        assert_eq!(alignment_of(&a2), 8);
    }

    #[test]
    fn classify_arg_primitives() {
        assert_eq!(classify_arg(&IRType::I32), ArgClass::Integer);
        assert_eq!(classify_arg(&IRType::U64), ArgClass::Integer);
        assert_eq!(classify_arg(&IRType::Ptr), ArgClass::Integer);
        assert_eq!(classify_arg(&IRType::Func), ArgClass::Integer);
        assert_eq!(classify_arg(&IRType::F32), ArgClass::FP);
        assert_eq!(classify_arg(&IRType::F64), ArgClass::FP);
        assert_eq!(classify_arg(&IRType::Void), ArgClass::Integer);
    }

    #[test]
    fn classify_arg_struct_and_hfa() {
        // HFA: struct { f64, f64 } → FP
        let hfa = IRType::Struct {
            name: "Vec2".to_string(),
            fields: vec![IRType::F64, IRType::F64],
        };
        assert!(hfa.is_hfa());
        assert_eq!(classify_arg(&hfa), ArgClass::FP);

        // Non-HFA small struct: struct { i32, i32 } → Integer (≤ 16 bytes)
        let small = IRType::Struct {
            name: "Pair".to_string(),
            fields: vec![IRType::I32, IRType::I32],
        };
        assert!(!small.is_hfa());
        assert_eq!(classify_arg(&small), ArgClass::Integer);

        // Large struct: > 16 bytes → Indirect
        let large = IRType::Struct {
            name: "Big".to_string(),
            fields: vec![IRType::I64; 4], // 32 bytes
        };
        assert_eq!(size_of(&large), 32);
        assert_eq!(classify_arg(&large), ArgClass::Indirect);
    }

    #[test]
    fn compute_calling_conv_simple() {
        // fn(i32, i64, f64) -> i64
        let args = vec![IRType::I32, IRType::I64, IRType::F64];
        let ret = IRType::I64;
        let cc = compute_calling_conv(&args, &ret);

        assert_eq!(cc.arg_locations.len(), 3);
        // i32 → X0
        assert_eq!(cc.arg_locations[0].register, Some((RegisterClass::X, 0)));
        assert_eq!(cc.arg_locations[0].class, ArgClass::Integer);
        // i64 → X1
        assert_eq!(cc.arg_locations[1].register, Some((RegisterClass::X, 1)));
        // f64 → V0
        assert_eq!(cc.arg_locations[2].register, Some((RegisterClass::V, 0)));
        assert_eq!(cc.arg_locations[2].class, ArgClass::FP);
        // Return in X0
        assert_eq!(cc.ret_location.registers, vec![(RegisterClass::X, 0)]);
        // No stack args
        assert_eq!(cc.stack_args_size, 0);
    }

    #[test]
    fn compute_calling_conv_stack_overflow() {
        // 10 integer args → X0–X7 + 2 on stack
        let args: Vec<IRType> = (0..10).map(|_| IRType::I64).collect();
        let cc = compute_calling_conv(&args, &IRType::Void);

        assert_eq!(cc.arg_locations.len(), 10);
        // First 8 in X0–X7
        for i in 0..8 {
            assert_eq!(
                cc.arg_locations[i].register,
                Some((RegisterClass::X, i as u32))
            );
        }
        // 9th and 10th on stack
        assert_eq!(cc.arg_locations[8].register, None);
        assert_eq!(cc.arg_locations[8].class, ArgClass::Stack);
        assert_eq!(cc.arg_locations[8].stack_offset, Some(0));
        assert_eq!(cc.arg_locations[9].stack_offset, Some(8));
        // Stack args size: 16 bytes (2 * 8, rounded up to 16)
        assert_eq!(cc.stack_args_size, 16);
    }

    #[test]
    fn compute_calling_conv_hfa_return() {
        // Return HFA struct { f32, f32, f32, f32 } → V0–V3
        let hfa_ret = IRType::Struct {
            name: "Vec4".to_string(),
            fields: vec![IRType::F32, IRType::F32, IRType::F32, IRType::F32],
        };
        let cc = compute_calling_conv(&[], &hfa_ret);
        assert_eq!(
            cc.ret_location.registers,
            vec![
                (RegisterClass::V, 0),
                (RegisterClass::V, 1),
                (RegisterClass::V, 2),
                (RegisterClass::V, 3),
            ]
        );
        assert_eq!(cc.ret_location.class, ArgClass::FP);
    }

    #[test]
    fn compute_calling_conv_large_struct_return() {
        // Return struct > 16 bytes → indirect via X8
        let large = IRType::Struct {
            name: "BigRet".to_string(),
            fields: vec![IRType::I64; 4],
        };
        let cc = compute_calling_conv(&[IRType::I32], &large);
        assert_eq!(cc.ret_location.class, ArgClass::Indirect);
        assert_eq!(cc.ret_location.registers, vec![(RegisterClass::X, 8)]);
    }

    #[test]
    fn compute_stack_layout_basic() {
        let mut func = IRFunction::new("test");
        func.current_block().push(IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 32,
        });
        func.current_block().push(IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 8,
        });
        func.current_block().terminator = IRTerminator::Return(vec![]);

        let layout = compute_stack_layout(&func);

        // Should have 2 local slots
        assert_eq!(layout.local_slots.len(), 2);
        // Total size must be 16-byte aligned
        assert_eq!(layout.total_size % 16, 0);
        // FP/LR slots at fixed positions
        assert_eq!(layout.fp_slot.offset, 0);
        assert_eq!(layout.lr_slot.offset, 8);
    }

    #[test]
    fn compute_stack_layout_with_callee_saves() {
        let func = IRFunction::new("test_callee");
        let layout = compute_stack_layout_with_info(&func, 4, &[]);

        // 4 callee-saved registers
        assert_eq!(layout.callee_save_slots.len(), 4);
        assert_eq!(layout.callee_saves_count, 4);
        // Total size includes: 16 (FP/LR) + 32 (4 * 8 callee saves) = 48, rounded to 48
        assert_eq!(layout.total_size, 48);
        // Callee-save offsets are negative from FP
        assert!(layout.callee_save_slots[0].offset < 0);
    }

    #[test]
    fn compute_stack_layout_with_outgoing_args() {
        let func = IRFunction::new("caller");
        // Simulate a call with 10 integer arguments (2 on stack)
        let call_args: Vec<IRType> = (0..10).map(|_| IRType::I64).collect();
        let layout = compute_stack_layout_with_info(&func, 0, &[call_args]);

        assert!(layout.outgoing_args_slot.is_some());
        let slot = layout.outgoing_args_slot.unwrap();
        assert_eq!(slot.size, 16); // 2 stack args * 8, rounded to 16
        assert_eq!(slot.alignment, 16);
    }

    #[test]
    fn irtype_display() {
        assert_eq!(format!("{}", IRType::I32), "i32");
        assert_eq!(format!("{}", IRType::F64), "f64");
        assert_eq!(format!("{}", IRType::Ptr), "ptr");
        assert_eq!(format!("{}", IRType::Void), "void");

        let s = IRType::Struct {
            name: "Point".to_string(),
            fields: vec![IRType::F64, IRType::F64],
        };
        assert_eq!(format!("{}", s), "struct Point { f64, f64 }");

        let a = IRType::Array {
            element: Box::new(IRType::I32),
            count: 4,
        };
        assert_eq!(format!("{}", a), "[i32; 4]");
    }

    #[test]
    fn irtype_helpers() {
        assert!(IRType::I32.is_integer());
        assert!(IRType::U64.is_integer());
        assert!(!IRType::F64.is_integer());
        assert!(IRType::F32.is_float());
        assert!(!IRType::I32.is_float());
        assert!(IRType::Ptr.is_register_passable());
        assert!(IRType::F64.is_register_passable());

        // HFA detection
        let hfa = IRType::Struct {
            name: "Triplet".to_string(),
            fields: vec![IRType::F64, IRType::F64, IRType::F64],
        };
        assert!(hfa.is_hfa());
        assert_eq!(hfa.hfa_info(), Some((&IRType::F64, 3)));

        let not_hfa = IRType::Struct {
            name: "Mixed".to_string(),
            fields: vec![IRType::F64, IRType::I32],
        };
        assert!(!not_hfa.is_hfa());
    }
}
