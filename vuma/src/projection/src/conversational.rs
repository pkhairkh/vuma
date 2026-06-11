//! # Conversational Projection
//!
//! Generates natural-language descriptions of SCG structures and changes,
//! enabling a dialogue between the developer and the program's semantic model.
//!
//! ## Capabilities
//!
//! - **Render SCG** — produces a natural-language summary of an entire SCG,
//!   describing its nodes, edges, regions, and overall architecture.
//! - **Explain node** — produces a plain-English explanation of what a specific
//!   node does, including its behavioural descriptors and relationships.
//! - **Explain region** — describes a region's purpose and the collective role
//!   of the nodes it contains.
//! - **Explain verification** — narrates the results of an aggregated
//!   verification pass, including which checks passed and which failed.
//! - **Suggest fix** — given a verification violation, proposes concrete steps
//!   to resolve it.
//! - **Describe** — produces a natural-language summary of a node's purpose,
//!   its behavioural descriptors, and its relationships.
//! - **Explain change** — takes an [`SCGDiff`] and narrates what changed in
//!   plain English.
//! - **Suggest modification** — given a high-level intent string, proposes a
//!   set of [`SCGEdit`] operations that would implement that intent.
//!
//! ## Verbosity Levels
//!
//! The projection supports three verbosity levels via [`Verbosity`]:
//!
//! - **Brief** — one-line summaries, suitable for compact logs and tooltips.
//! - **Normal** — balanced descriptions with key details (default).
//! - **Detailed** — exhaustive descriptions including every BD, edge, and
//!   region membership, suitable for documentation generation.
//!
//! ## AI-Driven Explanations
//!
//! The [`AIExplainerOutput`] struct provides a machine-readable, structured
//! representation that can be fed to an LLM for refinement. Use
//! [`ConversationalProjection::to_ai_prompt`] to generate this structure.
//!
//! ## Example
//!
//! ```
//! use vuma_projection::conversational::{ConversationalProjection, Verbosity};
//! use vuma_projection::SCG;
//!
//! let proj = ConversationalProjection::with_verbosity(Verbosity::Normal);
//! let scg = SCG::empty();
//! let desc = proj.render_scg(&scg);
//! ```

use crate::diff::SCGDiff;
use crate::{BdKind, EdgeKind, NodeId, NodeKind, RegionId, SCG};
use vuma_scg;

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

// ── Verbosity ─────────────────────────────────────────────────────────────────

/// Verbosity level for conversational output.
///
/// Controls how much detail is included in natural-language descriptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Verbosity {
    /// One-line summaries. Ideal for compact logs, tooltips, and quick
    /// overviews.
    Brief,
    /// Balanced descriptions with key details. The default level.
    #[default]
    Normal,
    /// Exhaustive descriptions including every BD, edge, region membership,
    /// and contextual notes. Suitable for documentation generation and
    /// deep debugging.
    Detailed,
}

impl Verbosity {
    /// Returns the numeric level (1 = Brief, 2 = Normal, 3 = Detailed).
    pub fn level(&self) -> u8 {
        match self {
            Verbosity::Brief => 1,
            Verbosity::Normal => 2,
            Verbosity::Detailed => 3,
        }
    }
}

// ── Verification types ────────────────────────────────────────────────────────

/// The severity of a verification violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ViolationSeverity {
    /// A warning that does not block compilation but should be addressed.
    Warning,
    /// An error that must be fixed before the program is considered correct.
    Error,
    /// A critical error indicating a fundamental safety or correctness issue.
    Critical,
}

/// A verification violation discovered during SCG analysis.
///
/// Each violation describes a specific rule that was broken, the node or
/// region involved, and a machine-readable code for categorisation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Violation {
    /// Machine-readable violation code (e.g. `"BD-MISSING-Send"`,
    /// `"EDGE-UNSAFE-DATAFLOW"`).
    pub code: String,
    /// Human-readable description of the violation.
    pub message: String,
    /// The severity of this violation.
    pub severity: ViolationSeverity,
    /// The node involved in the violation, if applicable.
    pub node_id: Option<NodeId>,
    /// The region involved in the violation, if applicable.
    pub region_id: Option<RegionId>,
    /// Suggested fix description, if available.
    pub suggestion: Option<String>,
}

/// The aggregated result of verifying an SCG.
///
/// Contains all violations discovered during analysis, plus summary counts
/// of passed and failed checks.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AggregatedResult {
    /// Total number of verification checks that were performed.
    pub total_checks: usize,
    /// Number of checks that passed successfully.
    pub passed: usize,
    /// All violations discovered during verification.
    pub violations: Vec<Violation>,
}

impl AggregatedResult {
    /// Creates an empty (all-passing) aggregated result.
    pub fn all_passed(total_checks: usize) -> Self {
        Self {
            total_checks,
            passed: total_checks,
            violations: Vec::new(),
        }
    }

    /// Returns `true` if there are no violations.
    pub fn is_ok(&self) -> bool {
        self.violations.is_empty()
    }

    /// Returns the number of violations.
    pub fn violation_count(&self) -> usize {
        self.violations.len()
    }

    /// Returns the number of failed checks.
    pub fn failed(&self) -> usize {
        self.total_checks - self.passed
    }

    /// Returns violations of a specific severity.
    pub fn violations_by_severity(&self, severity: ViolationSeverity) -> Vec<&Violation> {
        self.violations
            .iter()
            .filter(|v| v.severity == severity)
            .collect()
    }
}

// ── AI-driven structured output ───────────────────────────────────────────────

/// A structured representation of a conversational explanation, designed to
/// be serialised and fed to an LLM for refinement.
///
/// The LLM can take this machine-readable context and produce a polished,
/// context-aware natural-language explanation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AIExplainerOutput {
    /// The type of entity being explained.
    pub entity_type: String,
    /// A unique identifier for the entity (node ID, region ID, etc.).
    pub entity_id: String,
    /// The primary natural-language explanation at normal verbosity.
    pub explanation: String,
    /// A brief one-line summary.
    pub brief_summary: String,
    /// A detailed multi-paragraph explanation.
    pub detailed_explanation: String,
    /// Key facts about the entity, as label–value pairs, suitable for
    /// structured consumption by an LLM.
    pub key_facts: Vec<(String, String)>,
    /// Related entity IDs that an LLM may want to cross-reference.
    pub related_entities: Vec<String>,
    /// The schema version of this output format.
    pub schema_version: String,
}

// ── Conversational projection engine ──────────────────────────────────────────

/// A rule-based suggestion engine that analyses an SCG and an intent string
/// to propose a set of [`SCGEdit`] operations.
///
/// The engine uses keyword matching and graph-structure heuristics rather
/// than LLM calls. It examines the SCG's nodes, edges, and regions to
/// produce context-aware suggestions. For example, if the intent mentions
/// "rate limiting" and the graph contains an Effect node, the engine
/// suggests attaching a `Bounded` BD to that node rather than creating a
/// new one from scratch.
#[derive(Debug, Clone)]
pub struct SuggestionEngine<'a> {
    /// Reference to the SCG being analysed.
    scg: &'a SCG,
}

impl<'a> SuggestionEngine<'a> {
    /// Create a new suggestion engine for the given SCG.
    pub fn new(scg: &'a SCG) -> Self {
        Self { scg }
    }

    /// Generate a list of [`SCGEdit`] operations to implement the given intent.
    ///
    /// The engine performs the following analysis:
    /// 1. **Keyword matching** — Recognises common intent phrases (rate
    ///    limiting, thread safety, authentication, logging, etc.).
    /// 2. **Graph structure analysis** — Looks for existing nodes that match
    ///    the intent (e.g. a function named `auth_handler` when the intent
    ///    mentions "auth") and targets them for BD changes instead of
    ///    creating new nodes.
    /// 3. **Fallback** — If no recognised intent is found, a generic
    ///    function node is suggested.
    pub fn generate_suggestions(&self, intent: &str) -> Vec<SCGEdit> {
        let intent_lower = intent.to_lowercase();
        let mut edits: Vec<SCGEdit> = Vec::new();

        // Rate limiting / throttling
        if intent_lower.contains("rate limit") || intent_lower.contains("throttle") {
            // Try to find an existing Effect or Function node to annotate
            let target = self.find_node_by_keywords(&["handler", "api", "endpoint", "request", "service"]);
            edits.push(SCGEdit::AddNode {
                label: "rate_limiter".into(),
                kind: NodeKind::Effect,
                bds: vec!["SideEffect".into()],
            });
            edits.push(SCGEdit::ChangeBD {
                node_id: target,
                bd_name: "Bounded".into(),
                bd_kind: BdKind::Capability,
                add: true,
            });
        }

        // Thread safety
        if intent_lower.contains("thread-safe") || intent_lower.contains("send") || intent_lower.contains("sync") {
            let target = self.find_node_by_keywords(&["handler", "worker", "task", "thread", "pool"]);
            edits.push(SCGEdit::ChangeBD {
                node_id: target,
                bd_name: "Send".into(),
                bd_kind: BdKind::Capability,
                add: true,
            });
            edits.push(SCGEdit::ChangeBD {
                node_id: target,
                bd_name: "Sync".into(),
                bd_kind: BdKind::Capability,
                add: true,
            });
        }

        // 2FA / two-factor authentication
        if intent_lower.contains("2fa") || intent_lower.contains("two-factor") {
            edits.push(SCGEdit::AddNode {
                label: "verify_2fa".into(),
                kind: NodeKind::Function,
                bds: vec!["RequiresAuth".into()],
            });
        }

        // Logging / audit
        if intent_lower.contains("log") || intent_lower.contains("audit") {
            edits.push(SCGEdit::AddNode {
                label: "audit_log".into(),
                kind: NodeKind::Effect,
                bds: vec!["SideEffect".into()],
            });
        }

        // Remove / delete
        if intent_lower.contains("remove") || intent_lower.contains("delete") {
            let target = self.find_node_by_keywords(&["unused", "deprecated", "old"]);
            edits.push(SCGEdit::RemoveNode { node_id: target });
        }

        // Memory safety
        if intent_lower.contains("memory safe") || intent_lower.contains("no leak") {
            let target = self.find_node_by_keywords(&["alloc", "pool", "buffer", "region"]);
            edits.push(SCGEdit::ChangeBD {
                node_id: target,
                bd_name: "NoLeak".into(),
                bd_kind: BdKind::Safety,
                add: true,
            });
        }

        // Parallelism
        if intent_lower.contains("parallel") || intent_lower.contains("concurrent") {
            let target = self.find_node_by_keywords(&["map", "reduce", "fold", "process"]);
            edits.push(SCGEdit::ChangeBD {
                node_id: target,
                bd_name: "Send".into(),
                bd_kind: BdKind::Capability,
                add: true,
            });
            edits.push(SCGEdit::AddNode {
                label: "sync_barrier".into(),
                kind: NodeKind::Merge,
                bds: vec![],
            });
        }

        if edits.is_empty() {
            // Fallback: suggest a generic function node for the intent.
            edits.push(SCGEdit::AddNode {
                label: format!("new_node_for_{}", intent.replace(' ', "_")),
                kind: NodeKind::Function,
                bds: vec![],
            });
        }

        edits
    }

    /// Find a node in the SCG whose label contains any of the given keywords.
    ///
    /// Returns the first matching node's ID, or 0 if no match is found.
    /// The caller can use 0 as a placeholder node ID for operations that
    /// need a target but don't have an exact match.
    fn find_node_by_keywords(&self, keywords: &[&str]) -> NodeId {
        for node in &self.scg.nodes {
            let label_lower = node.label.to_lowercase();
            for kw in keywords {
                if label_lower.contains(kw) {
                    return node.id;
                }
            }
        }
        0 // no match found — placeholder
    }
}

// ── Conversational session from real SCG ──────────────────────────────────────

/// A conversational session wrapping a projection-side SCG that was created
/// from a real `vuma-scg` SCG.
///
/// This struct owns the projection SCG and provides convenience methods for
/// querying the conversational projection engine without the caller needing
/// to manage the conversion manually.
#[derive(Debug, Clone)]
pub struct ConversationalSession {
    /// The projection-side SCG converted from a real vuma-scg SCG.
    scg: SCG,
    /// The projection engine used for generating descriptions.
    projection: ConversationalProjection,
}

impl ConversationalSession {
    /// Create a new conversational session from a projection SCG.
    pub fn new(scg: SCG) -> Self {
        Self {
            scg,
            projection: ConversationalProjection::new(),
        }
    }

    /// Create a new conversational session with the given verbosity.
    pub fn with_verbosity(scg: SCG, verbosity: Verbosity) -> Self {
        Self {
            scg,
            projection: ConversationalProjection::with_verbosity(verbosity),
        }
    }

    /// Render the entire SCG as a natural-language description.
    pub fn render(&self) -> String {
        self.projection.render_scg(&self.scg)
    }

    /// Explain a specific node.
    pub fn explain_node(&self, node_id: NodeId) -> String {
        self.projection.explain_node(&self.scg, node_id)
    }

    /// Explain a specific region.
    pub fn explain_region(&self, region_id: RegionId) -> String {
        self.projection.explain_region(&self.scg, region_id)
    }

    /// Ask a query about the SCG in natural language.
    ///
    /// Currently this renders the full SCG description. In the future this
    /// could be backed by an LLM for more targeted answers.
    pub fn query(&self, _query: &str) -> String {
        self.render()
    }

    /// Get a reference to the underlying projection SCG.
    pub fn scg(&self) -> &SCG {
        &self.scg
    }
}

/// Create a conversational session from a real vuma-scg SCG.
///
/// This converts the real SCG into the projection's lightweight representation
/// and wraps it in a [`ConversationalSession`] for convenient querying.
pub fn session_from_scg(scg: vuma_scg::SCG) -> ConversationalSession {
    let proj_scg = crate::scg_adapter::from_scg(&scg);
    ConversationalSession::new(proj_scg)
}

/// The conversational projection engine.
///
/// Translates SCG structures and diffs into natural-language descriptions,
/// explanations, and modification suggestions.
#[derive(Debug, Clone)]
pub struct ConversationalProjection {
    /// How verbose the descriptions should be.
    pub verbosity: Verbosity,
}

impl Default for ConversationalProjection {
    fn default() -> Self {
        Self {
            verbosity: Verbosity::Normal,
        }
    }
}

impl ConversationalProjection {
    /// Creates a new conversational projection engine with default verbosity
    /// (Normal).
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new conversational projection engine with the given verbosity.
    pub fn with_verbosity(verbosity: Verbosity) -> Self {
        Self { verbosity }
    }

    // ── Render entire SCG ────────────────────────────────────────────────────

    /// Renders an entire SCG as a natural-language description.
    ///
    /// The output varies by verbosity level:
    /// - **Brief**: one sentence summarising the graph size and key component.
    /// - **Normal**: a paragraph describing the architecture, key nodes, and
    ///   regions.
    /// - **Detailed**: a multi-paragraph document covering every node, edge,
    ///   region, and their relationships.
    pub fn render_scg(&self, scg: &SCG) -> String {
        if scg.nodes.is_empty() {
            return "The semantic computation graph is empty.".to_string();
        }

        match self.verbosity {
            Verbosity::Brief => self.render_scg_brief(scg),
            Verbosity::Normal => self.render_scg_normal(scg),
            Verbosity::Detailed => self.render_scg_detailed(scg),
        }
    }

    fn render_scg_brief(&self, scg: &SCG) -> String {
        let node_count = scg.nodes.len();
        let edge_count = scg.edges.len();
        let region_count = scg.regions.len();

        let mut parts = vec![format!(
            "Graph with {} node(s), {} edge(s)",
            node_count, edge_count
        )];
        if region_count > 0 {
            parts.push(format!("and {} region(s)", region_count));
        }

        // Mention the first function or effect node as a key component.
        if let Some(key) = scg
            .nodes
            .iter()
            .find(|n| matches!(n.kind, NodeKind::Function | NodeKind::Effect))
        {
            parts.push(format!("key component: `{}`", key.label));
        }

        parts.join(", ") + "."
    }

    fn render_scg_normal(&self, scg: &SCG) -> String {
        let mut parts: Vec<String> = Vec::new();

        // Overview sentence.
        parts.push(format!(
            "The semantic computation graph contains {} node(s) and {} edge(s).",
            scg.nodes.len(),
            scg.edges.len()
        ));

        // Count by kind.
        let mut kind_counts: std::collections::HashMap<NodeKind, usize> =
            std::collections::HashMap::new();
        for node in &scg.nodes {
            *kind_counts.entry(node.kind).or_default() += 1;
        }
        let kind_summary: Vec<String> = kind_counts
            .iter()
            .map(|(k, c)| format!("{} {}(s)", c, self.describe_node_kind_noun(k)))
            .collect();
        parts.push(format!("It consists of {}.", kind_summary.join(", ")));

        // Regions.
        if !scg.regions.is_empty() {
            let region_names: Vec<&str> =
                scg.regions.iter().map(|r| r.name.as_str()).collect();
            parts.push(format!(
                "It is organised into {} region(s): {}.",
                scg.regions.len(),
                region_names.join(", ")
            ));
        }

        // Key edges.
        let data_edges = scg
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::DataFlow)
            .count();
        let call_edges = scg
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Call)
            .count();
        let msg_edges = scg
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Message)
            .count();
        let borrow_edges = scg
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Borrow)
            .count();
        let ctrl_edges = scg
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::ControlFlow)
            .count();

        let mut edge_parts: Vec<String> = Vec::new();
        if data_edges > 0 {
            edge_parts.push(format!("{} data-flow", data_edges));
        }
        if call_edges > 0 {
            edge_parts.push(format!("{} call", call_edges));
        }
        if msg_edges > 0 {
            edge_parts.push(format!("{} message", msg_edges));
        }
        if borrow_edges > 0 {
            edge_parts.push(format!("{} borrow", borrow_edges));
        }
        if ctrl_edges > 0 {
            edge_parts.push(format!("{} control-flow", ctrl_edges));
        }
        if !edge_parts.is_empty() {
            parts.push(format!("Edges: {}.", edge_parts.join(", ")));
        }

        parts.join(" ")
    }

    fn render_scg_detailed(&self, scg: &SCG) -> String {
        let mut paragraphs: Vec<String> = Vec::new();

        // Paragraph 1: Overview.
        paragraphs.push(self.render_scg_normal(scg));

        // Paragraph 2: Each node.
        let mut node_descriptions: Vec<String> = Vec::new();
        for node in &scg.nodes {
            node_descriptions.push(self.explain_node(scg, node.id));
        }
        if !node_descriptions.is_empty() {
            paragraphs.push(format!(
                "Node details:\n{}",
                node_descriptions
                    .iter()
                    .map(|d| format!("  - {}", d))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }

        // Paragraph 3: Each region.
        for region in &scg.regions {
            paragraphs.push(self.explain_region(scg, region.id));
        }

        // Paragraph 4: Edge catalogue.
        if !scg.edges.is_empty() {
            let edge_lines: Vec<String> = scg
                .edges
                .iter()
                .map(|e| {
                    let src_label = scg
                        .get_node(e.source)
                        .map(|n| n.label.as_str())
                        .unwrap_or("?");
                    let tgt_label = scg
                        .get_node(e.target)
                        .map(|n| n.label.as_str())
                        .unwrap_or("?");
                    format!(
                        "  - {} ──{:?}──▶ {}",
                        src_label, e.kind, tgt_label
                    )
                })
                .collect();
            paragraphs.push(format!("Edge catalogue:\n{}", edge_lines.join("\n")));
        }

        paragraphs.join("\n\n")
    }

    // ── Explain node ──────────────────────────────────────────────────────────

    /// Explains what a specific node does in plain English.
    ///
    /// At **Brief** verbosity, returns a one-line summary.
    /// At **Normal**, includes BDs and edge relationships.
    /// At **Detailed**, includes region membership and full BD parameters.
    pub fn explain_node(&self, scg: &SCG, node_id: NodeId) -> String {
        let Some(node) = scg.get_node(node_id) else {
            return format!("Node {} does not exist in the graph.", node_id);
        };

        match self.verbosity {
            Verbosity::Brief => {
                format!(
                    "`{}` is a {}.",
                    node.label,
                    self.describe_node_kind(&node.kind)
                )
            }
            Verbosity::Normal => self.describe(scg, node_id),
            Verbosity::Detailed => self.describe_detailed(scg, node_id),
        }
    }

    /// A more detailed version of [`describe`] that includes everything.
    fn describe_detailed(&self, scg: &SCG, node_id: NodeId) -> String {
        let Some(node) = scg.get_node(node_id) else {
            return format!("Node {} does not exist in the graph.", node_id);
        };

        let mut parts: Vec<String> = Vec::new();

        // Role description.
        parts.push(format!(
            "\"{}\" is a {} (id: {}).",
            node.label,
            self.describe_node_kind(&node.kind),
            node.id
        ));

        // Behavioural descriptors — all categories, with parameters.
        if !node.bds.is_empty() {
            for bd in &node.bds {
                let param_str = bd
                    .parameter
                    .as_ref()
                    .map(|p| format!(" with parameter {}", p))
                    .unwrap_or_default();
                let kind_label = self.bd_kind_label(&bd.kind);
                parts.push(format!(
                    "  - It has the {} `{}`{}.",
                    kind_label, bd.name, param_str
                ));
            }
        }

        // Edge relationships — all types.
        let outgoing = scg.outgoing_edges(node_id);
        let incoming = scg.incoming_edges(node_id);

        for edge in &outgoing {
            let tgt = scg
                .get_node(edge.target)
                .map(|n| n.label.as_str())
                .unwrap_or("?");
            let kind_str = self.edge_kind_verb(&edge.kind);
            parts.push(format!("  - It {} `{}`.", kind_str, tgt));
        }

        for edge in &incoming {
            let src = scg
                .get_node(edge.source)
                .map(|n| n.label.as_str())
                .unwrap_or("?");
            let kind_str = self.edge_kind_passive_verb(&edge.kind);
            parts.push(format!("  - It {} from `{}`.", kind_str, src));
        }

        // Region membership.
        if !node.regions.is_empty() {
            let region_names: Vec<String> = node
                .regions
                .iter()
                .filter_map(|&rid| scg.get_region(rid).map(|r| r.name.clone()))
                .collect();
            if !region_names.is_empty() {
                parts.push(format!(
                    "  - It belongs to region(s): {}.",
                    region_names.join(", ")
                ));
            }
        }

        parts.join(" ")
    }

    // ── Explain region ────────────────────────────────────────────────────────

    /// Explains a region's purpose in plain English.
    ///
    /// Describes what the region contains, the collective role of its nodes,
    /// and (at higher verbosity) the relationships between the region's nodes.
    pub fn explain_region(&self, scg: &SCG, region_id: RegionId) -> String {
        let Some(region) = scg.get_region(region_id) else {
            return format!("Region {} does not exist in the graph.", region_id);
        };

        match self.verbosity {
            Verbosity::Brief => {
                format!(
                    "Region `{}` contains {} node(s).",
                    region.name,
                    region.nodes.len()
                )
            }
            Verbosity::Normal => self.explain_region_normal(scg, region_id),
            Verbosity::Detailed => self.explain_region_detailed(scg, region_id),
        }
    }

    fn explain_region_normal(&self, scg: &SCG, region_id: RegionId) -> String {
        let region = scg.get_region(region_id).unwrap();

        if region.nodes.is_empty() {
            return format!("Region `{}` is currently empty.", region.name);
        }

        let mut parts: Vec<String> = Vec::new();

        parts.push(format!(
            "Region `{}` is a group of {} node(s) that work together.",
            region.name,
            region.nodes.len()
        ));

        // Describe the kinds of nodes in the region.
        let mut kind_counts: std::collections::HashMap<NodeKind, usize> =
            std::collections::HashMap::new();
        let mut node_labels: Vec<String> = Vec::new();
        for &nid in &region.nodes {
            if let Some(node) = scg.get_node(nid) {
                *kind_counts.entry(node.kind).or_default() += 1;
                node_labels.push(format!("`{}`", node.label));
            }
        }

        if !node_labels.is_empty() {
            parts.push(format!("It contains: {}.", node_labels.join(", ")));
        }

        let kind_summary: Vec<String> = kind_counts
            .iter()
            .map(|(k, c)| format!("{} {}(s)", c, self.describe_node_kind_noun(k)))
            .collect();
        if !kind_summary.is_empty() {
            parts.push(format!("By type: {}.", kind_summary.join(", ")));
        }

        // Describe internal data-flow edges.
        let internal_edges: Vec<&crate::SCGEdge> = scg
            .edges
            .iter()
            .filter(|e| {
                region.nodes.contains(&e.source) && region.nodes.contains(&e.target)
            })
            .collect();

        if !internal_edges.is_empty() {
            let data_count = internal_edges
                .iter()
                .filter(|e| e.kind == EdgeKind::DataFlow)
                .count();
            let call_count = internal_edges
                .iter()
                .filter(|e| e.kind == EdgeKind::Call)
                .count();
            let msg_count = internal_edges
                .iter()
                .filter(|e| e.kind == EdgeKind::Message)
                .count();

            let mut edge_parts: Vec<String> = Vec::new();
            if data_count > 0 {
                edge_parts.push(format!("{} data-flow", data_count));
            }
            if call_count > 0 {
                edge_parts.push(format!("{} call", call_count));
            }
            if msg_count > 0 {
                edge_parts.push(format!("{} message", msg_count));
            }
            if !edge_parts.is_empty() {
                parts.push(format!(
                    "Internal connections: {}.",
                    edge_parts.join(", ")
                ));
            }
        }

        parts.join(" ")
    }

    fn explain_region_detailed(&self, scg: &SCG, region_id: RegionId) -> String {
        let region = scg.get_region(region_id).unwrap();
        let normal = self.explain_region_normal(scg, region_id);

        let mut parts: Vec<String> = vec![normal];

        // Per-node details.
        for &nid in &region.nodes {
            if let Some(node) = scg.get_node(nid) {
                let bds: Vec<String> = node
                    .bds
                    .iter()
                    .map(|bd| {
                        let param = bd
                            .parameter
                            .as_ref()
                            .map(|p| format!("({})", p))
                            .unwrap_or_default();
                        format!("{}{} [{:?}]", bd.name, param, bd.kind)
                    })
                    .collect();
                if bds.is_empty() {
                    parts.push(format!(
                        "  - Node `{}` ({}): no behavioural descriptors.",
                        node.label,
                        self.describe_node_kind(&node.kind)
                    ));
                } else {
                    parts.push(format!(
                        "  - Node `{}` ({}): BDs = {}.",
                        node.label,
                        self.describe_node_kind(&node.kind),
                        bds.join(", ")
                    ));
                }
            }
        }

        // External edges (crossing the region boundary).
        let external_edges: Vec<&crate::SCGEdge> = scg
            .edges
            .iter()
            .filter(|e| {
                let src_in = region.nodes.contains(&e.source);
                let tgt_in = region.nodes.contains(&e.target);
                src_in != tgt_in // exactly one end inside
            })
            .collect();

        if !external_edges.is_empty() {
            let ext_lines: Vec<String> = external_edges
                .iter()
                .map(|e| {
                    let src = scg
                        .get_node(e.source)
                        .map(|n| n.label.as_str())
                        .unwrap_or("?");
                    let tgt = scg
                        .get_node(e.target)
                        .map(|n| n.label.as_str())
                        .unwrap_or("?");
                    format!(
                        "  - {} ──{:?}──▶ {} (crosses boundary)",
                        src, e.kind, tgt
                    )
                })
                .collect();
            parts.push(format!(
                "External connections:\n{}",
                ext_lines.join("\n")
            ));
        }

        parts.join("\n")
    }

    // ── Explain verification ──────────────────────────────────────────────────

    /// Explains the results of a verification pass in natural language.
    ///
    /// At **Brief** verbosity, a single pass/fail sentence.
    /// At **Normal**, a summary of checks and violations.
    /// At **Detailed**, a full breakdown of each violation.
    pub fn explain_verification(&self, result: &AggregatedResult) -> String {
        match self.verbosity {
            Verbosity::Brief => {
                if result.is_ok() {
                    format!(
                        "All {} check(s) passed.",
                        result.total_checks
                    )
                } else {
                    format!(
                        "{} of {} check(s) failed.",
                        result.failed(),
                        result.total_checks
                    )
                }
            }
            Verbosity::Normal => self.explain_verification_normal(result),
            Verbosity::Detailed => self.explain_verification_detailed(result),
        }
    }

    fn explain_verification_normal(&self, result: &AggregatedResult) -> String {
        let mut parts: Vec<String> = Vec::new();

        parts.push(format!(
            "Verification completed: {} check(s) performed, {} passed, {} failed.",
            result.total_checks,
            result.passed,
            result.failed()
        ));

        if result.violations.is_empty() {
            parts.push("No violations were found.".to_string());
        } else {
            // Group by severity.
            let warnings = result.violations_by_severity(ViolationSeverity::Warning);
            let errors = result.violations_by_severity(ViolationSeverity::Error);
            let criticals = result.violations_by_severity(ViolationSeverity::Critical);

            if !criticals.is_empty() {
                parts.push(format!(
                    "{} critical violation(s) found.",
                    criticals.len()
                ));
            }
            if !errors.is_empty() {
                parts.push(format!("{} error(s) found.", errors.len()));
            }
            if !warnings.is_empty() {
                parts.push(format!("{} warning(s) found.", warnings.len()));
            }

            // Summarise each violation in one line.
            for v in &result.violations {
                let location = v
                    .node_id
                    .map(|nid| format!(" at node {}", nid))
                    .or_else(|| v.region_id.map(|rid| format!(" in region {}", rid)))
                    .unwrap_or_default();
                parts.push(format!(
                    "  - [{}] {}{}: {}",
                    self.severity_label(&v.severity),
                    v.code,
                    location,
                    v.message
                ));
            }
        }

        parts.join(" ")
    }

    fn explain_verification_detailed(&self, result: &AggregatedResult) -> String {
        let normal = self.explain_verification_normal(result);
        if result.violations.is_empty() {
            return normal;
        }

        let mut parts: Vec<String> = vec![normal];
        parts.push(String::new());
        parts.push("Violation details:".to_string());

        for (i, v) in result.violations.iter().enumerate() {
            parts.push(format!("{}. [{}] {}", i + 1, v.code, v.message));
            parts.push(format!("   Severity: {}", self.severity_label(&v.severity)));
            if let Some(nid) = v.node_id {
                parts.push(format!("   Affected node: {}", nid));
            }
            if let Some(rid) = v.region_id {
                parts.push(format!("   Affected region: {}", rid));
            }
            if let Some(ref suggestion) = v.suggestion {
                parts.push(format!("   Suggestion: {}", suggestion));
            }
        }

        parts.join("\n")
    }

    // ── Suggest fix ───────────────────────────────────────────────────────────

    /// Suggests how to fix a verification violation in natural language.
    ///
    /// The suggestion is based on the violation code and message. For common
    /// violation patterns, specific remediation advice is provided. For
    /// unknown codes, a generic suggestion is returned.
    pub fn suggest_fix(&self, violation: &Violation) -> String {
        let base = match violation.code.as_str() {
            // Missing capability violations.
            code if code.starts_with("BD-MISSING-") => {
                let bd_name = code.strip_prefix("BD-MISSING-").unwrap_or("unknown");
                let node_str = violation
                    .node_id
                    .map(|nid| format!("node {}", nid))
                    .unwrap_or_else(|| "the affected node".to_string());
                format!(
                    "Add the `{}` behavioural descriptor to {}. This can \
                     typically be done by annotating the node with the \
                     required capability in the SCG, or by refactoring the \
                     node's implementation to satisfy the `{}` contract.",
                    bd_name, node_str, bd_name
                )
            }
            // Unsafe dataflow violations.
            code if code.starts_with("EDGE-UNSAFE-") => {
                let edge_type = code
                    .strip_prefix("EDGE-UNSAFE-")
                    .unwrap_or("unknown");
                format!(
                    "The {} edge is considered unsafe. Consider adding a \
                     safety check or validation step along this edge, or \
                     replacing the direct connection with a message-passing \
                     channel to isolate the unsafety.",
                    edge_type
                )
            }
            // Region coherence violations.
            code if code.starts_with("REGION-") => {
                let region_str = violation
                    .region_id
                    .map(|rid| format!("region {}", rid))
                    .unwrap_or_else(|| "the region".to_string());
                format!(
                    "There is a coherence problem in {}. Review the nodes \
                     within the region and ensure that all behavioural \
                     descriptors are consistent. A node may need to be moved \
                     to a different region, or the region boundary may need \
                     to be adjusted.",
                    region_str
                )
            }
            // Thread safety violations.
            code if code.contains("THREAD") || code.contains("CONCURRENT") => {
                "This is a thread-safety violation. Consider adding `Send` \
                 and/or `Sync` behavioural descriptors to the affected node, \
                 or wrap shared data in a thread-safe container (e.g. a \
                 mutex or atomic)."
                    .to_string()
            }
            // Memory safety violations.
            code if code.contains("MEMORY") || code.contains("PIN") || code.contains("ALIAS") => {
                "This is a memory-safety violation. Ensure that memory \
                 access follows the borrow rules: no mutable aliasing, \
                 and pinned data is not moved. Consider adding the `Pin` \
                 or `NoAlias` behavioural descriptor as appropriate."
                    .to_string()
            }
            // Side-effect violations.
            code if code.contains("SIDE-EFFECT") || code.contains("EFFECT") => {
                "This violation involves an unexpected side effect. Mark \
                 the node with the `SideEffect` behavioural descriptor, \
                 or refactor to eliminate the side effect if purity is \
                 required."
                    .to_string()
            }
            // Default / unknown.
            _ => {
                format!(
                    "Review the violation `{}` and address the underlying \
                     issue described: {}. If you are unsure how to proceed, \
                     consider consulting the VUMA documentation or using \
                     `suggest_modification` with a relevant intent string.",
                    violation.code, violation.message
                )
            }
        };

        match self.verbosity {
            Verbosity::Brief => {
                // Return just the first sentence.
                base.split('.')
                    .next()
                    .map(|s| s.trim().to_string())
                    .unwrap_or(base)
                    + "."
            }
            Verbosity::Normal => base,
            Verbosity::Detailed => {
                let mut detailed = base;

                // Add context about the violation.
                detailed.push_str(&format!(
                    "\n\nViolation details: code=`{}`, severity={}",
                    violation.code,
                    self.severity_label(&violation.severity)
                ));
                if let Some(nid) = violation.node_id {
                    detailed.push_str(&format!(", node_id={}", nid));
                }
                if let Some(rid) = violation.region_id {
                    detailed.push_str(&format!(", region_id={}", rid));
                }
                if let Some(ref suggestion) = violation.suggestion {
                    detailed.push_str(&format!("\nOriginal suggestion: {}", suggestion));
                }

                detailed
            }
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
        if !node.bds.is_empty() {
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
                parts.push(format!(
                    "It has the following capabilities: {}.",
                    caps.join(", ")
                ));
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
                parts.push(format!("It calls: {}.", call_targets.join(", ")));
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

        // ── Region membership ─────────────────────────────────────────────
        if !node.regions.is_empty() {
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
            NodeKind::Allocation => "memory allocation operation",
            NodeKind::Deallocation => "memory deallocation operation",
            NodeKind::Access => "memory access operation",
            NodeKind::Computation => "pure computation step",
        }
    }

    /// Returns a noun form of a [`NodeKind`] (e.g. "function" not "function
    /// that performs a computation").
    fn describe_node_kind_noun(&self, kind: &NodeKind) -> &'static str {
        match kind {
            NodeKind::Function => "function",
            NodeKind::Value => "value",
            NodeKind::MessageSend => "message-send",
            NodeKind::MessageReceive => "message-receive",
            NodeKind::Merge => "merge",
            NodeKind::Effect => "effect",
            NodeKind::Module => "module",
            NodeKind::Allocation => "allocation",
            NodeKind::Deallocation => "deallocation",
            NodeKind::Access => "access",
            NodeKind::Computation => "computation",
        }
    }

    /// Returns a verb phrase for an outgoing edge kind (e.g. "sends data to").
    fn edge_kind_verb(&self, kind: &EdgeKind) -> &'static str {
        match kind {
            EdgeKind::DataFlow => "sends data to",
            EdgeKind::ControlFlow => "transfers control to",
            EdgeKind::Message => "sends messages to",
            EdgeKind::Borrow => "borrows from",
            EdgeKind::Call => "calls",
            EdgeKind::Derivation => "derives from",
            EdgeKind::Annotation => "annotates",
        }
    }

    /// Returns a passive-verb phrase for an incoming edge kind
    /// (e.g. "receives data").
    fn edge_kind_passive_verb(&self, kind: &EdgeKind) -> &'static str {
        match kind {
            EdgeKind::DataFlow => "receives data",
            EdgeKind::ControlFlow => "receives control from",
            EdgeKind::Message => "receives messages from",
            EdgeKind::Borrow => "is borrowed by",
            EdgeKind::Call => "is called by",
            EdgeKind::Derivation => "is derived from",
            EdgeKind::Annotation => "is annotated by",
        }
    }

    /// Returns a human-readable label for a [`BdKind`].
    fn bd_kind_label(&self, kind: &BdKind) -> &'static str {
        match kind {
            BdKind::Capability => "capability",
            BdKind::MemoryLayout => "memory-layout property",
            BdKind::Safety => "safety property",
            BdKind::Relation => "relation",
            BdKind::Custom => "custom property",
        }
    }

    /// Returns a human-readable label for a [`ViolationSeverity`].
    fn severity_label(&self, severity: &ViolationSeverity) -> &'static str {
        match severity {
            ViolationSeverity::Warning => "warning",
            ViolationSeverity::Error => "error",
            ViolationSeverity::Critical => "critical",
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
    /// `"add rate limiting"` or `"make auth_handler thread-safe"`. The
    /// [`SuggestionEngine`] analyses the SCG structure and the intent string
    /// using rule-based heuristics to propose relevant edits.
    pub fn suggest_modification(&self, scg: &SCG, intent: &str) -> Vec<SCGEdit> {
        let engine = SuggestionEngine::new(scg);
        engine.generate_suggestions(intent)
    }

    // ── AI-driven structured output ───────────────────────────────────────────

    /// Generates a structured [`AIExplainerOutput`] for a specific node.
    ///
    /// This output is designed to be serialised (e.g. as JSON) and fed to an
    /// LLM for refinement. The LLM can use the `key_facts` and
    /// `related_entities` fields to produce a polished, context-aware
    /// explanation.
    pub fn to_ai_prompt_node(&mut self, scg: &SCG, node_id: NodeId) -> AIExplainerOutput {
        let Some(node) = scg.get_node(node_id) else {
            return AIExplainerOutput {
                entity_type: "node".into(),
                entity_id: node_id.to_string(),
                explanation: format!("Node {} does not exist in the graph.", node_id),
                brief_summary: format!("Non-existent node {}", node_id),
                detailed_explanation: format!(
                    "Node {} does not exist in the graph and cannot be explained.",
                    node_id
                ),
                key_facts: vec![],
                related_entities: vec![],
                schema_version: "1.0".into(),
            };
        };

        let outgoing = scg.outgoing_edges(node_id);
        let incoming = scg.incoming_edges(node_id);

        let brief = format!(
            "`{}` is a {}.",
            node.label,
            self.describe_node_kind(&node.kind)
        );

        // render_with_verbosity internally clones self and adjusts verbosity,
        // so no save/restore is needed.

        // Normal explanation.
        let _normal = self.render_with_verbosity(Verbosity::Normal, |proj| proj.describe(scg, node_id));
        // Detailed explanation.
        let detailed = self.render_with_verbosity(Verbosity::Detailed, |proj| {
            proj.describe_detailed(scg, node_id)
        });

        let mut key_facts: Vec<(String, String)> = vec![
            ("label".into(), node.label.clone()),
            ("kind".into(), format!("{:?}", node.kind)),
            ("id".into(), node_id.to_string()),
            ("bd_count".into(), node.bds.len().to_string()),
            ("outgoing_edges".into(), outgoing.len().to_string()),
            ("incoming_edges".into(), incoming.len().to_string()),
        ];

        for bd in &node.bds {
            key_facts.push((
                format!("bd_{}", bd.name),
                format!("{:?}{}", bd.kind, bd.parameter.as_ref().map(|p| format!("({})", p)).unwrap_or_default()),
            ));
        }

        let related: Vec<String> = outgoing
            .iter()
            .map(|e| format!("node_{}", e.target))
            .chain(incoming.iter().map(|e| format!("node_{}", e.source)))
            .chain(
                node.regions
                    .iter()
                    .map(|&rid| format!("region_{}", rid)),
            )
            .collect();

        AIExplainerOutput {
            entity_type: "node".into(),
            entity_id: node_id.to_string(),
            explanation: self.render_with_verbosity(Verbosity::Normal, |proj| {
                proj.describe(scg, node_id)
            }),
            brief_summary: brief,
            detailed_explanation: detailed,
            key_facts,
            related_entities: related,
            schema_version: "1.0".into(),
        }
    }

    /// Generates a structured [`AIExplainerOutput`] for a specific region.
    pub fn to_ai_prompt_region(&self, scg: &SCG, region_id: RegionId) -> AIExplainerOutput {
        let Some(region) = scg.get_region(region_id) else {
            return AIExplainerOutput {
                entity_type: "region".into(),
                entity_id: region_id.to_string(),
                explanation: format!("Region {} does not exist in the graph.", region_id),
                brief_summary: format!("Non-existent region {}", region_id),
                detailed_explanation: format!(
                    "Region {} does not exist in the graph and cannot be explained.",
                    region_id
                ),
                key_facts: vec![],
                related_entities: vec![],
                schema_version: "1.0".into(),
            };
        };

        let brief = format!(
            "Region `{}` contains {} node(s).",
            region.name,
            region.nodes.len()
        );

        let normal = self.render_with_verbosity(Verbosity::Normal, |proj| {
            proj.explain_region_normal(scg, region_id)
        });
        let detailed = self.render_with_verbosity(Verbosity::Detailed, |proj| {
            proj.explain_region_detailed(scg, region_id)
        });

        let key_facts: Vec<(String, String)> = vec![
            ("name".into(), region.name.clone()),
            ("id".into(), region_id.to_string()),
            ("node_count".into(), region.nodes.len().to_string()),
            (
                "node_ids".into(),
                region
                    .nodes
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        ];

        let related: Vec<String> = region
            .nodes
            .iter()
            .map(|&nid| format!("node_{}", nid))
            .collect();

        AIExplainerOutput {
            entity_type: "region".into(),
            entity_id: region_id.to_string(),
            explanation: normal,
            brief_summary: brief,
            detailed_explanation: detailed,
            key_facts,
            related_entities: related,
            schema_version: "1.0".into(),
        }
    }

    /// Generates a structured [`AIExplainerOutput`] for a verification result.
    pub fn to_ai_prompt_verification(&self, result: &AggregatedResult) -> AIExplainerOutput {
        let brief = if result.is_ok() {
            format!("All {} checks passed.", result.total_checks)
        } else {
            format!(
                "{} of {} checks failed.",
                result.failed(),
                result.total_checks
            )
        };

        let normal = self.render_with_verbosity(Verbosity::Normal, |proj| {
            proj.explain_verification_normal(result)
        });
        let detailed = self.render_with_verbosity(Verbosity::Detailed, |proj| {
            proj.explain_verification_detailed(result)
        });

        let mut key_facts: Vec<(String, String)> = vec![
            ("total_checks".into(), result.total_checks.to_string()),
            ("passed".into(), result.passed.to_string()),
            ("failed".into(), result.failed().to_string()),
            ("violation_count".into(), result.violation_count().to_string()),
        ];

        for v in &result.violations {
            key_facts.push((
                format!("violation_{}", v.code),
                format!(
                    "{:?}: {}",
                    v.severity, v.message
                ),
            ));
        }

        let related: Vec<String> = result
            .violations
            .iter()
            .filter_map(|v| v.node_id.map(|nid| format!("node_{}", nid)))
            .chain(
                result
                    .violations
                    .iter()
                    .filter_map(|v| v.region_id.map(|rid| format!("region_{}", rid))),
            )
            .collect();

        AIExplainerOutput {
            entity_type: "verification_result".into(),
            entity_id: "aggregated".into(),
            explanation: normal,
            brief_summary: brief,
            detailed_explanation: detailed,
            key_facts,
            related_entities: related,
            schema_version: "1.0".into(),
        }
    }

    /// Helper: temporarily render with a different verbosity.
    fn render_with_verbosity<F, R>(&self, v: Verbosity, f: F) -> R
    where
        F: FnOnce(&ConversationalProjection) -> R,
    {
        let mut proj = self.clone();
        proj.verbosity = v;
        f(&proj)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BehaviouralDescriptor, BdKind, SCGEdge, SCGNode, SCGRegion};

    // ── Helpers ───────────────────────────────────────────────────────────

    fn sample_scg() -> SCG {
        SCG {
            nodes: vec![
                SCGNode {
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
                    regions: vec![10],
                },
                SCGNode {
                    id: 2,
                    label: "session_token".into(),
                    kind: NodeKind::Value,
                    bds: vec![
                        BehaviouralDescriptor {
                            id: 101,
                            name: "Pin".into(),
                            kind: BdKind::MemoryLayout,
                            parameter: None,
                        },
                    ],
                    regions: vec![10],
                },
            ],
            edges: vec![SCGEdge {
                id: 10,
                source: 1,
                target: 2,
                kind: EdgeKind::DataFlow,
            }],
            regions: vec![SCGRegion {
                id: 10,
                name: "authentication".into(),
                nodes: vec![1, 2],
            }],
        }
    }

    fn sample_violation() -> Violation {
        Violation {
            code: "BD-MISSING-Send".into(),
            message: "Node 5 is missing the Send capability required for concurrent access.".into(),
            severity: ViolationSeverity::Error,
            node_id: Some(5),
            region_id: None,
            suggestion: Some("Add the Send BD to node 5.".into()),
        }
    }

    fn sample_aggregated_result() -> AggregatedResult {
        AggregatedResult {
            total_checks: 10,
            passed: 7,
            violations: vec![
                Violation {
                    code: "BD-MISSING-Send".into(),
                    message: "Node 5 is missing Send.".into(),
                    severity: ViolationSeverity::Error,
                    node_id: Some(5),
                    region_id: None,
                    suggestion: None,
                },
                Violation {
                    code: "REGION-COHERENCE".into(),
                    message: "Region 3 has inconsistent BDs.".into(),
                    severity: ViolationSeverity::Warning,
                    node_id: None,
                    region_id: Some(3),
                    suggestion: None,
                },
                Violation {
                    code: "EDGE-UNSAFE-DATAFLOW".into(),
                    message: "Unsafe data flow from node 1 to node 4.".into(),
                    severity: ViolationSeverity::Critical,
                    node_id: Some(1),
                    region_id: None,
                    suggestion: Some("Add a validation step.".into()),
                },
            ],
        }
    }

    // ── Test 1: explain_node for existing node ───────────────────────────

    #[test]
    fn explain_node_existing() {
        let scg = sample_scg();
        let proj = ConversationalProjection::with_verbosity(Verbosity::Normal);
        let desc = proj.explain_node(&scg, 1);
        assert!(desc.contains("auth_handler"));
        assert!(desc.contains("function"));
        assert!(desc.contains("Send"));
    }

    // ── Test 2: explain_node for non-existent node ──────────────────────

    #[test]
    fn explain_node_nonexistent() {
        let scg = sample_scg();
        let proj = ConversationalProjection::new();
        let desc = proj.explain_node(&scg, 999);
        assert!(desc.contains("does not exist"));
    }

    // ── Test 3: explain_region ──────────────────────────────────────────

    #[test]
    fn explain_region_existing() {
        let scg = sample_scg();
        let proj = ConversationalProjection::with_verbosity(Verbosity::Normal);
        let desc = proj.explain_region(&scg, 10);
        assert!(desc.contains("authentication"));
        assert!(desc.contains("node(s)"));
        assert!(desc.contains("auth_handler") || desc.contains("session_token"));
    }

    // ── Test 4: explain_region for non-existent region ──────────────────

    #[test]
    fn explain_region_nonexistent() {
        let scg = sample_scg();
        let proj = ConversationalProjection::new();
        let desc = proj.explain_region(&scg, 999);
        assert!(desc.contains("does not exist"));
    }

    // ── Test 5: explain_verification ────────────────────────────────────

    #[test]
    fn explain_verification_with_violations() {
        let result = sample_aggregated_result();
        let proj = ConversationalProjection::with_verbosity(Verbosity::Normal);
        let desc = proj.explain_verification(&result);
        assert!(desc.contains("10"));
        assert!(desc.contains("7"));
        assert!(desc.contains("BD-MISSING-Send"));
        assert!(desc.contains("REGION-COHERENCE"));
    }

    // ── Test 6: explain_verification all passed ─────────────────────────

    #[test]
    fn explain_verification_all_passed() {
        let result = AggregatedResult::all_passed(5);
        let proj = ConversationalProjection::with_verbosity(Verbosity::Brief);
        let desc = proj.explain_verification(&result);
        assert!(desc.contains("passed"));
        assert!(desc.contains("5"));
    }

    // ── Test 7: suggest_fix for BD-MISSING violation ───────────────────

    #[test]
    fn suggest_fix_missing_bd() {
        let violation = sample_violation();
        let proj = ConversationalProjection::with_verbosity(Verbosity::Normal);
        let fix = proj.suggest_fix(&violation);
        assert!(fix.contains("Send"));
        assert!(fix.contains("node 5") || fix.contains("affected node"));
    }

    // ── Test 8: suggest_fix for thread safety violation ─────────────────

    #[test]
    fn suggest_fix_thread_safety() {
        let violation = Violation {
            code: "THREAD-SAFETY-01".into(),
            message: "Data race detected.".into(),
            severity: ViolationSeverity::Critical,
            node_id: Some(7),
            region_id: None,
            suggestion: None,
        };
        let proj = ConversationalProjection::new();
        let fix = proj.suggest_fix(&violation);
        assert!(fix.contains("Send") || fix.contains("Sync") || fix.contains("thread"));
    }

    // ── Test 9: render_scg for non-empty graph ──────────────────────────

    #[test]
    fn render_scg_nonempty() {
        let scg = sample_scg();
        let proj = ConversationalProjection::with_verbosity(Verbosity::Brief);
        let desc = proj.render_scg(&scg);
        assert!(desc.contains("2 node(s)"));
        assert!(desc.contains("1 edge(s)"));
    }

    // ── Test 10: render_scg for empty graph ─────────────────────────────

    #[test]
    fn render_scg_empty() {
        let scg = SCG::empty();
        let proj = ConversationalProjection::new();
        let desc = proj.render_scg(&scg);
        assert!(desc.contains("empty"));
    }

    // ── Test 11: verbosity levels for explain_node ──────────────────────

    #[test]
    fn explain_node_verbosity_levels() {
        let scg = sample_scg();

        let brief = ConversationalProjection::with_verbosity(Verbosity::Brief);
        let brief_desc = brief.explain_node(&scg, 1);

        let normal = ConversationalProjection::with_verbosity(Verbosity::Normal);
        let normal_desc = normal.explain_node(&scg, 1);

        let detailed = ConversationalProjection::with_verbosity(Verbosity::Detailed);
        let detailed_desc = detailed.explain_node(&scg, 1);

        // Brief should be shortest.
        assert!(brief_desc.len() <= normal_desc.len());
        // Detailed should mention region.
        assert!(detailed_desc.contains("region") || detailed_desc.contains("Region"));
    }

    // ── Test 12: AI-driven structured output ────────────────────────────

    #[test]
    fn to_ai_prompt_node() {
        let scg = sample_scg();
        let mut proj = ConversationalProjection::new();
        let output = proj.to_ai_prompt_node(&scg, 1);
        assert_eq!(output.entity_type, "node");
        assert_eq!(output.entity_id, "1");
        assert!(output.brief_summary.contains("auth_handler"));
        assert!(!output.key_facts.is_empty());
        assert!(!output.related_entities.is_empty());
        assert_eq!(output.schema_version, "1.0");
    }

    // ── Test 13: suggest_fix for unknown violation code ─────────────────

    #[test]
    fn suggest_fix_unknown_code() {
        let violation = Violation {
            code: "CUSTOM-001".into(),
            message: "Something went wrong.".into(),
            severity: ViolationSeverity::Warning,
            node_id: None,
            region_id: None,
            suggestion: None,
        };
        let proj = ConversationalProjection::new();
        let fix = proj.suggest_fix(&violation);
        assert!(fix.contains("CUSTOM-001") || fix.contains("review"));
    }

    // ── Test 14: suggest_rate_limiting ──────────────────────────────────

    #[test]
    fn suggest_rate_limiting() {
        let proj = ConversationalProjection::new();
        let scg = SCG::empty();
        let edits = proj.suggest_modification(&scg, "add rate limiting to the API");
        assert!(!edits.is_empty());
        assert!(edits.iter().any(|e| matches!(e, SCGEdit::AddNode { label, .. } if label == "rate_limiter")));
    }

    // ── Test 15: suggest_thread_safety ──────────────────────────────────

    #[test]
    fn suggest_thread_safety() {
        let proj = ConversationalProjection::new();
        let scg = SCG::empty();
        let edits = proj.suggest_modification(&scg, "make auth_handler thread-safe");
        assert!(edits.iter().any(|e| matches!(e, SCGEdit::ChangeBD { bd_name, .. } if bd_name == "Send")));
    }

    // ── Test 16: AI prompt for verification result ─────────────────────

    #[test]
    fn to_ai_prompt_verification() {
        let result = sample_aggregated_result();
        let proj = ConversationalProjection::new();
        let output = proj.to_ai_prompt_verification(&result);
        assert_eq!(output.entity_type, "verification_result");
        assert!(output.brief_summary.contains("3") || output.brief_summary.contains("failed"));
        assert!(output.key_facts.len() >= 4); // at least total_checks, passed, failed, violation_count
    }

    // ── Test 17: AI prompt for region ───────────────────────────────────

    #[test]
    fn to_ai_prompt_region() {
        let scg = sample_scg();
        let proj = ConversationalProjection::new();
        let output = proj.to_ai_prompt_region(&scg, 10);
        assert_eq!(output.entity_type, "region");
        assert!(output.brief_summary.contains("authentication"));
        assert!(output.related_entities.contains(&"node_1".to_string()));
        assert!(output.related_entities.contains(&"node_2".to_string()));
    }

    // ── Test 18: explain_verification detailed ──────────────────────────

    #[test]
    fn explain_verification_detailed() {
        let result = sample_aggregated_result();
        let proj = ConversationalProjection::with_verbosity(Verbosity::Detailed);
        let desc = proj.explain_verification(&result);
        assert!(desc.contains("Violation details"));
        assert!(desc.contains("Severity"));
        assert!(desc.contains("BD-MISSING-Send"));
    }

    // ── Test 19: render_scg detailed mode ───────────────────────────────

    #[test]
    fn render_scg_detailed() {
        let scg = sample_scg();
        let proj = ConversationalProjection::with_verbosity(Verbosity::Detailed);
        let desc = proj.render_scg(&scg);
        assert!(desc.contains("Node details") || desc.contains("auth_handler"));
        assert!(desc.contains("authentication")); // region
        assert!(desc.contains("Edge catalogue") || desc.contains("DataFlow"));
    }

    // ── Test 20: suggest_fix for region violation ──────────────────────

    #[test]
    fn suggest_fix_region_violation() {
        let violation = Violation {
            code: "REGION-COHERENCE".into(),
            message: "Region has inconsistent BDs.".into(),
            severity: ViolationSeverity::Warning,
            node_id: None,
            region_id: Some(3),
            suggestion: None,
        };
        let proj = ConversationalProjection::new();
        let fix = proj.suggest_fix(&violation);
        assert!(fix.contains("region") || fix.contains("coherence"));
    }

    // ── Test 21: session from real vuma-scg SCG ────────────────────────

    #[test]
    fn test_session_from_real_scg() {
        let rid = vuma_scg::RegionId::new(1);
        let mut real_scg = vuma_scg::SCG::new();

        // Add an allocation node
        let alloc_id = real_scg.add_node(
            vuma_scg::NodeType::Allocation,
            vuma_scg::NodePayload::Allocation(vuma_scg::AllocationNode {
                size: 256,
                align: 16,
                region_id: rid,
                type_name: Some("MyBuffer".to_string()),
            }),
            vuma_scg::ProgramPoint {
                file: Some("main.vu".to_string()),
                line: Some(10),
                column: Some(5),
                offset: None,
            },
        );

        // Add a computation node
        let comp_id = real_scg.add_node(
            vuma_scg::NodeType::Computation,
            vuma_scg::NodePayload::Computation(vuma_scg::ComputationNode {
                operation: "write_buffer".to_string(),
                result_type: None,
                tail_call: false,
            }),
            vuma_scg::ProgramPoint {
                file: Some("main.vu".to_string()),
                line: Some(11),
                column: Some(3),
                offset: None,
            },
        );

        // Add an edge
        real_scg
            .add_edge(alloc_id, comp_id, vuma_scg::EdgeKind::DataFlow)
            .unwrap();

        // Create a session from the real SCG
        let session = super::session_from_scg(real_scg);

        // Verify the session works
        let desc = session.render();
        assert!(
            desc.contains("node") || desc.contains("Node"),
            "Expected node description, got: {}",
            desc
        );

        // Ask a query
        let answer = session.query("describe the graph");
        assert!(!answer.is_empty(), "Query answer should not be empty");

        // Explain a node by its projection ID
        let node_desc = session.explain_node(alloc_id.as_u64());
        assert!(
            node_desc.contains("alloc") || node_desc.contains("MyBuffer") || node_desc.contains("Allocation"),
            "Expected allocation node description, got: {}",
            node_desc
        );
    }

    // ── Test 22: conversational roundtrip ───────────────────────────────

    #[test]
    fn test_conversational_roundtrip() {
        // Create a real SCG, convert to projection, get description,
        // then convert back to real SCG and verify structural equivalence.
        let rid = vuma_scg::RegionId::new(1);
        let mut real_scg = vuma_scg::SCG::new();

        let alloc_id = real_scg.add_node(
            vuma_scg::NodeType::Allocation,
            vuma_scg::NodePayload::Allocation(vuma_scg::AllocationNode {
                size: 128,
                align: 8,
                region_id: rid,
                type_name: Some("DataBlock".to_string()),
            }),
            vuma_scg::ProgramPoint {
                file: None,
                line: None,
                column: None,
                offset: None,
            },
        );

        let dealloc_id = real_scg.add_node(
            vuma_scg::NodeType::Deallocation,
            vuma_scg::NodePayload::Deallocation(vuma_scg::DeallocationNode {
                allocation_node: alloc_id,
                region_id: rid,
            }),
            vuma_scg::ProgramPoint {
                file: None,
                line: None,
                column: None,
                offset: None,
            },
        );

        real_scg
            .add_edge(alloc_id, dealloc_id, vuma_scg::EdgeKind::Derivation)
            .unwrap();

        // Convert to projection
        let proj_scg = crate::scg_adapter::from_scg(&real_scg);

        // Get conversational output
        let session = super::ConversationalSession::new(proj_scg.clone());
        let desc = session.render();
        assert!(!desc.is_empty(), "Conversational output should not be empty");

        // Convert back to real SCG
        let roundtrip_scg = crate::scg_adapter::to_scg(&proj_scg);

        // Verify structural equivalence: same number of nodes and edges
        assert_eq!(
            roundtrip_scg.node_count(),
            real_scg.node_count(),
            "Node count should be preserved in roundtrip"
        );
        assert_eq!(
            roundtrip_scg.edge_count(),
            real_scg.edge_count(),
            "Edge count should be preserved in roundtrip"
        );

        // Verify node types are preserved where possible
        let orig_alloc_type = real_scg.get_node(alloc_id).map(|n| n.node_type.clone());
        let rt_alloc_type = roundtrip_scg.get_node(alloc_id).map(|n| n.node_type.clone());
        assert_eq!(
            orig_alloc_type, rt_alloc_type,
            "Allocation node type should be preserved in roundtrip"
        );
    }
}
