//! # Textual Projection
//!
//! Renders SCG nodes and structures as type-like annotations in a configurable
//! language style. This is the primary "code-facing" projection, turning the
//! graph IR into something that resembles source code so that developers can
//! inspect and reason about program structure.
//!
//! ## Example output (Rust-like style)
//!
//! ```text
//! fn auth_handler(input: HttpRequest) -> HttpResponse
//!     @Send + 'static
//!     └─ memory: aligned(8), pinned
//! ```
//!
//! ## Example output (C-like style)
//!
//! ```text
//! HttpResponse auth_handler(HttpRequest input)
//!     __attribute__((send, static_lifetime))
//!     └─ memory: aligned(8), pinned
//! ```

use crate::{BdKind, EdgeKind, NodeId, NodeKind, RegionId, SCG, SCGNode};

// ── Projection style ─────────────────────────────────────────────────────────

/// The language style used for textual projection output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectionStyle {
    /// Emit Rust-like syntax (`fn`, `->`, `+` bounds, `@` annotations).
    RustLike,
    /// Emit C-like syntax (function signatures, `__attribute__`).
    CLike,
    /// Emit using a custom style defined by the user.
    Custom,
}

impl Default for ProjectionStyle {
    fn default() -> Self {
        Self::RustLike
    }
}

// ── Configuration ─────────────────────────────────────────────────────────────

/// Configuration for the textual projection engine.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TextualConfig {
    /// Whether to include memory layout annotations (alignment, pinning).
    pub show_memory_layout: bool,
    /// Whether to include capability BDs (Send, Sync, …).
    pub show_capabilities: bool,
    /// Whether to include relational BDs (borrows_from, …).
    pub show_relations: bool,
    /// Maximum nesting depth for nested structure display.
    pub max_nesting_depth: u32,
    /// The language style for the output.
    pub language_style: ProjectionStyle,
}

impl Default for TextualConfig {
    fn default() -> Self {
        Self {
            show_memory_layout: true,
            show_capabilities: true,
            show_relations: true,
            max_nesting_depth: 4,
            language_style: ProjectionStyle::RustLike,
        }
    }
}

// ── Textual projection engine ─────────────────────────────────────────────────

/// The textual projection engine.
///
/// Renders SCG structures as source-code-like text in the configured language
/// style, with BDs formatted as type-like annotations.
#[derive(Debug, Clone)]
pub struct TextualProjection {
    /// Configuration controlling what is rendered and how.
    pub config: TextualConfig,
}

impl Default for TextualProjection {
    fn default() -> Self {
        Self {
            config: TextualConfig::default(),
        }
    }
}

impl TextualProjection {
    /// Creates a new textual projection engine with the given configuration.
    pub fn new(config: TextualConfig) -> Self {
        Self { config }
    }

    /// Creates a new textual projection engine with default configuration.
    pub fn default_with_style(style: ProjectionStyle) -> Self {
        let mut config = TextualConfig::default();
        config.language_style = style;
        Self { config }
    }

    // ── Single-node projection ────────────────────────────────────────────────

    /// Projects a single SCG node as a textual representation.
    ///
    /// The output includes the node's signature, attached BDs, and (depending
    /// on configuration) memory layout, capabilities, and relations.
    pub fn project(&self, scg: &SCG, node_id: NodeId) -> String {
        let Some(node) = scg.get_node(node_id) else {
            return format!("// <unknown node {}>", node_id);
        };

        let mut out = String::new();

        // ── Signature line ────────────────────────────────────────────────
        out.push_str(&self.format_node_signature(node, scg));
        out.push('\n');

        // ── Behavioural descriptors ───────────────────────────────────────
        let bd_lines = self.format_bds(node);
        if !bd_lines.is_empty() {
            out.push_str(&bd_lines);
            out.push('\n');
        }

        // ── Edge summary ─────────────────────────────────────────────────
        let edges = self.format_edge_summary(node_id, scg);
        if !edges.is_empty() {
            out.push_str(&edges);
            out.push('\n');
        }

        out
    }

    // ── Full-graph projection ─────────────────────────────────────────────────

    /// Projects the entire SCG as a textual representation.
    ///
    /// Each node is rendered in order, grouped by region where possible.
    pub fn project_full(&self, scg: &SCG) -> String {
        let mut out = String::new();

        // Render regions first
        for region in &scg.regions {
            out.push_str(&format!(
                "// ── Region: {} (id={}) ──────────────────\n",
                region.name, region.id
            ));

            for &node_id in &region.nodes {
                out.push_str(&self.project(scg, node_id));
            }
            out.push('\n');
        }

        // Render any nodes not belonging to a region
        let region_nodes: std::collections::HashSet<NodeId> = scg
            .regions
            .iter()
            .flat_map(|r| r.nodes.iter().copied())
            .collect();

        let orphans: Vec<&SCGNode> = scg
            .nodes
            .iter()
            .filter(|n| !region_nodes.contains(&n.id))
            .collect();

        if !orphans.is_empty() {
            out.push_str("// ── Unassigned nodes ──────────────────\n");
            for node in orphans {
                out.push_str(&self.project(scg, node.id));
            }
        }

        out
    }

    // ── Region projection ──────────────────────────────────────────────────────

    /// Projects all nodes within a specific region.
    pub fn project_region(&self, scg: &SCG, region_id: RegionId) -> String {
        let Some(region) = scg.get_region(region_id) else {
            return format!("// <unknown region {}>", region_id);
        };

        let mut out = String::new();
        out.push_str(&format!(
            "// ── Region: {} ──────────────────\n",
            region.name
        ));

        for &node_id in &region.nodes {
            out.push_str(&self.project(scg, node_id));
        }

        out
    }

    // ── Internal formatting helpers ────────────────────────────────────────────

    /// Formats the primary signature line for a node.
    fn format_node_signature(&self, node: &SCGNode, _scg: &SCG) -> String {
        match self.config.language_style {
            ProjectionStyle::RustLike => self.format_rust_signature(node),
            ProjectionStyle::CLike => self.format_c_signature(node),
            ProjectionStyle::Custom => {
                // TODO: Allow user-defined formatting templates.
                self.format_rust_signature(node)
            }
        }
    }

    /// Formats a node in Rust-like style.
    fn format_rust_signature(&self, node: &SCGNode) -> String {
        match node.kind {
            NodeKind::Function => {
                format!("fn {}(/* params */) -> /* return type */", node.label)
            }
            NodeKind::Value => {
                format!("let {}: /* type */", node.label)
            }
            NodeKind::MessageSend => {
                format!("send {}(/* payload */)", node.label)
            }
            NodeKind::MessageReceive => {
                format!("recv {}(/* channel */)", node.label)
            }
            NodeKind::Merge => {
                format!("merge {}(/* branches */)", node.label)
            }
            NodeKind::Effect => {
                format!("effect {}(/* side effects */)", node.label)
            }
            NodeKind::Module => {
                format!("mod {} {{ ... }}", node.label)
            }
        }
    }

    /// Formats a node in C-like style.
    fn format_c_signature(&self, node: &SCGNode) -> String {
        match node.kind {
            NodeKind::Function => {
                format!("/* ret */ {}(/* params */)", node.label)
            }
            NodeKind::Value => {
                format!("/* type */ {};", node.label)
            }
            NodeKind::MessageSend => {
                format!("send_{}(/* payload */)", node.label)
            }
            NodeKind::MessageReceive => {
                format!("recv_{}(/* channel */)", node.label)
            }
            NodeKind::Merge => {
                format!("merge_{}(/* branches */)", node.label)
            }
            NodeKind::Effect => {
                format!("effect_{}(/* side effects */)", node.label)
            }
            NodeKind::Module => {
                format!("namespace {} {{ ... }}", node.label)
            }
        }
    }

    /// Formats the behavioural descriptors of a node as annotation lines.
    fn format_bds(&self, node: &SCGNode) -> String {
        if node.bds.is_empty() {
            return String::new();
        }

        let mut lines: Vec<String> = Vec::new();

        // Capabilities
        if self.config.show_capabilities {
            let caps: Vec<&str> = node
                .bds
                .iter()
                .filter(|bd| bd.kind == BdKind::Capability)
                .map(|bd| bd.name.as_str())
                .collect();
            if !caps.is_empty() {
                match self.config.language_style {
                    ProjectionStyle::RustLike => {
                        lines.push(format!("    @{}", caps.join(" + ")));
                    }
                    ProjectionStyle::CLike => {
                        lines.push(format!("    __attribute__(({}))", caps.join(", ")));
                    }
                    ProjectionStyle::Custom => {
                        // TODO: Custom template for capability display.
                        lines.push(format!("    @{}", caps.join(" + ")));
                    }
                }
            }
        }

        // Memory layout
        if self.config.show_memory_layout {
            let mem: Vec<String> = node
                .bds
                .iter()
                .filter(|bd| bd.kind == BdKind::MemoryLayout)
                .map(|bd| {
                    if let Some(ref param) = bd.parameter {
                        format!("{}({})", bd.name, param)
                    } else {
                        bd.name.clone()
                    }
                })
                .collect();
            if !mem.is_empty() {
                lines.push(format!("    └─ memory: {}", mem.join(", ")));
            }
        }

        // Relations
        if self.config.show_relations {
            let rels: Vec<String> = node
                .bds
                .iter()
                .filter(|bd| bd.kind == BdKind::Relation)
                .map(|bd| {
                    if let Some(ref param) = bd.parameter {
                        format!("{}({})", bd.name, param)
                    } else {
                        bd.name.clone()
                    }
                })
                .collect();
            if !rels.is_empty() {
                lines.push(format!("    └─ relations: {}", rels.join(", ")));
            }
        }

        // Safety
        let safety: Vec<String> = node
            .bds
            .iter()
            .filter(|bd| bd.kind == BdKind::Safety)
            .map(|bd| bd.name.clone())
            .collect();
        if !safety.is_empty() {
            lines.push(format!("    ⚠ safety: {}", safety.join(", ")));
        }

        // Custom
        let custom: Vec<String> = node
            .bds
            .iter()
            .filter(|bd| bd.kind == BdKind::Custom)
            .map(|bd| bd.name.clone())
            .collect();
        if !custom.is_empty() {
            lines.push(format!("    ✦ custom: {}", custom.join(", ")));
        }

        lines.join("\n")
    }

    /// Formats a summary of edges connected to a node.
    fn format_edge_summary(&self, node_id: NodeId, scg: &SCG) -> String {
        let outgoing = scg.outgoing_edges(node_id);
        let incoming = scg.incoming_edges(node_id);

        if outgoing.is_empty() && incoming.is_empty() {
            return String::new();
        }

        let mut parts: Vec<String> = Vec::new();

        if !outgoing.is_empty() {
            let data_out: Vec<String> = outgoing
                .iter()
                .filter(|e| e.kind == EdgeKind::DataFlow)
                .map(|e| format!("→ node_{}", e.target))
                .collect();
            let ctrl_out: Vec<String> = outgoing
                .iter()
                .filter(|e| e.kind == EdgeKind::ControlFlow)
                .map(|e| format!("↳ node_{}", e.target))
                .collect();
            let msg_out: Vec<String> = outgoing
                .iter()
                .filter(|e| e.kind == EdgeKind::Message)
                .map(|e| format!("✉ node_{}", e.target))
                .collect();
            let call_out: Vec<String> = outgoing
                .iter()
                .filter(|e| e.kind == EdgeKind::Call)
                .map(|e| format!("📞 node_{}", e.target))
                .collect();
            let borrow_out: Vec<String> = outgoing
                .iter()
                .filter(|e| e.kind == EdgeKind::Borrow)
                .map(|e| format!("& node_{}", e.target))
                .collect();

            let mut all = Vec::new();
            if !data_out.is_empty() {
                all.push(format!("data: [{}]", data_out.join(", ")));
            }
            if !ctrl_out.is_empty() {
                all.push(format!("ctrl: [{}]", ctrl_out.join(", ")));
            }
            if !msg_out.is_empty() {
                all.push(format!("msg: [{}]", msg_out.join(", ")));
            }
            if !call_out.is_empty() {
                all.push(format!("call: [{}]", call_out.join(", ")));
            }
            if !borrow_out.is_empty() {
                all.push(format!("borrow: [{}]", borrow_out.join(", ")));
            }

            if !all.is_empty() {
                parts.push(format!("    outgoing: {}", all.join(", ")));
            }
        }

        if !incoming.is_empty() {
            parts.push(format!("    incoming: {} edge(s)", incoming.len()));
        }

        parts.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BehaviouralDescriptor, BdKind, SCGEdge, SCGNode, SCGRegion};

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
                        BehaviouralDescriptor {
                            id: 101,
                            name: "aligned".into(),
                            kind: BdKind::MemoryLayout,
                            parameter: Some("8".into()),
                        },
                    ],
                    regions: vec![10],
                },
                SCGNode {
                    id: 2,
                    label: "session_token".into(),
                    kind: NodeKind::Value,
                    bds: vec![],
                    regions: vec![10],
                },
            ],
            edges: vec![SCGEdge {
                id: 200,
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

    #[test]
    fn project_single_node_rust_style() {
        let scg = sample_scg();
        let proj = TextualProjection::default_with_style(ProjectionStyle::RustLike);
        let out = proj.project(&scg, 1);
        assert!(out.contains("fn auth_handler"));
        assert!(out.contains("@Send"));
        assert!(out.contains("aligned(8)"));
    }

    #[test]
    fn project_single_node_c_style() {
        let scg = sample_scg();
        let proj = TextualProjection::default_with_style(ProjectionStyle::CLike);
        let out = proj.project(&scg, 1);
        assert!(out.contains("auth_handler"));
        assert!(out.contains("__attribute__"));
    }

    #[test]
    fn project_full_graph() {
        let scg = sample_scg();
        let proj = TextualProjection::default();
        let out = proj.project_full(&scg);
        assert!(out.contains("Region: authentication"));
        assert!(out.contains("fn auth_handler"));
        assert!(out.contains("let session_token"));
    }

    #[test]
    fn project_region() {
        let scg = sample_scg();
        let proj = TextualProjection::default();
        let out = proj.project_region(&scg, 10);
        assert!(out.contains("authentication"));
    }

    #[test]
    fn project_unknown_node() {
        let scg = sample_scg();
        let proj = TextualProjection::default();
        let out = proj.project(&scg, 999);
        assert!(out.contains("unknown node"));
    }
}
