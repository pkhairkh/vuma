//! # Textual Projection
//!
//! Renders SCG nodes and structures as type-like annotations in a configurable
//! language style. This is the primary "code-facing" projection, turning the
//! graph IR into something that resembles source code so that developers can
//! inspect and reason about program structure.
//!
//! ## Top-level API
//!
//! - [`project_textual`] — Render an SCG as human-readable VUMA code.
//! - [`project_textual_detailed`] — Render with BD annotations and region info.
//! - [`project_textual_diff`] — Render changes as a unified diff.
//!
//! ## Example output (Rust-like style)
//!
//! ```text
//! fn auth_handler(input: HttpRequest) -> HttpResponse
//!     @Send + 'static
//!     └─ memory: aligned(8), pinned
//! ```
//!
//! ## Example output (detailed projection)
//!
//! ```text
//! ═══ Region: authentication (id=10) ═══
//!   fn auth_handler(input: HttpRequest) -> HttpResponse
//!       // BD: @Send [capability]
//!       // BD: aligned(8) [memory_layout]
//!       └─ memory: aligned(8)
//!       outgoing: data: [→ node_2], ctrl: [↳ node_3]
//!
//!   let session_token: /* type */
//!       // no BDs attached
//! ═══ End Region: authentication ═══
//! ```
//!
//! ## Example output (unified diff)
//!
//! ```text
//! --- VUMA SCG (before)
//! +++ VUMA SCG (after)
//! @@ nodes @@
//! +fn verify_2fa(/* params */) -> /* return type */
//! +    @RequiresAuth
//! @@ edges @@
//! +Call: 1 → 2
//! @@ behavioural descriptors @@
//! +node 1: gained RequiresAuth [capability]
//! ```

use crate::{BdKind, EdgeKind, NodeId, NodeKind, RegionId, SCG, SCGDiff, SCGNode};

// ── Projection style ─────────────────────────────────────────────────────────

/// A template engine supporting `{{variable}}` interpolation syntax.
///
/// Users can define custom formatting templates that control how SCG nodes
/// and their behavioural descriptors are rendered in the textual projection.
/// Template variables are enclosed in double curly braces and are replaced
/// with the corresponding value at rendering time.
///
/// # Supported Variables
///
/// | Variable          | Description                                    |
/// |-------------------|------------------------------------------------|
/// | `{{label}}`       | The node's human-readable label                |
/// | `{{kind}}`        | The node kind (Function, Value, Effect, etc.)  |
/// | `{{capabilities}}`| Comma-separated list of capability BD names     |
/// | `{{memory}}`      | Memory layout descriptors                      |
/// | `{{relations}}`   | Relational BD descriptors                      |
/// | `{{safety}}`      | Safety BD descriptors                          |
/// | `{{custom}}`      | Custom BD descriptors                          |
/// | `{{all_bds}}`     | All BD names, regardless of kind               |
///
/// # Example
///
/// ```
/// use vuma_projection::textual::TemplateEngine;
///
/// let engine = TemplateEngine::new("fn {{label}}() -> {{kind}}");
/// let result = engine.render(&[("label", "my_func"), ("kind", "i32")]);
/// assert_eq!(result, "fn my_func() -> i32");
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TemplateEngine {
    /// The template string with `{{variable}}` placeholders.
    template: String,
}

impl TemplateEngine {
    /// Create a new template engine with the given template string.
    pub fn new(template: impl Into<String>) -> Self {
        Self { template: template.into() }
    }

    /// Render the template by substituting the provided key-value pairs.
    ///
    /// Each `{{key}}` in the template is replaced with the corresponding
    /// value from `vars`. Unknown variables are replaced with an empty
    /// string.
    pub fn render(&self, vars: &[(&str, &str)]) -> String {
        let mut result = self.template.clone();
        for (key, value) in vars {
            let placeholder = format!("{{{{{}}}}}", key);
            result = result.replace(&placeholder, value);
        }
        // Replace any remaining unsubstituted variables with empty string
        let mut start = 0;
        while let Some(pos) = result[start..].find("{{") {
            let abs_pos = start + pos;
            if let Some(end) = result[abs_pos..].find("}}") {
                // Remove the unsubstituted variable
                result.replace_range(abs_pos..abs_pos + end + 2, "");
                start = abs_pos;
            } else {
                break;
            }
        }
        result
    }

    /// Create a signature template for a node.
    ///
    /// Default: `"fn {{label}}() -> {{kind}}"`
    pub fn default_signature_template() -> Self {
        Self::new("fn {{label}}() -> {{kind}}")
    }

    /// Create a capability display template.
    ///
    /// Default: `"@{{capabilities}}"`
    pub fn default_capability_template() -> Self {
        Self::new("@{{capabilities}}")
    }
}

impl Default for TemplateEngine {
    fn default() -> Self {
        Self::new("fn {{label}}() -> {{kind}}")
    }
}

// ── Projection style ─────────────────────────────────────────────────────────

/// The language style used for textual projection output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ProjectionStyle {
    /// Emit Rust-like syntax (`fn`, `->`, `+` bounds, `@` annotations).
    #[default]
    RustLike,
    /// Emit C-like syntax (function signatures, `__attribute__`).
    CLike,
    /// Emit using a custom style defined by the user.
    Custom,
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
    /// Number of spaces per indentation level.
    pub indent_width: usize,
    /// User-defined template for node signatures (Custom style only).
    /// Supports {{variable}} syntax. When set, this overrides the
    /// default rendering for `ProjectionStyle::Custom`.
    pub signature_template: Option<TemplateEngine>,
    /// User-defined template for capability display (Custom style only).
    /// Supports {{variable}} syntax. When set, this overrides the
    /// default rendering for capabilities in `ProjectionStyle::Custom`.
    pub capability_template: Option<TemplateEngine>,
}

impl Default for TextualConfig {
    fn default() -> Self {
        Self {
            show_memory_layout: true,
            show_capabilities: true,
            show_relations: true,
            max_nesting_depth: 4,
            language_style: ProjectionStyle::RustLike,
            indent_width: 4,
            signature_template: None,
            capability_template: None,
        }
    }
}

// ── Textual projection engine ─────────────────────────────────────────────────

/// The textual projection engine.
///
/// Renders SCG structures as source-code-like text in the configured language
/// style, with BDs formatted as type-like annotations.
#[derive(Debug, Clone, Default)]
pub struct TextualProjection {
    /// Configuration controlling what is rendered and how.
    pub config: TextualConfig,
}

impl TextualProjection {
    /// Creates a new textual projection engine with the given configuration.
    pub fn new(config: TextualConfig) -> Self {
        Self { config }
    }

    /// Creates a new textual projection engine with default configuration.
    pub fn default_with_style(style: ProjectionStyle) -> Self {
        Self {
            config: TextualConfig {
                language_style: style,
                ..Default::default()
            },
        }
    }

    /// Returns an indentation string of the given level.
    fn indent(&self, level: usize) -> String {
        " ".repeat(level * self.config.indent_width)
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
    /// Nodes within each region are grouped by [`NodeKind`] for readability.
    pub fn project_full(&self, scg: &SCG) -> String {
        let mut out = String::new();

        // Render regions first
        for region in &scg.regions {
            out.push_str(&format!(
                "// ── Region: {} (id={}) ──────────────────\n",
                region.name, region.id
            ));

            // Group nodes by kind within each region
            let grouped = self.group_nodes_by_kind(scg, &region.nodes);
            for (kind_label, nodes) in grouped {
                if !nodes.is_empty() {
                    out.push_str(&format!("{}// ─ {} ─\n", self.indent(1), kind_label));
                    for &node_id in &nodes {
                        out.push_str(&self.project(scg, node_id));
                    }
                }
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

    /// Groups node IDs by their kind, returning an ordered list of (kind_label, node_ids).
    fn group_nodes_by_kind(
        &self,
        scg: &SCG,
        node_ids: &[NodeId],
    ) -> Vec<(&'static str, Vec<NodeId>)> {
        // Define the ordering of node kinds for display
        let kind_order: Vec<(&'static str, Vec<NodeKind>)> = vec![
            ("Modules", vec![NodeKind::Module]),
            ("Functions", vec![NodeKind::Function]),
            ("Values", vec![NodeKind::Value]),
            (
                "Messaging",
                vec![NodeKind::MessageSend, NodeKind::MessageReceive],
            ),
            ("Control Flow", vec![NodeKind::Merge]),
            ("Effects", vec![NodeKind::Effect]),
            ("Memory", vec![NodeKind::Allocation, NodeKind::Deallocation, NodeKind::Access]),
            ("Computation", vec![NodeKind::Computation]),
        ];

        let mut result = Vec::new();
        for (label, kinds) in &kind_order {
            let matched: Vec<NodeId> = node_ids
                .iter()
                .filter(|&&nid| {
                    scg.get_node(nid)
                        .map(|n| kinds.contains(&n.kind))
                        .unwrap_or(false)
                })
                .copied()
                .collect();
            result.push((*label, matched));
        }

        result
    }

    /// Formats the primary signature line for a node.
    fn format_node_signature(&self, node: &SCGNode, _scg: &SCG) -> String {
        match self.config.language_style {
            ProjectionStyle::RustLike => self.format_rust_signature(node),
            ProjectionStyle::CLike => self.format_c_signature(node),
            ProjectionStyle::Custom => {
                // Use user-defined formatting template if available,
                // otherwise fall back to Rust-like style.
                if let Some(ref template) = self.config.signature_template {
                    let kind_str = format!("{:?}", node.kind);
                    template.render(&[
                        ("label", &node.label),
                        ("kind", &kind_str),
                    ])
                } else {
                    self.format_rust_signature(node)
                }
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
            NodeKind::Allocation => {
                format!("alloc {}(/* size, align */)", node.label)
            }
            NodeKind::Deallocation => {
                format!("dealloc {}(/* alloc_ref */)", node.label)
            }
            NodeKind::Access => {
                format!("access {}(/* target */)", node.label)
            }
            NodeKind::Computation => {
                format!("compute {}(/* operation */)", node.label)
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
            NodeKind::Allocation => {
                format!("alloc_{}(/* size, align */)", node.label)
            }
            NodeKind::Deallocation => {
                format!("dealloc_{}(/* alloc_ref */)", node.label)
            }
            NodeKind::Access => {
                format!("access_{}(/* target */)", node.label)
            }
            NodeKind::Computation => {
                format!("compute_{}(/* operation */)", node.label)
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
                        // Use user-defined capability template if available,
                        // otherwise fall back to Rust-like style.
                        if let Some(ref template) = self.config.capability_template {
                            let caps_str = caps.join(", ");
                            lines.push(template.render(&[
                                ("capabilities", &caps_str),
                            ]));
                        } else {
                            lines.push(format!("    @{}", caps.join(" + ")));
                        }
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
            lines.push(format!("    \u{26a0} safety: {}", safety.join(", ")));
        }

        // Custom
        let custom: Vec<String> = node
            .bds
            .iter()
            .filter(|bd| bd.kind == BdKind::Custom)
            .map(|bd| bd.name.clone())
            .collect();
        if !custom.is_empty() {
            lines.push(format!("    \u{2726} custom: {}", custom.join(", ")));
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
                .map(|e| format!("\u{2192} node_{}", e.target))
                .collect();
            let ctrl_out: Vec<String> = outgoing
                .iter()
                .filter(|e| e.kind == EdgeKind::ControlFlow)
                .map(|e| format!("\u{21b3} node_{}", e.target))
                .collect();
            let msg_out: Vec<String> = outgoing
                .iter()
                .filter(|e| e.kind == EdgeKind::Message)
                .map(|e| format!("\u{2709} node_{}", e.target))
                .collect();
            let call_out: Vec<String> = outgoing
                .iter()
                .filter(|e| e.kind == EdgeKind::Call)
                .map(|e| format!("call node_{}", e.target))
                .collect();
            let borrow_out: Vec<String> = outgoing
                .iter()
                .filter(|e| e.kind == EdgeKind::Borrow)
                .map(|e| format!("& node_{}", e.target))
                .collect();
            let deriv_out: Vec<String> = outgoing
                .iter()
                .filter(|e| e.kind == EdgeKind::Derivation)
                .map(|e| format!("derive node_{}", e.target))
                .collect();
            let annot_out: Vec<String> = outgoing
                .iter()
                .filter(|e| e.kind == EdgeKind::Annotation)
                .map(|e| format!("annot node_{}", e.target))
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
            if !deriv_out.is_empty() {
                all.push(format!("derive: [{}]", deriv_out.join(", ")));
            }
            if !annot_out.is_empty() {
                all.push(format!("annot: [{}]", annot_out.join(", ")));
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

    // ── Detailed projection helpers ────────────────────────────────────────────

    /// Formats a single node with detailed BD comments.
    fn format_node_detailed(&self, node: &SCGNode, scg: &SCG, indent_level: usize) -> String {
        let ind = self.indent(indent_level);
        let mut out = String::new();

        // Signature line
        out.push_str(&format!("{}{}\n", ind, self.format_node_signature(node, scg)));

        // Each BD as an annotated comment
        if node.bds.is_empty() {
            out.push_str(&format!("{}    // no BDs attached\n", ind));
        } else {
            for bd in &node.bds {
                let kind_tag = bd_kind_tag(&bd.kind);
                let param_str = bd
                    .parameter
                    .as_ref()
                    .map(|p| format!("({})", p))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "{}    // BD: {}{} [{}]\n",
                    ind, bd.name, param_str, kind_tag
                ));
            }
            // Also render the standard annotation lines
            let bd_lines = self.format_bds(node);
            if !bd_lines.is_empty() {
                for line in bd_lines.lines() {
                    out.push_str(&format!("{}{}\n", ind, line));
                }
            }
        }

        // Edge summary
        let edges = self.format_edge_summary(node.id, scg);
        if !edges.is_empty() {
            for line in edges.lines() {
                out.push_str(&format!("{}{}\n", ind, line));
            }
        }

        out
    }

    /// Formats a region header with section separators.
    fn format_region_header(&self, region_name: &str, region_id: RegionId) -> String {
        let title = format!(" Region: {} (id={}) ", region_name, region_id);
        format!("\u{2550}{}{}\n", title, "\u{2550}".repeat(60usize.saturating_sub(title.len())))
    }

    /// Formats a region footer.
    fn format_region_footer(&self, region_name: &str) -> String {
        let title = format!(" End Region: {} ", region_name);
        format!("\u{2550}{}{}\n", title, "\u{2550}".repeat(60usize.saturating_sub(title.len())))
    }
}

// ── Free functions (top-level API) ────────────────────────────────────────────

/// Renders an SCG as human-readable VUMA code.
///
/// This is the primary entry point for textual projection. It produces clean,
/// readable output with proper indentation, line breaks, and grouping of
/// related nodes. Nodes are grouped by region and then by [`NodeKind`]
/// within each region.
///
/// # Example
///
/// ```text
/// // ── Region: authentication (id=10) ──────────────────
///     // ─ Functions ─
///     fn auth_handler(/* params */) -> /* return type */
///         @Send
///         └─ memory: aligned(8)
///         outgoing: data: [→ node_2]
///
///     // ─ Values ─
///     let session_token: /* type */
///
/// // ── Unassigned nodes ──────────────────
/// mod config { ... }
/// ```
pub fn project_textual(scg: &SCG) -> String {
    let proj = TextualProjection::default();
    proj.project_full(scg)
}

/// Renders an SCG with detailed BD annotations and region information.
///
/// This is a richer output than [`project_textual`]. Every behavioural
/// descriptor is shown as an inline comment with its kind tag (e.g.
/// `[capability]`, `[memory_layout]`), and region boundaries are marked
/// with prominent section headers (`═══ ... ═══`).
///
/// # Example
///
/// ```text
/// ═══ Region: authentication (id=10) ═══
///   fn auth_handler(/* params */) -> /* return type */
///       // BD: Send [capability]
///       // BD: aligned(8) [memory_layout]
///       @Send
///       └─ memory: aligned(8)
///       outgoing: data: [→ node_2]
///
///   let session_token: /* type */
///       // no BDs attached
/// ═══ End Region: authentication ═══
/// ```
pub fn project_textual_detailed(scg: &SCG) -> String {
    let proj = TextualProjection::default();
    let mut out = String::new();

    // ── Header ───────────────────────────────────────────────────────────
    out.push_str(&format!(
        "VUMA SCG — {} node(s), {} edge(s), {} region(s)\n\n",
        scg.nodes.len(),
        scg.edges.len(),
        scg.regions.len()
    ));

    // ── Regions ──────────────────────────────────────────────────────────
    for region in &scg.regions {
        out.push_str(&proj.format_region_header(&region.name, region.id));

        // Group by kind
        let grouped = proj.group_nodes_by_kind(scg, &region.nodes);
        for (kind_label, node_ids) in grouped {
            if node_ids.is_empty() {
                continue;
            }
            out.push_str(&format!("{}// ─ {} ─\n", proj.indent(1), kind_label));
            for &nid in &node_ids {
                if let Some(node) = scg.get_node(nid) {
                    out.push_str(&proj.format_node_detailed(node, scg, 1));
                }
            }
        }

        out.push_str(&proj.format_region_footer(&region.name));
        out.push('\n');
    }

    // ── Orphan nodes ────────────────────────────────────────────────────
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
        out.push_str("\u{2550} Unassigned Nodes ");
        out.push_str(&"\u{2550}".repeat(44));
        out.push('\n');
        for node in orphans {
            out.push_str(&proj.format_node_detailed(node, scg, 1));
        }
        out.push_str(&proj.format_region_footer("Unassigned"));
    }

    out
}

/// Renders an SCG diff as a unified-diff-style string.
///
/// The output uses the standard unified diff conventions (`+` for additions,
/// `-` for removals) and groups changes into sections: nodes, edges, and
/// behavioural descriptors. Node labels from the SCG are used to make the
/// output human-readable.
///
/// # Example
///
/// ```text
/// --- VUMA SCG (before)
/// +++ VUMA SCG (after)
/// @@ nodes @@
/// +fn verify_2fa(/* params */) -> /* return type */
/// +    @RequiresAuth
/// -let deprecated_cache: /* type */
/// @@ edges @@
/// +Call: auth_handler(1) → verify_2fa(2)
/// -DataFlow: old_src(5) → old_dst(6)
/// @@ behavioural descriptors @@
/// +node auth_handler (1): gained RequiresAuth [capability]
/// -node auth_handler (1): lost Unpin [capability]
/// ```
pub fn project_textual_diff(scg: &SCG, diff: &SCGDiff) -> String {
    let proj = TextualProjection::default();
    let mut out = String::new();

    out.push_str("--- VUMA SCG (before)\n");
    out.push_str("+++ VUMA SCG (after)\n");

    if diff.is_empty() {
        out.push_str("// No changes detected.\n");
        return out;
    }

    // ── Nodes section ────────────────────────────────────────────────────
    if !diff.added_nodes.is_empty() || !diff.removed_nodes.is_empty() {
        out.push_str("@@ nodes @@\n");
        for node in &diff.added_nodes {
            let sig = proj.format_node_signature(node, scg);
            out.push_str(&format!("+{}\n", sig));
            // BDs for added node
            let bd_lines = proj.format_bds(node);
            for line in bd_lines.lines() {
                out.push_str(&format!("+{}\n", line));
            }
        }
        for node in &diff.removed_nodes {
            let sig = proj.format_node_signature(node, scg);
            out.push_str(&format!("-{}\n", sig));
            let bd_lines = proj.format_bds(node);
            for line in bd_lines.lines() {
                out.push_str(&format!("-{}\n", line));
            }
        }
    }

    // ── Edges section ────────────────────────────────────────────────────
    if !diff.added_edges.is_empty() || !diff.removed_edges.is_empty() {
        out.push_str("@@ edges @@\n");
        for edge in &diff.added_edges {
            let src_label = node_label_or_id(scg, edge.source);
            let tgt_label = node_label_or_id(scg, edge.target);
            out.push_str(&format!(
                "+{}: {}({}) \u{2192} {}({})\n",
                edge_kind_label(&edge.kind),
                src_label,
                edge.source,
                tgt_label,
                edge.target
            ));
        }
        for edge in &diff.removed_edges {
            let src_label = node_label_or_id(scg, edge.source);
            let tgt_label = node_label_or_id(scg, edge.target);
            out.push_str(&format!(
                "-{}: {}({}) \u{2192} {}({})\n",
                edge_kind_label(&edge.kind),
                src_label,
                edge.source,
                tgt_label,
                edge.target
            ));
        }
    }

    // ── BD changes section ───────────────────────────────────────────────
    if !diff.modified_nodes.is_empty() || !diff.modified_bds.is_empty() {
        out.push_str("@@ behavioural descriptors @@\n");
        for change in &diff.modified_nodes {
            let label = node_label_or_id(scg, change.id);
            for bd in &change.bd_changes {
                let prefix = if bd.added { "+" } else { "-" };
                out.push_str(&format!(
                    "{}node {} ({}): {} {} [{}]\n",
                    prefix,
                    label,
                    change.id,
                    if bd.added { "gained" } else { "lost" },
                    bd.name,
                    bd_kind_tag(&bd.kind)
                ));
            }
        }
        for bd in &diff.modified_bds {
            let prefix = if bd.added { "+" } else { "-" };
            let label = node_label_or_id(scg, bd.node_id);
            out.push_str(&format!(
                "{}node {} ({}): {} {} [{}]\n",
                prefix,
                label,
                bd.node_id,
                if bd.added { "gained" } else { "lost" },
                bd.name,
                bd_kind_tag(&bd.kind)
            ));
        }
    }

    out
}

// ── Utility functions ─────────────────────────────────────────────────────────

/// Returns a short tag for a [`BdKind`], used in detailed output.
fn bd_kind_tag(kind: &BdKind) -> &'static str {
    match kind {
        BdKind::Capability => "capability",
        BdKind::MemoryLayout => "memory_layout",
        BdKind::Safety => "safety",
        BdKind::Relation => "relation",
        BdKind::Custom => "custom",
    }
}

/// Returns a human-readable label for an [`EdgeKind`].
fn edge_kind_label(kind: &EdgeKind) -> &'static str {
    match kind {
        EdgeKind::DataFlow => "DataFlow",
        EdgeKind::ControlFlow => "ControlFlow",
        EdgeKind::Message => "Message",
        EdgeKind::Borrow => "Borrow",
        EdgeKind::Call => "Call",
        EdgeKind::Derivation => "Derivation",
        EdgeKind::Annotation => "Annotation",
    }
}

/// Looks up a node's label by ID, falling back to the numeric ID.
fn node_label_or_id(scg: &SCG, id: NodeId) -> String {
    scg.get_node(id)
        .map(|n| n.label.clone())
        .unwrap_or_else(|| format!("node_{}", id))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BehaviouralDescriptor, BdKind, SCGEdge, SCGNode, SCGRegion};

    // ── Helper builders ──────────────────────────────────────────────────────

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

    fn multi_region_scg() -> SCG {
        SCG {
            nodes: vec![
                SCGNode {
                    id: 1,
                    label: "main".into(),
                    kind: NodeKind::Function,
                    bds: vec![],
                    regions: vec![1],
                },
                SCGNode {
                    id: 2,
                    label: "config".into(),
                    kind: NodeKind::Module,
                    bds: vec![],
                    regions: vec![1],
                },
                SCGNode {
                    id: 3,
                    label: "request".into(),
                    kind: NodeKind::MessageSend,
                    bds: vec![BehaviouralDescriptor {
                        id: 300,
                        name: "borrows_from".into(),
                        kind: BdKind::Relation,
                        parameter: Some("config".into()),
                    }],
                    regions: vec![2],
                },
                SCGNode {
                    id: 4,
                    label: "response".into(),
                    kind: NodeKind::MessageReceive,
                    bds: vec![],
                    regions: vec![2],
                },
                SCGNode {
                    id: 5,
                    label: "join_point".into(),
                    kind: NodeKind::Merge,
                    bds: vec![],
                    regions: vec![2],
                },
                SCGNode {
                    id: 6,
                    label: "log".into(),
                    kind: NodeKind::Effect,
                    bds: vec![BehaviouralDescriptor {
                        id: 301,
                        name: "unsafe_io".into(),
                        kind: BdKind::Safety,
                        parameter: None,
                    }],
                    regions: vec![2],
                },
                // Orphan node — not in any region
                SCGNode {
                    id: 99,
                    label: "orphan_value".into(),
                    kind: NodeKind::Value,
                    bds: vec![BehaviouralDescriptor {
                        id: 302,
                        name: "my_custom".into(),
                        kind: BdKind::Custom,
                        parameter: None,
                    }],
                    regions: vec![],
                },
            ],
            edges: vec![
                SCGEdge {
                    id: 400,
                    source: 1,
                    target: 3,
                    kind: EdgeKind::ControlFlow,
                },
                SCGEdge {
                    id: 401,
                    source: 3,
                    target: 4,
                    kind: EdgeKind::Message,
                },
                SCGEdge {
                    id: 402,
                    source: 4,
                    target: 5,
                    kind: EdgeKind::ControlFlow,
                },
                SCGEdge {
                    id: 403,
                    source: 6,
                    target: 5,
                    kind: EdgeKind::ControlFlow,
                },
                SCGEdge {
                    id: 404,
                    source: 2,
                    target: 3,
                    kind: EdgeKind::Borrow,
                },
                SCGEdge {
                    id: 405,
                    source: 1,
                    target: 3,
                    kind: EdgeKind::Call,
                },
            ],
            regions: vec![
                SCGRegion {
                    id: 1,
                    name: "entrypoint".into(),
                    nodes: vec![1, 2],
                },
                SCGRegion {
                    id: 2,
                    name: "messaging".into(),
                    nodes: vec![3, 4, 5, 6],
                },
            ],
        }
    }

    fn empty_scg() -> SCG {
        SCG {
            nodes: vec![],
            edges: vec![],
            regions: vec![],
        }
    }

    fn scg_with_all_bd_kinds() -> SCG {
        SCG {
            nodes: vec![SCGNode {
                id: 1,
                label: "rich_node".into(),
                kind: NodeKind::Function,
                bds: vec![
                    BehaviouralDescriptor {
                        id: 10,
                        name: "Send".into(),
                        kind: BdKind::Capability,
                        parameter: None,
                    },
                    BehaviouralDescriptor {
                        id: 11,
                        name: "Sync".into(),
                        kind: BdKind::Capability,
                        parameter: None,
                    },
                    BehaviouralDescriptor {
                        id: 12,
                        name: "aligned".into(),
                        kind: BdKind::MemoryLayout,
                        parameter: Some("16".into()),
                    },
                    BehaviouralDescriptor {
                        id: 13,
                        name: "pinned".into(),
                        kind: BdKind::MemoryLayout,
                        parameter: None,
                    },
                    BehaviouralDescriptor {
                        id: 14,
                        name: "borrows_from".into(),
                        kind: BdKind::Relation,
                        parameter: Some("owner".into()),
                    },
                    BehaviouralDescriptor {
                        id: 15,
                        name: "unsafe_deref".into(),
                        kind: BdKind::Safety,
                        parameter: None,
                    },
                    BehaviouralDescriptor {
                        id: 16,
                        name: "my_annotation".into(),
                        kind: BdKind::Custom,
                        parameter: Some("42".into()),
                    },
                ],
                regions: vec![1],
            }],
            edges: vec![],
            regions: vec![SCGRegion {
                id: 1,
                name: "test_region".into(),
                nodes: vec![1],
            }],
        }
    }

    fn sample_diff() -> SCGDiff {
        SCGDiff {
            added_nodes: vec![SCGNode {
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
            }],
            removed_nodes: vec![SCGNode {
                id: 5,
                label: "deprecated_cache".into(),
                kind: NodeKind::Value,
                bds: vec![],
                regions: vec![],
            }],
            modified_nodes: vec![crate::diff::NodeChange {
                id: 1,
                label: "auth_handler".into(),
                bd_changes: vec![
                    crate::diff::BdChange {
                        node_id: 1,
                        name: "RequiresAuth".into(),
                        kind: BdKind::Capability,
                        added: true,
                    },
                    crate::diff::BdChange {
                        node_id: 1,
                        name: "Unpin".into(),
                        kind: BdKind::Capability,
                        added: false,
                    },
                ],
            }],
            added_edges: vec![SCGEdge {
                id: 10,
                source: 1,
                target: 2,
                kind: EdgeKind::Call,
            }],
            removed_edges: vec![SCGEdge {
                id: 20,
                source: 5,
                target: 6,
                kind: EdgeKind::DataFlow,
            }],
            modified_bds: vec![],
        }
    }

    // ── Test 1: Single node, Rust style ──────────────────────────────────

    #[test]
    fn project_single_node_rust_style() {
        let scg = sample_scg();
        let proj = TextualProjection::default_with_style(ProjectionStyle::RustLike);
        let out = proj.project(&scg, 1);
        assert!(out.contains("fn auth_handler"));
        assert!(out.contains("@Send"));
        assert!(out.contains("aligned(8)"));
    }

    // ── Test 2: Single node, C style ─────────────────────────────────────

    #[test]
    fn project_single_node_c_style() {
        let scg = sample_scg();
        let proj = TextualProjection::default_with_style(ProjectionStyle::CLike);
        let out = proj.project(&scg, 1);
        assert!(out.contains("auth_handler"));
        assert!(out.contains("__attribute__"));
    }

    // ── Test 3: Full graph projection ────────────────────────────────────

    #[test]
    fn project_full_graph() {
        let scg = sample_scg();
        let proj = TextualProjection::default();
        let out = proj.project_full(&scg);
        assert!(out.contains("Region: authentication"));
        assert!(out.contains("fn auth_handler"));
        assert!(out.contains("let session_token"));
    }

    // ── Test 4: Region projection ────────────────────────────────────────

    #[test]
    fn project_region() {
        let scg = sample_scg();
        let proj = TextualProjection::default();
        let out = proj.project_region(&scg, 10);
        assert!(out.contains("authentication"));
    }

    // ── Test 5: Unknown node ─────────────────────────────────────────────

    #[test]
    fn project_unknown_node() {
        let scg = sample_scg();
        let proj = TextualProjection::default();
        let out = proj.project(&scg, 999);
        assert!(out.contains("unknown node"));
    }

    // ── Test 6: project_textual free function ────────────────────────────

    #[test]
    fn project_textual_renders_full_scg() {
        let scg = sample_scg();
        let out = project_textual(&scg);
        assert!(out.contains("fn auth_handler"));
        assert!(out.contains("let session_token"));
        assert!(out.contains("Region: authentication"));
        // Nodes should be grouped by kind
        assert!(out.contains("Functions") || out.contains("Values"));
    }

    // ── Test 7: project_textual_detailed ─────────────────────────────────

    #[test]
    fn project_textual_detailed_shows_bd_annotations() {
        let scg = sample_scg();
        let out = project_textual_detailed(&scg);
        // Should contain BD comment annotations
        assert!(out.contains("// BD: Send [capability]"));
        assert!(out.contains("// BD: aligned(8) [memory_layout]"));
        // Should contain region headers
        assert!(out.contains("\u{2550}") | out.contains("Region: authentication"));
        // Should contain node count in header
        assert!(out.contains("2 node(s)"));
        assert!(out.contains("1 edge(s)"));
    }

    // ── Test 8: project_textual_detailed with all BD kinds ───────────────

    #[test]
    fn project_textual_detailed_all_bd_kinds() {
        let scg = scg_with_all_bd_kinds();
        let out = project_textual_detailed(&scg);
        assert!(out.contains("[capability]"));
        assert!(out.contains("[memory_layout]"));
        assert!(out.contains("[relation]"));
        assert!(out.contains("[safety]"));
        assert!(out.contains("[custom]"));
        // Also check the standard BD rendering
        assert!(out.contains("@Send + Sync"));
        assert!(out.contains("aligned(16), pinned"));
        assert!(out.contains("borrows_from(owner)"));
    }

    // ── Test 9: project_textual_diff ─────────────────────────────────────

    #[test]
    fn project_textual_diff_shows_unified_diff() {
        let scg = sample_scg();
        let diff = sample_diff();
        let out = project_textual_diff(&scg, &diff);
        // Should contain unified diff headers
        assert!(out.contains("--- VUMA SCG (before)"));
        assert!(out.contains("+++ VUMA SCG (after)"));
        // Should contain node section
        assert!(out.contains("@@ nodes @@"));
        assert!(out.contains("+fn verify_2fa"));
        assert!(out.contains("-let deprecated_cache"));
        // Should contain edge section
        assert!(out.contains("@@ edges @@"));
        assert!(out.contains("+Call:"));
        assert!(out.contains("-DataFlow:"));
        // Should contain BD section
        assert!(out.contains("@@ behavioural descriptors @@"));
        assert!(out.contains("gained RequiresAuth [capability]"));
        assert!(out.contains("lost Unpin [capability]"));
    }

    // ── Test 10: Empty diff ──────────────────────────────────────────────

    #[test]
    fn project_textual_diff_empty_diff() {
        let scg = sample_scg();
        let diff = SCGDiff::empty();
        let out = project_textual_diff(&scg, &diff);
        assert!(out.contains("--- VUMA SCG (before)"));
        assert!(out.contains("No changes detected"));
    }

    // ── Test 11: Empty SCG ───────────────────────────────────────────────

    #[test]
    fn project_textual_empty_scg() {
        let scg = empty_scg();
        let out = project_textual(&scg);
        let detailed = project_textual_detailed(&scg);
        // Should not panic, should produce empty or minimal output
        assert!(out.is_empty() || out.trim().is_empty());
        assert!(detailed.contains("0 node(s)"));
    }

    // ── Test 12: Multi-region SCG with grouping ──────────────────────────

    #[test]
    fn project_textual_multi_region_grouping() {
        let scg = multi_region_scg();
        let out = project_textual(&scg);
        // Both regions should appear
        assert!(out.contains("Region: entrypoint"));
        assert!(out.contains("Region: messaging"));
        // Nodes should be grouped by kind
        assert!(out.contains("Modules"));
        assert!(out.contains("Functions"));
        assert!(out.contains("Messaging"));
        assert!(out.contains("Control Flow"));
        assert!(out.contains("Effects"));
        // Orphan node should appear in unassigned
        assert!(out.contains("Unassigned nodes"));
        assert!(out.contains("orphan_value"));
    }

    // ── Test 13: Detailed projection with orphan and safety BD ───────────

    #[test]
    fn project_textual_detailed_orphan_and_safety() {
        let scg = multi_region_scg();
        let out = project_textual_detailed(&scg);
        // Orphan section
        assert!(out.contains("Unassigned"));
        assert!(out.contains("orphan_value"));
        // Safety BD on the effect node
        assert!(out.contains("// BD: unsafe_io [safety]"));
        // Custom BD on the orphan
        assert!(out.contains("// BD: my_custom [custom]"));
    }

    // ── Test 14: Diff with edge labels from SCG ──────────────────────────

    #[test]
    fn project_textual_diff_uses_node_labels() {
        let scg = sample_scg();
        let diff = sample_diff();
        let out = project_textual_diff(&scg, &diff);
        // The diff should use node labels, not just IDs
        assert!(out.contains("auth_handler"));
        assert!(out.contains("verify_2fa"));
    }

    // ── Test 15: Indentation is configurable ─────────────────────────────

    #[test]
    fn indentation_configurable() {
        let scg = sample_scg();
        let mut config = TextualConfig::default();
        config.indent_width = 2;
        let proj = TextualProjection::new(config);
        let out = proj.project_full(&scg);
        // With indent_width=2, lines should use 2-space groups
        assert!(out.contains("  // ─"));
    }
}
