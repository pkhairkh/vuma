//! # Conversational Projection
//!
//! Generates natural-language descriptions of SCG structures and changes,
//! enabling a dialogue between the developer and the program's semantic model.
//!
//! ## Capabilities
//!
//! - **Describe** — produces a natural-language summary of a node's purpose,
//!   its behavioural descriptors, and its relationships.
//! - **Explain change** — takes an [`SCGDiff`] and narrates what changed in
//!   plain English (e.g. *"The authentication flow now requires 2FA for admin
//!   accounts"*).
//! - **Suggest modification** — given a high-level intent string (e.g.
//!   *"add rate limiting"*), proposes a set of [`SCGEdit`] operations that
//!   would implement that intent.
//!
//! ## Example
//!
//! ```
//! use vuma_projection::conversational::ConversationalProjection;
//! use vuma_projection::SCG;
//!
//! let proj = ConversationalProjection::new();
//! let scg = SCG::empty();
//! // let desc = proj.describe(&scg, node_id);
//! ```

use crate::diff::SCGDiff;
use crate::{BdKind, EdgeKind, NodeId, NodeKind, SCG};

// ── SCG Edit operations ───────────────────────────────────────────────────────

/// An edit operation that can be applied to an SCG.
///
/// These are the primitive operations produced by the suggestion engine and
/// consumed by the bidirectional editor.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SCGEdit {
    /// Add a new node to the graph.
    AddNode {
        /// Label for the new node.
        label: String,
        /// The kind of node to add.
        kind: NodeKind,
        /// Behavioural descriptors to attach.
        bds: Vec<String>,
    },
    /// Remove an existing node (and its edges) from the graph.
    RemoveNode {
        /// ID of the node to remove.
        node_id: NodeId,
    },
    /// Modify an existing edge (e.g. change its kind or target).
    ModifyEdge {
        /// ID of the edge to modify.
        edge_id: u64,
        /// New kind for the edge, if changed.
        new_kind: Option<EdgeKind>,
        /// New target node, if changed.
        new_target: Option<NodeId>,
    },
    /// Change a behavioural descriptor on a node.
    ChangeBD {
        /// ID of the node whose BD is being changed.
        node_id: NodeId,
        /// Name of the BD to add or remove.
        bd_name: String,
        /// The kind of BD.
        bd_kind: BdKind,
        /// If `true`, add the BD; if `false`, remove it.
        add: bool,
    },
}

// ── Conversational projection engine ──────────────────────────────────────────

/// The conversational projection engine.
///
/// Translates SCG structures and diffs into natural-language descriptions,
/// explanations, and modification suggestions.
#[derive(Debug, Clone)]
pub struct ConversationalProjection {
    /// How verbose the descriptions should be (1 = minimal, 5 = exhaustive).
    pub verbosity: u8,
}

impl Default for ConversationalProjection {
    fn default() -> Self {
        Self { verbosity: 3 }
    }
}

impl ConversationalProjection {
    /// Creates a new conversational projection engine with default verbosity.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new conversational projection engine with the given verbosity.
    pub fn with_verbosity(verbosity: u8) -> Self {
        Self {
            verbosity: verbosity.clamp(1, 5),
        }
    }

    // ── Describe ──────────────────────────────────────────────────────────────

    /// Produces a natural-language description of a specific SCG node.
    ///
    /// The description includes the node's role, its behavioural descriptors,
    /// and its position within the graph (incoming/outgoing edges).
    pub fn describe(&self, scg: &SCG, node_id: NodeId) -> String {
        let Some(node) = scg.get_node(node_id) else {
            return format!("Node {} does not exist in the graph.", node_id);
        };

        let mut parts: Vec<String> = Vec::new();

        // ── Role description ──────────────────────────────────────────────
        parts.push(format!(
            "\"{}\" is a {}.",
            node.label,
            self.describe_node_kind(&node.kind)
        ));

        // ── Behavioural descriptors ───────────────────────────────────────
        if !node.bds.is_empty() && self.verbosity >= 2 {
            let caps: Vec<&str> = node
                .bds
                .iter()
                .filter(|bd| bd.kind == BdKind::Capability)
                .map(|bd| bd.name.as_str())
                .collect();

            let mem: Vec<String> = node
                .bds
                .iter()
                .filter(|bd| bd.kind == BdKind::MemoryLayout)
                .map(|bd| {
                    if let Some(ref p) = bd.parameter {
                        format!("{}({})", bd.name, p)
                    } else {
                        bd.name.clone()
                    }
                })
                .collect();

            let rels: Vec<String> = node
                .bds
                .iter()
                .filter(|bd| bd.kind == BdKind::Relation)
                .map(|bd| bd.name.clone())
                .collect();

            if !caps.is_empty() {
                parts.push(format!("It has the following capabilities: {}.", caps.join(", ")));
            }
            if !mem.is_empty() {
                parts.push(format!(
                    "Its memory layout properties are: {}.",
                    mem.join(", ")
                ));
            }
            if !rels.is_empty() {
                parts.push(format!(
                    "It has relational properties: {}.",
                    rels.join(", ")
                ));
            }
        }

        // ── Edge relationships ────────────────────────────────────────────
        if self.verbosity >= 3 {
            let outgoing = scg.outgoing_edges(node_id);
            let incoming = scg.incoming_edges(node_id);

            if !outgoing.is_empty() {
                let data_targets: Vec<String> = outgoing
                    .iter()
                    .filter(|e| e.kind == EdgeKind::DataFlow)
                    .map(|e| {
                        scg.get_node(e.target)
                            .map(|n| n.label.clone())
                            .unwrap_or_else(|| format!("node_{}", e.target))
                    })
                    .collect();
                let call_targets: Vec<String> = outgoing
                    .iter()
                    .filter(|e| e.kind == EdgeKind::Call)
                    .map(|e| {
                        scg.get_node(e.target)
                            .map(|n| n.label.clone())
                            .unwrap_or_else(|| format!("node_{}", e.target))
                    })
                    .collect();
                let msg_targets: Vec<String> = outgoing
                    .iter()
                    .filter(|e| e.kind == EdgeKind::Message)
                    .map(|e| {
                        scg.get_node(e.target)
                            .map(|n| n.label.clone())
                            .unwrap_or_else(|| format!("node_{}", e.target))
                    })
                    .collect();

                if !data_targets.is_empty() {
                    parts.push(format!(
                        "It sends data to: {}.",
                        data_targets.join(", ")
                    ));
                }
                if !call_targets.is_empty() {
                    parts.push(format!(
                        "It calls: {}.",
                        call_targets.join(", ")
                    ));
                }
                if !msg_targets.is_empty() {
                    parts.push(format!(
                        "It sends messages to: {}.",
                        msg_targets.join(", ")
                    ));
                }
            }

            if !incoming.is_empty() {
                let sources: Vec<String> = incoming
                    .iter()
                    .map(|e| {
                        scg.get_node(e.source)
                            .map(|n| n.label.clone())
                            .unwrap_or_else(|| format!("node_{}", e.source))
                    })
                    .collect();
                parts.push(format!(
                    "It receives input from: {}.",
                    sources.join(", ")
                ));
            }
        }

        // ── Region membership ─────────────────────────────────────────────
        if self.verbosity >= 4 && !node.regions.is_empty() {
            let region_names: Vec<String> = node
                .regions
                .iter()
                .filter_map(|&rid| scg.get_region(rid).map(|r| r.name.clone()))
                .collect();
            if !region_names.is_empty() {
                parts.push(format!(
                    "It belongs to the following regions: {}.",
                    region_names.join(", ")
                ));
            }
        }

        parts.join(" ")
    }

    /// Returns a human-readable description of a [`NodeKind`].
    fn describe_node_kind(&self, kind: &NodeKind) -> &'static str {
        match kind {
            NodeKind::Function => "function that performs a computation",
            NodeKind::Value => "data value stored in the program",
            NodeKind::MessageSend => "message-sending operation",
            NodeKind::MessageReceive => "message-receiving operation",
            NodeKind::Merge => "control-flow merge point",
            NodeKind::Effect => "side-effecting operation",
            NodeKind::Module => "module or namespace boundary",
        }
    }

    // ── Explain change ────────────────────────────────────────────────────────

    /// Explains an [`SCGDiff`] in natural language.
    ///
    /// Produces sentences like *"A new function `rate_limiter` was added"* or
    /// *"The edge from `auth` to `session` was changed from data-flow to message"*.
    pub fn explain_change(&self, diff: &SCGDiff) -> String {
        let mut parts: Vec<String> = Vec::new();

        // ── Added nodes ───────────────────────────────────────────────────
        for node in &diff.added_nodes {
            parts.push(format!(
                "A new {} `{}` was added.",
                self.describe_node_kind(&node.kind),
                node.label
            ));
        }

        // ── Removed nodes ─────────────────────────────────────────────────
        for node in &diff.removed_nodes {
            parts.push(format!(
                "The {} `{}` was removed.",
                self.describe_node_kind(&node.kind),
                node.label
            ));
        }

        // ── Modified nodes ────────────────────────────────────────────────
        for change in &diff.modified_nodes {
            parts.push(format!(
                "The node `{}` was modified (changes to {} behavioural descriptor(s)).",
                change.label,
                change.bd_changes.len()
            ));

            for bd_change in &change.bd_changes {
                if bd_change.added {
                    parts.push(format!(
                        "  - BD `{}` (kind: {:?}) was added.",
                        bd_change.name, bd_change.kind
                    ));
                } else {
                    parts.push(format!(
                        "  - BD `{}` (kind: {:?}) was removed.",
                        bd_change.name, bd_change.kind
                    ));
                }
            }
        }

        // ── Added edges ───────────────────────────────────────────────────
        for edge in &diff.added_edges {
            parts.push(format!(
                "A {:?} edge was added from node {} to node {}.",
                edge.kind, edge.source, edge.target
            ));
        }

        // ── Removed edges ─────────────────────────────────────────────────
        for edge in &diff.removed_edges {
            parts.push(format!(
                "A {:?} edge was removed (was from node {} to node {}).",
                edge.kind, edge.source, edge.target
            ));
        }

        // ── Modified BDs ──────────────────────────────────────────────────
        for bd_change in &diff.modified_bds {
            if bd_change.added {
                parts.push(format!(
                    "The behavioural descriptor `{}` was added to node {}.",
                    bd_change.name, bd_change.node_id
                ));
            } else {
                parts.push(format!(
                    "The behavioural descriptor `{}` was removed from node {}.",
                    bd_change.name, bd_change.node_id
                ));
            }
        }

        if parts.is_empty() {
            return "No changes were detected.".to_string();
        }

        parts.join(" ")
    }

    // ── Suggest modification ──────────────────────────────────────────────────

    /// Suggests a set of [`SCGEdit`] operations to implement the given intent.
    ///
    /// The `intent` is a free-form natural-language string such as
    /// `"add rate limiting"` or `"make auth_handler thread-safe"`. The engine
    /// performs simple keyword matching in the current implementation; a future
    /// version will integrate with the VUMA LLM backend for more intelligent
    /// suggestions.
    pub fn suggest_modification(&self, intent: &str) -> Vec<SCGEdit> {
        let intent_lower = intent.to_lowercase();

        let mut edits: Vec<SCGEdit> = Vec::new();

        // ── Keyword-based heuristics ──────────────────────────────────────
        // TODO: Replace with LLM-backed suggestion engine.

        if intent_lower.contains("rate limit") || intent_lower.contains("throttle") {
            edits.push(SCGEdit::AddNode {
                label: "rate_limiter".into(),
                kind: NodeKind::Effect,
                bds: vec!["SideEffect".into()],
            });
            edits.push(SCGEdit::ChangeBD {
                node_id: 0, // placeholder — real implementation would resolve target
                bd_name: "Bounded".into(),
                bd_kind: BdKind::Capability,
                add: true,
            });
        }

        if intent_lower.contains("thread-safe") || intent_lower.contains("send") {
            edits.push(SCGEdit::ChangeBD {
                node_id: 0,
                bd_name: "Send".into(),
                bd_kind: BdKind::Capability,
                add: true,
            });
            edits.push(SCGEdit::ChangeBD {
                node_id: 0,
                bd_name: "Sync".into(),
                bd_kind: BdKind::Capability,
                add: true,
            });
        }

        if intent_lower.contains("2fa") || intent_lower.contains("two-factor") {
            edits.push(SCGEdit::AddNode {
                label: "verify_2fa".into(),
                kind: NodeKind::Function,
                bds: vec!["RequiresAuth".into()],
            });
        }

        if intent_lower.contains("log") || intent_lower.contains("audit") {
            edits.push(SCGEdit::AddNode {
                label: "audit_log".into(),
                kind: NodeKind::Effect,
                bds: vec!["SideEffect".into()],
            });
        }

        if intent_lower.contains("remove") || intent_lower.contains("delete") {
            // Can't know which node without more context; return a placeholder.
            edits.push(SCGEdit::RemoveNode { node_id: 0 });
        }

        if edits.is_empty() {
            // Fallback: no recognised intent.
            edits.push(SCGEdit::AddNode {
                label: format!("new_node_for_{}", intent.replace(' ', "_")),
                kind: NodeKind::Function,
                bds: vec![],
            });
        }

        edits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BehaviouralDescriptor, BdKind, SCGEdge, SCGNode};

    fn sample_scg() -> SCG {
        SCG {
            nodes: vec![SCGNode {
                id: 1,
                label: "auth_handler".into(),
                kind: NodeKind::Function,
                bds: vec![
                    BehaviouralDescriptor {
                        id: 100,
                        name: "Send".into(),
                        kind: BdKind::Capability,
                        parameter: None,
                    },
                ],
                regions: vec![],
            }],
            edges: vec![SCGEdge {
                id: 10,
                source: 1,
                target: 2,
                kind: EdgeKind::DataFlow,
            }],
            regions: vec![],
        }
    }

    #[test]
    fn describe_existing_node() {
        let scg = sample_scg();
        let proj = ConversationalProjection::new();
        let desc = proj.describe(&scg, 1);
        assert!(desc.contains("auth_handler"));
        assert!(desc.contains("function"));
    }

    #[test]
    fn describe_nonexistent_node() {
        let scg = sample_scg();
        let proj = ConversationalProjection::new();
        let desc = proj.describe(&scg, 999);
        assert!(desc.contains("does not exist"));
    }

    #[test]
    fn suggest_rate_limiting() {
        let proj = ConversationalProjection::new();
        let edits = proj.suggest_modification("add rate limiting to the API");
        assert!(!edits.is_empty());
        assert!(edits.iter().any(|e| matches!(e, SCGEdit::AddNode { label, .. } if label == "rate_limiter")));
    }

    #[test]
    fn suggest_thread_safety() {
        let proj = ConversationalProjection::new();
        let edits = proj.suggest_modification("make auth_handler thread-safe");
        assert!(edits.iter().any(|e| matches!(e, SCGEdit::ChangeBD { bd_name, .. } if bd_name == "Send")));
    }
}
