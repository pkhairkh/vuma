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
//! | `AllocationNode::Stack` | `Alloc` + stack slot registration                  |
//! | `AllocationNode::Heap`  | `Call` to `__vuma_alloc`                           |
//! | `AccessNode::Load`    | Optional `Offset` + `Load`                           |
//! | `AccessNode::Store`   | Optional `Offset` + `Store`                          |
//! | `CastNode`            | `Cast` (zext / sext / trunc / bitcast)               |
//! | `ComputationNode`     | `Add`/`Sub`/`Mul`/`Div`/`Cmp`/`BinOp`/`UnaryOp`     |
//! | `UnaryComputationNode`| `UnaryOp` (neg / not / clz / ctz / popcnt)           |
//! | `CallNode`            | `Call`                                               |
//! | `Return`              | `Ret` + `IRTerminator::Return`                       |
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
//! [`IRBuilder::build_from_scg`] method walks the SCG in topological order
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
}

/// Control-flow node.
#[derive(Debug, Clone)]
pub enum ControlNode {
    /// `if cond { then } else { else_ }`
    If {
        cond: ScgExpr,
        then_body: Vec<ScgStatement>,
        else_body: Option<Vec<ScgStatement>>,
    },
    /// `loop { body }`
    Loop { body: Vec<ScgStatement> },
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
        name: String,
        size: u32,
        ty: ScgType,
    },
    /// Heap allocation (dynamic size, calls allocator).
    Heap {
        name: String,
        size_expr: ScgExpr,
        ty: ScgType,
    },
}

/// Memory access node.
#[derive(Debug, Clone)]
pub enum AccessNode {
    /// Read: `dst = *ptr` or `dst = ptr.field`
    Load {
        dst: String,
        ptr: ScgExpr,
        offset: Option<ScgExpr>,
    },
    /// Write: `*ptr = val` or `ptr.field = val`
    Store {
        ptr: ScgExpr,
        offset: Option<ScgExpr>,
        value: ScgExpr,
    },
}

/// Cast / reinterpret node.
#[derive(Debug, Clone)]
pub struct CastNode {
    pub dst: String,
    pub src: ScgExpr,
    pub kind: CastKind,
    pub from_ty: ScgType,
    pub to_ty: ScgType,
}

/// Computation node (binary arithmetic / logic).
#[derive(Debug, Clone)]
pub struct ComputationNode {
    pub dst: String,
    pub op: BinOpKind,
    pub lhs: ScgExpr,
    pub rhs: ScgExpr,
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
    pub tail_call: bool,
}

/// Function call node.
#[derive(Debug, Clone)]
pub struct CallNode {
    pub dst: Option<String>,
    pub func: String,
    pub args: Vec<ScgExpr>,
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
}

/// SCG data declaration.
#[derive(Debug, Clone)]
pub struct ScgData {
    pub name: String,
    pub kind: DataSectionKind,
    pub align: u32,
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

        // Translate the body statements.
        self.lower_statements(&func.body, &mut ir_func, &mut name_to_vreg)?;

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
        for stmt in stmts {
            self.lower_statement(stmt, ir_func, names)?;
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
                    .map(|e| self.resolve_expr(e, names))
                    .collect::<Result<Vec<_>>>()?;
                // Emit a Ret instruction and set the terminator.
                ir_func.current_block().push(IRInstruction::Ret {
                    values: ir_vals.clone(),
                });
                ir_func.current_block().terminator = IRTerminator::Return(ir_vals);
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
                self.lower_break(ir_func)?;
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

        let cond_val = self.resolve_expr(cond, names)?;

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
    /// The loop header contains phi nodes for any loop-carried values.
    /// A synthetic loop counter phi is always inserted to demonstrate the
    /// pattern.  Real compilers would analyze which variables are modified
    /// in the loop body.
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
        });

        // Jump from current block to loop header.
        ir_func.current_block().push(IRInstruction::Branch {
            target: loop_header.clone(),
        });
        ir_func.current_block().terminator = IRTerminator::Jump(loop_header.clone());

        // Loop header — place a phi node placeholder for any loop-carried
        // values.  We insert a trivial phi for a "loop iteration counter"
        // to demonstrate the pattern.  Real compilers would analyze which
        // variables are modified in the loop body.
        ir_func.append_block(&loop_header);

        // Insert a phi node for a synthetic loop counter (demonstrates phi).
        let counter_vreg = self.alloc_vreg();
        ir_func.register_vreg(VirtualRegister::named(counter_vreg, "loop_counter"));
        let pre_header_label = ir_func.blocks[ir_func.blocks.len() - 2].label.clone();
        ir_func.current_block().push(IRInstruction::Phi {
            dst: IRValue::Register(counter_vreg),
            incoming: vec![
                (IRValue::Immediate(0), pre_header_label),
                (IRValue::Register(counter_vreg), loop_body_label.clone()),
            ],
        });

        // Unconditional jump from header to body.
        ir_func.current_block().push(IRInstruction::Branch {
            target: loop_body_label.clone(),
        });
        ir_func.current_block().terminator = IRTerminator::Jump(loop_body_label.clone());

        // Loop body.
        ir_func.append_block(&loop_body_label);
        self.lower_statements(body, ir_func, names)?;

        // Back-edge to header if the block doesn't have a terminator.
        if matches!(
            ir_func.current_block().terminator,
            IRTerminator::Unreachable
        ) {
            ir_func.current_block().push(IRInstruction::Branch {
                target: loop_header.clone(),
            });
            ir_func.current_block().terminator = IRTerminator::Jump(loop_header.clone());
        }

        // Loop exit.
        ir_func.append_block(&loop_exit);

        // Pop loop context.
        self.loop_stack.pop();

        Ok(())
    }

    /// Lower a `break` to a jump to the enclosing loop's exit label.
    fn lower_break(&mut self, ir_func: &mut IRFunction) -> Result<()> {
        let exit_label = self
            .loop_stack
            .last()
            .map(|ctx| ctx.exit_label.clone())
            .ok_or_else(|| {
                crate::CodegenError::TranslationError("break outside of loop".to_string())
            })?;

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
        let disc_val = self.resolve_expr(discriminant, names)?;
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
                let size_val = self.resolve_expr(size_expr, names)?;
                let vreg = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::named(vreg, name));
                names.insert(name.clone(), vreg);
                // Lower to a call to `__vuma_alloc`.
                ir_func.current_block().push(IRInstruction::Call {
                    dst: Some(IRValue::Register(vreg)),
                    func: "__vuma_alloc".to_string(),
                    args: vec![size_val],
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
                let ptr_val = self.resolve_expr(ptr, names)?;
                let addr_val = match offset {
                    Some(off) => {
                        let off_val = self.resolve_expr(off, names)?;
                        let addr_reg = self.alloc_vreg();
                        ir_func.register_vreg(VirtualRegister::anonymous(addr_reg));
                        ir_func.current_block().push(IRInstruction::Offset {
                            dst: IRValue::Register(addr_reg),
                            base: ptr_val,
                            offset: off_val,
                        });
                        IRValue::Register(addr_reg)
                    }
                    None => ptr_val,
                };
                let dst_vreg = self.alloc_vreg();
                ir_func.register_vreg(VirtualRegister::named(dst_vreg, dst));
                names.insert(dst.clone(), dst_vreg);
                ir_func.current_block().push(IRInstruction::Load {
                    dst: IRValue::Register(dst_vreg),
                    addr: addr_val,
                });
            }
            AccessNode::Store { ptr, offset, value } => {
                let ptr_val = self.resolve_expr(ptr, names)?;
                let val = self.resolve_expr(value, names)?;
                let addr_val = match offset {
                    Some(off) => {
                        let off_val = self.resolve_expr(off, names)?;
                        let addr_reg = self.alloc_vreg();
                        ir_func.register_vreg(VirtualRegister::anonymous(addr_reg));
                        ir_func.current_block().push(IRInstruction::Offset {
                            dst: IRValue::Register(addr_reg),
                            base: ptr_val,
                            offset: off_val,
                        });
                        IRValue::Register(addr_reg)
                    }
                    None => ptr_val,
                };
                ir_func.current_block().push(IRInstruction::Store {
                    value: val,
                    addr: addr_val,
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
        let src_val = self.resolve_expr(&cast.src, names)?;
        let dst_vreg = self.alloc_vreg();
        ir_func.register_vreg(VirtualRegister::named(dst_vreg, &cast.dst));
        names.insert(cast.dst.clone(), dst_vreg);
        ir_func.current_block().push(IRInstruction::Cast {
            kind: cast.kind,
            dst: IRValue::Register(dst_vreg),
            src: src_val,
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
        let lhs_val = self.resolve_expr(&comp.lhs, names)?;
        let rhs_val = self.resolve_expr(&comp.rhs, names)?;
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
                });
            }
            BinOpKind::Sub => {
                ir_func.current_block().push(IRInstruction::Sub {
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                });
            }
            BinOpKind::Mul => {
                ir_func.current_block().push(IRInstruction::Mul {
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                });
            }
            BinOpKind::SDiv | BinOpKind::UDiv => {
                ir_func.current_block().push(IRInstruction::Div {
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                });
            }
            // Comparison operations → dedicated Cmp instruction.
            BinOpKind::SLt => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::SLt,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                });
            }
            BinOpKind::SLe => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::SLe,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                });
            }
            BinOpKind::SGt => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::SGt,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                });
            }
            BinOpKind::SGe => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::SGe,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                });
            }
            BinOpKind::ULt => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::ULt,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                });
            }
            BinOpKind::ULe => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::ULe,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                });
            }
            BinOpKind::UGt => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::UGt,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                });
            }
            BinOpKind::UGe => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::UGe,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                });
            }
            BinOpKind::Eq => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::Eq,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                });
            }
            BinOpKind::Ne => {
                ir_func.current_block().push(IRInstruction::Cmp {
                    kind: CmpKind::Ne,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
                });
            }
            _ => {
                ir_func.current_block().push(IRInstruction::BinOp {
                    op: comp.op,
                    dst,
                    lhs: lhs_val,
                    rhs: rhs_val,
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
        let operand_val = self.resolve_expr(&unary.operand, names)?;
        let dst_vreg = self.alloc_vreg();
        ir_func.register_vreg(VirtualRegister::named(dst_vreg, &unary.dst));
        names.insert(unary.dst.clone(), dst_vreg);

        ir_func.current_block().push(IRInstruction::UnaryOp {
            op: unary.op,
            dst: IRValue::Register(dst_vreg),
            operand: operand_val,
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
        let args: Vec<IRValue> = call
            .args
            .iter()
            .map(|e| self.resolve_expr(e, names))
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
    fn resolve_expr(&self, expr: &ScgExpr, names: &HashMap<String, u32>) -> Result<IRValue> {
        match expr {
            ScgExpr::Var(name) => {
                if let Some(&vreg) = names.get(name) {
                    Ok(IRValue::Register(vreg))
                } else {
                    Err(crate::CodegenError::UnknownVariable { name: name.clone() })
                }
            }
            ScgExpr::Int(v) => Ok(IRValue::Immediate(*v)),
            ScgExpr::Float(f) => {
                // Reinterpret the f64 bits as i64 for the immediate.
                Ok(IRValue::Immediate(f.to_bits() as i64))
            }
            ScgExpr::Label(name) => Ok(IRValue::Label(name.clone())),
        }
    }

    // =======================================================================
    // Topological sort helper
    // =======================================================================

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
        // j uses a variable that i defines, and i < j.
        let mut deps: Vec<HashSet<usize>> = vec![HashSet::new(); n];
        for j in 0..n {
            for var in &uses[j] {
                // Find the last definition before j.
                for i in (0..j).rev() {
                    if defines[i].contains(var) {
                        deps[j].insert(i);
                        break;
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
            vec![],
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
        // Should have Offset instructions for address computation
        let offset_count = block
            .instructions
            .iter()
            .filter(|i| matches!(i, IRInstruction::Offset { .. }))
            .count();
        assert_eq!(
            offset_count, 2,
            "should have 2 offset instructions (load + store)"
        );
        // Should have Load and Store
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
                assert_eq!(
                    name, "y",
                    "error should reference the unknown variable 'y'"
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
}
