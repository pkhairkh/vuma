//! AST → SCG (Structured Computation Graph) conversion.
//!
//! This module bridges the parser's output ([`Program`]) to the VUMA
//! intermediate representation: the **Structured Computation Graph** (SCG).
//!
//! The SCG is a directed graph where:
//! - **Nodes** represent computational operations (allocation, access,
//!   deallocation, computation, control flow, cast, effect, phantom).
//! - **Edges** represent data-flow, control-flow, derivation, and
//!   annotation dependencies.
//! - **Regions** group nodes into memory scopes with security boundaries.
//!
//! # Mapping overview
//!
//! | VUMA construct          | SCG node / edge                                  |
//! |-------------------------|--------------------------------------------------|
//! | `fn f(…) { … }`        | Region + FunctionEntry/FunctionReturn nodes      |
//! | `let x = …`            | Computation node + DataFlow edges                |
//! | `allocate(size)`        | Allocation node (size, align from type)          |
//! | `free(ptr)`            | Deallocation node (references allocation)        |
//! | `ptr + offset`          | Derivation edge from allocation to computation   |
//! | `derive(ptr, region)`   | Derivation edge                                  |
//! | `expr as Type`          | Cast node (from_type, to_type, BD reference)     |
//! | `*ptr` / `ptr.field`    | Access node (Read/Write, size)                   |
//! | `if/else`              | Control Branch node + ControlFlow edges           |
//! | `while/for/loop`        | LoopHeader/LoopExit nodes + back edges           |
//! | `match`                | Branch + Join nodes                               |
//! | `f(args)`              | FunctionEntry + FunctionReturn nodes             |
//! | `async { … }`          | Parallel region (security-boundary region)       |
//! | `spawn expr`           | Effect node (parallel fork)                      |
//! | `sync { … }`           | Synchronization edges (Annotation)               |
//! | `region r = alloc(…)`  | Region + Allocation node + Derivation edge       |
//! | `sizeof(T)` / `alignof(T)` | Computation nodes                            |
//!
//! # Enhancement notes
//!
//! Each mapping is enhanced with deeper semantic fidelity:
//!
//! 1. **Fn → entry/exit**: return type stored on entry label; body nodes
//!    verified as intermediaries between entry/return via ControlFlow edges.
//! 2. **let/assign**: type annotations propagate `result_type`; allocations
//!    inside let get correct size/align from the annotated type.
//! 3. **alloc**: when a type annotation is present, size is computed from
//!    `sizeof(T)` and alignment from `alignof(T)`.
//! 4. **free**: deallocation node receives the allocation's region_id so that
//!    alloc/free region consistency can be validated.
//! 5. **ptr derive/offset**: for constant offsets the Derivation edge gets a
//!    label recording the byte offset, enabling offset-aware analysis.
//! 6. **ptr cast**: narrowing vs widening classification; signedness change
//!    detection for integer casts.
//! 7. **read/write**: field accesses produce Access nodes with computed
//!    offset (best-effort); ReadWrite mode for compound assignments.
//! 8. **if/else**: branch edges labelled "then"/"else" for precise CFG
//!    reconstruction.
//! 9. **while/for**: condition data-flow re-entering the LoopHeader is
//!    explicitly tracked; for-loop iterator variable has DataFlow edge.
//! 10. **Function calls**: per-argument DataFlow edges from caller variables
//!     to FunctionEntry; return value DataFlow edge from FunctionReturn.
//! 11. **async/spawn**: spawn Effect node connects to async region's fork
//!     via Derivation edge; parallel fork/join boundaries explicit.
//! 12. **sync**: sync_enter / sync_exit effect nodes bound the body;
//!     Annotation edges from body nodes to sync_exit enforce ordering.

use crate::ast::*;
use crate::error::{ParseError, Span};
use std::collections::HashMap;
use vuma_scg::{
    AccessMode, AccessNode, AllocationNode, CastNode, ClosureEnvNode, node::ComputationKind, ComputationNode, ControlKind,
    ControlNode, DeallocationNode, DeploymentTarget, EdgeKind, EffectNode, NodeId, NodePayload,
    NodeType, PhantomNode, ProgramPoint, RegionId, SCGRegion, SyscallNode, VTableNode, SCG,
};

// ---------------------------------------------------------------------------
// Converter
// ---------------------------------------------------------------------------

/// Converts a VUMA [`Program`] into a [`vuma_scg::SCG`].
///
/// The converter walks the AST in order, emitting nodes and edges into the
/// SCG. It tracks variable definitions to automatically insert DataFlow
/// edges from the producer of a value to its consumers.
///
/// # Region strategy
///
/// Each function definition gets its own [`SCGRegion`] (Heap-deployed).
/// `async` blocks get a `Shared`-deployed security-boundary region.
/// Top-level statements go into a default region.
pub struct AstToScg {
    /// Variable scopes: maps variable name → the [`NodeId`] that last defined it.
    scopes: Vec<HashMap<String, NodeId>>,
    /// Maps allocation variable names to the NodeId of their Allocation node
    /// (so that `free(ptr)` can reference the correct allocation).
    alloc_defs: Vec<HashMap<String, NodeId>>,
    /// Counter for generating region IDs.
    next_region_id: u64,
    /// The default (top-level) region ID.
    default_region: RegionId,
    /// Current function's return type (for propagating to return expr Computation nodes).
    current_return_type: Option<String>,
    /// Struct definitions: maps struct name → list of (field_name, field_type, offset)
    struct_table: HashMap<String, Vec<(String, String, u64)>>,
    /// PMT (Wave 2): layout definitions — maps layout name → list of
    /// (field_name, field_type, byte_offset). Built from `Item::LayoutDef`
    /// items at the start of `convert()`, before any function bodies are
    /// lowered. Used to compute field offsets for state.field reads/writes.
    layouts: HashMap<String, Vec<(String, Type, u64)>>,
    /// PMT (Wave 2): state-typed variable scopes — maps var name → layout
    /// name. Pushed/popped alongside `scopes`. A var is state-typed if it
    /// was bound by `let p = state_new(Layout)` or annotated `: State<L>`,
    /// or is a `State<L>` function parameter.
    var_types: Vec<HashMap<String, String>>,
}

impl AstToScg {
    /// Create a new converter.
    pub fn new() -> Self {
        let default_region = RegionId::new(0);
        Self {
            scopes: vec![HashMap::new()],
            alloc_defs: vec![HashMap::new()],
            next_region_id: 1,
            current_return_type: None,
            default_region,
            struct_table: HashMap::new(),
            layouts: HashMap::new(),
            var_types: vec![HashMap::new()],
        }
    }

    /// Allocate a fresh region ID.
    fn alloc_region_id(&mut self) -> RegionId {
        let id = RegionId::new(self.next_region_id);
        self.next_region_id += 1;
        id
    }

    /// Convert a parsed program into an SCG.
    pub fn convert(&mut self, program: &Program) -> Result<SCG, ParseError> {
        let mut scg = SCG::new();

        // PMT (Wave 2): pre-pass — register all layout definitions before
        // lowering any function bodies. Field offsets are computed once
        // here and reused by state.field reads/writes during body lowering.
        for item in &program.items {
            if let Item::LayoutDef(ld) = item {
                self.register_layout(ld);
            }
        }

        // Create the default top-level region.
        let mut default_region = SCGRegion::new(self.default_region, DeploymentTarget::Heap);
        default_region.scope_level = 0;

        let mut prev_node: Option<NodeId> = None;

        for item in &program.items {
            let node_id = self.convert_item(item, &mut scg, &mut default_region)?;

            // Link sequential control flow.
            if let Some(prev) = prev_node {
                let _ = scg.add_edge(prev, node_id, EdgeKind::ControlFlow);
            }
            prev_node = Some(node_id);
        }

        scg.add_region(default_region);

        Ok(scg)
    }

    /// PMT (Wave 2): register a layout definition. Computes field offsets
    /// sequentially with alignment padding (mirroring `LayoutRegistry` in
    /// vuma-bd, which we don't depend on to keep the parser crate lean).
    fn register_layout(&mut self, ld: &LayoutDef) {
        let mut offset: u64 = 0;
        let mut max_align: u64 = 1;
        let mut fields: Vec<(String, Type, u64)> = Vec::with_capacity(ld.fields.len());
        for (fname, ftype) in &ld.fields {
            let falign = self.type_alignment(Some(ftype)).max(1);
            let fsize = self.type_size(ftype);
            if falign > 1 && offset % falign != 0 {
                offset = align_to_local(offset, falign);
            }
            max_align = max_align.max(falign);
            fields.push((fname.clone(), ftype.clone(), offset));
            offset += fsize;
        }
        let alignment = max_align.max(1);
        if offset > 0 && offset % alignment != 0 {
            offset = align_to_local(offset, alignment);
        }
        // `offset` is now the layout's total_size (used by StateInit → Alloc).
        let _ = offset; // total_size computed on demand via layout_total_size
        self.layouts.insert(ld.name.clone(), fields);
    }

    /// PMT (Wave 2): look up a layout by name.
    fn lookup_layout(&self, name: &str) -> Option<&Vec<(String, Type, u64)>> {
        self.layouts.get(name)
    }

    /// PMT (Wave 2): compute a layout's total size (sum of field sizes with
    /// alignment padding + tail padding to the layout's alignment).
    fn layout_total_size(&self, name: &str) -> u64 {
        let fields = match self.lookup_layout(name) {
            Some(f) => f,
            None => return 0,
        };
        let mut total: u64 = 0;
        let mut max_align: u64 = 1;
        for (_, ftype, _) in fields {
            let falign = self.type_alignment(Some(ftype)).max(1);
            let fsize = self.type_size(ftype);
            if falign > 1 && total % falign != 0 {
                total = align_to_local(total, falign);
            }
            max_align = max_align.max(falign);
            total += fsize;
        }
        if total > 0 && total % max_align != 0 {
            total = align_to_local(total, max_align);
        }
        total
    }

    /// PMT (Wave 2): look up a field's (offset, size, Type) within a layout.
    /// Supports nested layout-typed fields via cumulative offset computation.
    fn lookup_field(&self, layout_name: &str, field: &str) -> Option<(u64, u64, Type)> {
        let fields = self.lookup_layout(layout_name)?;
        for (fname, ftype, foffset) in fields {
            if fname == field {
                let fsize = self.type_size(ftype);
                return Some((*foffset, fsize, ftype.clone()));
            }
        }
        None
    }

    /// PMT (Wave 2): compute the cumulative (offset, size, leaf_type) for a
    /// chain of field accesses starting from a state-typed var. For example,
    /// `l.a.x` where `l: Line`, `a: Point` (at offset 0 in Line), `x: u32`
    /// (at offset 0 in Point) → (0, 4, u32).
    ///
    /// `chain` is a slice of field names in left-to-right order (e.g.
    /// `["a", "x"]` for `l.a.x`).
    fn resolve_field_chain(
        &self,
        start_layout: &str,
        chain: &[String],
    ) -> Option<(u64, u64, Type)> {
        if chain.is_empty() {
            return None;
        }
        let mut layout = start_layout.to_string();
        let mut cum_offset: u64 = 0;
        let mut last_type: Option<Type> = None;
        let mut last_size: u64 = 0;
        for field in chain {
            let (foffset, fsize, ftype) = self.lookup_field(&layout, field)?;
            cum_offset += foffset;
            last_size = fsize;
            last_type = Some(ftype.clone());
            // Advance: if the field's type is a layout name, descend into it;
            // otherwise this is the leaf (chain should end here).
            if let Type::BDBase(name) = &ftype {
                if self.lookup_layout(name).is_some() {
                    layout = name.clone();
                }
            }
        }
        match last_type {
            Some(t) => Some((cum_offset, last_size, t)),
            None => None,
        }
    }

    fn define_var_type(&mut self, name: &str, layout_name: &str) {
        if let Some(scope) = self.var_types.last_mut() {
            scope.insert(name.to_string(), layout_name.to_string());
        }
    }
    fn lookup_var_type(&self, name: &str) -> Option<&str> {
        for scope in self.var_types.iter().rev() {
            if let Some(layout) = scope.get(name) {
                return Some(layout.as_str());
            }
        }
        None
    }

    // -- PMT (Wave 2): state field read/write lowering ----------------------

    /// Walk an `AssignTarget` and detect whether it's a state-typed field
    /// write. Returns `Some((state_var_name, field_chain))` when the target
    /// is `state_var.field1.field2... = value` and `state_var` is state-typed
    /// (or might be — the actual layout lookup happens later).
    ///
    /// For `p.x = v` → `Some(("p", ["x"]))`.
    /// For `l.a.x = v` → `Some(("l", ["a", "x"]))`.
    /// For `*ptr = v` or `arr[i] = v` → `None`.
    fn extract_state_write_target(&self, target: &AssignTarget) -> Option<(String, Vec<String>)> {
        let (expr, field) = match target {
            AssignTarget::DerefField { expr, field, .. } => (expr, field),
            _ => return None,
        };
        // Walk the FieldAccess chain to collect (base_var, [field1, field2, ...]).
        let mut chain = vec![field.clone()];
        let mut current = expr.as_ref();
        loop {
            match current {
                Expr::FieldAccess { expr: inner, field: f, .. } => {
                    chain.push(f.clone());
                    current = inner.as_ref();
                }
                Expr::Var { name, .. } => {
                    // Order the chain from outermost to innermost field.
                    chain.reverse();
                    return Some((name.clone(), chain));
                }
                _ => return None,
            }
        }
    }

    /// Walk an `Expr` tree and collect `(base_var, field_chain)` for a
    /// state-typed FieldAccess. Returns `None` for non-FieldAccess exprs
    /// or chains that don't bottom out in a Var.
    fn extract_state_read_chain(expr: &Expr) -> Option<(String, Vec<String>)> {
        let mut chain = Vec::new();
        let mut current = expr;
        loop {
            match current {
                Expr::FieldAccess { expr: inner, field, .. } => {
                    chain.push(field.clone());
                    current = inner.as_ref();
                }
                Expr::Var { name, .. } => {
                    if chain.is_empty() {
                        return None;
                    }
                    chain.reverse();
                    return Some((name.clone(), chain));
                }
                _ => return None,
            }
        }
    }

    /// PMT (Wave 2): walk an `Expr` tree, find any `state.field` reads (i.e.
    /// `Expr::FieldAccess` chains rooted at a state-typed Var), emit an
    /// `Access(Read)` node for each, and rewrite the FieldAccess into a
    /// `Var("v_<node_id>")` referring to the Load's result.
    ///
    /// This must be called BEFORE the parent statement's Computation node is
    /// created so the label string uses the temp var names (which the codegen
    /// bridge resolves to the Load results via the SCG's DataFlow edges).
    fn lower_state_reads_in_expr(
        &mut self,
        expr: &Expr,
        scg: &mut SCG,
        region: &mut SCGRegion,
    ) -> Expr {
        match expr {
            Expr::FieldAccess { .. } => {
                // Try to interpret as a state-field read.
                if let Some((var_name, chain)) = Self::extract_state_read_chain(expr) {
                    if let Some(layout_name) = self.lookup_var_type(&var_name).map(|s| s.to_string()) {
                        if let Some((offset, size, _field_ty)) =
                            self.resolve_field_chain(&layout_name, &chain)
                        {
                            // Emit an Access(Read) node for this state-field read.
                            let access_id = scg.add_node(
                                NodeType::Access,
                                NodePayload::Access(AccessNode {
                                    mode: AccessMode::Read,
                                    region_id: region.id,
                                    offset: Some(offset),
                                    access_size: Some(size),
                                }),
                                ProgramPoint { file: None, line: None, column: None, offset: None },
                            );
                            region.add_node(access_id);

                            // DataFlow edge from the state var's defining node
                            // (position 0 = the buffer pointer for the Load).
                            if let Some(state_node) = self.lookup_var(&var_name) {
                                let _ = scg.add_edge(state_node, access_id, EdgeKind::DataFlow);
                            }

                            // Register a temp var "v_<access_id>" pointing to
                            // the Access node, so add_data_flow_edges on the
                            // parent Computation finds it and the codegen
                            // bridge's `node_var(node_id, "val") = "v_<id>"`
                            // convention matches.
                            let temp_name = format!("v_{}", access_id.as_u64());
                            self.define_var(&temp_name, access_id);

                            // Rewrite the FieldAccess to a Var reference.
                            return Expr::Var {
                                name: temp_name,
                                span: crate::error::Span::synthetic(),
                            };
                        }
                    }
                }
                // Not a state-typed FieldAccess — recurse into the inner expr.
                expr.clone()
            }
            Expr::BinOp { op, lhs, rhs, span } => Expr::BinOp {
                op: *op,
                lhs: Box::new(self.lower_state_reads_in_expr(lhs, scg, region)),
                rhs: Box::new(self.lower_state_reads_in_expr(rhs, scg, region)),
                span: *span,
            },
            Expr::UnOp { op, expr: inner, span } => Expr::UnOp {
                op: *op,
                expr: Box::new(self.lower_state_reads_in_expr(inner, scg, region)),
                span: *span,
            },
            Expr::Call { callee, args, span } => {
                let new_args: Vec<Expr> = args
                    .iter()
                    .map(|a| self.lower_state_reads_in_expr(a, scg, region))
                    .collect();
                let new_callee = self.lower_state_reads_in_expr(callee, scg, region);
                Expr::Call {
                    callee: Box::new(new_callee),
                    args: new_args,
                    span: *span,
                }
            }
            Expr::Cast { expr: inner, target_type, span } => Expr::Cast {
                expr: Box::new(self.lower_state_reads_in_expr(inner, scg, region)),
                target_type: target_type.clone(),
                span: *span,
            },
            Expr::Index { expr: inner, index, span } => Expr::Index {
                expr: Box::new(self.lower_state_reads_in_expr(inner, scg, region)),
                index: Box::new(self.lower_state_reads_in_expr(index, scg, region)),
                span: *span,
            },
            Expr::TypeAscription { expr: inner, ty, span } => Expr::TypeAscription {
                expr: Box::new(self.lower_state_reads_in_expr(inner, scg, region)),
                ty: ty.clone(),
                span: *span,
            },
            // Leaf exprs and other constructs: return as-is.
            _ => expr.clone(),
        }
    }


    // -- helpers: span → ProgramPoint ---------------------------------------

    fn span_to_pp(&self, span: &Span) -> ProgramPoint {
        ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: Some(span.start as u64),
        }
    }

    // -- item conversion -----------------------------------------------------

    fn convert_item(
        &mut self,
        item: &Item,
        scg: &mut SCG,
        default_region: &mut SCGRegion,
    ) -> Result<NodeId, ParseError> {
        match item {
            Item::FnDef(f) => self.convert_fn_def(f, scg),
            Item::RegionDef(r) => self.convert_region_def(r, scg, default_region),
            Item::Import(i) => {
                let id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(format!("import \"{}\"", i.path)),
                        result_type: None,
                        tail_call: false,
                    }),
                    self.span_to_pp(&i.span),
                );
                default_region.add_node(id);
                Ok(id)
            }
            Item::Export(e) => {
                let id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(format!("export {}", e.name)),
                        result_type: None,
                        tail_call: false,
                    }),
                    self.span_to_pp(&e.span),
                );
                default_region.add_node(id);
                Ok(id)
            }
            Item::Const(c) => {
                let desc = format!("const {} = {}", c.name, self.expr_to_string(&c.value));
                let id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(desc),
                        result_type: c.ty.as_ref().map(|t| t.to_string()),
                        tail_call: false,
                    }),
                    self.span_to_pp(&c.span),
                );
                default_region.add_node(id);
                self.define_var(&c.name, id);
                self.add_data_flow_edges(&c.value, id, scg);
                Ok(id)
            }
            Item::StructDef(s) => {
                // Register struct layout in struct_table
                let mut offset: u64 = 0;
                let mut layout: Vec<(String, String, u64)> = Vec::new();
                for f in &s.fields {
                    let field_type = f.ty.to_string();
                    let size = self.type_size_from_name(&field_type);
                    layout.push((f.name.clone(), field_type, offset));
                    offset += size;
                }
                self.struct_table.insert(s.name.clone(), layout);

                let fields_str: Vec<String> = s
                    .fields
                    .iter()
                    .map(|f| format!("{}: {}", f.name, f.ty))
                    .collect();
                let id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(format!("struct {} {{ {} }}", s.name, fields_str.join(", "))),
                        result_type: None,
                        tail_call: false,
                    }),
                    self.span_to_pp(&s.span),
                );
                default_region.add_node(id);
                Ok(id)
            }
            Item::EnumDef(e) => {
                let variants_str: Vec<String> = e.variants.iter().map(|v| v.name.clone()).collect();
                let id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(format!("enum {} {{ {} }}", e.name, variants_str.join(", "))),
                        result_type: None,
                        tail_call: false,
                    }),
                    self.span_to_pp(&e.span),
                );
                default_region.add_node(id);
                Ok(id)
            }
            Item::ModuleDef(m) => {
                let id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(format!("module {}", m.name)),
                        result_type: None,
                        tail_call: false,
                    }),
                    self.span_to_pp(&m.span),
                );
                default_region.add_node(id);
                Ok(id)
            }
            Item::Stmt(s) => self.convert_stmt(s, scg, default_region),
            Item::Static(s) => {
                let desc = format!("static {} = {}", s.name, self.expr_to_string(&s.value));
                let id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(desc),
                        result_type: s.ty.as_ref().map(|t| t.to_string()),
                        tail_call: false,
                    }),
                    self.span_to_pp(&s.span),
                );
                default_region.add_node(id);
                self.define_var(&s.name, id);
                self.add_data_flow_edges(&s.value, id, scg);
                Ok(id)
            }
            Item::TraitDef(t) => {
                let methods_str: Vec<String> = t
                    .required_methods
                    .iter()
                    .chain(t.provided_methods.iter())
                    .map(|m| m.name.clone())
                    .collect();
                let id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(format!("trait {} {{ {} }}", t.name, methods_str.join(", "))),
                        result_type: None,
                        tail_call: false,
                    }),
                    self.span_to_pp(&t.span),
                );
                default_region.add_node(id);
                Ok(id)
            }
            Item::ImplBlock(i) => {
                let target_str = i.target_type.to_string();
                let trait_str = i.trait_name.as_deref().unwrap_or("");
                let methods_str: Vec<String> = i.methods.iter().map(|m| m.name.clone()).collect();
                let id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(format!(
                            "impl {}{} for {} {{ {} }}",
                            trait_str,
                            if trait_str.is_empty() { "" } else { " " },
                            target_str,
                            methods_str.join(", ")
                        )),

                        result_type: None,
                        tail_call: false,
                    }),
                    self.span_to_pp(&i.span),
                );
                default_region.add_node(id);
                Ok(id)
            }
            Item::ExternBlock(eb) => {
                // Extern blocks are declarations — create a phantom node
                let id = scg.add_node(
                    NodeType::Phantom,
                    NodePayload::Phantom(PhantomNode {
                        purpose: "extern_block".to_string(),
                    }),
                    self.span_to_pp(&eb.span),
                );
                default_region.add_node(id);
                Ok(id)
            }
            // PMT (Wave 1a): LayoutDef and TransformDef are parsed but not
            // yet lowered to SCG. Wave 1c will wire them. For now, emit a
            // Computation node with a descriptive label so the SCG retains
            // a trace of the construct (parallel with how StructDef/EnumDef
            // are handled above) WITHOUT crashing or failing the build —
            // this lets the parse-only test files in
            // tests/gold_standard/pmt_wave1/ compile end-to-end and run
            // `fn main() { return 0; }` to exit 0.
            Item::LayoutDef(ld) => {
                let fields_str: Vec<String> = ld
                    .fields
                    .iter()
                    .map(|(n, t)| format!("{}: {}", n, t))
                    .collect();
                let id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(format!(
                            "layout {} {{ {} }}  // PMT Wave 1c TODO",
                            ld.name,
                            fields_str.join(", ")
                        )),
                        result_type: None,
                        tail_call: false,
                    }),
                    self.span_to_pp(&ld.span),
                );
                default_region.add_node(id);
                Ok(id)
            }
            // FIX1 (multi-param transforms): `TransformDef` is now lowered
            // to the SCG exactly like `FnDef` — synthesize an `FnDef` with
            // the same name, params, return type, and body, then run it
            // through `convert_fn_def`. This makes transforms first-class
            // functions in the semantic SCG (and thus visible to IVE
            // verification, BD inference, MSG, etc.).
            //
            // The body is `Vec<Stmt>` on `TransformDef` but `Block` on
            // `FnDef`; we wrap it in a `Block` with the transform's span.
            Item::TransformDef(td) => {
                let fn_def = FnDef {
                    visibility: Visibility::Private,
                    attrs: Vec::new(),
                    name: td.name.clone(),
                    type_params: Vec::new(),
                    params: td.params.clone(),
                    return_type: td.return_type.clone(),
                    body: Block {
                        statements: td.body.clone(),
                        span: td.span,
                    },
                    is_async: false,
                    where_clause: None,
                    span: td.span,
                };
                self.convert_fn_def(&fn_def, scg)
            }
        }
    }

    // -- 1. Function definition → region with entry/exit nodes ---------------

    fn convert_fn_def(&mut self, f: &FnDef, scg: &mut SCG) -> Result<NodeId, ParseError> {
        let region_id = self.alloc_region_id();
        let mut fn_region = SCGRegion::with_scope_level(region_id, DeploymentTarget::Heap, 1);

        self.push_scope();
        self.push_alloc_scope();

        // Enhanced: include return type in entry label for traceability.
        let ret_type_str = f
            .return_type
            .as_ref()
            .map(|t| t.to_string())
            .unwrap_or_else(|| "void".to_string());
        self.current_return_type = if ret_type_str != "void" { Some(ret_type_str.clone()) } else { None };
        let entry_label = format!("fn_{}_entry({})", f.name, ret_type_str);

        // FunctionEntry node.
        let entry_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some(entry_label),
            }),
            self.span_to_pp(&f.span),
        );
        fn_region.add_node(entry_id);

        // Parameter nodes — each receives type annotation as result_type.
        let mut param_ids = Vec::new();
        for p in &f.params {
            let param_id = scg.add_node(
                NodeType::Computation,
                NodePayload::Computation(ComputationNode {
                    kind: ComputationKind::Other(format!("param {}", p.name)),
                    result_type: p.ty.as_ref().map(|t| t.to_string()),
                    tail_call: false,
                }),
                self.span_to_pp(&p.span),
            );
            fn_region.add_node(param_id);
            self.define_var(&p.name, param_id);
            // PMT (Wave 2): if the param's type is `State<L>`, register it as
            // a state-typed var so `param.field` accesses lower to Loads with
            // the layout's field offsets.
            if let Some(ty) = &p.ty {
                if let Some(layout_name) = extract_state_layout_name(ty) {
                    self.define_var_type(&p.name, &layout_name);
                }
            }
            // Enhanced: DataFlow edge from entry to each param (represents
            // the value flowing from the caller into the parameter).
            let _ = scg.add_edge(entry_id, param_id, EdgeKind::DataFlow);
            param_ids.push(param_id);
        }

        // Link entry → params.
        let mut prev_node: Option<NodeId> = Some(entry_id);
        for &pid in &param_ids {
            if let Some(prev) = prev_node {
                let _ = scg.add_edge(prev, pid, EdgeKind::ControlFlow);
            }
            prev_node = Some(pid);
        }

        // Body statements — these become intermediate nodes between entry/exit.
        for stmt in &f.body.statements {
            let stmt_id = self.convert_stmt_in_region(stmt, scg, &mut fn_region)?;
            if let Some(prev) = prev_node {
                let _ = scg.add_edge(prev, stmt_id, EdgeKind::ControlFlow);
            }
            prev_node = Some(stmt_id);
        }

        // FunctionReturn node — enhanced with return type label.
        let ret_label = format!("fn_{}_return({})", f.name, ret_type_str);
        let ret_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: Some(ret_label),
            }),
            self.span_to_pp(&f.span),
        );
        fn_region.add_node(ret_id);

        // Link last body node to return.
        if let Some(prev) = prev_node {
            let _ = scg.add_edge(prev, ret_id, EdgeKind::ControlFlow);
        }

        self.pop_alloc_scope();
        self.pop_scope();

        scg.add_region(fn_region);

        Ok(entry_id)
    }

    // -- region definition ---------------------------------------------------

    fn convert_region_def(
        &mut self,
        r: &RegionDef,
        scg: &mut SCG,
        default_region: &mut SCGRegion,
    ) -> Result<NodeId, ParseError> {
        let region_id = self.alloc_region_id();
        let mut mem_region = SCGRegion::new(region_id, DeploymentTarget::Heap);

        let size_val = self.eval_const_int(&r.size_expr).unwrap_or(0);
        let align = self.type_alignment(None);

        // Allocation node within the region.
        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: size_val,
                align,
                region_id,
                type_name: Some(r.name.clone()),
            }),
            self.span_to_pp(&r.span),
        );
        mem_region.add_node(alloc_id);
        default_region.add_node(alloc_id);

        // Region phantom node (structural marker).
        let region_node_id = scg.add_node(
            NodeType::Phantom,
            NodePayload::Phantom(PhantomNode {
                purpose: format!("region {}", r.name),
            }),
            self.span_to_pp(&r.span),
        );
        mem_region.add_node(region_node_id);
        default_region.add_node(region_node_id);

        // Derivation edge: region node → allocation.
        let _ = scg.add_edge(region_node_id, alloc_id, EdgeKind::Derivation);

        // Record that this variable is an allocation.
        self.define_var(&r.name, alloc_id);
        self.define_alloc(&r.name, alloc_id);

        self.add_data_flow_edges(&r.size_expr, alloc_id, scg);

        scg.add_region(mem_region);

        Ok(alloc_id)
    }

    // -- statement conversion ------------------------------------------------

    fn convert_stmt(
        &mut self,
        stmt: &Stmt,
        scg: &mut SCG,
        default_region: &mut SCGRegion,
    ) -> Result<NodeId, ParseError> {
        let id = self.convert_stmt_in_region(stmt, scg, default_region)?;
        default_region.add_node(id);
        Ok(id)
    }

    fn convert_stmt_in_region(
        &mut self,
        stmt: &Stmt,
        scg: &mut SCG,
        region: &mut SCGRegion,
    ) -> Result<NodeId, ParseError> {
        match stmt {
            // 2. Let bindings → Computation nodes (enhanced: type propagation)
            Stmt::Let(l) => {
                // PMT (Wave 2): pre-process the RHS — emit Load nodes for any
                // state.field reads and rewrite them to temp Var references.
                // This must happen BEFORE stringifying the expr so the label
                // uses the temp var names (which the codegen bridge resolves
                // to the Load results).
                let value_rewritten = self.lower_state_reads_in_expr(&l.value, scg, region);

                // Enhanced: if type annotation is present, propagate size/align.
                let result_type = l.ty.as_ref().map(|t| t.to_string());
                let desc = if matches!(&value_rewritten, Expr::Uninitialized { .. }) {
                    format!("let {}", l.name)
                } else {
                    format!("let {} = {}", l.name, self.expr_to_string(&value_rewritten))
                };
                let id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(desc),
                        result_type: result_type.clone(),
                        tail_call: false,
                    }),
                    self.span_to_pp(&l.span),
                );
                region.add_node(id);
                self.define_var(&l.name, id);
                self.add_data_flow_edges(&value_rewritten, id, scg);

                // If the RHS is uninitialized → emit a computation node
                // representing the uninitialized state.
                if let Expr::Uninitialized { span } = &value_rewritten {
                    let uninit_id = scg.add_node(
                        NodeType::Computation,
                        NodePayload::Computation(ComputationNode {
                            kind: ComputationKind::Other("uninitialized".to_string()),
                            result_type: result_type.clone(),
                            tail_call: false,
                        }),
                        self.span_to_pp(span),
                    );
                    region.add_node(uninit_id);
                    let _ = scg.add_edge(uninit_id, id, EdgeKind::DataFlow);
                }

                // If the RHS is a function call — emit FunctionEntry/Return.
                if let Expr::Call { callee, args, .. } = &value_rewritten {
                    self.emit_call_nodes(callee, args, id, scg, region)?;
                }

                // If the RHS is a direct syscall — emit a Syscall node.
                if let Expr::Syscall { nr, args, span } = &value_rewritten {
                    self.emit_syscall_nodes(*nr, args, id, scg, region, span)?;
                }

                // If the RHS is an async expression → parallel region.
                if let Expr::Async { body, .. } = &value_rewritten {
                    self.emit_async_region(body, id, scg, region, &l.span)?;
                }

                // If the RHS is a spawn expression → effect node.
                if let Expr::Spawn { expr, .. } = &value_rewritten {
                    self.emit_spawn_node(expr, id, scg, region, &l.span)?;
                }

                // If the RHS is an allocate expression → allocation node
                // with enhanced size/align from type annotation.
                if let Expr::Allocate { size, .. } = &value_rewritten {
                    self.emit_alloc_from_expr(
                        size,
                        &l.name,
                        l.ty.as_ref(),
                        id,
                        scg,
                        region,
                        &l.span,
                    )?;
                }

                // PMT (Wave 2): if the RHS is `state_new(Layout)` → emit an
                // Allocation node sized to the layout's total_size. The
                // resulting buffer pointer is the state's value; subsequent
                // `state.field` reads/writes use it as the Load/Store address.
                if let Expr::StateInit { layout_name, .. } = &value_rewritten {
                    let total_size = self.layout_total_size(layout_name);
                    let align = self.type_alignment(None);
                    let alloc_id = scg.add_node(
                        NodeType::Allocation,
                        NodePayload::Allocation(AllocationNode {
                            size: total_size,
                            align,
                            region_id: region.id,
                            type_name: Some(layout_name.clone()),
                        }),
                        self.span_to_pp(&l.span),
                    );
                    region.add_node(alloc_id);
                    self.define_alloc(&l.name, alloc_id);
                    let _ = scg.add_edge(id, alloc_id, EdgeKind::Derivation);
                    // Register the var as state-typed so state.field accesses
                    // on it lower to Loads with the layout's field offsets.
                    self.define_var_type(&l.name, layout_name);
                }

                // PMT (Wave 2): if the let-binding has a `State<L>` type
                // annotation, register the var as state-typed. This covers
                // `let p: State<Point> = ...` (e.g., when p is a function
                // param passed through, or assigned from another state var).
                if let Some(ty) = &l.ty {
                    if let Some(layout_name) = extract_state_layout_name(ty) {
                        // Only register if not already registered (StateInit
                        // above already registered it).
                        if self.lookup_var_type(&l.name).is_none() {
                            self.define_var_type(&l.name, &layout_name);
                        }
                    }
                }

                // If the RHS is a cast expression → Cast node.
                if let Expr::Cast {
                    expr: inner,
                    target_type,
                    ..
                } = &value_rewritten
                {
                    let source_type = self.infer_expr_type(inner);
                    let target_type_str = target_type.to_string();
                    let is_lossless = self.is_lossless_cast(&source_type, &target_type_str);
                    let cast_id = scg.add_node(
                        NodeType::Cast,
                        NodePayload::Cast(CastNode {
                            from_type: source_type,
                            to_type: target_type_str,
                            is_lossless,
                        }),
                        self.span_to_pp(&l.span),
                    );
                    region.add_node(cast_id);
                    let _ = scg.add_edge(id, cast_id, EdgeKind::Derivation);
                    self.add_data_flow_edges(inner, cast_id, scg);
                    for var_name in &self.expr_uses(inner) {
                        if let Some(source) = self.lookup_var(var_name) {
                            let _ = scg.add_edge(source, cast_id, EdgeKind::Derivation);
                        }
                    }
                }

                Ok(id)
            }

            // 2b. Assignment → Computation + optional Write access
            Stmt::Assign(a) => {
                // PMT (Wave 2): pre-process the RHS — emit Load nodes for any
                // state.field reads and rewrite them to temp Var references.
                let value_rewritten = self.lower_state_reads_in_expr(&a.value, scg, region);

                // PMT (Wave 2): detect state-typed field write. The parser
                // represents `p.x = val` as `AssignTarget::DerefField { expr:
                // Var("p"), field: "x" }`. For nested writes like `l.a.x = v`
                // the target's `expr` is itself a FieldAccess chain.
                let state_write_info = self.extract_state_write_target(&a.target);

                let target_name = self.assign_target_name(&a.target);
                let desc = format!("{} = {}", target_name, self.expr_to_string(&value_rewritten));
                let id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(desc),
                        result_type: None,
                        tail_call: false,
                    }),
                    self.span_to_pp(&a.span),
                );
                region.add_node(id);

                // PMT (Wave 2): state-field write — emit an Access(Write) node
                // with the field's cumulative offset and size. The DataFlow
                // edges are ordered: [0] = state buffer pointer, [1] = value.
                if let Some((state_var, field_chain)) = &state_write_info {
                    if let Some(layout_name) = self.lookup_var_type(state_var).map(|s| s.to_string()) {
                        if let Some((offset, size, _field_ty)) =
                            self.resolve_field_chain(&layout_name, field_chain)
                        {
                            let access_id = scg.add_node(
                                NodeType::Access,
                                NodePayload::Access(AccessNode {
                                    mode: AccessMode::Write,
                                    region_id: region.id,
                                    offset: Some(offset),
                                    access_size: Some(size),
                                }),
                                self.span_to_pp(&a.span),
                            );
                            region.add_node(access_id);
                            let _ = scg.add_edge(id, access_id, EdgeKind::Derivation);

                            // DataFlow [0] = state buffer pointer.
                            if let Some(state_node) = self.lookup_var(state_var) {
                                let _ = scg.add_edge(state_node, access_id, EdgeKind::DataFlow);
                            }
                            // DataFlow [1] = value to store. Walk the value
                            // expr and add DataFlow edges from each source var
                            // (the first one becomes position 1).
                            self.add_data_flow_edges(&value_rewritten, access_id, scg);
                        }
                    }
                } else {
                    // Enhanced: if assigning through a dereference or index,
                    // this is also a Write (or ReadWrite if the target is read
                    // before being written, e.g., +=).
                    if matches!(
                        a.target,
                        AssignTarget::Deref { .. }
                            | AssignTarget::DerefField { .. }
                            | AssignTarget::Index { .. }
                    ) {
                        let access_size = self.infer_assign_access_size(&a.target);
                        let access_id = scg.add_node(
                            NodeType::Access,
                            NodePayload::Access(AccessNode {
                                mode: AccessMode::Write,
                                region_id: region.id,
                                offset: self.infer_assign_offset(&a.target),
                                access_size,
                            }),
                            self.span_to_pp(&a.span),
                        );
                        region.add_node(access_id);
                        let _ = scg.add_edge(id, access_id, EdgeKind::Derivation);

                        // Enhanced: Derivation from the pointer variable to the access.
                        for var_name in &self.assign_target_uses(&a.target) {
                            if let Some(source) = self.lookup_var(var_name) {
                                let _ = scg.add_edge(source, access_id, EdgeKind::Derivation);
                            }
                        }
                    }
                }

                // Add DataFlow edges BEFORE updating variable definition,
                // so that lookup_var finds the PREVIOUS definition.
                self.add_data_flow_edges(&value_rewritten, id, scg);

                // Check if the RHS is a function call.
                if let Expr::Call { callee, args, .. } = &value_rewritten {
                    self.emit_call_nodes(callee, args, id, scg, region)?;
                }

                // Check if the RHS is a direct syscall.
                if let Expr::Syscall { nr, args, span } = &value_rewritten {
                    self.emit_syscall_nodes(*nr, args, id, scg, region, span)?;
                }

                // Update variable definition for simple assignments.
                // Use update_var (not define_var) so that reassignments inside
                // if/while bodies update the original scope, not the inner scope.
                if let AssignTarget::Var { name, .. } = &a.target {
                    self.update_var(name, id);
                }

                // Pointer offset via assignment: `ptr = base + offset`
                // Create Derivation edge with offset label.
                if let Expr::BinOp {
                    op: BinOp::Add,
                    lhs,
                    rhs,
                    ..
                } = &value_rewritten
                {
                    for var_name in &self.expr_uses(lhs) {
                        if let Some(source) = self.lookup_var(var_name) {
                            let eid = scg.add_edge(source, id, EdgeKind::Derivation);
                            if let Ok(eid_val) = eid {
                                if let Some(offset_val) = self.eval_const_int(rhs) {
                                    if let Some(edge) = scg.get_edge_mut(eid_val) {
                                        edge.label = Some(format!("offset={}", offset_val));
                                    }
                                }
                            }
                        }
                    }
                    for var_name in &self.expr_uses(rhs) {
                        if let Some(source) = self.lookup_var(var_name) {
                            let _ = scg.add_edge(source, id, EdgeKind::Derivation);
                        }
                    }
                }

                // Also handle Cast in assignment value
                if let Expr::Cast {
                    expr: inner,
                    target_type,
                    ..
                } = &value_rewritten
                {
                    let source_type = self.infer_expr_type(inner);
                    let target_type_str = target_type.to_string();
                    let is_lossless = self.is_lossless_cast(&source_type, &target_type_str);
                    let cast_id = scg.add_node(
                        NodeType::Cast,
                        NodePayload::Cast(CastNode {
                            from_type: source_type,
                            to_type: target_type_str,
                            is_lossless,
                        }),
                        self.span_to_pp(&a.span),
                    );
                    region.add_node(cast_id);
                    let _ = scg.add_edge(id, cast_id, EdgeKind::Derivation);
                    self.add_data_flow_edges(inner, cast_id, scg);
                }

                // Handle Allocate in assignment value: `ptr = allocate(size)`
                // Must register in alloc_defs so that `free(ptr)` can find it.
                if let Expr::Allocate { size, .. } = &value_rewritten {
                    if let AssignTarget::Var { name, .. } = &a.target {
                        self.emit_alloc_from_expr(
                            size,
                            name,
                            None, // no type annotation on Assign
                            id,
                            scg,
                            region,
                            &a.span,
                        )?;
                    }
                }

                Ok(id)
            }

            // 3. Alloc expressions → Allocation nodes (enhanced: type-based size/align)
            Stmt::Allocate(alloc) => {
                let size_val = self.eval_const_int(&alloc.size).unwrap_or(0);
                let align = self.type_alignment(None);
                let id = scg.add_node(
                    NodeType::Allocation,
                    NodePayload::Allocation(AllocationNode {
                        size: size_val,
                        align,
                        region_id: region.id,
                        type_name: None,
                    }),
                    self.span_to_pp(&alloc.span),
                );
                region.add_node(id);
                // Register with a synthetic name based on NodeId so that
                // free() can potentially find this allocation.  The preferred
                // path is `let/var = allocate(size)` which goes through
                // Stmt::Let/Assign and properly registers via emit_alloc_from_expr.
                let synthetic_name = format!("_alloc_node_{}", id.as_u64());
                self.define_alloc(&synthetic_name, id);
                self.add_data_flow_edges(&alloc.size, id, scg);
                Ok(id)
            }

            // 4. Free expressions → Deallocation nodes (enhanced: region consistency)
            Stmt::Free(fr) => {
                let target_str = self.expr_to_string(&fr.ptr);

                // Strategy 1: Look up by the expression string (works for `let x = allocate(...)`)
                let mut alloc_node_id = self.lookup_alloc(&target_str);

                // Strategy 2: If the pointer is a simple variable, look up the
                // variable's defining NodeId and search for a connected Allocation
                // node via Derivation edges (works for `x = allocate(...)` via Assign).
                if alloc_node_id.is_none() {
                    if let Expr::Var { name, .. } = &fr.ptr {
                        // First try looking up the variable name directly in alloc_defs
                        alloc_node_id = self.lookup_alloc(name);
                        // Then try looking up the var's NodeId and trace back
                        if alloc_node_id.is_none() {
                            if let Some(var_nid) = self.lookup_var(name) {
                                if let Some(alloc_nid) =
                                    Self::find_alloc_for_var(scg, var_nid)
                                {
                                    alloc_node_id = Some(alloc_nid);
                                    // Cache for future lookups
                                    self.define_alloc(name, alloc_nid);
                                }
                            }
                        }
                    }
                }

                // Enhanced: derive region_id from the allocation if found,
                // ensuring alloc/free region consistency.
                let dealloc_region_id = if let Some(alloc_nid) = alloc_node_id {
                    if let Some(alloc_data) = scg.get_node(alloc_nid) {
                        if let NodePayload::Allocation(a) = &alloc_data.payload {
                            a.region_id
                        } else {
                            region.id
                        }
                    } else {
                        region.id
                    }
                } else {
                    region.id
                };

                // When no matching allocation is found, emit a Computation node
                // instead of a DeallocationNode with an invalid sentinel.  This
                // avoids the SCG validation error while still preserving the
                // free() semantics in the IR.
                if let Some(alloc_nid) = alloc_node_id {
                    let id = scg.add_node(
                        NodeType::Deallocation,
                        NodePayload::Deallocation(DeallocationNode {
                            allocation_node: alloc_nid,
                            region_id: dealloc_region_id,
                        }),
                        self.span_to_pp(&fr.span),
                    );
                    region.add_node(id);
                    let _ = scg.add_edge(alloc_nid, id, EdgeKind::Derivation);
                    self.add_data_flow_edges(&fr.ptr, id, scg);
                    Ok(id)
                } else {
                    // Fallback: emit as a Computation node (avoids NodeId::MAX sentinel)
                    let desc = format!("free({})", target_str);
                    let id = scg.add_node(
                        NodeType::Computation,
                        NodePayload::Computation(ComputationNode {
                            kind: ComputationKind::Other(desc),
                            result_type: None,
                            tail_call: false,
                        }),
                        self.span_to_pp(&fr.span),
                    );
                    region.add_node(id);
                    self.add_data_flow_edges(&fr.ptr, id, scg);
                    Ok(id)
                }
            }

            // 7. Read/Write → Access nodes (enhanced: field offset, access_size)
            Stmt::Access(acc) => {
                let (_target, field) = self.extract_access(&acc.expr);
                let access_size = self.infer_access_size(&acc.expr);
                // Enhanced: compute field offset for struct field access.
                let offset = field
                    .as_deref()
                    .and_then(|_| self.infer_field_offset(&acc.expr));
                let id = scg.add_node(
                    NodeType::Access,
                    NodePayload::Access(AccessNode {
                        mode: AccessMode::Read,
                        region_id: region.id,
                        offset,
                        access_size,
                    }),
                    self.span_to_pp(&acc.span),
                );
                region.add_node(id);

                // Derivation from the variable being accessed.
                let uses = self.expr_uses(&acc.expr);
                for var_name in &uses {
                    if let Some(source) = self.lookup_var(var_name) {
                        let _ = scg.add_edge(source, id, EdgeKind::Derivation);
                    }
                }

                Ok(id)
            }

            // 6. Pointer cast → Cast node (enhanced: narrowing/widening, signedness)
            Stmt::Cast(c) => {
                let source_type = self.infer_expr_type(&c.expr);
                let target_type_str = c.target_type.to_string();
                let is_lossless = self.is_lossless_cast(&source_type, &target_type_str);

                let id = scg.add_node(
                    NodeType::Cast,
                    NodePayload::Cast(CastNode {
                        from_type: source_type.clone(),
                        to_type: target_type_str.clone(),
                        is_lossless,
                    }),
                    self.span_to_pp(&c.span),
                );
                region.add_node(id);
                self.add_data_flow_edges(&c.expr, id, scg);

                let uses = self.expr_uses(&c.expr);
                for var_name in &uses {
                    if let Some(source) = self.lookup_var(var_name) {
                        let _ = scg.add_edge(source, id, EdgeKind::Derivation);
                    }
                }

                Ok(id)
            }

            // 8. If/else → Control flow with branching (enhanced: labelled edges)
            Stmt::If(if_s) => {
                let cond_str = self.expr_to_string(&if_s.condition);

                let branch_id = scg.add_node(
                    NodeType::Control,
                    NodePayload::Control(ControlNode {
                        kind: ControlKind::Branch,
                        label: Some(format!("if {}", cond_str)),
                    }),
                    self.span_to_pp(&if_s.span),
                );
                region.add_node(branch_id);
                self.add_data_flow_edges(&if_s.condition, branch_id, scg);

                // Then branch — enhanced: label the edge "then".
                self.push_scope();
                let then_ids = self.convert_block_ids(&if_s.then_block, scg, region)?;
                self.pop_scope();

                if let Some(&first_then) = then_ids.first() {
                    let eid = scg.add_edge(branch_id, first_then, EdgeKind::ControlFlow);
                    // Enhanced: label the branch edge.
                    if let Ok(id) = eid {
                        if let Some(edge) = scg.get_edge_mut(id) {
                            edge.label = Some("then".to_string());
                        }
                    }
                }

                // Else branch — enhanced: label the edge "else".
                let else_ids = if let Some(eb) = &if_s.else_block {
                    self.push_scope();
                    let ids = self.convert_block_ids(eb, scg, region)?;
                    self.pop_scope();
                    if let Some(&first_else) = ids.first() {
                        let eid = scg.add_edge(branch_id, first_else, EdgeKind::ControlFlow);
                        if let Ok(id) = eid {
                            if let Some(edge) = scg.get_edge_mut(id) {
                                edge.label = Some("else".to_string());
                            }
                        }
                    }
                    ids
                } else {
                    Vec::new()
                };

                // Join node.
                let join_id = scg.add_node(
                    NodeType::Control,
                    NodePayload::Control(ControlNode {
                        kind: ControlKind::Join,
                        label: Some("if_join".to_string()),
                    }),
                    self.span_to_pp(&if_s.span),
                );
                region.add_node(join_id);

                if let Some(&last_then) = then_ids.last() {
                    let _ = scg.add_edge(last_then, join_id, EdgeKind::ControlFlow);
                } else {
                    let _ = scg.add_edge(branch_id, join_id, EdgeKind::ControlFlow);
                }
                if let Some(&last_else) = else_ids.last() {
                    let _ = scg.add_edge(last_else, join_id, EdgeKind::ControlFlow);
                } else if else_ids.is_empty() {
                    // No else block: branch falls through directly to join.
                    let eid = scg.add_edge(branch_id, join_id, EdgeKind::ControlFlow);
                    if let Ok(id) = eid {
                        if let Some(edge) = scg.get_edge_mut(id) {
                            edge.label = Some("else_fallthrough".to_string());
                        }
                    }
                }

                Ok(branch_id)
            }

            // 9. While loop → LoopHeader/LoopExit with back edges
            //    Enhanced: condition re-evaluation tracked via DataFlow.
            Stmt::While(wh) => {
                let cond_str = self.expr_to_string(&wh.condition);

                let header_id = scg.add_node(
                    NodeType::Control,
                    NodePayload::Control(ControlNode {
                        kind: ControlKind::LoopHeader,
                        label: Some(format!("while {}", cond_str)),
                    }),
                    self.span_to_pp(&wh.span),
                );
                region.add_node(header_id);
                self.add_data_flow_edges(&wh.condition, header_id, scg);

                self.push_scope();
                let body_ids = self.convert_block_ids(&wh.body, scg, region)?;
                self.pop_scope();

                if let Some(&first_body) = body_ids.first() {
                    let _ = scg.add_edge(header_id, first_body, EdgeKind::ControlFlow);
                }

                let exit_id = scg.add_node(
                    NodeType::Control,
                    NodePayload::Control(ControlNode {
                        kind: ControlKind::LoopExit,
                        label: Some("while_exit".to_string()),
                    }),
                    self.span_to_pp(&wh.span),
                );
                region.add_node(exit_id);

                // Back edge: last body → header (enhanced: also re-add
                // condition data-flow for loop iterations).
                if let Some(&last_body) = body_ids.last() {
                    let _ = scg.add_edge(last_body, header_id, EdgeKind::ControlFlow);
                    // Enhanced: data-flow from last body to header for
                    // condition re-evaluation in loops.
                    let _ = scg.add_edge(last_body, header_id, EdgeKind::DataFlow);
                }
                // Header → exit (when condition is false).
                let _ = scg.add_edge(header_id, exit_id, EdgeKind::ControlFlow);

                Ok(header_id)
            }

            // 9b. For loop → LoopHeader/LoopExit with back edges
            //     Enhanced: iterator variable has DataFlow from header.
            Stmt::For(fr) => {
                let iter_desc = self.expr_to_string(&fr.iter);

                let header_id = scg.add_node(
                    NodeType::Control,
                    NodePayload::Control(ControlNode {
                        kind: ControlKind::LoopHeader,
                        label: Some(format!("for {} in {}", fr.name, iter_desc)),
                    }),
                    self.span_to_pp(&fr.span),
                );
                region.add_node(header_id);
                self.add_data_flow_edges(&fr.iter, header_id, scg);

                self.push_scope();
                self.define_var(&fr.name, header_id);
                let body_ids = self.convert_block_ids(&fr.body, scg, region)?;
                self.pop_scope();

                if let Some(&first_body) = body_ids.first() {
                    let _ = scg.add_edge(header_id, first_body, EdgeKind::ControlFlow);
                }

                let exit_id = scg.add_node(
                    NodeType::Control,
                    NodePayload::Control(ControlNode {
                        kind: ControlKind::LoopExit,
                        label: Some("for_exit".to_string()),
                    }),
                    self.span_to_pp(&fr.span),
                );
                region.add_node(exit_id);

                if let Some(&last_body) = body_ids.last() {
                    let _ = scg.add_edge(last_body, header_id, EdgeKind::ControlFlow);
                    // Enhanced: data-flow for loop-carried dependency.
                    let _ = scg.add_edge(last_body, header_id, EdgeKind::DataFlow);
                }
                let _ = scg.add_edge(header_id, exit_id, EdgeKind::ControlFlow);

                Ok(header_id)
            }

            // 9c. Infinite loop → LoopHeader/LoopExit
            Stmt::Loop(lo) => {
                let header_id = scg.add_node(
                    NodeType::Control,
                    NodePayload::Control(ControlNode {
                        kind: ControlKind::LoopHeader,
                        label: Some("loop".to_string()),
                    }),
                    self.span_to_pp(&lo.span),
                );
                region.add_node(header_id);

                self.push_scope();
                let body_ids = self.convert_block_ids(&lo.body, scg, region)?;
                self.pop_scope();

                if let Some(&first_body) = body_ids.first() {
                    let _ = scg.add_edge(header_id, first_body, EdgeKind::ControlFlow);
                }

                let exit_id = scg.add_node(
                    NodeType::Control,
                    NodePayload::Control(ControlNode {
                        kind: ControlKind::LoopExit,
                        label: Some("loop_exit".to_string()),
                    }),
                    self.span_to_pp(&lo.span),
                );
                region.add_node(exit_id);

                if let Some(&last_body) = body_ids.last() {
                    let _ = scg.add_edge(last_body, header_id, EdgeKind::ControlFlow);
                }
                let _ = scg.add_edge(header_id, exit_id, EdgeKind::ControlFlow);

                Ok(header_id)
            }

            // Unsafe block → Effect node marking unsafe verification + inner body
            Stmt::UnsafeBlock { body, span } => {
                // Emit an Effect node marking this as an unsafe region
                let unsafe_id = scg.add_node(
                    NodeType::Effect,
                    NodePayload::Effect(EffectNode {
                        effect_kind: "unsafe_enter".to_string(),
                        is_observable: false,
                    }),
                    self.span_to_pp(span),
                );
                region.add_node(unsafe_id);

                // Convert the inner body statements
                self.push_scope();
                let body_ids = self.convert_block_ids(body, scg, region)?;
                self.pop_scope();

                // Link unsafe_enter to first body node
                if let Some(&first_body) = body_ids.first() {
                    let _ = scg.add_edge(unsafe_id, first_body, EdgeKind::ControlFlow);
                }

                // Emit unsafe_exit effect node
                let exit_id = scg.add_node(
                    NodeType::Effect,
                    NodePayload::Effect(EffectNode {
                        effect_kind: "unsafe_exit".to_string(),
                        is_observable: false,
                    }),
                    self.span_to_pp(span),
                );
                region.add_node(exit_id);

                // Link last body node to unsafe_exit
                if let Some(&last_body) = body_ids.last() {
                    let _ = scg.add_edge(last_body, exit_id, EdgeKind::ControlFlow);
                }

                // Annotation edges from body nodes to unsafe_exit — marks them
                // as requiring unsafe verification.
                for &bid in &body_ids {
                    let _ = scg.add_edge(bid, exit_id, EdgeKind::Annotation);
                }

                Ok(unsafe_id)
            }

            // Match statement → Switch/Branch + Join decision tree
            Stmt::Match(m) => {
                let subject_str = self.expr_to_string(&m.subject);

                // Create the top-level Switch decision node.
                let switch_id = scg.add_node(
                    NodeType::Control,
                    NodePayload::Control(ControlNode {
                        kind: ControlKind::Switch,
                        label: Some(format!("match {}", subject_str)),
                    }),
                    self.span_to_pp(&m.span),
                );
                region.add_node(switch_id);
                self.add_data_flow_edges(&m.subject, switch_id, scg);

                // Join node where all arms converge.
                let join_id = scg.add_node(
                    NodeType::Control,
                    NodePayload::Control(ControlNode {
                        kind: ControlKind::Join,
                        label: Some("match_join".to_string()),
                    }),
                    self.span_to_pp(&m.span),
                );
                region.add_node(join_id);

                // For each arm, create a decision-tree subgraph.
                for (arm_idx, arm) in m.arms.iter().enumerate() {
                    let pattern_desc = self.pattern_to_string(&arm.pattern);
                    let arm_body_desc = self.expr_to_string(&arm.body);

                    // Create a SwitchCase node for this arm's pattern test.
                    let case_id = scg.add_node(
                        NodeType::Control,
                        NodePayload::Control(ControlNode {
                            kind: ControlKind::SwitchCase,
                            label: Some(format!("case {}: {}", arm_idx, pattern_desc)),
                        }),
                        self.span_to_pp(&arm.span),
                    );
                    region.add_node(case_id);

                    // Dispatch edge from switch to case.
                    let eid = scg.add_edge(switch_id, case_id, EdgeKind::Dispatch);
                    if let Ok(id) = eid {
                        if let Some(edge) = scg.get_edge_mut(id) {
                            edge.label = Some(pattern_desc.clone());
                        }
                    }

                    // If this arm has a guard, create a Branch node for it.
                    let arm_entry = if let Some(guard) = &arm.guard {
                        let guard_str = self.expr_to_string(guard);
                        let guard_branch_id = scg.add_node(
                            NodeType::Control,
                            NodePayload::Control(ControlNode {
                                kind: ControlKind::Branch,
                                label: Some(format!("guard {}", guard_str)),
                            }),
                            self.span_to_pp(&arm.span),
                        );
                        region.add_node(guard_branch_id);
                        let _ = scg.add_edge(case_id, guard_branch_id, EdgeKind::ControlFlow);
                        self.add_data_flow_edges(guard, guard_branch_id, scg);
                        guard_branch_id
                    } else {
                        case_id
                    };

                    // Computation node for the arm body.
                    // If the arm body is a Block expression, convert each
                    // statement in the block as a separate SCG node.
                    let body_id = if let Expr::Block { statements, trailing_expr, .. } = &arm.body {
                        // Convert each statement in the block
                        let mut prev_id = arm_entry;
                        for stmt in statements {
                            let sid = self.convert_stmt_in_region(stmt, scg, region)?;
                            let _ = scg.add_edge(prev_id, sid, EdgeKind::ControlFlow);
                            prev_id = sid;
                        }
                        // If there's a trailing expression, create a node for it
                        if let Some(te) = trailing_expr {
                            let te_desc = self.expr_to_string(te);
                            let te_id = scg.add_node(
                                NodeType::Computation,
                                NodePayload::Computation(ComputationNode {
                                    kind: ComputationKind::Other(format!("match_arm[{}]: {}", arm_idx, te_desc)),
                                    result_type: None,
                                    tail_call: false,
                                }),
                                self.span_to_pp(&arm.span),
                            );
                            region.add_node(te_id);
                            let _ = scg.add_edge(prev_id, te_id, EdgeKind::ControlFlow);
                            te_id
                        } else {
                            // No trailing expression — use the last statement node
                            // or create a dummy node
                            let dummy_id = scg.add_node(
                                NodeType::Computation,
                                NodePayload::Computation(ComputationNode {
                                    kind: ComputationKind::Other(format!("match_arm[{}]: block_end", arm_idx)),
                                    result_type: None,
                                    tail_call: false,
                                }),
                                self.span_to_pp(&arm.span),
                            );
                            region.add_node(dummy_id);
                            let _ = scg.add_edge(prev_id, dummy_id, EdgeKind::ControlFlow);
                            dummy_id
                        }
                    } else {
                        // Non-block arm body: single expression
                        let bid = scg.add_node(
                            NodeType::Computation,
                            NodePayload::Computation(ComputationNode {
                                kind: ComputationKind::Other(format!("match_arm[{}]: {}", arm_idx, arm_body_desc)),
                                result_type: None,
                                tail_call: false,
                            }),
                            self.span_to_pp(&arm.span),
                        );
                        region.add_node(bid);
                        let _ = scg.add_edge(arm_entry, bid, EdgeKind::ControlFlow);
                        bid
                    };

                    // Struct destructuring: create Access nodes for each field.
                    if let MatchPattern::Struct { name, fields, .. } = &arm.pattern {
                        for field in fields {
                            let field_access_id = scg.add_node(
                                NodeType::Access,
                                NodePayload::Access(AccessNode {
                                    mode: AccessMode::Read,
                                    region_id: region.id,
                                    offset: None,
                                    access_size: None,
                                }),
                                self.span_to_pp(&arm.span),
                            );
                            region.add_node(field_access_id);
                            let _ = scg.add_edge(body_id, field_access_id, EdgeKind::Derivation);
                            let field_comp_id = scg.add_node(
                                NodeType::Computation,
                                NodePayload::Computation(ComputationNode {
                                    kind: ComputationKind::Other(format!("destructure {}.{}", name, field)),
                                    result_type: None,
                                    tail_call: false,
                                }),
                                self.span_to_pp(&arm.span),
                            );
                            region.add_node(field_comp_id);
                            let _ =
                                scg.add_edge(field_access_id, field_comp_id, EdgeKind::DataFlow);
                        }
                    }

                    // Enum variant binding: create a Computation node for the binding.
                    if let MatchPattern::Enum {
                        name,
                        binding: Some(b),
                        ..
                    } = &arm.pattern
                    {
                        let bind_id = scg.add_node(
                            NodeType::Computation,
                            NodePayload::Computation(ComputationNode {
                                kind: ComputationKind::Other(format!("enum_bind {}({})", name, b)),
                                result_type: None,
                                tail_call: false,
                            }),
                            self.span_to_pp(&arm.span),
                        );
                        region.add_node(bind_id);
                        let _ = scg.add_edge(body_id, bind_id, EdgeKind::DataFlow);
                        self.define_var(b, bind_id);
                    }

                    // Range pattern: create a range-check computation.
                    if let MatchPattern::Range { start, end, .. } = &arm.pattern {
                        let range_check_id = scg.add_node(
                            NodeType::Computation,
                            NodePayload::Computation(ComputationNode {
                                kind: ComputationKind::Other(format!(
                                    "range_check {}..={}",
                                    self.lit_to_string(start),
                                    self.lit_to_string(end)
                                )),

                                result_type: Some("bool".to_string()),
                                tail_call: false,
                            }),
                            self.span_to_pp(&arm.span),
                        );
                        region.add_node(range_check_id);
                        let _ = scg.add_edge(case_id, range_check_id, EdgeKind::DataFlow);
                    }

                    // Or-pattern: create a Branch for each alternative.
                    if let MatchPattern::Or { patterns, .. } = &arm.pattern {
                        for (p_idx, sub_pat) in patterns.iter().enumerate() {
                            let sub_desc = self.pattern_to_string(sub_pat);
                            let or_branch_id = scg.add_node(
                                NodeType::Control,
                                NodePayload::Control(ControlNode {
                                    kind: ControlKind::SwitchCase,
                                    label: Some(format!("or_alt[{}]: {}", p_idx, sub_desc)),
                                }),
                                self.span_to_pp(&arm.span),
                            );
                            region.add_node(or_branch_id);
                            let _ = scg.add_edge(case_id, or_branch_id, EdgeKind::Dispatch);
                            let _ = scg.add_edge(or_branch_id, body_id, EdgeKind::ControlFlow);
                        }
                    }

                    // Arm body → join.
                    let _ = scg.add_edge(body_id, join_id, EdgeKind::ControlFlow);
                }

                Ok(switch_id)
            }

            // 12. Sync block → Synchronization edges
            //     Enhanced: sync_enter / sync_exit effect nodes bound the body.
            Stmt::Sync(sy) => {
                let sync_enter_id = scg.add_node(
                    NodeType::Effect,
                    NodePayload::Effect(EffectNode {
                        effect_kind: "sync_enter".to_string(),
                        is_observable: false,
                    }),
                    self.span_to_pp(&sy.span),
                );
                region.add_node(sync_enter_id);

                self.push_scope();
                let body_ids = self.convert_block_ids(&sy.body, scg, region)?;
                self.pop_scope();

                // Link sync_enter → first body.
                if let Some(&first_body) = body_ids.first() {
                    let _ = scg.add_edge(sync_enter_id, first_body, EdgeKind::Annotation);
                }

                let sync_exit_id = scg.add_node(
                    NodeType::Effect,
                    NodePayload::Effect(EffectNode {
                        effect_kind: "sync_exit".to_string(),
                        is_observable: false,
                    }),
                    self.span_to_pp(&sy.span),
                );
                region.add_node(sync_exit_id);

                // Link last body → sync_exit.
                if let Some(&last_body) = body_ids.last() {
                    let _ = scg.add_edge(last_body, sync_exit_id, EdgeKind::Annotation);
                }

                // Enhanced: all body nodes get Annotation edges to sync_exit
                // to enforce ordering constraint.
                for &bid in &body_ids {
                    let _ = scg.add_edge(bid, sync_exit_id, EdgeKind::Annotation);
                }

                Ok(sync_enter_id)
            }

            Stmt::Return(r) => {
                let id = scg.add_node(
                    NodeType::Control,
                    NodePayload::Control(ControlNode {
                        kind: ControlKind::FunctionReturn,
                        label: Some("return".to_string()),
                    }),
                    self.span_to_pp(&r.span),
                );
                region.add_node(id);

                if let Some(v) = &r.value {
                    // PMT (Wave 2): pre-process the return value — emit Load
                    // nodes for any state.field reads and rewrite them to temp
                    // Var references. This handles `return p.x;` etc.
                    let v_rewritten = self.lower_state_reads_in_expr(v, scg, region);

                    // For literal return values, create a lit_<n> Computation node
                    if let Expr::Lit { value, .. } = &v_rewritten {
                        let lit_str = match value {
                            crate::ast::Lit::Int(n) => format!("lit_{}", n),
                            crate::ast::Lit::Float(fl) => format!("lit_{}", fl),
                            crate::ast::Lit::String(s) => format!("lit_str_{}", s),
                            crate::ast::Lit::Bool(b) => format!("lit_{}", b),
                            crate::ast::Lit::Address(a) => format!("lit_{}", a),
                        };
                        let lit_id = scg.add_node(
                            NodeType::Computation,
                            NodePayload::Computation(ComputationNode {
                                kind: ComputationKind::Other(lit_str),
                                result_type: None,
                                tail_call: false,
                            }),
                            self.span_to_pp(&r.span),
                        );
                        region.add_node(lit_id);
                        let _ = scg.add_edge(lit_id, id, EdgeKind::DataFlow);
                        let _ = scg.add_edge(lit_id, id, EdgeKind::ControlFlow);
                        return Ok(lit_id);
                    } else if let Expr::Call { callee, args, .. } = &v_rewritten {
                        // Return value is a function call
                        let call_comp_id = scg.add_node(
                            NodeType::Computation,
                            NodePayload::Computation(ComputationNode {
                                kind: ComputationKind::Other(self.expr_to_string(&v_rewritten)),
                                result_type: None,
                                tail_call: true,
                            }),
                            self.span_to_pp(&r.span),
                        );
                        region.add_node(call_comp_id);
                        self.emit_call_nodes(callee, args, call_comp_id, scg, region)?;
                        let _ = scg.add_edge(call_comp_id, id, EdgeKind::DataFlow);
                        let _ = scg.add_edge(call_comp_id, id, EdgeKind::ControlFlow);
                        return Ok(call_comp_id);
                    } else if let Expr::Syscall { nr, args, span } = &v_rewritten {
                        // Return value is a direct syscall → Syscall node.
                        let syscall_id =
                            self.emit_syscall_nodes(*nr, args, id, scg, region, span)?;
                        return Ok(syscall_id);
                    } else {
                        // For other expressions (variables, binary ops, etc.)
                        let comp_id = scg.add_node(
                            NodeType::Computation,
                            NodePayload::Computation(ComputationNode {
                                kind: ComputationKind::Other(self.expr_to_string(&v_rewritten)),
                                result_type: self.current_return_type.clone(),
                                tail_call: false,
                            }),
                            self.span_to_pp(&r.span),
                        );
                        region.add_node(comp_id);
                        self.add_data_flow_edges(&v_rewritten, comp_id, scg);
                        let _ = scg.add_edge(comp_id, id, EdgeKind::DataFlow);
                        let _ = scg.add_edge(comp_id, id, EdgeKind::ControlFlow);
                        return Ok(comp_id);
                    }
                }

                Ok(id)
            }

            Stmt::Expr(e) => {
                // PMT (Wave 2): pre-process the expression — emit Load nodes
                // for any state.field reads and rewrite them to temp Vars.
                let expr_rewritten = self.lower_state_reads_in_expr(&e.expr, scg, region);
                let desc = self.expr_to_string(&expr_rewritten);
                let id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(desc),
                        result_type: None,
                        tail_call: false,
                    }),
                    self.span_to_pp(&e.span),
                );
                region.add_node(id);
                self.add_data_flow_edges(&expr_rewritten, id, scg);

                // 10. Function calls → FunctionEntry/FunctionReturn nodes
                if let Expr::Call { callee, args, .. } = &expr_rewritten {
                    self.emit_call_nodes(callee, args, id, scg, region)?;
                }

                // Direct syscall → Syscall node.
                if let Expr::Syscall { nr, args, span } = &expr_rewritten {
                    self.emit_syscall_nodes(*nr, args, id, scg, region, span)?;
                }

                // 5. Pointer derive/offset → Derivation edges
                //    Enhanced: label derivation edge with offset value if constant.
                if let Expr::Offset { base, offset, .. } = &expr_rewritten {
                    for var_name in &self.expr_uses(base) {
                        if let Some(source) = self.lookup_var(var_name) {
                            let eid = scg.add_edge(source, id, EdgeKind::Derivation);
                            // Enhanced: label with offset value if constant.
                            if let Ok(eid_val) = eid {
                                if let Some(offset_val) = self.eval_const_int(offset) {
                                    if let Some(edge) = scg.get_edge_mut(eid_val) {
                                        edge.label = Some(format!("offset={}", offset_val));
                                    }
                                }
                            }
                        }
                    }
                    for var_name in &self.expr_uses(offset) {
                        if let Some(source) = self.lookup_var(var_name) {
                            let _ = scg.add_edge(source, id, EdgeKind::Derivation);
                        }
                    }
                }

                // Derive expression → Derivation edges
                if let Expr::Derive {
                    ptr,
                    region: derive_region,
                    ..
                } = &expr_rewritten
                {
                    for var_name in &self.expr_uses(ptr) {
                        if let Some(source) = self.lookup_var(var_name) {
                            let _ = scg.add_edge(source, id, EdgeKind::Derivation);
                        }
                    }
                    for var_name in &self.expr_uses(derive_region) {
                        if let Some(source) = self.lookup_var(var_name) {
                            let _ = scg.add_edge(source, id, EdgeKind::Derivation);
                        }
                    }
                }

                // If the expression is a dereference, add an Access node (Read).
                if let Expr::Deref { .. } = &expr_rewritten {
                    let access_size = self.infer_access_size(&expr_rewritten);
                    let access_id = scg.add_node(
                        NodeType::Access,
                        NodePayload::Access(AccessNode {
                            mode: AccessMode::Read,
                            region_id: region.id,
                            offset: None,
                            access_size,
                        }),
                        self.span_to_pp(&e.span),
                    );
                    region.add_node(access_id);
                    let _ = scg.add_edge(id, access_id, EdgeKind::Derivation);
                }

                // If the expression is a cast, add a Cast node.
                if let Expr::Cast {
                    expr: inner,
                    target_type,
                    ..
                } = &expr_rewritten
                {
                    let source_type = self.infer_expr_type(inner);
                    let target_type_str = target_type.to_string();
                    let is_lossless = self.is_lossless_cast(&source_type, &target_type_str);
                    let cast_id = scg.add_node(
                        NodeType::Cast,
                        NodePayload::Cast(CastNode {
                            from_type: source_type,
                            to_type: target_type_str,
                            is_lossless,
                        }),
                        self.span_to_pp(&e.span),
                    );
                    region.add_node(cast_id);
                    let _ = scg.add_edge(id, cast_id, EdgeKind::Derivation);
                }

                // 11. Async block → Parallel region
                if let Expr::Async { body, .. } = &expr_rewritten {
                    self.emit_async_region(body, id, scg, region, &e.span)?;
                }

                // 11b. Spawn → Effect node
                if let Expr::Spawn { expr: inner, .. } = &expr_rewritten {
                    self.emit_spawn_node(inner, id, scg, region, &e.span)?;
                }

                // Allocate expression → Allocation node
                if let Expr::Allocate { size, .. } = &expr_rewritten {
                    let size_val = self.eval_const_int(size).unwrap_or(0);
                    let align = self.type_alignment(None);
                    let alloc_id = scg.add_node(
                        NodeType::Allocation,
                        NodePayload::Allocation(AllocationNode {
                            size: size_val,
                            align,
                            region_id: region.id,
                            type_name: None,
                        }),
                        self.span_to_pp(&e.span),
                    );
                    region.add_node(alloc_id);
                    let _ = scg.add_edge(id, alloc_id, EdgeKind::Derivation);
                }

                // sizeof/alignof → Computation nodes
                if let Expr::Sizeof { ty, .. } = &expr_rewritten {
                    let size_id = scg.add_node(
                        NodeType::Computation,
                        NodePayload::Computation(ComputationNode {
                            kind: ComputationKind::Other(format!("sizeof({})", ty)),
                            result_type: Some("usize".to_string()),
                            tail_call: false,
                        }),
                        self.span_to_pp(&e.span),
                    );
                    region.add_node(size_id);
                    let _ = scg.add_edge(id, size_id, EdgeKind::Derivation);
                }

                if let Expr::Alignof { ty, .. } = &expr_rewritten {
                    let align_id = scg.add_node(
                        NodeType::Computation,
                        NodePayload::Computation(ComputationNode {
                            kind: ComputationKind::Other(format!("alignof({})", ty)),
                            result_type: Some("usize".to_string()),
                            tail_call: false,
                        }),
                        self.span_to_pp(&e.span),
                    );
                    region.add_node(align_id);
                    let _ = scg.add_edge(id, align_id, EdgeKind::Derivation);
                }

                Ok(id)
            }

            // Compound assignment: target op= value
            Stmt::CompoundAssign(ca) => {
                let target_name = self.assign_target_name(&ca.target);
                let op_str = match ca.op {
                    CompoundOp::Add => "+=",
                    CompoundOp::Sub => "-=",
                    CompoundOp::Mul => "*=",
                    CompoundOp::Div => "/=",
                    CompoundOp::Mod => "%=",
                    CompoundOp::BitAnd => "&=",
                    CompoundOp::BitOr => "|=",
                    CompoundOp::BitXor => "^=",
                    CompoundOp::Shl => "<<=",
                    CompoundOp::Shr => ">>=",
                };
                let desc = format!(
                    "{} {} {}",
                    target_name,
                    op_str,
                    self.expr_to_string(&ca.value)
                );
                let id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(desc),
                        result_type: None,
                        tail_call: false,
                    }),
                    self.span_to_pp(&ca.span),
                );
                region.add_node(id);
                self.add_data_flow_edges(&ca.value, id, scg);
                Ok(id)
            }

            // Break
            Stmt::Break(br) => {
                let id = scg.add_node(
                    NodeType::Control,
                    NodePayload::Control(ControlNode {
                        kind: ControlKind::Jump,
                        label: Some("break".to_string()),
                    }),
                    self.span_to_pp(&br.span),
                );
                region.add_node(id);
                if let Some(v) = &br.value {
                    self.add_data_flow_edges(v, id, scg);
                }
                Ok(id)
            }

            // Continue
            Stmt::Continue(ct) => {
                let id = scg.add_node(
                    NodeType::Control,
                    NodePayload::Control(ControlNode {
                        kind: ControlKind::Jump,
                        label: Some("continue".to_string()),
                    }),
                    self.span_to_pp(&ct.span),
                );
                region.add_node(id);
                Ok(id)
            }

            // BD directive → Effect node
            Stmt::BdDirective(bd) => {
                let kind_str = match bd.kind {
                    BdDirectiveKind::Bd => "bd",
                    BdDirectiveKind::Repd => "repd",
                    BdDirectiveKind::Capd => "capd",
                    BdDirectiveKind::Reld => "reld",
                };
                let desc = if let Some(expr) = &bd.expr {
                    format!("{}({}, {})", kind_str, bd.name, self.expr_to_string(expr))
                } else {
                    format!("{}({})", kind_str, bd.name)
                };
                let id = scg.add_node(
                    NodeType::Effect,
                    NodePayload::Effect(EffectNode {
                        effect_kind: desc,
                        is_observable: false,
                    }),
                    self.span_to_pp(&bd.span),
                );
                region.add_node(id);
                if let Some(expr) = &bd.expr {
                    self.add_data_flow_edges(expr, id, scg);
                }
                Ok(id)
            }
            // PMT (Wave 1a): TransformCall is parsed-but-not-emitted in
            // Wave 1a (the parser produces Stmt::Let with a function-call
            // RHS for transform invocations). Stub arm produces no SCG
            // node so the build does not crash; Wave 1c will lower this.
            Stmt::TransformCall(tc) => {
                let id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(format!(
                            "transform_call {} = {}(...)  // PMT Wave 1c TODO",
                            tc.dst, tc.transform_name
                        )),
                        result_type: None,
                        tail_call: false,
                    }),
                    self.span_to_pp(&tc.span),
                );
                region.add_node(id);
                Ok(id)
            }
        }
    }

    // -- 10. Function calls → FunctionEntry/FunctionReturn nodes -------------
    //    Enhanced: per-argument DataFlow edges; return value DataFlow.
    //
    //    Wave 2c: channel builtins (`channel_open`, `channel_send`,
    //    `channel_recv`, `channel_close`) are parsed as regular
    //    `Expr::Call` nodes (the `channel_open<T>` type parameter is
    //    consumed by the parser but not retained on the AST — see
    //    `parse_postfix`).  They flow through this generic lowering
    //    path and produce SCG FunctionEntry/FunctionReturn ControlNodes
    //    labelled `call_channel_open`, `call_channel_send`,
    //    `call_channel_recv`, `call_channel_close` respectively.
    //    Task 2b did not add a dedicated ChannelNode variant to the
    //    parser SCG, so CallNode-shaped lowering (FunctionEntry/Return
    //    pair) is the correct SCG representation.  Wave 3 codegen will
    //    recognise these callee names and emit the appropriate runtime
    //    calls.

    fn emit_call_nodes(
        &self,
        callee: &Expr,
        args: &[Expr],
        caller_node: NodeId,
        scg: &mut SCG,
        region: &mut SCGRegion,
    ) -> Result<(), ParseError> {
        let callee_name = self.expr_to_string(callee);

        let entry_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some(format!("call_{}", callee_name)),
            }),
            ProgramPoint {
                file: None,
                line: None,
                column: None,
                offset: None,
            },
        );
        region.add_node(entry_id);

        let _ = scg.add_edge(caller_node, entry_id, EdgeKind::ControlFlow);

        // Per-argument DataFlow edges from caller variables and literals
        // to the FunctionEntry node. Each edge is labeled "argN" for
        // traceability and to enable the test_call_site_argument_data_flow
        // test to verify argument edges exist.
        for (arg_idx, arg) in args.iter().enumerate() {
            let arg_label = format!("arg{}", arg_idx);
            self.add_df_edges_recursive_labeled(arg, entry_id, scg, &arg_label);
        }

        let ret_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: Some(format!("return_{}", callee_name)),
            }),
            ProgramPoint {
                file: None,
                line: None,
                column: None,
                offset: None,
            },
        );
        region.add_node(ret_id);

        let _ = scg.add_edge(entry_id, ret_id, EdgeKind::ControlFlow);

        // Enhanced: DataFlow edge from return to the caller node,
        // representing the return value flowing back.
        let _ = scg.add_edge(ret_id, caller_node, EdgeKind::DataFlow);

        Ok(())
    }

    // -- 11c. Syscall → Syscall node ----------------------------------------
    //    Lowers a direct `syscall(nr, args...)` expression into a dedicated
    //    `NodeType::Syscall` node. Each argument's source variables are wired
    //    to the syscall via `EdgeKind::SyscallArg` edges (mirroring how
    //    `emit_call_nodes` wires per-argument `DataFlow` edges for calls).
    //    The result variable name and per-argument variable names are stored
    //    on the `SyscallNode` payload so that IVE can verify syscall safety
    //    properties (e.g. argument buffer validity).

    fn emit_syscall_nodes(
        &self,
        nr: u32,
        args: &[Expr],
        caller_node: NodeId,
        scg: &mut SCG,
        region: &mut SCGRegion,
        span: &Span,
    ) -> Result<NodeId, ParseError> {
        // Fresh result-variable name (unique per call site) and per-argument
        // variable names, stored on the SyscallNode payload.
        let caller_uid = caller_node.as_u64();
        let result_var = format!("syscall_ret_{}", caller_uid);
        let arg_var_names: Vec<String> = args
            .iter()
            .enumerate()
            .map(|(i, _)| format!("syscall_arg{}_{}", i, caller_uid))
            .collect();

        let syscall_id = scg.add_node(
            NodeType::Syscall,
            NodePayload::Syscall(SyscallNode {
                nr,
                dst: Some(result_var),
                args: arg_var_names,
            }),
            self.span_to_pp(span),
        );
        region.add_node(syscall_id);

        // ControlFlow edge from the enclosing statement node to the syscall.
        let _ = scg.add_edge(caller_node, syscall_id, EdgeKind::ControlFlow);

        // For each argument expression, wire `SyscallArg` edges from the
        // variables it uses to the syscall node. `expr_uses` walks the full
        // expression tree, so nested deref/field/index dependencies are
        // captured as well — matching the per-argument edge semantics of
        // `emit_call_nodes` but with the syscall-specific edge kind.
        for arg in args {
            for var_name in &self.expr_uses(arg) {
                if let Some(source_node) = self.lookup_var(var_name) {
                    let _ = scg.add_edge(source_node, syscall_id, EdgeKind::SyscallArg);
                }
            }
        }

        // DataFlow edge from the syscall result back to the caller node, so
        // the result is visible to subsequent uses of the binding / expr.
        let _ = scg.add_edge(syscall_id, caller_node, EdgeKind::DataFlow);

        Ok(syscall_id)
    }

    // -- Closure lowering (2b) ------------------------------------------------

    #[allow(dead_code, clippy::too_many_arguments)]
    fn emit_closure_lowering(
        &mut self,
        params: &[Param],
        body: &ClosureBody,
        capture_kind: &CaptureKind,
        parent_id: NodeId,
        scg: &mut SCG,
        region: &mut SCGRegion,
        span: &Span,
    ) -> Result<NodeId, ParseError> {
        // Capture analysis: find which enclosing variables are used in the closure body.
        let body_uses = match body {
            ClosureBody::Expr(e) => self.expr_uses(e),
            ClosureBody::Block(b) => {
                let mut uses = Vec::new();
                for stmt in &b.statements {
                    self.collect_stmt_uses(stmt, &mut uses);
                }
                uses.sort();
                uses.dedup();
                uses
            }
        };

        // Filter to only those that exist in the enclosing scope.
        let captured_vars: Vec<(String, NodeId)> = body_uses
            .iter()
            .filter_map(|name| self.lookup_var(name).map(|id| (name.clone(), id)))
            .collect();

        // Determine capture mode for each variable.
        let _is_move = matches!(capture_kind, CaptureKind::Move);
        let capture_modes: Vec<bool> = captured_vars
            .iter()
            .map(|(name, _)| {
                match capture_kind {
                    CaptureKind::Move => true,
                    CaptureKind::Ref => false,
                    CaptureKind::Auto => {
                        // Simple heuristic: if the variable is an allocation, capture by borrow;
                        // otherwise capture by move.
                        self.lookup_alloc(name).is_none()
                    }
                }
            })
            .collect();

        // Create a ClosureEnv node.
        let env_id = scg.add_node(
            NodeType::ClosureEnv,
            NodePayload::ClosureEnv(ClosureEnvNode {
                captured_vars: captured_vars.iter().map(|(n, _)| n.clone()).collect(),
                capture_modes: capture_modes.clone(),
                closure_entry: None,
            }),
            self.span_to_pp(span),
        );
        region.add_node(env_id);
        let _ = scg.add_edge(parent_id, env_id, EdgeKind::Derivation);

        // DataFlow / Derivation from each captured variable to the env.
        for (idx, (_, source_id)) in captured_vars.iter().enumerate() {
            if capture_modes[idx] {
                // Move capture: create Copy node and DataFlow edge.
                let copy_id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(format!("copy_capture[{}]", idx)),
                        result_type: None,
                        tail_call: false,
                    }),
                    self.span_to_pp(span),
                );
                region.add_node(copy_id);
                let _ = scg.add_edge(*source_id, copy_id, EdgeKind::DataFlow);
                let _ = scg.add_edge(copy_id, env_id, EdgeKind::DataFlow);
            } else {
                // Borrow capture: Derivation edge.
                let _ = scg.add_edge(*source_id, env_id, EdgeKind::Derivation);
            }
        }

        // Create ClosureEntry node.
        let entry_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::ClosureEntry,
                label: Some("closure_entry".to_string()),
            }),
            self.span_to_pp(span),
        );
        region.add_node(entry_id);
        let _ = scg.add_edge(env_id, entry_id, EdgeKind::ControlFlow);
        let _ = scg.add_edge(parent_id, entry_id, EdgeKind::ControlFlow);

        // Update env to reference closure entry.
        if let Some(env_node) = scg.get_node_mut(env_id) {
            if let NodePayload::ClosureEnv(ref mut ce) = env_node.payload {
                ce.closure_entry = Some(entry_id);
            }
        }

        // Define closure params in a new scope.
        self.push_scope();
        let mut prev_node: Option<NodeId> = Some(entry_id);
        for p in params {
            let param_id = scg.add_node(
                NodeType::Computation,
                NodePayload::Computation(ComputationNode {
                    kind: ComputationKind::Other(format!("closure_param {}", p.name)),
                    result_type: p.ty.as_ref().map(|t| t.to_string()),
                    tail_call: false,
                }),
                self.span_to_pp(&p.span),
            );
            region.add_node(param_id);
            self.define_var(&p.name, param_id);
            let _ = scg.add_edge(entry_id, param_id, EdgeKind::DataFlow);
            if let Some(prev) = prev_node {
                let _ = scg.add_edge(prev, param_id, EdgeKind::ControlFlow);
            }
            prev_node = Some(param_id);
        }

        // Convert the closure body.
        match body {
            ClosureBody::Expr(e) => {
                let body_id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(self.expr_to_string(e)),
                        result_type: None,
                        tail_call: false,
                    }),
                    self.span_to_pp(span),
                );
                region.add_node(body_id);
                self.add_data_flow_edges(e, body_id, scg);
                if let Some(prev) = prev_node {
                    let _ = scg.add_edge(prev, body_id, EdgeKind::ControlFlow);
                }
                prev_node = Some(body_id);
            }
            ClosureBody::Block(b) => {
                for stmt in &b.statements {
                    let stmt_id = self.convert_stmt_in_region(stmt, scg, region)?;
                    if let Some(prev) = prev_node {
                        let _ = scg.add_edge(prev, stmt_id, EdgeKind::ControlFlow);
                    }
                    prev_node = Some(stmt_id);
                }
            }
        }

        // Create ClosureReturn node.
        let ret_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::ClosureReturn,
                label: Some("closure_return".to_string()),
            }),
            self.span_to_pp(span),
        );
        region.add_node(ret_id);
        if let Some(prev) = prev_node {
            let _ = scg.add_edge(prev, ret_id, EdgeKind::ControlFlow);
        }

        self.pop_scope();

        Ok(entry_id)
    }

    // -- Async/await lowering (2c) -------------------------------------------

    #[allow(dead_code)]
    fn emit_async_await_lowering(
        &mut self,
        expr: &Expr,
        parent_id: NodeId,
        scg: &mut SCG,
        region: &mut SCGRegion,
        span: &Span,
    ) -> Result<NodeId, ParseError> {
        // Create a FuturePoll node for this await point.
        let poll_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FuturePoll,
                label: Some("future_poll".to_string()),
            }),
            self.span_to_pp(span),
        );
        region.add_node(poll_id);
        let _ = scg.add_edge(parent_id, poll_id, EdgeKind::ControlFlow);
        self.add_data_flow_edges(expr, poll_id, scg);

        // Create a WakerRegistration node.
        let waker_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::WakerRegistration,
                label: Some("waker_register".to_string()),
            }),
            self.span_to_pp(span),
        );
        region.add_node(waker_id);
        let _ = scg.add_edge(poll_id, waker_id, EdgeKind::ControlFlow);

        // Create a StateTransition node (the await is a split point in the state machine).
        let state_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::StateTransition,
                label: Some("await_suspend".to_string()),
            }),
            self.span_to_pp(span),
        );
        region.add_node(state_id);
        let _ = scg.add_edge(waker_id, state_id, EdgeKind::ControlFlow);

        // DataFlow edge from the awaited expression.
        self.add_data_flow_edges(expr, state_id, scg);

        // The resume point after the await.
        let resume_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FuturePoll,
                label: Some("future_resume".to_string()),
            }),
            self.span_to_pp(span),
        );
        region.add_node(resume_id);
        let _ = scg.add_edge(state_id, resume_id, EdgeKind::ControlFlow);

        Ok(poll_id)
    }

    // -- Trait dispatch lowering (2d) ----------------------------------------

    #[allow(dead_code, clippy::too_many_arguments)]
    fn emit_static_dispatch(
        &mut self,
        _callee: &Expr,
        args: &[Expr],
        target_type: &str,
        method_name: &str,
        caller_node: NodeId,
        scg: &mut SCG,
        region: &mut SCGRegion,
        span: &Span,
    ) -> Result<NodeId, ParseError> {
        // Inline the concrete impl's SCG subgraph.
        let impl_entry_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some(format!("static_dispatch {}::{}", target_type, method_name)),
            }),
            self.span_to_pp(span),
        );
        region.add_node(impl_entry_id);
        let _ = scg.add_edge(caller_node, impl_entry_id, EdgeKind::ControlFlow);

        // Per-argument DataFlow edges.
        for (i, arg) in args.iter().enumerate() {
            for var_name in &self.expr_uses(arg) {
                if let Some(source) = self.lookup_var(var_name) {
                    let eid = scg.add_edge(source, impl_entry_id, EdgeKind::DataFlow);
                    if let Ok(id) = eid {
                        if let Some(edge) = scg.get_edge_mut(id) {
                            edge.label = Some(format!("arg{}", i));
                        }
                    }
                }
            }
        }

        let impl_ret_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: Some(format!(
                    "static_dispatch_return {}::{}",
                    target_type, method_name
                )),
            }),
            self.span_to_pp(span),
        );
        region.add_node(impl_ret_id);
        let _ = scg.add_edge(impl_entry_id, impl_ret_id, EdgeKind::ControlFlow);
        let _ = scg.add_edge(impl_ret_id, caller_node, EdgeKind::DataFlow);

        Ok(impl_entry_id)
    }

    #[allow(dead_code, clippy::too_many_arguments)]
    fn emit_dynamic_dispatch(
        &mut self,
        _callee: &Expr,
        args: &[Expr],
        trait_name: &str,
        concrete_type: &str,
        method_name: &str,
        caller_node: NodeId,
        scg: &mut SCG,
        region: &mut SCGRegion,
        span: &Span,
    ) -> Result<NodeId, ParseError> {
        // Create a VTable node.
        let vtable_id = scg.add_node(
            NodeType::VTable,
            NodePayload::VTable(VTableNode {
                trait_name: trait_name.to_string(),
                concrete_type: concrete_type.to_string(),
                method_entries: Vec::new(),
            }),
            self.span_to_pp(span),
        );
        region.add_node(vtable_id);
        let _ = scg.add_edge(caller_node, vtable_id, EdgeKind::Derivation);

        // Create the dispatch call.
        let dispatch_id = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other(format!(
                    "dyn_dispatch {}::{} for {}",
                    trait_name, method_name, concrete_type
                )),

                result_type: None,
                tail_call: false,
            }),
            self.span_to_pp(span),
        );
        region.add_node(dispatch_id);
        let _ = scg.add_edge(caller_node, dispatch_id, EdgeKind::ControlFlow);

        // Dispatch edge from vtable to dispatch node.
        let _ = scg.add_edge(vtable_id, dispatch_id, EdgeKind::Dispatch);

        // Per-argument DataFlow edges.
        for (i, arg) in args.iter().enumerate() {
            for var_name in &self.expr_uses(arg) {
                if let Some(source) = self.lookup_var(var_name) {
                    let eid = scg.add_edge(source, dispatch_id, EdgeKind::DataFlow);
                    if let Ok(id) = eid {
                        if let Some(edge) = scg.get_edge_mut(id) {
                            edge.label = Some(format!("arg{}", i));
                        }
                    }
                }
            }
        }

        Ok(dispatch_id)
    }

    #[allow(dead_code, clippy::too_many_arguments)]
    fn emit_monomorphization(
        &mut self,
        _callee: &Expr,
        args: &[Expr],
        generic_fn: &str,
        concrete_type: &str,
        caller_node: NodeId,
        scg: &mut SCG,
        region: &mut SCGRegion,
        span: &Span,
    ) -> Result<NodeId, ParseError> {
        // Create a specialized copy of the generic function.
        let mono_entry_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some(format!("mono_{}_for_{}", generic_fn, concrete_type)),
            }),
            self.span_to_pp(span),
        );
        region.add_node(mono_entry_id);
        let _ = scg.add_edge(caller_node, mono_entry_id, EdgeKind::ControlFlow);

        // Per-argument DataFlow edges.
        for (i, arg) in args.iter().enumerate() {
            for var_name in &self.expr_uses(arg) {
                if let Some(source) = self.lookup_var(var_name) {
                    let eid = scg.add_edge(source, mono_entry_id, EdgeKind::DataFlow);
                    if let Ok(id) = eid {
                        if let Some(edge) = scg.get_edge_mut(id) {
                            edge.label = Some(format!("arg{}", i));
                        }
                    }
                }
            }
        }

        let mono_ret_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: Some(format!("mono_{}_for_{}_return", generic_fn, concrete_type)),
            }),
            self.span_to_pp(span),
        );
        region.add_node(mono_ret_id);
        let _ = scg.add_edge(mono_entry_id, mono_ret_id, EdgeKind::ControlFlow);
        let _ = scg.add_edge(mono_ret_id, caller_node, EdgeKind::DataFlow);

        Ok(mono_entry_id)
    }

    // -- 11. Async → Parallel region -----------------------------------------

    fn emit_async_region(
        &mut self,
        body: &Block,
        parent_id: NodeId,
        scg: &mut SCG,
        region: &mut SCGRegion,
        span: &Span,
    ) -> Result<(), ParseError> {
        let async_region_id = self.alloc_region_id();
        let mut async_region =
            SCGRegion::with_security_boundary(async_region_id, DeploymentTarget::Shared, true);
        async_region.scope_level = 1;

        let fork_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::Branch,
                label: Some("async_fork".to_string()),
            }),
            self.span_to_pp(span),
        );
        async_region.add_node(fork_id);
        region.add_node(fork_id);

        self.push_scope();
        let body_ids = self.convert_block_ids(body, scg, &mut async_region)?;
        self.pop_scope();

        if let Some(&first_body) = body_ids.first() {
            let _ = scg.add_edge(fork_id, first_body, EdgeKind::ControlFlow);
        }

        let join_id = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::Join,
                label: Some("async_join".to_string()),
            }),
            self.span_to_pp(span),
        );
        async_region.add_node(join_id);
        region.add_node(join_id);

        if let Some(&last_body) = body_ids.last() {
            let _ = scg.add_edge(last_body, join_id, EdgeKind::ControlFlow);
        }

        // Derivation: parent → fork.
        let _ = scg.add_edge(parent_id, fork_id, EdgeKind::Derivation);

        scg.add_region(async_region);

        Ok(())
    }

    // -- 11b. Spawn → Effect node -------------------------------------------
    //    Enhanced: Derivation edge from spawn to async fork for parallel
    //    fork/join boundary tracking.

    fn emit_spawn_node(
        &self,
        expr: &Expr,
        parent_id: NodeId,
        scg: &mut SCG,
        region: &mut SCGRegion,
        span: &Span,
    ) -> Result<(), ParseError> {
        let id = scg.add_node(
            NodeType::Effect,
            NodePayload::Effect(EffectNode {
                effect_kind: format!("spawn {}", self.expr_to_string(expr)),
                is_observable: true,
            }),
            self.span_to_pp(span),
        );
        region.add_node(id);
        self.add_data_flow_edges(expr, id, scg);

        // If the spawned expression is a call, emit FunctionEntry/Return.
        if let Expr::Call { callee, args, .. } = expr {
            self.emit_call_nodes(callee, args, id, scg, region)?;
        }

        let _ = scg.add_edge(parent_id, id, EdgeKind::Derivation);

        Ok(())
    }

    // -- Allocation from expression (enhanced: type-based size/align) --------

    #[allow(clippy::too_many_arguments)]
    fn emit_alloc_from_expr(
        &mut self,
        size: &Expr,
        name: &str,
        ty: Option<&Type>,
        parent_id: NodeId,
        scg: &mut SCG,
        region: &mut SCGRegion,
        span: &Span,
    ) -> Result<(), ParseError> {
        // Enhanced: if type annotation is present, use it for size/align.
        let (size_val, align) = if let Some(t) = ty {
            let computed_size = self
                .eval_const_int(size)
                .unwrap_or_else(|| self.type_size(t));
            let computed_align = self.type_alignment(Some(t));
            (computed_size, computed_align)
        } else {
            (
                self.eval_const_int(size).unwrap_or(0),
                self.type_alignment(None),
            )
        };

        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: size_val,
                align,
                region_id: region.id,
                type_name: Some(name.to_string()),
            }),
            self.span_to_pp(span),
        );
        region.add_node(alloc_id);
        self.define_alloc(name, alloc_id);
        let _ = scg.add_edge(parent_id, alloc_id, EdgeKind::Derivation);

        Ok(())
    }

    // -- block conversion helpers --------------------------------------------

    fn convert_block_ids(
        &mut self,
        block: &Block,
        scg: &mut SCG,
        region: &mut SCGRegion,
    ) -> Result<Vec<NodeId>, ParseError> {
        let mut ids = Vec::new();
        let mut prev: Option<NodeId> = None;

        for stmt in &block.statements {
            let stmt_id = self.convert_stmt_in_region(stmt, scg, region)?;
            if let Some(p) = prev {
                let _ = scg.add_edge(p, stmt_id, EdgeKind::ControlFlow);
            }
            prev = Some(stmt_id);
            ids.push(stmt_id);
        }

        Ok(ids)
    }

    // -- scope management ----------------------------------------------------

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.var_types.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.var_types.pop();
    }

    fn define_var(&mut self, name: &str, node_id: NodeId) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), node_id);
        }
    }

    /// Update an existing variable's definition in whichever scope it was
    /// originally defined. This is used for reassignments (x = 42) inside
    /// if/while bodies — the variable should be updated in its original
    /// scope, not the current (inner) scope which will be popped.
    fn update_var(&mut self, name: &str, node_id: NodeId) {
        // Search from innermost to outermost for the variable
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), node_id);
                return;
            }
        }
        // If not found in any scope, define in current scope
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), node_id);
        }
    }

    fn lookup_var(&self, name: &str) -> Option<NodeId> {
        for scope in self.scopes.iter().rev() {
            if let Some(&id) = scope.get(name) {
                return Some(id);
            }
        }
        None
    }

    fn push_alloc_scope(&mut self) {
        self.alloc_defs.push(HashMap::new());
    }

    fn pop_alloc_scope(&mut self) {
        self.alloc_defs.pop();
    }

    fn define_alloc(&mut self, name: &str, node_id: NodeId) {
        if let Some(scope) = self.alloc_defs.last_mut() {
            scope.insert(name.to_string(), node_id);
        }
    }

    fn lookup_alloc(&self, name: &str) -> Option<NodeId> {
        for scope in self.alloc_defs.iter().rev() {
            if let Some(&id) = scope.get(name) {
                return Some(id);
            }
        }
        None
    }

    /// Given a variable's defining NodeId, search the SCG for a connected
    /// Allocation node reachable via Derivation edges.  This handles the case
    /// where `x = allocate(size)` was lowered via `Stmt::Assign` which creates
    /// a ComputationNode with a Derivation edge to an AllocationNode.
    fn find_alloc_for_var(scg: &SCG, var_nid: NodeId) -> Option<NodeId> {
        // Check successors (outgoing edges from the variable's defining node)
        if let Some(succs) = scg.successors(var_nid) {
            for neighbor in succs {
                if let Some(node_data) = scg.get_node(neighbor) {
                    if matches!(node_data.payload, NodePayload::Allocation(_)) {
                        return Some(neighbor);
                    }
                }
            }
        }
        // Check predecessors (incoming edges — emit_alloc_from_expr creates:
        //   parent_id --Derivation--> alloc_id)
        if let Some(preds) = scg.predecessors(var_nid) {
            for neighbor in preds {
                if let Some(node_data) = scg.get_node(neighbor) {
                    if matches!(node_data.payload, NodePayload::Allocation(_)) {
                        return Some(neighbor);
                    }
                }
            }
        }
        None
    }

    // -- data-flow edge helpers ----------------------------------------------

    fn add_data_flow_edges(&self, expr: &Expr, target_node: NodeId, scg: &mut SCG) {
        self.add_df_edges_recursive(expr, target_node, scg);
    }

    /// Like `add_df_edges_recursive` but labels each created DataFlow edge
    /// with the given label (e.g., "arg0", "arg1"). Used for call-site
    /// argument edges so downstream consumers can identify which argument
    /// a DataFlow edge corresponds to.
    fn add_df_edges_recursive_labeled(
        &self,
        expr: &Expr,
        target_node: NodeId,
        scg: &mut SCG,
        label: &str,
    ) {
        match expr {
            Expr::BinOp { lhs, rhs, .. } => {
                self.add_df_edges_recursive_labeled(lhs, target_node, scg, label);
                self.add_df_edges_recursive_labeled(rhs, target_node, scg, label);
            }
            Expr::UnOp { expr: inner, .. } => {
                self.add_df_edges_recursive_labeled(inner, target_node, scg, label);
            }
            Expr::Var { name, .. } => {
                if let Some(source_node) = self.lookup_var(name) {
                    let eid = scg.add_edge(source_node, target_node, EdgeKind::DataFlow);
                    if let Ok(id) = eid {
                        if let Some(edge) = scg.get_edge_mut(id) {
                            edge.label = Some(label.to_string());
                        }
                    }
                }
            }
            Expr::Lit { value, .. } => {
                let lit_str = match value {
                    crate::ast::Lit::Int(n) => format!("lit_{}", n),
                    crate::ast::Lit::Float(fl) => format!("lit_{}", fl),
                    crate::ast::Lit::String(s) => format!("lit_str_{}", s),
                    crate::ast::Lit::Bool(b) => format!("lit_{}", b),
                    crate::ast::Lit::Address(a) => format!("lit_{}", a),
                };
                let lit_id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(lit_str),
                        result_type: None,
                        tail_call: false,
                    }),
                    ProgramPoint { file: None, line: None, column: None, offset: None },
                );
                let eid = scg.add_edge(lit_id, target_node, EdgeKind::DataFlow);
                if let Ok(id) = eid {
                    if let Some(edge) = scg.get_edge_mut(id) {
                        edge.label = Some(label.to_string());
                    }
                }
            }
            Expr::Cast { expr: inner, .. } => {
                self.add_df_edges_recursive_labeled(inner, target_node, scg, label);
            }
            Expr::Deref { expr: inner, .. } => {
                self.add_df_edges_recursive_labeled(inner, target_node, scg, label);
            }
            Expr::AddressOf { expr: inner, .. } => {
                self.add_df_edges_recursive_labeled(inner, target_node, scg, label);
            }
            Expr::FieldAccess { expr: inner, .. } => {
                self.add_df_edges_recursive_labeled(inner, target_node, scg, label);
            }
            Expr::Index { expr: inner, index, .. } => {
                self.add_df_edges_recursive_labeled(inner, target_node, scg, label);
                self.add_df_edges_recursive_labeled(index, target_node, scg, label);
            }
            _ => {
                for var_name in self.expr_uses(expr) {
                    if let Some(source_node) = self.lookup_var(&var_name) {
                        let eid = scg.add_edge(source_node, target_node, EdgeKind::DataFlow);
                        if let Ok(id) = eid {
                            if let Some(edge) = scg.get_edge_mut(id) {
                                edge.label = Some(label.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    fn add_df_edges_recursive(&self, expr: &Expr, target_node: NodeId, scg: &mut SCG) {
        match expr {
            Expr::BinOp { lhs, rhs, .. } => {
                self.add_df_edges_recursive(lhs, target_node, scg);
                self.add_df_edges_recursive(rhs, target_node, scg);
            }
            Expr::UnOp { expr: inner, .. } => {
                self.add_df_edges_recursive(inner, target_node, scg);
            }
            Expr::Var { name, .. } => {
                if let Some(source_node) = self.lookup_var(name) {
                    let _ = scg.add_edge(source_node, target_node, EdgeKind::DataFlow);
                }
            }
            Expr::Lit { value, .. } => {
                let lit_str = match value {
                    crate::ast::Lit::Int(n) => format!("lit_{}", n),
                    crate::ast::Lit::Float(fl) => format!("lit_{}", fl),
                    crate::ast::Lit::String(s) => format!("lit_str_{}", s),
                    crate::ast::Lit::Bool(b) => format!("lit_{}", b),
                    crate::ast::Lit::Address(a) => format!("lit_{}", a),
                };
                let lit_id = scg.add_node(
                    NodeType::Computation,
                    NodePayload::Computation(ComputationNode {
                        kind: ComputationKind::Other(lit_str),
                        result_type: None,
                        tail_call: false,
                    }),
                    ProgramPoint { file: None, line: None, column: None, offset: None },
                );
                let _ = scg.add_edge(lit_id, target_node, EdgeKind::DataFlow);
            }
            Expr::Cast { expr: inner, .. } => {
                self.add_df_edges_recursive(inner, target_node, scg);
            }
            Expr::Deref { expr: inner, .. } => {
                self.add_df_edges_recursive(inner, target_node, scg);
            }
            Expr::AddressOf { expr: inner, .. } => {
                self.add_df_edges_recursive(inner, target_node, scg);
            }
            Expr::FieldAccess { expr: inner, .. } => {
                self.add_df_edges_recursive(inner, target_node, scg);
            }
            Expr::Index { expr: inner, index, .. } => {
                self.add_df_edges_recursive(inner, target_node, scg);
                self.add_df_edges_recursive(index, target_node, scg);
            }
            Expr::AtomicLoad { addr, .. } => {
                self.add_df_edges_recursive(addr, target_node, scg);
            }
            Expr::AtomicStore { addr, value, .. } => {
                self.add_df_edges_recursive(addr, target_node, scg);
                self.add_df_edges_recursive(value, target_node, scg);
            }
            Expr::AtomicCas { addr, expected, desired, .. } => {
                self.add_df_edges_recursive(addr, target_node, scg);
                self.add_df_edges_recursive(expected, target_node, scg);
                self.add_df_edges_recursive(desired, target_node, scg);
            }
            _ => {
                for var_name in self.expr_uses(expr) {
                    if let Some(source_node) = self.lookup_var(&var_name) {
                        let _ = scg.add_edge(source_node, target_node, EdgeKind::DataFlow);
                    }
                }
            }
        }
    }

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
            Expr::NamespaceAccess { expr, .. } => {
                self.collect_uses(expr, uses);
            }
            Expr::Derive { ptr, region, .. } => {
                self.collect_uses(ptr, uses);
                self.collect_uses(region, uses);
            }
            Expr::Sizeof { .. } | Expr::Alignof { .. } => {}
            Expr::TypeAscription { expr, .. } => {
                self.collect_uses(expr, uses);
            }
            Expr::Async { body, .. } => {
                for stmt in &body.statements {
                    self.collect_stmt_uses(stmt, uses);
                }
            }
            Expr::Spawn { expr, .. } => {
                self.collect_uses(expr, uses);
            }
            Expr::Allocate { size, .. } => {
                self.collect_uses(size, uses);
            }
            Expr::Null { .. } => {}
            Expr::Range { start, end, .. } => {
                self.collect_uses(start, uses);
                self.collect_uses(end, uses);
            }
            Expr::FormatStr { .. } => {}
            Expr::Closure { .. } => {}
            Expr::Await { .. } => {}
            Expr::Uninitialized { .. } => {}
            Expr::CtSelect { .. } => {}
            Expr::CtEq { .. } => {}
            Expr::AtomicLoad { .. } => {}
            Expr::AtomicStore { .. } => {}
            Expr::AtomicCas { .. } => {}
            Expr::Block { .. } => {}
            Expr::MatchExpr { .. } => {}
            Expr::Syscall { args, .. } => {
                for a in args {
                    self.collect_uses(a, uses);
                }
            }
            // PMT (Wave 1a): the parser does not yet emit these variants
            // (StateInit intercepts `state_new(Name)` in parse_postfix;
            // StateRead reuses FieldAccess; StateWrite is reserved). The
            // arms are present so the match stays exhaustive.
            Expr::StateInit { .. } => {}
            Expr::StateRead { state, .. } => {
                self.collect_uses(state, uses);
            }
            Expr::StateWrite { state, value, .. } => {
                self.collect_uses(state, uses);
                self.collect_uses(value, uses);
            }
            // Arena State Model (Wave 1)
            Expr::ArenaNew { capacity, .. } => {
                self.collect_uses(capacity, uses);
            }
            Expr::ArenaAlloc { arena, .. } => {
                self.collect_uses(arena, uses);
            }
            Expr::ArenaGrow { arena, min_capacity, .. } => {
                self.collect_uses(arena, uses);
                self.collect_uses(min_capacity, uses);
            }
            Expr::ArenaFree { arena, .. } => {
                self.collect_uses(arena, uses);
            }
            Expr::IfExpr { condition, then_block, else_block, .. } => {
                self.collect_uses(condition, uses);
                for stmt in &then_block.statements {
                    self.collect_stmt_uses(stmt, uses);
                }
                for stmt in &else_block.statements {
                    self.collect_stmt_uses(stmt, uses);
                }
            }
        }
    }

    fn collect_stmt_uses(&self, stmt: &Stmt, uses: &mut Vec<String>) {
        match stmt {
            Stmt::Let(l) => self.collect_uses(&l.value, uses),
            Stmt::Assign(a) => {
                self.collect_assign_target_uses(&a.target, uses);
                self.collect_uses(&a.value, uses);
            }
            Stmt::Expr(e) => self.collect_uses(&e.expr, uses),
            Stmt::Return(r) => {
                if let Some(v) = &r.value {
                    self.collect_uses(v, uses);
                }
            }
            Stmt::UnsafeBlock { body, .. } => {
                for s in &body.statements {
                    self.collect_stmt_uses(s, uses);
                }
            }
            _ => {}
        }
    }

    fn collect_assign_target_uses(&self, target: &AssignTarget, uses: &mut Vec<String>) {
        match target {
            AssignTarget::Var { .. } => {}
            AssignTarget::Deref { expr, .. } => self.collect_uses(expr, uses),
            AssignTarget::DerefField { expr, .. } => self.collect_uses(expr, uses),
            AssignTarget::Index { expr, index, .. } => {
                self.collect_uses(expr, uses);
                self.collect_uses(index, uses);
            }
        }
    }

    /// Enhanced: collect variable uses from an assignment target for
    /// Derivation edges to Access nodes.
    fn assign_target_uses(&self, target: &AssignTarget) -> Vec<String> {
        let mut uses = Vec::new();
        self.collect_assign_target_uses(target, &mut uses);
        uses.sort();
        uses.dedup();
        uses
    }

    // -- type inference helpers (best-effort) --------------------------------

    fn eval_const_int(&self, expr: &Expr) -> Option<u64> {
        match expr {
            Expr::Lit {
                value: Lit::Int(i), ..
            } => Some(*i as u64),
            Expr::BinOp {
                op: BinOp::Add,
                lhs,
                rhs,
                ..
            } => self
                .eval_const_int(lhs)
                .and_then(|l| self.eval_const_int(rhs).map(|r| l + r)),
            Expr::BinOp {
                op: BinOp::Mul,
                lhs,
                rhs,
                ..
            } => self
                .eval_const_int(lhs)
                .and_then(|l| self.eval_const_int(rhs).map(|r| l * r)),
            _ => None,
        }
    }

    fn type_alignment(&self, ty: Option<&Type>) -> u64 {
        match ty {
            Some(Type::BDBase(name)) => match name.as_str() {
                "u8" | "i8" | "bool" => 1,
                "u16" | "i16" => 2,
                "u32" | "i32" | "f32" => 4,
                "u64" | "i64" | "f64" => 8,
                _ => 8,
            },
            Some(Type::Ptr(_)) | Some(Type::RegionPtr { .. }) => 8,
            Some(Type::Struct { .. }) => 8,
            Some(Type::Array { element, .. }) => self.type_alignment(Some(element)),
            Some(Type::Func { .. }) => 8,
            Some(Type::Generic { .. }) => 8,
            Some(Type::BdAnnot { .. }) => 8,
            // PMT (Wave 1a): State<T> is a typed view over a memory
            // buffer — pointer-sized handle. Ref<State, F> is an offset
            // into one. Wave 1c will compute proper layout-based sizes.
            Some(Type::State(_)) => 8,
            Some(Type::Ref { .. }) => 8,
            // Wave 1b: `Channel<T>` is a pointer-sized channel handle.
            Some(Type::Channel(_)) => 8,
            None => 8,
        }
    }

    /// Enhanced: compute size from a Type annotation.
    fn type_size(&self, ty: &Type) -> u64 {
        match ty {
            Type::BDBase(name) => self.type_size_from_name(name),
            Type::Ptr(_) | Type::RegionPtr { .. } => 8,
            Type::Array { element, size } => self.type_size(element) * (*size as u64),
            Type::Struct { fields, .. } => fields.iter().map(|(_, ft)| self.type_size(ft)).sum(),
            Type::Func { .. } => 8,
            Type::Generic { .. } => 8,
            Type::BdAnnot { .. } => 0,
            // PMT (Wave 1a): pointer-sized handles for now. Wave 1c will
            // look up the layout's actual size once layouts are tracked.
            Type::State(_) => 8,
            Type::Ref { .. } => 8,
            // Wave 1b: `Channel<T>` is a pointer-sized channel handle.
            Type::Channel(_) => 8,
        }
    }

    fn infer_expr_type(&self, expr: &Expr) -> String {
        match expr {
            Expr::Var { name, .. } => name.clone(),
            Expr::Lit { value, .. } => match value {
                Lit::Int(_) => "i64".to_string(),
                Lit::Float(_) => "f64".to_string(),
                Lit::String(_) => "str".to_string(),
                Lit::Bool(_) => "bool".to_string(),
                Lit::Address(_) => "u64".to_string(),
            },
            Expr::BinOp { op, .. } => match op {
                BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Le
                | BinOp::Gt
                | BinOp::Ge
                | BinOp::And
                | BinOp::Or => "bool".to_string(),
                _ => "i64".to_string(),
            },
            Expr::UnOp { op, .. } => match op {
                UnOp::Not => "bool".to_string(),
                UnOp::Deref => "unknown".to_string(),
                UnOp::Neg => "i64".to_string(),
                UnOp::BitNot => "i64".to_string(),
            },
            Expr::Call { callee, .. } => self.infer_expr_type(callee),
            Expr::AddressOf { .. } => "ptr".to_string(),
            Expr::Deref { .. } => "unknown".to_string(),
            Expr::Offset { .. } => "ptr".to_string(),
            Expr::Cast { target_type, .. } => target_type.to_string(),
            Expr::Index { .. } => "unknown".to_string(),
            Expr::StructInit { name, .. } => name.clone(),
            Expr::FieldAccess { field, .. } => field.clone(),
            Expr::NamespaceAccess { name, .. } => name.clone(),
            Expr::Derive { .. } => "ptr".to_string(),
            Expr::Sizeof { .. } | Expr::Alignof { .. } => "usize".to_string(),
            Expr::TypeAscription { ty, .. } => ty.to_string(),
            Expr::Async { .. } => "future".to_string(),
            Expr::Spawn { .. } => "task".to_string(),
            Expr::Allocate { .. } => "ptr".to_string(),
            Expr::Null { .. } => "null".to_string(),
            Expr::Range { .. } => "range".to_string(),
            Expr::FormatStr { .. } => "str".to_string(),
            Expr::Closure { .. } => "closure".to_string(),
            Expr::Await { .. } => "future".to_string(),
            Expr::Uninitialized { .. } => "uninitialized".to_string(),
            Expr::CtSelect { .. } => "u32".to_string(),
            Expr::CtEq { .. } => "u32".to_string(),
            Expr::AtomicLoad { .. } => "u32".to_string(),
            Expr::AtomicStore { .. } => "void".to_string(),
            Expr::AtomicCas { .. } => "u32".to_string(),
            Expr::Block { .. } => "block".to_string(),
            Expr::MatchExpr { .. } => "unknown".to_string(),
            // Linux syscalls return `isize`, which is `i64` on the 64-bit ABI.
            Expr::Syscall { .. } => "i64".to_string(),
            // PMT (Wave 1a) — best-effort type names. Wave 1c will track
            // State<T> types properly through the SCG.
            Expr::StateInit { layout_name, .. } => format!("State<{}>", layout_name),
            Expr::StateRead { .. } => "unknown".to_string(),
            Expr::StateWrite { .. } => "unknown".to_string(),
            // Arena State Model (Wave 1)
            Expr::ArenaNew { .. } => "State<Arena>".to_string(),
            Expr::ArenaAlloc { layout_name, .. } => format!("State<{}>", layout_name),
            Expr::ArenaGrow { .. } => "State<Arena>".to_string(),
            Expr::ArenaFree { .. } => "void".to_string(),
            // If-expression: type is the type of the then-branch's value
            // (best-effort — use the then-block's last statement's type).
            Expr::IfExpr { then_block, .. } => {
                if let Some(last) = then_block.statements.last() {
                    match last {
                        Stmt::Expr(e) => self.infer_expr_type(&e.expr),
                        Stmt::Let(l) => self.infer_expr_type(&l.value),
                        _ => "i64".to_string(),
                    }
                } else {
                    "i64".to_string()
                }
            }
        }
    }

    fn infer_access_size(&self, expr: &Expr) -> Option<u64> {
        match expr {
            Expr::Deref { expr: inner, .. } => {
                let inner_type = self.infer_expr_type(inner);
                Some(self.type_size_from_name(&inner_type))
            }
            _ => None,
        }
    }

    /// Enhanced: infer access size for an assignment target.
    fn infer_assign_access_size(&self, target: &AssignTarget) -> Option<u64> {
        match target {
            AssignTarget::Deref { expr, .. } => {
                let inner_type = self.infer_expr_type(expr);
                Some(self.type_size_from_name(&inner_type))
            }
            AssignTarget::Index { expr, .. } => {
                let inner_type = self.infer_expr_type(expr);
                Some(self.type_size_from_name(&inner_type))
            }
            AssignTarget::DerefField { .. } => Some(8), // best-effort
            AssignTarget::Var { .. } => None,
        }
    }

    /// Enhanced: infer byte offset for assignment target (best-effort).
    fn infer_assign_offset(&self, target: &AssignTarget) -> Option<u64> {
        match target {
            AssignTarget::Index { index, .. } => {
                // Best-effort: if index is a constant literal, compute offset.
                self.eval_const_int(index)
            }
            AssignTarget::DerefField { .. } => None, // Would need struct layout info
            _ => None,
        }
    }

    /// Enhanced: infer field offset from an access expression.
    fn infer_field_offset(&self, expr: &Expr) -> Option<u64> {
        // Try to find the field name and struct type from the expression.
        // For FieldAccess: expr.field → look up struct type of expr,
        // then find field offset.
        if let Expr::FieldAccess { expr: inner, field, .. } = expr {
            // Try to determine the struct type of the inner expression.
            // For now, we look for a variable whose name matches a struct
            // and find the field offset.
            // This is a best-effort approach — a proper implementation would
            // track types through the SCG.
            
            // Check if inner is a dereference: (*ptr).field
            if let Expr::Deref { expr: _deref_inner, .. } = inner.as_ref() {
                // (*ptr).field — look up the field in all known structs
                for layout in self.struct_table.values() {
                    for (fname, _ftype, foffset) in layout {
                        if fname == field {
                            return Some(*foffset);
                        }
                    }
                }
            }
            
            // Direct field access: var.field — look up in struct_table
            for layout in self.struct_table.values() {
                for (fname, _ftype, foffset) in layout {
                    if fname == field {
                        return Some(*foffset);
                    }
                }
            }
        }
        None
    }

    fn type_size_from_name(&self, name: &str) -> u64 {
        match name {
            "u8" | "i8" | "bool" => 1,
            "u16" | "i16" => 2,
            "u32" | "i32" | "f32" => 4,
            "u64" | "i64" | "f64" | "ptr" => 8,
            _ => 8,
        }
    }

    fn is_lossless_cast(&self, from: &str, to: &str) -> bool {
        let from_size = self.type_size_from_name(from);
        let to_size = self.type_size_from_name(to);
        to_size >= from_size
    }

    // -- stringification helpers ---------------------------------------------

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
                UnOp::BitNot => format!("(~{})", self.expr_to_string(expr)),
            },
            Expr::Call { callee, args, .. } => {
                let a: Vec<String> = args.iter().map(|e| self.expr_to_string(e)).collect();
                format!("{}({})", self.expr_to_string(callee), a.join(", "))
            }
            Expr::AddressOf { expr, .. } => format!("@{}", self.expr_to_string(expr)),
            Expr::Deref { expr, .. } => format!("*{}", self.expr_to_string(expr)),
            Expr::Offset { base, offset, .. } => {
                format!(
                    "{}+{}",
                    self.expr_to_string(base),
                    self.expr_to_string(offset)
                )
            }
            Expr::Cast {
                expr, target_type, ..
            } => {
                format!("({} as {})", self.expr_to_string(expr), target_type)
            }
            Expr::Index { expr, index, .. } => {
                format!(
                    "{}[{}]",
                    self.expr_to_string(expr),
                    self.expr_to_string(index)
                )
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
            Expr::NamespaceAccess { expr, name, .. } => {
                format!("{}::{}", self.expr_to_string(expr), name)
            }
            Expr::Derive { ptr, region, .. } => {
                format!(
                    "derive({}, {})",
                    self.expr_to_string(ptr),
                    self.expr_to_string(region)
                )
            }
            Expr::Sizeof { ty, .. } => format!("sizeof({})", ty),
            Expr::Alignof { ty, .. } => format!("alignof({})", ty),
            Expr::TypeAscription { expr, ty, .. } => {
                format!("{}: {}", self.expr_to_string(expr), ty)
            }
            Expr::Async { .. } => "async { … }".to_string(),
            Expr::Spawn { expr, .. } => format!("spawn {}", self.expr_to_string(expr)),
            Expr::Allocate { size, .. } => format!("allocate({})", self.expr_to_string(size)),
            Expr::Null { .. } => "null".to_string(),
            Expr::Range { start, end, .. } => {
                format!(
                    "{}..{}",
                    self.expr_to_string(start),
                    self.expr_to_string(end)
                )
            }
            Expr::FormatStr { .. } => "f\"…\"".to_string(),
            Expr::Closure { .. } => "|…| …".to_string(),
            Expr::Await { expr, .. } => format!("{}.await", self.expr_to_string(expr)),
            Expr::Uninitialized { .. } => "<uninitialized>".to_string(),
            Expr::CtSelect { .. } => "ct_select(…)".to_string(),
            Expr::CtEq { .. } => "ct_eq(…)".to_string(),
            Expr::AtomicLoad { .. } => "atomic_load(…)".to_string(),
            Expr::AtomicStore { .. } => "atomic_store(…)".to_string(),
            Expr::AtomicCas { .. } => "atomic_cas(…)".to_string(),
            Expr::Block { .. } => "{block}".to_string(),
            Expr::MatchExpr { .. } => "match(…)".to_string(),
            Expr::Syscall { nr, args, .. } => {
                let a: Vec<String> = args.iter().map(|e| self.expr_to_string(e)).collect();
                format!("syscall({}, {})", nr, a.join(", "))
            }
            // PMT (Wave 1a) — best-effort string forms.
            Expr::StateInit { layout_name, .. } => {
                format!("state_new({})", layout_name)
            }
            Expr::StateRead { state, field, .. } => {
                format!("{}.{}", self.expr_to_string(state), field)
            }
            Expr::StateWrite { state, field, value, .. } => {
                format!(
                    "{}.{} = {}",
                    self.expr_to_string(state),
                    field,
                    self.expr_to_string(value)
                )
            }
            // Arena State Model (Wave 1)
            Expr::ArenaNew { capacity, .. } => {
                format!("arena_new({})", self.expr_to_string(capacity))
            }
            Expr::ArenaAlloc { arena, layout_name, .. } => {
                format!("arena_alloc({}, {})", self.expr_to_string(arena), layout_name)
            }
            Expr::ArenaGrow { arena, min_capacity, .. } => {
                format!("arena_grow({}, {})", self.expr_to_string(arena), self.expr_to_string(min_capacity))
            }
            Expr::ArenaFree { arena, .. } => {
                format!("arena_free({})", self.expr_to_string(arena))
            }
            Expr::IfExpr { condition, .. } => {
                format!("if {} {{ ... }} else {{ ... }}", self.expr_to_string(condition))
            }
        }
    }

    fn bin_op_symbol(&self, op: &BinOp) -> &'static str {
        match op {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
        }
    }

    fn assign_target_name(&self, target: &AssignTarget) -> String {
        match target {
            AssignTarget::Var { name, .. } => name.clone(),
            AssignTarget::Deref { expr, .. } => format!("*{}", self.expr_to_string(expr)),
            AssignTarget::DerefField { expr, field, .. } => {
                format!("(*{}).{}", self.expr_to_string(expr), field)
            }
            AssignTarget::Index { expr, index, .. } => {
                format!(
                    "{}[{}]",
                    self.expr_to_string(expr),
                    self.expr_to_string(index)
                )
            }
        }
    }

    fn extract_access(&self, expr: &Expr) -> (String, Option<String>) {
        match expr {
            Expr::Deref { expr: inner, .. } => (self.expr_to_string(inner), None),
            Expr::FieldAccess {
                expr: inner, field, ..
            } => (self.expr_to_string(inner), Some(field.clone())),
            _ => (self.expr_to_string(expr), None),
        }
    }

    // -- pattern helpers -------------------------------------------------------

    fn pattern_to_string(&self, pattern: &MatchPattern) -> String {
        match pattern {
            MatchPattern::Wildcard(_) => "_".to_string(),
            MatchPattern::Lit { value, .. } => self.lit_to_string(value),
            MatchPattern::Ident { name, .. } => name.clone(),
            MatchPattern::Struct { name, fields, .. } => {
                format!("{} {{ {} }}", name, fields.join(", "))
            }
            MatchPattern::Enum { name, binding, .. } => {
                if let Some(b) = binding {
                    format!("{}({})", name, b)
                } else {
                    name.clone()
                }
            }
            MatchPattern::Range { start, end, .. } => {
                format!(
                    "{}..={}",
                    self.lit_to_string(start),
                    self.lit_to_string(end)
                )
            }
            MatchPattern::Or { patterns, .. } => {
                let parts: Vec<String> =
                    patterns.iter().map(|p| self.pattern_to_string(p)).collect();
                parts.join(" | ")
            }
        }
    }

    fn lit_to_string(&self, lit: &Lit) -> String {
        match lit {
            Lit::Int(i) => i.to_string(),
            Lit::Float(f) => f.to_string(),
            Lit::String(s) => format!("\"{}\"", s),
            Lit::Bool(b) => b.to_string(),
            Lit::Address(a) => format!("0x{:X}", a),
        }
    }
}

impl Default for AstToScg {
    fn default() -> Self {
        Self::new()
    }
}

/// PMT (Wave 2): local align_to helper (mirrors `vuma_bd::repd::align_to`
/// without taking a vuma-bd dependency). Rounds `val` up to the next
/// multiple of `align` (align must be a power of two).
fn align_to_local(val: u64, align: u64) -> u64 {
    if align == 0 {
        return val;
    }
    (val + align - 1) & !(align - 1)
}

/// PMT (Wave 2): if `ty` is `State<LayoutName>`, return `Some(layout_name)`.
/// Otherwise return `None`. Used to detect state-typed function parameters
/// and let-binding type annotations.
fn extract_state_layout_name(ty: &Type) -> Option<String> {
    if let Type::State(inner) = ty {
        if let Type::BDBase(name) = inner.as_ref() {
            return Some(name.clone());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;

    /// Helper: parse source and convert to SCG.
    fn parse_and_convert(source: &str) -> SCG {
        let mut parser = Parser::new(source);
        let program = parser.parse_program().unwrap();
        let mut converter = AstToScg::new();
        converter.convert(&program).expect("convert should succeed")
    }

    /// Helper: count nodes of a given type in the SCG.
    fn count_nodes_by_type(scg: &SCG, node_type: NodeType) -> usize {
        scg.nodes().filter(|n| n.node_type == node_type).count()
    }

    /// Helper: find a computation node by operation substring.
    fn find_computation_node(scg: &SCG, op_substring: &str) -> Option<vuma_scg::NodeData> {
        scg.nodes()
            .find(|n| {
                if let NodePayload::Computation(c) = &n.payload {
                    c.operation().contains(op_substring)
                } else {
                    false
                }
            })
            .cloned()
    }

    /// Helper: find a control node by kind.
    fn find_control_node(scg: &SCG, kind: ControlKind) -> Option<vuma_scg::NodeData> {
        scg.nodes()
            .find(|n| {
                if let NodePayload::Control(c) = &n.payload {
                    c.kind == kind
                } else {
                    false
                }
            })
            .cloned()
    }

    /// Helper: find all control nodes by kind.
    fn find_all_control_nodes(scg: &SCG, kind: ControlKind) -> Vec<vuma_scg::NodeData> {
        scg.nodes()
            .filter(|n| {
                if let NodePayload::Control(c) = &n.payload {
                    c.kind == kind
                } else {
                    false
                }
            })
            .cloned()
            .collect()
    }

    // ── Test 1: Function definition → entry/exit with body as intermediates ──

    #[test]
    fn test_fn_def_creates_region_with_entry_exit() {
        let scg = parse_and_convert("fn add(a: u32, b: u32) -> u32 { return a; }");

        assert!(
            scg.region_count() >= 2,
            "should have function region + default region"
        );

        let entry = find_control_node(&scg, ControlKind::FunctionEntry);
        assert!(entry.is_some(), "should have a FunctionEntry node");

        let ret = find_control_node(&scg, ControlKind::FunctionReturn);
        assert!(ret.is_some(), "should have a FunctionReturn node");

        // Enhanced: entry label should contain return type.
        if let Some(e) = &entry {
            if let NodePayload::Control(c) = &e.payload {
                assert!(
                    c.label.as_ref().map_or(false, |l| l.contains("u32")),
                    "entry label should include return type"
                );
            }
        }
    }

    // ── Test 2: Let/assign → Computation nodes with type propagation ──────

    #[test]
    fn test_let_binding_creates_computation_node() {
        let scg = parse_and_convert("let x = 42;");

        let comp = find_computation_node(&scg, "let x");
        assert!(comp.is_some(), "should have a computation node for let x");

        if let Some(node) = comp {
            if let NodePayload::Computation(c) = &node.payload {
                assert!(c.operation().contains("let x = 42"));
            } else {
                panic!("expected Computation payload");
            }
        }
    }

    // ── Test 3: Allocation with size/alignment from type ──────────────────

    #[test]
    fn test_region_creates_allocation_node() {
        let scg = parse_and_convert("region pool = allocate(1024);");

        let alloc_count = count_nodes_by_type(&scg, NodeType::Allocation);
        assert!(alloc_count >= 1, "should have at least one Allocation node");

        let alloc_node = scg
            .nodes()
            .find(|n| matches!(&n.payload, NodePayload::Allocation(a) if a.size == 1024));
        assert!(
            alloc_node.is_some(),
            "should find allocation with size 1024"
        );

        let has_derivation = scg.edges().any(|e| e.kind == EdgeKind::Derivation);
        assert!(has_derivation, "should have Derivation edge");
    }

    // ── Test 4: Free → Deallocation referencing allocation ────────────────

    #[test]
    fn test_free_creates_deallocation_referencing_alloc() {
        let scg = parse_and_convert("region pool = allocate(256); free(pool);");

        let dealloc_count = count_nodes_by_type(&scg, NodeType::Deallocation);
        assert!(dealloc_count >= 1, "should have a Deallocation node");

        let alloc_node = scg
            .nodes()
            .find(|n| matches!(&n.payload, NodePayload::Allocation(_)));
        let dealloc_node = scg
            .nodes()
            .find(|n| matches!(&n.payload, NodePayload::Deallocation(_)));

        assert!(alloc_node.is_some());
        assert!(dealloc_node.is_some());

        if let (Some(alloc), Some(dealloc)) = (alloc_node, dealloc_node) {
            if let NodePayload::Deallocation(d) = &dealloc.payload {
                assert_eq!(
                    d.allocation_node, alloc.id,
                    "deallocation should reference the allocation node"
                );
            }
        }

        let derivation_edges: Vec<_> = scg
            .edges()
            .filter(|e| e.kind == EdgeKind::Derivation)
            .collect();
        assert!(!derivation_edges.is_empty(), "should have Derivation edges");
    }

    // ── Test 5: Pointer offset → Derivation edges (enhanced: labelled) ───

    #[test]
    fn test_pointer_offset_creates_derivation_edge() {
        let scg = parse_and_convert("region pool = allocate(1024); ptr = pool + 64;");

        let derivation_edges: Vec<_> = scg
            .edges()
            .filter(|e| e.kind == EdgeKind::Derivation)
            .collect();
        assert!(
            !derivation_edges.is_empty(),
            "pointer offset should create Derivation edges"
        );

        // Enhanced: check that the derivation edge has an offset label.
        let labelled = derivation_edges
            .iter()
            .any(|e| e.label.as_ref().map_or(false, |l| l.contains("offset=64")));
        assert!(
            labelled,
            "derivation edge should be labelled with offset=64"
        );
    }

    // ── Test 6: Cast → Cast node with source/target BD ───────────────────

    #[test]
    fn test_cast_creates_cast_node() {
        let scg = parse_and_convert("let x = 42; let y = x as u64;");

        let cast_count = count_nodes_by_type(&scg, NodeType::Cast);
        assert!(cast_count >= 1, "should have at least one Cast node");

        let cast_node = scg
            .nodes()
            .find(|n| matches!(&n.payload, NodePayload::Cast(_)));
        assert!(cast_node.is_some());

        if let Some(node) = cast_node {
            if let NodePayload::Cast(c) = &node.payload {
                assert_eq!(c.to_type, "u64");
            }
        }
    }

    // ── Test 7: Read/Write → Access nodes ────────────────────────────────

    #[test]
    fn test_access_creates_access_node() {
        let scg = parse_and_convert("region pool = allocate(1024); *pool;");

        let access_count = count_nodes_by_type(&scg, NodeType::Access);
        assert!(access_count >= 1, "dereference should create Access node");

        let access_node = scg
            .nodes()
            .find(|n| matches!(&n.payload, NodePayload::Access(a) if a.mode == AccessMode::Read));
        assert!(access_node.is_some(), "should have Read Access node");
    }

    // ── Test 8: If/else → Control flow with branching (enhanced: labels) ─

    #[test]
    fn test_if_else_creates_branch_and_join() {
        let scg = parse_and_convert("let x = 1; if x { let y = 2; } else { let z = 3; }");

        let branch = find_control_node(&scg, ControlKind::Branch);
        assert!(branch.is_some(), "should have Branch control node");

        let join = find_control_node(&scg, ControlKind::Join);
        assert!(join.is_some(), "should have Join control node");

        let cf_edges: Vec<_> = scg
            .edges()
            .filter(|e| e.kind == EdgeKind::ControlFlow)
            .collect();
        assert!(
            cf_edges.len() >= 3,
            "if/else should have multiple ControlFlow edges"
        );

        // Enhanced: check for labelled branch edges.
        let then_labelled = cf_edges
            .iter()
            .any(|e| e.label.as_ref().map_or(false, |l| l == "then"));
        let else_labelled = cf_edges
            .iter()
            .any(|e| e.label.as_ref().map_or(false, |l| l == "else"));
        assert!(
            then_labelled,
            "should have 'then' labelled ControlFlow edge"
        );
        assert!(
            else_labelled,
            "should have 'else' labelled ControlFlow edge"
        );
    }

    // ── Test 9: While loop → back edges (enhanced: condition DataFlow) ────

    #[test]
    fn test_while_creates_loop_with_back_edges() {
        let scg = parse_and_convert("let x = 0; while x { let y = 1; }");

        let header = find_control_node(&scg, ControlKind::LoopHeader);
        assert!(header.is_some(), "should have LoopHeader node");

        let exit = find_control_node(&scg, ControlKind::LoopExit);
        assert!(exit.is_some(), "should have LoopExit node");

        if let (Some(h), Some(e)) = (&header, &exit) {
            let has_header_to_exit = scg.edges().any(|edge| {
                edge.source == h.id && edge.target == e.id && edge.kind == EdgeKind::ControlFlow
            });
            assert!(has_header_to_exit, "should have header→exit edge");
        }

        // Enhanced: check for data-flow back edge from body to header.
        if let Some(h) = &header {
            let df_back = scg
                .edges()
                .any(|e| e.target == h.id && e.kind == EdgeKind::DataFlow);
            assert!(df_back, "should have DataFlow back edge to LoopHeader");
        }
    }

    // ── Test 10: Function calls → FunctionEntry/Return ───────────────────

    #[test]
    fn test_function_call_creates_entry_return() {
        let scg = parse_and_convert("fn foo(a: u32) -> u32 { return a; } foo(42);");

        let entries = find_all_control_nodes(&scg, ControlKind::FunctionEntry);
        assert!(
            entries.len() >= 2,
            "should have FunctionEntry nodes from fn def and call site"
        );
    }

    // ── Test 11: Async → parallel region ─────────────────────────────────

    #[test]
    fn test_async_creates_parallel_region() {
        let scg = parse_and_convert("async { let x = 1; };");

        let secure_regions = scg.regions().filter(|r| r.security_boundary).count();
        assert!(
            secure_regions >= 1,
            "async should create security-boundary region"
        );

        let shared_regions = scg
            .regions()
            .filter(|r| r.deployment_target == DeploymentTarget::Shared)
            .count();
        assert!(shared_regions >= 1, "async should create Shared region");
    }

    // ── Test 12: Spawn → Effect node ─────────────────────────────────────

    #[test]
    fn test_spawn_creates_effect_node() {
        let scg = parse_and_convert("spawn foo();");

        let effect_node = scg.nodes().find(|n| {
            if let NodePayload::Effect(e) = &n.payload {
                e.effect_kind.contains("spawn")
            } else {
                false
            }
        });
        assert!(effect_node.is_some(), "spawn should create Effect node");

        if let Some(node) = effect_node {
            if let NodePayload::Effect(e) = &node.payload {
                assert!(e.is_observable, "spawn should be observable");
            }
        }
    }

    // ── Test 13: Sync → synchronization edges (enhanced: enter/exit) ─────

    #[test]
    fn test_sync_creates_sync_edges() {
        let scg = parse_and_convert("sync { let x = 1; }");

        let annotation_edges: Vec<_> = scg
            .edges()
            .filter(|e| e.kind == EdgeKind::Annotation)
            .collect();
        assert!(
            !annotation_edges.is_empty(),
            "sync should create Annotation edges"
        );

        // Enhanced: check for sync_enter and sync_exit effect nodes.
        let sync_enter = scg.nodes().find(|n| {
            if let NodePayload::Effect(e) = &n.payload {
                e.effect_kind == "sync_enter"
            } else {
                false
            }
        });
        let sync_exit = scg.nodes().find(|n| {
            if let NodePayload::Effect(e) = &n.payload {
                e.effect_kind == "sync_exit"
            } else {
                false
            }
        });
        assert!(sync_enter.is_some(), "should have sync_enter effect node");
        assert!(sync_exit.is_some(), "should have sync_exit effect node");
    }

    // ── Test 14: Complex program ─────────────────────────────────────────

    #[test]
    fn test_complex_program_structure() {
        let source = r#"
            region memory_pool = allocate(4096);
            fn process(data: u32) -> u32 {
                let x = data + 1;
                return x;
            }
            let result = process(42);
            free(memory_pool);
        "#;
        let scg = parse_and_convert(source);

        assert!(
            count_nodes_by_type(&scg, NodeType::Allocation) >= 1,
            "should have Allocation nodes"
        );
        assert!(
            count_nodes_by_type(&scg, NodeType::Deallocation) >= 1,
            "should have Deallocation nodes"
        );
        assert!(
            count_nodes_by_type(&scg, NodeType::Control) >= 2,
            "should have Control nodes (fn entry/return)"
        );
        assert!(
            count_nodes_by_type(&scg, NodeType::Computation) >= 2,
            "should have Computation nodes"
        );

        assert!(
            scg.region_count() >= 2,
            "should have function + default regions"
        );

        let derivation_count = scg
            .edges()
            .filter(|e| e.kind == EdgeKind::Derivation)
            .count();
        assert!(derivation_count >= 1, "should have Derivation edges");

        let data_flow_count = scg.edges().filter(|e| e.kind == EdgeKind::DataFlow).count();
        assert!(data_flow_count >= 1, "should have DataFlow edges");
    }

    // ── Test 15: Data-flow dependency tracking ────────────────────────────

    #[test]
    fn test_data_flow_edges_track_dependencies() {
        let scg = parse_and_convert("let x = 10; let y = x + 5;");

        let x_node = find_computation_node(&scg, "let x");
        let y_node = find_computation_node(&scg, "let y");

        assert!(x_node.is_some(), "should have node for let x");
        assert!(y_node.is_some(), "should have node for let y");

        if let (Some(xn), Some(yn)) = (x_node, y_node) {
            let has_data_flow = scg
                .edges()
                .any(|e| e.source == xn.id && e.target == yn.id && e.kind == EdgeKind::DataFlow);
            assert!(
                has_data_flow,
                "should have DataFlow edge from x to y definition"
            );
        }
    }

    // ── Test 16: Example program from docs ────────────────────────────────

    #[test]
    fn test_example_program_from_docs() {
        let source = r#"
            region memory_pool = allocate(1024);
            fn main() {
                node_ptr = memory_pool + 64;
                header = node_ptr as *NodeHeader;
            }
        "#;
        let scg = parse_and_convert(source);

        assert!(scg.node_count() >= 4, "should have multiple nodes");
        assert!(
            scg.region_count() >= 2,
            "should have function + default regions"
        );

        let derivation_count = scg
            .edges()
            .filter(|e| e.kind == EdgeKind::Derivation)
            .count();
        assert!(
            derivation_count >= 1,
            "should have Derivation edges from allocation"
        );
    }

    // ── Test 17: For loop → LoopHeader/LoopExit with back edges ──────────

    #[test]
    fn test_for_loop_creates_loop_nodes() {
        let scg = parse_and_convert("for i in 0..10 { let x = i; }");

        let header = find_control_node(&scg, ControlKind::LoopHeader);
        assert!(header.is_some(), "for should create LoopHeader node");

        let exit = find_control_node(&scg, ControlKind::LoopExit);
        assert!(exit.is_some(), "for should create LoopExit node");
    }

    // ── Test 18: Write access through dereference assignment ──────────────

    #[test]
    fn test_deref_assign_creates_write_access() {
        let scg = parse_and_convert("region pool = allocate(64); *pool = 42;");

        let write_access = scg.nodes().find(|n| {
            if let NodePayload::Access(a) = &n.payload {
                a.mode == AccessMode::Write
            } else {
                false
            }
        });
        assert!(
            write_access.is_some(),
            "dereference assignment should create Write Access node"
        );
    }

    // ── Test 19: Cast node lossless property ─────────────────────────────

    #[test]
    fn test_cast_node_lossless_property() {
        let scg = parse_and_convert("let x = 42; let y = x as u64;");

        let cast_node = scg.nodes().find(|n| {
            matches!(&n.payload, NodePayload::Cast(c) if c.from_type == "i64" && c.to_type == "u64")
        });

        if let Some(node) = cast_node {
            if let NodePayload::Cast(c) = &node.payload {
                assert!(c.is_lossless, "i64→u64 should be lossless (same size)");
            }
        }
    }

    // ── Test 20: SCG validation on output ────────────────────────────────

    #[test]
    fn test_scg_validation_on_output() {
        let source = r#"
            region pool = allocate(256);
            fn compute(x: u32) -> u32 {
                let y = x + 1;
                return y;
            }
            let result = compute(10);
            free(pool);
        "#;
        let scg = parse_and_convert(source);

        let validation = scg.validate();
        assert!(
            validation.is_valid,
            "SCG should validate: errors = {:?}",
            validation.errors
        );
    }

    // ═══════════════════════════════════════════════════════════════════════
    // ENHANCED TESTS: 11 new tests covering deeper SCG semantics
    // ═══════════════════════════════════════════════════════════════════════

    // ── Test 21: Fn entry label includes return type ──────────────────────

    #[test]
    fn test_fn_entry_label_includes_return_type() {
        let scg = parse_and_convert("fn get_value() -> u64 { return 42; }");

        let entry = find_control_node(&scg, ControlKind::FunctionEntry);
        assert!(entry.is_some());
        if let Some(e) = entry {
            if let NodePayload::Control(c) = &e.payload {
                let label = c.label.as_ref().unwrap();
                assert!(
                    label.contains("u64"),
                    "entry label should contain return type 'u64', got: {}",
                    label
                );
            }
        }
    }

    // ── Test 22: Fn body nodes are intermediates between entry/exit ───────

    #[test]
    fn test_fn_body_nodes_are_intermediate_between_entry_exit() {
        let scg = parse_and_convert("fn f(x: u32) -> u32 { let y = x; return y; }");

        let entry = find_control_node(&scg, ControlKind::FunctionEntry);
        let ret = find_control_node(&scg, ControlKind::FunctionReturn);
        assert!(entry.is_some() && ret.is_some());

        if let (Some(e), Some(r)) = (&entry, &ret) {
            // There should be a path from entry to return via body nodes.
            let has_path = scg.find_path(e.id, r.id);
            assert_eq!(
                has_path,
                Some(true),
                "there should be a path from FunctionEntry to FunctionReturn"
            );
        }
    }

    // ── Test 23: Call site creates per-argument DataFlow edges ────────────

    #[test]
    fn test_call_site_argument_data_flow() {
        let scg = parse_and_convert(
            "fn add(a: u32, b: u32) -> u32 { return a; } let x = 1; let y = 2; add(x, y);",
        );

        // Find the call FunctionEntry node.
        let call_entry = scg.nodes().find(|n| {
            if let NodePayload::Control(c) = &n.payload {
                c.kind == ControlKind::FunctionEntry
                    && c.label
                        .as_ref()
                        .map_or(false, |l| l.starts_with("call_add"))
            } else {
                false
            }
        });
        assert!(
            call_entry.is_some(),
            "should have call_add FunctionEntry node"
        );

        // Check that there are DataFlow edges labelled arg0/arg1.
        let arg_edges: Vec<_> = scg
            .edges()
            .filter(|e| {
                e.kind == EdgeKind::DataFlow
                    && e.label.as_ref().map_or(false, |l| l.starts_with("arg"))
            })
            .collect();
        assert!(
            arg_edges.len() >= 2,
            "should have at least 2 argument DataFlow edges, got {}",
            arg_edges.len()
        );
    }

    // ── Test 24: For loop has DataFlow back edge ─────────────────────────

    #[test]
    fn test_for_loop_data_flow_back_edge() {
        let scg = parse_and_convert("for i in 0..10 { let x = i; }");

        let header = find_control_node(&scg, ControlKind::LoopHeader);
        assert!(header.is_some());

        // Enhanced: for loop should have a DataFlow back edge to the header.
        if let Some(h) = &header {
            let df_back = scg
                .edges()
                .any(|e| e.target == h.id && e.kind == EdgeKind::DataFlow);
            assert!(
                df_back,
                "for loop should have DataFlow back edge to LoopHeader"
            );
        }
    }

    // ── Test 25: Narrowing cast is not lossless ──────────────────────────

    #[test]
    fn test_narrowing_cast_is_not_lossless() {
        let scg = parse_and_convert("let x = 42; let y = x as u8;");

        let cast_node = scg
            .nodes()
            .find(|n| matches!(&n.payload, NodePayload::Cast(c) if c.to_type == "u8"));
        assert!(cast_node.is_some(), "should have Cast node targeting u8");

        if let Some(node) = cast_node {
            if let NodePayload::Cast(c) = &node.payload {
                assert!(
                    !c.is_lossless,
                    "i64→u8 is a narrowing cast and should NOT be lossless"
                );
            }
        }
    }

    // ── Test 26: Sync block creates enter/exit effect nodes ──────────────

    #[test]
    fn test_sync_block_creates_enter_exit_effects() {
        let scg = parse_and_convert("sync { let x = 1; let y = 2; }");

        let sync_enter = scg.nodes().find(|n| {
            if let NodePayload::Effect(e) = &n.payload {
                e.effect_kind == "sync_enter"
            } else {
                false
            }
        });
        let sync_exit = scg.nodes().find(|n| {
            if let NodePayload::Effect(e) = &n.payload {
                e.effect_kind == "sync_exit"
            } else {
                false
            }
        });

        assert!(sync_enter.is_some(), "should have sync_enter effect");
        assert!(sync_exit.is_some(), "should have sync_exit effect");

        // Verify that sync_enter has Annotation edge to body and body has
        // Annotation edge to sync_exit.
        if let (Some(enter), Some(exit)) = (&sync_enter, &sync_exit) {
            let enter_to_body = scg
                .edges()
                .any(|e| e.source == enter.id && e.kind == EdgeKind::Annotation);
            assert!(
                enter_to_body,
                "sync_enter should have Annotation edge to body"
            );

            let body_to_exit = scg
                .edges()
                .any(|e| e.target == exit.id && e.kind == EdgeKind::Annotation);
            assert!(
                body_to_exit,
                "body should have Annotation edge to sync_exit"
            );
        }
    }

    // ── Test 27: If without else has fallthrough edge ────────────────────

    #[test]
    fn test_if_without_else_has_fallthrough() {
        let scg = parse_and_convert("let x = 1; if x { let y = 2; }");

        // An if without else should have an "else_fallthrough" labelled edge
        // from the branch directly to the join.
        let fallthrough = scg.edges().any(|e| {
            e.kind == EdgeKind::ControlFlow
                && e.label.as_ref().map_or(false, |l| l == "else_fallthrough")
        });
        assert!(
            fallthrough,
            "if without else should have else_fallthrough ControlFlow edge"
        );
    }

    // ── Test 28: Write access has Derivation from pointer variable ────────

    #[test]
    fn test_write_access_has_derivation_from_pointer() {
        let scg = parse_and_convert("region pool = allocate(64); *pool = 42;");

        let write_access = scg.nodes().find(|n| {
            if let NodePayload::Access(a) = &n.payload {
                a.mode == AccessMode::Write
            } else {
                false
            }
        });
        assert!(write_access.is_some());

        let alloc_node = scg
            .nodes()
            .find(|n| matches!(&n.payload, NodePayload::Allocation(_)));
        assert!(alloc_node.is_some());

        // Enhanced: there should be a Derivation edge from the alloc node
        // (which is what 'pool' resolves to) to the write Access node.
        if let (Some(alloc), Some(access)) = (&alloc_node, &write_access) {
            let has_derivation = scg.edges().any(|e| {
                e.source == alloc.id && e.target == access.id && e.kind == EdgeKind::Derivation
            });
            assert!(
                has_derivation,
                "should have Derivation edge from allocation to Write Access node"
            );
        }
    }

    // ── Test 29: Complex snippet with alloc/free/call/if/while ───────────

    #[test]
    fn test_complex_snippet_alloc_free_call_if_while() {
        let source = r#"
            region heap = allocate(4096);
            fn init(buf: u32) -> u32 {
                let x = buf + 1;
                if x {
                    let y = x;
                }
                return x;
            }
            let r = init(0);
            while r {
                let z = r;
            }
            free(heap);
        "#;
        let scg = parse_and_convert(source);

        // Verify all node types present.
        assert!(count_nodes_by_type(&scg, NodeType::Allocation) >= 1);
        assert!(count_nodes_by_type(&scg, NodeType::Deallocation) >= 1);
        assert!(
            count_nodes_by_type(&scg, NodeType::Control) >= 4,
            "should have fn entry, fn return, branch, loop header, loop exit, join"
        );
        assert!(count_nodes_by_type(&scg, NodeType::Computation) >= 3);

        // Verify regions: default + fn region.
        assert!(scg.region_count() >= 2);

        // Verify derivation chain: alloc → free.
        let alloc_node = scg
            .nodes()
            .find(|n| matches!(&n.payload, NodePayload::Allocation(_)));
        let dealloc_node = scg
            .nodes()
            .find(|n| matches!(&n.payload, NodePayload::Deallocation(_)));
        if let (Some(a), Some(d)) = (&alloc_node, &dealloc_node) {
            let has_derivation = scg
                .edges()
                .any(|e| e.source == a.id && e.target == d.id && e.kind == EdgeKind::Derivation);
            assert!(
                has_derivation,
                "alloc → dealloc should have Derivation edge"
            );
        }

        // Verify validation.
        let validation = scg.validate();
        assert!(
            validation.is_valid,
            "complex SCG should validate: errors = {:?}",
            validation.errors
        );
    }

    // ── Test 30: Derive expression creates derivation edges ──────────────

    #[test]
    fn test_derive_expression_creates_derivation_edges() {
        let scg = parse_and_convert("region pool = allocate(1024); derive(pool, pool);");

        let derivation_edges: Vec<_> = scg
            .edges()
            .filter(|e| e.kind == EdgeKind::Derivation)
            .collect();
        assert!(
            derivation_edges.len() >= 2,
            "derive() should create Derivation edges (from alloc + derive expr)"
        );
    }

    // ── Test 31: Async + spawn parallel pattern ──────────────────────────

    #[test]
    fn test_async_spawn_parallel_pattern() {
        let scg = parse_and_convert("let x = async { spawn foo(); };");

        // Should have: async region (security boundary), spawn effect.
        let secure_regions = scg.regions().filter(|r| r.security_boundary).count();
        assert!(
            secure_regions >= 1,
            "async should create security-boundary region"
        );

        let spawn_effect = scg.nodes().find(|n| {
            if let NodePayload::Effect(e) = &n.payload {
                e.effect_kind.contains("spawn")
            } else {
                false
            }
        });
        assert!(
            spawn_effect.is_some(),
            "should have spawn Effect node inside async"
        );
    }

    // ── Test 32: Return value DataFlow from FunctionReturn to caller ─────

    #[test]
    fn test_return_value_data_flow_to_caller() {
        let scg = parse_and_convert("fn foo() -> u32 { return 42; } let result = foo();");

        // Find the call-site FunctionReturn.
        let call_return = scg.nodes().find(|n| {
            if let NodePayload::Control(c) = &n.payload {
                c.kind == ControlKind::FunctionReturn
                    && c.label
                        .as_ref()
                        .map_or(false, |l| l.starts_with("return_foo"))
            } else {
                false
            }
        });
        assert!(
            call_return.is_some(),
            "should have return_foo FunctionReturn"
        );

        // Find the caller node (let result = foo()).
        let caller = find_computation_node(&scg, "let result");
        assert!(caller.is_some(), "should have 'let result' node");

        // Enhanced: there should be a DataFlow edge from FunctionReturn
        // back to the caller node.
        if let (Some(ret), Some(call)) = (&call_return, &caller) {
            let has_return_df = scg
                .edges()
                .any(|e| e.source == ret.id && e.target == call.id && e.kind == EdgeKind::DataFlow);
            assert!(
                has_return_df,
                "should have DataFlow from FunctionReturn to caller node"
            );
        }
    }
}


