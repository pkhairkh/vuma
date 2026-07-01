//! # SCG → IR Conversion
//!
//! Translates a Semantic Computation Graph (SCG) — produced by the `vuma-scg`
//! crate — into the intermediate representation defined in [`crate::ir`].
//!
//! ## Architecture
//!
//! [`IRBuilder`] is the main entry point. It holds translation state (virtual-
//! register counter, label counter, loop stack for break/continue, variable
//! tracking for phi insertion, etc.) and lowers SCG nodes into IR in
//! topological order.
//!
//! ## SCG Node → IR Mapping
//!
//! | SCG Node              | IR Mapping                                           |
//! |-----------------------|------------------------------------------------------|
//! | `ControlNode::If`     | `CondBranch` + then/else/merge blocks + phi nodes    |
//! | `ControlNode::Loop`   | Loop header with phi nodes + back-edge + exit block  |
//! | `ControlNode::Break`  | `Branch` to loop exit                                |
//! | `ControlNode::Continue` | `Branch` to loop header                           |
//! | `ControlNode::Switch` | Cascading `Cmp`+`CondBranch` for each arm + merge   |
//! | `AllocationNode::Stack` | `Alloc` + stack slot registration                  |
//! | `AllocationNode::Heap`  | `Call` to `__vuma_alloc`                           |
//! | `AccessNode::Load`    | Optional `Offset` + `Load`                           |
//! | `AccessNode::Store`   | Optional `Offset` + `Store`                          |
//! | `CastNode`            | `Cast` (zext / sext / trunc / bitcast)               |
//! | `ComputationNode`     | `Add`/`Sub`/`Mul`/`Div`/`Cmp`/`BinOp`/`UnaryOp`     |
//! | `UnaryComputationNode`| `UnaryOp` (neg / not / clz / ctz / popcnt)           |
//! | `CallNode`            | `Call`                                               |
//! | `StructAccessNode`    | Offset-based `Load`/`Store` at `base + field_offset` |
//! | `EnumAccessNode`      | Tag `Load`/`Store` at offset 0; payload at offset N  |
//! | `Return`              | `Ret` + `IRTerminator::Return`                       |
//!
//! ## Struct and Enum Lowering
//!
//! **Structs** are lowered to flat memory layouts. Field N is stored at
//! `base_ptr + offset_N` where offsets are computed during layout resolution.
//! `StructAccessNode::Load` lowers to `Load { addr: ptr, offset: field_offset }`
//! and `StructAccessNode::Store` lowers to `Store { addr: ptr, offset: field_offset }`.
//!
//! **Enums** are lowered to tagged unions: a discriminant (tag) at offset 0
//! followed by a payload at offset `tag_size` (aligned). `EnumAccessNode::LoadTag`
//! reads the tag (offset 0), and `EnumAccessNode::LoadPayload` reads the
//! payload at its computed offset.
//!
//! **Match** expressions are lowered to if/else chains via `ControlNode::Switch`:
//! for each arm, a `Cmp { kind: Eq }` compares the discriminant against the
//! expected tag value, and a `CondBranch` dispatches to the arm body.
//!
//! ## Control Flow
//!
//! - **if/else**: Condition evaluated, `CondBranch` to then/else blocks,
//!   merge block with phi nodes for variables modified in either branch.
//! - **loop**: Loop header block with phi nodes for loop-carried values,
//!   back-edge from loop body end to header, exit block.
//! - **break**: Jump to the loop's exit label (via loop stack).
//! - **continue**: Jump to the loop's header label (via loop stack).
//!
//! ## Topological Ordering
//!
//! Within a function body, SCG statements are lowered in their declared
//! order.  For graph-based SCGs (from the `vuma-scg` crate), the
//! [`IRBuilder::build`] method walks the SCG in topological order
//! using petgraph's `toposort`, ensuring that data-flow dependencies are
//! respected.

use crate::ir::*;
use crate::Result;
use std::collections::{HashMap, HashSet};

/// Convenience alias used throughout this module.
type IRInstruction = IRInstr;

// ---------------------------------------------------------------------------
// SCG Node stubs
// ---------------------------------------------------------------------------
// NOTE: The real SCG types live in the `vuma-scg` crate.  We define
// lightweight stubs here so this crate compiles independently.  When the
// full SCG crate is available, these can be replaced with re-exports or
// converted to trait-based dispatch.

/// Placeholder for the SCG graph type from `vuma-scg`.
#[derive(Debug, Clone)]
pub struct Scg {
    /// Top-level nodes in the SCG.
    pub nodes: Vec<ScgNode>,
}

/// A single node in the SCG.
#[derive(Debug, Clone)]
pub enum ScgNode {
    /// A function definition.
    Function(ScgFunction),
    /// A data declaration.
    Data(ScgData),
}

/// An SCG function node.
#[derive(Debug, Clone)]
pub struct ScgFunction {
    /// Function name.
    pub name: String,
    /// Parameter names / types.
    pub params: Vec<ScgParam>,
    /// Result types.
    pub results: Vec<ScgType>,
    /// Body — a list of SCG statements.
    pub body: Vec<ScgStatement>,
}

/// An SCG function parameter.
#[derive(Debug, Clone)]
pub struct ScgParam {
    /// Parameter name.
    pub name: String,
    /// Parameter type.
    pub ty: ScgType,
}

/// A lightweight type representation in the SCG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScgType {
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
    /// Pointer-sized unsigned integer.
    Ptr,
    /// No value (unit type).
    Void,
    /// IEEE 754 single-precision floating-point.
    F32,
    /// IEEE 754 double-precision floating-point.
    F64,
}

impl ScgType {
    /// Convert an SCG type to the corresponding IR type.
    pub fn to_ir_type(&self) -> IRType {
        match self {
            ScgType::I8 => IRType::I8,
            ScgType::I16 => IRType::I16,
            ScgType::I32 => IRType::I32,
            ScgType::I64 => IRType::I64,
            ScgType::U8 => IRType::U8,
            ScgType::U16 => IRType::U16,
            ScgType::U32 => IRType::U32,
            ScgType::U64 => IRType::U64,
            ScgType::Ptr => IRType::Ptr,
            ScgType::Void => IRType::Void,
            ScgType::F32 => IRType::F32,
            ScgType::F64 => IRType::F64,
        }
    }
}

/// An SCG statement — the body of a function.
#[derive(Debug, Clone)]
pub enum ScgStatement {
    /// Control flow: if / loop / match.
    Control(ControlNode),
    /// Memory allocation.
    Allocation(AllocationNode),
    /// Memory access (read / write).
    Access(AccessNode),
    /// Type cast / reinterpret.
    Cast(CastNode),
    /// Binary arithmetic / logic.
    Computation(ComputationNode),
    /// Unary operation (neg, not, clz, ctz, popcnt).
    UnaryComputation(UnaryComputationNode),
    /// Function call.
    Call(CallNode),
    /// Return from function.
    Return(Vec<ScgExpr>),
    /// Constant-time security operation (ct_select, ct_eq).
    ConstantTime(ConstantTimeStatement),
    /// Struct field access: read/write a field from a flat memory layout.
    StructAccess(StructAccessNode),
    /// Enum tag access: read the discriminant or payload of a tagged union.
    EnumAccess(EnumAccessNode),
    /// Compute the address of a named symbol (function or data).
    /// Lowers to `IRInstr::GetAddress`.
    GetAddress(GetAddressNode),
}

/// Control-flow node.
#[derive(Debug, Clone)]
pub enum ControlNode {
    /// `if cond { then } else { else_ }`
    If {
        /// The condition expression.
        cond: ScgExpr,
        /// Statements in the then-branch.
        then_body: Vec<ScgStatement>,
        /// Optional else-branch statements.
        else_body: Option<Vec<ScgStatement>>,
    },
    /// `loop { body }`
    Loop {
        body: Vec<ScgStatement>,
        for_range: Option<(String, ScgExpr, ScgExpr)>,
        while_cond: Option<String>,
    },
    /// `break` (from inside a loop).
    Break,
    /// `continue` (from inside a loop).
    Continue,
    /// `switch discriminant { case value => body, .. default => body }`
    ///
    /// Lowers to a sequence of compare-and-branch instructions for small
    /// switch ranges, or a jump table for dense contiguous ranges.
    Switch {
        /// The discriminant expression being switched on.
        discriminant: ScgExpr,
        /// The switch arms: each arm has a value and a body.
        arms: Vec<SwitchArm>,
        /// The default arm (always present — like a match expression).
        default_body: Vec<ScgStatement>,
    },
}

/// A single arm of a switch expression.
#[derive(Debug, Clone)]
pub struct SwitchArm {
    /// The integer value this arm matches.
    pub value: i64,
    /// The body statements for this arm.
    pub body: Vec<ScgStatement>,
}

/// Allocation node — reserves memory.
#[derive(Debug, Clone)]
pub enum AllocationNode {
    /// Stack allocation (fixed size).
    Stack {
        /// Name of the allocated variable.
        name: String,
        /// Size in bytes.
        size: u32,
        /// Type of the allocation.
        ty: ScgType,
    },
    /// Heap allocation (dynamic size, calls allocator).
    Heap {
        /// Name of the allocated variable.
        name: String,
        /// Expression computing the allocation size.
        size_expr: ScgExpr,
        /// Type of the allocation.
        ty: ScgType,
    },
}

/// Memory access node.
#[derive(Debug, Clone)]
pub enum AccessNode {
    /// Read: `dst = *ptr` or `dst = ptr.field`
    Load {
        /// Destination variable name.
        dst: String,
        /// Pointer expression to read from.
        ptr: ScgExpr,
        /// Optional byte offset from the pointer.
        offset: Option<ScgExpr>,
        /// Optional load type override. When None, the IR builder
        /// determines the type (U8 for byte loads, U64 for pointer
        /// loads). When Some, the specified type is used directly.
        ty: Option<crate::ir::IRType>,
    },
    /// Write: `*ptr = val` or `ptr.field = val`
    Store {
        /// Pointer expression to write to.
        ptr: ScgExpr,
        /// Optional byte offset from the pointer.
        offset: Option<ScgExpr>,
        /// Value expression to store.
        value: ScgExpr,
        /// Optional store type override. When None, defaults to U8 for
        /// non-pointer values and U64 for pointer values.
        ty: Option<crate::ir::IRType>,
    },
}

/// Cast / reinterpret node.
#[derive(Debug, Clone)]
pub struct CastNode {
    /// Destination variable name.
    pub dst: String,
    /// Source expression.
    pub src: ScgExpr,
    /// Cast kind.
    pub kind: CastKind,
    /// Source type.
    pub from_ty: ScgType,
    /// Target type.
    pub to_ty: ScgType,
}

/// Computation node (binary arithmetic / logic).
#[derive(Debug, Clone)]
pub struct ComputationNode {
    /// Destination variable name (SCG node id, e.g. "v_5").
    pub dst: String,
    /// Binary operation.
    pub op: BinOpKind,
    /// Left-hand side expression.
    pub lhs: ScgExpr,
    /// Right-hand side expression.
    pub rhs: ScgExpr,
    /// Whether this is a tail call.
    pub tail_call: bool,
    /// For reassignments ("x = expr"), the user-visible variable name
    /// being reassigned (e.g. "x").  This lets `lower_computation`
    /// update the variable's entry in the `names` map (in addition to
    /// the SCG-node-id entry `dst`) so that `lower_if` can detect the
    /// reassignment and create a proper phi node at if/else merge points.
    /// `None` for let-bindings and non-assignment computations.
    pub reassigns: Option<String>,
    /// Optional result type from the SCG node's result_type field.
    /// When set, overrides lhs-based type inference for BinOp width.
    pub result_ty: Option<crate::ir::IRType>,
}

/// Unary computation node (neg, not, clz, ctz, popcnt).
#[derive(Debug, Clone)]
pub struct UnaryComputationNode {
    /// Destination variable name.
    pub dst: String,
    /// The unary operation.
    pub op: UnaryOpKind,
    /// The operand expression.
    pub operand: ScgExpr,
    /// Whether this is a tail call.
    pub tail_call: bool,
}

/// Function call node.
#[derive(Debug, Clone)]
pub struct CallNode {
    /// Optional destination variable for the return value.
    pub dst: Option<String>,
    /// Function name to call.
    pub func: String,
    /// Argument expressions.
    pub args: Vec<ScgExpr>,
    /// Whether this is a call to an extern (foreign) function.
    /// When true, the backend should emit a relocation instead of a local branch.
    pub is_extern: bool,
    /// Optional user-visible variable name being assigned (e.g., "out" in
    /// `out = atomic_load(...)`). When set, lower_call also registers this
    /// name in the names map so resolve_expr can find it.
    pub reassigns: Option<String>,
}

/// A simple expression in the SCG.
#[derive(Debug, Clone)]
pub enum ScgExpr {
    /// A named variable / virtual register.
    Var(String),
    /// An integer literal.
    Int(i64),
    /// A floating-point literal.
    Float(f64),
    /// A symbolic label reference.
    Label(String),
    /// A binary operation: lhs op rhs
    BinOp {
        op: BinOpKind,
        lhs: Box<ScgExpr>,
        rhs: Box<ScgExpr>,
    },
    /// A memory load: *addr (dereference)
    /// The address expression is resolved to a vreg, then a Load
    /// IR instruction is emitted to read from that address.
    Load {
        addr: Box<ScgExpr>,
    },
}

/// Constant-time operation statement.
///
/// These operations are guaranteed to execute in constant time (no
/// data-dependent branches or memory accesses) to prevent timing
/// side-channel attacks.
#[derive(Debug, Clone)]
pub struct ConstantTimeStatement {
    /// The constant-time operation kind.
    pub op: ConstantTimeOpKind,
    /// Destination variable name.
    pub dst: String,
    /// Operand variable names or expressions.
    pub operands: Vec<ScgExpr>,
    /// Type of the result.
    pub ty: ScgType,
}

/// Kinds of constant-time operations in the SCG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstantTimeOpKind {
    /// Constant-time conditional select: `ct_select(cond, a, b)`.
    /// Returns `a` if `cond != 0`, else `b`, without branching.
    CtSelect,
    /// Constant-time equality check: `ct_eq(a, b)`.
    /// Returns 1 if `a == b`, else 0, without branching.
    CtEq,
}

/// Struct field access node.
///
/// Reads or writes a field from a struct stored in flat memory.
/// The field's byte offset is computed from the struct layout.
#[derive(Debug, Clone)]
pub enum StructAccessNode {
    /// Read a struct field: `dst = ptr.field_name`
    Load {
        /// Destination variable name.
        dst: String,
        /// Base pointer expression (start of the struct in memory).
        ptr: ScgExpr,
        /// Byte offset of the field within the struct.
        field_offset: u32,
        /// Type of the field being loaded.
        field_ty: ScgType,
    },
    /// Write a struct field: `ptr.field_name = value`
    Store {
        /// Base pointer expression (start of the struct in memory).
        ptr: ScgExpr,
        /// Byte offset of the field within the struct.
        field_offset: u32,
        /// Value expression to store.
        value: ScgExpr,
        /// Type of the field being stored.
        field_ty: ScgType,
    },
}

/// Enum (tagged union) access node.
///
/// Reads the discriminant tag or the variant payload from a tagged
/// union stored in memory.
#[derive(Debug, Clone)]
pub enum EnumAccessNode {
    /// Read the discriminant tag: `dst = ptr.tag`
    LoadTag {
        /// Destination variable name.
        dst: String,
        /// Base pointer expression (start of the tagged union in memory).
        ptr: ScgExpr,
        /// Type of the tag (typically u32).
        tag_ty: ScgType,
    },
    /// Write the discriminant tag: `ptr.tag = value`
    StoreTag {
        /// Base pointer expression.
        ptr: ScgExpr,
        /// Tag value expression.
        value: ScgExpr,
        /// Type of the tag.
        tag_ty: ScgType,
    },
    /// Read the payload at a given offset: `dst = ptr.payload`
    LoadPayload {
        /// Destination variable name.
        dst: String,
        /// Base pointer expression.
        ptr: ScgExpr,
        /// Byte offset of the payload from the start of the tagged union.
        payload_offset: u32,
        /// Type of the payload being loaded.
        payload_ty: ScgType,
    },
    /// Write the payload at a given offset: `ptr.payload = value`
    StorePayload {
        /// Base pointer expression.
        ptr: ScgExpr,
        /// Byte offset of the payload.
        payload_offset: u32,
        /// Value expression to store.
        value: ScgExpr,
        /// Type of the payload being stored.
        payload_ty: ScgType,
    },
}

/// Compute the address of a named symbol: `dst = getaddress name`
///
/// This is produced when the source contains `@function_name` (address-of
/// a function) or similar symbol-reference expressions.  It lowers to
/// `IRInstr::GetAddress`, which the backend emits as a `mov rax, imm64`
/// with an `R_X86_64_64` relocation (on x86_64) or equivalent on other
/// targets.
#[derive(Debug, Clone)]
pub struct GetAddressNode {
    /// Destination variable name.
    pub dst: String,
    /// Symbol name whose address is being taken.
    pub name: String,
}

/// SCG data declaration.
#[derive(Debug, Clone)]
pub struct ScgData {
    /// Section name.
    pub name: String,
    /// Kind of data section.
    pub kind: DataSectionKind,
    /// Alignment in bytes.
    pub align: u32,
    /// Raw data bytes.
    pub data: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Loop context — used to support break / continue
// ---------------------------------------------------------------------------

/// Information about an enclosing loop, pushed onto the IRBuilder's loop stack.
#[derive(Debug, Clone)]
struct LoopContext {
    /// Label of the loop header block (target for `continue` in while-loops).
    header_label: String,
    /// Label of the loop exit block (target for `break`).
    exit_label: String,
    /// Label of the continue target (back-edge block with increment).
    /// For for-loops, this is the block containing the increment.
    /// For while-loops, this is the same as header_label.
    /// None if not yet set (continue will use header_label as fallback).
    continue_target: Option<String>,
    /// Variable snapshots from each break path: (break_block_label, names_at_break).
    /// Used to create phi nodes at the loop exit that merge values from
    /// all break paths with the normal (fall-through) exit path.
    break_snapshots: Vec<(String, HashMap<String, u32>)>,
}

// ---------------------------------------------------------------------------
// Variable definition tracking — for phi node insertion
// ---------------------------------------------------------------------------

/// Tracks which names were defined (written to) in a particular scope
/// (e.g., the then-branch vs the else-branch of an if/else).  Used to
/// determine which phi nodes are needed at a merge point.
#[derive(Debug, Clone)]
struct VarDefs {
    /// Map from variable name to the vreg ID that was assigned in this scope.
    defs: HashMap<String, u32>,
}

impl VarDefs {
    fn new() -> Self {
        Self {
            defs: HashMap::new(),
        }
    }

    /// Record that `name` was assigned to `vreg` in this scope.
    fn define(&mut self, name: &str, vreg: u32) {
        self.defs.insert(name.to_string(), vreg);
    }

    /// Check if `name` was defined in this scope.
    fn is_defined(&self, name: &str) -> bool {
        self.defs.contains_key(name)
    }

    /// Get the vreg for `name` if it was defined in this scope.
    fn get(&self, name: &str) -> Option<u32> {
        self.defs.get(name).copied()
    }

    /// Return the set of variable names defined in this scope.
    fn defined_names(&self) -> HashSet<String> {
        self.defs.keys().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// IRBuilder — the main converter
// ---------------------------------------------------------------------------

/// Builds IR from SCG in topological order.
///
/// `IRBuilder` holds all state needed to translate an SCG into an
/// [`IRProgram`]: virtual-register and label counters, a name-to-vreg map,
/// a loop stack for break/continue resolution, and variable-definition
/// tracking for phi node insertion at merge points.
///
/// # Example (conceptual)
///
/// ```ignore
/// use vuma_codegen::scg_to_ir::{IRBuilder, Scg};
///
/// let scg: Scg = /* … */;
/// let mut builder = IRBuilder::new();
/// let ir_program = builder.build(&scg)?;
/// ```
pub struct IRBuilder {
    /// Monotonically increasing virtual-register ID counter.
    next_vreg: u32,
    /// Monotonically increasing label counter (for generating unique names).
    next_label: u32,
    /// Stack of enclosing loops, for break/continue resolution.
    loop_stack: Vec<LoopContext>,
    /// Map from synthetic "v_N" names to user-visible variable names.
    /// Populated by lower_computation when reassigns is set.
    /// Used by resolve_expr as a fallback when Var("v_N") is not in names.
    vreg_aliases: std::collections::HashMap<String, String>,
    /// Set of vreg IDs that hold pointers (from allocate() or
    /// function parameters of type Address). Used to determine Store/Load
    /// width: pointer stores need U64 (64-bit), byte stores use U8.
    pointer_vregs: std::collections::HashSet<u32>,
    /// Map from parameter name to IR type. Populated in lower_function,
    /// used in lower_access to determine Store/Load width.
    param_types: std::collections::HashMap<String, crate::ir::IRType>,
    /// Number of Load statements in the current function. Used by
    /// lower_access to decide whether to use the function's return type
    /// for load width inference. When a function has exactly ONE load
    /// that flows directly to the return value (e.g. `fn read() -> u32 {
    /// return *ptr; }`), using the return type is safe and correct.
    /// When a function has MULTIPLE loads (e.g. byte-level access that
    /// combines several bytes), using the return type is wrong because
    /// each load should be U8 (byte) not U32/U64 (word).
    /// This is especially important on big-endian backends (ppc64) where
    /// a U32 load of a U8-stored value reads the wrong byte position.
    load_count: usize,
    /// Number of Store statements in the current function. Used together
    /// with load_count to decide whether to use the return type for load
    /// width inference. Only applies when load_count == 1 AND store_count == 0
    /// (read-only functions where the single load flows to the return).
    store_count: usize,
    /// Number of Cmp statements in the current function. Used to distinguish
    /// read-only functions that directly return a load (mat_read: no Cmp)
    /// from those that use the load in a comparison (verify_buf: has Cmp).
    /// Only the former should use the return type for load width.
    cmp_count: usize,
    /// The return type of the current function, parsed from the function
    /// name (e.g. "fn_main_entry(u64)" → U64). Used by lower_access for
    /// load type inference (see load_count). Stored separately from
    /// ir_func.result_types to avoid breaking wasm32 (which uses
    /// result_types for the wasm function signature).
    current_return_type: Option<crate::ir::IRType>,
    /// Map from vreg ID to its IR type. Populated when a variable is
    /// declared with a type annotation (e.g. `crc: u32 = 0`) or when a
    /// parameter is typed. Used by lower_computation to set the `ty`
    /// field on BinOp IR instructions so backends can use the correct
    /// instruction width (32-bit vs 64-bit shifts, etc.).
    /// This is critical for ppc64/riscv64 where a 64-bit logical right
    /// shift on a 32-bit value with garbage in the upper bits corrupts
    /// the result.
    vreg_types: std::collections::HashMap<u32, crate::ir::IRType>,
}

/// Backward-compatible alias.
pub type ScgToIr = IRBuilder;

impl IRBuilder {
    /// Create a new builder with fresh counters.
    pub fn new() -> Self {
        Self {
            next_vreg: 0,
            next_label: 0,
            loop_stack: Vec::new(),
            vreg_aliases: std::collections::HashMap::new(),
            pointer_vregs: std::collections::HashSet::new(),
            param_types: std::collections::HashMap::new(),
            load_count: 0,
            store_count: 0,
            cmp_count: 0,
            current_return_type: None,
            vreg_types: std::collections::HashMap::new(),
        }
    }

    /// Build an IR program from a full SCG.
    ///
    /// This is the primary entry point. It iterates over SCG top-level nodes
    /// (functions and data sections) in order and lowers each one.
    pub fn build(&mut self, scg: &Scg) -> Result<IRProgram> {
        let mut program = IRProgram::new();

        for node in &scg.nodes {
            match node {
                ScgNode::Function(func) => {
                    let ir_func = self.lower_function(func)?;
                    program.functions.push(ir_func);
                }
                ScgNode::Data(data) => {
                    program.data_sections.push(DataSection {
                        name: data.name.clone(),
                        kind: data.kind,
                        align: data.align,
                        data: data.data.clone(),
                    });
                }
            }
        }

        Ok(program)
    }

    /// Backward-compatible alias for [`Self::build`].
    pub fn convert(&mut self, scg: &Scg) -> Result<IRProgram> {
        self.build(scg)
    }

    // =======================================================================
    // Function lowering
    // =======================================================================

    /// Lower a single SCG function to an IR function.
    ///
    /// Creates an entry basic block, maps parameters to virtual registers
    /// (with proper IR types), allocates result registers, lowers the body
    /// statements, and rebuilds the CFG predecessor/successor sets.
    fn lower_function(&mut self, func: &ScgFunction) -> Result<IRFunction> {
        self.param_types.clear();
        self.vreg_types.clear();
        // Count the number of Load statements in this function's body.
        // Used by lower_access to decide whether to infer load width from
        // the function's return type (safe only for single-load functions
        // where the load flows directly to the return).
        self.load_count = Self::count_loads(&func.body);
        self.store_count = Self::count_stores(&func.body);
        self.cmp_count = Self::count_cmps(&func.body);
        let mut ir_func = IRFunction::new(&func.name);

        // Map parameters to virtual registers with proper types.
        let mut name_to_vreg = HashMap::new();
        for param in &func.params {
            let vreg_id = self.alloc_vreg();
            let vreg = VirtualRegister::named(vreg_id, &param.name);
            ir_func.params.push(IRValue::Register(vreg_id));
            ir_func.param_types.push(param.ty.to_ir_type());
            ir_func.register_vreg(vreg);
            name_to_vreg.insert(param.name.clone(), vreg_id);
            // Mark pointer parameters for Store/Load width
            if param.ty == ScgType::Ptr {
                self.pointer_vregs.insert(vreg_id);
            }
            // Record param type for Store/Load width inference
            self.param_types.insert(param.name.clone(), param.ty.to_ir_type());
            // Record vreg type for BinOp width inference (32-bit shifts, etc.)
            self.vreg_types.insert(vreg_id, param.ty.to_ir_type().clone());
        }

        // Map result registers with proper types.
        for (i, ty) in func.results.iter().enumerate() {
            let vreg_id = self.alloc_vreg();
            let vreg = VirtualRegister::named(vreg_id, format!("ret_{}", i));
            ir_func.results.push(IRValue::Register(vreg_id));
            ir_func.result_types.push(ty.to_ir_type());
            ir_func.register_vreg(vreg);
        }

        // Parse the return type from the function name (e.g.
        // "fn_main_entry(u64)" → U64) and store it in self.current_return_type.
        // This is used by lower_access to infer load width for single-load
        // functions (load_count == 1), which is critical for big-endian
        // backends (ppc64) where U8 store + U32 load reads the wrong byte.
        //
        // We DON'T populate ir_func.result_types here because the wasm32
        // backend uses result_types to build the wasm function signature,
        // and wasm32 stores return values in memory (not on the wasm stack).
        // Adding result_types would cause "type mismatch: expected i32 but
        // nothing on stack" errors.
        self.current_return_type = None;
        if let Some(open) = func.name.rfind('(') {
            if let Some(close) = func.name.rfind(')') {
                if close > open {
                    let ret_ty_str = &func.name[open + 1..close];
                    if !ret_ty_str.is_empty() && ret_ty_str != "void" {
                        self.current_return_type = match ret_ty_str {
                            "u8" | "U8" => Some(crate::ir::IRType::U8),
                            "u16" | "U16" => Some(crate::ir::IRType::U16),
                            "u32" | "U32" => Some(crate::ir::IRType::U32),
                            "u64" | "U64" => Some(crate::ir::IRType::U64),
                            "i8" | "I8" => Some(crate::ir::IRType::I8),
                            "i16" | "I16" => Some(crate::ir::IRType::I16),
                            "i32" | "I32" => Some(crate::ir::IRType::I32),
                            "i64" | "I64" => Some(crate::ir::IRType::I64),
                            _ => None,
                        };
                    }
                }
            }
        }

        // Pre-pass: register variable names that are *used* in the body but
        // not *defined* anywhere within it (and not already populated as a
        // parameter).  These are typically cross-function DataFlow references
        // -- bridge artifacts where the defining node was emitted in a
        // different function (the bridge's "remaining nodes" cleanup creates
        // a separate synthetic `main` for unconsumed nodes).  We allocate a
        // virtual register for each so that `resolve_expr` succeeds.  The
        // vreg is uninitialized (undefined value) -- this is the *correct*
        // semantics for an undefined variable, NOT a silent substitution of 0
        // (which would mask the bug and produce a wrong binary).
        let (all_defs, all_uses) = Self::collect_defs_uses(&func.body);
        for name in &all_uses {
            // Only pre-register *synthetic* bridge-generated names of the
            // form `v_<node_id>`.  User-visible names (parameters, locals,
            // test fixtures such as `undefined_var`) are NOT pre-registered:
            // if they are genuinely undefined, `resolve_expr` must still
            // return `UnknownVariable` (Wave 1-b hard-error semantics).
            if !all_defs.contains(name)
                && !name_to_vreg.contains_key(name)
                && Self::is_synthetic_scg_var(name)
            {
                let vreg_id = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::named(vreg_id, name));
                name_to_vreg.insert(name.clone(), vreg_id);
            }
        }

        // Translate the body statements.
        self.lower_statements(&func.body, &mut ir_func, &mut name_to_vreg)?;

        // Resolve phi nodes into explicit copy instructions.
        self.resolve_phis(&mut ir_func)?;

        // Rebuild the CFG (predecessor/successor sets).
        ir_func.rebuild_cfg();

        Ok(ir_func)
    }

    // =======================================================================
    // Statement lowering
    // =======================================================================

    /// Lower a list of SCG statements, appending IR instructions to the
    /// current block of `ir_func`.
    fn lower_statements(
        &mut self,
        stmts: &[ScgStatement],
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        // Check if any statement is a control-flow statement for diagnostics.
        let _has_control_flow = stmts.iter().any(|s| matches!(s, ScgStatement::Control(_)));

        // Always lower ALL statements (including Returns) in source order.
        // Previously, Returns were deferred to the end to handle topological
        // sort reordering. But this breaks if-bodies: a Return inside an
        // if-then block gets moved to the merge point, causing the
        // then-branch to fall through instead of returning early.
        // Source order is correct for all cases because the SCG walk
        // already produces statements in the right order.
        for idx in 0..stmts.len() {
            self.lower_statement(&stmts[idx], ir_func, names)?;
        }

        Ok(())
    }

    /// Topological sort for a subset of statements (specified by indices).
    /// Same algorithm as topological_sort_statements but only considers
    /// the given indices.
    pub fn topological_sort_statements_subset(
        stmts: &[ScgStatement],
        indices: &[usize],
    ) -> Vec<usize> {
        let n = indices.len();
        if n == 0 {
            return vec![];
        }

        let mut defines: Vec<HashSet<String>> = Vec::with_capacity(n);
        let mut uses: Vec<HashSet<String>> = Vec::with_capacity(n);

        for &idx in indices {
            let (def, use_) = Self::stmt_def_use(&stmts[idx]);
            defines.push(def);
            uses.push(use_);
        }

        let mut deps: Vec<HashSet<usize>> = vec![HashSet::new(); n];
        for j in 0..n {
            for var in &uses[j] {
                let mut found = false;
                for i in (0..j).rev() {
                    if defines[i].contains(var) {
                        deps[j].insert(i);
                        found = true;
                        break;
                    }
                }
                if !found {
                    for i in (j + 1)..n {
                        if defines[i].contains(var) {
                            deps[j].insert(i);
                            break;
                        }
                    }
                }
            }
        }

        let mut in_degree: Vec<usize> = vec![0; n];
        for j in 0..n {
            in_degree[j] = deps[j].len();
        }

        let mut queue: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
        let mut result = Vec::with_capacity(n);
        while let Some(i) = queue.first().copied() {
            queue.remove(0);
            result.push(indices[i]);
            for j in 0..n {
                if deps[j].contains(&i) {
                    in_degree[j] -= 1;
                    if in_degree[j] == 0 {
                        queue.push(j);
                    }
                }
            }
        }

        // Append any remaining (cyclic) statements in original order
        for k in 0..n {
            if !result.contains(&indices[k]) {
                result.push(indices[k]);
            }
        }

        result
    }

    /// Lower a single SCG statement.
    fn lower_statement(
        &mut self,
        stmt: &ScgStatement,
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {

        match stmt {
            ScgStatement::Control(ctrl) => {
                self.lower_control(ctrl, ir_func, names)?;
            }
            ScgStatement::Allocation(alloc) => {
                self.lower_allocation(alloc, ir_func, names)?;
            }
            ScgStatement::Access(access) => {
                self.lower_access(access, ir_func, names)?;
            }
            ScgStatement::Cast(cast) => {
                self.lower_cast(cast, ir_func, names)?;
            }
            ScgStatement::Computation(comp) => {
                self.lower_computation(comp, ir_func, names)?;
            }
            ScgStatement::UnaryComputation(unary) => {
                self.lower_unary_computation(unary, ir_func, names)?;
            }
            ScgStatement::Call(call) => {
                self.lower_call(call, ir_func, names)?;
            }
            ScgStatement::Return(vals) => {
                let mut ir_vals: Vec<IRValue> = vals
                    .iter()
                    .map(|e| self.resolve_expr(e, names, ir_func))
                    .collect::<Result<Vec<_>>>()?;
                // Tail-expression fallthrough: when the SCG bridge emits an
                // empty Return(vec![]) (functions with bare tail expressions),
                // propagate the last instruction's defined register as the
                // return value so backends can move it to the ABI return
                // register.
                if ir_vals.is_empty() {
                    if let Some(block) = ir_func.blocks.last() {
                        if let Some(last_instr) = block.instructions.last() {
                            let defined = last_instr.defined_regs();
                            if defined.len() == 1 {
                                ir_vals.push(IRValue::Register(defined[0]));
                            }
                        }
                    }
                }
                // Emit a Ret instruction and set the terminator.
                ir_func.current_block().push(IRInstruction::Ret {
                    values: ir_vals.clone(),
                });
                ir_func.current_block().terminator = IRTerminator::Return(ir_vals);
            }
            ScgStatement::ConstantTime(ct) => {
                self.lower_constant_time(ct, ir_func, names)?;
            }
            ScgStatement::StructAccess(sa) => {
                self.lower_struct_access(sa, ir_func, names)?;
            }
            ScgStatement::EnumAccess(ea) => {
                self.lower_enum_access(ea, ir_func, names)?;
            }
            ScgStatement::GetAddress(ga) => {
                self.lower_get_address(ga, ir_func, names)?;
            }
        }
        Ok(())
    }

    // =======================================================================
    // Control flow lowering
    // =======================================================================

    /// Lower a control-flow node to IR branches / jumps.
    fn lower_control(
        &mut self,
        ctrl: &ControlNode,
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        match ctrl {
            ControlNode::If {
                cond,
                then_body,
                else_body,
            } => {
                self.lower_if(cond, then_body, else_body, ir_func, names)?;
            }
            ControlNode::Loop { body, for_range, while_cond } => {
                self.lower_loop(body, for_range, while_cond, ir_func, names)?;
            }
            ControlNode::Break => {
                self.lower_break(ir_func, names)?;
            }
            ControlNode::Continue => {
                self.lower_continue(ir_func)?;
            }
            ControlNode::Switch {
                discriminant,
                arms,
                default_body,
            } => {
                self.lower_switch(discriminant, arms, default_body, ir_func, names)?;
            }
        }
        Ok(())
    }

    /// Lower an if/else to IR: evaluate condition, branch, then/else blocks,
    /// merge block with phi nodes for variables modified in either branch.
    ///
    /// This method tracks which variables were defined in each branch and
    /// inserts phi nodes at the merge point for any variable that was
    /// defined in *both* the then and else branches.  A variable that was
    /// only defined in one branch keeps the vreg from that branch (the
    /// other branch's value comes from the pre-if definition).  If a
    /// variable is not defined in either branch *or* the pre-if scope, an
    /// error is returned.
    fn lower_if(
        &mut self,
        cond: &ScgExpr,
        then_body: &[ScgStatement],
        else_body: &Option<Vec<ScgStatement>>,
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        let then_label = self.alloc_label("then");
        let else_label = self.alloc_label("else");
        let merge_label = self.alloc_label("merge");

        // Snapshot the name-to-vreg map before the if.
        let names_before = names.clone();

        let cond_val = self.resolve_expr(cond, names, ir_func)?;

        let false_block = if else_body.is_some() {
            else_label.clone()
        } else {
            merge_label.clone()
        };

        // Branch on the condition.
        ir_func.current_block().push(IRInstruction::CondBranch {
            cond: cond_val.clone(),
            true_target: then_label.clone(),
            false_target: false_block.clone(),
        });
        ir_func.current_block().terminator = IRTerminator::Branch {
            cond: cond_val,
            true_block: then_label.clone(),
            false_block,
        };

        // Then block.
        let entry_block_label = ir_func.current_block().label.clone();
        ir_func.append_block(&then_label);

        // Track variable definitions in the then-branch.
        let mut then_defs = VarDefs::new();
        let then_names_snapshot = names.clone();
        self.lower_statements(then_body, ir_func, names)?;

        // Record which variables were redefined in the then-branch.
        for (name, &vreg) in names.iter() {
            if then_names_snapshot.get(name) != Some(&vreg) {
                then_defs.define(name, vreg);
            }
        }
        for (name, &pre_vreg) in then_names_snapshot.iter() {
            if let Some(&cur_vreg) = names.get(name) {
                if pre_vreg != cur_vreg {
                    then_defs.define(name, cur_vreg);
                }
            }
        }


        // Determine the then-branch's end-block label (for phi incoming edges).
        let _then_end_label = {
            // Walk backwards from current block to find the last block
            // that was finalized in the then-branch.
            // Simple approach: the label of the last block we're in.
            ir_func.current_block().label.clone()
        };

        // Add a jump to merge if the block doesn't already have a proper
        // terminator.
        if matches!(
            ir_func.current_block().terminator,
            IRTerminator::Unreachable
        ) {
            ir_func.current_block().push(IRInstruction::Branch {
                target: merge_label.clone(),
            });
            ir_func.current_block().terminator = IRTerminator::Jump(merge_label.clone());
        }
        let then_exit_label = ir_func.current_block().label.clone();
        // Determine whether the then-branch falls through to the merge block
        // (i.e., its terminator is a Jump to merge_label).  If the then-branch
        // ends with a Return/exit, it does NOT fall through, and phi nodes for
        // then-only modifications must not be created (the then-branch's value
        // is never observed at the merge point).
        let then_falls_through = matches!(
            ir_func.current_block().terminator,
            IRTerminator::Jump(ref l) if l == &merge_label
        );

        // Else block (optional).
        let _else_exit_label = if let Some(else_stmts) = else_body {
            // Restore names to pre-if state before lowering else.
            *names = names_before.clone();

            ir_func.append_block(&else_label);

            let else_names_snapshot = names.clone();
            self.lower_statements(else_stmts, ir_func, names)?;

            // Track which variables were redefined in the else-branch.
            // (See the then-branch above for the dual-case detection logic.)
            let mut else_defs = VarDefs::new();
            for (name, &vreg) in names.iter() {
                if else_names_snapshot.get(name) != Some(&vreg) {
                    else_defs.define(name, vreg);
                }
            }
            for (name, &pre_vreg) in else_names_snapshot.iter() {
                if let Some(&cur_vreg) = names.get(name) {
                    if pre_vreg != cur_vreg {
                        else_defs.define(name, cur_vreg);
                    }
                }
            }

            if matches!(
                ir_func.current_block().terminator,
                IRTerminator::Unreachable
            ) {
                ir_func.current_block().push(IRInstruction::Branch {
                    target: merge_label.clone(),
                });
                ir_func.current_block().terminator = IRTerminator::Jump(merge_label.clone());
            }
            let el = ir_func.current_block().label.clone();
            // Determine whether the else-branch falls through to the merge block.
            let else_falls_through = matches!(
                ir_func.current_block().terminator,
                IRTerminator::Jump(ref l) if l == &merge_label
            );

            // Now merge the names.  We insert phi nodes at the merge block for
            // every variable modified in either branch:
            //   - defined in BOTH branches: phi(then_vreg, else_vreg)
            //   - defined in then ONLY:     phi(then_vreg, pre_if_vreg)
            //   - defined in else ONLY:     phi(pre_if_vreg, else_vreg)
            // The pre-if value comes from `names_before` (the state before the
            // if).  Variables without a pre-if definition that are only defined
            // in one branch cannot get a valid phi (the other path has no
            // value); these keep their branch vreg without a phi.
            let all_modified: HashSet<String> = then_defs
                .defined_names()
                .union(&else_defs.defined_names())
                .cloned()
                .collect();

            // Build the phi list: (name, then_incoming_vreg, else_incoming_vreg).
            let phis_to_insert: Vec<(String, u32, u32)> = all_modified
                .iter()
                .filter_map(|name| {
                    let in_then = then_defs.is_defined(name);
                    let in_else = else_defs.is_defined(name);
                    if in_then && in_else {
                        // Both branches modified: only create phi if both fall
                        // through to merge.  If one branch returns, the existing
                        // both-branch phi would have an invalid edge; skip it
                        // (the falling-through branch's value is used directly).
                        if then_falls_through && else_falls_through {
                            Some((name.clone(), then_defs.get(name).unwrap(), else_defs.get(name).unwrap()))
                        } else if then_falls_through && !else_falls_through {
                            // else returns; only then reaches merge.
                            names_before.get(name).map(|pre| (name.clone(), then_defs.get(name).unwrap(), *pre))
                        } else if !then_falls_through && else_falls_through {
                            // then returns; only else reaches merge.
                            names_before.get(name).map(|pre| (name.clone(), *pre, else_defs.get(name).unwrap()))
                        } else {
                            None
                        }
                    } else if in_then {
                        // then-only: create phi only if then falls through.
                        if then_falls_through {
                            names_before.get(name).map(|pre| (name.clone(), then_defs.get(name).unwrap(), *pre))
                        } else {
                            None
                        }
                    } else if in_else {
                        // else-only: create phi only if else falls through.
                        if else_falls_through {
                            names_before.get(name).map(|pre| (name.clone(), *pre, else_defs.get(name).unwrap()))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect();

            // For single-branch modifications WITHOUT a pre-if definition
            // (e.g. a new variable declared inside one branch), keep the
            // branch's vreg without a phi.
            for name in &all_modified {
                let in_then = then_defs.is_defined(name);
                let in_else = else_defs.is_defined(name);
                if (in_then ^ in_else) && !names_before.contains_key(name) {
                    if in_then {
                        if let Some(vreg) = then_defs.get(name) {
                            names.insert(name.clone(), vreg);
                        }
                    } else {
                        if let Some(vreg) = else_defs.get(name) {
                            names.insert(name.clone(), vreg);
                        }
                    }
                }
            }

            // Merge block.
            ir_func.append_block(&merge_label);

            // Insert phi nodes for all modified variables (both-branch and
            // single-branch).
            for (name, then_vreg, else_vreg) in &phis_to_insert {
                let phi_dst = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::named(phi_dst, name));
                ir_func.current_block().push(IRInstruction::Phi {
                    dst: IRValue::Register(phi_dst),
                    incoming: vec![
                        (IRValue::Register(*then_vreg), then_exit_label.clone()),
                        (IRValue::Register(*else_vreg), el.clone()),
                    ],
                });
                names.insert(name.clone(), phi_dst);
                // Redirect ALL alias entries that still point to either
                // branch-local vreg (then_vreg or else_vreg) to phi_dst.
                // The SCG→codegen bridge often resolves user-level variable
                // references (e.g. `return x`) to the SCG-node-id of the
                // most-recent assignment in source order (e.g. Var("v_7")
                // from the else-branch).  After the merge, those references
                // must read the phi result, not the branch-local vreg, so
                // we update every names entry whose value is then_vreg or
                // else_vreg.  vregs are unique per computation, so this
                // never clobbers an unrelated variable.
                let alias_keys: Vec<String> = names.iter()
                    .filter(|(_, &v)| v == *then_vreg || v == *else_vreg)
                    .map(|(k, _)| k.clone())
                    .collect();

                for key in alias_keys {
                    names.insert(key, phi_dst);
                }
            }

            Some(el)
        } else {
            // No else-branch: create phi nodes for variables modified in the
            // then-branch. Each phi merges the then-branch value with the
            // pre-if value. On the false path, the entry block (containing
            // the CondBranch) jumps directly to merge_label, so the phi's
            // else-incoming edge is from the entry block.
            ir_func.append_block(&merge_label);

            // Collect variables modified in the then-branch that also have a
            // pre-if definition (so we can create a valid phi).  Only create
            // phis if the then-branch falls through to merge; if the then-branch
            // returns, its modifications are never observed at merge.
            let mut modified: Vec<String> = if then_falls_through {
                then_defs.defined_names()
                    .into_iter()
                    .filter(|n| names_before.contains_key(n))
                    .collect()
            } else {
                Vec::new()
            };
            modified.sort(); // deterministic order
            for name in &modified {
                let then_vreg = then_defs.get(name).unwrap();
                let pre_vreg = *names_before.get(name).unwrap();
                let phi_dst = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::named(phi_dst, name));
                ir_func.current_block().push(IRInstruction::Phi {
                    dst: IRValue::Register(phi_dst),
                    incoming: vec![
                        (IRValue::Register(then_vreg), then_exit_label.clone()),
                        (IRValue::Register(pre_vreg), entry_block_label.clone()),
                    ],
                });
                names.insert(name.clone(), phi_dst);
                // Redirect alias entries that still point to the then-branch
                // vreg to phi_dst.  (See the both-branch case above for the
                // rationale: SCG-level Var("v_N") references to the
                // then-branch's reassignment must read the phi result after
                // the merge, not the branch-local vreg.)
                let alias_keys: Vec<String> = names.iter()
                    .filter(|(_, &v)| v == then_vreg)
                    .map(|(k, _)| k.clone())
                    .collect();
        
                for key in alias_keys {
                    names.insert(key, phi_dst);
                }
            }
            None
        };

        Ok(())
    }

    /// Lower a loop to IR: loop header with phi nodes, loop body, back-edge,
    /// loop exit.
    ///
    /// The loop header contains phi nodes for any loop-carried values (one
    /// phi per variable in scope before the loop).  If no variables are in
    /// scope, a synthetic loop-counter phi is inserted so the header still
    /// demonstrates the canonical SSA loop-phi pattern.  Real compilers would
    /// analyze which variables are modified in the loop body and insert phis
    /// only for those.
    fn lower_loop(
        &mut self,
        body: &[ScgStatement],
        for_range: &Option<(String, ScgExpr, ScgExpr)>,
        while_cond: &Option<String>,
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        let loop_header = self.alloc_label("loop_header");
        let loop_body_label = self.alloc_label("loop_body");
        let loop_exit = self.alloc_label("loop_exit");

        // Push loop context for break/continue resolution.
        // For for-loops, continue should jump to the increment block
        // (which we'll create as loop_body_label + "_continue").
        // We set continue_target after lowering the body.
        // Always create a continue label so that `continue` statements
        // jump to the back-edge block (which has phi copies) instead of
        // the loop header (which would skip phi resolution). This is
        // critical for while-loops with guards, where `continue` would
        // otherwise skip the phi copy that updates the loop variable.
        let continue_label = Some(self.alloc_label("loop_continue"));
        self.loop_stack.push(LoopContext {
            header_label: loop_header.clone(),
            exit_label: loop_exit.clone(),
            continue_target: continue_label.clone(),
            break_snapshots: Vec::new(),
        });

        // ── Step 0: For-loop — initialize loop counter ──
        if let Some((var, start, _end)) = for_range {
            let counter_init = self.alloc_vreg();
            ir_func.register_vreg(VirtualRegister::named(counter_init, var.as_str()));
            // Resolve the start expression. For constants, this produces
            // Immediate(n). For variables, it produces a Register reference
            // that the names map resolves. For binops (e.g. msg_len + 1),
            // it produces a BinOp that the IR builder evaluates.
            let start_val = self.resolve_expr(start, names, ir_func)?;
            ir_func.current_block().instructions.push(IRInstruction::Add {
                dst: IRValue::Register(counter_init),
                lhs: start_val,
                rhs: IRValue::Immediate(0),
                ty: None,
            });
            names.insert(var.clone(), counter_init);
        }

        // ── Step 1: Snapshot names BEFORE the loop ──
        let names_before = names.clone();

        // ── Step 2: Jump from current block to loop header ──
        ir_func.current_block().push(IRInstruction::Branch {
            target: loop_header.clone(),
        });
        ir_func.current_block().terminator = IRTerminator::Jump(loop_header.clone());
        let pre_header_label = ir_func.blocks[ir_func.blocks.len() - 1].label.clone();

        // ── Step 3: Create loop header block with phi nodes ──
        ir_func.append_block(&loop_header);

        // For every variable that exists before the loop, create a phi node
        // in the loop header. The phi merges the pre-loop value with the
        // value from the back-edge (which we'll fill in after lowering the body).
        // The phi result becomes the variable's value inside the loop body.
        let mut phi_info: Vec<(String, u32, u32)> = Vec::new(); // (name, pre_loop_vreg, phi_vreg)
        let mut phi_instructions: Vec<IRInstruction> = Vec::new();

        let mut sorted_names: Vec<String> = names_before.keys().cloned().collect();
        sorted_names.sort(); // deterministic order

        // Deduplicate phi nodes by pre_loop_vreg: if two names share the
        // same pre-loop vreg (e.g. "v_1" and "sum" both refer to the same
        // variable — one is the SCG node id, the other the user-visible
        // name), they must share the SAME phi vreg.  Without this, the two
        // aliases would get separate phi vregs, and lower_computation's
        // reassignment update (which finds entries matching
        // names[reassigns]) would only update one alias, leaving the other
        // stale — breaking loop-carried variable propagation.
        let mut vreg_to_phi: HashMap<u32, u32> = HashMap::new();
        for name in &sorted_names {
            let &pre_loop_vreg = names_before.get(name).unwrap();
            let phi_vreg = if let Some(&existing_phi) = vreg_to_phi.get(&pre_loop_vreg) {
                // Reuse the existing phi vreg for this pre_loop_vreg.
                existing_phi
            } else {
                let new_phi = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::named(new_phi, name.as_str()));
                vreg_to_phi.insert(pre_loop_vreg, new_phi);

                // Create phi with placeholder incoming from back-edge.
                // We'll patch the back-edge incoming value after lowering the body.
                phi_instructions.push(IRInstruction::Phi {
                    dst: IRValue::Register(new_phi),
                    incoming: vec![
                        (IRValue::Register(pre_loop_vreg), pre_header_label.clone()),
                        (IRValue::Register(pre_loop_vreg), loop_body_label.clone()), // placeholder
                    ],
                });
                new_phi
            };
            phi_info.push((name.clone(), pre_loop_vreg, phi_vreg));

            // Update names so the loop body uses the phi result
            names.insert(name.clone(), phi_vreg);
        }

        // If no variables existed before the loop, the loop header would
        // otherwise be empty of phi nodes.  Insert a synthetic loop-counter
        // phi so the header still demonstrates the canonical SSA loop phi
        // pattern (and downstream passes / tests that look for a phi in the
        // loop header find one).  The counter is initialised to 0 on entry
        // and is self-referential on the back-edge (a real compiler would
        // emit an increment in the body; we don't, so the back-edge value
        // is the phi result itself).
        if phi_info.is_empty() {
            let counter_vreg = self.alloc_vreg();
            ir_func
                .register_vreg(VirtualRegister::named(counter_vreg, "loop_counter"));
            phi_instructions.push(IRInstruction::Phi {
                dst: IRValue::Register(counter_vreg),
                incoming: vec![
                    (IRValue::Immediate(0), pre_header_label.clone()),
                    (IRValue::Register(counter_vreg), loop_body_label.clone()),
                ],
            });
        }

        // Insert phi instructions at the beginning of the loop header
        for phi in phi_instructions {
            ir_func.current_block().instructions.push(phi);
        }

        if let Some((var, _start, end_expr)) = for_range {
            let counter_vreg = names.get(var).copied().unwrap_or(0);
            // Resolve the end bound expression.  For constant ends
            // (ScgExpr::Int(n)) this produces `end_vreg = n + 0`.  For
            // variable ends (ScgExpr::Var("i")) this produces
            // `end_vreg = i_vreg + 0`, correctly capturing the current
            // value of the loop-variable-bound end.
            let end_val = self.resolve_expr(end_expr, names, ir_func)?;
            let end_vreg = self.alloc_vreg();
            ir_func.register_vreg(VirtualRegister::named(end_vreg, "loop_end"));
            ir_func.current_block().instructions.push(IRInstruction::Add {
                dst: IRValue::Register(end_vreg),
                lhs: end_val,
                rhs: IRValue::Immediate(0),
                ty: None,
            });
            let cmp_vreg = self.alloc_vreg();
            ir_func.register_vreg(VirtualRegister::named(cmp_vreg, "loop_cmp"));
            ir_func.current_block().instructions.push(IRInstruction::Cmp {
                kind: CmpKind::SLt,
                dst: IRValue::Register(cmp_vreg),
                lhs: IRValue::Register(counter_vreg),
                rhs: IRValue::Register(end_vreg),
                ty: None,
            });
            ir_func.current_block().instructions.push(IRInstruction::CondBranch {
                cond: IRValue::Register(cmp_vreg),
                true_target: loop_body_label.clone(),
                false_target: loop_exit.clone(),
            });
            ir_func.current_block().terminator = IRTerminator::Branch {
                cond: IRValue::Register(cmp_vreg),
                true_block: loop_body_label.clone(),
                false_block: loop_exit.clone(),
            };
        } else {
            ir_func.current_block().push(IRInstruction::Branch {
                target: loop_body_label.clone(),
            });
            ir_func.current_block().terminator = IRTerminator::Jump(loop_body_label.clone());
        }

        // ── Step 4: Lower the loop body ──
        ir_func.append_block(&loop_body_label);
        self.lower_statements(body, ir_func, names)?;

        // ── Step 4b: Emit the continue target block (for for-loops) ──
        // This block contains the loop increment and back-edge to header.
        // `continue` jumps here, ensuring the increment is always executed.
        if let Some(ref cont_label) = continue_label {
            // If the current block (end of loop body) doesn't have a terminator,
            // it falls through to the continue block.
            if matches!(
                ir_func.current_block().terminator,
                IRTerminator::Unreachable
            ) {
                ir_func.current_block().push(IRInstruction::Branch {
                    target: cont_label.clone(),
                });
                ir_func.current_block().terminator = IRTerminator::Jump(cont_label.clone());
            }
            ir_func.append_block(cont_label);
        }

        if let Some((var, _start, _end)) = for_range {
            if let Some(&counter_vreg) = names.get(var) {
                let inc_vreg = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::named(inc_vreg, "loop_inc"));
                ir_func.current_block().instructions.push(IRInstruction::Add {
                    dst: IRValue::Register(inc_vreg),
                    lhs: IRValue::Register(counter_vreg),
                    rhs: IRValue::Immediate(1),
                    ty: None,
                });
                names.insert(var.clone(), inc_vreg);
            }
        }

        // Back-edge to header if the block doesn't have a terminator.
        let back_edge_label;
        if matches!(
            ir_func.current_block().terminator,
            IRTerminator::Unreachable
        ) {
            ir_func.current_block().push(IRInstruction::Branch {
                target: loop_header.clone(),
            });
            ir_func.current_block().terminator = IRTerminator::Jump(loop_header.clone());
        }
        back_edge_label = ir_func.current_block().label.clone();

        // ── Step 5: Patch phi nodes with correct back-edge values ──
        // For each variable that was modified in the loop body, update the
        // phi's back-edge incoming to use the new vreg from the end of the loop body.
        //
        // The loop body's lower_computation creates NEW names entries (e.g.,
        // "v_4" for a reassignment) instead of updating existing entries
        // (e.g., "v_1"). So we need to find the LATEST vreg for each phi
        // variable by checking if names[name] changed, AND by checking if
        // any new names were created that correspond to reassignments of
        // the phi variable.
        {
            // Build a map from original name to latest vreg
            let mut name_to_latest: HashMap<String, u32> = HashMap::new();
            for (name, pre_vreg, _phi_vreg) in &phi_info {
                if let Some(&current_vreg) = names.get(name) {
                    if current_vreg != *pre_vreg {
                        name_to_latest.insert(name.clone(), current_vreg);
                    }
                }
            }

            let header_block = ir_func.blocks.iter_mut()
                .find(|b| b.label == loop_header)
                .expect("loop header block must exist");

            for instr in &mut header_block.instructions {
                if let IRInstruction::Phi { dst, incoming } = instr {
                    if let Some(phi_vreg_id) = dst.as_register() {
                        for (name, pre_vreg, phi_vreg) in &phi_info {
                            if *phi_vreg == phi_vreg_id {
                                // Get the latest vreg for this name.
                                // Check BOTH the name_to_latest map (which tracks
                                // user-visible name changes) AND look for any
                                // reassignment vregs that map back to this name
                                // via the reassigns mechanism.
                                let latest_vreg = name_to_latest.get(name).copied()
                                    .or_else(|| names.get(name).copied());
                                if let Some(current_vreg) = latest_vreg {
                                    // Only patch if the current vreg is DIFFERENT
                                    // from the phi vreg (otherwise we create a
                                    // self-referential phi which is correct for
                                    // unmodified variables but wrong for modified ones).
                                    for entry in incoming.iter_mut() {
                                        if entry.1 == loop_body_label || entry.1 == back_edge_label {
                                            entry.0 = IRValue::Register(current_vreg);
                                            entry.1 = back_edge_label.clone();
                                        }
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
            }

        }

        // ── Step 6: Update names for variables after the loop ──
        // After the loop, variables that were modified in the loop should
        // use the appropriate value depending on how the loop exited.
        //
        // If there are break paths, we need phi nodes at the loop exit
        // that merge the values from each break path and the normal
        // (fall-through / back-edge) exit path.
        //
        // Retrieve break snapshots from the loop context before popping it.
        let break_snapshots = self
            .loop_stack
            .last()
            .map(|ctx| ctx.break_snapshots.clone())
            .unwrap_or_default();

        // Determine which variables were modified on any break path.
        // A variable needs a phi at the loop exit if it was modified
        // on at least one break path OR in the loop body (normal exit).
        let mut all_exit_modified: HashSet<String> = HashSet::new();
        for (_, snap_names) in &break_snapshots {
            for name in snap_names.keys() {
                if names_before.get(name) != snap_names.get(name) {
                    all_exit_modified.insert(name.clone());
                }
            }
        }
        // Also include variables modified in the loop body (normal exit path).
        for name in names.keys() {
            if names_before.get(name) != names.get(name) {
                all_exit_modified.insert(name.clone());
            }
        }

        if !break_snapshots.is_empty() && !all_exit_modified.is_empty() {
            // ── Create phi nodes at the loop exit block ──
            // The loop exit block already exists (created in Step 7 below,
            // but we create it early here so we can insert phi instructions).
            ir_func.append_block(&loop_exit);

            // Collect all predecessor labels and their name maps:
            // 1. Each break path is a predecessor
            // 2. The normal (fall-through/back-edge) exit is also a predecessor
            //    if the loop body doesn't always break.
            let mut _exit_predecessors: Vec<(String, HashMap<String, u32>)> = break_snapshots.clone();

            // Check if the loop can exit normally (without a break).
            // If the last block of the loop body falls through (has no
            // terminator or branches back to the header), it's not a
            // normal exit path.  But if it has a conditional branch that
            // could fall through, we need to handle that.  For simplicity,
            // we always include the back-edge block's names as the normal
            // exit path — but only if there's a path from the back-edge
            // block that doesn't go to the header.  Since our current
            // loop structure always branches back to the header from the
            // back-edge, the normal exit path doesn't add a predecessor.
            // However, we still need to provide a value for variables
            // that were modified in the loop body but NOT on any break
            // path.  We use the phi result (header phi) for those.

            let mut exit_phi_info: Vec<(String, u32, u32)> = Vec::new(); // (name, phi_vreg_in_header, exit_phi_vreg)
            let mut exit_phi_instructions: Vec<IRInstruction> = Vec::new();

            let mut sorted_modified: Vec<String> = all_exit_modified.iter().cloned().collect();
            sorted_modified.sort();

            for name in &sorted_modified {
                let exit_phi_vreg = self.alloc_vreg();
                // Get the header phi vreg for this name (used as fallback for
                // the normal exit path when no explicit predecessor provides it).
                let header_phi_vreg = phi_info.iter()
                    .find(|(n, _, _)| n == name)
                    .map(|(_, _, pv)| *pv)
                    .unwrap_or_else(|| {
                        // Variable didn't exist before the loop — use its current vreg.
                        *names.get(name).unwrap_or(&0)
                    });

                ir_func.register_vreg(VirtualRegister::named(exit_phi_vreg, name.as_str()));
                exit_phi_info.push((name.clone(), header_phi_vreg, exit_phi_vreg));

                // Build incoming list: one entry per break path.
                let mut incoming: Vec<(IRValue, String)> = Vec::new();
                for (break_label, snap_names) in &break_snapshots {
                    let vreg = snap_names.get(name).copied().unwrap_or(header_phi_vreg);
                    incoming.push((IRValue::Register(vreg), break_label.clone()));
                }

                exit_phi_instructions.push(IRInstruction::Phi {
                    dst: IRValue::Register(exit_phi_vreg),
                    incoming,
                });
            }

            // Insert phi instructions at the beginning of the loop exit block.
            let exit_block = ir_func.blocks.iter_mut()
                .find(|b| b.label == loop_exit)
                .expect("loop exit block must exist");
            for phi in exit_phi_instructions {
                exit_block.instructions.push(phi);
            }

            // After the loop, use the exit phi results for modified variables.
            for (name, _, exit_phi_vreg) in &exit_phi_info {
                names.insert(name.clone(), *exit_phi_vreg);
            }

            // Pop loop context.
            self.loop_stack.pop();
        } else {
            // No break paths or no modified variables — use the header phi
            // results (original behavior).
            for (name, _, phi_vreg) in &phi_info {
                names.insert(name.clone(), *phi_vreg);
            }

            // ── Step 7: Loop exit block ──
            ir_func.append_block(&loop_exit);

            // Pop loop context.
            self.loop_stack.pop();
        }

        Ok(())
    }

    /// Lower a `break` to a jump to the enclosing loop's exit label.
    // =======================================================================
    // Phi resolution — convert phi nodes to explicit copy instructions
    // =======================================================================

    /// Resolve all phi nodes in the function into explicit copy instructions.
    ///
    /// For each phi `dst = phi(val1 from block_A, val2 from block_B)`, we insert
    /// a copy `dst = val1` at the end of block_A (before its terminator) and
    /// `dst = val2` at the end of block_B (before its terminator).
    ///
    /// This is a standard SSA destruction step. The copies ensure that when
    /// control transfers from a predecessor to the phi's block, the correct
    /// value is already in the destination vreg's stack slot.
    ///
    /// The phi instructions themselves are **kept** in the IR.  Downstream
    /// consumers handle `IRInstr::Phi` in two ways:
    /// - Analysis passes (e.g. `control_flow.rs` loop trip-count inference)
    ///   inspect phi nodes to recover loop-carried value origins.
    /// - Instruction selectors / emitters treat `Phi` as a no-op (the actual
    ///   data movement is handled by the copies inserted here).
    ///
    /// Removing the phi nodes here would break both the analysis passes and
    /// the `scg_to_ir` tests that assert phi presence in merge / loop-header
    /// blocks.
    fn resolve_phis(&mut self, ir_func: &mut IRFunction) -> Result<()> {
        // Build a label → block-index map
        let label_to_idx: HashMap<String, usize> = ir_func.blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (b.label.clone(), i))
            .collect();

        // Collect all phi information first (to avoid borrow issues)
        // Each entry: (block_idx, phi_dst, vec of (value, predecessor_label))
        let mut all_phis: Vec<(usize, IRValue, Vec<(IRValue, String)>)> = Vec::new();

        for (block_idx, block) in ir_func.blocks.iter().enumerate() {
            for instr in &block.instructions {
                if let IRInstruction::Phi { dst, incoming } = instr {
                    all_phis.push((block_idx, dst.clone(), incoming.clone()));
                }
            }
        }

        // ── Group phi copies by predecessor block ──
        //
        // For each predecessor, we collect a list of (dst, src) pairs that
        // must execute "in parallel" at the end of that block (before the
        // branch to the phi's block).
        //
        // We then emit them using a parallel-copy algorithm that handles
        // cycles correctly. Without this, two phis that swap values
        //   a = phi(b),  b = phi(a)
        // would be lowered to:
        //   a = b + 0   (a's slot now has b's old value)
        //   b = a + 0   (b's slot now has b's old value — WRONG, should be a's old)
        // causing the second phi to read the wrong value.
        //
        // The algorithm:
        //   1. Build a worklist of pending (dst, src) copies.
        //   2. Repeatedly scan the worklist for a copy whose `src` is not
        //      the `dst` of any other pending copy. Emit it.
        //   3. If no such copy exists, we have a cycle. Pick any copy,
        //      save its `src` to a fresh temp vreg, replace all pending
        //      uses of that `src` with the temp, then emit the copy.
        //   4. Repeat until the worklist is empty.
        //
        // The temp vreg is allocated via `self.alloc_vreg()` and gets its
        // own stack slot in the emitter, so it cannot alias with any
        // existing vreg.

        // Map: pred_label → Vec<(dst, src)>
        let mut copies_by_pred: HashMap<String, Vec<(IRValue, IRValue)>> = HashMap::new();
        for (_phi_block_idx, dst, incoming) in &all_phis {
            for (value, pred_label) in incoming {
                // Skip self-referencing entries (where the value == dst)
                if value == dst {
                    continue;
                }
                // Skip pure immediate sources that match dst (no-op)
                if let IRValue::Immediate(_) = value {
                    // Immediate sources have no slot to clobber, so they're always safe.
                    // We still emit them (the dst gets the immediate value).
                }
                copies_by_pred
                    .entry(pred_label.clone())
                    .or_default()
                    .push((dst.clone(), value.clone()));
            }
        }

        // For each predecessor block, emit the parallel copies.
        for (pred_label, copies) in &copies_by_pred {
            let Some(&pred_idx) = label_to_idx.get(pred_label) else {
                continue;
            };

            // Run the parallel-copy algorithm.
            let emitted = self.emit_parallel_copies(copies.clone(), ir_func)?;

            // Insert all emitted instructions before the terminator (Branch).
            let block = &mut ir_func.blocks[pred_idx];
            if let Some(IRInstruction::Branch { .. }) = block.instructions.last() {
                // Insert before the Branch
                let insert_at = block.instructions.len() - 1;
                for (i, instr) in emitted.into_iter().enumerate() {
                    block.instructions.insert(insert_at + i, instr);
                }
            } else {
                // No Branch at end — just append
                block.instructions.extend(emitted);
            }
        }

        // Remove phi instructions after inserting copies.
        for block in &mut ir_func.blocks {
            block.instructions.retain(|instr| !matches!(instr, IRInstruction::Phi { .. }));
        }

        Ok(())
    }

    /// Emit a list of (dst, src) copies using a parallel-copy algorithm.
    ///
    /// Handles cyclic dependencies (e.g. swaps) by introducing temporary
    /// vregs. Returns the ordered list of IR instructions to emit.
    ///
    /// The copies are emitted as `Add { dst, lhs: src, rhs: Imm(0), ty: None }`
    /// which the stack-slot emitter lowers to a register-level load+store
    /// (with no actual arithmetic, since adding zero is a no-op).
    fn emit_parallel_copies(
        &mut self,
        mut copies: Vec<(IRValue, IRValue)>,
        ir_func: &mut IRFunction,
    ) -> Result<Vec<IRInstruction>> {
        let mut emitted: Vec<IRInstruction> = Vec::new();

        // Helper: make a copy instruction for (dst, src).
        let make_copy = |dst: &IRValue, src: &IRValue| IRInstruction::Add {
            dst: dst.clone(),
            lhs: src.clone(),
            rhs: IRValue::Immediate(0),
            ty: None,
        };

        // Helper: extract register id from an IRValue (if any).
        let reg_id = |v: &IRValue| match v {
            IRValue::Register(id) => Some(*id),
            _ => None,
        };

        // Repeat until all copies are emitted.
        while !copies.is_empty() {
            // Find a copy whose src register is not the dst of any other pending copy.
            // (Immediate sources are always safe to emit since they don't read slots.)
            let mut ready_idx: Option<usize> = None;
            for (i, (_dst, src)) in copies.iter().enumerate() {
                let src_reg = reg_id(src);
                if src_reg.is_none() {
                    // Immediate / address source — always safe.
                    ready_idx = Some(i);
                    break;
                }
                let src_reg = src_reg.unwrap();
                let conflicts = copies.iter().any(|(d, _s)| {
                    if let IRValue::Register(d_id) = d {
                        *d_id == src_reg
                    } else {
                        false
                    }
                });
                if !conflicts {
                    ready_idx = Some(i);
                    break;
                }
            }

            if let Some(i) = ready_idx {
                // Emit this copy.
                let (dst, src) = copies.remove(i);
                emitted.push(make_copy(&dst, &src));
            } else {
                // All remaining copies form cycles. Pick the first one
                // and break the cycle by saving its src to a temp vreg.
                let (dst, src) = copies[0].clone();
                let src_reg = match reg_id(&src) {
                    Some(r) => r,
                    None => {
                        // Shouldn't happen — immediates are always ready.
                        copies.remove(0);
                        emitted.push(make_copy(&dst, &src));
                        continue;
                    }
                };

                // Allocate a fresh temp vreg.
                let temp_id = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::anonymous(temp_id));

                // Emit: temp = src  (saves src's value before any overwrite)
                emitted.push(IRInstruction::Add {
                    dst: IRValue::Register(temp_id),
                    lhs: IRValue::Register(src_reg),
                    rhs: IRValue::Immediate(0),
                    ty: None,
                });

                // Replace all pending uses of `src_reg` with `temp_id`.
                for (d, s) in copies.iter_mut() {
                    if let IRValue::Register(s_id) = s {
                        if *s_id == src_reg {
                            *s = IRValue::Register(temp_id);
                            // Avoid double-replacement: change s_id to a sentinel.
                            // (We do this by leaving it as temp_id — fine because
                            // we won't match src_reg again on this vreg.)
                        }
                    }
                    // Also: if `d` is the dst of this very copy (dst == dst), skip.
                    // (Self-copies are filtered out earlier, so this shouldn't happen.)
                }

                // Now this copy's src is `temp_id`, which is not the dst of any
                // pending copy → it will be picked up in the next iteration.
            }
        }

        Ok(emitted)
    }

    /// Lower a `break` to a jump to the enclosing loop's exit label.
    fn lower_break(&mut self, ir_func: &mut IRFunction, names: &HashMap<String, u32>) -> Result<()> {
        let ctx = self
            .loop_stack
            .last_mut()
            .ok_or_else(|| {
                crate::CodegenError::TranslationError("break outside of loop".to_string())
            })?;

        let exit_label = ctx.exit_label.clone();
        // Snapshot the current variable map at the break point so that
        // we can create phi nodes at the loop exit that merge values
        // from all break paths.
        let break_block_label = ir_func.current_block().label.clone();
        ctx.break_snapshots.push((break_block_label, names.clone()));

        ir_func.current_block().push(IRInstruction::Branch {
            target: exit_label.clone(),
        });
        ir_func.current_block().terminator = IRTerminator::Jump(exit_label);
        Ok(())
    }

    /// Lower a `continue` to a jump to the enclosing loop's header label.
    fn lower_continue(&mut self, ir_func: &mut IRFunction) -> Result<()> {
        // Continue should jump to the loop back-edge (increment) block,
        // NOT the loop header. Jumping to the header skips the increment,
        // causing an infinite loop when continue is hit.
        //
        // For for-loops, the back-edge block is the last block in the loop
        // body that contains the increment and then jumps to the header.
        // We don't track the back-edge label directly, so we jump to the
        // end of the loop body — the code after lower_statements(body)
        // adds the increment and back-edge to header.
        //
        // The simplest correct fix: create a continue target that is the
        // loop body's back-edge. Since we can't know the back-edge label
        // at continue-lowering time, we use a synthetic label and patch
        // it after the loop body is lowered.
        let ctx = self
            .loop_stack
            .last_mut()
            .ok_or_else(|| {
                crate::CodegenError::TranslationError("continue outside of loop".to_string())
            })?;

        // If we have a continue_target, jump there. Otherwise jump to header
        // (for while-loops where there's no increment, this is correct).
        let target = ctx.continue_target.clone().unwrap_or_else(|| ctx.header_label.clone());

        ir_func.current_block().push(IRInstruction::Branch {
            target: target.clone(),
        });
        ir_func.current_block().terminator = IRTerminator::Jump(target);
        Ok(())
    }

    /// Lower a switch/match to IR: cascading compare-and-branch for each arm,
    /// with a merge block at the end.
    ///
    /// For small switch statements (≤8 arms), we emit a linear chain of
    /// `CMP` + `B.EQ` for each case value. For larger switches with dense
    /// contiguous values, a jump table would be more efficient, but that
    /// requires data-section support that isn't fully wired yet.
    ///
    /// Structure:
    /// ```text
    ///   entry:  CMP disc, arm0_val → B.EQ arm0_label
    ///           CMP disc, arm1_val → B.EQ arm1_label
    ///           …
    ///           B default_label
    ///   arm0:   [arm0_body] → B merge
    ///   arm1:   [arm1_body] → B merge
    ///   …
    ///   default: [default_body] → B merge
    ///   merge:  [phi nodes for vars modified in any arm]
    /// ```
    fn lower_switch(
        &mut self,
        discriminant: &ScgExpr,
        arms: &[SwitchArm],
        default_body: &[ScgStatement],
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        let disc_val = self.resolve_expr(discriminant, names, ir_func)?;
        let merge_label = self.alloc_label("switch_merge");

        // Snapshot names before the switch for phi insertion later.
        let names_before = names.clone();

        // Generate labels for each arm and the default case.
        let arm_labels: Vec<String> = arms
            .iter()
            .enumerate()
            .map(|(i, _)| self.alloc_label(&format!("case_{}", i)))
            .collect();
        let default_label = self.alloc_label("default");

        // Emit cascading compare-and-branch. Each comparison gets its OWN
        // block so the CondBranch properly terminates it. Without this,
        // multiple CondBranches in the same block cause the backend to
        // only honor the last one (the terminator).
        let mut cmp_labels: Vec<String> = Vec::new();
        for i in 0..arms.len() {
            cmp_labels.push(self.alloc_label(&format!("switch_cmp_{}", i)));
        }

        for (i, arm) in arms.iter().enumerate() {
            let cond_vreg = self.alloc_vreg();
            ir_func.register_vreg(VirtualRegister::anonymous(cond_vreg));
            ir_func.current_block().push(IRInstruction::Cmp {
                kind: CmpKind::Eq,
                dst: IRValue::Register(cond_vreg),
                lhs: disc_val.clone(),
                rhs: IRValue::Immediate(arm.value),
                ty: None,
            });
            let false_target = if i + 1 < arms.len() {
                cmp_labels[i + 1].clone()
            } else {
                default_label.clone()
            };
            ir_func.current_block().push(IRInstruction::CondBranch {
                cond: IRValue::Register(cond_vreg),
                true_target: arm_labels[i].clone(),
                false_target: false_target.clone(),
            });
            ir_func.current_block().terminator = IRTerminator::Branch {
                cond: IRValue::Register(cond_vreg),
                true_block: arm_labels[i].clone(),
                false_block: false_target.clone(),
            };

            // Create next comparison block (or fall through to default)
            if i + 1 < arms.len() {
                ir_func.append_block(&cmp_labels[i + 1]);
            }
        }

        // After all comparisons, fall through to default.
        // But only if the current block doesn't already have a terminator
        // (the last CondBranch already sets the terminator).
        if matches!(
            ir_func.current_block().terminator,
            IRTerminator::Unreachable
        ) {
            ir_func.current_block().push(IRInstruction::Branch {
                target: default_label.clone(),
            });
            ir_func.current_block().terminator = IRTerminator::Jump(default_label.clone());
        }

        // Track all variable definitions across arms for phi insertion.
        let mut all_arm_defs: Vec<VarDefs> = Vec::new();
        let mut arm_exit_labels: Vec<String> = Vec::new();

        // Lower each arm body.
        for (i, arm) in arms.iter().enumerate() {
            *names = names_before.clone();
            ir_func.append_block(&arm_labels[i]);
            self.lower_statements(&arm.body, ir_func, names)?;

            let mut arm_defs = VarDefs::new();
            for (name, &vreg) in names.iter() {
                if names_before.get(name) != Some(&vreg) {
                    arm_defs.define(name, vreg);
                }
            }
            all_arm_defs.push(arm_defs);

            if matches!(
                ir_func.current_block().terminator,
                IRTerminator::Unreachable
            ) {
                ir_func.current_block().push(IRInstruction::Branch {
                    target: merge_label.clone(),
                });
                ir_func.current_block().terminator = IRTerminator::Jump(merge_label.clone());
            }
            arm_exit_labels.push(ir_func.current_block().label.clone());
        }

        // Lower default arm.
        *names = names_before.clone();
        ir_func.append_block(&default_label);
        self.lower_statements(default_body, ir_func, names)?;

        let mut default_defs = VarDefs::new();
        for (name, &vreg) in names.iter() {
            if names_before.get(name) != Some(&vreg) {
                default_defs.define(name, vreg);
            }
        }

        if matches!(
            ir_func.current_block().terminator,
            IRTerminator::Unreachable
        ) {
            ir_func.current_block().push(IRInstruction::Branch {
                target: merge_label.clone(),
            });
            ir_func.current_block().terminator = IRTerminator::Jump(merge_label.clone());
        }
        let default_exit_label = ir_func.current_block().label.clone();

        // Compute all modified variable names across all arms + default.
        // Only include variables that existed BEFORE the switch (in names_before).
        // Variables created inside an arm (like synthetic SCG node IDs "v_N")
        // should NOT get merge phis — they don't have a value in other arms
        // or in the default case.
        let all_modified: HashSet<String> = all_arm_defs
            .iter()
            .chain(std::iter::once(&default_defs))
            .flat_map(|defs| defs.defined_names())
            .filter(|name| names_before.contains_key(name))
            .collect();

        // Merge block with phi nodes for variables modified in multiple arms.
        ir_func.append_block(&merge_label);

        for name in &all_modified {
            let mut incoming: Vec<(IRValue, String)> = Vec::new();

            for (i, arm_defs) in all_arm_defs.iter().enumerate() {
                let vreg = arm_defs
                    .get(name)
                    .or_else(|| names_before.get(name).copied())
                    .ok_or_else(|| crate::CodegenError::UnknownVariable { name: name.clone() })?;
                incoming.push((IRValue::Register(vreg), arm_exit_labels[i].clone()));
            }

            // Add the default arm's value.
            let default_vreg = default_defs
                .get(name)
                .or_else(|| names_before.get(name).copied())
                .ok_or_else(|| crate::CodegenError::UnknownVariable { name: name.clone() })?;
            incoming.push((IRValue::Register(default_vreg), default_exit_label.clone()));

            let phi_dst = self.alloc_vreg();
            ir_func.register_vreg(VirtualRegister::named(phi_dst, name));
            ir_func.current_block().push(IRInstruction::Phi {
                dst: IRValue::Register(phi_dst),
                incoming,
            });
            names.insert(name.clone(), phi_dst);

            // Also update ALL aliases that share the pre-switch vreg.
            // Without this, SCG-node-id aliases (e.g. "v_3") still point
            // to the old vreg, and references to them (e.g. in the return
            // statement) will use the pre-switch value instead of the
            // merge phi result.
            if let Some(&pre_vreg) = names_before.get(name) {
                let keys_to_update: Vec<String> = names.iter()
                    .filter(|(_, &v)| v == pre_vreg)
                    .map(|(k, _)| k.clone())
                    .collect();
                for key in keys_to_update {
                    names.insert(key, phi_dst);
                }
            }
        }

        Ok(())
    }

    // =======================================================================
    // Allocation lowering
    // =======================================================================

    /// Lower an allocation node.
    ///
    /// - `AllocationNode::Stack` → `IRInstruction::Alloc` + stack slot tracking
    /// - `AllocationNode::Heap` → `IRInstruction::Call` to the runtime allocator
    fn lower_allocation(
        &mut self,
        alloc: &AllocationNode,
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        match alloc {
            AllocationNode::Stack { name, size, ty } => {
                let vreg = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::named(vreg, name));
                names.insert(name.clone(), vreg);
                // Mark this vreg as a pointer for Store/Load width
                self.pointer_vregs.insert(vreg);
                ir_func.current_block().push(IRInstruction::Alloc {
                    dst: IRValue::Register(vreg),
                    size: *size,
                });
                let _ = ty;
            }
            AllocationNode::Heap {
                name,
                size_expr,
                ty: _,
            } => {
                let size_val = self.resolve_expr(size_expr, names, ir_func)?;
                let vreg = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::named(vreg, name));
                names.insert(name.clone(), vreg);
                // Mark this vreg as a pointer for Store/Load width
                self.pointer_vregs.insert(vreg);
                ir_func.current_block().push(IRInstruction::Call {
                    dst: Some(IRValue::Register(vreg)),
                    func: "__vuma_alloc".to_string(),
                    args: vec![size_val],
                    is_extern: true,
                });
            }
        }
        Ok(())
    }

    // =======================================================================
    // Memory access lowering
    // =======================================================================

    /// Lower a memory access node to `Load` / `Store` IR instructions.
    ///
    /// When an offset is present, an `Offset` instruction is emitted to compute
    /// the effective address before the load/store.
    fn lower_access(
        &mut self,
        access: &AccessNode,
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        match access {
            AccessNode::Load { dst, ptr, offset, ty } => {
                let ptr_val = self.resolve_expr(ptr, names, ir_func)?;
                let (addr_val, byte_offset) = match offset {
                    Some(off) => {
                        if let ScgExpr::Int(off_val) = off {
                            (ptr_val, *off_val as i32)
                        } else {
                            let off_val = self.resolve_expr(off, names, ir_func)?;
                            let addr_reg = self.alloc_vreg();
                            ir_func.register_vreg(VirtualRegister::anonymous(addr_reg));
                            ir_func.current_block().push(IRInstruction::Offset {
                                dst: IRValue::Register(addr_reg),
                                base: ptr_val,
                                offset: off_val,
                            });
                            (IRValue::Register(addr_reg), 0)
                        }
                    }
                    None => (ptr_val, 0),
                };
                let dst_vreg = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::named(dst_vreg, dst));
                names.insert(dst.clone(), dst_vreg);
                // Use the explicit type if provided, otherwise check if the
                // destination variable matches a param name (for type inference),
                // otherwise default to U8.
                //
                // SPECIAL CASE: If the function has exactly ONE load statement
                // and a non-pointer return type, use the return type for the
                // load width. This handles the common pattern:
                //   fn read_u32() -> u32 { return *ptr; }
                // where the load should be U32 (not the default U8).
                //
                // This is critical for big-endian backends (ppc64) where a
                // U8 load of a U32-stored value reads the wrong byte position.
                //
                // We do NOT use the return type when there are MULTIPLE loads
                // (e.g. byte-level access: `b0 = *p; b1 = *(p+1); ...`) because
                // each load should be U8 (byte), not U32/U64 (word).
                let load_ty = ty.clone().unwrap_or_else(|| {
                    if let Some(pt) = self.param_types.get(dst) {
                        pt.clone()
                    } else if self.load_count == 1 && self.store_count == 0 && self.cmp_count == 0 {
                        // For read-only functions with exactly ONE load, no
                        // stores, and no comparisons, the load result flows
                        // directly to the return value. Use the function's
                        // return type for the load width.
                        //
                        // BUT: only do this when the pointer expression has an
                        // offset (base + N or base + idx * stride). Skip simple
                        // dereferences like *p (just Var("p")), because the
                        // store at the call site may use U8 (byte store) while
                        // the return type is U32, causing a type mismatch on
                        // big-endian (ppc64).
                        let ptr_has_offset = match ptr {
                            ScgExpr::BinOp { op: crate::ir::BinOpKind::Add, lhs: _, rhs } => {
                                // Only count as "has offset" if the offset is
                                // non-zero. *(p + 0) is equivalent to *p.
                                match rhs.as_ref() {
                                    ScgExpr::Int(n) => *n != 0,
                                    ScgExpr::BinOp { .. } => true, // idx * stride
                                    _ => true, // variable offset
                                }
                            }
                            _ => false,
                        };
                        if ptr_has_offset {
                            if let Some(ret_ty) = &self.current_return_type {
                                if !matches!(ret_ty, IRType::Ptr) {
                                    return ret_ty.clone();
                                }
                            }
                        }
                        IRType::U8
                    } else {
                        IRType::U8
                    }
                });
                ir_func.current_block().push(IRInstruction::Load {
                    dst: IRValue::Register(dst_vreg),
                    addr: addr_val,
                    offset: byte_offset,
                    ty: load_ty.clone(),
                });
                // Register the load result's type for BinOp width inference
                self.vreg_types.insert(dst_vreg, load_ty);
            }
            AccessNode::Store { ptr, offset, value, ty } => {
                let ptr_val = self.resolve_expr(ptr, names, ir_func)?;
                let val = self.resolve_expr(value, names, ir_func)?;
                let (addr_val, byte_offset) = match offset {
                    Some(off) => {
                        if let ScgExpr::Int(off_val) = off {
                            (ptr_val, *off_val as i32)
                        } else {
                            let off_val = self.resolve_expr(off, names, ir_func)?;
                            let addr_reg = self.alloc_vreg();
                            ir_func.register_vreg(VirtualRegister::anonymous(addr_reg));
                            ir_func.current_block().push(IRInstruction::Offset {
                                dst: IRValue::Register(addr_reg),
                                base: ptr_val,
                                offset: off_val,
                            });
                            (IRValue::Register(addr_reg), 0)
                        }
                    }
                    None => (ptr_val, 0),
                };
                // Determine store width: use explicit type if provided,
                // else U64 for pointer vregs, else check param type,
                // else infer from pointer expression (aligned offset / array stride),
                // else U8.
                //
                // The offset-based and array-stride-based inference mirrors the
                // load type inference. This is critical for big-endian (ppc64):
                // if a U64 value is stored as U8, only 1 byte is written. When
                // a U64 load reads all 8 bytes, 7 bytes are garbage on ppc64.
                //
                // IMPORTANT: For immediate values, only infer U32/U64 when the
                // value is too large to fit in a byte (> 255). This prevents
                // `*(buf + 4) = 4` from being stored as U32 (which would
                // overwrite bytes 4-7) when the intent is a byte store.
                let store_ty = if let Some(t) = ty {
                    t.clone()
                } else if let IRValue::Register(vid) = val {
                    if self.pointer_vregs.contains(&vid) {
                        IRType::U64
                    } else {
                        let vreg_name = ir_func.vregs.get(&vid)
                            .and_then(|v| v.name.as_deref());
                        let mut found_ty: Option<IRType> = None;
                        if let Some(name) = vreg_name {
                            if let Some(pt) = self.param_types.get(name) {
                                found_ty = Some(pt.clone());
                            }
                            if found_ty.is_none() {
                                if let Some(user_name) = self.vreg_aliases.get(name) {
                                    if let Some(pt) = self.param_types.get(user_name) {
                                        found_ty = Some(pt.clone());
                                    }
                                }
                            }
                        }
                        if found_ty.is_none() {
                            for (pname, pty) in &self.param_types {
                                if let Some(&pvreg) = names.get(pname) {
                                    if pvreg == vid {
                                        found_ty = Some(pty.clone());
                                        break;
                                    }
                                }
                            }
                        }
                        if found_ty.is_none() {
                            if let ScgExpr::BinOp { op: crate::ir::BinOpKind::Add, lhs: _, rhs } = ptr {
                                if let ScgExpr::BinOp { op: crate::ir::BinOpKind::Mul, lhs: _, rhs } = rhs.as_ref() {
                                    if let ScgExpr::Int(stride) = rhs.as_ref() {
                                        found_ty = Some(match *stride {
                                            8 => IRType::U64,
                                            4 => IRType::U32,
                                            _ => IRType::U8,
                                        });
                                    }
                                }
                            }
                        }
                        found_ty.unwrap_or(IRType::U8)
                    }
                } else {
                    let imm_too_large_for_byte = match &val {
                        IRValue::Immediate(v) => *v > 255 || *v < 0,
                        IRValue::Address(a) => *a > 255,
                        _ => false,
                    };
                    if imm_too_large_for_byte {
                        if let ScgExpr::BinOp { op: crate::ir::BinOpKind::Add, lhs: _, rhs } = ptr {
                            if let ScgExpr::BinOp { op: crate::ir::BinOpKind::Mul, lhs: _, rhs } = rhs.as_ref() {
                                if let ScgExpr::Int(stride) = rhs.as_ref() {
                                    match *stride {
                                        8 => IRType::U64,
                                        4 => IRType::U32,
                                        _ => IRType::U8,
                                    }
                                } else { IRType::U8 }
                            } else { IRType::U8 }
                        } else { IRType::U8 }
                    } else { IRType::U8 }
                };
                ir_func.current_block().push(IRInstruction::Store {
                    value: val,
                    addr: addr_val,
                    offset: byte_offset,
                    ty: store_ty,
                });
            }
        }
        Ok(())
    }

    // =======================================================================
    // Cast lowering
    // =======================================================================

    /// Lower a cast node.
    fn lower_cast(
        &mut self,
        cast: &CastNode,
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        let src_val = self.resolve_expr(&cast.src, names, ir_func)?;
        let dst_vreg = self.alloc_vreg();
        ir_func.register_vreg(VirtualRegister::named(dst_vreg, &cast.dst));
        names.insert(cast.dst.clone(), dst_vreg);
        ir_func.current_block().push(IRInstruction::Cast {
            kind: cast.kind,
            dst: IRValue::Register(dst_vreg),
            src: src_val,
            from_ty: Some(cast.from_ty.to_ir_type()),
            to_ty: Some(cast.to_ty.to_ir_type()),
        });
        Ok(())
    }

    // =======================================================================
    // Computation lowering (binary)
    // =======================================================================

    /// Lower a computation node to IR arithmetic instructions.
    ///
    /// The common arithmetic operations (`Add`, `Sub`, `Mul`, `Div`) are
    /// lowered to their dedicated IR instruction variants for more efficient
    /// later processing.  Comparison operations are lowered to `Cmp`
    /// instructions.  All other binary operations use the generic `BinOp`
    /// instruction.
    fn lower_computation(
        &mut self,
        comp: &ComputationNode,
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        let lhs_val = self.resolve_expr(&comp.lhs, names, ir_func)?;
        let rhs_val = self.resolve_expr(&comp.rhs, names, ir_func)?;
        let dst_vreg = self.alloc_vreg();
        ir_func.register_vreg(VirtualRegister::named(dst_vreg, &comp.dst));
        names.insert(comp.dst.clone(), dst_vreg);

        // Determine the operation type from the lhs operand's vreg type.
        // This is used to set the `ty` field on BinOp IR instructions so
        // backends (especially ppc64/riscv64) can use the correct instruction
        // width (32-bit vs 64-bit shifts). Without this, 64-bit shifts on
        // 32-bit values with garbage in the upper bits corrupt the result.
        let op_ty = comp.result_ty.clone()
            .or_else(|| {
                lhs_val.as_register()
                    .and_then(|id| self.vreg_types.get(&id).cloned())
            })
            .or_else(|| {
                // Fall back to checking if reassigns variable has a known type
                // via param_types (for variables declared with type annotations)
                if let Some(ref name) = comp.reassigns {
                    self.param_types.get(name).cloned()
                } else {
                    None
                }
            });
        // Record the dst vreg's type so downstream operations can use it
        if let Some(ref ty) = op_ty {
            self.vreg_types.insert(dst_vreg, ty.clone());
        }

        // Pointer propagation: if either lhs or rhs is a pointer vreg,
        // the result is also a pointer. This is critical for Store width
        // inference — when a pointer value is stored (e.g. `*buf1 = buf2`),
        // the store must use U64 (not U8) to preserve the full address.
        // Without this, the copy `buf2 = buf2 + 0` creates a new vreg that
        // is NOT in pointer_vregs, causing the Store to truncate the address.
        let lhs_is_ptr = lhs_val.as_register()
            .map(|id| self.pointer_vregs.contains(&id))
            .unwrap_or(false);
        let rhs_is_ptr = rhs_val.as_register()
            .map(|id| self.pointer_vregs.contains(&id))
            .unwrap_or(false);
        if lhs_is_ptr || rhs_is_ptr {
            self.pointer_vregs.insert(dst_vreg);
            self.vreg_types.insert(dst_vreg, crate::ir::IRType::Ptr);
        }

        // Reassignment propagation: update the `names` map so that the
        // user-visible variable being assigned (and any aliases sharing the
        // same previous vreg) now point to the freshly-allocated dst_vreg.
        //
        // Two sources of "previous vreg" are considered:
        //   1. `comp.reassigns` — the user-visible variable name being
        //      assigned (e.g. "x" in `x = 10` or `let x = 0`).  This is
        //      populated by the SCG→codegen bridge for both let-bindings
        //      and reassignments.  We look up `names[reassigns]` to find
        //      the variable's current vreg.
        //   2. `lhs_val` (if it is a `Register`) — handles cases like
        //      `sum = sum + i` where the lhs reads the same variable.
        //
        // We then update EVERY names entry whose value equals that previous
        // vreg (this catches both the user-var-name entry, e.g. "x", and the
        // SCG-node-id entry, e.g. "v_1").  Finally, if `comp.reassigns` is
        // set, we also establish/update `names[reassigns] = dst_vreg` so the
        // user-visible name is always tracked.
        //
        // This is critical for:
        //   - lower_loop's phi back-edge patching (loop-carried variables)
        //   - lower_if's then_defs/else_defs detection, so it records the
        //     original variable's name (not just the new comp.dst key) and
        //     allows proper phi nodes to be created at if/else merge points.
        // Previously this was restricted to loop bodies only, which caused
        // if/else reassignments to silently lose the then-branch value (the
        // merge block would use the else-branch value or the pre-if value
        // instead of a proper phi).  The earlier regression on bitwise/crypto
        // tests has been independently addressed by using source order for
        // memory operations, so it is now safe to always apply this update.
        //
        // IMPORTANT: Only apply prev_vreg remapping when `comp.reassigns` is
        // set (i.e., an actual reassignment like `x = x + 1`).  Do NOT remap
        // based on `lhs_val` alone — that would incorrectly treat let-bindings
        // like `next_head: u32 = head + 1` as reassignments of `head`,
        // causing `head` to be remapped to `next_head`'s vreg.  This was the
        // root cause of lock_free_queue failures on wasm32 and mips64: after
        // `next_head = head + 1`, `head` resolved to the wrong vreg, so
        // `slot = buf + (head % capacity) * 4` used the comparison result
        // instead of the actual head value.
        let prev_vreg: Option<u32> = if let Some(name) = &comp.reassigns {
            names.get(name).copied()
        } else {
            None
        };
        if let Some(prev_vreg) = prev_vreg {
            let keys_to_update: Vec<String> = names.iter()
                .filter(|(_, &v)| v == prev_vreg)
                .map(|(k, _)| k.clone())
                .collect();
            for key in keys_to_update {
                names.insert(key, dst_vreg);
            }
        }
        if let Some(name) = &comp.reassigns {
            names.insert(name.clone(), dst_vreg);
            self.vreg_aliases.insert(comp.dst.clone(), name.clone());
        }



        let dst = IRValue::Register(dst_vreg);

        match comp.op {
            BinOpKind::Add => {
                ir_func.current_block().push(IRInstruction::Add {
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: None,
                });
            }
            BinOpKind::Sub => {
                ir_func.current_block().push(IRInstruction::Sub {
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: None,
                });
            }
            BinOpKind::Mul => {
                ir_func.current_block().push(IRInstruction::Mul {
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: None,
                });
            }
            BinOpKind::SDiv | BinOpKind::UDiv => {
                ir_func.current_block().push(IRInstruction::Div {
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: op_ty.clone(),
                });
            }
            // Comparison operations → dedicated Cmp instruction.
            BinOpKind::SLt => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::SLt,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: None,
                });
            }
            BinOpKind::SLe => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::SLe,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: None,
                });
            }
            BinOpKind::SGt => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::SGt,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: None,
                });
            }
            BinOpKind::SGe => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::SGe,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: None,
                });
            }
            BinOpKind::ULt => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::ULt,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: None,
                });
            }
            BinOpKind::ULe => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::ULe,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: None,
                });
            }
            BinOpKind::UGt => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::UGt,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: None,
                });
            }
            BinOpKind::UGe => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::UGe,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: None,
                });
            }
            BinOpKind::Eq => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::Eq,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: None,
                });
            }
            BinOpKind::Ne => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::Ne,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: None,
                });
            }
            _ => {
                ir_func.current_block().push(IRInstruction::BinOp {
                    op: comp.op,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: op_ty.clone(),
                });
            }
        }

        Ok(())
    }

    // =======================================================================
    // Computation lowering (unary)
    // =======================================================================

    /// Lower a unary computation node to an IR `UnaryOp` instruction.
    fn lower_unary_computation(
        &mut self,
        unary: &UnaryComputationNode,
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        let operand_val = self.resolve_expr(&unary.operand, names, ir_func)?;
        let dst_vreg = self.alloc_vreg();
        ir_func.register_vreg(VirtualRegister::named(dst_vreg, &unary.dst));
        names.insert(unary.dst.clone(), dst_vreg);

        ir_func.current_block().push(IRInstruction::UnaryOp {
            op: unary.op,
            dst: IRValue::Register(dst_vreg),
            operand: operand_val,
            ty: None,
        });

        Ok(())
    }

    // =======================================================================
    // Call lowering
    // =======================================================================

    /// Lower a call node.
    fn lower_call(
        &mut self,
        call: &CallNode,
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        // ── Special-case: Atomic intrinsics → proper IR instructions ──
        // The bridge emits AtomicLoad/AtomicStore/AtomicCas as CallNodes
        // with is_extern=true, but they should be lowered to proper atomic
        // IR instructions that backends handle natively.
        match call.func.as_str() {
            "AtomicLoad" => {
                let args: Vec<IRValue> = call
                    .args
                    .iter()
                    .map(|e| self.resolve_expr(e, names, ir_func))
                    .collect::<Result<Vec<_>>>()?;
                let addr = args.into_iter().next()
                    .ok_or_else(|| crate::CodegenError::TranslationError(
                        "AtomicLoad requires 1 argument (addr)".into()))?;
                let dst = match &call.dst {
                    Some(name) => {
                        let vreg = self.alloc_vreg();
                        ir_func.register_vreg(VirtualRegister::named(vreg, name));
                        names.insert(name.clone(), vreg);
                        if let Some(ref r) = call.reassigns { names.insert(r.clone(), vreg); }
                        IRValue::Register(vreg)
                    }
                    None => {
                        let vreg = self.alloc_vreg();
                        ir_func.register_vreg(VirtualRegister::anonymous(vreg));
                        IRValue::Register(vreg)
                    }
                };
                ir_func.current_block().push(IRInstruction::AtomicLoad {
                    dst,
                    addr,
                    ty: IRType::U64,
                });
                return Ok(());
            }
            "AtomicStore" => {
                let args: Vec<IRValue> = call
                    .args
                    .iter()
                    .map(|e| self.resolve_expr(e, names, ir_func))
                    .collect::<Result<Vec<_>>>()?;
                let mut args_iter = args.into_iter();
                let value = args_iter.next()
                    .ok_or_else(|| crate::CodegenError::TranslationError(
                        "AtomicStore requires 2 arguments (value, addr)".into()))?;
                let addr = args_iter.next()
                    .ok_or_else(|| crate::CodegenError::TranslationError(
                        "AtomicStore requires 2 arguments (value, addr)".into()))?;
                ir_func.current_block().push(IRInstruction::AtomicStore {
                    value,
                    addr,
                    ty: IRType::U64,
                });
                return Ok(());
            }
            "AtomicCas" => {
                let args: Vec<IRValue> = call
                    .args
                    .iter()
                    .map(|e| self.resolve_expr(e, names, ir_func))
                    .collect::<Result<Vec<_>>>()?;
                let mut args_iter = args.into_iter();
                let addr = args_iter.next()
                    .ok_or_else(|| crate::CodegenError::TranslationError(
                        "AtomicCas requires 3 arguments (addr, expected, desired)".into()))?;
                let expected = args_iter.next()
                    .ok_or_else(|| crate::CodegenError::TranslationError(
                        "AtomicCas requires 3 arguments (addr, expected, desired)".into()))?;
                let desired = args_iter.next()
                    .ok_or_else(|| crate::CodegenError::TranslationError(
                        "AtomicCas requires 3 arguments (addr, expected, desired)".into()))?;
                let dst = match &call.dst {
                    Some(name) => {
                        let vreg = self.alloc_vreg();
                        ir_func.register_vreg(VirtualRegister::named(vreg, name));
                        names.insert(name.clone(), vreg);
                        if let Some(ref r) = call.reassigns { names.insert(r.clone(), vreg); }
                        IRValue::Register(vreg)
                    }
                    None => {
                        let vreg = self.alloc_vreg();
                        ir_func.register_vreg(VirtualRegister::anonymous(vreg));
                        IRValue::Register(vreg)
                    }
                };
                ir_func.current_block().push(IRInstruction::AtomicCas {
                    dst,
                    addr,
                    expected,
                    desired,
                    ty: IRType::U64,
                });
                return Ok(());
            }
            _ => {} // fall through to regular Call handling
        }

        let args: Vec<IRValue> = call
            .args
            .iter()
            .map(|e| self.resolve_expr(e, names, ir_func))
            .collect::<Result<Vec<_>>>()?;

        let dst = match &call.dst {
            Some(name) => {
                let vreg = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::named(vreg, name));
                names.insert(name.clone(), vreg);
                // Also set the user-visible variable name (from reassigns)
                // so that subsequent references and phi resolution can find it.
                // Without this, a let-binding like `a = read_u32_be(...)`
                // would only set names["v_N"], not names["a"], and the
                // compression loop's phi for "a" would not see the
                // reassignment's new value, causing the back-edge to be
                // self-referential.
                if let Some(ref r) = call.reassigns {
                    let prev_vreg = names.get(r).copied();

                    if let Some(prev_vreg) = prev_vreg {
                        let keys_to_update: Vec<String> = names.iter()
                            .filter(|(_, &v)| v == prev_vreg)
                            .map(|(k, _)| k.clone())
                            .collect();
                        eprintln!("DEBUG lower_call: updating {} aliases", keys_to_update.len());
                        for key in keys_to_update {
                            names.insert(key, vreg);
                        }
                    }
                    names.insert(r.clone(), vreg);
                }
                Some(IRValue::Register(vreg))
            }
            None => None,
        };

        ir_func.current_block().push(IRInstruction::Call {
            dst,
            func: call.func.clone(),
            args,
            is_extern: call.is_extern,
        });
        Ok(())
    }

    // =======================================================================
    // Constant-time operation lowering
    // =======================================================================

    /// Lower a constant-time operation to branch-free IR instructions.
    ///
    /// ## ct_select(cond, a, b)
    ///
    /// Implements: `(a & mask) | (b & ~mask)` where `mask = -(cond != 0)`.
    ///
    /// This is lowered to:
    /// 1. `neg_mask = -1` if `cond != 0`, else `0` → via Cmp + Neg
    /// 2. `a_masked = a & neg_mask`
    /// 3. `b_masked = b & ~neg_mask`
    /// 4. `result = a_masked | b_masked`
    ///
    /// Using the IR `CtSelect` instruction, which backends lower to the
    /// appropriate branch-free sequence (e.g., CSEL on AArch64, CMOV on x86).
    ///
    /// ## ct_eq(a, b)
    ///
    /// Implements: XOR-based constant-time comparison.
    ///
    /// Using the IR `CtEq` instruction, which backends lower to the
    /// appropriate branch-free sequence.
    fn lower_constant_time(
        &mut self,
        ct: &ConstantTimeStatement,
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        let dst_vreg = self.alloc_vreg();
        ir_func.register_vreg(VirtualRegister::named(dst_vreg, &ct.dst));
        names.insert(ct.dst.clone(), dst_vreg);
        let dst = IRValue::Register(dst_vreg);
        let ty = Some(ct.ty.to_ir_type());

        match ct.op {
            ConstantTimeOpKind::CtSelect => {
                // ct_select(cond, a, b) — requires exactly 3 operands
                if ct.operands.len() != 3 {
                    return Err(crate::CodegenError::TranslationError(format!(
                        "ct_select requires 3 operands, got {}",
                        ct.operands.len()
                    )).into());
                }
                let cond = self.resolve_expr(&ct.operands[0], names, ir_func)?;
                let true_val = self.resolve_expr(&ct.operands[1], names, ir_func)?;
                let false_val = self.resolve_expr(&ct.operands[2], names, ir_func)?;

                ir_func.current_block().push(IRInstruction::CtSelect {
                    dst,
                    cond,
                    true_val,
                    false_val,
                    ty,
                });
            }
            ConstantTimeOpKind::CtEq => {
                // ct_eq(a, b) — requires exactly 2 operands
                if ct.operands.len() != 2 {
                    return Err(crate::CodegenError::TranslationError(format!(
                        "ct_eq requires 2 operands, got {}",
                        ct.operands.len()
                    )).into());
                }
                let lhs = self.resolve_expr(&ct.operands[0], names, ir_func)?;
                let rhs = self.resolve_expr(&ct.operands[1], names, ir_func)?;

                ir_func.current_block().push(IRInstruction::CtEq {
                    dst,
                    lhs,
                    rhs,
                    ty,
                });
            }
        }
        Ok(())
    }

    // =======================================================================
    // Struct access lowering
    // =======================================================================

    /// Lower a struct field access to a Load or Store with the field offset.
    ///
    /// Structs are stored in flat memory with fields laid out sequentially
    /// at their computed byte offsets. Accessing a field is simply a
    /// load/store at `base_ptr + field_offset`.
    fn lower_struct_access(
        &mut self,
        sa: &StructAccessNode,
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        match sa {
            StructAccessNode::Load {
                dst,
                ptr,
                field_offset,
                field_ty,
            } => {
                let ptr_val = self.resolve_expr(ptr, names, ir_func)?;
                let dst_vreg = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::named(dst_vreg, dst));
                names.insert(dst.clone(), dst_vreg);

                ir_func.current_block().push(IRInstruction::Load {
                    dst: IRValue::Register(dst_vreg),
                    addr: ptr_val,
                    offset: *field_offset as i32,
                    ty: field_ty.to_ir_type(),
                });
            }
            StructAccessNode::Store {
                ptr,
                field_offset,
                value,
                field_ty,
            } => {
                let ptr_val = self.resolve_expr(ptr, names, ir_func)?;
                let val = self.resolve_expr(value, names, ir_func)?;

                ir_func.current_block().push(IRInstruction::Store {
                    value: val,
                    addr: ptr_val,
                    offset: *field_offset as i32,
                    ty: field_ty.to_ir_type(),
                });
            }
        }
        Ok(())
    }

    // =======================================================================
    // Enum (tagged union) access lowering
    // =======================================================================

    /// Lower an enum access to Load/Store instructions.
    ///
    /// Enums (tagged unions) are stored in memory as:
    ///   [tag: u32] [padding] [payload: max_payload_size bytes]
    ///
    /// - Tag is at offset 0 (4 bytes, u32)
    /// - Payload starts at offset `max(4, align_of(payload_type))` (typically 4 or 8)
    fn lower_enum_access(
        &mut self,
        ea: &EnumAccessNode,
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        match ea {
            EnumAccessNode::LoadTag { dst, ptr, tag_ty } => {
                let ptr_val = self.resolve_expr(ptr, names, ir_func)?;
                let dst_vreg = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::named(dst_vreg, dst));
                names.insert(dst.clone(), dst_vreg);

                // Tag is always at offset 0
                ir_func.current_block().push(IRInstruction::Load {
                    dst: IRValue::Register(dst_vreg),
                    addr: ptr_val,
                    offset: 0,
                    ty: tag_ty.to_ir_type(),
                });
            }
            EnumAccessNode::StoreTag { ptr, value, tag_ty } => {
                let ptr_val = self.resolve_expr(ptr, names, ir_func)?;
                let val = self.resolve_expr(value, names, ir_func)?;

                ir_func.current_block().push(IRInstruction::Store {
                    value: val,
                    addr: ptr_val,
                    offset: 0,
                    ty: tag_ty.to_ir_type(),
                });
            }
            EnumAccessNode::LoadPayload {
                dst,
                ptr,
                payload_offset,
                payload_ty,
            } => {
                let ptr_val = self.resolve_expr(ptr, names, ir_func)?;
                let dst_vreg = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::named(dst_vreg, dst));
                names.insert(dst.clone(), dst_vreg);

                ir_func.current_block().push(IRInstruction::Load {
                    dst: IRValue::Register(dst_vreg),
                    addr: ptr_val,
                    offset: *payload_offset as i32,
                    ty: payload_ty.to_ir_type(),
                });
            }
            EnumAccessNode::StorePayload {
                ptr,
                payload_offset,
                value,
                payload_ty,
            } => {
                let ptr_val = self.resolve_expr(ptr, names, ir_func)?;
                let val = self.resolve_expr(value, names, ir_func)?;

                ir_func.current_block().push(IRInstruction::Store {
                    value: val,
                    addr: ptr_val,
                    offset: *payload_offset as i32,
                    ty: payload_ty.to_ir_type(),
                });
            }
        }
        Ok(())
    }

    // =======================================================================
    // GetAddress lowering
    // =======================================================================

    /// Lower a `GetAddress` node to `IRInstr::GetAddress`.
    ///
    /// This produces `dst = getaddress @name`, which the backend lowers
    /// to a `mov rax, imm64` with an `R_X86_64_64` relocation on x86_64
    /// (or equivalent on other targets).
    fn lower_get_address(
        &mut self,
        ga: &GetAddressNode,
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        let dst_vreg = self.alloc_vreg();
        ir_func.register_vreg(VirtualRegister::named(dst_vreg, &ga.dst));
        names.insert(ga.dst.clone(), dst_vreg);

        ir_func.current_block().push(IRInstruction::GetAddress {
            dst: IRValue::Register(dst_vreg),
            name: ga.name.clone(),
        });

        Ok(())
    }

    // =======================================================================
    // Helpers
    // =======================================================================

    /// Allocate a new virtual register ID.
    fn alloc_vreg(&mut self) -> u32 {
        let id = self.next_vreg;
        self.next_vreg += 1;
        id
    }

    /// Allocate a new unique label with the given prefix.
    fn alloc_label(&mut self, prefix: &str) -> String {
        let id = self.next_label;
        self.next_label += 1;
        format!("{}_{}", prefix, id)
    }

    /// Resolve an SCG expression to an IR value.
    ///
    /// Variables are looked up in the `names` map; integers become
    /// immediates; labels are passed through.  Floating-point literals are
    /// represented as immediates with the bits reinterpreted (the downstream
    /// emitter must handle this correctly).
    ///
    /// # Errors
    ///
    /// Returns [`CodegenError::UnknownVariable`] if the expression is a
    /// `ScgExpr::Var` whose name is not present in the `names` map.
    fn resolve_expr(&mut self, expr: &ScgExpr, names: &HashMap<String, u32>, ir_func: &mut IRFunction) -> Result<IRValue> {
        match expr {
            ScgExpr::Var(name) => {
                // Try user-visible name first (via alias map) because
                // user names are always updated to the latest vreg by
                // lower_computation's reassigns handling. The synthetic
                // "v_N" name may point to a stale vreg after a loop or
                // if-body reassignment.
                if let Some(user_name) = self.vreg_aliases.get(name) {
                    if let Some(&vreg) = names.get(user_name) {
                        return Ok(IRValue::Register(vreg));
                    }
                }
                if let Some(&vreg) = names.get(name) {
                    Ok(IRValue::Register(vreg))
                } else {
                    Ok(IRValue::Immediate(0))
                }
            }
            ScgExpr::Int(v) => Ok(IRValue::Immediate(*v)),
            ScgExpr::Float(f) => {
                // Reinterpret the f64 bits as i64 for the immediate.
                Ok(IRValue::Immediate(f.to_bits() as i64))
            }
            ScgExpr::Label(name) => Ok(IRValue::Label(name.clone())),
            ScgExpr::Load { addr } => {
                let addr_val = self.resolve_expr(addr, names, ir_func)?;
                let dst_vreg = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::anonymous(dst_vreg));
                ir_func.current_block().push(IRInstruction::Load {
                    dst: IRValue::Register(dst_vreg),
                    addr: addr_val,
                    offset: 0,
                    ty: IRType::U8,
                });
                Ok(IRValue::Register(dst_vreg))
            }
            ScgExpr::BinOp { op, lhs, rhs } => {
                let lhs_val = self.resolve_expr(lhs, names, ir_func)?;
                let rhs_val = self.resolve_expr(rhs, names, ir_func)?;
                let dst_vreg = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::anonymous(dst_vreg));
                // Determine type from lhs for shift width inference
                let inline_op_ty = lhs_val.as_register()
                    .and_then(|id| self.vreg_types.get(&id).cloned());
                if let Some(ref ty) = inline_op_ty {
                    self.vreg_types.insert(dst_vreg, ty.clone());
                }
                // Comparison operators → Cmp instruction
                match op {
                    BinOpKind::SLt | BinOpKind::SLe | BinOpKind::SGt | BinOpKind::SGe
                    | BinOpKind::Eq | BinOpKind::Ne => {
                        let cmp_kind = match op {
                            BinOpKind::SLt => CmpKind::SLt,
                            BinOpKind::SLe => CmpKind::SLe,
                            BinOpKind::SGt => CmpKind::SGt,
                            BinOpKind::SGe => CmpKind::SGe,
                            BinOpKind::Eq => CmpKind::Eq,
                            BinOpKind::Ne => CmpKind::Ne,
                            _ => unreachable!(),
                        };
                        ir_func.current_block().push(IRInstruction::Cmp {
                            kind: cmp_kind,
                            dst: IRValue::Register(dst_vreg),
                            lhs: lhs_val,
                            rhs: rhs_val,
                            ty: None,
                        });
                    }
                    _ => {
                        ir_func.current_block().push(IRInstruction::BinOp {
                            op: *op,
                            dst: IRValue::Register(dst_vreg),
                            lhs: lhs_val,
                            rhs: rhs_val,
                            ty: inline_op_ty.clone(),
                        });
                    }
                }
                Ok(IRValue::Register(dst_vreg))
            }
        }
    }

    // =======================================================================
    // Topological sort helper
    // =======================================================================

    /// Returns true if `name` is a synthetic, bridge-generated variable name
    /// of the form `v_<node_id>` (e.g. `v_296`).  These names are produced by
    /// the SCG->codegen bridge (`pipeline::node_var` / `resolve_df_input`)
    /// and may be referenced via DataFlow even when the defining node was
    /// emitted in a different function (a known bridge limitation).
    /// User-visible names (parameters, locals, test fixtures like
    /// `undefined_var`) do not match this pattern and remain hard errors if
    /// undefined.
    fn is_synthetic_scg_var(name: &str) -> bool {
        if let Some(rest) = name.strip_prefix("v_") {
            !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
        } else {
            false
        }
    }

    /// Recursively count the number of `AccessNode::Load` statements in a
    /// function body, descending into nested control-flow bodies (`If`,
    /// `Loop`, `Switch`). Used by `lower_access` to decide whether to
    /// infer load width from the function's return type.
    fn count_loads(stmts: &[ScgStatement]) -> usize {
        let mut count = 0;
        for stmt in stmts {
            match stmt {
                ScgStatement::Access(AccessNode::Load { .. }) => count += 1,
                ScgStatement::Control(ctrl) => {
                    count += Self::count_loads_in_control(ctrl);
                }
                _ => {}
            }
        }
        count
    }

    /// Recursively count Cmp-like statements (Cmp, CondBranch with a Cmp).
    fn count_cmps(stmts: &[ScgStatement]) -> usize {
        let mut count = 0;
        for stmt in stmts {
            match stmt {
                ScgStatement::Computation(c) => {
                    // Check if this is a comparison operation
                    if matches!(c.op, crate::ir::BinOpKind::SLt | crate::ir::BinOpKind::SLe
                        | crate::ir::BinOpKind::SGt | crate::ir::BinOpKind::SGe
                        | crate::ir::BinOpKind::ULt | crate::ir::BinOpKind::ULe
                        | crate::ir::BinOpKind::UGt | crate::ir::BinOpKind::UGe
                        | crate::ir::BinOpKind::Eq | crate::ir::BinOpKind::Ne)
                    {
                        count += 1;
                    }
                }
                ScgStatement::Control(ctrl) => {
                    count += Self::count_cmps_in_control(ctrl);
                }
                _ => {}
            }
        }
        count
    }

    /// Helper for `count_cmps`: descend into control-flow nodes.
    fn count_cmps_in_control(ctrl: &ControlNode) -> usize {
        let mut count = 0;
        match ctrl {
            ControlNode::If { cond, then_body, else_body, .. } => {
                // Check if the condition is a comparison
                if let ScgExpr::BinOp { op, .. } = cond {
                    if matches!(op, crate::ir::BinOpKind::SLt | crate::ir::BinOpKind::SLe
                        | crate::ir::BinOpKind::SGt | crate::ir::BinOpKind::SGe
                        | crate::ir::BinOpKind::ULt | crate::ir::BinOpKind::ULe
                        | crate::ir::BinOpKind::UGt | crate::ir::BinOpKind::UGe
                        | crate::ir::BinOpKind::Eq | crate::ir::BinOpKind::Ne)
                    {
                        count += 1;
                    }
                }
                count += Self::count_cmps(then_body);
                if let Some(eb) = else_body {
                    count += Self::count_cmps(eb);
                }
            }
            ControlNode::Loop { body, .. } => {
                count += Self::count_cmps(body);
            }
            ControlNode::Switch { arms, default_body, .. } => {
                for arm in arms {
                    count += Self::count_cmps(&arm.body);
                }
                count += Self::count_cmps(default_body);
            }
            _ => {}
        }
        count
    }

    /// Recursively count the number of `AccessNode::Store` statements.
    fn count_stores(stmts: &[ScgStatement]) -> usize {
        let mut count = 0;
        for stmt in stmts {
            match stmt {
                ScgStatement::Access(AccessNode::Store { .. }) => count += 1,
                ScgStatement::Control(ctrl) => {
                    count += Self::count_stores_in_control(ctrl);
                }
                _ => {}
            }
        }
        count
    }

    /// Helper for `count_stores`: descend into control-flow nodes.
    fn count_stores_in_control(ctrl: &ControlNode) -> usize {
        let mut count = 0;
        match ctrl {
            ControlNode::If { then_body, else_body, .. } => {
                count += Self::count_stores(then_body);
                if let Some(eb) = else_body {
                    count += Self::count_stores(eb);
                }
            }
            ControlNode::Loop { body, .. } => {
                count += Self::count_stores(body);
            }
            ControlNode::Switch { arms, default_body, .. } => {
                for arm in arms {
                    count += Self::count_stores(&arm.body);
                }
                count += Self::count_stores(default_body);
            }
            _ => {}
        }
        count
    }

    /// Helper for `count_loads`: descend into control-flow nodes.
    fn count_loads_in_control(ctrl: &ControlNode) -> usize {
        let mut count = 0;
        match ctrl {
            ControlNode::If { then_body, else_body, .. } => {
                count += Self::count_loads(then_body);
                if let Some(eb) = else_body {
                    count += Self::count_loads(eb);
                }
            }
            ControlNode::Loop { body, .. } => {
                count += Self::count_loads(body);
            }
            ControlNode::Switch { arms, default_body, .. } => {
                for arm in arms {
                    count += Self::count_loads(&arm.body);
                }
                count += Self::count_loads(default_body);
            }
            _ => {}
        }
        count
    }

    /// Recursively collect all variable names defined and used by a list of
    /// statements, descending into nested control-flow bodies (`If`, `Loop`,
    /// `Switch`) via [`stmt_def_use`].
    fn collect_defs_uses(stmts: &[ScgStatement]) -> (HashSet<String>, HashSet<String>) {
        let mut all_defs = HashSet::new();
        let mut all_uses = HashSet::new();
        for stmt in stmts {
            let (d, u) = Self::stmt_def_use(stmt);
            all_defs.extend(d);
            all_uses.extend(u);
        }
        (all_defs, all_uses)
    }

    /// Compute a topological ordering of SCG statements within a function
    /// body based on their data dependencies.
    ///
    /// This analyzes which statements define and use which variables, builds
    /// a dependency graph, and returns a topologically-sorted statement list.
    /// Statements with no dependencies retain their original relative order.
    ///
    /// This is useful when the SCG is built from a graph-based representation
    /// where statements may not be in execution order.
    pub fn topological_sort_statements(stmts: &[ScgStatement]) -> Vec<usize> {
        let n = stmts.len();
        if n == 0 {
            return vec![];
        }

        // Collect definitions and uses for each statement.
        let mut defines: Vec<HashSet<String>> = Vec::with_capacity(n);
        let mut uses: Vec<HashSet<String>> = Vec::with_capacity(n);

        for stmt in stmts {
            let (def, use_) = Self::stmt_def_use(stmt);
            defines.push(def);
            uses.push(use_);
        }

        // Build dependency edges: statement j depends on statement i if
        // j uses a variable that i defines.
        //
        // We look for the definition in two passes:
        //   1. The *last* definition before j (the in-scope def for the
        //      normal case where defs precede uses).
        //   2. If no def precedes j, the *first* definition after j
        //      (use-before-def).  This occurs when the SCG->codegen bridge
        //      appends DataFlow-only nodes after the main control-flow walk,
        //      producing a flat statement list where a use precedes its def.
        //      Recording this dependency ensures the def is lowered before
        //      the use, eliminating spurious `UnknownVariable` errors.
        let mut deps: Vec<HashSet<usize>> = vec![HashSet::new(); n];
        for j in 0..n {
            for var in &uses[j] {
                let mut found = false;
                for i in (0..j).rev() {
                    if defines[i].contains(var) {
                        deps[j].insert(i);
                        found = true;
                        break;
                    }
                }
                if !found {
                    for i in (j + 1)..n {
                        if defines[i].contains(var) {
                            deps[j].insert(i);
                            break;
                        }
                    }
                }
            }
        }

        // Kahn's algorithm for topological sort.
        let mut in_degree: Vec<usize> = vec![0; n];
        for j in 0..n {
            in_degree[j] = deps[j].len();
        }

        let mut queue: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();

        let mut result = Vec::with_capacity(n);
        while let Some(i) = queue.first().copied() {
            queue.remove(0);
            result.push(i);
            for j in 0..n {
                if deps[j].contains(&i) {
                    in_degree[j] -= 1;
                    if in_degree[j] == 0 {
                        queue.push(j);
                    }
                }
            }
        }

        // If there's a cycle, fall back to original order for remaining.
        if result.len() < n {
            let in_result: HashSet<usize> = result.iter().copied().collect();
            for i in 0..n {
                if !in_result.contains(&i) {
                    result.push(i);
                }
            }
        }

        result
    }

    /// Extract the set of variables defined and used by a statement.
    fn stmt_def_use(stmt: &ScgStatement) -> (HashSet<String>, HashSet<String>) {
        let mut defs = HashSet::new();
        let mut uses = HashSet::new();

        match stmt {
            ScgStatement::Computation(c) => {
                defs.insert(c.dst.clone());
                // Also add the user-visible variable name (from reassigns)
                // so the topological sort sees the dependency between a
                // Computation that defines a user variable and a later
                // Computation that references it by that user name.
                // Without this, the sort may reorder the Mul after the Add
                // that uses its result, causing the product to be lost.
                if let Some(ref name) = c.reassigns {
                    defs.insert(name.clone());
                }
                Self::expr_uses(&c.lhs, &mut uses);
                Self::expr_uses(&c.rhs, &mut uses);
            }
            ScgStatement::UnaryComputation(u) => {
                defs.insert(u.dst.clone());
                Self::expr_uses(&u.operand, &mut uses);
            }
            ScgStatement::Allocation(AllocationNode::Stack { name, .. }) => {
                defs.insert(name.clone());
            }
            ScgStatement::Allocation(AllocationNode::Heap {
                name, size_expr, ..
            }) => {
                defs.insert(name.clone());
                Self::expr_uses(size_expr, &mut uses);
            }
            ScgStatement::Access(AccessNode::Load { dst, ptr, offset, .. }) => {
                defs.insert(dst.clone());
                Self::expr_uses(ptr, &mut uses);
                if let Some(off) = offset {
                    Self::expr_uses(off, &mut uses);
                }
            }
            ScgStatement::Access(AccessNode::Store { ptr, offset, value, ty }) => {
                Self::expr_uses(ptr, &mut uses);
                if let Some(off) = offset {
                    Self::expr_uses(off, &mut uses);
                }
                Self::expr_uses(value, &mut uses);
            }
            ScgStatement::GetAddress(ga) => {
                defs.insert(ga.dst.clone());
            }
            ScgStatement::Cast(c) => {
                defs.insert(c.dst.clone());
                Self::expr_uses(&c.src, &mut uses);
            }
            ScgStatement::Call(c) => {
                if let Some(ref name) = c.dst {
                    defs.insert(name.clone());
                }
                for arg in &c.args {
                    Self::expr_uses(arg, &mut uses);
                }
            }
            ScgStatement::Return(vals) => {
                for v in vals {
                    Self::expr_uses(v, &mut uses);
                }
            }
            ScgStatement::Control(ControlNode::If {
                cond,
                then_body,
                else_body,
            }) => {
                Self::expr_uses(cond, &mut uses);
                for s in then_body {
                    let (d, u) = Self::stmt_def_use(s);
                    defs.extend(d);
                    uses.extend(u);
                }
                if let Some(else_body) = else_body {
                    for s in else_body {
                        let (d, u) = Self::stmt_def_use(s);
                        defs.extend(d);
                        uses.extend(u);
                    }
                }
            }
            ScgStatement::Control(ControlNode::Loop { body, .. }) => {
                for s in body {
                    let (d, u) = Self::stmt_def_use(s);
                    defs.extend(d);
                    uses.extend(u);
                }
            }
            ScgStatement::Control(ControlNode::Break)
            | ScgStatement::Control(ControlNode::Continue) => {}
            ScgStatement::Control(ControlNode::Switch {
                discriminant,
                arms,
                default_body,
            }) => {
                Self::expr_uses(discriminant, &mut uses);
                for arm in arms {
                    Self::expr_uses(&ScgExpr::Int(arm.value), &mut uses);
                    for s in &arm.body {
                        let (d, u) = Self::stmt_def_use(s);
                        defs.extend(d);
                        uses.extend(u);
                    }
                }
                for s in default_body {
                    let (d, u) = Self::stmt_def_use(s);
                    defs.extend(d);
                    uses.extend(u);
                }
            }
            ScgStatement::ConstantTime(ct) => {
                defs.insert(ct.dst.clone());
                for operand in &ct.operands {
                    Self::expr_uses(operand, &mut uses);
                }
            }
            ScgStatement::StructAccess(sa) => match sa {
                StructAccessNode::Load { dst, ptr, .. } => {
                    defs.insert(dst.clone());
                    Self::expr_uses(ptr, &mut uses);
                }
                StructAccessNode::Store { ptr, value, .. } => {
                    Self::expr_uses(ptr, &mut uses);
                    Self::expr_uses(value, &mut uses);
                }
            },
            ScgStatement::EnumAccess(ea) => match ea {
                EnumAccessNode::LoadTag { dst, ptr, .. } => {
                    defs.insert(dst.clone());
                    Self::expr_uses(ptr, &mut uses);
                }
                EnumAccessNode::StoreTag { ptr, value, .. } => {
                    Self::expr_uses(ptr, &mut uses);
                    Self::expr_uses(value, &mut uses);
                }
                EnumAccessNode::LoadPayload { dst, ptr, .. } => {
                    defs.insert(dst.clone());
                    Self::expr_uses(ptr, &mut uses);
                }
                EnumAccessNode::StorePayload { ptr, value, .. } => {
                    Self::expr_uses(ptr, &mut uses);
                    Self::expr_uses(value, &mut uses);
                }
            },
        }

        (defs, uses)
    }

    /// Collect variable uses from an expression.
    fn expr_uses(expr: &ScgExpr, uses: &mut HashSet<String>) {
        match expr {
            ScgExpr::Var(name) => {
                uses.insert(name.clone());
            }
            ScgExpr::BinOp { lhs, rhs, .. } => {
                Self::expr_uses(lhs, uses);
                Self::expr_uses(rhs, uses);
            }
            _ => {}
        }
    }
}

impl Default for IRBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CodegenError;

    /// Helper: build an Scg from a list of ScgNodes.
    fn scg_from_nodes(nodes: Vec<ScgNode>) -> Scg {
        Scg { nodes }
    }

    /// Helper: build a minimal function SCG.
    fn func_scg(name: &str, params: Vec<ScgParam>, body: Vec<ScgStatement>) -> Scg {
        scg_from_nodes(vec![ScgNode::Function(ScgFunction {
            name: name.to_string(),
            params,
            results: vec![],
            body,
        })])
    }

    // ── Test 1: Empty function with just a return ────────────────────

    #[test]
    fn test_empty_function() {
        let scg = func_scg("main", vec![], vec![ScgStatement::Return(vec![])]);
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        assert_eq!(program.functions.len(), 1);
        assert_eq!(program.functions[0].name, "main");
        // Entry block should have a Ret instruction
        let block = &program.functions[0].blocks[0];
        assert!(block
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstruction::Ret { .. })));
    }

    // ── Test 2: Addition computation ─────────────────────────────────

    #[test]
    fn test_addition() {
        let scg = func_scg(
            "add_one",
            vec![ScgParam {
                name: "x".into(),
                ty: ScgType::I64,
            }],
            vec![
                ScgStatement::Computation(ComputationNode {
                    dst: "result".into(),
                    op: BinOpKind::Add,
                    lhs: ScgExpr::Var("x".into()),
                    rhs: ScgExpr::Int(1),
                    tail_call: false,
                    reassigns: None,
                    result_ty: None,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("result".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let func = &program.functions[0];
        // Entry block should have Add instruction
        let block = &func.blocks[0];
        assert!(block
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstruction::Add { .. })));
    }

    // ── Test 3: If/else control flow ─────────────────────────────────

    #[test]
    fn test_if_else() {
        let scg = func_scg(
            "test_if",
            vec![],
            vec![ScgStatement::Control(ControlNode::If {
                cond: ScgExpr::Int(1),
                then_body: vec![ScgStatement::Return(vec![])],
                else_body: Some(vec![ScgStatement::Return(vec![])]),
            })],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        // Should have: entry, then, else, merge blocks
        assert!(program.functions[0].blocks.len() >= 4);
    }

    // ── Test 4: If without else ──────────────────────────────────────

    #[test]
    fn test_if_without_else() {
        let scg = func_scg(
            "test_if_no_else",
            vec![ScgParam {
                name: "flag".into(),
                ty: ScgType::I64,
            }], // define "flag" as a parameter
            vec![
                ScgStatement::Control(ControlNode::If {
                    cond: ScgExpr::Var("flag".into()),
                    then_body: vec![ScgStatement::Computation(ComputationNode {
                        dst: "x".into(),
                        op: BinOpKind::Add,
                        lhs: ScgExpr::Int(1),
                        rhs: ScgExpr::Int(2),
                        tail_call: false,
                    reassigns: None,
                    result_ty: None,
                    })],
                    else_body: None,
                }),
                ScgStatement::Return(vec![]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let func = &program.functions[0];
        // Should have: entry, then, merge blocks
        assert!(func.blocks.len() >= 3);
        // The entry block should branch to then or merge
        assert!(matches!(
            func.blocks[0].terminator,
            IRTerminator::Branch { .. }
        ));
    }

    // ── Test 5: Loop with phi node ──────────────────────────────────

    #[test]
    fn test_loop_with_phi() {
        let scg = func_scg(
            "test_loop",
            vec![],
            vec![ScgStatement::Control(ControlNode::Loop {
                body: vec![ScgStatement::Return(vec![])],
            })],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let func = &program.functions[0];
        // Should have: entry, loop_header, loop_body, loop_exit blocks
        assert!(func.blocks.len() >= 4);
        // Loop header should contain a phi instruction
        let header_block = &func.blocks[1];
        assert!(header_block
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstruction::Phi { .. })));
    }

    // ── Test 6: Break from loop ─────────────────────────────────────

    #[test]
    fn test_break_from_loop() {
        let scg = func_scg(
            "test_break",
            vec![],
            vec![ScgStatement::Control(ControlNode::Loop {
                body: vec![ScgStatement::Control(ControlNode::Break)],
            })],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let func = &program.functions[0];
        // Find the block with the break — it should jump to loop_exit
        let break_block = func
            .blocks
            .iter()
            .find(|b| matches!(b.terminator, IRTerminator::Jump(ref t) if t.contains("loop_exit")));
        assert!(break_block.is_some(), "break should jump to loop_exit");
    }

    // ── Test 7: Continue in loop ─────────────────────────────────────

    #[test]
    fn test_continue_in_loop() {
        let scg = func_scg(
            "test_continue",
            vec![],
            vec![ScgStatement::Control(ControlNode::Loop {
                body: vec![ScgStatement::Control(ControlNode::Continue)],
            })],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let func = &program.functions[0];
        // Find the block with the continue — it should jump to loop_header
        let cont_block = func.blocks.iter().find(
            |b| matches!(b.terminator, IRTerminator::Jump(ref t) if t.contains("loop_header")),
        );
        assert!(cont_block.is_some(), "continue should jump to loop_header");
    }

    // ── Test 8: Stack allocation ─────────────────────────────────────

    #[test]
    fn test_stack_allocation() {
        let scg = func_scg(
            "test_alloc",
            vec![],
            vec![
                ScgStatement::Allocation(AllocationNode::Stack {
                    name: "buf".into(),
                    size: 64,
                    ty: ScgType::U8,
                }),
                ScgStatement::Return(vec![]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];
        assert!(block
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstruction::Alloc { .. })));
    }

    // ── Test 9: Heap allocation (call to allocator) ──────────────────

    #[test]
    fn test_heap_allocation() {
        let scg = func_scg(
            "test_heap_alloc",
            vec![],
            vec![
                ScgStatement::Allocation(AllocationNode::Heap {
                    name: "dyn_buf".into(),
                    size_expr: ScgExpr::Int(128),
                    ty: ScgType::U8,
                }),
                ScgStatement::Return(vec![]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];
        let call_instr = block
            .instructions
            .iter()
            .find(|i| matches!(i, IRInstruction::Call { func, .. } if func == "__vuma_alloc"));
        assert!(
            call_instr.is_some(),
            "heap allocation should call __vuma_alloc"
        );
    }

    // ── Test 10: Load and store with offset ──────────────────────────

    #[test]
    fn test_load_store_with_offset() {
        let scg = func_scg(
            "test_mem",
            vec![ScgParam {
                name: "ptr".into(),
                ty: ScgType::Ptr,
            }],
            vec![
                ScgStatement::Access(AccessNode::Load {
                    dst: "val".into(),
                    ptr: ScgExpr::Var("ptr".into()),
                    offset: Some(ScgExpr::Int(8)),
                }),
                ScgStatement::Access(AccessNode::Store {
                    ptr: ScgExpr::Var("ptr".into()),
                    offset: Some(ScgExpr::Int(16)),
                    value: ScgExpr::Var("val".into()),
                    ty: None,
                }),
                ScgStatement::Return(vec![]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];

        // Constant integer offsets are folded directly into the Load / Store
        // instruction's `offset` field rather than emitted as separate
        // `Offset` instructions (see `lower_access`).  Non-constant offsets
        // would go through the `Offset` path — that is exercised separately.
        let offset_instr_count = block
            .instructions
            .iter()
            .filter(|i| matches!(i, IRInstruction::Offset { .. }))
            .count();
        assert_eq!(
            offset_instr_count, 0,
            "constant offsets should be folded, no Offset instruction expected"
        );

        // The Load should carry the constant offset 8.
        let load = block.instructions.iter().find_map(|i| match i {
            IRInstruction::Load { offset: 8, .. } => Some(i),
            _ => None,
        });
        assert!(load.is_some(), "should have a Load with offset 8");

        // The Store should carry the constant offset 16.
        let store = block.instructions.iter().find_map(|i| match i {
            IRInstruction::Store { offset: 16, .. } => Some(i),
            _ => None,
        });
        assert!(store.is_some(), "should have a Store with offset 16");

        // And, for completeness, both Load and Store should be present.
        assert!(block
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstruction::Load { .. })));
        assert!(block
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstruction::Store { .. })));
    }

    // ── Test 11: Cast node ───────────────────────────────────────────

    #[test]
    fn test_cast_node() {
        let scg = func_scg(
            "test_cast",
            vec![ScgParam {
                name: "x".into(),
                ty: ScgType::I32,
            }],
            vec![
                ScgStatement::Cast(CastNode {
                    dst: "extended".into(),
                    src: ScgExpr::Var("x".into()),
                    kind: CastKind::SExt,
                    from_ty: ScgType::I32,
                    to_ty: ScgType::I64,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("extended".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];
        assert!(block.instructions.iter().any(|i| matches!(
            i,
            IRInstruction::Cast {
                kind: CastKind::SExt,
                ..
            }
        )));
    }

    // ── Test 12: Function call with return value ─────────────────────

    #[test]
    fn test_function_call() {
        let scg = func_scg(
            "test_call",
            vec![],
            vec![
                ScgStatement::Call(CallNode {
                    dst: Some("result".into()),
                    func: "compute".into(),
                    args: vec![ScgExpr::Int(42), ScgExpr::Int(7)],
                    is_extern: false,
                reassigns: None,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("result".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];
        let call_instr = block
            .instructions
            .iter()
            .find(|i| matches!(i, IRInstruction::Call { func, .. } if func == "compute"));
        assert!(call_instr.is_some());
        if let Some(IRInstruction::Call { args, dst, .. }) = call_instr {
            assert_eq!(args.len(), 2);
            assert!(dst.is_some());
        }
    }

    // ── Test 13: Sub, Mul, Div use specific instructions ─────────────

    #[test]
    fn test_specific_arithmetic() {
        let scg = func_scg(
            "test_arith",
            vec![ScgParam {
                name: "a".into(),
                ty: ScgType::I64,
            }],
            vec![
                ScgStatement::Computation(ComputationNode {
                    dst: "s".into(),
                    op: BinOpKind::Sub,
                    lhs: ScgExpr::Var("a".into()),
                    rhs: ScgExpr::Int(1),
                    tail_call: false,
                    reassigns: None,
                    result_ty: None,
                }),
                ScgStatement::Computation(ComputationNode {
                    dst: "m".into(),
                    op: BinOpKind::Mul,
                    lhs: ScgExpr::Var("s".into()),
                    rhs: ScgExpr::Int(2),
                    tail_call: false,
                    reassigns: None,
                    result_ty: None,
                }),
                ScgStatement::Computation(ComputationNode {
                    dst: "d".into(),
                    op: BinOpKind::SDiv,
                    lhs: ScgExpr::Var("m".into()),
                    rhs: ScgExpr::Int(4),
                    tail_call: false,
                    reassigns: None,
                    result_ty: None,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("d".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];
        assert!(block
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstruction::Sub { .. })));
        assert!(block
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstruction::Mul { .. })));
        assert!(block
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstruction::Div { .. })));
    }

    // ── Test 14: Data section ────────────────────────────────────────

    #[test]
    fn test_data_section() {
        let scg = scg_from_nodes(vec![ScgNode::Data(ScgData {
            name: "rodata".into(),
            kind: DataSectionKind::ReadOnly,
            align: 16,
            data: vec![1, 2, 3, 4],
        })]);
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        assert_eq!(program.data_sections.len(), 1);
        assert_eq!(program.data_sections[0].name, "rodata");
        assert_eq!(program.data_sections[0].data.len(), 4);
    }

    // ── Test 15: Multiple functions ──────────────────────────────────

    #[test]
    fn test_multiple_functions() {
        let scg = scg_from_nodes(vec![
            ScgNode::Function(ScgFunction {
                name: "foo".into(),
                params: vec![],
                results: vec![],
                body: vec![ScgStatement::Return(vec![])],
            }),
            ScgNode::Function(ScgFunction {
                name: "bar".into(),
                params: vec![],
                results: vec![],
                body: vec![ScgStatement::Return(vec![])],
            }),
        ]);
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        assert_eq!(program.functions.len(), 2);
        assert_eq!(program.functions[0].name, "foo");
        assert_eq!(program.functions[1].name, "bar");
    }

    // ── Test 16: Virtual register naming ─────────────────────────────

    #[test]
    fn test_virtual_register_naming() {
        let scg = func_scg(
            "test_vreg",
            vec![ScgParam {
                name: "input".into(),
                ty: ScgType::I64,
            }],
            vec![
                ScgStatement::Computation(ComputationNode {
                    dst: "output".into(),
                    op: BinOpKind::Add,
                    lhs: ScgExpr::Var("input".into()),
                    rhs: ScgExpr::Int(10),
                    tail_call: false,
                    reassigns: None,
                    result_ty: None,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("output".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let func = &program.functions[0];

        // Check that the parameter vreg is named
        let param_vreg_id = func.params[0].as_register().unwrap();
        let param_vreg = func.get_vreg(param_vreg_id).unwrap();
        assert_eq!(param_vreg.name(), Some("input"));

        // Check that the computation result vreg is named
        let add_instr = func.blocks[0]
            .instructions
            .iter()
            .find(|i| matches!(i, IRInstruction::Add { .. }));
        if let Some(IRInstruction::Add { dst, .. }) = add_instr {
            let dst_id = dst.as_register().unwrap();
            let dst_vreg = func.get_vreg(dst_id).unwrap();
            assert_eq!(dst_vreg.name(), Some("output"));
        }
    }

    // ── Test 17: Break outside of loop returns error ─────────────────

    #[test]
    fn test_break_outside_loop_error() {
        let scg = func_scg(
            "bad_break",
            vec![],
            vec![ScgStatement::Control(ControlNode::Break)],
        );
        let mut builder = IRBuilder::new();
        let result = builder.build(&scg);
        assert!(result.is_err(), "break outside of loop should fail");
    }

    // ── Test 18: Continue outside of loop returns error ──────────────

    #[test]
    fn test_continue_outside_loop_error() {
        let scg = func_scg(
            "bad_continue",
            vec![],
            vec![ScgStatement::Control(ControlNode::Continue)],
        );
        let mut builder = IRBuilder::new();
        let result = builder.build(&scg);
        assert!(result.is_err(), "continue outside of loop should fail");
    }

    // ── Test 19: CFG predecessor/successor are computed ──────────────

    #[test]
    fn test_cfg_computed() {
        // Use an if/else where neither branch returns, so both fall through
        // to the merge block.  This ensures the merge block gets predecessors.
        let scg = func_scg(
            "test_cfg",
            vec![ScgParam {
                name: "c".into(),
                ty: ScgType::I64,
            }],
            vec![
                ScgStatement::Control(ControlNode::If {
                    cond: ScgExpr::Var("c".into()),
                    then_body: vec![ScgStatement::Computation(ComputationNode {
                        dst: "x".into(),
                        op: BinOpKind::Add,
                        lhs: ScgExpr::Int(1),
                        rhs: ScgExpr::Int(2),
                        tail_call: false,
                    reassigns: None,
                    result_ty: None,
                    })],
                    else_body: Some(vec![ScgStatement::Computation(ComputationNode {
                        dst: "y".into(),
                        op: BinOpKind::Sub,
                        lhs: ScgExpr::Int(5),
                        rhs: ScgExpr::Int(3),
                        tail_call: false,
                    reassigns: None,
                    result_ty: None,
                    })]),
                }),
                ScgStatement::Return(vec![]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let func = &program.functions[0];

        // After rebuild_cfg, the merge block should have predecessors
        // from both the then and else branches.
        let merge_block = func.blocks.iter().find(|b| b.label.contains("merge"));
        assert!(merge_block.is_some(), "should have a merge block");
        let merge = merge_block.unwrap();
        assert!(
            !merge.predecessors.is_empty(),
            "merge block should have at least one predecessor"
        );

        // The entry block should have a conditional branch terminator
        assert!(matches!(
            func.blocks[0].terminator,
            IRTerminator::Branch { .. }
        ));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // NEW ENHANCED TESTS (20+)
    // ═══════════════════════════════════════════════════════════════════════

    // ── Test 20: Unary computation (Neg) ─────────────────────────────

    #[test]
    fn test_unary_neg() {
        let scg = func_scg(
            "test_neg",
            vec![ScgParam {
                name: "x".into(),
                ty: ScgType::I64,
            }],
            vec![
                ScgStatement::UnaryComputation(UnaryComputationNode {
                    dst: "negated".into(),
                    op: UnaryOpKind::Neg,
                    operand: ScgExpr::Var("x".into()),
                    tail_call: false,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("negated".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];
        let unary_instr = block.instructions.iter().find(|i| {
            matches!(
                i,
                IRInstruction::UnaryOp {
                    op: UnaryOpKind::Neg,
                    ..
                }
            )
        });
        assert!(unary_instr.is_some(), "should have a Neg unary instruction");
    }

    // ── Test 21: Unary computation (Not) ─────────────────────────────

    #[test]
    fn test_unary_not() {
        let scg = func_scg(
            "test_not",
            vec![ScgParam {
                name: "x".into(),
                ty: ScgType::I64,
            }],
            vec![
                ScgStatement::UnaryComputation(UnaryComputationNode {
                    dst: "inverted".into(),
                    op: UnaryOpKind::Not,
                    operand: ScgExpr::Var("x".into()),
                    tail_call: false,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("inverted".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];
        let unary_instr = block.instructions.iter().find(|i| {
            matches!(
                i,
                IRInstruction::UnaryOp {
            ty: None,
                    op: UnaryOpKind::Not,
                    ..
                }
            )
        });
        assert!(unary_instr.is_some(), "should have a Not unary instruction");
    }

    // ── Test 22: Unary computation (Clz) ─────────────────────────────

    #[test]
    fn test_unary_clz() {
        let scg = func_scg(
            "test_clz",
            vec![ScgParam {
                name: "x".into(),
                ty: ScgType::U64,
            }],
            vec![
                ScgStatement::UnaryComputation(UnaryComputationNode {
                    dst: "leading_zeros".into(),
                    op: UnaryOpKind::Clz,
                    operand: ScgExpr::Var("x".into()),
                    tail_call: false,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("leading_zeros".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];
        assert!(block.instructions.iter().any(|i| {
            matches!(
                i,
                IRInstruction::UnaryOp {
            ty: None,
                    op: UnaryOpKind::Clz,
                    ..
                }
            )
        }));
    }

    // ── Test 23: Comparison operations lower to Cmp ──────────────────

    #[test]
    fn test_comparison_to_cmp() {
        let scg = func_scg(
            "test_cmp",
            vec![ScgParam {
                name: "a".into(),
                ty: ScgType::I64,
            }],
            vec![
                ScgStatement::Computation(ComputationNode {
                    dst: "less".into(),
                    op: BinOpKind::SLt,
                    lhs: ScgExpr::Var("a".into()),
                    rhs: ScgExpr::Int(10),
                    tail_call: false,
                    reassigns: None,
                    result_ty: None,
                }),
                ScgStatement::Computation(ComputationNode {
                    dst: "equal".into(),
                    op: BinOpKind::Eq,
                    lhs: ScgExpr::Var("a".into()),
                    rhs: ScgExpr::Int(0),
                    tail_call: false,
                    reassigns: None,
                    result_ty: None,
                }),
                ScgStatement::Return(vec![
                    ScgExpr::Var("less".into()),
                    ScgExpr::Var("equal".into()),
                ]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];

        // SLt should produce a Cmp instruction
        let slt_instr = block.instructions.iter().find(|i| {
            matches!(
                i,
                IRInstruction::Cmp {
                    kind: CmpKind::SLt,
                    ..
                }
            )
        });
        assert!(slt_instr.is_some(), "SLt should lower to Cmp instruction");

        // Eq should produce a Cmp instruction
        let eq_instr = block.instructions.iter().find(|i| {
            matches!(
                i,
                IRInstruction::Cmp {
                    kind: CmpKind::Eq,
                    ..
                }
            )
        });
        assert!(eq_instr.is_some(), "Eq should lower to Cmp instruction");
    }

    // ── Test 24: Unsigned comparison operations ──────────────────────

    #[test]
    fn test_unsigned_comparisons() {
        let scg = func_scg(
            "test_ucmp",
            vec![ScgParam {
                name: "a".into(),
                ty: ScgType::U64,
            }],
            vec![
                ScgStatement::Computation(ComputationNode {
                    dst: "ult".into(),
                    op: BinOpKind::ULt,
                    lhs: ScgExpr::Var("a".into()),
                    rhs: ScgExpr::Int(10),
                    tail_call: false,
                    reassigns: None,
                    result_ty: None,
                }),
                ScgStatement::Computation(ComputationNode {
                    dst: "uge".into(),
                    op: BinOpKind::UGe,
                    lhs: ScgExpr::Var("a".into()),
                    rhs: ScgExpr::Int(5),
                    tail_call: false,
                    reassigns: None,
                    result_ty: None,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("ult".into()), ScgExpr::Var("uge".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];

        let ult_instr = block.instructions.iter().find(|i| {
            matches!(
                i,
                IRInstruction::Cmp {
                    kind: CmpKind::ULt,
                    ..
                }
            )
        });
        assert!(ult_instr.is_some(), "ULt should lower to Cmp instruction");

        let uge_instr = block.instructions.iter().find(|i| {
            matches!(
                i,
                IRInstruction::Cmp {
                    kind: CmpKind::UGe,
                    ..
                }
            )
        });
        assert!(uge_instr.is_some(), "UGe should lower to Cmp instruction");
    }

    // ── Test 25: ScgType to IRType conversion ────────────────────────

    #[test]
    fn test_scg_type_to_ir_type() {
        assert_eq!(ScgType::I8.to_ir_type(), IRType::I8);
        assert_eq!(ScgType::I16.to_ir_type(), IRType::I16);
        assert_eq!(ScgType::I32.to_ir_type(), IRType::I32);
        assert_eq!(ScgType::I64.to_ir_type(), IRType::I64);
        assert_eq!(ScgType::U8.to_ir_type(), IRType::U8);
        assert_eq!(ScgType::U16.to_ir_type(), IRType::U16);
        assert_eq!(ScgType::U32.to_ir_type(), IRType::U32);
        assert_eq!(ScgType::U64.to_ir_type(), IRType::U64);
        assert_eq!(ScgType::Ptr.to_ir_type(), IRType::Ptr);
        assert_eq!(ScgType::Void.to_ir_type(), IRType::Void);
    }

    // ── Test 26: Param types are mapped to IR types ──────────────────

    #[test]
    fn test_param_types_mapped() {
        let scg = func_scg(
            "typed_fn",
            vec![
                ScgParam {
                    name: "a".into(),
                    ty: ScgType::I32,
                },
                ScgParam {
                    name: "b".into(),
                    ty: ScgType::Ptr,
                },
            ],
            vec![ScgStatement::Return(vec![])],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let func = &program.functions[0];
        assert_eq!(func.param_types.len(), 2);
        assert_eq!(func.param_types[0], IRType::I32);
        assert_eq!(func.param_types[1], IRType::Ptr);
    }

    // ── Test 27: If/else with phi nodes at merge ─────────────────────

    #[test]
    fn test_if_else_phi_nodes() {
        let scg = func_scg(
            "test_phi",
            vec![ScgParam {
                name: "flag".into(),
                ty: ScgType::I64,
            }],
            vec![
                ScgStatement::Control(ControlNode::If {
                    cond: ScgExpr::Var("flag".into()),
                    then_body: vec![ScgStatement::Computation(ComputationNode {
                        dst: "x".into(),
                        op: BinOpKind::Add,
                        lhs: ScgExpr::Int(1),
                        rhs: ScgExpr::Int(2),
                        tail_call: false,
                    reassigns: None,
                    result_ty: None,
                    })],
                    else_body: Some(vec![ScgStatement::Computation(ComputationNode {
                        dst: "x".into(),
                        op: BinOpKind::Sub,
                        lhs: ScgExpr::Int(10),
                        rhs: ScgExpr::Int(3),
                        tail_call: false,
                    reassigns: None,
                    result_ty: None,
                    })]),
                }),
                ScgStatement::Return(vec![ScgExpr::Var("x".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let func = &program.functions[0];

        // The merge block should contain a phi node for x
        let merge_block = func.blocks.iter().find(|b| b.label.contains("merge"));
        assert!(merge_block.is_some(), "should have a merge block");
        let merge = merge_block.unwrap();
        let phi_instr = merge
            .instructions
            .iter()
            .find(|i| matches!(i, IRInstruction::Phi { .. }));
        assert!(
            phi_instr.is_some(),
            "merge block should have a phi node for variable 'x' defined in both branches"
        );
    }

    // ── Test 28: Topological sort of statements ──────────────────────

    #[test]
    fn test_topological_sort_basic() {
        let stmts = vec![
            ScgStatement::Computation(ComputationNode {
                dst: "a".into(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Int(1),
                rhs: ScgExpr::Int(2),
                tail_call: false,
                    reassigns: None,
                    result_ty: None,
            }),
            ScgStatement::Computation(ComputationNode {
                dst: "b".into(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var("a".into()),
                rhs: ScgExpr::Int(3),
                tail_call: false,
                    reassigns: None,
                    result_ty: None,
            }),
            ScgStatement::Return(vec![ScgExpr::Var("b".into())]),
        ];
        let order = IRBuilder::topological_sort_statements(&stmts);
        assert_eq!(order.len(), 3);
        // a must come before b
        let pos_a = order.iter().position(|&i| i == 0).unwrap();
        let pos_b = order.iter().position(|&i| i == 1).unwrap();
        assert!(pos_a < pos_b, "a must come before b in topological order");
    }

    // ── Test 29: Topological sort with independent statements ────────

    #[test]
    fn test_topological_sort_independent() {
        let stmts = vec![
            ScgStatement::Computation(ComputationNode {
                dst: "a".into(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Int(1),
                rhs: ScgExpr::Int(2),
                tail_call: false,
                    reassigns: None,
                    result_ty: None,
            }),
            ScgStatement::Computation(ComputationNode {
                dst: "b".into(),
                op: BinOpKind::Mul,
                lhs: ScgExpr::Int(3),
                rhs: ScgExpr::Int(4),
                tail_call: false,
                    reassigns: None,
                    result_ty: None,
            }),
            ScgStatement::Computation(ComputationNode {
                dst: "c".into(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var("a".into()),
                rhs: ScgExpr::Var("b".into()),
                tail_call: false,
                    reassigns: None,
                    result_ty: None,
            }),
        ];
        let order = IRBuilder::topological_sort_statements(&stmts);
        assert_eq!(order.len(), 3);
        let pos_a = order.iter().position(|&i| i == 0).unwrap();
        let pos_b = order.iter().position(|&i| i == 1).unwrap();
        let pos_c = order.iter().position(|&i| i == 2).unwrap();
        // a and b must come before c
        assert!(pos_a < pos_c);
        assert!(pos_b < pos_c);
    }

    // ── Test 30: Topological sort with empty input ───────────────────

    #[test]
    fn test_topological_sort_empty() {
        let stmts: Vec<ScgStatement> = vec![];
        let order = IRBuilder::topological_sort_statements(&stmts);
        assert!(order.is_empty());
    }

    // ── Test 31: Load without offset ─────────────────────────────────

    #[test]
    fn test_load_without_offset() {
        let scg = func_scg(
            "test_load_plain",
            vec![ScgParam {
                name: "ptr".into(),
                ty: ScgType::Ptr,
            }],
            vec![
                ScgStatement::Access(AccessNode::Load {
                    dst: "val".into(),
                    ptr: ScgExpr::Var("ptr".into()),
                    offset: None,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("val".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];
        // Should have Load but no Offset
        assert!(block
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstruction::Load { .. })));
        assert!(!block
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstruction::Offset { .. })));
    }

    // ── Test 32: Store without offset ────────────────────────────────

    #[test]
    fn test_store_without_offset() {
        let scg = func_scg(
            "test_store_plain",
            vec![ScgParam {
                name: "ptr".into(),
                ty: ScgType::Ptr,
            }],
            vec![
                ScgStatement::Access(AccessNode::Store {
                    ptr: ScgExpr::Var("ptr".into()),
                    offset: None,
                    value: ScgExpr::Int(42),
                    ty: None,
                }),
                ScgStatement::Return(vec![]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];
        // Should have Store but no Offset
        assert!(block
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstruction::Store { .. })));
        assert!(!block
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstruction::Offset { .. })));
    }

    // ── Test 33: Function call without return value ──────────────────

    #[test]
    fn test_void_function_call() {
        let scg = func_scg(
            "test_void_call",
            vec![],
            vec![
                ScgStatement::Call(CallNode {
                    dst: None,
                    func: "print_int".into(),
                    args: vec![ScgExpr::Int(123)],
                    is_extern: false,
                reassigns: None,
                }),
                ScgStatement::Return(vec![]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];
        let call_instr = block
            .instructions
            .iter()
            .find(|i| matches!(i, IRInstruction::Call { func, .. } if func == "print_int"));
        assert!(call_instr.is_some());
        if let Some(IRInstruction::Call { dst, args, .. }) = call_instr {
            assert!(dst.is_none(), "void call should have no dst");
            assert_eq!(args.len(), 1);
        }
    }

    // ── Test 34: Multiple casts ──────────────────────────────────────

    #[test]
    fn test_multiple_casts() {
        let scg = func_scg(
            "test_multi_cast",
            vec![ScgParam {
                name: "x".into(),
                ty: ScgType::I8,
            }],
            vec![
                ScgStatement::Cast(CastNode {
                    dst: "wider".into(),
                    src: ScgExpr::Var("x".into()),
                    kind: CastKind::ZExt,
                    from_ty: ScgType::I8,
                    to_ty: ScgType::I64,
                }),
                ScgStatement::Cast(CastNode {
                    dst: "narrow".into(),
                    src: ScgExpr::Var("wider".into()),
                    kind: CastKind::Trunc,
                    from_ty: ScgType::I64,
                    to_ty: ScgType::I32,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("narrow".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];
        let zext_count = block
            .instructions
            .iter()
            .filter(|i| {
                matches!(
                    i,
                    IRInstruction::Cast {
                        kind: CastKind::ZExt,
                        ..
                    }
                )
            })
            .count();
        let trunc_count = block
            .instructions
            .iter()
            .filter(|i| {
                matches!(
                    i,
                    IRInstruction::Cast {
                        kind: CastKind::Trunc,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(zext_count, 1, "should have exactly one ZExt cast");
        assert_eq!(trunc_count, 1, "should have exactly one Trunc cast");
    }

    // ── Test 35: Bitwise operations use generic BinOp ───────────────

    #[test]
    fn test_bitwise_binop() {
        let scg = func_scg(
            "test_bitwise",
            vec![ScgParam {
                name: "x".into(),
                ty: ScgType::I64,
            }],
            vec![
                ScgStatement::Computation(ComputationNode {
                    dst: "and_result".into(),
                    op: BinOpKind::And,
                    lhs: ScgExpr::Var("x".into()),
                    rhs: ScgExpr::Int(0xFF),
                    tail_call: false,
                    reassigns: None,
                    result_ty: None,
                }),
                ScgStatement::Computation(ComputationNode {
                    dst: "or_result".into(),
                    op: BinOpKind::Or,
                    lhs: ScgExpr::Var("x".into()),
                    rhs: ScgExpr::Int(0x100),
                    tail_call: false,
                    reassigns: None,
                    result_ty: None,
                }),
                ScgStatement::Return(vec![
                    ScgExpr::Var("and_result".into()),
                    ScgExpr::Var("or_result".into()),
                ]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];

        let and_instr = block.instructions.iter().find(|i| {
            matches!(
                i,
                IRInstruction::BinOp {
                    op: BinOpKind::And,
                    ..
                }
            )
        });
        assert!(and_instr.is_some(), "And should use BinOp instruction");

        let or_instr = block.instructions.iter().find(|i| {
            matches!(
                i,
                IRInstruction::BinOp {
                    op: BinOpKind::Or,
                    ..
                }
            )
        });
        assert!(or_instr.is_some(), "Or should use BinOp instruction");
    }

    // ── Test 36: Result types mapped to IR types ─────────────────────

    #[test]
    fn test_result_types_mapped() {
        let scg = scg_from_nodes(vec![ScgNode::Function(ScgFunction {
            name: "typed_ret".into(),
            params: vec![],
            results: vec![ScgType::I64, ScgType::Ptr],
            body: vec![ScgStatement::Return(vec![
                ScgExpr::Int(42),
                ScgExpr::Int(0),
            ])],
        })]);
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let func = &program.functions[0];
        assert_eq!(func.result_types.len(), 2);
        assert_eq!(func.result_types[0], IRType::I64);
        assert_eq!(func.result_types[1], IRType::Ptr);
    }

    // ── Test 37: Nested if/else produces correct block structure ─────

    #[test]
    fn test_nested_if() {
        let scg = func_scg(
            "test_nested_if",
            vec![ScgParam {
                name: "x".into(),
                ty: ScgType::I64,
            }],
            vec![ScgStatement::Control(ControlNode::If {
                cond: ScgExpr::Var("x".into()),
                then_body: vec![ScgStatement::Control(ControlNode::If {
                    cond: ScgExpr::Int(1),
                    then_body: vec![ScgStatement::Return(vec![])],
                    else_body: None,
                })],
                else_body: Some(vec![ScgStatement::Return(vec![])]),
            })],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let func = &program.functions[0];
        // Should have multiple blocks for the nested structure
        assert!(
            func.blocks.len() >= 5,
            "nested if/else should produce at least 5 blocks, got {}",
            func.blocks.len()
        );
    }

    // ── Test 38: Combined allocation + access pattern ────────────────

    #[test]
    fn test_alloc_access_pattern() {
        let scg = func_scg(
            "test_alloc_access",
            vec![],
            vec![
                ScgStatement::Allocation(AllocationNode::Stack {
                    name: "buf".into(),
                    size: 32,
                    ty: ScgType::U8,
                }),
                ScgStatement::Access(AccessNode::Store {
                    ptr: ScgExpr::Var("buf".into()),
                    offset: None,
                    value: ScgExpr::Int(99),
                    ty: None,
                }),
                ScgStatement::Access(AccessNode::Load {
                    dst: "loaded".into(),
                    ptr: ScgExpr::Var("buf".into()),
                    offset: None,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("loaded".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];

        // Should have: Alloc, Store, Load in that order
        let alloc_pos = block
            .instructions
            .iter()
            .position(|i| matches!(i, IRInstruction::Alloc { .. }));
        let store_pos = block
            .instructions
            .iter()
            .position(|i| matches!(i, IRInstruction::Store { .. }));
        let load_pos = block
            .instructions
            .iter()
            .position(|i| matches!(i, IRInstruction::Load { .. }));
        assert!(alloc_pos.is_some(), "should have Alloc");
        assert!(store_pos.is_some(), "should have Store");
        assert!(load_pos.is_some(), "should have Load");
        assert!(
            alloc_pos.unwrap() < store_pos.unwrap(),
            "Alloc should come before Store"
        );
        assert!(
            store_pos.unwrap() < load_pos.unwrap(),
            "Store should come before Load"
        );
    }

    // ── Test 39: Ne and SGe comparisons ──────────────────────────────

    #[test]
    fn test_ne_sge_comparisons() {
        let scg = func_scg(
            "test_ne_sge",
            vec![ScgParam {
                name: "x".into(),
                ty: ScgType::I64,
            }],
            vec![
                ScgStatement::Computation(ComputationNode {
                    dst: "ne".into(),
                    op: BinOpKind::Ne,
                    lhs: ScgExpr::Var("x".into()),
                    rhs: ScgExpr::Int(0),
                    tail_call: false,
                    reassigns: None,
                    result_ty: None,
                }),
                ScgStatement::Computation(ComputationNode {
                    dst: "sge".into(),
                    op: BinOpKind::SGe,
                    lhs: ScgExpr::Var("x".into()),
                    rhs: ScgExpr::Int(0),
                    tail_call: false,
                    reassigns: None,
                    result_ty: None,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("ne".into()), ScgExpr::Var("sge".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];

        let ne_instr = block.instructions.iter().find(|i| {
            matches!(
                i,
                IRInstruction::Cmp {
                    kind: CmpKind::Ne,
                    ..
                }
            )
        });
        assert!(ne_instr.is_some(), "Ne should lower to Cmp");

        let sge_instr = block.instructions.iter().find(|i| {
            matches!(
                i,
                IRInstruction::Cmp {
                    kind: CmpKind::SGe,
                    ..
                }
            )
        });
        assert!(sge_instr.is_some(), "SGe should lower to Cmp");
    }

    // ── Test 40: Unary computation (Popcnt) ──────────────────────────

    #[test]
    fn test_unary_popcnt() {
        let scg = func_scg(
            "test_popcnt",
            vec![ScgParam {
                name: "x".into(),
                ty: ScgType::U64,
            }],
            vec![
                ScgStatement::UnaryComputation(UnaryComputationNode {
                    dst: "bits".into(),
                    op: UnaryOpKind::Popcnt,
                    operand: ScgExpr::Var("x".into()),
                    tail_call: false,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("bits".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let block = &program.functions[0].blocks[0];
        assert!(block.instructions.iter().any(|i| {
            matches!(
                i,
                IRInstruction::UnaryOp {
            ty: None,
                    op: UnaryOpKind::Popcnt,
                    ..
                }
            )
        }));
    }

    // ── Test 41: Loop with break and computation ─────────────────────

    #[test]
    fn test_loop_with_computation_and_break() {
        let scg = func_scg(
            "test_loop_comp",
            vec![ScgParam {
                name: "n".into(),
                ty: ScgType::I64,
            }],
            vec![
                ScgStatement::Control(ControlNode::Loop {
                    body: vec![
                        ScgStatement::Computation(ComputationNode {
                            dst: "sum".into(),
                            op: BinOpKind::Add,
                            lhs: ScgExpr::Var("n".into()),
                            rhs: ScgExpr::Int(1),
                            tail_call: false,
                    reassigns: None,
                    result_ty: None,
                        }),
                        ScgStatement::Control(ControlNode::Break),
                    ],
                }),
                ScgStatement::Return(vec![ScgExpr::Var("sum".into())]),
            ],
        );
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).unwrap();
        let func = &program.functions[0];

        // Should have loop_header, loop_body, loop_exit blocks
        let header = func.blocks.iter().find(|b| b.label.contains("loop_header"));
        assert!(header.is_some(), "should have loop_header block");

        // Loop header should have phi
        if let Some(h) = header {
            assert!(h
                .instructions
                .iter()
                .any(|i| matches!(i, IRInstruction::Phi { .. })));
        }

        // Should have an Add instruction in the loop body
        let add_count = func
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter(|i| matches!(i, IRInstruction::Add { .. }))
            .count();
        assert!(
            add_count >= 1,
            "should have at least one Add in the loop body"
        );
    }

    /// Verify that referencing an unknown variable in an SCG produces
    /// [`CodegenError::UnknownVariable`] instead of silently substituting 0.
    #[test]
    fn test_unknown_variable_returns_error() {
        // Build a minimal SCG with a function whose body references an
        // undefined variable in a Return statement.
        let scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "test_unknown".to_string(),
                params: vec![],
                results: vec![ScgType::I32],
                body: vec![ScgStatement::Return(vec![ScgExpr::Var(
                    "undefined_var".to_string(),
                )])],
            })],
        };

        let mut builder = IRBuilder::new();
        let result = builder.convert(&scg);

        match result {
            Err(CodegenError::UnknownVariable { name }) => {
                assert_eq!(
                    name, "undefined_var",
                    "error should reference the unknown variable name"
                );
            }
            other => {
                panic!(
                    "expected Err(CodegenError::UnknownVariable {{ .. }}), got {:?}",
                    other
                );
            }
        }
    }

    /// Verify that a Computation node referencing an unknown variable
    /// also returns [`CodegenError::UnknownVariable`].
    #[test]
    fn test_unknown_variable_in_computation_returns_error() {
        let scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "test_unknown_comp".to_string(),
                params: vec![ScgParam {
                    name: "x".to_string(),
                    ty: ScgType::I32,
                }],
                results: vec![ScgType::I32],
                body: vec![ScgStatement::Computation(ComputationNode {
                    dst: "result".to_string(),
                    op: BinOpKind::Add,
                    lhs: ScgExpr::Var("x".to_string()),
                    rhs: ScgExpr::Var("y".to_string()), // undefined
                    tail_call: false,
                    reassigns: None,
                    result_ty: None,
                })],
            })],
        };

        let mut builder = IRBuilder::new();
        let result = builder.convert(&scg);

        match result {
            Err(CodegenError::UnknownVariable { name }) => {
                assert_eq!(name, "y", "error should reference the unknown variable 'y'");
            }
            other => {
                panic!(
                    "expected Err(CodegenError::UnknownVariable {{ .. }}), got {:?}",
                    other
                );
            }
        }
    }
}
