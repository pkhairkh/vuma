//! # SCG → IR Conversion
//!
//! Translates a Semantic Computation Graph (SCG) — produced by the `vuma-scg`
//! crate — into the intermediate representation defined in [`crate::ir`].
//!
//! ## SCG Node Types
//!
//! The converter maps each SCG node category to IR instructions:
//!
//! | SCG Node            | IR Mapping                                           |
//! |---------------------|------------------------------------------------------|
//! | `ControlNode`       | Conditional branches / jumps                         |
//! | `AllocationNode`    | Stack allocation (`Alloc`) or heap call (`Call`)     |
//! | `AccessNode`        | `Load` / `Store`                                    |
//! | `CastNode`          | `Cast` (reinterpret / extend / truncate)             |
//! | `ComputationNode`   | `BinOp` / `UnaryOp`                                 |
//! | `CallNode`          | `Call`                                              |
//!
//! ## Architecture
//!
//! [`ScgToIr`] is the main entry point.  It holds translation state (virtual-
//! register counter, label counter, etc.) and implements a visitor pattern
//! over SCG nodes.

use crate::ir::*;
use crate::Result;

// ---------------------------------------------------------------------------
// SCG Node stubs
// ---------------------------------------------------------------------------
// NOTE: The real SCG types live in the `vuma-scg` crate.  We define
// lightweight stubs here so this crate compiles independently.  When the
// full SCG crate is available, these can be replaced with re-exports or
// converted to trait-based dispatch.

/// Placeholder for the SCG graph type from `vuma-scg`.
///
/// In production, this will be `vuma_scg::Scg` or similar.
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
    pub name: String,
    pub ty: ScgType,
}

/// A lightweight type representation in the SCG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScgType {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    Ptr,
    Void,
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
    /// Arithmetic / logic.
    Computation(ComputationNode),
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
    Loop {
        body: Vec<ScgStatement>,
    },
    /// `break` (from inside a loop).
    Break,
    /// `continue` (from inside a loop).
    Continue,
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

/// Computation node (arithmetic / logic).
#[derive(Debug, Clone)]
pub struct ComputationNode {
    pub dst: String,
    pub op: BinOpKind,
    pub lhs: ScgExpr,
    pub rhs: ScgExpr,
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
// ScgToIr — the converter
// ---------------------------------------------------------------------------

/// Translates an SCG into an [`IRProgram`].
///
/// # Example (conceptual)
///
/// ```ignore
/// use vuma_codegen::scg_to_ir::{ScgToIr, Scg};
///
/// let scg: Scg = /* … */;
/// let converter = ScgToIr::new();
/// let ir_program = converter.convert(&scg)?;
/// ```
pub struct ScgToIr {
    /// Monotonically increasing virtual-register ID counter.
    next_vreg: u32,
    /// Monotonically increasing label counter (for generating unique names).
    next_label: u32,
}

impl ScgToIr {
    /// Create a new converter with fresh counters.
    pub fn new() -> Self {
        Self {
            next_vreg: 0,
            next_label: 0,
        }
    }

    /// Convert a full SCG into an IR program.
    pub fn convert(&mut self, scg: &Scg) -> Result<IRProgram> {
        let mut program = IRProgram::new();

        for node in &scg.nodes {
            match node {
                ScgNode::Function(func) => {
                    let ir_func = self.convert_function(func)?;
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

    // ---- Function translation ----

    /// Convert a single SCG function to an IR function.
    fn convert_function(&mut self, func: &ScgFunction) -> Result<IRFunction> {
        let mut ir_func = IRFunction::new(&func.name);

        // Map parameters to virtual registers.
        let mut name_to_vreg = std::collections::HashMap::new();
        for param in &func.params {
            let vreg = self.alloc_vreg();
            ir_func.params.push(IRValue::Register(vreg));
            name_to_vreg.insert(param.name.clone(), vreg);
        }

        // Map result registers.
        for _ in &func.results {
            let vreg = self.alloc_vreg();
            ir_func.results.push(IRValue::Register(vreg));
        }

        // Translate the body statements.
        self.convert_statements(&func.body, &mut ir_func, &mut name_to_vreg)?;

        Ok(ir_func)
    }

    /// Convert a list of SCG statements, appending IR instructions to the
    /// current block of `ir_func`.
    fn convert_statements(
        &mut self,
        stmts: &[ScgStatement],
        ir_func: &mut IRFunction,
        names: &mut std::collections::HashMap<String, u32>,
    ) -> Result<()> {
        for stmt in stmts {
            self.convert_statement(stmt, ir_func, names)?;
        }
        Ok(())
    }

    /// Convert a single SCG statement.
    fn convert_statement(
        &mut self,
        stmt: &ScgStatement,
        ir_func: &mut IRFunction,
        names: &mut std::collections::HashMap<String, u32>,
    ) -> Result<()> {
        match stmt {
            ScgStatement::Control(ctrl) => {
                self.convert_control(ctrl, ir_func, names)?;
            }
            ScgStatement::Allocation(alloc) => {
                self.convert_allocation(alloc, ir_func, names)?;
            }
            ScgStatement::Access(access) => {
                self.convert_access(access, ir_func, names)?;
            }
            ScgStatement::Cast(cast) => {
                self.convert_cast(cast, ir_func, names)?;
            }
            ScgStatement::Computation(comp) => {
                self.convert_computation(comp, ir_func, names)?;
            }
            ScgStatement::Call(call) => {
                self.convert_call(call, ir_func, names)?;
            }
            ScgStatement::Return(vals) => {
                let ir_vals: Vec<IRValue> = vals
                    .iter()
                    .map(|e| self.resolve_expr(e, names))
                    .collect();
                ir_func.current_block().terminator = IRTerminator::Return(ir_vals);
            }
        }
        Ok(())
    }

    // ---- Control flow ----

    /// Convert a control-flow node to IR branches / jumps.
    fn convert_control(
        &mut self,
        ctrl: &ControlNode,
        ir_func: &mut IRFunction,
        names: &mut std::collections::HashMap<String, u32>,
    ) -> Result<()> {
        match ctrl {
            ControlNode::If {
                cond,
                then_body,
                else_body,
            } => {
                let then_label = self.alloc_label("then");
                let else_label = self.alloc_label("else");
                let merge_label = self.alloc_label("merge");

                let cond_val = self.resolve_expr(cond, names);

                // Branch on the condition.
                ir_func.current_block().terminator = IRTerminator::Branch {
                    cond: cond_val,
                    true_block: then_label.clone(),
                    false_block: if else_body.is_some() {
                        else_label.clone()
                    } else {
                        merge_label.clone()
                    },
                };

                // Then block.
                ir_func.append_block(&then_label);
                self.convert_statements(then_body, ir_func, names)?;
                // Add a jump to merge if the block doesn't already have a
                // proper terminator.
                if matches!(ir_func.current_block().terminator, IRTerminator::Unreachable) {
                    ir_func.current_block().terminator = IRTerminator::Jump(merge_label.clone());
                }

                // Else block (optional).
                if let Some(else_stmts) = else_body {
                    ir_func.append_block(&else_label);
                    self.convert_statements(else_stmts, ir_func, names)?;
                    if matches!(ir_func.current_block().terminator, IRTerminator::Unreachable) {
                        ir_func.current_block().terminator = IRTerminator::Jump(merge_label.clone());
                    }
                }

                // Merge block.
                ir_func.append_block(&merge_label);
            }
            ControlNode::Loop { body } => {
                let loop_header = self.alloc_label("loop_header");
                let loop_body = self.alloc_label("loop_body");
                let loop_exit = self.alloc_label("loop_exit");

                // Jump from current block to loop header.
                ir_func.current_block().terminator = IRTerminator::Jump(loop_header.clone());

                // Loop header (future: phi nodes for loop-carried values).
                ir_func.append_block(&loop_header);
                ir_func.current_block().terminator = IRTerminator::Jump(loop_body.clone());

                // Loop body.
                ir_func.append_block(&loop_body);
                self.convert_statements(body, ir_func, names)?;
                // Back-edge to header if the block doesn't have a terminator.
                if matches!(ir_func.current_block().terminator, IRTerminator::Unreachable) {
                    ir_func.current_block().terminator = IRTerminator::Jump(loop_header.clone());
                }

                // Loop exit.
                ir_func.append_block(&loop_exit);
                // TODO: wire Break → loop_exit, Continue → loop_header
            }
            ControlNode::Break => {
                // TODO: implement break by looking up the enclosing loop's
                // exit label in a loop-stack.
                let exit_label = self.alloc_label("break_target");
                ir_func.current_block().terminator = IRTerminator::Jump(exit_label);
            }
            ControlNode::Continue => {
                // TODO: implement continue by looking up the enclosing loop's
                // header label in a loop-stack.
                let header_label = self.alloc_label("continue_target");
                ir_func.current_block().terminator = IRTerminator::Jump(header_label);
            }
        }
        Ok(())
    }

    // ---- Allocation ----

    /// Convert an allocation node.
    ///
    /// - `AllocationNode::Stack` → `IRInstr::Alloc`
    /// - `AllocationNode::Heap` → `IRInstr::Call` to the runtime allocator
    fn convert_allocation(
        &mut self,
        alloc: &AllocationNode,
        ir_func: &mut IRFunction,
        names: &mut std::collections::HashMap<String, u32>,
    ) -> Result<()> {
        match alloc {
            AllocationNode::Stack { name, size, ty: _ } => {
                let vreg = self.alloc_vreg();
                names.insert(name.clone(), vreg);
                ir_func.current_block().push(IRInstr::Alloc {
                    dst: IRValue::Register(vreg),
                    size: *size,
                });
            }
            AllocationNode::Heap {
                name,
                size_expr,
                ty: _,
            } => {
                let size_val = self.resolve_expr(size_expr, names);
                let vreg = self.alloc_vreg();
                names.insert(name.clone(), vreg);
                // Lower to a call to `__vuma_alloc`.
                ir_func.current_block().push(IRInstr::Call {
                    dst: Some(IRValue::Register(vreg)),
                    func: "__vuma_alloc".to_string(),
                    args: vec![size_val],
                });
            }
        }
        Ok(())
    }

    // ---- Memory access ----

    /// Convert a memory access node to `Load` / `Store` IR instructions.
    fn convert_access(
        &mut self,
        access: &AccessNode,
        ir_func: &mut IRFunction,
        names: &mut std::collections::HashMap<String, u32>,
    ) -> Result<()> {
        match access {
            AccessNode::Load { dst, ptr, offset } => {
                let ptr_val = self.resolve_expr(ptr, names);
                let addr_val = match offset {
                    Some(off) => {
                        let off_val = self.resolve_expr(off, names);
                        let addr_reg = self.alloc_vreg();
                        ir_func.current_block().push(IRInstr::Offset {
                            dst: IRValue::Register(addr_reg),
                            base: ptr_val,
                            offset: off_val,
                        });
                        IRValue::Register(addr_reg)
                    }
                    None => ptr_val,
                };
                let dst_vreg = self.alloc_vreg();
                names.insert(dst.clone(), dst_vreg);
                ir_func.current_block().push(IRInstr::Load {
                    dst: IRValue::Register(dst_vreg),
                    addr: addr_val,
                });
            }
            AccessNode::Store {
                ptr,
                offset,
                value,
            } => {
                let ptr_val = self.resolve_expr(ptr, names);
                let val = self.resolve_expr(value, names);
                let addr_val = match offset {
                    Some(off) => {
                        let off_val = self.resolve_expr(off, names);
                        let addr_reg = self.alloc_vreg();
                        ir_func.current_block().push(IRInstr::Offset {
                            dst: IRValue::Register(addr_reg),
                            base: ptr_val,
                            offset: off_val,
                        });
                        IRValue::Register(addr_reg)
                    }
                    None => ptr_val,
                };
                ir_func.current_block().push(IRInstr::Store {
                    value: val,
                    addr: addr_val,
                });
            }
        }
        Ok(())
    }

    // ---- Cast ----

    /// Convert a cast node.
    fn convert_cast(
        &mut self,
        cast: &CastNode,
        ir_func: &mut IRFunction,
        names: &mut std::collections::HashMap<String, u32>,
    ) -> Result<()> {
        let src_val = self.resolve_expr(&cast.src, names);
        let dst_vreg = self.alloc_vreg();
        names.insert(cast.dst.clone(), dst_vreg);
        ir_func.current_block().push(IRInstr::Cast {
            kind: cast.kind,
            dst: IRValue::Register(dst_vreg),
            src: src_val,
        });
        Ok(())
    }

    // ---- Computation ----

    /// Convert a computation node to a `BinOp` IR instruction.
    fn convert_computation(
        &mut self,
        comp: &ComputationNode,
        ir_func: &mut IRFunction,
        names: &mut std::collections::HashMap<String, u32>,
    ) -> Result<()> {
        let lhs_val = self.resolve_expr(&comp.lhs, names);
        let rhs_val = self.resolve_expr(&comp.rhs, names);
        let dst_vreg = self.alloc_vreg();
        names.insert(comp.dst.clone(), dst_vreg);
        ir_func.current_block().push(IRInstr::BinOp {
            op: comp.op,
            dst: IRValue::Register(dst_vreg),
            lhs: lhs_val,
            rhs: rhs_val,
        });
        Ok(())
    }

    // ---- Call ----

    /// Convert a call node.
    fn convert_call(
        &mut self,
        call: &CallNode,
        ir_func: &mut IRFunction,
        names: &mut std::collections::HashMap<String, u32>,
    ) -> Result<()> {
        let args: Vec<IRValue> = call
            .args
            .iter()
            .map(|e| self.resolve_expr(e, names))
            .collect();

        let dst = match &call.dst {
            Some(name) => {
                let vreg = self.alloc_vreg();
                names.insert(name.clone(), vreg);
                Some(IRValue::Register(vreg))
            }
            None => None,
        };

        ir_func.current_block().push(IRInstr::Call {
            dst,
            func: call.func.clone(),
            args,
        });
        Ok(())
    }

    // ---- Helpers ----

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
    /// immediates.
    fn resolve_expr(
        &self,
        expr: &ScgExpr,
        names: &std::collections::HashMap<String, u32>,
    ) -> IRValue {
        match expr {
            ScgExpr::Var(name) => {
                if let Some(&vreg) = names.get(name) {
                    IRValue::Register(vreg)
                } else {
                    // Unknown variable — use 0 as a placeholder.
                    // TODO: proper error reporting
                    log::warn!("unknown variable '{}' in SCG, substituting 0", name);
                    IRValue::Immediate(0)
                }
            }
            ScgExpr::Int(v) => IRValue::Immediate(*v),
            ScgExpr::Label(name) => IRValue::Label(name.clone()),
        }
    }
}

impl Default for ScgToIr {
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

    #[test]
    fn convert_empty_function() {
        let scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "main".into(),
                params: vec![],
                results: vec![],
                body: vec![ScgStatement::Return(vec![])],
            })],
        };
        let mut converter = ScgToIr::new();
        let program = converter.convert(&scg).unwrap();
        assert_eq!(program.functions.len(), 1);
        assert_eq!(program.functions[0].name, "main");
    }

    #[test]
    fn convert_addition() {
        let scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "add_one".into(),
                params: vec![ScgParam {
                    name: "x".into(),
                    ty: ScgType::I64,
                }],
                results: vec![ScgType::I64],
                body: vec![
                    ScgStatement::Computation(ComputationNode {
                        dst: "result".into(),
                        op: BinOpKind::Add,
                        lhs: ScgExpr::Var("x".into()),
                        rhs: ScgExpr::Int(1),
                    }),
                    ScgStatement::Return(vec![ScgExpr::Var("result".into())]),
                ],
            })],
        };
        let mut converter = ScgToIr::new();
        let program = converter.convert(&scg).unwrap();
        let func = &program.functions[0];
        // Entry block should have: Alloc (no), BinOp, Return
        let block = &func.blocks[0];
        assert!(matches!(block.instructions[0], IRInstr::BinOp { op: BinOpKind::Add, .. }));
    }

    #[test]
    fn convert_if_else() {
        let scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "test_if".into(),
                params: vec![],
                results: vec![],
                body: vec![ScgStatement::Control(ControlNode::If {
                    cond: ScgExpr::Int(1),
                    then_body: vec![ScgStatement::Return(vec![])],
                    else_body: Some(vec![ScgStatement::Return(vec![])]),
                })],
            })],
        };
        let mut converter = ScgToIr::new();
        let program = converter.convert(&scg).unwrap();
        // Should have: entry, then, else, merge blocks
        assert!(program.functions[0].blocks.len() >= 4);
    }
}
