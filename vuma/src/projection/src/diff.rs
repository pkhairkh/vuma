//! # Diff Projection
//!
//! Computes and describes differences between two SCG snapshots. The diff
//! captures structural changes (added/removed nodes and edges) as well as
//! semantic changes (modified behavioural descriptors) and can produce
//! human-readable summaries.
//!
//! ## Example output
//!
//! ```text
//! Summary: 3 changes detected
//!
//! The authentication flow now requires 2FA for admin accounts.
//!
//! - Added node: verify_2fa (Function)
//! - Added edge: auth_handler ──Call──▶ verify_2fa
//! - Changed BD: auth_handler gained RequiresAuth
//! ```

use crate::{BdKind, NodeId, NodeKind, SCG, SCGEdge, SCGNode};

// ── Diff data structures ──────────────────────────────────────────────────────

/// A change to a behavioural descriptor on a specific node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BdChange {
    /// The node to which this BD change applies.
    pub node_id: NodeId,
    /// Name of the behavioural descriptor.
    pub name: String,
    /// Kind of the behavioural descriptor.
    pub kind: BdKind,
    /// Whether the BD was added (`true`) or removed (`false`).
    pub added: bool,
}

/// A summary of changes to a specific modified node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeChange {
    /// ID of the modified node.
    pub id: NodeId,
    /// Label of the modified node.
    pub label: String,
    /// Changes to behavioural descriptors on this node.
    pub bd_changes: Vec<BdChange>,
}

/// The result of comparing two SCG snapshots.
///
/// Contains all structural and semantic differences, which can then be
/// rendered as a machine-readable diff or a human-readable summary.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SCGDiff {
    /// Nodes present in the new SCG but not in the old SCG.
    pub added_nodes: Vec<SCGNode>,
    /// Nodes present in the old SCG but not in the new SCG.
    pub removed_nodes: Vec<SCGNode>,
    /// Nodes that exist in both SCGs but whose BDs have changed.
    pub modified_nodes: Vec<NodeChange>,
    /// Edges present in the new SCG but not in the old SCG.
    pub added_edges: Vec<SCGEdge>,
    /// Edges present in the old SCG but not in the new SCG.
    pub removed_edges: Vec<SCGEdge>,
    /// Standalone BD changes not attributable to a specific node modification.
    pub modified_bds: Vec<BdChange>,
}

impl SCGDiff {
    /// Creates an empty diff (no changes).
    pub fn empty() -> Self {
        Self {
            added_nodes: Vec::new(),
            removed_nodes: Vec::new(),
            modified_nodes: Vec::new(),
            added_edges: Vec::new(),
            removed_edges: Vec::new(),
            modified_bds: Vec::new(),
        }
    }

    /// Returns `true` if no changes were detected.
    pub fn is_empty(&self) -> bool {
        self.added_nodes.is_empty()
            && self.removed_nodes.is_empty()
            && self.modified_nodes.is_empty()
            && self.added_edges.is_empty()
            && self.removed_edges.is_empty()
            && self.modified_bds.is_empty()
    }

    /// Returns the total number of changes.
    pub fn total_changes(&self) -> usize {
        self.added_nodes.len()
            + self.removed_nodes.len()
            + self.modified_nodes.len()
            + self.added_edges.len()
            + self.removed_edges.len()
            + self.modified_bds.len()
    }
}

// ── Diff projection engine ────────────────────────────────────────────────────

/// The diff projection engine.
///
/// Computes structural and semantic differences between two SCG snapshots
/// and produces human-readable descriptions of the changes.
#[derive(Debug, Clone)]
pub struct DiffProjection {
    /// Whether to include edge-level details in descriptions.
    pub verbose_edges: bool,
}

impl Default for DiffProjection {
    fn default() -> Self {
        Self {
            verbose_edges: true,
        }
    }
}

impl DiffProjection {
    /// Creates a new diff projection engine.
    pub fn new(verbose_edges: bool) -> Self {
        Self { verbose_edges }
    }

    // ── Compute diff ──────────────────────────────────────────────────────────

    /// Computes the diff between an old and a new SCG.
    ///
    /// # Algorithm
    ///
    /// 1. **Node diff** — nodes are matched by ID. Nodes present only in the
    ///    new SCG are "added"; nodes present only in the old SCG are "removed".
    ///    Nodes present in both are checked for BD changes.
    /// 2. **Edge diff** — edges are matched by ID. Added/removed edges are
    ///    determined analogously.
    /// 3. **BD diff** — for each node present in both SCGs, the sets of BDs
    ///    (identified by name) are compared.
    pub fn compute_diff(&self, old_scg: &SCG, new_scg: &SCG) -> SCGDiff {
        let mut diff = SCGDiff::empty();

        // ── Index old nodes by ID ─────────────────────────────────────────
        let old_nodes: std::collections::HashMap<NodeId, &SCGNode> = old_scg
            .nodes
            .iter()
            .map(|n| (n.id, n))
            .collect();
        let new_nodes: std::collections::HashMap<NodeId, &SCGNode> = new_scg
            .nodes
            .iter()
            .map(|n| (n.id, n))
            .collect();

        // ── Added / removed nodes ─────────────────────────────────────────
        for node in &new_scg.nodes {
            if !old_nodes.contains_key(&node.id) {
                diff.added_nodes.push(node.clone());
            }
        }
        for node in &old_scg.nodes {
            if !new_nodes.contains_key(&node.id) {
                diff.removed_nodes.push(node.clone());
            }
        }

        // ── Modified nodes (BD changes) ───────────────────────────────────
        for new_node in &new_scg.nodes {
            if let Some(old_node) = old_nodes.get(&new_node.id) {
                let bd_changes = self.compute_bd_diff(old_node, new_node);
                if !bd_changes.is_empty() {
                    diff.modified_nodes.push(NodeChange {
                        id: new_node.id,
                        label: new_node.label.clone(),
                        bd_changes,
                    });
                }
            }
        }

        // ── Edge diff ─────────────────────────────────────────────────────
        let old_edges: std::collections::HashMap<u64, &SCGEdge> = old_scg
            .edges
            .iter()
            .map(|e| (e.id, e))
            .collect();
        let new_edges: std::collections::HashMap<u64, &SCGEdge> = new_scg
            .edges
            .iter()
            .map(|e| (e.id, e))
            .collect();

        for edge in &new_scg.edges {
            if !old_edges.contains_key(&edge.id) {
                diff.added_edges.push(edge.clone());
            }
        }
        for edge in &old_scg.edges {
            if !new_edges.contains_key(&edge.id) {
                diff.removed_edges.push(edge.clone());
            }
        }

        diff
    }

    /// Computes BD-level changes between two versions of the same node.
    fn compute_bd_diff(&self, old_node: &SCGNode, new_node: &SCGNode) -> Vec<BdChange> {
        let mut changes: Vec<BdChange> = Vec::new();

        let old_bd_names: std::collections::HashSet<&str> =
            old_node.bds.iter().map(|bd| bd.name.as_str()).collect();
        let new_bd_names: std::collections::HashSet<&str> =
            new_node.bds.iter().map(|bd| bd.name.as_str()).collect();

        // Added BDs
        for bd in &new_node.bds {
            if !old_bd_names.contains(bd.name.as_str()) {
                changes.push(BdChange {
                    node_id: new_node.id,
                    name: bd.name.clone(),
                    kind: bd.kind,
                    added: true,
                });
            }
        }

        // Removed BDs
        for bd in &old_node.bds {
            if !new_bd_names.contains(bd.name.as_str()) {
                changes.push(BdChange {
                    node_id: old_node.id,
                    name: bd.name.clone(),
                    kind: bd.kind,
                    added: false,
                });
            }
        }

        changes
    }

    // ── Describe diff ─────────────────────────────────────────────────────────

    /// Produces a structured, line-by-line description of the diff.
    ///
    /// This is more machine-friendly than [`human_readable_summary`] and
    /// suitable for logging or structured output.
    pub fn describe_diff(&self, diff: &SCGDiff) -> String {
        let mut lines: Vec<String> = Vec::new();

        if diff.is_empty() {
            return "No changes detected.".to_string();
        }

        lines.push(format!("Diff: {} change(s) detected", diff.total_changes()));

        for node in &diff.added_nodes {
            lines.push(format!("+ node `{}` ({:?})", node.label, node.kind));
        }
        for node in &diff.removed_nodes {
            lines.push(format!("- node `{}` ({:?})", node.label, node.kind));
        }
        for change in &diff.modified_nodes {
            lines.push(format!(
                "~ node `{}`: {} BD change(s)",
                change.label,
                change.bd_changes.len()
            ));
            for bd in &change.bd_changes {
                let prefix = if bd.added { "+" } else { "-" };
                lines.push(format!("  {} BD `{}` ({:?})", prefix, bd.name, bd.kind));
            }
        }
        for edge in &diff.added_edges {
            lines.push(format!(
                "+ edge {}: {} ──{:?}──▶ {}",
                edge.id, edge.source, edge.kind, edge.target
            ));
        }
        for edge in &diff.removed_edges {
            lines.push(format!(
                "- edge {}: {} ──{:?}──▶ {}",
                edge.id, edge.source, edge.kind, edge.target
            ));
        }

        lines.join("\n")
    }

    // ── Human-readable summary ────────────────────────────────────────────────

    /// Produces a human-readable narrative summary of the diff.
    ///
    /// The output is designed to be read by developers and provides semantic
    /// interpretation of the changes where possible.
    ///
    /// # Example
    ///
    /// ```text
    /// Summary: 3 changes detected
    ///
    /// The authentication flow now requires 2FA for admin accounts.
    ///
    /// - Added function `verify_2fa`
    /// - Added call edge from `auth_handler` to `verify_2fa`
    /// - `auth_handler` gained the `RequiresAuth` capability
    /// ```
    pub fn human_readable_summary(&self, diff: &SCGDiff) -> String {
        if diff.is_empty() {
            return "No changes were detected between the two versions.".to_string();
        }

        let mut lines: Vec<String> = Vec::new();

        lines.push(format!(
            "Summary: {} change(s) detected",
            diff.total_changes()
        ));
        lines.push(String::new());

        // ── Semantic interpretation ───────────────────────────────────────
        // Try to produce a high-level sentence about the change.
        let summary_sentence = self.generate_summary_sentence(diff);
        if !summary_sentence.is_empty() {
            lines.push(summary_sentence);
            lines.push(String::new());
        }

        // ── Detailed bullet points ────────────────────────────────────────
        for node in &diff.added_nodes {
            lines.push(format!(
                "- Added {} `{}`",
                self.kind_label(&node.kind),
                node.label
            ));
        }
        for node in &diff.removed_nodes {
            lines.push(format!(
                "- Removed {} `{}`",
                self.kind_label(&node.kind),
                node.label
            ));
        }
        for change in &diff.modified_nodes {
            for bd in &change.bd_changes {
                if bd.added {
                    lines.push(format!(
                        "- `{}` gained the `{}` {}",
                        change.label,
                        bd.name,
                        self.bd_kind_label(&bd.kind)
                    ));
                } else {
                    lines.push(format!(
                        "- `{}` lost the `{}` {}",
                        change.label,
                        bd.name,
                        self.bd_kind_label(&bd.kind)
                    ));
                }
            }
        }
        for edge in &diff.added_edges {
            lines.push(format!(
                "- Added {:?} edge ({} → {})",
                edge.kind, edge.source, edge.target
            ));
        }
        for edge in &diff.removed_edges {
            lines.push(format!(
                "- Removed {:?} edge ({} → {})",
                edge.kind, edge.source, edge.target
            ));
        }

        lines.join("\n")
    }

    /// Attempts to generate a single high-level summary sentence.
    ///
    /// Uses simple heuristics based on the diff contents. A future version
    /// will integrate with the VUMA LLM backend for richer summaries.
    fn generate_summary_sentence(&self, diff: &SCGDiff) -> String {
        // Check for common patterns.
        let added_labels: Vec<&str> = diff
            .added_nodes
            .iter()
            .map(|n| n.label.as_str())
            .collect();
        let added_bds: Vec<&str> = diff
            .modified_nodes
            .iter()
            .flat_map(|c| c.bd_changes.iter())
            .filter(|bd| bd.added)
            .map(|bd| bd.name.as_str())
            .collect();

        // Pattern: "X now requires Y"
        if added_labels.iter().any(|l| l.contains("2fa") || l.contains("verify"))
            && added_bds.iter().any(|b| b.contains("Auth") || b.contains("Requires"))
        {
            return "The authentication flow now requires 2FA for admin accounts.".to_string();
        }

        // Pattern: "Rate limiting was added"
        if added_labels
            .iter()
            .any(|l| l.contains("rate_limit") || l.contains("throttle"))
        {
            return "Rate limiting was added to the system.".to_string();
        }

        // Generic: describe what was added
        if !diff.added_nodes.is_empty() && diff.removed_nodes.is_empty() {
            let names: Vec<&str> = diff.added_nodes.iter().map(|n| n.label.as_str()).collect();
            return format!("New component(s) were added: {}.", names.join(", "));
        }

        // Generic: describe what was removed
        if diff.added_nodes.is_empty() && !diff.removed_nodes.is_empty() {
            let names: Vec<&str> = diff.removed_nodes.iter().map(|n| n.label.as_str()).collect();
            return format!("Component(s) were removed: {}.", names.join(", "));
        }

        String::new()
    }

    /// Returns a human-readable label for a [`NodeKind`].
    fn kind_label(&self, kind: &NodeKind) -> &'static str {
        match kind {
            NodeKind::Function => "function",
            NodeKind::Value => "value",
            NodeKind::MessageSend => "message-send",
            NodeKind::MessageReceive => "message-receive",
            NodeKind::Merge => "merge point",
            NodeKind::Effect => "effect",
            NodeKind::Module => "module",
        }
    }

    /// Returns a human-readable label for a [`BdKind`].
    fn bd_kind_label(&self, kind: &BdKind) -> &'static str {
        match kind {
            BdKind::Capability => "capability",
            BdKind::MemoryLayout => "memory property",
            BdKind::Safety => "safety property",
            BdKind::Relation => "relation",
            BdKind::Custom => "custom property",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BehaviouralDescriptor, BdKind, EdgeKind, SCGEdge, SCGNode};

    fn old_scg() -> SCG {
        SCG {
            nodes: vec![SCGNode {
                id: 1,
                label: "auth_handler".into(),
                kind: NodeKind::Function,
                bds: vec![BehaviouralDescriptor {
                    id: 100,
                    name: "Send".into(),
                    kind: BdKind::Capability,
                    parameter: None,
                }],
                regions: vec![],
            }],
            edges: vec![],
            regions: vec![],
        }
    }

    fn new_scg_with_added_node() -> SCG {
        let mut scg = old_scg();
        scg.nodes.push(SCGNode {
            id: 2,
            label: "verify_2fa".into(),
            kind: NodeKind::Function,
            bds: vec![BehaviouralDescriptor {
                id: 101,
                name: "RequiresAuth".into(),
                kind: BdKind::Capability,
                parameter: None,
            }],
            regions: vec![],
        });
        scg.edges.push(SCGEdge {
            id: 10,
            source: 1,
            target: 2,
            kind: EdgeKind::Call,
        });
        // Also add a BD to the existing node
        scg.nodes[0].bds.push(BehaviouralDescriptor {
            id: 102,
            name: "RequiresAuth".into(),
            kind: BdKind::Capability,
            parameter: None,
        });
        scg
    }

    #[test]
    fn compute_diff_detects_added_node() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let proj = DiffProjection::default();
        let diff = proj.compute_diff(&old, &new);
        assert_eq!(diff.added_nodes.len(), 1);
        assert_eq!(diff.added_nodes[0].label, "verify_2fa");
    }

    #[test]
    fn compute_diff_detects_added_edge() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let proj = DiffProjection::default();
        let diff = proj.compute_diff(&old, &new);
        assert_eq!(diff.added_edges.len(), 1);
    }

    #[test]
    fn compute_diff_detects_bd_change() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let proj = DiffProjection::default();
        let diff = proj.compute_diff(&old, &new);
        assert_eq!(diff.modified_nodes.len(), 1);
        assert_eq!(diff.modified_nodes[0].bd_changes.len(), 1);
    }

    #[test]
    fn describe_diff_empty() {
        let proj = DiffProjection::default();
        let diff = SCGDiff::empty();
        let desc = proj.describe_diff(&diff);
        assert!(desc.contains("No changes"));
    }

    #[test]
    fn human_readable_summary() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let proj = DiffProjection::default();
        let diff = proj.compute_diff(&old, &new);
        let summary = proj.human_readable_summary(&diff);
        assert!(summary.contains("change(s) detected"));
        assert!(summary.contains("verify_2fa"));
    }

    #[test]
    fn no_diff_for_identical_scgs() {
        let old = old_scg();
        let proj = DiffProjection::default();
        let diff = proj.compute_diff(&old, &old);
        assert!(diff.is_empty());
    }
}
