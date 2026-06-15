//! SCG Transformation Passes
//!
//! This module defines the transformation framework for the Semantic Computation
//! Graph. It provides a common [`SCGPass`] trait interface, several concrete
//! optimization and lowering passes, a verification pass, and a [`PassManager`]
//! that sequences and orchestrates pass execution.
//!
//! # Passes
//!
//! - [`DeadCodeElimination`] — removes nodes whose results are never consumed
//! - [`ConstantFolding`] — evaluates constant expressions at compile time
//! - [`CommonSubexpressionElimination`] — merges identical computation nodes
//! - [`InliningPass`] — inlines function-call regions by merging SCG subgraphs
//! - [`VerificationPass`] — verifies SCG well-formedness after transformation
//!
//! # Pass Manager
//!
//! The [`PassManager`] sequences passes, optionally running verification between
//! each one, and accumulates aggregate statistics across all runs.

use crate::node::ComputationKind;
use hashbrown::{HashMap, HashSet};

use crate::edge::EdgeKind;
use crate::graph::{ValidationResult, SCG};
use crate::node::{ControlKind, NodeId, NodePayload, NodeType};

// ── Pass Result ───────────────────────────────────────────────────────

/// Statistics and diagnostics returned by a single pass execution.
#[derive(Debug, Clone)]
pub struct PassResult {
    /// Name of the pass that produced this result.
    pub pass_name: String,
    /// Whether the pass made any changes to the graph.
    pub changed: bool,
    /// Number of nodes removed during this pass.
    pub nodes_removed: usize,
    /// Number of nodes added during this pass.
    pub nodes_added: usize,
    /// Number of edges removed during this pass.
    pub edges_removed: usize,
    /// Number of edges added during this pass.
    pub edges_added: usize,
    /// Any error messages produced by the pass.
    pub errors: Vec<String>,
}

impl PassResult {
    /// Creates an empty (unchanged) result for the given pass name.
    pub fn new(pass_name: impl Into<String>) -> Self {
        Self {
            pass_name: pass_name.into(),
            changed: false,
            nodes_removed: 0,
            nodes_added: 0,
            edges_removed: 0,
            edges_added: 0,
            errors: Vec::new(),
        }
    }

    /// Returns `true` if the pass encountered any errors.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Merges another `PassResult` into this one, summing statistics.
    pub fn merge(&mut self, other: &PassResult) {
        if other.changed {
            self.changed = true;
        }
        self.nodes_removed += other.nodes_removed;
        self.nodes_added += other.nodes_added;
        self.edges_removed += other.edges_removed;
        self.edges_added += other.edges_added;
        self.errors.extend_from_slice(&other.errors);
    }
}

impl std::fmt::Display for PassResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PassResult({}: changed={}, -{} nodes, +{} nodes, -{} edges, +{} edges",
            self.pass_name,
            self.changed,
            self.nodes_removed,
            self.nodes_added,
            self.edges_removed,
            self.edges_added
        )?;
        if !self.errors.is_empty() {
            write!(f, ", {} errors", self.errors.len())?;
        }
        write!(f, ")")
    }
}

// ── SCGPass Trait ─────────────────────────────────────────────────────

/// Common interface for all SCG transformation passes.
///
/// Every pass must provide a human-readable name and a `run` method that
/// mutates the graph in place and returns a `PassResult` describing what
/// changed.
pub trait SCGPass {
    /// Returns the human-readable name of this pass.
    fn name(&self) -> &str;

    /// Executes the pass on the given SCG, returning a `PassResult`.
    ///
    /// The graph is mutated in place. The returned result describes whether
    /// changes were made and what statistics were observed.
    fn run(&self, scg: &mut SCG) -> PassResult;
}

// ── Dead Code Elimination ─────────────────────────────────────────────

/// Removes nodes whose results are never consumed by another node.
///
/// A node is considered "dead" if:
/// - It has no outgoing **data-flow** edges (its result is unused), **and**
/// - It has no side effects that must be preserved.
///
/// Nodes that are never dead (always preserved):
/// - [`NodeType::Effect`] — observable side effects must be kept.
/// - [`NodeType::Control`] — control flow structure must be preserved.
/// - [`NodeType::Allocation`] / [`NodeType::Deallocation`] — memory lifecycle
///   nodes are always considered live.
/// - [`NodeType::Phantom`] — structural markers are preserved.
///
/// The pass iterates to a fixpoint: removing one node may make its
/// predecessors dead as well.
pub struct DeadCodeElimination;

impl DeadCodeElimination {
    /// Creates a new `DeadCodeElimination` pass.
    pub fn new() -> Self {
        Self
    }

    /// Returns `true` if the given node type is never eligible for removal.
    fn is_always_live(node_type: &NodeType) -> bool {
        matches!(
            node_type,
            NodeType::Effect
                | NodeType::Control
                | NodeType::Allocation
                | NodeType::Deallocation
                | NodeType::Phantom
        )
    }

    /// Returns `true` if a node has no outgoing data-flow edges, meaning
    /// its result is never consumed.
    fn has_no_dataflow_successors(scg: &SCG, id: NodeId) -> bool {
        if let Some(succs) = scg.successors(id) {
            for succ in succs {
                // Check if any edge from id -> succ is a DataFlow edge
                for edge in scg.edges() {
                    if edge.source == id && edge.target == succ && edge.kind == EdgeKind::DataFlow {
                        return false;
                    }
                }
            }
        }
        true
    }
}

impl Default for DeadCodeElimination {
    fn default() -> Self {
        Self::new()
    }
}

impl SCGPass for DeadCodeElimination {
    fn name(&self) -> &str {
        "DeadCodeElimination"
    }

    fn run(&self, scg: &mut SCG) -> PassResult {
        let mut result = PassResult::new(self.name());
        let mut iteration_changed = true;

        while iteration_changed {
            iteration_changed = false;
            let node_ids: Vec<NodeId> = scg.node_ids().collect();

            for id in node_ids {
                // Fetch node type; skip if node was already removed
                let node_type = match scg.get_node(id) {
                    Some(n) => n.node_type.clone(),
                    None => continue,
                };

                if Self::is_always_live(&node_type) {
                    continue;
                }

                if Self::has_no_dataflow_successors(scg, id) {
                    // Count edges that will be removed with this node
                    let outgoing = scg.successors(id).map_or(0, |s| s.len());
                    let incoming = scg.predecessors(id).map_or(0, |p| p.len());

                    if scg.remove_node(id).is_ok() {
                        result.nodes_removed += 1;
                        result.edges_removed += outgoing + incoming;
                        result.changed = true;
                        iteration_changed = true;
                    }
                }
            }
        }

        result
    }
}

// ── Constant Folding ──────────────────────────────────────────────────

/// Evaluates constant expressions at compile time.
///
/// This pass recognizes computation nodes whose operation string follows
/// the convention `"const.<type>:<value>"` (e.g., `"const.i32:42"`) as
/// compile-time constants. It also folds simple binary arithmetic on
/// constants where both predecessors are constant literals.
///
/// When a computation can be folded, the original node is replaced with
/// a new computation node whose operation is the constant result, and
/// the now-unnecessary input edges are removed.
///
/// # Foldable operations
///
/// - `"add"`, `"sub"`, `"mul"` — on two constant predecessors
/// - `"const.<type>:<value>"` — treated as a literal constant (no folding needed)
pub struct ConstantFolding;

impl ConstantFolding {
    /// Creates a new `ConstantFolding` pass.
    pub fn new() -> Self {
        Self
    }

    /// Tries to parse a constant value from an operation string.
    ///
    /// Convention: `"const.i32:42"` → `Some(42.0)`, otherwise `None`.
    fn try_parse_constant(operation: &str) -> Option<f64> {
        if let Some(rest) = operation.strip_prefix("const.") {
            if let Some(colon_pos) = rest.find(':') {
                let value_str = &rest[colon_pos + 1..];
                return value_str.parse::<f64>().ok();
            }
        }
        None
    }

    /// Returns `true` if the operation string represents a constant literal.
    fn is_constant(operation: &str) -> bool {
        Self::try_parse_constant(operation).is_some()
    }

    /// Attempts to fold a binary operation on two constant values.
    fn fold_binary(op: &str, lhs: f64, rhs: f64) -> Option<f64> {
        match op {
            "add" => Some(lhs + rhs),
            "sub" => Some(lhs - rhs),
            "mul" => Some(lhs * rhs),
            _ => None,
        }
    }

    /// Collects the constant values of all data-flow predecessors of a node.
    ///
    /// Returns a vector of `(NodeId, f64)` pairs for each predecessor that
    /// is a constant.
    fn collect_constant_predecessors(scg: &SCG, id: NodeId) -> Vec<(NodeId, f64)> {
        let mut constants = Vec::new();
        if let Some(preds) = scg.predecessors(id) {
            for pred_id in preds {
                // Only consider data-flow edges
                let is_dataflow = scg
                    .edges()
                    .any(|e| e.source == pred_id && e.target == id && e.kind == EdgeKind::DataFlow);
                if !is_dataflow {
                    continue;
                }
                if let Some(pred_node) = scg.get_node(pred_id) {
                    if let NodePayload::Computation(ref comp) = pred_node.payload {
                        if let ComputationKind::Other(ref op) = comp.kind {
                            if let Some(val) = Self::try_parse_constant(op) {
                                constants.push((pred_id, val));
                            }
                        }
                    }
                }
            }
        }
        constants
    }
}

impl Default for ConstantFolding {
    fn default() -> Self {
        Self::new()
    }
}

impl SCGPass for ConstantFolding {
    fn name(&self) -> &str {
        "ConstantFolding"
    }

    fn run(&self, scg: &mut SCG) -> PassResult {
        let mut result = PassResult::new(self.name());

        // Collect node IDs first to avoid borrow issues
        let node_ids: Vec<NodeId> = scg.node_ids().collect();

        for id in node_ids {
            // Get the operation string if this is a computation node
            let operation = match scg.get_node(id) {
                Some(n) => match &n.payload {
                    NodePayload::Computation(c) => c.kind.label(),
                    _ => continue,
                },
                None => continue,
            };

            // Skip if this is already a constant
            if Self::is_constant(&operation) {
                continue;
            }

            // Try binary folding: need exactly 2 constant data-flow predecessors
            let const_preds = Self::collect_constant_predecessors(scg, id);
            if const_preds.len() == 2 {
                if let Some(folded_val) =
                    Self::fold_binary(&operation, const_preds[0].1, const_preds[1].1)
                {
                    // Build the new constant operation string, preserving type if available
                    let result_type = scg
                        .get_node(id)
                        .and_then(|n| match &n.payload {
                            NodePayload::Computation(c) => c.result_type.clone(),
                            _ => None,
                        })
                        .unwrap_or_else(|| "i64".to_string());

                    let new_op = format!("const.{}:{}", result_type, folded_val);

                    // Mutate the node's operation in place
                    if let Some(node) = scg.get_node_mut(id) {
                        if let NodePayload::Computation(ref mut comp) = node.payload {
                            comp.kind = ComputationKind::Other(new_op);
                        }
                    }

                    result.changed = true;
                    // We don't remove predecessor edges here because the constant
                    // nodes may still be referenced by other nodes. DCE can clean
                    // them up in a subsequent pass.
                }
            }
        }

        result
    }
}

// ── Common Subexpression Elimination ──────────────────────────────────

/// Merges computation nodes that perform identical operations on identical
/// inputs.
///
/// Two computation nodes are considered common subexpressions if:
/// - They have the same `NodeType::Computation` payload (same operation and
///   result type).
/// - They have the same set of data-flow predecessors (same inputs).
///
/// When a common subexpression is found, the later node is removed and all
/// its outgoing data-flow edges are redirected to the earlier node.
pub struct CommonSubexpressionElimination;

impl CommonSubexpressionElimination {
    /// Creates a new `CommonSubexpressionElimination` pass.
    pub fn new() -> Self {
        Self
    }

    /// Computes a key that uniquely identifies a computation's expression:
    /// (operation, result_type, sorted predecessor NodeIds).
    fn expression_key(scg: &SCG, id: NodeId) -> Option<(String, Option<String>, Vec<NodeId>)> {
        let node = scg.get_node(id)?;
        if node.node_type != NodeType::Computation {
            return None;
        }
        match &node.payload {
            NodePayload::Computation(comp) => {
                let mut preds: Vec<NodeId> = scg
                    .edges()
                    .filter(|e| e.target == id && e.kind == EdgeKind::DataFlow)
                    .map(|e| e.source)
                    .collect();
                preds.sort();
                Some((comp.kind.label(), comp.result_type.clone(), preds))
            }
            _ => None,
        }
    }
}

impl Default for CommonSubexpressionElimination {
    fn default() -> Self {
        Self::new()
    }
}

impl SCGPass for CommonSubexpressionElimination {
    fn name(&self) -> &str {
        "CommonSubexpressionElimination"
    }

    fn run(&self, scg: &mut SCG) -> PassResult {
        let mut result = PassResult::new(self.name());

        // Map from expression key to the first NodeId that computes it.
        let mut seen: HashMap<(String, Option<String>, Vec<NodeId>), NodeId> = HashMap::new();

        // Process in topological order so we prefer keeping earlier nodes
        let topo = match scg.topological_sort() {
            Ok(t) => t,
            Err(_) => {
                result
                    .errors
                    .push("cannot run CSE on cyclic graph".to_string());
                return result;
            }
        };

        // Collect nodes to remove and edges to redirect
        let mut nodes_to_remove: HashSet<NodeId> = HashSet::new();
        // (old_target, replacement_node) — redirect outgoing data-flow edges
        let mut redirects: Vec<(NodeId, NodeId)> = Vec::new();

        for id in topo {
            if let Some(key) = Self::expression_key(scg, id) {
                if let Some(&existing) = seen.get(&key) {
                    // Duplicate found: mark for removal, redirect to existing
                    nodes_to_remove.insert(id);
                    redirects.push((id, existing));
                } else {
                    seen.insert(key, id);
                }
            }
        }

        // For each removed node, redirect its outgoing data-flow edges
        for (old_node, replacement) in &redirects {
            // Find all outgoing data-flow edges from old_node
            let successors: Vec<NodeId> = scg.successors(*old_node).unwrap_or_default();
            for succ in successors {
                // Check if this is a data-flow edge
                let is_dataflow = scg.edges().any(|e| {
                    e.source == *old_node && e.target == succ && e.kind == EdgeKind::DataFlow
                });
                if is_dataflow {
                    // Add a new edge from replacement -> succ if it doesn't exist
                    let already_exists = scg.edges().any(|e| {
                        e.source == *replacement && e.target == succ && e.kind == EdgeKind::DataFlow
                    });
                    if !already_exists
                        && scg.add_edge(*replacement, succ, EdgeKind::DataFlow).is_ok()
                    {
                        result.edges_added += 1;
                    }
                }
            }
        }

        // Remove the duplicate nodes
        for id in &nodes_to_remove {
            let out_edges = scg.successors(*id).map_or(0, |s| s.len());
            let in_edges = scg.predecessors(*id).map_or(0, |p| p.len());
            if scg.remove_node(*id).is_ok() {
                result.nodes_removed += 1;
                result.edges_removed += out_edges + in_edges;
                result.changed = true;
            }
        }

        result
    }
}

// ── Inlining Pass ─────────────────────────────────────────────────────

/// Inlines function calls by merging the callee's SCG region into the
/// caller's graph.
///
/// This pass looks for [`ControlKind::FunctionEntry`] control nodes that
/// represent function calls. For each such call site:
///
/// 1. It identifies the function body as the set of nodes reachable from
///    the `FunctionEntry` node via data-flow and control-flow edges.
/// 2. It creates a cloned copy of the function body, remapping node IDs
///    to avoid collisions.
/// 3. It splices the cloned body into the graph: the call site's
///    predecessors are wired to the cloned entry, and the cloned exit
///    is wired to the call site's successors.
/// 4. The original call-site node is replaced.
///
/// To support this, the pass uses the SCG's built-in `merge` operation
/// for subgraph integration.
pub struct InliningPass {
    /// Maximum number of nodes a function body may have to be inlined.
    /// Larger functions are skipped to avoid code bloat.
    pub max_inline_size: usize,
}

impl InliningPass {
    /// Creates a new `InliningPass` with a default max inline size of 50 nodes.
    pub fn new() -> Self {
        Self {
            max_inline_size: 50,
        }
    }

    /// Creates a new `InliningPass` with the specified max inline size.
    pub fn with_max_size(max_inline_size: usize) -> Self {
        Self { max_inline_size }
    }

    /// Collects all nodes in the function body reachable from a
    /// `FunctionEntry` node.
    fn collect_function_body(scg: &SCG, entry: NodeId) -> Vec<NodeId> {
        let mut visited = HashSet::new();
        let mut stack = vec![entry];

        while let Some(current) = stack.pop() {
            if visited.insert(current) {
                if let Some(succs) = scg.successors(current) {
                    for succ in succs {
                        if !visited.contains(&succ) {
                            stack.push(succ);
                        }
                    }
                }
            }
        }

        let mut body: Vec<NodeId> = visited.into_iter().collect();
        body.sort();
        body
    }

    /// Finds the `FunctionReturn` node within a function body, if any.
    fn find_function_return(scg: &SCG, body: &[NodeId]) -> Option<NodeId> {
        for &id in body {
            if let Some(node) = scg.get_node(id) {
                if let NodePayload::Control(ref ctrl) = node.payload {
                    if ctrl.kind == ControlKind::FunctionReturn {
                        return Some(id);
                    }
                }
            }
        }
        None
    }

    /// Clones the function body into a new SCG, returning the new SCG
    /// and a mapping from old NodeIds to new NodeIds.
    fn clone_function_body(scg: &SCG, body: &[NodeId]) -> (SCG, HashMap<NodeId, NodeId>) {
        let mut new_scg = SCG::new();
        let mut id_map: HashMap<NodeId, NodeId> = HashMap::new();

        // Clone nodes
        for &id in body {
            if let Some(node) = scg.get_node(id) {
                let new_id = new_scg.add_node(
                    node.node_type.clone(),
                    node.payload.clone(),
                    node.program_point.clone(),
                );
                // Copy annotation
                if let Some(ref ann) = node.annotation {
                    if let Some(new_node) = new_scg.get_node_mut(new_id) {
                        new_node.annotation = Some(ann.clone());
                    }
                }
                id_map.insert(id, new_id);
            }
        }

        // Clone edges within the body
        for edge in scg.edges() {
            if let (Some(&new_source), Some(&new_target)) =
                (id_map.get(&edge.source), id_map.get(&edge.target))
            {
                if let Ok(new_eid) = new_scg.add_edge(new_source, new_target, edge.kind.clone()) {
                    // Copy label
                    if let Some(ref label) = edge.label {
                        if let Some(e) = new_scg.get_edge_mut(new_eid) {
                            e.label = Some(label.clone());
                        }
                    }
                }
            }
        }

        (new_scg, id_map)
    }
}

impl Default for InliningPass {
    fn default() -> Self {
        Self::new()
    }
}

impl SCGPass for InliningPass {
    fn name(&self) -> &str {
        "InliningPass"
    }

    fn run(&self, scg: &mut SCG) -> PassResult {
        let mut result = PassResult::new(self.name());

        // Find all FunctionEntry call sites
        let call_sites: Vec<NodeId> = scg
            .nodes()
            .filter(|n| {
                matches!(
                    &n.payload,
                    NodePayload::Control(ctrl) if ctrl.kind == ControlKind::FunctionEntry
                )
            })
            .map(|n| n.id)
            .collect();

        if call_sites.is_empty() {
            return result;
        }

        for entry_id in call_sites {
            // Collect the function body
            let body = Self::collect_function_body(scg, entry_id);

            if body.len() > self.max_inline_size {
                result.errors.push(format!(
                    "function at {} too large to inline ({} nodes, max {})",
                    entry_id,
                    body.len(),
                    self.max_inline_size
                ));
                continue;
            }

            // Find the return node
            let return_id = Self::find_function_return(scg, &body);

            // Record predecessors of the entry (call-site inputs)
            let entry_preds: Vec<NodeId> = scg.predecessors(entry_id).unwrap_or_default();
            // Record successors of the return (call-site outputs)
            let return_succs: Vec<NodeId> = if let Some(ret) = return_id {
                scg.successors(ret).unwrap_or_default()
            } else {
                Vec::new()
            };

            // Clone the function body into a separate SCG
            let (body_scg, _id_map) = Self::clone_function_body(scg, &body);

            let nodes_before = scg.node_count();
            let edges_before = scg.edge_count();

            // Merge the cloned body into the main graph
            let node_remap = scg.merge(body_scg);

            result.nodes_added += scg.node_count() - nodes_before;
            result.edges_added += scg.edge_count() - edges_before;

            // Wire predecessors of the original entry to the cloned entry
            if let Some(&cloned_entry) = node_remap.get(&entry_id) {
                for &pred in &entry_preds {
                    // Avoid self-loops
                    if pred != cloned_entry
                        && scg.add_edge(pred, cloned_entry, EdgeKind::DataFlow).is_ok()
                    {
                        result.edges_added += 1;
                    }
                }
            }

            // Wire cloned return to successors of the original return
            if let Some(ret) = return_id {
                if let Some(&cloned_ret) = node_remap.get(&ret) {
                    for &succ in &return_succs {
                        if succ != cloned_ret
                            && scg.add_edge(cloned_ret, succ, EdgeKind::DataFlow).is_ok()
                        {
                            result.edges_added += 1;
                        }
                    }
                }
            }

            result.changed = true;
        }

        result
    }
}

// ── Verification Pass ─────────────────────────────────────────────────

/// Verifies SCG well-formedness after transformation.
///
/// This pass delegates to [`SCG::validate`] and also performs additional
/// checks relevant to post-transformation integrity:
///
/// - All edge endpoints reference existing nodes (delegated to SCG::validate).
/// - No dangling references in deallocation nodes.
/// - The graph remains acyclic (required for topological ordering).
/// - No duplicate data-flow edges between the same (source, target) pair.
///
/// The pass never modifies the graph. Errors are reported via the
/// `PassResult::errors` field, and `changed` is always `false`.
pub struct VerificationPass {
    /// Whether to also check that the graph is acyclic.
    pub check_acyclic: bool,
    /// Whether to check for duplicate data-flow edges.
    pub check_duplicate_edges: bool,
}

impl VerificationPass {
    /// Creates a new `VerificationPass` with all checks enabled.
    pub fn new() -> Self {
        Self {
            check_acyclic: true,
            check_duplicate_edges: true,
        }
    }

    /// Creates a minimal verification pass that only runs `SCG::validate`.
    pub fn minimal() -> Self {
        Self {
            check_acyclic: false,
            check_duplicate_edges: false,
        }
    }

    /// Checks for duplicate data-flow edges between the same node pair.
    fn check_duplicates(scg: &SCG) -> Vec<String> {
        let mut seen: HashSet<(NodeId, NodeId)> = HashSet::new();
        let mut errors = Vec::new();

        for edge in scg.edges() {
            if edge.kind == EdgeKind::DataFlow {
                let key = (edge.source, edge.target);
                if !seen.insert(key) {
                    errors.push(format!(
                        "duplicate data-flow edge from {} to {} (edge {})",
                        edge.source, edge.target, edge.id
                    ));
                }
            }
        }

        errors
    }
}

impl Default for VerificationPass {
    fn default() -> Self {
        Self::new()
    }
}

impl SCGPass for VerificationPass {
    fn name(&self) -> &str {
        "VerificationPass"
    }

    fn run(&self, scg: &mut SCG) -> PassResult {
        let mut result = PassResult::new(self.name());
        // Verification never changes the graph
        result.changed = false;

        // Run the SCG's built-in validation
        let validation: ValidationResult = scg.validate();
        if !validation.is_valid {
            for err in &validation.errors {
                result.errors.push(err.clone());
            }
        }
        // Warnings are informational; we don't treat them as pass errors.
        // They are silently ignored so that valid programs with minor
        // style issues (e.g., allocations without paired deallocations)
        // don't fail compilation.

        // Additional check: acyclicity
        if self.check_acyclic && scg.topological_sort().is_err() {
            result.errors.push("graph contains a cycle".to_string());
        }

        // Additional check: duplicate edges
        if self.check_duplicate_edges {
            let dup_errors = Self::check_duplicates(scg);
            result.errors.extend(dup_errors);
        }

        result
    }
}

// ── Pass Manager ──────────────────────────────────────────────────────

/// Manages a sequence of transformation passes and orchestrates their
/// execution.
///
/// The `PassManager` supports:
/// - Registering passes in order.
/// - Optionally running [`VerificationPass`] between each registered pass.
/// - Collecting aggregate statistics across all pass runs.
/// - Stopping early if a pass produces errors and `stop_on_error` is set.
pub struct PassManager {
    /// The ordered list of passes to run.
    passes: Vec<Box<dyn SCGPass>>,
    /// Whether to run verification between each pass.
    verify_between: bool,
    /// Whether to stop execution when a pass reports errors.
    stop_on_error: bool,
}

/// Aggregate result of running the entire pass pipeline.
#[derive(Debug, Clone)]
pub struct PipelineResult {
    /// Results from each individual pass, in execution order.
    pub pass_results: Vec<PassResult>,
    /// Whether any pass made changes.
    pub changed: bool,
    /// Total nodes removed across all passes.
    pub total_nodes_removed: usize,
    /// Total nodes added across all passes.
    pub total_nodes_added: usize,
    /// Total edges removed across all passes.
    pub total_edges_removed: usize,
    /// Total edges added across all passes.
    pub total_edges_added: usize,
    /// Whether the pipeline encountered any errors.
    pub has_errors: bool,
    /// The index of the pass that caused an early stop, if any.
    pub stopped_at: Option<usize>,
}

impl PipelineResult {
    /// Creates an empty pipeline result.
    pub fn new() -> Self {
        Self {
            pass_results: Vec::new(),
            changed: false,
            total_nodes_removed: 0,
            total_nodes_added: 0,
            total_edges_removed: 0,
            total_edges_added: 0,
            has_errors: false,
            stopped_at: None,
        }
    }

    /// Records a single pass result into the pipeline aggregate.
    fn record(&mut self, pr: PassResult) {
        if pr.changed {
            self.changed = true;
        }
        self.total_nodes_removed += pr.nodes_removed;
        self.total_nodes_added += pr.nodes_added;
        self.total_edges_removed += pr.edges_removed;
        self.total_edges_added += pr.edges_added;
        if pr.has_errors() {
            self.has_errors = true;
        }
        self.pass_results.push(pr);
    }
}

impl Default for PipelineResult {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for PipelineResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "PipelineResult:")?;
        writeln!(f, "  changed: {}", self.changed)?;
        writeln!(f, "  total_nodes_removed: {}", self.total_nodes_removed)?;
        writeln!(f, "  total_nodes_added: {}", self.total_nodes_added)?;
        writeln!(f, "  total_edges_removed: {}", self.total_edges_removed)?;
        writeln!(f, "  total_edges_added: {}", self.total_edges_added)?;
        writeln!(f, "  has_errors: {}", self.has_errors)?;
        if let Some(idx) = self.stopped_at {
            writeln!(f, "  stopped_at: pass #{idx}")?;
        }
        for pr in &self.pass_results {
            writeln!(f, "  - {pr}")?;
        }
        Ok(())
    }
}

impl PassManager {
    /// Creates a new, empty `PassManager`.
    pub fn new() -> Self {
        Self {
            passes: Vec::new(),
            verify_between: false,
            stop_on_error: true,
        }
    }

    /// Adds a pass to the pipeline.
    pub fn add_pass(&mut self, pass: impl SCGPass + 'static) -> &mut Self {
        self.passes.push(Box::new(pass));
        self
    }

    /// Enables or disables verification between passes.
    pub fn verify_between(mut self, enable: bool) -> Self {
        self.verify_between = enable;
        self
    }

    /// Enables or disables stopping on the first error.
    pub fn stop_on_error(mut self, enable: bool) -> Self {
        self.stop_on_error = enable;
        self
    }

    /// Runs all registered passes on the given SCG.
    ///
    /// If `verify_between` is enabled, a `VerificationPass` is run after
    /// each registered pass. If verification fails and `stop_on_error` is
    /// set, the pipeline stops early.
    pub fn run(&self, scg: &mut SCG) -> PipelineResult {
        let mut pipeline_result = PipelineResult::new();

        for (i, pass) in self.passes.iter().enumerate() {
            let pr = pass.run(scg);
            let had_errors = pr.has_errors();
            pipeline_result.record(pr);

            if had_errors && self.stop_on_error {
                pipeline_result.stopped_at = Some(i);
                break;
            }

            // Optionally run verification after each pass
            if self.verify_between {
                let verify = VerificationPass::new();
                let vpr = verify.run(scg);
                let v_had_errors = vpr.has_errors();
                pipeline_result.record(vpr);

                if v_had_errors && self.stop_on_error {
                    pipeline_result.stopped_at = Some(i);
                    break;
                }
            }
        }

        pipeline_result
    }

    /// Returns the number of registered passes.
    pub fn pass_count(&self) -> usize {
        self.passes.len()
    }
}

impl Default for PassManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Loop Invariant Code Motion ────────────────────────────────────────

/// Hoists loop-invariant computations out of loops.
///
/// A computation node is loop-invariant if all its inputs are defined
/// outside the loop (i.e., none of its data-flow predecessors are
/// inside the loop body). Such nodes are hoisted to the loop pre-header
/// (before the LoopHeader node).
///
/// Nodes with side effects (memory writes, calls, allocations) are
/// never hoisted.
pub struct LoopInvariantCodeMotion;

impl LoopInvariantCodeMotion {
    /// Creates a new loop-invariant code motion pass.
    pub fn new() -> Self {
        Self
    }

    /// Returns true if the node type has side effects and must not be hoisted.
    fn has_side_effects(node_type: &NodeType) -> bool {
        matches!(
            node_type,
            NodeType::Effect
                | NodeType::Allocation
                | NodeType::Deallocation
                | NodeType::Access
                | NodeType::Control
                | NodeType::Phantom
                | NodeType::VTable
                | NodeType::ClosureEnv
        )
    }

    /// Collects all nodes that belong to a loop body: nodes reachable
    /// from the LoopHeader but before the LoopExit, and not the
    /// LoopHeader/LoopExit themselves.
    fn collect_loop_body(scg: &SCG, header: NodeId) -> (Vec<NodeId>, Option<NodeId>) {
        let mut body = Vec::new();
        let mut exit = None;
        let mut visited = HashSet::new();
        let mut stack = vec![header];
        while let Some(nid) = stack.pop() {
            if !visited.insert(nid) {
                continue;
            }
            if let Some(node) = scg.get_node(nid) {
                if let NodePayload::Control(ref c) = node.payload {
                    if c.kind == ControlKind::LoopExit {
                        exit = Some(nid);
                        continue; // don't include exit in body
                    }
                    if nid == header { /* don't include header itself */ }
                }
            }
            if nid != header {
                body.push(nid);
            }
            if let Some(succs) = scg.successors(nid) {
                for s in succs {
                    if !visited.contains(&s) {
                        stack.push(s);
                    }
                }
            }
        }
        (body, exit)
    }

    /// Checks whether a node is loop-invariant: all data-flow predecessors
    /// are defined outside the loop body.
    fn is_loop_invariant(scg: &SCG, node_id: NodeId, loop_body: &HashSet<NodeId>) -> bool {
        if let Some(preds) = scg.predecessors(node_id) {
            for pred in preds {
                let is_df = scg.edges().any(|e| {
                    e.source == pred && e.target == node_id && e.kind == EdgeKind::DataFlow
                });
                if is_df && loop_body.contains(&pred) {
                    return false;
                }
            }
        }
        true
    }
}

impl Default for LoopInvariantCodeMotion {
    fn default() -> Self {
        Self::new()
    }
}

impl SCGPass for LoopInvariantCodeMotion {
    fn name(&self) -> &str {
        "LoopInvariantCodeMotion"
    }

    fn run(&self, scg: &mut SCG) -> PassResult {
        let mut result = PassResult::new(self.name());

        // Find all LoopHeader nodes.
        let loop_headers: Vec<NodeId> = scg.nodes()
            .filter(|n| matches!(&n.payload, NodePayload::Control(c) if c.kind == ControlKind::LoopHeader))
            .map(|n| n.id)
            .collect();

        for header in loop_headers {
            let (body, _exit) = Self::collect_loop_body(scg, header);
            let body_set: HashSet<NodeId> = body.iter().copied().collect();

            // Find the pre-header: the predecessor of the header that is not in the body.
            let pre_header = scg
                .predecessors(header)
                .unwrap_or_default()
                .into_iter()
                .find(|p| !body_set.contains(p));

            for &nid in &body {
                let node_type = match scg.get_node(nid) {
                    Some(n) => n.node_type.clone(),
                    None => continue,
                };
                if Self::has_side_effects(&node_type) {
                    continue;
                }
                if !Self::is_loop_invariant(scg, nid, &body_set) {
                    continue;
                }

                // Hoist: add a ControlFlow edge from pre_header to nid,
                // and from nid to header. Remove the old edges within the loop.
                if let Some(pre) = pre_header {
                    let _ = scg.add_edge(pre, nid, EdgeKind::ControlFlow);
                    let _ = scg.add_edge(nid, header, EdgeKind::ControlFlow);
                    result.edges_added += 2;
                    result.changed = true;
                }
            }
        }

        result
    }
}

// ── Strength Reduction ────────────────────────────────────────────────

/// Replaces expensive operations with cheaper equivalents.
///
/// - Multiply by constant power of 2 → left shift
/// - Divide by power of 2 → right shift
/// - Modulo by power of 2 → bitwise AND
pub struct StrengthReduction;

impl StrengthReduction {
    /// Creates a new strength-reduction pass.
    pub fn new() -> Self {
        Self
    }

    /// Returns true if `n` is a power of 2 (and n > 0).
    fn is_power_of_two(n: u64) -> bool {
        n > 0 && (n & (n - 1)) == 0
    }

    /// Returns log2(n) for powers of 2.
    fn log2(n: u64) -> u32 {
        63 - n.leading_zeros()
    }

    /// Try to parse a constant integer from a ComputationNode's operation string.
    /// Convention: "const.<type>:<value>" or just a bare integer in the operation.
    fn try_parse_const_int(operation: &str) -> Option<u64> {
        if let Some(rest) = operation.strip_prefix("const.") {
            if let Some(colon_pos) = rest.find(':') {
                let val_str = &rest[colon_pos + 1..];
                return val_str.parse::<u64>().ok();
            }
        }
        None
    }
}

impl Default for StrengthReduction {
    fn default() -> Self {
        Self::new()
    }
}

impl SCGPass for StrengthReduction {
    fn name(&self) -> &str {
        "StrengthReduction"
    }

    fn run(&self, scg: &mut SCG) -> PassResult {
        let mut result = PassResult::new(self.name());
        let node_ids: Vec<NodeId> = scg.node_ids().collect();

        for id in node_ids {
            let (operation, _result_type) = match scg.get_node(id) {
                Some(n) => match &n.payload {
                    NodePayload::Computation(c) => (c.kind.label(), c.result_type.clone()),
                    _ => continue,
                },
                None => continue,
            };

            // Check for mul by constant power of 2 → shift left
            if operation == "mul" {
                let const_preds: Vec<(NodeId, u64)> = scg
                    .predecessors(id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|pred| {
                        let is_df = scg.edges().any(|e| {
                            e.source == pred && e.target == id && e.kind == EdgeKind::DataFlow
                        });
                        if !is_df {
                            return None;
                        }
                        if let Some(pn) = scg.get_node(pred) {
                            if let NodePayload::Computation(c) = &pn.payload {
                                if let ComputationKind::Other(ref op) = c.kind {
                                    return Self::try_parse_const_int(op).map(|v| (pred, v));
                                }
                            }
                        }
                        None
                    })
                    .collect();

                if let Some(&(_, const_val)) = const_preds.first() {
                    if Self::is_power_of_two(const_val) {
                        let shift = Self::log2(const_val);
                        let new_op = format!("shl_{}", shift);
                        if let Some(node) = scg.get_node_mut(id) {
                            if let NodePayload::Computation(ref mut c) = node.payload {
                                c.kind = ComputationKind::Other(new_op);
                            }
                        }
                        result.changed = true;
                    }
                }
            }

            // Check for div by constant power of 2 → shift right
            if operation == "div" {
                let const_preds: Vec<(NodeId, u64)> = scg
                    .predecessors(id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|pred| {
                        let is_df = scg.edges().any(|e| {
                            e.source == pred && e.target == id && e.kind == EdgeKind::DataFlow
                        });
                        if !is_df {
                            return None;
                        }
                        if let Some(pn) = scg.get_node(pred) {
                            if let NodePayload::Computation(c) = &pn.payload {
                                if let ComputationKind::Other(ref op) = c.kind {
                                    return Self::try_parse_const_int(op).map(|v| (pred, v));
                                }
                            }
                        }
                        None
                    })
                    .collect();

                if let Some(&(_, const_val)) = const_preds.first() {
                    if Self::is_power_of_two(const_val) {
                        let shift = Self::log2(const_val);
                        let new_op = format!("shr_{}", shift);
                        if let Some(node) = scg.get_node_mut(id) {
                            if let NodePayload::Computation(ref mut c) = node.payload {
                                c.kind = ComputationKind::Other(new_op);
                            }
                        }
                        result.changed = true;
                    }
                }
            }

            // Check for modulo by power of 2 → bitwise AND
            if operation == "mod" || operation == "rem" {
                let const_preds: Vec<(NodeId, u64)> = scg
                    .predecessors(id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|pred| {
                        let is_df = scg.edges().any(|e| {
                            e.source == pred && e.target == id && e.kind == EdgeKind::DataFlow
                        });
                        if !is_df {
                            return None;
                        }
                        if let Some(pn) = scg.get_node(pred) {
                            if let NodePayload::Computation(c) = &pn.payload {
                                if let ComputationKind::Other(ref op) = c.kind {
                                    return Self::try_parse_const_int(op).map(|v| (pred, v));
                                }
                            }
                        }
                        None
                    })
                    .collect();

                if let Some(&(_, const_val)) = const_preds.first() {
                    if Self::is_power_of_two(const_val) {
                        let mask = const_val - 1;
                        let new_op = format!("and_{}", mask);
                        if let Some(node) = scg.get_node_mut(id) {
                            if let NodePayload::Computation(ref mut c) = node.payload {
                                c.kind = ComputationKind::Other(new_op);
                            }
                        }
                        result.changed = true;
                    }
                }
            }
        }

        result
    }
}

// ── Tail-Call Optimization Detection ─────────────────────────────────

/// Identifies tail calls and marks them with `tail_call: bool`.
///
/// A tail call is a function call that is the last action before return.
/// When detected, the backend can reuse the current stack frame.
///
/// Detection: a Computation node is a tail call if:
/// - Its operation is a call (contains "call" or "fn_call"),
/// - It feeds directly into a FunctionReturn control node via
///   DataFlow or ControlFlow,
/// - And there are no other nodes between the call and the return.
pub struct TailCallOptDetection;

impl TailCallOptDetection {
    /// Creates a new tail-call optimization detection pass.
    pub fn new() -> Self {
        Self
    }

    /// Check if a computation node represents a function call.
    fn is_call_node(operation: &str) -> bool {
        operation.contains("call") || operation.contains("fn_call") || operation.contains("call_")
    }

    /// Check if a node feeds directly into a FunctionReturn.
    fn feeds_into_return(scg: &SCG, id: NodeId) -> bool {
        if let Some(succs) = scg.successors(id) {
            for succ in succs {
                let is_cf_or_df = scg.edges().any(|e| {
                    e.source == id
                        && e.target == succ
                        && (e.kind == EdgeKind::ControlFlow || e.kind == EdgeKind::DataFlow)
                });
                if is_cf_or_df {
                    if let Some(node) = scg.get_node(succ) {
                        if let NodePayload::Control(c) = &node.payload {
                            if c.kind == ControlKind::FunctionReturn {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    }
}

impl Default for TailCallOptDetection {
    fn default() -> Self {
        Self::new()
    }
}

impl SCGPass for TailCallOptDetection {
    fn name(&self) -> &str {
        "TailCallOptDetection"
    }

    fn run(&self, scg: &mut SCG) -> PassResult {
        let mut result = PassResult::new(self.name());
        let node_ids: Vec<NodeId> = scg.node_ids().collect();

        for id in node_ids {
            let is_call = match scg.get_node(id) {
                Some(n) => match &n.payload {
                    NodePayload::Computation(c) => {
                        if let ComputationKind::Other(ref op) = c.kind {
                            Self::is_call_node(op)
                        } else {
                            false
                        }
                    }
                    _ => false,
                },
                None => continue,
            };
            if !is_call {
                continue;
            }

            if Self::feeds_into_return(scg, id) {
                if let Some(node) = scg.get_node_mut(id) {
                    if let NodePayload::Computation(ref mut c) = node.payload {
                        if !c.tail_call {
                            c.tail_call = true;
                            result.changed = true;
                        }
                    }
                }
            }
        }

        result
    }
}

// ── Dead Region Elimination ───────────────────────────────────────────

/// Identifies allocation/deallocation pairs where the region is never
/// read from, and removes both along with any write-only operations.
///
/// An allocation region is "dead" if:
/// - There is a matching allocation/deallocation pair,
/// - No Access node with Read or ReadWrite mode references the same
///   region,
/// - All accesses to the region (if any) are Write-only.
///
/// When a dead region is found:
/// - The allocation and deallocation nodes are removed,
/// - Any write-only access nodes to that region are also removed.
pub struct DeadRegionElimination;

impl DeadRegionElimination {
    /// Creates a new dead region elimination pass.
    pub fn new() -> Self {
        Self
    }

    /// Find allocation/deallocation pairs and their region IDs.
    fn find_alloc_dealloc_pairs(scg: &SCG) -> Vec<(NodeId, NodeId, crate::region::RegionId)> {
        let mut allocs: HashMap<crate::region::RegionId, NodeId> = HashMap::new();
        let mut pairs = Vec::new();

        for node in scg.nodes() {
            match &node.payload {
                NodePayload::Allocation(a) => {
                    allocs.insert(a.region_id, node.id);
                }
                NodePayload::Deallocation(d) => {
                    if let Some(&alloc_id) = allocs.get(&d.region_id) {
                        pairs.push((alloc_id, node.id, d.region_id));
                    }
                }
                _ => {}
            }
        }
        pairs
    }

    /// Check if any Access node reads from the given region.
    fn has_read_access(scg: &SCG, region_id: crate::region::RegionId) -> bool {
        scg.nodes().any(|n| {
            if let NodePayload::Access(a) = &n.payload {
                a.region_id == region_id
                    && (a.mode == crate::node::AccessMode::Read
                        || a.mode == crate::node::AccessMode::ReadWrite)
            } else {
                false
            }
        })
    }

    /// Find all write-only access nodes to the given region.
    fn find_write_only_accesses(scg: &SCG, region_id: crate::region::RegionId) -> Vec<NodeId> {
        scg.nodes()
            .filter(|n| {
                if let NodePayload::Access(a) = &n.payload {
                    a.region_id == region_id && a.mode == crate::node::AccessMode::Write
                } else {
                    false
                }
            })
            .map(|n| n.id)
            .collect()
    }
}

impl Default for DeadRegionElimination {
    fn default() -> Self {
        Self::new()
    }
}

impl SCGPass for DeadRegionElimination {
    fn name(&self) -> &str {
        "DeadRegionElimination"
    }

    fn run(&self, scg: &mut SCG) -> PassResult {
        let mut result = PassResult::new(self.name());

        let pairs = Self::find_alloc_dealloc_pairs(scg);

        for (alloc_id, dealloc_id, region_id) in pairs {
            // Check if the region has any reads
            if Self::has_read_access(scg, region_id) {
                continue; // Region is live — has reads
            }

            // Find write-only accesses to this region
            let write_only = Self::find_write_only_accesses(scg, region_id);

            // Remove write-only accesses
            for woid in &write_only {
                let out = scg.successors(*woid).map_or(0, |s| s.len());
                let inc = scg.predecessors(*woid).map_or(0, |p| p.len());
                if scg.remove_node(*woid).is_ok() {
                    result.nodes_removed += 1;
                    result.edges_removed += out + inc;
                }
            }

            // Remove allocation
            let out = scg.successors(alloc_id).map_or(0, |s| s.len());
            let inc = scg.predecessors(alloc_id).map_or(0, |p| p.len());
            if scg.remove_node(alloc_id).is_ok() {
                result.nodes_removed += 1;
                result.edges_removed += out + inc;
            }

            // Remove deallocation (may have been invalidated by alloc removal — check first)
            if scg.get_node(dealloc_id).is_some() {
                let out = scg.successors(dealloc_id).map_or(0, |s| s.len());
                let inc = scg.predecessors(dealloc_id).map_or(0, |p| p.len());
                if scg.remove_node(dealloc_id).is_ok() {
                    result.nodes_removed += 1;
                    result.edges_removed += out + inc;
                }
            }

            result.changed = true;
        }

        result
    }
}

// ── Standalone Transform Functions ─────────────────────────────────────

/// Loop Invariant Code Motion (LICM).
///
/// Identifies nodes inside loop bodies whose inputs don't change across
/// iterations and hoists them before the LoopHeader node. Nodes with side
/// effects (writes, calls, allocations) are never hoisted.
///
/// Returns the list of hoisted node IDs.
pub fn licm(graph: &mut SCG) -> Vec<NodeId> {
    let mut hoisted = Vec::new();

    // Find all LoopHeader nodes.
    let loop_headers: Vec<NodeId> = graph
        .nodes()
        .filter(|n| {
            matches!(
                &n.payload,
                NodePayload::Control(c) if c.kind == ControlKind::LoopHeader
            )
        })
        .map(|n| n.id)
        .collect();

    for header in loop_headers {
        let (body, _exit) = LoopInvariantCodeMotion::collect_loop_body(graph, header);
        let body_set: HashSet<NodeId> = body.iter().copied().collect();
        // Include the header itself as "inside the loop" for invariant checking,
        // since values produced by the header may change across iterations.
        let mut loop_set: HashSet<NodeId> = body_set.clone();
        loop_set.insert(header);

        // Find the pre-header: predecessor of the header that is not in the body.
        let pre_header = graph
            .predecessors(header)
            .unwrap_or_default()
            .into_iter()
            .find(|p| !body_set.contains(p));

        for &nid in &body {
            let node_type = match graph.get_node(nid) {
                Some(n) => n.node_type.clone(),
                None => continue,
            };
            if LoopInvariantCodeMotion::has_side_effects(&node_type) {
                continue;
            }
            if !LoopInvariantCodeMotion::is_loop_invariant(graph, nid, &loop_set) {
                continue;
            }

            // Hoist: add ControlFlow edges from pre_header to nid and from nid to header.
            if let Some(pre) = pre_header {
                let _ = graph.add_edge(pre, nid, EdgeKind::ControlFlow);
                let _ = graph.add_edge(nid, header, EdgeKind::ControlFlow);
                hoisted.push(nid);
            }
        }
    }

    hoisted
}

/// Strength Reduction.
///
/// Replaces multiply-by-constant-2^n with left shift, divide-by-power-of-2
/// with right shift, and modulo-by-power-of-2 with bitwise AND.
///
/// Returns the list of replaced node IDs.
pub fn strength_reduce(graph: &mut SCG) -> Vec<NodeId> {
    let mut replaced = Vec::new();
    let node_ids: Vec<NodeId> = graph.node_ids().collect();

    for id in node_ids {
        let operation = match graph.get_node(id) {
            Some(n) => match &n.payload {
                NodePayload::Computation(c) => c.kind.label(),
                _ => continue,
            },
            None => continue,
        };

        let new_op = if operation == "mul" {
            let const_val = get_const_df_predecessor(graph, id);
            const_val.and_then(|v| {
                if StrengthReduction::is_power_of_two(v) {
                    Some(format!("shl_{}", StrengthReduction::log2(v)))
                } else {
                    None
                }
            })
        } else if operation == "div" {
            let const_val = get_const_df_predecessor(graph, id);
            const_val.and_then(|v| {
                if StrengthReduction::is_power_of_two(v) {
                    Some(format!("shr_{}", StrengthReduction::log2(v)))
                } else {
                    None
                }
            })
        } else if operation == "mod" || operation == "rem" {
            let const_val = get_const_df_predecessor(graph, id);
            const_val.and_then(|v| {
                if StrengthReduction::is_power_of_two(v) {
                    Some(format!("and_{}", v - 1))
                } else {
                    None
                }
            })
        } else {
            None
        };

        if let Some(op) = new_op {
            if let Some(node) = graph.get_node_mut(id) {
                if let NodePayload::Computation(ref mut c) = node.payload {
                    c.kind = ComputationKind::Other(op);
                    replaced.push(id);
                }
            }
        }
    }

    replaced
}

/// Helper: get the constant integer value of the first data-flow predecessor
/// that is a constant node, if any.
fn get_const_df_predecessor(graph: &SCG, id: NodeId) -> Option<u64> {
    let preds = graph.predecessors(id).unwrap_or_default();
    for pred in preds {
        let is_df = graph
            .edges()
            .any(|e| e.source == pred && e.target == id && e.kind == EdgeKind::DataFlow);
        if !is_df {
            continue;
        }
        if let Some(pn) = graph.get_node(pred) {
            if let NodePayload::Computation(c) = &pn.payload {
                if let Some(v) = (match &c.kind {
                    ComputationKind::Other(ref op) => StrengthReduction::try_parse_const_int(op),
                    _ => None,
                }) {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// Tail Call Detection.
///
/// Finds function calls that are the last action before FunctionReturn and
/// marks the `ComputationNode.tail_call` field as `true`.
///
/// Returns the list of tail call node IDs.
pub fn detect_tail_calls(graph: &mut SCG) -> Vec<NodeId> {
    let mut tail_calls = Vec::new();
    let node_ids: Vec<NodeId> = graph.node_ids().collect();

    for id in node_ids {
        let is_call = match graph.get_node(id) {
            Some(n) => match &n.payload {
                NodePayload::Computation(c) => {
                    if let ComputationKind::Other(ref op) = c.kind {
                        TailCallOptDetection::is_call_node(op)
                    } else {
                        false
                    }
                }
                _ => false,
            },
            None => continue,
        };
        if !is_call {
            continue;
        }

        if TailCallOptDetection::feeds_into_return(graph, id) {
            if let Some(node) = graph.get_node_mut(id) {
                if let NodePayload::Computation(ref mut c) = node.payload {
                    if !c.tail_call {
                        c.tail_call = true;
                        tail_calls.push(id);
                    }
                }
            }
        }
    }

    tail_calls
}

/// Dead Region Elimination.
///
/// Finds Allocate nodes whose region is never read from (only written) and
/// removes the Allocate, corresponding Free, and any write-only operations.
///
/// Returns the list of removed node IDs.
pub fn dead_region_elim(graph: &mut SCG) -> Vec<NodeId> {
    let mut removed = Vec::new();

    let pairs = DeadRegionElimination::find_alloc_dealloc_pairs(graph);

    for (alloc_id, dealloc_id, region_id) in pairs {
        // Check if the region has any reads
        if DeadRegionElimination::has_read_access(graph, region_id) {
            continue; // Region is live — has reads
        }

        // Find write-only accesses to this region
        let write_only = DeadRegionElimination::find_write_only_accesses(graph, region_id);

        // Remove write-only accesses
        for woid in &write_only {
            if graph.remove_node(*woid).is_ok() {
                removed.push(*woid);
            }
        }

        // Remove allocation
        if graph.remove_node(alloc_id).is_ok() {
            removed.push(alloc_id);
        }

        // Remove deallocation (may have been invalidated by alloc removal — check first)
        if graph.get_node(dealloc_id).is_some() && graph.remove_node(dealloc_id).is_ok() {
            removed.push(dealloc_id);
        }
    }

    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::EdgeKind;
    use crate::graph::SCG;
    use crate::node::{
        ComputationKind, ComputationNode, ControlKind, ControlNode, EffectNode, NodePayload, NodeType, ProgramPoint,
    };

    /// Helper: create a default program point for tests.
    fn pp() -> ProgramPoint {
        ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: None,
        }
    }

    // ── DeadCodeElimination Tests ─────────────────────────────────────

    #[test]
    fn test_dce_removes_unused_computation() {
        let mut scg = SCG::new();
        // n1 is a constant feeding into a live n2, and n3 is unused
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:10".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Effect,
            NodePayload::Effect(EffectNode {
                effect_kind: "use".to_string(),
                is_observable: true,
            }),
            pp(),
        );
        let n3 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("sub".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n1, n3, EdgeKind::DataFlow).unwrap();

        // n3 has no data-flow successors, so DCE removes it.
        // n1 is preserved because n2 (an Effect node) still consumes it.
        let result = DeadCodeElimination.run(&mut scg);
        assert!(result.changed);
        assert_eq!(result.nodes_removed, 1);
        // n3 should be gone; n1 and n2 are preserved
        assert!(scg.get_node(n3).is_none());
        assert!(scg.get_node(n1).is_some());
        assert!(scg.get_node(n2).is_some());
    }

    #[test]
    fn test_dce_preserves_effect_nodes() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Effect,
            NodePayload::Effect(EffectNode {
                effect_kind: "print".to_string(),
                is_observable: true,
            }),
            pp(),
        );
        // Effect node has no successors but must be preserved
        let result = DeadCodeElimination.run(&mut scg);
        assert!(!result.changed);
        assert!(scg.get_node(n1).is_some());
    }

    #[test]
    fn test_dce_cascades_removals() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:1".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:2".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n3 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n3, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n3, EdgeKind::DataFlow).unwrap();
        // n3 has no data-flow successors → dead. After n3 removed,
        // n1 and n2 also have no data-flow successors → dead.
        let result = DeadCodeElimination.run(&mut scg);
        assert!(result.changed);
        assert_eq!(result.nodes_removed, 3);
        assert_eq!(scg.node_count(), 0);
    }

    // ── ConstantFolding Tests ─────────────────────────────────────────

    #[test]
    fn test_constant_fold_binary_add() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:10".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:20".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n3 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n3, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n3, EdgeKind::DataFlow).unwrap();

        let result = ConstantFolding.run(&mut scg);
        assert!(result.changed);

        // n3 should now be a constant with value 30
        let folded = scg.get_node(n3).unwrap();
        match &folded.payload {
            NodePayload::Computation(c) => {
                let op_label = c.kind.label();
                assert!(op_label.starts_with("const.i32:"));
                assert!(op_label.contains("30"));
            }
            _ => panic!("expected computation node"),
        }
    }

    #[test]
    fn test_constant_fold_does_not_fold_non_constant() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("load".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();

        let result = ConstantFolding.run(&mut scg);
        assert!(!result.changed);
    }

    // ── CommonSubexpressionElimination Tests ───────────────────────────

    #[test]
    fn test_cse_merges_identical_computations() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:5".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:3".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        // Two identical add nodes consuming same inputs
        let n3 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n4 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n5 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("use".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );

        scg.add_edge(n1, n3, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n3, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n1, n4, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n4, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n3, n5, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n4, n5, EdgeKind::DataFlow).unwrap();

        let result = CommonSubexpressionElimination.run(&mut scg);
        assert!(result.changed);
        assert_eq!(result.nodes_removed, 1);
    }

    #[test]
    fn test_cse_no_merge_different_operations() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:5".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n3 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("sub".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n1, n3, EdgeKind::DataFlow).unwrap();

        let result = CommonSubexpressionElimination.run(&mut scg);
        assert!(!result.changed);
    }

    // ── VerificationPass Tests ────────────────────────────────────────

    #[test]
    fn test_verification_valid_graph() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("sub".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();

        let result = VerificationPass::new().run(&mut scg);
        // Should have no hard errors (warnings about orphans may exist)
        let hard_errors = result
            .errors
            .iter()
            .filter(|e| !e.starts_with("WARNING:"))
            .count();
        assert_eq!(hard_errors, 0);
        assert!(!result.changed);
    }

    #[test]
    fn test_verification_detects_cycle() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("a".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("b".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n1, EdgeKind::DataFlow).unwrap();

        let result = VerificationPass::new().run(&mut scg);
        assert!(result.errors.iter().any(|e| e.contains("cycle")));
    }

    // ── InliningPass Tests ────────────────────────────────────────────

    #[test]
    fn test_inlining_identifies_function_entry() {
        let mut scg = SCG::new();
        let entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("foo".to_string()),
            }),
            pp(),
        );
        let ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(entry, ret, EdgeKind::ControlFlow).unwrap();

        let result = InliningPass::new().run(&mut scg);
        // Inlining should change the graph (merge the cloned body)
        assert!(result.changed);
        assert!(result.nodes_added > 0);
    }

    // ── PassManager Tests ─────────────────────────────────────────────

    #[test]
    fn test_pass_manager_runs_all_passes() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:10".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:20".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n3 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n3, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n3, EdgeKind::DataFlow).unwrap();

        let mut pm = PassManager::new();
        pm.add_pass(ConstantFolding)
            .add_pass(DeadCodeElimination)
            .add_pass(VerificationPass::minimal());

        let pipeline = pm.run(&mut scg);
        assert!(pipeline.pass_results.len() >= 3);
        assert!(pipeline.changed);
    }

    #[test]
    fn test_pass_manager_with_verification_between() {
        let mut scg = SCG::new();
        let _n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );

        let mut pm = PassManager::new().verify_between(true).stop_on_error(false);
        pm.add_pass(DeadCodeElimination);

        let pipeline = pm.run(&mut scg);
        // Should have DCE result + verification result (2 total)
        assert_eq!(pipeline.pass_results.len(), 2);
    }

    // ── PassResult Tests ──────────────────────────────────────────────

    #[test]
    fn test_pass_result_merge() {
        let mut r1 = PassResult::new("pass1");
        r1.changed = true;
        r1.nodes_removed = 2;
        r1.edges_removed = 3;

        let mut r2 = PassResult::new("pass2");
        r2.nodes_removed = 1;
        r2.nodes_added = 4;

        r1.merge(&r2);
        assert!(r1.changed);
        assert_eq!(r1.nodes_removed, 3);
        assert_eq!(r1.nodes_added, 4);
        assert_eq!(r1.edges_removed, 3);
    }

    #[test]
    fn test_pass_result_no_errors() {
        let r = PassResult::new("test");
        assert!(!r.has_errors());
        assert_eq!(r.errors.len(), 0);
    }

    // ── LICM (licm) Tests ─────────────────────────────────────────────

    #[test]
    fn test_licm_hoists_invariant_computation() {
        let mut scg = SCG::new();
        use crate::node::{AccessMode, AccessNode, AllocationNode, DeallocationNode};
        use crate::region::RegionId;

        // pre_header → header → body_add → exit
        //              ↑___________|
        // outer_const feeds body_add from outside loop
        let pre_header = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("pre".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let outer_const = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:10".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let header = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::LoopHeader,
                label: None,
            }),
            pp(),
        );
        let body_add = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let exit = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::LoopExit,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(pre_header, header, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(header, body_add, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(outer_const, body_add, EdgeKind::DataFlow)
            .unwrap();
        scg.add_edge(body_add, header, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(header, exit, EdgeKind::ControlFlow).unwrap();

        let hoisted = licm(&mut scg);
        assert_eq!(hoisted.len(), 1);
        assert!(hoisted.contains(&body_add));
    }

    #[test]
    fn test_licm_does_not_hoist_side_effect_nodes() {
        let mut scg = SCG::new();
        use crate::node::AllocationNode;
        use crate::region::RegionId;

        let pre_header = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("pre".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let header = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::LoopHeader,
                label: None,
            }),
            pp(),
        );
        // An allocation inside the loop — must NOT be hoisted
        let body_alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: RegionId::new(1),
                type_name: None,
            }),
            pp(),
        );
        let exit = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::LoopExit,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(pre_header, header, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(header, body_alloc, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(body_alloc, header, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(header, exit, EdgeKind::ControlFlow).unwrap();

        let hoisted = licm(&mut scg);
        assert!(hoisted.is_empty());
    }

    #[test]
    fn test_licm_does_not_hoist_loop_variant() {
        let mut scg = SCG::new();
        // body_add depends on header (a loop-body node), so it is NOT invariant
        let pre_header = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("pre".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let header = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::LoopHeader,
                label: None,
            }),
            pp(),
        );
        let body_add = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let exit = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::LoopExit,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(pre_header, header, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(header, body_add, EdgeKind::ControlFlow)
            .unwrap();
        // body_add depends on header via DataFlow → not invariant
        scg.add_edge(header, body_add, EdgeKind::DataFlow).unwrap();
        scg.add_edge(body_add, header, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(header, exit, EdgeKind::ControlFlow).unwrap();

        let hoisted = licm(&mut scg);
        assert!(hoisted.is_empty());
    }

    #[test]
    fn test_licm_no_loops_returns_empty() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let hoisted = licm(&mut scg);
        assert!(hoisted.is_empty());
    }

    #[test]
    fn test_licm_multiple_invariant_nodes() {
        let mut scg = SCG::new();
        let pre_header = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("pre".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let outer1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:5".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let outer2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:7".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let header = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::LoopHeader,
                label: None,
            }),
            pp(),
        );
        let inv1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let inv2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("sub".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let exit = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::LoopExit,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(pre_header, header, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(header, inv1, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(header, inv2, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(outer1, inv1, EdgeKind::DataFlow).unwrap();
        scg.add_edge(outer2, inv2, EdgeKind::DataFlow).unwrap();
        scg.add_edge(inv1, header, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(inv2, header, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(header, exit, EdgeKind::ControlFlow).unwrap();

        let hoisted = licm(&mut scg);
        assert_eq!(hoisted.len(), 2);
        assert!(hoisted.contains(&inv1));
        assert!(hoisted.contains(&inv2));
    }

    // ── Strength Reduction (strength_reduce) Tests ────────────────────

    #[test]
    fn test_strength_reduce_mul_by_power_of_two() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:8".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("load".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n3 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("mul".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n3, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n3, EdgeKind::DataFlow).unwrap();

        let replaced = strength_reduce(&mut scg);
        assert_eq!(replaced.len(), 1);
        let node = scg.get_node(n3).unwrap();
        match &node.payload {
            NodePayload::Computation(c) => assert_eq!(c.kind.label(), "shl_3"),
            _ => panic!("expected computation"),
        }
    }

    #[test]
    fn test_strength_reduce_div_by_power_of_two() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:16".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("load".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n3 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("div".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n3, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n3, EdgeKind::DataFlow).unwrap();

        let replaced = strength_reduce(&mut scg);
        assert_eq!(replaced.len(), 1);
        let node = scg.get_node(n3).unwrap();
        match &node.payload {
            NodePayload::Computation(c) => assert_eq!(c.kind.label(), "shr_4"),
            _ => panic!("expected computation"),
        }
    }

    #[test]
    fn test_strength_reduce_mod_by_power_of_two() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:8".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("load".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n3 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("mod".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n3, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n3, EdgeKind::DataFlow).unwrap();

        let replaced = strength_reduce(&mut scg);
        assert_eq!(replaced.len(), 1);
        let node = scg.get_node(n3).unwrap();
        match &node.payload {
            NodePayload::Computation(c) => assert_eq!(c.kind.label(), "and_7"),
            _ => panic!("expected computation"),
        }
    }

    #[test]
    fn test_strength_reduce_non_power_of_two_unchanged() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const.i32:3".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("load".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n3 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("mul".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n3, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n3, EdgeKind::DataFlow).unwrap();

        let replaced = strength_reduce(&mut scg);
        assert!(replaced.is_empty());
        // Operation should remain "mul"
        let node = scg.get_node(n3).unwrap();
        match &node.payload {
            NodePayload::Computation(c) => assert_eq!(c.kind.label(), "mul"),
            _ => panic!("expected computation"),
        }
    }

    #[test]
    fn test_strength_reduce_no_constant_predecessor() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("load".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("load".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n3 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("mul".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n3, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n3, EdgeKind::DataFlow).unwrap();

        let replaced = strength_reduce(&mut scg);
        assert!(replaced.is_empty());
    }

    // ── Tail Call Detection (detect_tail_calls) Tests ─────────────────

    #[test]
    fn test_detect_tail_calls_simple() {
        let mut scg = SCG::new();
        let call_node = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("call_foo".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(call_node, ret, EdgeKind::ControlFlow).unwrap();

        let detected = detect_tail_calls(&mut scg);
        assert_eq!(detected.len(), 1);
        assert!(detected.contains(&call_node));
        // Check the node was marked
        let node = scg.get_node(call_node).unwrap();
        match &node.payload {
            NodePayload::Computation(c) => assert!(c.tail_call),
            _ => panic!("expected computation"),
        }
    }

    #[test]
    fn test_detect_tail_calls_via_dataflow() {
        let mut scg = SCG::new();
        let call_node = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("call_bar".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(call_node, ret, EdgeKind::DataFlow).unwrap();

        let detected = detect_tail_calls(&mut scg);
        assert_eq!(detected.len(), 1);
    }

    #[test]
    fn test_detect_tail_calls_not_tail() {
        let mut scg = SCG::new();
        let call_node = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("call_foo".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let other = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("use".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        // Call feeds into "use", not directly into a return
        scg.add_edge(call_node, other, EdgeKind::DataFlow).unwrap();

        let detected = detect_tail_calls(&mut scg);
        assert!(detected.is_empty());
        // Not marked
        let node = scg.get_node(call_node).unwrap();
        match &node.payload {
            NodePayload::Computation(c) => assert!(!c.tail_call),
            _ => panic!("expected computation"),
        }
    }

    #[test]
    fn test_detect_tail_calls_non_call_node() {
        let mut scg = SCG::new();
        let add_node = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(add_node, ret, EdgeKind::ControlFlow).unwrap();

        let detected = detect_tail_calls(&mut scg);
        assert!(detected.is_empty());
    }

    #[test]
    fn test_detect_tail_calls_idempotent() {
        let mut scg = SCG::new();
        let call_node = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("fn_call_baz".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(call_node, ret, EdgeKind::ControlFlow).unwrap();

        let d1 = detect_tail_calls(&mut scg);
        assert_eq!(d1.len(), 1);
        let d2 = detect_tail_calls(&mut scg);
        assert!(d2.is_empty()); // Already marked
    }

    // ── Dead Region Elimination (dead_region_elim) Tests ──────────────

    #[test]
    fn test_dead_region_elim_removes_write_only_region() {
        use crate::node::{AccessMode, AccessNode, AllocationNode, DeallocationNode};
        use crate::region::RegionId;

        let mut scg = SCG::new();
        let region = RegionId::new(1);
        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: region,
                type_name: None,
            }),
            pp(),
        );
        let write = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );
        let dealloc = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: region,
            }),
            pp(),
        );
        scg.add_edge(alloc, dealloc, EdgeKind::Derivation).unwrap();
        scg.add_edge(alloc, write, EdgeKind::DataFlow).unwrap();

        let removed = dead_region_elim(&mut scg);
        // Should remove write, alloc, dealloc = 3 nodes
        assert_eq!(removed.len(), 3);
        assert!(scg.get_node(alloc).is_none());
        assert!(scg.get_node(dealloc).is_none());
        assert!(scg.get_node(write).is_none());
    }

    #[test]
    fn test_dead_region_elim_preserves_read_region() {
        use crate::node::{AccessMode, AccessNode, AllocationNode, DeallocationNode};
        use crate::region::RegionId;

        let mut scg = SCG::new();
        let region = RegionId::new(2);
        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: region,
                type_name: None,
            }),
            pp(),
        );
        let read = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id: region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );
        let dealloc = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: region,
            }),
            pp(),
        );
        scg.add_edge(alloc, dealloc, EdgeKind::Derivation).unwrap();
        scg.add_edge(alloc, read, EdgeKind::DataFlow).unwrap();

        let removed = dead_region_elim(&mut scg);
        assert!(removed.is_empty());
        assert!(scg.get_node(alloc).is_some());
        assert!(scg.get_node(dealloc).is_some());
        assert!(scg.get_node(read).is_some());
    }

    #[test]
    fn test_dead_region_elim_preserves_readwrite_region() {
        use crate::node::{AccessMode, AccessNode, AllocationNode, DeallocationNode};
        use crate::region::RegionId;

        let mut scg = SCG::new();
        let region = RegionId::new(3);
        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 32,
                align: 4,
                region_id: region,
                type_name: None,
            }),
            pp(),
        );
        let rw = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::ReadWrite,
                region_id: region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );
        let dealloc = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: region,
            }),
            pp(),
        );
        scg.add_edge(alloc, dealloc, EdgeKind::Derivation).unwrap();

        let removed = dead_region_elim(&mut scg);
        assert!(removed.is_empty());
    }

    #[test]
    fn test_dead_region_elim_no_dealloc_not_removed() {
        use crate::node::{AccessMode, AccessNode, AllocationNode};
        use crate::region::RegionId;

        let mut scg = SCG::new();
        let region = RegionId::new(4);
        // Alloc with no dealloc pair → no pair found, nothing removed
        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: region,
                type_name: None,
            }),
            pp(),
        );
        let write = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );

        let removed = dead_region_elim(&mut scg);
        assert!(removed.is_empty());
    }

    #[test]
    fn test_dead_region_elim_multiple_write_only_accesses() {
        use crate::node::{AccessMode, AccessNode, AllocationNode, DeallocationNode};
        use crate::region::RegionId;

        let mut scg = SCG::new();
        let region = RegionId::new(5);
        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 128,
                align: 16,
                region_id: region,
                type_name: None,
            }),
            pp(),
        );
        let w1 = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: region,
                offset: Some(0),
                access_size: Some(4),
            }),
            pp(),
        );
        let w2 = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: region,
                offset: Some(4),
                access_size: Some(4),
            }),
            pp(),
        );
        let dealloc = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: region,
            }),
            pp(),
        );
        scg.add_edge(alloc, dealloc, EdgeKind::Derivation).unwrap();

        let removed = dead_region_elim(&mut scg);
        // alloc + dealloc + w1 + w2 = 4
        assert_eq!(removed.len(), 4);
    }

    #[test]
    fn test_dead_region_elim_empty_graph() {
        let mut scg = SCG::new();
        let removed = dead_region_elim(&mut scg);
        assert!(removed.is_empty());
    }

    #[test]
    fn test_dead_region_elim_multiple_regions_one_dead_one_live() {
        use crate::node::{AccessMode, AccessNode, AllocationNode, DeallocationNode};
        use crate::region::RegionId;

        let mut scg = SCG::new();
        // Dead region (write-only)
        let dead_region = RegionId::new(10);
        let alloc1 = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: dead_region,
                type_name: None,
            }),
            pp(),
        );
        let write1 = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: dead_region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );
        let dealloc1 = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc1,
                region_id: dead_region,
            }),
            pp(),
        );
        scg.add_edge(alloc1, dealloc1, EdgeKind::Derivation)
            .unwrap();

        // Live region (has a read)
        let live_region = RegionId::new(11);
        let alloc2 = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: live_region,
                type_name: None,
            }),
            pp(),
        );
        let read2 = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id: live_region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );
        let dealloc2 = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc2,
                region_id: live_region,
            }),
            pp(),
        );
        scg.add_edge(alloc2, dealloc2, EdgeKind::Derivation)
            .unwrap();

        let removed = dead_region_elim(&mut scg);
        // Only the dead region should be removed: alloc1 + dealloc1 + write1 = 3
        assert_eq!(removed.len(), 3);
        assert!(scg.get_node(alloc2).is_some());
        assert!(scg.get_node(read2).is_some());
        assert!(scg.get_node(dealloc2).is_some());
    }

    #[test]
    fn test_dead_region_elim_alloc_only_no_accesses() {
        use crate::node::{AllocationNode, DeallocationNode};
        use crate::region::RegionId;

        let mut scg = SCG::new();
        let region = RegionId::new(20);
        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: region,
                type_name: None,
            }),
            pp(),
        );
        let dealloc = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: region,
            }),
            pp(),
        );
        scg.add_edge(alloc, dealloc, EdgeKind::Derivation).unwrap();

        // Region with no accesses at all → no reads → dead
        let removed = dead_region_elim(&mut scg);
        assert_eq!(removed.len(), 2); // alloc + dealloc
        assert!(scg.get_node(alloc).is_none());
        assert!(scg.get_node(dealloc).is_none());
    }

    #[test]
    fn test_dead_region_elim_write_and_read_different_regions() {
        use crate::node::{AccessMode, AccessNode, AllocationNode, DeallocationNode};
        use crate::region::RegionId;

        let mut scg = SCG::new();
        // Region A: write only (dead)
        let region_a = RegionId::new(30);
        let alloc_a = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 32,
                align: 4,
                region_id: region_a,
                type_name: None,
            }),
            pp(),
        );
        let write_a = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: region_a,
                offset: None,
                access_size: None,
            }),
            pp(),
        );
        let dealloc_a = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_a,
                region_id: region_a,
            }),
            pp(),
        );
        scg.add_edge(alloc_a, dealloc_a, EdgeKind::Derivation)
            .unwrap();

        // Region B: read (live)
        let region_b = RegionId::new(31);
        let alloc_b = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 32,
                align: 4,
                region_id: region_b,
                type_name: None,
            }),
            pp(),
        );
        let read_b = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id: region_b,
                offset: None,
                access_size: None,
            }),
            pp(),
        );
        let dealloc_b = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_b,
                region_id: region_b,
            }),
            pp(),
        );
        scg.add_edge(alloc_b, dealloc_b, EdgeKind::Derivation)
            .unwrap();

        let removed = dead_region_elim(&mut scg);
        assert_eq!(removed.len(), 3); // alloc_a + dealloc_a + write_a
                                      // Region B should survive
        assert!(scg.get_node(alloc_b).is_some());
        assert!(scg.get_node(read_b).is_some());
        assert!(scg.get_node(dealloc_b).is_some());
    }

    #[test]
    fn test_dead_region_elim_rem_call_in_operation_is_not_access() {
        use crate::node::{AllocationNode, DeallocationNode};
        use crate::region::RegionId;

        // Make sure computation nodes don't interfere with region analysis
        let mut scg = SCG::new();
        let region = RegionId::new(40);
        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: region,
                type_name: None,
            }),
            pp(),
        );
        let dealloc = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: region,
            }),
            pp(),
        );
        // A computation node that references the alloc but is NOT an Access node
        let comp = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("call_process".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(alloc, dealloc, EdgeKind::Derivation).unwrap();
        scg.add_edge(alloc, comp, EdgeKind::DataFlow).unwrap();

        // No Access nodes → no reads → dead region
        let removed = dead_region_elim(&mut scg);
        assert_eq!(removed.len(), 2); // alloc + dealloc
    }
}
