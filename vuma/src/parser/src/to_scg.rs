//! AST → SCG (Structured Computation Graph) conversion.
//!
//! This module bridges the parser's output ([`Program`]) to the VUMA
//! intermediate representation: the **Structured Computation Graph** (SCG).
//!
//! The SCG is a directed graph where:
//! - **Nodes** represent computational operations (allocation, access,
//!   deallocation, computation, control flow).
//! - **Edges** represent data-flow and control-flow dependencies.
//!
//! Because the `vuma-scg` crate may not yet be fully populated, this
//! module defines *local* SCG types that mirror the intended SCG schema.
//! Once `vuma-scg` stabilises, these types can be replaced by imports.
//!
//! # Mapping overview
//!
//! | VUMA construct          | SCG node / edge                       |
//! |-------------------------|---------------------------------------|
//! | `allocate(size)`        | `AllocationNode`                      |
//! | `free(ptr)`             | `DeallocationNode`                    |
//! | `*expr`                 | `AccessNode` (dereference)            |
//! | `expr as Type`          | `CastNode`                            |
//! | `let x = …`            | `ComputationNode` + DataFlow edge     |
//! | `x = …`                | `ComputationNode` + DataFlow edge     |
//! | `if / while`           | `ControlFlowNode`                     |
//! | `fn f(…) { … }`        | `SubgraphNode` (nested SCG)           |
//! | `region r = alloc(…)`   | `RegionNode` + `AllocationNode`       |

use crate::ast::*;
use crate::error::{ParseError, Span};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Local SCG types (to be replaced by vuma-scg imports)
// ---------------------------------------------------------------------------

/// Unique identifier for an SCG node.
pub type NodeId = usize;

/// Unique identifier for an SCG edge.
pub type EdgeId = usize;

/// A complete Structured Computation Graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SCG {
    /// All nodes in the graph, indexed by [`NodeId`].
    pub nodes: Vec<ScgNode>,
    /// All edges in the graph, indexed by [`EdgeId`].
    pub edges: Vec<ScgEdge>,
    /// Entry node id (the first operation).
    pub entry: Option<NodeId>,
}

impl SCG {
    /// Create an empty SCG.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            entry: None,
        }
    }

    /// Add a node and return its id.
    pub fn add_node(&mut self, node: ScgNode) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(node);
        if self.entry.is_none() {
            self.entry = Some(id);
        }
        id
    }

    /// Add an edge.
    pub fn add_edge(&mut self, edge: ScgEdge) -> EdgeId {
        let id = self.edges.len();
        self.edges.push(edge);
        id
    }
}

impl Default for SCG {
    fn default() -> Self {
        Self::new()
    }
}

/// Classification of SCG nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScgNode {
    /// Memory allocation: `allocate(size)`.
    AllocationNode {
        /// Logical name / variable bound to the allocation.
        name: String,
        /// Size in bytes (may be symbolic).
        size: String,
        /// Source span.
        span: Span,
    },
    /// Memory deallocation: `free(ptr)`.
    DeallocationNode {
        /// Name of the pointer / region being freed.
        target: String,
        /// Source span.
        span: Span,
    },
    /// Memory access (dereference): `*ptr` or `(*ptr).field`.
    AccessNode {
        /// Expression being accessed (string representation).
        target: String,
        /// Optional field name.
        field: Option<String>,
        /// Source span.
        span: Span,
    },
    /// Type cast: `expr as Type`.
    CastNode {
        /// Source expression (string representation).
        source: String,
        /// Target type.
        target_type: String,
        /// Source span.
        span: Span,
    },
    /// General computation (assignment, arithmetic, call).
    ComputationNode {
        /// Human-readable description.
        description: String,
        /// Variables defined by this computation.
        defines: Vec<String>,
        /// Variables used by this computation.
        uses: Vec<String>,
        /// Source span.
        span: Span,
    },
    /// Control-flow branch.
    ControlFlowNode {
        /// "if" or "while".
        kind: String,
        /// Condition expression (string).
        condition: String,
        /// SCG for the then-branch.
        then_subgraph: Box<SCG>,
        /// SCG for the else-branch (if any).
        else_subgraph: Option<Box<SCG>>,
        /// Source span.
        span: Span,
    },
    /// Function subgraph.
    SubgraphNode {
        /// Function name.
        name: String,
        /// Parameter names.
        params: Vec<String>,
        /// Nested SCG for the function body.
        body: Box<SCG>,
        /// Source span.
        span: Span,
    },
    /// Region (arena) node.
    RegionNode {
        /// Region name.
        name: String,
        /// Size expression.
        size: String,
        /// Source span.
        span: Span,
    },
}

/// An edge in the SCG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScgEdge {
    /// Source node.
    pub from: NodeId,
    /// Target node.
    pub to: NodeId,
    /// Edge classification.
    pub kind: EdgeKind,
}

/// Classification of SCG edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    /// Sequential control flow.
    ControlFlow,
    /// Data dependency: the target reads a value produced by the source.
    DataFlow,
    /// The target node is inside a region owned by the source.
    RegionOwnership,
}

// ---------------------------------------------------------------------------
// Converter
// ---------------------------------------------------------------------------

/// Converts a VUMA [`Program`] into an [`SCG`].
pub struct AstToScg {
    /// Variable scopes: maps variable name → the [`NodeId`] that last defined it.
    scopes: Vec<HashMap<String, NodeId>>,
}

impl AstToScg {
    /// Create a new converter.
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    /// Convert a parsed program into an SCG.
    pub fn convert(&mut self, program: &Program) -> Result<SCG, ParseError> {
        let mut scg = SCG::new();
        let mut prev_node: Option<NodeId> = None;

        for item in &program.items {
            let node_id = self.convert_item(item, &mut scg)?;

            // Link sequential control flow.
            if let Some(prev) = prev_node {
                scg.add_edge(ScgEdge {
                    from: prev,
                    to: node_id,
                    kind: EdgeKind::ControlFlow,
                });
            }
            prev_node = Some(node_id);
        }

        Ok(scg)
    }

    // -- item conversion -----------------------------------------------------

    fn convert_item(&mut self, item: &Item, scg: &mut SCG) -> Result<NodeId, ParseError> {
        match item {
            Item::FnDef(f) => self.convert_fn_def(f, scg),
            Item::RegionDef(r) => self.convert_region_def(r, scg),
            Item::Import(i) => {
                // Imports are metadata — represent as a lightweight node.
                let id = scg.add_node(ScgNode::ComputationNode {
                    description: format!("import \"{}\"", i.path),
                    defines: vec![],
                    uses: vec![],
                    span: i.span,
                });
                Ok(id)
            }
            Item::Export(e) => {
                let id = scg.add_node(ScgNode::ComputationNode {
                    description: format!("export {}", e.name),
                    defines: vec![],
                    uses: vec![e.name.clone()],
                    span: e.span,
                });
                Ok(id)
            }
            Item::Const(c) => {
                let uses = self.expr_uses(&c.value);
                let id = scg.add_node(ScgNode::ComputationNode {
                    description: format!("const {} = …", c.name),
                    defines: vec![c.name.clone()],
                    uses,
                    span: c.span,
                });
                self.define_var(&c.name, id);
                self.add_data_flow_edges(&c.value, id, scg);
                Ok(id)
            }
            Item::Stmt(s) => self.convert_stmt(s, scg),
        }
    }

    fn convert_fn_def(&mut self, f: &FnDef, scg: &mut SCG) -> Result<NodeId, ParseError> {
        // Build a nested SCG for the function body.
        self.push_scope();
        let mut body_scg = SCG::new();
        let mut prev_node: Option<NodeId> = None;

        // Define parameters in the function scope.
        for p in &f.params {
            // We don't have real nodes for params; use a placeholder.
            let param_id = body_scg.add_node(ScgNode::ComputationNode {
                description: format!("param {}", p.name),
                defines: vec![p.name.clone()],
                uses: vec![],
                span: p.span,
            });
            self.define_var(&p.name, param_id);
        }

        for stmt in &f.body.statements {
            let stmt_id = self.convert_stmt(stmt, &mut body_scg)?;
            if let Some(prev) = prev_node {
                body_scg.add_edge(ScgEdge {
                    from: prev,
                    to: stmt_id,
                    kind: EdgeKind::ControlFlow,
                });
            }
            prev_node = Some(stmt_id);
        }

        self.pop_scope();

        let id = scg.add_node(ScgNode::SubgraphNode {
            name: f.name.clone(),
            params: f.params.iter().map(|p| p.name.clone()).collect(),
            body: Box::new(body_scg),
            span: f.span,
        });
        Ok(id)
    }

    fn convert_region_def(&mut self, r: &RegionDef, scg: &mut SCG) -> Result<NodeId, ParseError> {
        let size_str = self.expr_to_string(&r.size_expr);

        // Region node.
        let region_id = scg.add_node(ScgNode::RegionNode {
            name: r.name.clone(),
            size: size_str.clone(),
            span: r.span,
        });

        // Allocation node within the region.
        let alloc_id = scg.add_node(ScgNode::AllocationNode {
            name: r.name.clone(),
            size: size_str,
            span: r.span,
        });

        // Region owns the allocation.
        scg.add_edge(ScgEdge {
            from: region_id,
            to: alloc_id,
            kind: EdgeKind::RegionOwnership,
        });

        self.define_var(&r.name, region_id);
        self.add_data_flow_edges(&r.size_expr, alloc_id, scg);

        Ok(region_id)
    }

    // -- statement conversion ------------------------------------------------

    fn convert_stmt(&mut self, stmt: &Stmt, scg: &mut SCG) -> Result<NodeId, ParseError> {
        match stmt {
            Stmt::Let(l) => {
                let uses = self.expr_uses(&l.value);
                let id = scg.add_node(ScgNode::ComputationNode {
                    description: format!("let {} = …", l.name),
                    defines: vec![l.name.clone()],
                    uses,
                    span: l.span,
                });
                self.define_var(&l.name, id);
                self.add_data_flow_edges(&l.value, id, scg);
                Ok(id)
            }
            Stmt::Assign(a) => {
                let target_name = self.assign_target_name(&a.target);
                let uses = self.expr_uses(&a.value);
                let id = scg.add_node(ScgNode::ComputationNode {
                    description: format!("{} = …", target_name),
                    defines: vec![target_name],
                    uses,
                    span: a.span,
                });
                self.add_data_flow_edges(&a.value, id, scg);
                Ok(id)
            }
            Stmt::Allocate(alloc) => {
                let size_str = self.expr_to_string(&alloc.size);
                let id = scg.add_node(ScgNode::AllocationNode {
                    name: String::new(),
                    size: size_str,
                    span: alloc.span,
                });
                self.add_data_flow_edges(&alloc.size, id, scg);
                Ok(id)
            }
            Stmt::Free(fr) => {
                let target_str = self.expr_to_string(&fr.ptr);
                let id = scg.add_node(ScgNode::DeallocationNode {
                    target: target_str,
                    span: fr.span,
                });
                self.add_data_flow_edges(&fr.ptr, id, scg);
                Ok(id)
            }
            Stmt::Access(acc) => {
                let (target, field) = self.extract_access(&acc.expr);
                let id = scg.add_node(ScgNode::AccessNode {
                    target,
                    field,
                    span: acc.span,
                });
                Ok(id)
            }
            Stmt::Cast(c) => {
                let source_str = self.expr_to_string(&c.expr);
                let id = scg.add_node(ScgNode::CastNode {
                    source: source_str,
                    target_type: c.target_type.to_string(),
                    span: c.span,
                });
                self.add_data_flow_edges(&c.expr, id, scg);
                Ok(id)
            }
            Stmt::If(if_s) => {
                self.push_scope();
                let then_scg = self.convert_block_to_scg(&if_s.then_block)?;
                self.pop_scope();

                let else_scg = if let Some(eb) = &if_s.else_block {
                    self.push_scope();
                    let es = self.convert_block_to_scg(eb)?;
                    self.pop_scope();
                    Some(Box::new(es))
                } else {
                    None
                };

                let cond_str = self.expr_to_string(&if_s.condition);
                let id = scg.add_node(ScgNode::ControlFlowNode {
                    kind: "if".to_string(),
                    condition: cond_str,
                    then_subgraph: Box::new(then_scg),
                    else_subgraph: else_scg,
                    span: if_s.span,
                });
                Ok(id)
            }
            Stmt::While(wh) => {
                self.push_scope();
                let body_scg = self.convert_block_to_scg(&wh.body)?;
                self.pop_scope();

                let cond_str = self.expr_to_string(&wh.condition);
                let id = scg.add_node(ScgNode::ControlFlowNode {
                    kind: "while".to_string(),
                    condition: cond_str,
                    then_subgraph: Box::new(body_scg),
                    else_subgraph: None,
                    span: wh.span,
                });
                Ok(id)
            }
            Stmt::Return(r) => {
                let mut uses = Vec::new();
                if let Some(v) = &r.value {
                    uses = self.expr_uses(v);
                }
                let id = scg.add_node(ScgNode::ComputationNode {
                    description: "return".to_string(),
                    defines: vec![],
                    uses,
                    span: r.span,
                });
                if let Some(v) = &r.value {
                    self.add_data_flow_edges(v, id, scg);
                }
                Ok(id)
            }
            Stmt::Expr(e) => {
                let uses = self.expr_uses(&e.expr);
                let desc = self.expr_to_string(&e.expr);
                let id = scg.add_node(ScgNode::ComputationNode {
                    description: desc,
                    defines: vec![],
                    uses,
                    span: e.span,
                });
                self.add_data_flow_edges(&e.expr, id, scg);
                Ok(id)
            }
        }
    }

    /// Convert a block of statements into a nested SCG.
    fn convert_block_to_scg(&mut self, block: &Block) -> Result<SCG, ParseError> {
        let mut scg = SCG::new();
        let mut prev_node: Option<NodeId> = None;

        for stmt in &block.statements {
            let node_id = self.convert_stmt(stmt, &mut scg)?;
            if let Some(prev) = prev_node {
                scg.add_edge(ScgEdge {
                    from: prev,
                    to: node_id,
                    kind: EdgeKind::ControlFlow,
                });
            }
            prev_node = Some(node_id);
        }

        Ok(scg)
    }

    // -- scope management ----------------------------------------------------

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define_var(&mut self, name: &str, node_id: NodeId) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), node_id);
        }
    }

    /// Look up which node last defined a variable (for DataFlow edges).
    fn lookup_var(&self, name: &str) -> Option<NodeId> {
        for scope in self.scopes.iter().rev() {
            if let Some(&id) = scope.get(name) {
                return Some(id);
            }
        }
        None
    }

    // -- data-flow edge helpers ----------------------------------------------

    /// Add DataFlow edges from every variable used in `expr` to `target_node`.
    fn add_data_flow_edges(&self, expr: &Expr, target_node: NodeId, scg: &mut SCG) {
        for var_name in self.expr_uses(expr) {
            if let Some(source_node) = self.lookup_var(&var_name) {
                scg.add_edge(ScgEdge {
                    from: source_node,
                    to: target_node,
                    kind: EdgeKind::DataFlow,
                });
            }
        }
    }

    /// Collect all variable names used (read) by an expression.
    fn expr_uses(&self, expr: &Expr) -> Vec<String> {
        let mut uses = Vec::new();
        self.collect_uses(expr, &mut uses);
        uses.sort();
        uses.dedup();
        uses
    }

    fn collect_uses(&self, expr: &Expr, uses: &mut Vec<String>) {
        match expr {
            Expr::Var { name, .. } => {
                uses.push(name.clone());
            }
            Expr::Lit { .. } => {}
            Expr::BinOp { lhs, rhs, .. } => {
                self.collect_uses(lhs, uses);
                self.collect_uses(rhs, uses);
            }
            Expr::UnOp { expr, .. } => {
                self.collect_uses(expr, uses);
            }
            Expr::Call { callee, args, .. } => {
                self.collect_uses(callee, uses);
                for a in args {
                    self.collect_uses(a, uses);
                }
            }
            Expr::AddressOf { expr, .. } => {
                self.collect_uses(expr, uses);
            }
            Expr::Deref { expr, .. } => {
                self.collect_uses(expr, uses);
            }
            Expr::Offset { base, offset, .. } => {
                self.collect_uses(base, uses);
                self.collect_uses(offset, uses);
            }
            Expr::Cast { expr, .. } => {
                self.collect_uses(expr, uses);
            }
            Expr::Index { expr, index, .. } => {
                self.collect_uses(expr, uses);
                self.collect_uses(index, uses);
            }
            Expr::StructInit { fields, .. } => {
                for (_, v) in fields {
                    self.collect_uses(v, uses);
                }
            }
            Expr::FieldAccess { expr, .. } => {
                self.collect_uses(expr, uses);
            }
        }
    }

    // -- stringification helpers (for node descriptions) ---------------------

    /// Convert an expression to a human-readable string for SCG descriptions.
    fn expr_to_string(&self, expr: &Expr) -> String {
        match expr {
            Expr::Var { name, .. } => name.clone(),
            Expr::Lit { value, .. } => match value {
                Lit::Int(i) => i.to_string(),
                Lit::Float(f) => f.to_string(),
                Lit::String(s) => format!("\"{}\"", s),
                Lit::Bool(b) => b.to_string(),
                Lit::Address(a) => format!("0x{:X}", a),
            },
            Expr::BinOp { op, lhs, rhs, .. } => {
                format!(
                    "({} {} {})",
                    self.expr_to_string(lhs),
                    self.bin_op_symbol(op),
                    self.expr_to_string(rhs)
                )
            }
            Expr::UnOp { op, expr, .. } => match op {
                UnOp::Neg => format!("(-{})", self.expr_to_string(expr)),
                UnOp::Not => format!("(!{})", self.expr_to_string(expr)),
                UnOp::Deref => format!("*{}", self.expr_to_string(expr)),
            },
            Expr::Call { callee, args, .. } => {
                let a: Vec<String> = args.iter().map(|e| self.expr_to_string(e)).collect();
                format!("{}({})", self.expr_to_string(callee), a.join(", "))
            }
            Expr::AddressOf { expr, .. } => format!("@{}", self.expr_to_string(expr)),
            Expr::Deref { expr, .. } => format!("*{}", self.expr_to_string(expr)),
            Expr::Offset { base, offset, .. } => {
                format!("{}+{}", self.expr_to_string(base), self.expr_to_string(offset))
            }
            Expr::Cast { expr, target_type, .. } => {
                format!("({} as {})", self.expr_to_string(expr), target_type)
            }
            Expr::Index { expr, index, .. } => {
                format!("{}[{}]", self.expr_to_string(expr), self.expr_to_string(index))
            }
            Expr::StructInit { name, fields, .. } => {
                let f: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, self.expr_to_string(v)))
                    .collect();
                format!("{} {{ {} }}", name, f.join(", "))
            }
            Expr::FieldAccess { expr, field, .. } => {
                format!("{}.{}", self.expr_to_string(expr), field)
            }
        }
    }

    fn bin_op_symbol(&self, op: &BinOp) -> &'static str {
        match op {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }

    /// Extract the name of an assignment target.
    fn assign_target_name(&self, target: &AssignTarget) -> String {
        match target {
            AssignTarget::Var { name, .. } => name.clone(),
            AssignTarget::Deref { expr, .. } => format!("*{}", self.expr_to_string(expr)),
            AssignTarget::DerefField { expr, field, .. } => {
                format!("(*{}).{}", self.expr_to_string(expr), field)
            }
            AssignTarget::Index { expr, index, .. } => {
                format!("{}[{}]", self.expr_to_string(expr), self.expr_to_string(index))
            }
        }
    }

    /// Extract (target_expr, optional_field) from a dereference expression.
    fn extract_access(&self, expr: &Expr) -> (String, Option<String>) {
        match expr {
            Expr::Deref { expr: inner, .. } => {
                // Check for (*expr).field pattern.
                (self.expr_to_string(inner), None)
            }
            Expr::FieldAccess { expr: inner, field, .. } => {
                (self.expr_to_string(inner), Some(field.clone()))
            }
            _ => (self.expr_to_string(expr), None),
        }
    }
}

impl Default for AstToScg {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Serde imports
// ---------------------------------------------------------------------------
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;

    #[test]
    fn convert_simple_region() {
        let source = "region pool = allocate(1024);";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse");
        let mut converter = AstToScg::new();
        let scg = converter.convert(&program).expect("convert");

        // Expect at least a RegionNode and an AllocationNode.
        assert!(scg.nodes.len() >= 2);
        assert!(scg.edges.iter().any(|e| e.kind == EdgeKind::RegionOwnership));
    }

    #[test]
    fn convert_fn_def() {
        let source = "fn add(a: u32, b: u32) -> u32 { return a; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse");
        let mut converter = AstToScg::new();
        let scg = converter.convert(&program).expect("convert");

        assert_eq!(scg.nodes.len(), 1);
        match &scg.nodes[0] {
            ScgNode::SubgraphNode { name, .. } => assert_eq!(name, "add"),
            other => panic!("expected SubgraphNode, got {:?}", other),
        }
    }

    #[test]
    fn convert_example_program() {
        let source = r#"
            region memory_pool = allocate(1024);
            fn main() {
                node_ptr = memory_pool + 64;
                header = node_ptr as *NodeHeader;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse");
        let mut converter = AstToScg::new();
        let scg = converter.convert(&program).expect("convert");

        // Should have region node, allocation node, and fn subgraph.
        assert!(scg.nodes.len() >= 2);
    }
}
