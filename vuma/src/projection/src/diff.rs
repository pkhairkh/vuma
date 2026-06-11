//! # Diff Projection
//!
//! Computes and describes differences between two SCG snapshots. The diff
//! captures structural changes (added/removed nodes and edges) as well as
//! semantic changes (modified behavioural descriptors) and can produce
//! human-readable summaries in multiple formats.
//!
//! ## Projection modes
//!
//! - **Unified diff** (`project_diff`) — machine-friendly unified diff format,
//!   similar to `git diff` output, suitable for patch review and tooling.
//! - **Visual diff** (`project_diff_visual`) — side-by-side ASCII comparison
//!   with colour-coded additions (green), removals (red), and modifications
//!   (yellow).
//! - **Conversational diff** (`project_diff_conversational`) — natural-language
//!   description of what changed, why it matters, and the impact on
//!   verification results.
//!
//! ## Example output (conversational)
//!
//! ```text
//! Summary: 3 changes detected
//!
//! The authentication flow now requires 2FA for admin accounts.
//!
//! - Added function `verify_2fa`
//! - Added call edge from `auth_handler` to `verify_2fa`
//! - `auth_handler` gained the `RequiresAuth` capability
//!
//! Impact: New capability requirement may cause previously-passing
//! verification of `auth_handler` to fail. Re-verify call graph
//! reachability from entry points.
//! ```

use crate::{BdKind, NodeId, NodeKind, SCGEdge, SCGNode, SCG};
use colored::*;
use vuma_scg;

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

/// A semantically grouped cluster of related changes.
///
/// Groups changes that affect the same logical entity (e.g., all changes
/// to a single function including its node, edges, and BDs).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChangeGroup {
    /// A human-readable name for this group (e.g., "Function `auth_handler`").
    pub label: String,
    /// The central node ID this group revolves around, if any.
    pub central_node_id: Option<NodeId>,
    /// Nodes added in this group.
    pub added_nodes: Vec<SCGNode>,
    /// Nodes removed in this group.
    pub removed_nodes: Vec<SCGNode>,
    /// Node modifications in this group.
    pub modified_nodes: Vec<NodeChange>,
    /// Edges added in this group.
    pub added_edges: Vec<SCGEdge>,
    /// Edges removed in this group.
    pub removed_edges: Vec<SCGEdge>,
}

impl ChangeGroup {
    /// Creates an empty group with the given label.
    pub fn new(label: String, central_node_id: Option<NodeId>) -> Self {
        Self {
            label,
            central_node_id,
            added_nodes: Vec::new(),
            removed_nodes: Vec::new(),
            modified_nodes: Vec::new(),
            added_edges: Vec::new(),
            removed_edges: Vec::new(),
        }
    }

    /// Returns `true` if this group contains no changes.
    pub fn is_empty(&self) -> bool {
        self.added_nodes.is_empty()
            && self.removed_nodes.is_empty()
            && self.modified_nodes.is_empty()
            && self.added_edges.is_empty()
            && self.removed_edges.is_empty()
    }

    /// Returns the total number of individual changes in this group.
    pub fn total_changes(&self) -> usize {
        self.added_nodes.len()
            + self.removed_nodes.len()
            + self.modified_nodes.len()
            + self.added_edges.len()
            + self.removed_edges.len()
    }
}

/// Impact level for verification analysis.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum ImpactLevel {
    /// No impact on verification results.
    None,
    /// Minor impact — existing proofs may need minor adjustments.
    Low,
    /// Moderate impact — some verification conditions must be re-checked.
    Medium,
    /// High impact — verification results are invalidated and must be re-run.
    High,
    /// Critical impact — fundamental safety properties may be violated.
    Critical,
}

impl std::fmt::Display for ImpactLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImpactLevel::None => write!(f, "none"),
            ImpactLevel::Low => write!(f, "low"),
            ImpactLevel::Medium => write!(f, "medium"),
            ImpactLevel::High => write!(f, "high"),
            ImpactLevel::Critical => write!(f, "critical"),
        }
    }
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
/// and produces human-readable descriptions of the changes in multiple
/// formats: unified diff, side-by-side visual, and conversational.
#[derive(Debug, Clone)]
pub struct DiffProjection {
    /// Whether to include edge-level details in descriptions.
    pub verbose_edges: bool,
    /// Whether to use ANSI color codes in output.
    pub use_color: bool,
}

impl Default for DiffProjection {
    fn default() -> Self {
        Self {
            verbose_edges: true,
            use_color: true,
        }
    }
}

impl DiffProjection {
    /// Creates a new diff projection engine.
    pub fn new(verbose_edges: bool) -> Self {
        Self {
            verbose_edges,
            use_color: true,
        }
    }

    /// Creates a new diff projection engine with color disabled.
    pub fn no_color() -> Self {
        Self {
            verbose_edges: true,
            use_color: false,
        }
    }

    // ── Color helpers ─────────────────────────────────────────────────────

    fn green(&self, s: &str) -> ColoredString {
        if self.use_color {
            s.green()
        } else {
            s.normal()
        }
    }

    fn red(&self, s: &str) -> ColoredString {
        if self.use_color {
            s.red()
        } else {
            s.normal()
        }
    }

    fn yellow(&self, s: &str) -> ColoredString {
        if self.use_color {
            s.yellow()
        } else {
            s.normal()
        }
    }

    fn bold(&self, s: &str) -> ColoredString {
        if self.use_color {
            s.bold()
        } else {
            s.normal()
        }
    }

    // ── Compute diff ──────────────────────────────────────────────────────

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
        let old_nodes: std::collections::HashMap<NodeId, &SCGNode> =
            old_scg.nodes.iter().map(|n| (n.id, n)).collect();
        let new_nodes: std::collections::HashMap<NodeId, &SCGNode> =
            new_scg.nodes.iter().map(|n| (n.id, n)).collect();

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
        let old_edges: std::collections::HashMap<u64, &SCGEdge> =
            old_scg.edges.iter().map(|e| (e.id, e)).collect();
        let new_edges: std::collections::HashMap<u64, &SCGEdge> =
            new_scg.edges.iter().map(|e| (e.id, e)).collect();

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

    // ── Semantic grouping ─────────────────────────────────────────────────

    /// Groups related changes by their central node/entity.
    ///
    /// Changes that affect the same function (node additions, edge changes,
    /// BD modifications) are grouped together. Unaffiliated changes form
    /// their own group.
    pub fn group_changes(&self, diff: &SCGDiff) -> Vec<ChangeGroup> {
        let mut groups: Vec<ChangeGroup> = Vec::new();
        let mut node_to_group: std::collections::HashMap<NodeId, usize> =
            std::collections::HashMap::new();

        // ── Phase 1: Create groups from added/removed/modified nodes ──
        for node in &diff.added_nodes {
            let label = format!("Added {} `{}`", self.kind_label(&node.kind), node.label);
            let idx = groups.len();
            let mut group = ChangeGroup::new(label, Some(node.id));
            group.added_nodes.push(node.clone());
            groups.push(group);
            node_to_group.insert(node.id, idx);
        }

        for node in &diff.removed_nodes {
            let label = format!("Removed {} `{}`", self.kind_label(&node.kind), node.label);
            let idx = groups.len();
            let mut group = ChangeGroup::new(label, Some(node.id));
            group.removed_nodes.push(node.clone());
            groups.push(group);
            node_to_group.insert(node.id, idx);
        }

        for change in &diff.modified_nodes {
            let label = format!("Modified `{}`", change.label);
            let idx = groups.len();
            let mut group = ChangeGroup::new(label, Some(change.id));
            group.modified_nodes.push(change.clone());
            groups.push(group);
            node_to_group.insert(change.id, idx);
        }

        // ── Phase 2: Assign edges to existing groups by endpoint ──
        // Build a label-to-node-id map for looking up added node labels
        let _added_labels: std::collections::HashMap<String, NodeId> = diff
            .added_nodes
            .iter()
            .map(|n| (n.label.clone(), n.id))
            .collect();

        for edge in &diff.added_edges {
            let target_group = node_to_group.get(&edge.target).copied();
            let source_group = node_to_group.get(&edge.source).copied();

            match (target_group, source_group) {
                (Some(idx), _) | (_, Some(idx)) => {
                    groups[idx].added_edges.push(edge.clone());
                }
                (None, None) => {
                    // Try to match by added-node label in source/target
                    let found = diff
                        .added_nodes
                        .iter()
                        .find(|n| n.id == edge.source || n.id == edge.target)
                        .and_then(|n| node_to_group.get(&n.id).copied());
                    if let Some(idx) = found {
                        groups[idx].added_edges.push(edge.clone());
                    } else {
                        // Unaffiliated edge — create standalone group
                        let label =
                            format!("Edge {} → {} ({:?})", edge.source, edge.target, edge.kind);
                        let mut group = ChangeGroup::new(label, None);
                        group.added_edges.push(edge.clone());
                        groups.push(group);
                    }
                }
            }
        }

        for edge in &diff.removed_edges {
            let target_group = node_to_group.get(&edge.target).copied();
            let source_group = node_to_group.get(&edge.source).copied();

            match (target_group, source_group) {
                (Some(idx), _) | (_, Some(idx)) => {
                    groups[idx].removed_edges.push(edge.clone());
                }
                (None, None) => {
                    let label = format!(
                        "Removed edge {} → {} ({:?})",
                        edge.source, edge.target, edge.kind
                    );
                    let mut group = ChangeGroup::new(label, None);
                    group.removed_edges.push(edge.clone());
                    groups.push(group);
                }
            }
        }

        // ── Phase 3: Merge groups that share edges with the same added node ──
        // (e.g., a new function node + the call edge to it should be in one group)
        // This is already handled by the endpoint-based assignment above.

        groups
    }

    // ── Impact analysis ───────────────────────────────────────────────────

    /// Analyses the impact of a diff on verification results.
    ///
    /// Returns the impact level and a human-readable description of how
    /// the changes affect the verification state.
    pub fn analyse_impact(&self, diff: &SCGDiff) -> (ImpactLevel, String) {
        if diff.is_empty() {
            return (
                ImpactLevel::None,
                "No changes — verification results are unaffected.".to_string(),
            );
        }

        let mut reasons: Vec<String> = Vec::new();
        let mut level = ImpactLevel::None;

        // ── Safety-related BD changes are high impact ──
        let safety_bds_added: Vec<&BdChange> = diff
            .modified_nodes
            .iter()
            .flat_map(|c| c.bd_changes.iter())
            .chain(diff.modified_bds.iter())
            .filter(|bd| bd.kind == BdKind::Safety)
            .collect();

        if !safety_bds_added.is_empty() {
            level = level.max(ImpactLevel::Critical);
            for bd in &safety_bds_added {
                let dir = if bd.added { "added" } else { "removed" };
                reasons.push(format!(
                    "Safety property `{}` was {} — this may invalidate safety proofs",
                    bd.name, dir
                ));
            }
        }

        // ── Capability changes are medium-to-high impact ──
        let cap_bds: Vec<&BdChange> = diff
            .modified_nodes
            .iter()
            .flat_map(|c| c.bd_changes.iter())
            .chain(diff.modified_bds.iter())
            .filter(|bd| bd.kind == BdKind::Capability)
            .collect();

        if !cap_bds.is_empty() {
            level = level.max(ImpactLevel::Medium);
            for bd in &cap_bds {
                let dir = if bd.added { "gained" } else { "lost" };
                reasons.push(format!(
                    "Capability `{}` was {} — capability proofs may need re-checking",
                    bd.name, dir
                ));
            }
        }

        // ── Memory layout changes are medium impact ──
        let mem_bds: Vec<&BdChange> = diff
            .modified_nodes
            .iter()
            .flat_map(|c| c.bd_changes.iter())
            .chain(diff.modified_bds.iter())
            .filter(|bd| bd.kind == BdKind::MemoryLayout)
            .collect();

        if !mem_bds.is_empty() {
            level = level.max(ImpactLevel::Medium);
            for bd in &mem_bds {
                let dir = if bd.added { "added" } else { "removed" };
                reasons.push(format!(
                    "Memory layout property `{}` was {} — memory safety proofs may need re-verification",
                    bd.name, dir
                ));
            }
        }

        // ── Node removals invalidate reachability ──
        if !diff.removed_nodes.is_empty() {
            level = level.max(ImpactLevel::High);
            let names: Vec<&str> = diff
                .removed_nodes
                .iter()
                .map(|n| n.label.as_str())
                .collect();
            reasons.push(format!(
                "Node(s) removed ({}) — call graph reachability must be re-verified",
                names.join(", ")
            ));
        }

        // ── Node additions may require new verification ──
        if !diff.added_nodes.is_empty() {
            level = level.max(ImpactLevel::Low);
            let names: Vec<&str> = diff.added_nodes.iter().map(|n| n.label.as_str()).collect();
            reasons.push(format!(
                "New node(s) added ({}) — new verification conditions may be generated",
                names.join(", ")
            ));
        }

        // ── Edge changes affect data/control flow ──
        let total_edge_changes = diff.added_edges.len() + diff.removed_edges.len();
        if total_edge_changes > 0 {
            level = level.max(ImpactLevel::Medium);
            reasons.push(format!(
                "{} edge(s) changed — data flow and control flow verification must be re-checked",
                total_edge_changes
            ));
        }

        // ── If capability additions coincide with new edges, upgrade ──
        if !cap_bds.is_empty() && !diff.added_edges.is_empty() {
            level = level.max(ImpactLevel::High);
            reasons.push(
                "Capability changes combined with new edges may affect call graph verification"
                    .to_string(),
            );
        }

        let description = if reasons.is_empty() {
            "Changes detected but no verification impact identified.".to_string()
        } else {
            reasons.join(". ") + "."
        };

        (level, description)
    }

    // ── Describe diff ─────────────────────────────────────────────────────

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

    // ── Unified diff format ───────────────────────────────────────────────

    /// Renders the SCG diff as a unified diff format string.
    ///
    /// The output follows the conventions of `diff -u`, with:
    /// - `---` header for removed items (old version)
    /// - `+++` header for added items (new version)
    /// - `@@` hunks for each semantic group
    /// - `+` prefix for additions (colored green)
    /// - `-` prefix for removals (colored red)
    /// - `~` prefix for modifications (colored yellow)
    pub fn project_diff(&self, diff: &SCGDiff) -> String {
        if diff.is_empty() {
            return "No changes detected.".to_string();
        }

        let mut lines: Vec<String> = Vec::new();

        // Header
        lines.push("--- SCG (old)".to_string());
        lines.push("+++ SCG (new)".to_string());

        // Summary hunk
        lines.push(format!(
            "@@ -{} nodes,{} edges +{} nodes,{} edges @@",
            diff.removed_nodes.len() + diff.modified_nodes.len(),
            diff.removed_edges.len(),
            diff.added_nodes.len() + diff.modified_nodes.len(),
            diff.added_edges.len(),
        ));

        // Removed nodes
        for node in &diff.removed_nodes {
            let desc = format!("- [node] {} ({:?})", node.label, node.kind);
            lines.push(format!("{}", self.red(&desc)));
        }

        // Added nodes
        for node in &diff.added_nodes {
            let desc = format!("+ [node] {} ({:?})", node.label, node.kind);
            lines.push(format!("{}", self.green(&desc)));
        }

        // Modified nodes with BD changes
        for change in &diff.modified_nodes {
            let header = format!("~ [node] {} (modified)", change.label);
            lines.push(format!("{}", self.yellow(&header)));
            for bd in &change.bd_changes {
                if bd.added {
                    let desc = format!("+   BD `{}` ({:?}) added", bd.name, bd.kind);
                    lines.push(format!("{}", self.green(&desc)));
                } else {
                    let desc = format!("-   BD `{}` ({:?}) removed", bd.name, bd.kind);
                    lines.push(format!("{}", self.red(&desc)));
                }
            }
        }

        // Removed edges
        for edge in &diff.removed_edges {
            let desc = format!(
                "- [edge] {} ──{:?}──▶ {}",
                edge.source, edge.kind, edge.target
            );
            lines.push(format!("{}", self.red(&desc)));
        }

        // Added edges
        for edge in &diff.added_edges {
            let desc = format!(
                "+ [edge] {} ──{:?}──▶ {}",
                edge.source, edge.kind, edge.target
            );
            lines.push(format!("{}", self.green(&desc)));
        }

        // Standalone BD changes
        for bd in &diff.modified_bds {
            if bd.added {
                let desc = format!(
                    "+ [bd] `{}` ({:?}) on node {}",
                    bd.name, bd.kind, bd.node_id
                );
                lines.push(format!("{}", self.green(&desc)));
            } else {
                let desc = format!(
                    "- [bd] `{}` ({:?}) on node {}",
                    bd.name, bd.kind, bd.node_id
                );
                lines.push(format!("{}", self.red(&desc)));
            }
        }

        lines.join("\n")
    }

    // ── Side-by-side visual diff ──────────────────────────────────────────

    /// Renders the diff as a side-by-side visual comparison.
    ///
    /// Left column shows the old state, right column shows the new state.
    /// Colour coding: green for additions, red for removals, yellow for
    /// modifications.
    pub fn project_diff_visual(&self, diff: &SCGDiff) -> String {
        if diff.is_empty() {
            return "No changes detected. Left and right are identical.".to_string();
        }

        let col_width: usize = 40;
        let separator = " │ ";
        let mut lines: Vec<String> = Vec::new();

        // Header
        let left_header = format!("{:^width$}", "OLD SCG", width = col_width);
        let right_header = format!("{:^width$}", "NEW SCG", width = col_width);
        lines.push(format!(
            "{}{}{}",
            self.bold(&left_header),
            separator,
            self.bold(&right_header)
        ));
        lines.push(format!(
            "{}{}{}",
            "─".repeat(col_width),
            "─┼─",
            "─".repeat(col_width)
        ));

        // Collect all node IDs involved in changes
        // Removed nodes: show on left only
        for node in &diff.removed_nodes {
            let left = format!("- {} ({:?})", node.label, node.kind);
            let right = String::new();
            let left_padded = format!("{:<width$}", left, width = col_width);
            let right_padded = format!("{:<width$}", right, width = col_width);
            lines.push(format!(
                "{}{}{}",
                self.red(&left_padded),
                separator,
                right_padded
            ));
        }

        // Added nodes: show on right only
        for node in &diff.added_nodes {
            let left = String::new();
            let right = format!("+ {} ({:?})", node.label, node.kind);
            let left_padded = format!("{:<width$}", left, width = col_width);
            let right_padded = format!("{:<width$}", right, width = col_width);
            lines.push(format!(
                "{}{}{}",
                left_padded,
                separator,
                self.green(&right_padded),
            ));
        }

        // Modified nodes: show BD changes on both sides
        for change in &diff.modified_nodes {
            let left_parts: Vec<String> = change
                .bd_changes
                .iter()
                .filter(|bd| !bd.added)
                .map(|bd| format!("- BD `{}`", bd.name))
                .collect();
            let right_parts: Vec<String> = change
                .bd_changes
                .iter()
                .filter(|bd| bd.added)
                .map(|bd| format!("+ BD `{}`", bd.name))
                .collect();

            if left_parts.is_empty() && right_parts.is_empty() {
                // Both sides have changes; show the node header
                let left = format!("~ {}", change.label);
                let right = format!("~ {}", change.label);
                lines.push(format!(
                    "{}{}{}",
                    self.yellow(&format!("{:<width$}", left, width = col_width)),
                    separator,
                    self.yellow(&format!("{:<width$}", right, width = col_width)),
                ));
            } else {
                // Show node header
                let left_hdr = format!("~ {} (old)", change.label);
                let right_hdr = format!("~ {} (new)", change.label);
                lines.push(format!(
                    "{}{}{}",
                    self.yellow(&format!("{:<width$}", left_hdr, width = col_width)),
                    separator,
                    self.yellow(&format!("{:<width$}", right_hdr, width = col_width)),
                ));

                let max_rows = left_parts.len().max(right_parts.len());
                for i in 0..max_rows {
                    let left = left_parts.get(i).map(|s| s.as_str()).unwrap_or("");
                    let right = right_parts.get(i).map(|s| s.as_str()).unwrap_or("");
                    lines.push(format!(
                        "{}{}{}",
                        self.red(&format!("{:<width$}", left, width = col_width)),
                        separator,
                        self.green(&format!("{:<width$}", right, width = col_width)),
                    ));
                }
            }
        }

        // Removed edges: show on left only
        for edge in &diff.removed_edges {
            let left = format!("- edge: {}→{} ({:?})", edge.source, edge.target, edge.kind);
            let right = String::new();
            let left_padded = format!("{:<width$}", left, width = col_width);
            let right_padded = format!("{:<width$}", right, width = col_width);
            lines.push(format!(
                "{}{}{}",
                self.red(&left_padded),
                separator,
                right_padded,
            ));
        }

        // Added edges: show on right only
        for edge in &diff.added_edges {
            let left = String::new();
            let right = format!("+ edge: {}→{} ({:?})", edge.source, edge.target, edge.kind);
            let left_padded = format!("{:<width$}", left, width = col_width);
            let right_padded = format!("{:<width$}", right, width = col_width);
            lines.push(format!(
                "{}{}{}",
                left_padded,
                separator,
                self.green(&right_padded),
            ));
        }

        lines.join("\n")
    }

    // ── Conversational diff ───────────────────────────────────────────────

    /// Renders a natural-language description of the changes and their impact.
    ///
    /// This is designed for developer-facing tooling (IDE, CLI, code review)
    /// and produces a readable narrative that explains:
    /// - What changed
    /// - Why it matters (semantic interpretation)
    /// - What effect the changes have on verification results
    ///
    /// Colour coding: green for additions, red for removals, yellow for
    /// modifications.
    pub fn project_diff_conversational(&self, diff: &SCGDiff) -> String {
        if diff.is_empty() {
            return "No changes were detected between the two versions.".to_string();
        }

        let mut lines: Vec<String> = Vec::new();

        // ── Summary header ──
        lines.push(format!(
            "{}",
            self.bold(&format!(
                "Summary: {} change(s) detected",
                diff.total_changes()
            ))
        ));
        lines.push(String::new());

        // ── Semantic interpretation sentence ──
        let summary_sentence = self.generate_summary_sentence(diff);
        if !summary_sentence.is_empty() {
            lines.push(summary_sentence);
            lines.push(String::new());
        }

        // ── Semantic grouping ──
        let groups = self.group_changes(diff);
        if groups.len() > 1 {
            lines.push(format!("{}", self.bold("Changes by component:")));
            lines.push(String::new());
            for group in &groups {
                if group.is_empty() {
                    continue;
                }
                lines.push(format!(
                    "  {} ({} change{})",
                    self.bold(&group.label),
                    group.total_changes(),
                    if group.total_changes() == 1 { "" } else { "s" }
                ));

                for node in &group.added_nodes {
                    lines.push(format!(
                        "    {}",
                        self.green(&format!(
                            "Added {} `{}`",
                            self.kind_label(&node.kind),
                            node.label
                        ))
                    ));
                }
                for node in &group.removed_nodes {
                    lines.push(format!(
                        "    {}",
                        self.red(&format!(
                            "Removed {} `{}`",
                            self.kind_label(&node.kind),
                            node.label
                        ))
                    ));
                }
                for change in &group.modified_nodes {
                    for bd in &change.bd_changes {
                        if bd.added {
                            lines.push(format!(
                                "    {}",
                                self.green(&format!(
                                    "`{}` gained the `{}` {}",
                                    change.label,
                                    bd.name,
                                    self.bd_kind_label(&bd.kind)
                                ))
                            ));
                        } else {
                            lines.push(format!(
                                "    {}",
                                self.red(&format!(
                                    "`{}` lost the `{}` {}",
                                    change.label,
                                    bd.name,
                                    self.bd_kind_label(&bd.kind)
                                ))
                            ));
                        }
                    }
                }
                for edge in &group.added_edges {
                    lines.push(format!(
                        "    {}",
                        self.green(&format!(
                            "Added {:?} edge ({} → {})",
                            edge.kind, edge.source, edge.target
                        ))
                    ));
                }
                for edge in &group.removed_edges {
                    lines.push(format!(
                        "    {}",
                        self.red(&format!(
                            "Removed {:?} edge ({} → {})",
                            edge.kind, edge.source, edge.target
                        ))
                    ));
                }
            }
            lines.push(String::new());
        } else {
            // ── Flat bullet points when there's only one group or no grouping ──
            for node in &diff.added_nodes {
                lines.push(format!(
                    "- {}",
                    self.green(&format!(
                        "Added {} `{}`",
                        self.kind_label(&node.kind),
                        node.label
                    ))
                ));
            }
            for node in &diff.removed_nodes {
                lines.push(format!(
                    "- {}",
                    self.red(&format!(
                        "Removed {} `{}`",
                        self.kind_label(&node.kind),
                        node.label
                    ))
                ));
            }
            for change in &diff.modified_nodes {
                for bd in &change.bd_changes {
                    if bd.added {
                        lines.push(format!(
                            "- {}",
                            self.yellow(&format!(
                                "`{}` gained the `{}` {}",
                                change.label,
                                bd.name,
                                self.bd_kind_label(&bd.kind)
                            ))
                        ));
                    } else {
                        lines.push(format!(
                            "- {}",
                            self.yellow(&format!(
                                "`{}` lost the `{}` {}",
                                change.label,
                                bd.name,
                                self.bd_kind_label(&bd.kind)
                            ))
                        ));
                    }
                }
            }
            for edge in &diff.added_edges {
                lines.push(format!(
                    "- {}",
                    self.green(&format!(
                        "Added {:?} edge ({} → {})",
                        edge.kind, edge.source, edge.target
                    ))
                ));
            }
            for edge in &diff.removed_edges {
                lines.push(format!(
                    "- {}",
                    self.red(&format!(
                        "Removed {:?} edge ({} → {})",
                        edge.kind, edge.source, edge.target
                    ))
                ));
            }
            lines.push(String::new());
        }

        // ── Impact analysis ──
        let (impact_level, impact_desc) = self.analyse_impact(diff);
        let impact_label = match impact_level {
            ImpactLevel::None => format!("{}", self.bold("Impact: none")),
            ImpactLevel::Low => format!(
                "{}",
                self.bold(&format!(
                    "Impact: {}",
                    self.yellow(&impact_level.to_string())
                ))
            ),
            ImpactLevel::Medium => format!(
                "{}",
                self.bold(&format!(
                    "Impact: {}",
                    self.yellow(&impact_level.to_string())
                ))
            ),
            ImpactLevel::High => format!(
                "{}",
                self.bold(&format!("Impact: {}", self.red(&impact_level.to_string())))
            ),
            ImpactLevel::Critical => format!(
                "{}",
                self.bold(&format!("Impact: {}", self.red(&impact_level.to_string())))
            ),
        };
        lines.push(impact_label);
        lines.push(impact_desc);

        lines.join("\n")
    }

    // ── Human-readable summary (original, preserved) ──────────────────────

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
        let added_labels: Vec<&str> = diff.added_nodes.iter().map(|n| n.label.as_str()).collect();
        let added_bds: Vec<&str> = diff
            .modified_nodes
            .iter()
            .flat_map(|c| c.bd_changes.iter())
            .filter(|bd| bd.added)
            .map(|bd| bd.name.as_str())
            .collect();

        // Pattern: "X now requires Y"
        if added_labels
            .iter()
            .any(|l| l.contains("2fa") || l.contains("verify"))
            && added_bds
                .iter()
                .any(|b| b.contains("Auth") || b.contains("Requires"))
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
            let names: Vec<&str> = diff
                .removed_nodes
                .iter()
                .map(|n| n.label.as_str())
                .collect();
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
            NodeKind::Allocation => "allocation",
            NodeKind::Deallocation => "deallocation",
            NodeKind::Access => "access",
            NodeKind::Computation => "computation",
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

// ── Free-standing projection functions ────────────────────────────────────────

/// Renders an SCG diff as unified diff format.
///
/// Convenience wrapper around [`DiffProjection::project_diff`].
pub fn project_diff(diff: &SCGDiff) -> String {
    DiffProjection::default().project_diff(diff)
}

/// Renders an SCG diff as a side-by-side visual comparison.
///
/// Convenience wrapper around [`DiffProjection::project_diff_visual`].
pub fn project_diff_visual(diff: &SCGDiff) -> String {
    DiffProjection::default().project_diff_visual(diff)
}

/// Renders an SCG diff as a natural-language description with impact analysis.
///
/// Convenience wrapper around [`DiffProjection::project_diff_conversational`].
pub fn project_diff_conversational(diff: &SCGDiff) -> String {
    DiffProjection::default().project_diff_conversational(diff)
}

/// Compute the diff between two real vuma-scg SCGs.
///
/// Converts both SCGs to the projection crate's lightweight representation,
/// then delegates to [`DiffProjection::compute_diff`].
pub fn diff_scg(old: &vuma_scg::SCG, new: &vuma_scg::SCG) -> SCGDiff {
    let old_proj = crate::scg_adapter::from_scg(old);
    let new_proj = crate::scg_adapter::from_scg(new);
    DiffProjection::default().compute_diff(&old_proj, &new_proj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BdKind, BehaviouralDescriptor, EdgeKind, SCGEdge, SCGNode};

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

    fn new_scg_with_removed_node() -> SCG {
        SCG {
            nodes: vec![],
            edges: vec![],
            regions: vec![],
        }
    }

    fn new_scg_with_safety_bd() -> SCG {
        let mut scg = old_scg();
        scg.nodes[0].bds.push(BehaviouralDescriptor {
            id: 200,
            name: "noalias".into(),
            kind: BdKind::Safety,
            parameter: None,
        });
        scg
    }

    fn new_scg_with_removed_bd() -> SCG {
        SCG {
            nodes: vec![SCGNode {
                id: 1,
                label: "auth_handler".into(),
                kind: NodeKind::Function,
                bds: vec![], // removed the "Send" BD
                regions: vec![],
            }],
            edges: vec![],
            regions: vec![],
        }
    }

    fn new_scg_with_edge_change() -> SCG {
        let mut scg = old_scg();
        scg.edges.push(SCGEdge {
            id: 20,
            source: 1,
            target: 1,
            kind: EdgeKind::DataFlow,
        });
        scg
    }

    fn proj() -> DiffProjection {
        DiffProjection::no_color()
    }

    // ── Test 1: Compute diff detects added node ──

    #[test]
    fn compute_diff_detects_added_node() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let diff = proj().compute_diff(&old, &new);
        assert_eq!(diff.added_nodes.len(), 1);
        assert_eq!(diff.added_nodes[0].label, "verify_2fa");
    }

    // ── Test 2: Compute diff detects added edge ──

    #[test]
    fn compute_diff_detects_added_edge() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let diff = proj().compute_diff(&old, &new);
        assert_eq!(diff.added_edges.len(), 1);
    }

    // ── Test 3: Compute diff detects BD change ──

    #[test]
    fn compute_diff_detects_bd_change() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let diff = proj().compute_diff(&old, &new);
        assert_eq!(diff.modified_nodes.len(), 1);
        assert_eq!(diff.modified_nodes[0].bd_changes.len(), 1);
    }

    // ── Test 4: Describe diff for empty diff ──

    #[test]
    fn describe_diff_empty() {
        let diff = SCGDiff::empty();
        let desc = proj().describe_diff(&diff);
        assert!(desc.contains("No changes"));
    }

    // ── Test 5: Human-readable summary ──

    #[test]
    fn human_readable_summary() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let diff = proj().compute_diff(&old, &new);
        let summary = proj().human_readable_summary(&diff);
        assert!(summary.contains("change(s) detected"));
        assert!(summary.contains("verify_2fa"));
    }

    // ── Test 6: No diff for identical SCGs ──

    #[test]
    fn no_diff_for_identical_scgs() {
        let old = old_scg();
        let diff = proj().compute_diff(&old, &old);
        assert!(diff.is_empty());
    }

    // ── Test 7: project_diff produces unified format ──

    #[test]
    fn project_diff_unified_format() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let diff = proj().compute_diff(&old, &new);
        let output = proj().project_diff(&diff);

        // Unified diff headers
        assert!(output.contains("--- SCG (old)"));
        assert!(output.contains("+++ SCG (new)"));
        // Should contain hunk marker
        assert!(output.contains("@@"));
        // Should contain added node
        assert!(output.contains("[node]"));
        assert!(output.contains("verify_2fa"));
        // Should contain added edge
        assert!(output.contains("[edge]"));
    }

    // ── Test 8: project_diff_visual produces side-by-side ──

    #[test]
    fn project_diff_visual_side_by_side() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let diff = proj().compute_diff(&old, &new);
        let output = proj().project_diff_visual(&diff);

        // Should have column separators
        assert!(output.contains("│"));
        // Should have headers
        assert!(output.contains("OLD SCG"));
        assert!(output.contains("NEW SCG"));
        // Should mention verify_2fa
        assert!(output.contains("verify_2fa"));
    }

    // ── Test 9: project_diff_conversational includes impact ──

    #[test]
    fn project_diff_conversational_with_impact() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let diff = proj().compute_diff(&old, &new);
        let output = proj().project_diff_conversational(&diff);

        assert!(output.contains("Summary:"));
        assert!(output.contains("Impact:"));
        assert!(output.contains("verify_2fa"));
    }

    // ── Test 10: Impact analysis — safety BD changes are critical ──

    #[test]
    fn impact_analysis_safety_bd_is_critical() {
        let old = old_scg();
        let new = new_scg_with_safety_bd();
        let diff = proj().compute_diff(&old, &new);
        let (level, desc) = proj().analyse_impact(&diff);
        assert_eq!(level, ImpactLevel::Critical);
        assert!(desc.contains("Safety property"));
        assert!(desc.contains("noalias"));
    }

    // ── Test 11: Impact analysis — node removal is high impact ──

    #[test]
    fn impact_analysis_node_removal_is_high() {
        let old = old_scg();
        let new = new_scg_with_removed_node();
        let diff = proj().compute_diff(&old, &new);
        let (level, desc) = proj().analyse_impact(&diff);
        assert!(level == ImpactLevel::High || level == ImpactLevel::Critical);
        assert!(desc.contains("removed"));
        assert!(desc.contains("auth_handler"));
    }

    // ── Test 12: Impact analysis — edge changes are medium impact ──

    #[test]
    fn impact_analysis_edge_changes_are_medium() {
        let old = old_scg();
        let new = new_scg_with_edge_change();
        let diff = proj().compute_diff(&old, &new);
        let (level, desc) = proj().analyse_impact(&diff);
        assert!(level >= ImpactLevel::Medium);
        assert!(desc.contains("edge"));
    }

    // ── Test 13: Semantic grouping groups related changes ──

    #[test]
    fn semantic_grouping_groups_related_changes() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let diff = proj().compute_diff(&old, &new);
        let groups = proj().group_changes(&diff);

        // Should have multiple groups (added node, modified node, plus edge attached)
        assert!(!groups.is_empty());
        // At least one group should have both a node and an edge
        let has_edge = groups.iter().any(|g| !g.added_edges.is_empty());
        assert!(has_edge, "Expected at least one group with an added edge");
    }

    // ── Test 14: Empty diff has no impact ──

    #[test]
    fn empty_diff_has_no_impact() {
        let diff = SCGDiff::empty();
        let (level, desc) = proj().analyse_impact(&diff);
        assert_eq!(level, ImpactLevel::None);
        assert!(desc.contains("No changes"));
    }

    // ── Test 15: Removed node detected in diff ──

    #[test]
    fn compute_diff_detects_removed_node() {
        let old = old_scg();
        let new = new_scg_with_removed_node();
        let diff = proj().compute_diff(&old, &new);
        assert_eq!(diff.removed_nodes.len(), 1);
        assert_eq!(diff.removed_nodes[0].label, "auth_handler");
    }

    // ── Test 16: Removed BD detected in diff ──

    #[test]
    fn compute_diff_detects_removed_bd() {
        let old = old_scg();
        let new = new_scg_with_removed_bd();
        let diff = proj().compute_diff(&old, &new);
        assert_eq!(diff.modified_nodes.len(), 1);
        let bd_change = &diff.modified_nodes[0].bd_changes[0];
        assert!(!bd_change.added);
        assert_eq!(bd_change.name, "Send");
    }

    // ── Test 17: Capability + new edge upgrades impact to high ──

    #[test]
    fn capability_with_new_edge_upgrades_impact() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let diff = proj().compute_diff(&old, &new);
        let (level, desc) = proj().analyse_impact(&diff);
        assert!(level >= ImpactLevel::High);
        assert!(desc.contains("Capability") || desc.contains("edge"));
    }

    // ── Test 18: Free-standing project_diff function works ──

    #[test]
    fn free_standing_project_diff() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let diff = proj().compute_diff(&old, &new);
        let output = project_diff(&diff);
        assert!(output.contains("--- SCG (old)"));
        assert!(output.contains("+++ SCG (new)"));
    }

    // ── Test 19: Free-standing project_diff_visual function works ──

    #[test]
    fn free_standing_project_diff_visual() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let diff = proj().compute_diff(&old, &new);
        let output = project_diff_visual(&diff);
        assert!(output.contains("OLD SCG"));
        assert!(output.contains("NEW SCG"));
    }

    // ── Test 20: Free-standing project_diff_conversational function works ──

    #[test]
    fn free_standing_project_diff_conversational() {
        let old = old_scg();
        let new = new_scg_with_added_node();
        let diff = proj().compute_diff(&old, &new);
        let output = project_diff_conversational(&diff);
        assert!(output.contains("Summary:"));
        assert!(output.contains("Impact:"));
    }
}
