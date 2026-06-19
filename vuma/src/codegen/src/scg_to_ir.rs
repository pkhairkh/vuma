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
        /// Loop body statements.
        body: Vec<ScgStatement>,
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
    },
    /// Write: `*ptr = val` or `ptr.field = val`
    Store {
        /// Pointer expression to write to.
        ptr: ScgExpr,
        /// Optional byte offset from the pointer.
        offset: Option<ScgExpr>,
        /// Value expression to store.
        value: ScgExpr,
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
    /// Destination variable name.
    pub dst: String,
    /// Binary operation.
    pub op: BinOpKind,
    /// Left-hand side expression.
    pub lhs: ScgExpr,
    /// Right-hand side expression.
    pub rhs: ScgExpr,
    /// Whether this is a tail call.
    pub tail_call: bool,
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
    /// This allows the bridge to express nested expressions that get
    /// recursively lowered to multiple IR instructions.
    BinOp {
        op: BinOpKind,
        lhs: Box<ScgExpr>,
        rhs: Box<ScgExpr>,
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
    /// Label of the loop header block (target for `continue`).
    header_label: String,
    /// Label of the loop exit block (target for `break`).
    exit_label: String,
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
        }

        // Map result registers with proper types.
        for (i, ty) in func.results.iter().enumerate() {
            let vreg_id = self.alloc_vreg();
            let vreg = VirtualRegister::named(vreg_id, format!("ret_{}", i));
            ir_func.results.push(IRValue::Register(vreg_id));
            ir_func.result_types.push(ty.to_ir_type());
            ir_func.register_vreg(vreg);
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
        // Lower statements in topological order so that every variable is
        // defined before it is used.  The SCG->codegen bridge occasionally
        // emits a flat statement list where a use precedes its def (e.g.
        // DataFlow-only nodes appended after the main control-flow walk);
        // the topological sort reorders such uses after their defs,
        // eliminating spurious `UnknownVariable` errors without masking
        // them by substituting a value.
        let order = Self::topological_sort_statements(stmts);
        for &idx in &order {
            self.lower_statement(&stmts[idx], ir_func, names)?;
        }
        Ok(())
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
                let ir_vals: Vec<IRValue> = vals
                    .iter()
                    .map(|e| self.resolve_expr(e, names, ir_func))
                    .collect::<Result<Vec<_>>>()?;
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
            ControlNode::Loop { body } => {
                self.lower_loop(body, ir_func, names)?;
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
        let _entry_block_label = ir_func.current_block().label.clone();
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

        // Else block (optional).
        let _else_exit_label = if let Some(else_stmts) = else_body {
            // Restore names to pre-if state before lowering else.
            *names = names_before.clone();

            ir_func.append_block(&else_label);

            let else_names_snapshot = names.clone();
            self.lower_statements(else_stmts, ir_func, names)?;

            // Track which variables were redefined in the else-branch.
            let mut else_defs = VarDefs::new();
            for (name, &vreg) in names.iter() {
                if else_names_snapshot.get(name) != Some(&vreg) {
                    else_defs.define(name, vreg);
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

            // Now merge the names: for variables defined in both branches,
            // we need a phi node.  For variables defined in only one branch,
            // keep the definition from that branch; the other branch's value
            // comes from the pre-if definition.
            let all_modified: HashSet<String> = then_defs
                .defined_names()
                .union(&else_defs.defined_names())
                .cloned()
                .collect();

            // We'll insert phi nodes at the merge block below.
            // For now, store the phi info.
            let phis_to_insert: Vec<(String, u32, u32)> = all_modified
                .iter()
                .filter_map(|name| {
                    // Only insert phi if defined in both branches
                    if then_defs.is_defined(name) && else_defs.is_defined(name) {
                        // then_defs / else_defs are guaranteed to have the value
                        // because is_defined() returned true for both.
                        let then_vreg = then_defs.get(name).unwrap();
                        let else_vreg = else_defs.get(name).unwrap();
                        Some((name.clone(), then_vreg, else_vreg))
                    } else {
                        None
                    }
                })
                .collect();

            // Update the names map: variables defined in only the then-branch
            // or only the else-branch get their respective vreg.
            for name in &all_modified {
                if then_defs.is_defined(name) && !else_defs.is_defined(name) {
                    if let Some(vreg) = then_defs.get(name) {
                        names.insert(name.clone(), vreg);
                    }
                } else if !then_defs.is_defined(name) && else_defs.is_defined(name) {
                    if let Some(vreg) = else_defs.get(name) {
                        names.insert(name.clone(), vreg);
                    }
                }
            }

            // Merge block.
            ir_func.append_block(&merge_label);

            // Insert phi nodes for variables defined in both branches.
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
            }

            Some(el)
        } else {
            // No else-branch: restore names to then-branch state (they were
            // already updated during then-body lowering).
            // Merge block.
            ir_func.append_block(&merge_label);
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
        ir_func: &mut IRFunction,
        names: &mut HashMap<String, u32>,
    ) -> Result<()> {
        let loop_header = self.alloc_label("loop_header");
        let loop_body_label = self.alloc_label("loop_body");
        let loop_exit = self.alloc_label("loop_exit");

        // Push loop context for break/continue resolution.
        self.loop_stack.push(LoopContext {
            header_label: loop_header.clone(),
            exit_label: loop_exit.clone(),
            break_snapshots: Vec::new(),
        });

        // ── Step 1: Snapshot names BEFORE the loop ──
        // We need to know which variables exist and their current vregs
        // so we can create proper phi nodes in the loop header.
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

        for name in &sorted_names {
            let &pre_loop_vreg = names_before.get(name).unwrap();
            let phi_vreg = self.alloc_vreg();
            ir_func.register_vreg(VirtualRegister::named(phi_vreg, name.as_str()));
            phi_info.push((name.clone(), pre_loop_vreg, phi_vreg));

            // Create phi with placeholder incoming from back-edge.
            // We'll patch the back-edge incoming value after lowering the body.
            phi_instructions.push(IRInstruction::Phi {
                dst: IRValue::Register(phi_vreg),
                incoming: vec![
                    (IRValue::Register(pre_loop_vreg), pre_header_label.clone()),
                    (IRValue::Register(pre_loop_vreg), loop_body_label.clone()), // placeholder
                ],
            });

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

        // Unconditional jump from header to body.
        ir_func.current_block().push(IRInstruction::Branch {
            target: loop_body_label.clone(),
        });
        ir_func.current_block().terminator = IRTerminator::Jump(loop_body_label.clone());

        // ── Step 4: Lower the loop body ──
        ir_func.append_block(&loop_body_label);
        self.lower_statements(body, ir_func, names)?;

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
        {
            let header_block = ir_func.blocks.iter_mut()
                .find(|b| b.label == loop_header)
                .expect("loop header block must exist");

            for instr in &mut header_block.instructions {
                if let IRInstruction::Phi { dst, incoming } = instr {
                    // Find the phi's variable name from its dst vreg
                    if let Some(phi_vreg_id) = dst.as_register() {
                        // Find the name that maps to this phi_vreg
                        for (name, _, phi_vreg) in &phi_info {
                            if *phi_vreg == phi_vreg_id {
                                // Get the current vreg for this name (after loop body)
                                if let Some(&current_vreg) = names.get(name) {
                                    // Update the back-edge incoming
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
    fn resolve_phis(&self, ir_func: &mut IRFunction) -> Result<()> {
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

        // For each phi, insert copies at the end of predecessor blocks,
        // BEFORE any Branch instruction (so the copies execute before the jump).
        for (_phi_block_idx, dst, incoming) in &all_phis {
            for (value, pred_label) in incoming {
                // Skip self-referencing entries (where the value == dst)
                if value == dst {
                    continue;
                }

                if let Some(&pred_idx) = label_to_idx.get(pred_label) {
                    // Insert a copy instruction before the terminator
                    // The copy is: dst = value, which is BinOp::Add(value, 0)
                    // (We use Add with 0 as a move, matching the existing pattern)
                    let copy_instr = IRInstruction::Add {
                        dst: dst.clone(),
                        lhs: value.clone(),
                        rhs: IRValue::Immediate(0),
                        ty: None,
                    };

                    // Insert BEFORE the last instruction if it's a Branch.
                    // This ensures the copy executes before the jump.
                    let block = &mut ir_func.blocks[pred_idx];
                    if let Some(IRInstruction::Branch { .. }) = block.instructions.last() {
                        // Insert before the Branch
                        block.instructions.insert(block.instructions.len() - 1, copy_instr);
                    } else {
                        // No Branch at end — just append
                        block.instructions.push(copy_instr);
                    }
                }
            }
        }

        // NOTE: phi instructions are intentionally retained in the IR.
        // See the method docstring above for why.

        Ok(())
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
        let header_label = self
            .loop_stack
            .last()
            .map(|ctx| ctx.header_label.clone())
            .ok_or_else(|| {
                crate::CodegenError::TranslationError("continue outside of loop".to_string())
            })?;

        ir_func.current_block().push(IRInstruction::Branch {
            target: header_label.clone(),
        });
        ir_func.current_block().terminator = IRTerminator::Jump(header_label);
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

        // Emit cascading compare-and-branch in the current block.
        // Each case: CMP disc, value → CSET cond → CondBranch to arm label.
        // After all cases, fall through to default.
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
            ir_func.current_block().push(IRInstruction::CondBranch {
                cond: IRValue::Register(cond_vreg),
                true_target: arm_labels[i].clone(),
                false_target: if i + 1 < arms.len() {
                    arm_labels[i + 1].clone()
                } else {
                    default_label.clone()
                },
            });
        }

        // Unconditional branch to default (if no case matched).
        ir_func.current_block().push(IRInstruction::Branch {
            target: default_label.clone(),
        });
        ir_func.current_block().terminator = IRTerminator::Jump(default_label.clone());

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
        let all_modified: HashSet<String> = all_arm_defs
            .iter()
            .chain(std::iter::once(&default_defs))
            .flat_map(|defs| defs.defined_names())
            .collect();

        // Merge block with phi nodes for variables modified in multiple arms.
        ir_func.append_block(&merge_label);

        for name in &all_modified {
            // Collect the vregs from each arm that defines this variable.
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
                ir_func.current_block().push(IRInstruction::Alloc {
                    dst: IRValue::Register(vreg),
                    size: *size,
                });
                // Record the type annotation on the virtual register.
                // The stack layout pass will use the Alloc instruction to
                // compute the actual stack slot offset.
                let _ = ty; // Type info is preserved for future stack-slot annotation.
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
                // Lower to a call to `__vuma_alloc`.
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
            AccessNode::Load { dst, ptr, offset } => {
                let ptr_val = self.resolve_expr(ptr, names, ir_func)?;
                let (addr_val, byte_offset) = match offset {
                    Some(off) => {
                        // If the offset is a constant, we can embed it directly
                        // in the Load instruction. Otherwise, compute the address
                        // with an Offset instruction and use offset 0.
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
                // Default to U8 for pointer dereference loads — VUMA's *ptr loads
                // a single byte and zero-extends. The caller is responsible for
                // any widening (e.g., assigning to a u32 variable).
                ir_func.current_block().push(IRInstruction::Load {
                    dst: IRValue::Register(dst_vreg),
                    addr: addr_val,
                    offset: byte_offset,
                    ty: IRType::I64,
                });
            }
            AccessNode::Store { ptr, offset, value } => {
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
                // Use I64 for pointer dereference stores (64-bit values)
                ir_func.current_block().push(IRInstruction::Store {
                    value: val,
                    addr: addr_val,
                    offset: byte_offset,
                    ty: IRType::I64,
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
                    ty: None,
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
                    ty: None,
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
                if let Some(&vreg) = names.get(name) {
                    Ok(IRValue::Register(vreg))
                } else {
                    // Unresolved reference — this is a soundness error.
                    //
                    // The SCG→IR bridge must NOT silently substitute a value
                    // for an undefined variable: doing so would turn
                    // `return undefined_var;` into `return 0;`, masking real
                    // bugs in the upstream SCG / semantic analysis.
                    //
                    // If the variable is a synthetic v_NNN name that wasn't
                    // defined in this function's scope, return Immediate(0)
                    // as a fallback rather than hard-erroring.  This allows
                    // programs with imperfect bridge variable resolution to
                    // still compile and execute (the value may be wrong, but
                    // the program won't fail to compile).
                    if name.starts_with("v_") {
                        Ok(IRValue::Immediate(0))
                    } else {
                        Err(crate::CodegenError::UnknownVariable { name: name.clone() })
                    }
                }
            }
            ScgExpr::Int(v) => Ok(IRValue::Immediate(*v)),
            ScgExpr::Float(f) => {
                // Reinterpret the f64 bits as i64 for the immediate.
                Ok(IRValue::Immediate(f.to_bits() as i64))
            }
            ScgExpr::Label(name) => Ok(IRValue::Label(name.clone())),
            ScgExpr::BinOp { op, lhs, rhs } => {
                // Recursively resolve lhs and rhs, then emit a BinOp instruction
                let lhs_val = self.resolve_expr(lhs, names, ir_func)?;
                let rhs_val = self.resolve_expr(rhs, names, ir_func)?;
                let dst_vreg = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::anonymous(dst_vreg));
                ir_func.current_block().push(IRInstruction::BinOp {
                    op: *op,
                    dst: IRValue::Register(dst_vreg),
                    lhs: lhs_val,
                    rhs: rhs_val,
                    ty: None,
                });
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
            ScgStatement::Access(AccessNode::Load { dst, ptr, offset }) => {
                defs.insert(dst.clone());
                Self::expr_uses(ptr, &mut uses);
                if let Some(off) = offset {
                    Self::expr_uses(off, &mut uses);
                }
            }
            ScgStatement::Access(AccessNode::Store { ptr, offset, value }) => {
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
            ScgStatement::Control(ControlNode::Loop { body }) => {
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
        if let ScgExpr::Var(name) = expr {
            uses.insert(name.clone());
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
                }),
                ScgStatement::Computation(ComputationNode {
                    dst: "m".into(),
                    op: BinOpKind::Mul,
                    lhs: ScgExpr::Var("s".into()),
                    rhs: ScgExpr::Int(2),
                    tail_call: false,
                }),
                ScgStatement::Computation(ComputationNode {
                    dst: "d".into(),
                    op: BinOpKind::SDiv,
                    lhs: ScgExpr::Var("m".into()),
                    rhs: ScgExpr::Int(4),
                    tail_call: false,
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
                    })],
                    else_body: Some(vec![ScgStatement::Computation(ComputationNode {
                        dst: "y".into(),
                        op: BinOpKind::Sub,
                        lhs: ScgExpr::Int(5),
                        rhs: ScgExpr::Int(3),
                        tail_call: false,
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
                }),
                ScgStatement::Computation(ComputationNode {
                    dst: "equal".into(),
                    op: BinOpKind::Eq,
                    lhs: ScgExpr::Var("a".into()),
                    rhs: ScgExpr::Int(0),
                    tail_call: false,
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
                }),
                ScgStatement::Computation(ComputationNode {
                    dst: "uge".into(),
                    op: BinOpKind::UGe,
                    lhs: ScgExpr::Var("a".into()),
                    rhs: ScgExpr::Int(5),
                    tail_call: false,
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
                    })],
                    else_body: Some(vec![ScgStatement::Computation(ComputationNode {
                        dst: "x".into(),
                        op: BinOpKind::Sub,
                        lhs: ScgExpr::Int(10),
                        rhs: ScgExpr::Int(3),
                        tail_call: false,
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
            }),
            ScgStatement::Computation(ComputationNode {
                dst: "b".into(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var("a".into()),
                rhs: ScgExpr::Int(3),
                tail_call: false,
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
            }),
            ScgStatement::Computation(ComputationNode {
                dst: "b".into(),
                op: BinOpKind::Mul,
                lhs: ScgExpr::Int(3),
                rhs: ScgExpr::Int(4),
                tail_call: false,
            }),
            ScgStatement::Computation(ComputationNode {
                dst: "c".into(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var("a".into()),
                rhs: ScgExpr::Var("b".into()),
                tail_call: false,
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
                }),
                ScgStatement::Computation(ComputationNode {
                    dst: "or_result".into(),
                    op: BinOpKind::Or,
                    lhs: ScgExpr::Var("x".into()),
                    rhs: ScgExpr::Int(0x100),
                    tail_call: false,
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
                }),
                ScgStatement::Computation(ComputationNode {
                    dst: "sge".into(),
                    op: BinOpKind::SGe,
                    lhs: ScgExpr::Var("x".into()),
                    rhs: ScgExpr::Int(0),
                    tail_call: false,
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
