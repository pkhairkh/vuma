//! SCG Structured Output for LLMs
//!
//! This module provides LLM-friendly serialization of the Semantic Computation
//! Graph. Unlike the raw serde-based JSON dump in `serialize`, these outputs
//! are specifically designed for consumption by large language models:
//!
//! - **`to_json()`** — Clean, structured JSON with nodes, edges, functions,
//!   regions, and type information organized for easy parsing and reasoning.
//! - **`to_text()`** — Human-readable text with function-by-function breakdown,
//!   node listings with inputs/outputs, and data flow descriptions.
//!
//! # Design Principles
//!
//! 1. **Minimal redundancy**: IDs are used for cross-referencing, but key
//!    information is inlined so LLMs don't need to resolve references.
//! 2. **Function-centric**: Functions are first-class citizens since LLMs
//!    reason best about code in terms of functions.
//! 3. **Type information**: Every node includes its type signature so LLMs
//!    can reason about type flow.
//! 4. **Direction clarity**: Edges clearly indicate data flow direction.

use serde::{Deserialize, Serialize};

use crate::callgraph::CallGraph;
use crate::edge::EdgeKind;
use crate::graph::SCG;
use crate::node::{
    AccessMode, NodeData, NodeId, NodePayload,
    ProgramPoint,
};

// ── LLM-friendly JSON types ────────────────────────────────────────────

/// LLM-friendly JSON representation of an SCG node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmNode {
    /// The node's unique identifier.
    pub id: u64,
    /// The semantic type of this node (e.g., "Computation", "Allocation").
    pub node_type: String,
    /// A human-readable description of the operation this node performs.
    pub operation: String,
    /// The result type of this node, if applicable.
    pub result_type: Option<String>,
    /// IDs of nodes that flow data into this node (inputs).
    pub inputs: Vec<u64>,
    /// IDs of nodes that this node flows data to (outputs).
    pub outputs: Vec<u64>,
    /// Source location, if available.
    pub source_location: Option<LlmSourceLocation>,
    /// The function this node belongs to, if any.
    pub function: Option<String>,
}

/// LLM-friendly source location.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmSourceLocation {
    pub file: Option<String>,
    pub line: Option<u64>,
    pub column: Option<u64>,
}

/// LLM-friendly JSON representation of an SCG edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmEdge {
    /// The edge's unique identifier.
    pub id: u64,
    /// Source node ID.
    pub from: u64,
    /// Target node ID.
    pub to: u64,
    /// The kind of edge (e.g., "DataFlow", "ControlFlow", "Derivation").
    pub kind: String,
    /// An optional label for the edge.
    pub label: Option<String>,
}

/// LLM-friendly JSON representation of a function in the SCG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmFunction {
    /// The function's entry node ID.
    pub entry_node_id: u64,
    /// The function's return node ID, if known.
    pub return_node_id: Option<u64>,
    /// The function's name/label, if available.
    pub name: Option<String>,
    /// IDs of nodes belonging to this function.
    pub nodes: Vec<u64>,
    /// IDs of functions this function calls.
    pub calls: Vec<LlmCallTarget>,
    /// IDs of functions that call this function.
    pub called_by: Vec<u64>,
    /// Whether this function is recursive.
    pub is_recursive: bool,
}

/// A call target within a function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmCallTarget {
    /// The entry node ID of the callee.
    pub callee_entry_node_id: u64,
    /// The callee's name, if available.
    pub callee_name: Option<String>,
}

/// LLM-friendly JSON representation of a memory region.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRegion {
    /// The region's unique identifier.
    pub id: u64,
    /// The deployment target (e.g., "Heap", "Stack").
    pub deployment_target: String,
    /// Whether this region is a security boundary.
    pub security_boundary: bool,
    /// IDs of nodes in this region.
    pub nodes: Vec<u64>,
}

/// The top-level LLM-friendly JSON representation of an SCG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmScgJson {
    /// Summary statistics.
    pub summary: LlmSummary,
    /// All nodes in the graph.
    pub nodes: Vec<LlmNode>,
    /// All edges in the graph.
    pub edges: Vec<LlmEdge>,
    /// All functions in the graph.
    pub functions: Vec<LlmFunction>,
    /// All regions in the graph.
    pub regions: Vec<LlmRegion>,
}

/// Summary statistics for the SCG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmSummary {
    /// Total number of nodes.
    pub total_nodes: usize,
    /// Total number of edges.
    pub total_edges: usize,
    /// Total number of functions.
    pub total_functions: usize,
    /// Total number of regions.
    pub total_regions: usize,
    /// Breakdown of nodes by type.
    pub node_type_counts: std::collections::BTreeMap<String, usize>,
}

// ── Implementation ─────────────────────────────────────────────────────

impl SCG {
    /// Produces a clean, LLM-friendly JSON representation of the graph.
    ///
    /// The output includes:
    /// - Nodes with their types, operations, and connections
    /// - Edges with data flow direction
    /// - Function boundaries with caller/callee information
    /// - Type information for each node
    /// - Region information
    ///
    /// # Example
    ///
    /// ```rust
    /// use vuma_scg::*;
    /// let mut scg = SCG::new();
    /// let n1 = scg.add_node(
    ///     NodeType::Computation,
    ///     NodePayload::Computation(ComputationNode {
    ///         operation: "add".to_string(),
    ///         result_type: Some("i32".to_string()),
    ///         tail_call: false,
    ///     }),
    ///     ProgramPoint { file: None, line: None, column: None, offset: None },
    /// );
    /// let json = scg.to_json();
    /// assert!(json.contains("Computation"));
    /// ```
    pub fn to_json(&self) -> String {
        let llm = self.build_llm_representation();
        serde_json::to_string_pretty(&llm).unwrap_or_else(|e| {
            format!(r#"{{"error": "JSON serialization failed: {}"}}"#, e)
        })
    }

    /// Produces a human/LLM-readable text representation of the graph.
    ///
    /// The output includes:
    /// - Function-by-function breakdown
    /// - Node listing with inputs/outputs
    /// - Data flow description
    /// - Region information
    ///
    /// # Example
    ///
    /// ```rust
    /// use vuma_scg::*;
    /// let mut scg = SCG::new();
    /// let n1 = scg.add_node(
    ///     NodeType::Computation,
    ///     NodePayload::Computation(ComputationNode {
    ///         operation: "add".to_string(),
    ///         result_type: Some("i32".to_string()),
    ///         tail_call: false,
    ///     }),
    ///     ProgramPoint { file: None, line: None, column: None, offset: None },
    /// );
    /// let text = scg.to_text();
    /// assert!(text.contains("Computation"));
    /// ```
    pub fn to_text(&self) -> String {
        let llm = self.build_llm_representation();
        format_llm_as_text(&llm)
    }

    /// Build the LLM-friendly intermediate representation.
    fn build_llm_representation(&self) -> LlmScgJson {
        let call_graph = CallGraph::build(self);

        // Build a mapping from NodeId to function name
        let mut node_to_function: std::collections::HashMap<NodeId, String> =
            std::collections::HashMap::new();
        for fid in call_graph.functions() {
            let func_name = call_graph
                .function_label(fid)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("func_{}", fid.0.as_u64()));

            // Walk function entry via ControlFlow to find all nodes in this function
            let mut visited = hashbrown::HashSet::new();
            let mut queue = std::collections::VecDeque::new();
            queue.push_back(fid.0);
            visited.insert(fid.0);

            while let Some(current) = queue.pop_front() {
                node_to_function.insert(current, func_name.clone());
                if let Some(succs) = self.successors(current) {
                    for succ in succs {
                        if visited.insert(succ) {
                            // Only follow ControlFlow edges within the function
                            let is_cf = self.edges().any(|e| {
                                e.source == current
                                    && e.target == succ
                                    && matches!(e.kind, EdgeKind::ControlFlow)
                            });
                            let is_df = self.edges().any(|e| {
                                e.source == current
                                    && e.target == succ
                                    && matches!(e.kind, EdgeKind::DataFlow)
                            });
                            // Include nodes reachable via ControlFlow or DataFlow
                            // but stop at Call/Return edges (they cross function boundaries)
                            let is_call_or_return = self.edges().any(|e| {
                                e.source == current
                                    && e.target == succ
                                    && matches!(e.kind, EdgeKind::Call { .. } | EdgeKind::Return { .. })
                            });
                            if (is_cf || is_df) && !is_call_or_return {
                                queue.push_back(succ);
                            }
                        }
                    }
                }
            }
        }

        // Build input/output adjacency from edges
        let mut node_inputs: std::collections::HashMap<u64, Vec<u64>> =
            std::collections::HashMap::new();
        let mut node_outputs: std::collections::HashMap<u64, Vec<u64>> =
            std::collections::HashMap::new();

        for edge in self.edges() {
            node_inputs
                .entry(edge.target.as_u64())
                .or_default()
                .push(edge.source.as_u64());
            node_outputs
                .entry(edge.source.as_u64())
                .or_default()
                .push(edge.target.as_u64());
        }

        // Build nodes
        let nodes: Vec<LlmNode> = self
            .nodes()
            .map(|n| LlmNode {
                id: n.id.as_u64(),
                node_type: format!("{}", n.node_type),
                operation: node_operation(n),
                result_type: node_result_type(n),
                inputs: node_inputs
                    .get(&n.id.as_u64())
                    .cloned()
                    .unwrap_or_default(),
                outputs: node_outputs
                    .get(&n.id.as_u64())
                    .cloned()
                    .unwrap_or_default(),
                source_location: format_source_location(&n.program_point),
                function: node_to_function.get(&n.id).cloned(),
            })
            .collect();

        // Build edges
        let edges: Vec<LlmEdge> = self
            .edges()
            .map(|e| LlmEdge {
                id: e.id.as_u64(),
                from: e.source.as_u64(),
                to: e.target.as_u64(),
                kind: format!("{}", e.kind),
                label: e.label.clone(),
            })
            .collect();

        // Build functions
        let mut functions = Vec::new();
        for fid in call_graph.functions() {
            let name = call_graph
                .function_label(fid)
                .map(|s| s.to_string());
            let return_node_id = call_graph.function_return(fid).map(|n| n.as_u64());

            // Collect nodes belonging to this function
            let fallback_name = format!("func_{}", fid.0.as_u64());
            let expected_name = name.as_deref().unwrap_or(&fallback_name);
            let func_nodes: Vec<u64> = node_to_function
                .iter()
                .filter(|(_, fname)| *fname == expected_name)
                .map(|(nid, _)| nid.as_u64())
                .collect();

            let calls: Vec<LlmCallTarget> = call_graph
                .callees(&fid)
                .iter()
                .map(|cge| LlmCallTarget {
                    callee_entry_node_id: cge.callee.0.as_u64(),
                    callee_name: call_graph.function_label(&cge.callee).map(|s| s.to_string()),
                })
                .collect();

            let called_by: Vec<u64> = call_graph
                .callers(&fid)
                .iter()
                .map(|caller_fid| caller_fid.0.as_u64())
                .collect();

            functions.push(LlmFunction {
                entry_node_id: fid.0.as_u64(),
                return_node_id,
                name,
                nodes: func_nodes,
                calls,
                called_by,
                is_recursive: call_graph.is_recursive(&fid),
            });
        }

        // Build regions
        let regions: Vec<LlmRegion> = self
            .regions()
            .map(|r| LlmRegion {
                id: r.id.as_u64(),
                deployment_target: format!("{}", r.deployment_target),
                security_boundary: r.security_boundary,
                nodes: r.iter_nodes().map(|n| n.as_u64()).collect(),
            })
            .collect();

        // Build summary
        let mut node_type_counts = std::collections::BTreeMap::new();
        for node in self.nodes() {
            *node_type_counts
                .entry(format!("{}", node.node_type))
                .or_insert(0) += 1;
        }

        let summary = LlmSummary {
            total_nodes: self.node_count(),
            total_edges: self.edge_count(),
            total_functions: call_graph.function_count(),
            total_regions: self.region_count(),
            node_type_counts,
        };

        LlmScgJson {
            summary,
            nodes,
            edges,
            functions,
            regions,
        }
    }
}

// ── Text formatting ────────────────────────────────────────────────────

/// Format the LLM representation as a human-readable text document.
fn format_llm_as_text(llm: &LlmScgJson) -> String {
    let mut out = String::with_capacity(8192);

    // Summary
    out.push_str("=== SCG Semantic Computation Graph ===\n\n");
    out.push_str(&format!(
        "Summary: {} nodes, {} edges, {} functions, {} regions\n",
        llm.summary.total_nodes,
        llm.summary.total_edges,
        llm.summary.total_functions,
        llm.summary.total_regions,
    ));

    if !llm.summary.node_type_counts.is_empty() {
        out.push_str("Node types: ");
        let parts: Vec<String> = llm
            .summary
            .node_type_counts
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        out.push_str(&parts.join(", "));
        out.push('\n');
    }
    out.push('\n');

    // Functions
    if !llm.functions.is_empty() {
        out.push_str("--- Functions ---\n\n");
        for func in &llm.functions {
            let default_name = format!("func_{}", func.entry_node_id);
            let name = func
                .name
                .as_deref()
                .unwrap_or(&default_name);
            out.push_str(&format!("Function: {}\n", name));
            out.push_str(&format!(
                "  Entry: node_{}  Return: {}\n",
                func.entry_node_id,
                func.return_node_id
                    .map(|id| format!("node_{}", id))
                    .unwrap_or_else(|| "?".to_string())
            ));
            out.push_str(&format!(
                "  Recursive: {}\n",
                if func.is_recursive { "yes" } else { "no" }
            ));
            if !func.nodes.is_empty() {
                out.push_str(&format!(
                    "  Nodes: {}\n",
                    func.nodes
                        .iter()
                        .map(|id| format!("node_{}", id))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if !func.calls.is_empty() {
                out.push_str("  Calls:\n");
                for call in &func.calls {
                    let default_callee = format!("func_{}", call.callee_entry_node_id);
                    let callee_name = call
                        .callee_name
                        .as_deref()
                        .unwrap_or(&default_callee);
                    out.push_str(&format!("    -> {} (node_{})\n", callee_name, call.callee_entry_node_id));
                }
            }
            if !func.called_by.is_empty() {
                out.push_str(&format!(
                    "  Called by: {}\n",
                    func.called_by
                        .iter()
                        .map(|id| format!("func_{}", id))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            out.push('\n');
        }
    }

    // Nodes
    if !llm.nodes.is_empty() {
        out.push_str("--- Nodes ---\n\n");
        for node in &llm.nodes {
            out.push_str(&format!(
                "Node_{} [{}]: {}\n",
                node.id, node.node_type, node.operation
            ));
            if let Some(ref rt) = node.result_type {
                out.push_str(&format!("  Result type: {}\n", rt));
            }
            if let Some(ref func) = node.function {
                out.push_str(&format!("  Function: {}\n", func));
            }
            if let Some(ref loc) = node.source_location {
                if loc.file.is_some() || loc.line.is_some() {
                    let file = loc.file.as_deref().unwrap_or("?");
                    let line = loc.line.map(|l| l.to_string()).unwrap_or_else(|| "?".to_string());
                    out.push_str(&format!("  Location: {}:{}\n", file, line));
                }
            }
            if !node.inputs.is_empty() {
                out.push_str(&format!(
                    "  Inputs: {}\n",
                    node.inputs
                        .iter()
                        .map(|id| format!("node_{}", id))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if !node.outputs.is_empty() {
                out.push_str(&format!(
                    "  Outputs: {}\n",
                    node.outputs
                        .iter()
                        .map(|id| format!("node_{}", id))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            out.push('\n');
        }
    }

    // Edges (data flow description)
    if !llm.edges.is_empty() {
        out.push_str("--- Data Flow ---\n\n");
        for edge in &llm.edges {
            let label = edge
                .label
                .as_deref()
                .unwrap_or("");
            let label_suffix = if label.is_empty() {
                String::new()
            } else {
                format!(" ({})", label)
            };
            out.push_str(&format!(
                "node_{} --[{}]--> node_{}{}\n",
                edge.from, edge.kind, edge.to, label_suffix
            ));
        }
        out.push('\n');
    }

    // Regions
    if !llm.regions.is_empty() {
        out.push_str("--- Regions ---\n\n");
        for region in &llm.regions {
            out.push_str(&format!(
                "Region_{} [{}]{}\n",
                region.id,
                region.deployment_target,
                if region.security_boundary {
                    " [SECURITY BOUNDARY]"
                } else {
                    ""
                }
            ));
            if !region.nodes.is_empty() {
                out.push_str(&format!(
                    "  Nodes: {}\n",
                    region.nodes
                        .iter()
                        .map(|id| format!("node_{}", id))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            out.push('\n');
        }
    }

    out
}

// ── Node description helpers ───────────────────────────────────────────

/// Extract a human-readable operation description from a node.
fn node_operation(node: &NodeData) -> String {
    match &node.payload {
        NodePayload::Computation(c) => c.operation.clone(),
        NodePayload::Allocation(a) => {
            let tn = a
                .type_name
                .as_deref()
                .map(|t| format!(" {}", t))
                .unwrap_or_default();
            format!("alloc {}B align={}{}", a.size, a.align, tn)
        }
        NodePayload::Deallocation(_) => "dealloc".to_string(),
        NodePayload::Access(a) => {
            let mode = match a.mode {
                AccessMode::Read => "read",
                AccessMode::Write => "write",
                AccessMode::ReadWrite => "read_write",
            };
            let offset = a
                .offset
                .map(|o| format!("+{}", o))
                .unwrap_or_default();
            format!("{}{} @region_{}", mode, offset, a.region_id.as_u64())
        }
        NodePayload::Cast(c) => format!("cast {} -> {}", c.from_type, c.to_type),
        NodePayload::Effect(e) => format!("effect({})", e.effect_kind),
        NodePayload::Control(c) => {
            let lbl = c
                .label
                .as_deref()
                .map(|l| format!(" {}", l))
                .unwrap_or_default();
            format!("{:?}{}", c.kind, lbl)
        }
        NodePayload::Phantom(p) => format!("phantom({})", p.purpose),
        NodePayload::VTable(v) => format!("vtable({} for {})", v.trait_name, v.concrete_type),
        NodePayload::ClosureEnv(e) => format!("closure_env({:?})", e.captured_vars),
    }
}

/// Extract the result type from a node, if applicable.
fn node_result_type(node: &NodeData) -> Option<String> {
    match &node.payload {
        NodePayload::Computation(c) => c.result_type.clone(),
        NodePayload::Allocation(a) => a.type_name.clone(),
        NodePayload::Cast(c) => Some(c.to_type.clone()),
        NodePayload::Access(a) => a.access_size.map(|s| format!("{}B", s)),
        _ => None,
    }
}

/// Format a ProgramPoint as an LlmSourceLocation.
fn format_source_location(pp: &ProgramPoint) -> Option<LlmSourceLocation> {
    if pp.file.is_none() && pp.line.is_none() && pp.column.is_none() {
        None
    } else {
        Some(LlmSourceLocation {
            file: pp.file.clone(),
            line: pp.line,
            column: pp.column,
        })
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::EdgeKind;
    use crate::graph::SCG;
    use crate::node::{
        AllocationNode, ComputationNode, ControlKind, ControlNode, DeallocationNode,
        NodePayload, NodeType, ProgramPoint,
    };
    use crate::region::{DeploymentTarget, RegionId, SCGRegion};

    fn pp() -> ProgramPoint {
        ProgramPoint {
            file: Some("test.vu".to_string()),
            line: Some(1),
            column: Some(1),
            offset: None,
        }
    }

    #[test]
    fn test_to_json_empty_scg() {
        let scg = SCG::new();
        let json = scg.to_json();
        assert!(json.contains("summary"));
        assert!(json.contains("total_nodes"));
        let parsed: LlmScgJson = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.summary.total_nodes, 0);
        assert_eq!(parsed.summary.total_edges, 0);
    }

    #[test]
    fn test_to_json_simple_graph() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "mul".to_string(),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();

        let json = scg.to_json();
        let parsed: LlmScgJson = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.summary.total_nodes, 2);
        assert_eq!(parsed.summary.total_edges, 1);
        assert_eq!(parsed.nodes.len(), 2);
        assert_eq!(parsed.edges.len(), 1);

        // Check node operation
        let add_node = parsed.nodes.iter().find(|n| n.operation == "add").unwrap();
        assert_eq!(add_node.result_type, Some("i32".to_string()));
        assert!(add_node.outputs.contains(&n2.as_u64()));

        let mul_node = parsed.nodes.iter().find(|n| n.operation == "mul").unwrap();
        assert!(mul_node.inputs.contains(&n1.as_u64()));
    }

    #[test]
    fn test_to_json_with_function() {
        let mut scg = SCG::new();
        let entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("main".to_string()),
            }),
            pp(),
        );
        let comp = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "hello".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: Some("main".to_string()),
            }),
            pp(),
        );
        scg.add_edge(entry, comp, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(comp, ret, EdgeKind::ControlFlow).unwrap();

        let json = scg.to_json();
        let parsed: LlmScgJson = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.summary.total_functions, 1);
        let func = &parsed.functions[0];
        assert_eq!(func.name, Some("main".to_string()));
        assert_eq!(func.entry_node_id, entry.as_u64());
        assert_eq!(func.return_node_id, Some(ret.as_u64()));
    }

    #[test]
    fn test_to_json_with_region() {
        let mut scg = SCG::new();
        let rid = RegionId::new(1);
        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 256,
                align: 16,
                region_id: rid,
                type_name: Some("Buffer".to_string()),
            }),
            pp(),
        );
        let mut region = SCGRegion::new(rid, DeploymentTarget::Heap);
        region.add_node(alloc);
        scg.add_region(region);

        let json = scg.to_json();
        let parsed: LlmScgJson = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.regions.len(), 1);
        assert_eq!(parsed.regions[0].id, 1);
        assert_eq!(parsed.regions[0].deployment_target, "Heap");

        // Check the allocation node has result_type set from type_name
        let alloc_node = parsed
            .nodes
            .iter()
            .find(|n| n.node_type == "Allocation")
            .unwrap();
        assert_eq!(alloc_node.result_type, Some("Buffer".to_string()));
    }

    #[test]
    fn test_to_text_basic() {
        let mut scg = SCG::new();
        scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );

        let text = scg.to_text();
        assert!(text.contains("SCG Semantic Computation Graph"));
        assert!(text.contains("Computation"));
        assert!(text.contains("add"));
    }

    #[test]
    fn test_to_text_with_function() {
        let mut scg = SCG::new();
        let entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("my_func".to_string()),
            }),
            pp(),
        );
        let ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: Some("my_func".to_string()),
            }),
            pp(),
        );
        scg.add_edge(entry, ret, EdgeKind::ControlFlow).unwrap();

        let text = scg.to_text();
        assert!(text.contains("my_func"));
        assert!(text.contains("Functions"));
    }

    #[test]
    fn test_to_text_with_data_flow() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "source".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "sink".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();

        let text = scg.to_text();
        assert!(text.contains("Data Flow"));
        assert!(text.contains("DataFlow"));
    }

    #[test]
    fn test_to_json_round_trip() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "sub".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();

        let json = scg.to_json();
        let parsed: LlmScgJson = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.nodes.len(), 2);
        assert_eq!(parsed.edges.len(), 1);
        assert_eq!(parsed.summary.total_nodes, 2);
        assert_eq!(parsed.summary.total_edges, 1);

        // Verify type counts
        assert_eq!(parsed.summary.node_type_counts.get("Computation"), Some(&2));
    }

    #[test]
    fn test_to_json_alloc_dealloc_pair() {
        let mut scg = SCG::new();
        let rid = RegionId::new(1);
        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 1024,
                align: 8,
                region_id: rid,
                type_name: Some("MyStruct".to_string()),
            }),
            pp(),
        );
        let dealloc = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: rid,
            }),
            pp(),
        );
        scg.add_edge(alloc, dealloc, EdgeKind::Derivation).unwrap();

        let json = scg.to_json();
        let parsed: LlmScgJson = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.nodes.len(), 2);
        let alloc_node = parsed
            .nodes
            .iter()
            .find(|n| n.node_type == "Allocation")
            .unwrap();
        assert_eq!(alloc_node.operation, "alloc 1024B align=8 MyStruct");
        assert_eq!(alloc_node.result_type, Some("MyStruct".to_string()));

        let dealloc_node = parsed
            .nodes
            .iter()
            .find(|n| n.node_type == "Deallocation")
            .unwrap();
        assert_eq!(dealloc_node.operation, "dealloc");
        assert!(dealloc_node.inputs.contains(&alloc.as_u64()));
    }
}
