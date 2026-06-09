//! # Visual Projection
//!
//! Renders SCG structures as visual diagrams in multiple formats:
//!
//! - **ASCII/Unicode** — Terminal-friendly box-drawing diagrams
//! - **Graphviz DOT** — Standard graph description language with styling
//! - **Mermaid** — Markdown-embeddable diagram format
//! - **SVG** — Standalone Scalable Vector Graphics rendering
//!
//! ## Color Coding
//!
//! | Visual Category   | Color   | Node Kinds                              |
//! |--------------------|---------|-----------------------------------------|
//! | Allocations        | Green   | `Allocation`, `Value`                   |
//! | Deallocations      | Red     | `Deallocation`                          |
//! | Accesses           | Blue    | `Access`, `MessageSend`, `MessageReceive`|
//! | Computations       | Orange  | `Function`, `Effect`, `Computation`     |
//! | ControlFlow        | Purple  | `Merge`, `Module`                       |
//!
//! ## Layout
//!
//! - Hierarchical top-down layout (rankdir=TB in DOT)
//! - Region boundaries rendered as subgraph clusters
//! - Edge labels identify the relationship type

use crate::{EdgeKind, NodeId, NodeKind, RegionId, SCG, SCGEdge, SCGNode};
use std::collections::{HashMap, HashSet};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Box top-left corner.
const BOX_TL: &str = "┌";
/// Box top-right corner.
const BOX_TR: &str = "┐";
/// Box bottom-left corner.
const BOX_BL: &str = "└";
/// Box bottom-right corner.
const BOX_BR: &str = "┘";
/// Box horizontal line.
const BOX_H: &str = "─";
/// Box vertical line.
const BOX_V: &str = "│";
/// Arrow head right.
#[allow(dead_code)]
const ARROW_R: &str = "▶";
/// Arrow head down.
#[allow(dead_code)]
const ARROW_D: &str = "▼";
/// Vertical connector.
const VLINE: &str = "│";

// ── Visual Category & Color Mapping ──────────────────────────────────────────

/// Visual category for color-coded rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualCategory {
    /// Memory allocation nodes → green.
    Allocation,
    /// Memory deallocation nodes → red.
    Deallocation,
    /// Memory access nodes → blue.
    Access,
    /// Computation nodes → orange.
    Computation,
    /// Control-flow nodes → purple.
    ControlFlow,
}

impl VisualCategory {
    /// Maps a [`NodeKind`] to its visual category.
    pub fn from_node_kind(kind: NodeKind) -> Self {
        match kind {
            NodeKind::Allocation | NodeKind::Value => VisualCategory::Allocation,
            NodeKind::Deallocation => VisualCategory::Deallocation,
            NodeKind::Access | NodeKind::MessageSend | NodeKind::MessageReceive => {
                VisualCategory::Access
            }
            NodeKind::Function | NodeKind::Effect | NodeKind::Computation => {
                VisualCategory::Computation
            }
            NodeKind::Merge | NodeKind::Module => VisualCategory::ControlFlow,
        }
    }

    /// Returns the CSS color name for this category.
    pub fn color_name(&self) -> &'static str {
        match self {
            VisualCategory::Allocation => "green",
            VisualCategory::Deallocation => "red",
            VisualCategory::Access => "blue",
            VisualCategory::Computation => "orange",
            VisualCategory::ControlFlow => "purple",
        }
    }

    /// Returns the hex color for this category (for SVG/DOT).
    pub fn hex_color(&self) -> &'static str {
        match self {
            VisualCategory::Allocation => "#4CAF50",
            VisualCategory::Deallocation => "#F44336",
            VisualCategory::Access => "#2196F3",
            VisualCategory::Computation => "#FF9800",
            VisualCategory::ControlFlow => "#9C27B0",
        }
    }

    /// Returns a lighter hex color for backgrounds/fills.
    pub fn fill_hex(&self) -> &'static str {
        match self {
            VisualCategory::Allocation => "#C8E6C9",
            VisualCategory::Deallocation => "#FFCDD2",
            VisualCategory::Access => "#BBDEFB",
            VisualCategory::Computation => "#FFE0B2",
            VisualCategory::ControlFlow => "#E1BEE7",
        }
    }

    /// Returns the DOT fillcolor attribute value.
    pub fn dot_fillcolor(&self) -> &'static str {
        self.fill_hex()
    }

    /// Returns the DOT fontcolor attribute value.
    pub fn dot_fontcolor(&self) -> &'static str {
        self.hex_color()
    }
}

// ── Edge label helpers ───────────────────────────────────────────────────────

/// Returns a display label for an [`EdgeKind`].
fn edge_label(kind: EdgeKind) -> &'static str {
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

/// Returns the DOT style for an edge kind.
fn dot_edge_style(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::DataFlow => "solid",
        EdgeKind::ControlFlow => "dashed",
        EdgeKind::Message => "bold",
        EdgeKind::Borrow => "dotted",
        EdgeKind::Call => "solid",
        EdgeKind::Derivation => "dashed",
        EdgeKind::Annotation => "dotted",
    }
}

/// Returns the DOT color for an edge kind.
fn dot_edge_color(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::DataFlow => "#4CAF50",
        EdgeKind::ControlFlow => "#9C27B0",
        EdgeKind::Message => "#2196F3",
        EdgeKind::Borrow => "#FF9800",
        EdgeKind::Call => "#607D8B",
        EdgeKind::Derivation => "#00BCD4",
        EdgeKind::Annotation => "#795548",
    }
}

/// Returns the Mermaid arrow style for an edge kind.
fn mermaid_edge_style(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::DataFlow => "-->",
        EdgeKind::ControlFlow => "-.->",
        EdgeKind::Message => "==>",
        EdgeKind::Borrow => "-.->",
        EdgeKind::Call => "-->",
        EdgeKind::Derivation => "-.->",
        EdgeKind::Annotation => "-.->",
    }
}

// ── Visual projection engine ──────────────────────────────────────────────────

/// The visual projection engine.
///
/// Produces ASCII/Unicode art diagrams, Graphviz DOT, Mermaid, and SVG
/// renderings of SCG structures.
#[derive(Debug, Clone)]
pub struct VisualProjection {
    /// Minimum width for node boxes (ASCII mode).
    pub min_box_width: usize,
    /// Whether to use colour in the output (via `colored` crate, ASCII mode).
    pub use_color: bool,
}

impl Default for VisualProjection {
    fn default() -> Self {
        Self {
            min_box_width: 20,
            use_color: true,
        }
    }
}

impl VisualProjection {
    /// Creates a new visual projection engine.
    pub fn new(min_box_width: usize, use_color: bool) -> Self {
        Self {
            min_box_width,
            use_color,
        }
    }

    // ── Graphviz DOT projection ───────────────────────────────────────────────

    /// Projects the SCG as a Graphviz DOT string.
    ///
    /// Features:
    /// - Hierarchical top-down layout (`rankdir=TB`)
    /// - Nodes colored by [`VisualCategory`]
    /// - Region boundaries as `subgraph cluster_*` groups
    /// - Edge labels for each [`EdgeKind`]
    /// - Edge styles (solid, dashed, dotted, bold) by kind
    pub fn project_dot(&self, scg: &SCG) -> String {
        if scg.nodes.is_empty() {
            return "digraph SCG {\n    label=\"<empty SCG>\";\n}\n".to_string();
        }

        let mut out = String::new();
        out.push_str("digraph SCG {\n");
        out.push_str("    rankdir=TB;\n");
        out.push_str("    node [shape=box, style=filled, fontname=\"Helvetica\"];\n");
        out.push_str("    edge [fontname=\"Helvetica\", fontsize=10];\n\n");

        // Group nodes by region for subgraph clusters
        let mut nodes_in_regions: HashSet<NodeId> = HashSet::new();

        // Render region clusters
        for region in &scg.regions {
            out.push_str(&format!(
                "    subgraph cluster_region_{} {{\n",
                region.id
            ));
            out.push_str(&format!(
                "        label=\"{}\";\n",
                dot_escape(&region.name)
            ));
            out.push_str("        style=dashed;\n");
            out.push_str("        color=\"#9E9E9E\";\n\n");

            for &nid in &region.nodes {
                if let Some(node) = scg.get_node(nid) {
                    out.push_str(&self.dot_node_decl(node, 2));
                    nodes_in_regions.insert(nid);
                }
            }
            out.push_str("    }\n\n");
        }

        // Render nodes not in any region
        for node in &scg.nodes {
            if !nodes_in_regions.contains(&node.id) {
                out.push_str(&self.dot_node_decl(node, 1));
            }
        }

        out.push('\n');

        // Render edges
        for edge in &scg.edges {
            out.push_str(&format!(
                "    node_{} -> node_{} [label=\"{}\", style={}, color=\"{}\"];\n",
                edge.source,
                edge.target,
                edge_label(edge.kind),
                dot_edge_style(edge.kind),
                dot_edge_color(edge.kind),
            ));
        }

        out.push_str("}\n");
        out
    }

    /// Generates a DOT node declaration string.
    fn dot_node_decl(&self, node: &SCGNode, indent: usize) -> String {
        let cat = VisualCategory::from_node_kind(node.kind);
        let pad = "    ".repeat(indent);
        format!(
            "{}node_{} [label=\"{}\\n[{:?}]\", fillcolor=\"{}\", fontcolor=\"{}\"];\n",
            pad,
            node.id,
            dot_escape(&node.label),
            node.kind,
            cat.dot_fillcolor(),
            cat.dot_fontcolor(),
        )
    }

    // ── Mermaid projection ────────────────────────────────────────────────────

    /// Projects the SCG as a Mermaid diagram string.
    ///
    /// Features:
    /// - Top-down graph (`graph TD`)
    /// - Nodes styled by [`VisualCategory`]
    /// - Region boundaries as `subgraph` groups
    /// - Edge labels for each [`EdgeKind`]
    pub fn project_mermaid(&self, scg: &SCG) -> String {
        if scg.nodes.is_empty() {
            return "graph TD\n    empty[\"<empty SCG>\"]\n".to_string();
        }

        let mut out = String::new();
        out.push_str("graph TD\n");

        // Region subgraphs
        let mut nodes_in_regions: HashSet<NodeId> = HashSet::new();

        for region in &scg.regions {
            out.push_str(&format!(
                "    subgraph region_{}[\"{}\"]\n",
                region.id, region.name
            ));
            for &nid in &region.nodes {
                if let Some(node) = scg.get_node(nid) {
                    out.push_str(&self.mermaid_node_decl(node, 2));
                    nodes_in_regions.insert(nid);
                }
            }
            out.push_str("    end\n");
        }

        // Nodes not in any region
        for node in &scg.nodes {
            if !nodes_in_regions.contains(&node.id) {
                out.push_str(&self.mermaid_node_decl(node, 1));
            }
        }

        // Edges
        for edge in &scg.edges {
            let arrow = mermaid_edge_style(edge.kind);
            out.push_str(&format!(
                "    node_{} {}|{}| node_{}\n",
                edge.source,
                arrow,
                edge_label(edge.kind),
                edge.target,
            ));
        }

        // Style directives
        out.push('\n');
        for node in &scg.nodes {
            let cat = VisualCategory::from_node_kind(node.kind);
            out.push_str(&format!(
                "    style node_{} fill:{},color:{}\n",
                node.id,
                cat.fill_hex(),
                cat.hex_color(),
            ));
        }

        out
    }

    /// Generates a Mermaid node declaration string.
    fn mermaid_node_decl(&self, node: &SCGNode, indent: usize) -> String {
        let pad = "    ".repeat(indent);
        format!(
            "{}node_{}[\"{}<br/><small>{:?}</small>\"]\n",
            pad, node.id, node.label, node.kind,
        )
    }

    // ── SVG projection ────────────────────────────────────────────────────────

    /// Projects the SCG as an SVG string.
    ///
    /// Uses a hierarchical top-down layout computed from the graph topology.
    /// Regions are rendered as dashed rectangles enclosing their nodes.
    /// Nodes are colored by [`VisualCategory`] and edges include arrowheads
    /// and labels.
    pub fn project_svg(&self, scg: &SCG) -> String {
        if scg.nodes.is_empty() {
            return "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"200\" height=\"60\">\
                   <text x=\"100\" y=\"35\" text-anchor=\"middle\" fill=\"#999\">&lt;empty SCG&gt;</text>\
                   </svg>"
                .to_string();
        }

        let layout = self.compute_layout(scg);

        const NODE_W: f64 = 160.0;
        const NODE_H: f64 = 50.0;
        const LEVEL_GAP: f64 = 90.0;
        const NODE_GAP: f64 = 40.0;
        const MARGIN: f64 = 60.0;

        // Compute positions
        let level_counts = &layout.level_counts;
        let max_count = level_counts.values().copied().max().unwrap_or(1).max(1);
        let num_levels = layout.num_levels.max(1);

        let svg_w = MARGIN * 2.0 + max_count as f64 * (NODE_W + NODE_GAP) - NODE_GAP;
        let svg_h = MARGIN * 2.0 + num_levels as f64 * (NODE_H + LEVEL_GAP) - LEVEL_GAP;

        let mut svg = String::new();
        svg.push_str(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\">\n",
            svg_w as u32, svg_h as u32, svg_w as u32, svg_h as u32,
        ));
        svg.push_str("  <defs>\n");
        svg.push_str("    <marker id=\"arrowhead\" markerWidth=\"10\" markerHeight=\"7\" refX=\"10\" refY=\"3.5\" orient=\"auto\">\n");
        svg.push_str("      <polygon points=\"0 0, 10 3.5, 0 7\" fill=\"#555\" />\n");
        svg.push_str("    </marker>\n");
        svg.push_str("  </defs>\n\n");

        // Render region backgrounds first (behind everything)
        for region in &scg.regions {
            let region_bounds = self.region_bounds(
                &region.nodes,
                &layout.positions,
                NODE_W,
                NODE_H,
                MARGIN,
            );
            if let Some((rx, ry, rw, rh)) = region_bounds {
                svg.push_str(&format!(
                    "  <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
                     fill=\"none\" stroke=\"#9E9E9E\" stroke-width=\"1.5\" \
                     stroke-dasharray=\"6,3\" rx=\"8\" />\n",
                    rx - 10.0,
                    ry - 10.0,
                    rw + 20.0,
                    rh + 20.0,
                ));
                svg.push_str(&format!(
                    "  <text x=\"{}\" y=\"{}\" font-family=\"Helvetica\" \
                     font-size=\"11\" fill=\"#757575\">{}</text>\n",
                    rx - 5.0,
                    ry - 15.0,
                    svg_escape(&region.name),
                ));
            }
        }

        // Render edges
        for edge in &scg.edges {
            if let (Some((sx, sy)), Some((tx, ty))) = (
                layout.positions.get(&edge.source),
                layout.positions.get(&edge.target),
            ) {
                let x1 = MARGIN + sx * (NODE_W + NODE_GAP) + NODE_W / 2.0;
                let y1 = MARGIN + sy * (NODE_H + LEVEL_GAP) + NODE_H;
                let x2 = MARGIN + tx * (NODE_W + NODE_GAP) + NODE_W / 2.0;
                let y2 = MARGIN + ty * (NODE_H + LEVEL_GAP);

                let edge_color = dot_edge_color(edge.kind);
                let dash = match edge.kind {
                    EdgeKind::ControlFlow | EdgeKind::Derivation => " stroke-dasharray=\"6,3\"",
                    EdgeKind::Borrow | EdgeKind::Annotation => " stroke-dasharray=\"3,3\"",
                    _ => "",
                };

                svg.push_str(&format!(
                    "  <line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" \
                     stroke=\"{}\" stroke-width=\"1.5\"{} marker-end=\"url(#arrowhead)\" />\n",
                    x1, y1, x2, y2, edge_color, dash,
                ));

                // Edge label
                let mx = (x1 + x2) / 2.0;
                let my = (y1 + y2) / 2.0;
                svg.push_str(&format!(
                    "  <text x=\"{}\" y=\"{}\" font-family=\"Helvetica\" \
                     font-size=\"9\" fill=\"{}\" text-anchor=\"middle\">{}</text>\n",
                    mx,
                    my - 4.0,
                    edge_color,
                    edge_label(edge.kind),
                ));
            }
        }

        // Render nodes
        for node in &scg.nodes {
            if let Some((col, row)) = layout.positions.get(&node.id) {
                let x = MARGIN + col * (NODE_W + NODE_GAP);
                let y = MARGIN + row * (NODE_H + LEVEL_GAP);
                let cat = VisualCategory::from_node_kind(node.kind);

                // Node rectangle
                svg.push_str(&format!(
                    "  <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
                     fill=\"{}\" stroke=\"{}\" stroke-width=\"2\" rx=\"6\" />\n",
                    x,
                    y,
                    NODE_W,
                    NODE_H,
                    cat.fill_hex(),
                    cat.hex_color(),
                ));

                // Node label
                svg.push_str(&format!(
                    "  <text x=\"{}\" y=\"{}\" font-family=\"Helvetica\" \
                     font-size=\"12\" font-weight=\"bold\" fill=\"{}\" \
                     text-anchor=\"middle\">{}</text>\n",
                    x + NODE_W / 2.0,
                    y + 20.0,
                    cat.hex_color(),
                    svg_escape(&node.label),
                ));

                // Node kind
                svg.push_str(&format!(
                    "  <text x=\"{}\" y=\"{}\" font-family=\"Helvetica\" \
                     font-size=\"10\" fill=\"#666\" text-anchor=\"middle\">{:?}</text>\n",
                    x + NODE_W / 2.0,
                    y + 38.0,
                    node.kind,
                ));
            }
        }

        svg.push_str("</svg>\n");
        svg
    }

    /// Computes bounding rectangle for a set of nodes.
    fn region_bounds(
        &self,
        node_ids: &[NodeId],
        positions: &HashMap<NodeId, (f64, f64)>,
        nw: f64,
        nh: f64,
        margin: f64,
    ) -> Option<(f64, f64, f64, f64)> {
        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;

        for &nid in node_ids {
            if let Some((col, row)) = positions.get(&nid) {
                let x = margin + col * (nw + 40.0);
                let y = margin + row * (nh + 90.0);
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x + nw);
                max_y = max_y.max(y + nh);
            }
        }

        if min_x == f64::MAX {
            None
        } else {
            Some((min_x, min_y, max_x - min_x, max_y - min_y))
        }
    }

    /// Computes hierarchical layout positions for all nodes.
    fn compute_layout(&self, scg: &SCG) -> LayoutResult {
        // Build adjacency info
        let incoming: HashMap<NodeId, Vec<NodeId>> = {
            let mut map: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
            for node in &scg.nodes {
                map.entry(node.id).or_default();
            }
            for edge in &scg.edges {
                map.entry(edge.target).or_default().push(edge.source);
            }
            map
        };

        // Topological level assignment
        let mut levels: HashMap<NodeId, usize> = HashMap::new();
        let mut visited: HashSet<NodeId> = HashSet::new();

        fn assign_level(
            node_id: NodeId,
            incoming: &HashMap<NodeId, Vec<NodeId>>,
            levels: &mut HashMap<NodeId, usize>,
            visited: &mut HashSet<NodeId>,
        ) {
            if visited.contains(&node_id) {
                return;
            }
            visited.insert(node_id);

            let preds = incoming.get(&node_id).cloned().unwrap_or_default();
            let pred_level = preds
                .iter()
                .map(|p| {
                    assign_level(*p, incoming, levels, visited);
                    *levels.get(p).unwrap_or(&0)
                })
                .max()
                .unwrap_or(0);

            levels.insert(node_id, pred_level + 1);
        }

        for node in &scg.nodes {
            assign_level(node.id, &incoming, &mut levels, &mut visited);
        }

        // Normalize: shift so minimum level is 0
        let min_level = levels.values().copied().min().unwrap_or(0);
        for v in levels.values_mut() {
            *v -= min_level;
        }

        let num_levels = levels.values().copied().max().unwrap_or(0) + 1;

        // Assign columns within each level
        let mut level_counts: HashMap<usize, usize> = HashMap::new();
        let mut positions: HashMap<NodeId, (f64, f64)> = HashMap::new();

        // Group nodes by level
        let mut by_level: HashMap<usize, Vec<NodeId>> = HashMap::new();
        for node in &scg.nodes {
            let lvl = levels.get(&node.id).copied().unwrap_or(0);
            by_level.entry(lvl).or_default().push(node.id);
        }

        for (lvl, nodes) in &by_level {
            for (i, nid) in nodes.iter().enumerate() {
                positions.insert(*nid, (i as f64, *lvl as f64));
            }
            level_counts.insert(*lvl, nodes.len());
        }

        LayoutResult {
            positions,
            level_counts,
            num_levels,
        }
    }

    // ── ASCII dataflow rendering ──────────────────────────────────────────────

    /// Renders the entire SCG as a dataflow diagram using Unicode box-drawing.
    pub fn render_dataflow(&self, scg: &SCG) -> String {
        if scg.nodes.is_empty() {
            return "// <empty SCG>\n".to_string();
        }

        let mut lines: Vec<String> = Vec::new();

        for node in &scg.nodes {
            lines.push(self.render_node_box(node));

            let outgoing: Vec<&SCGEdge> =
                scg.edges.iter().filter(|e| e.source == node.id).collect();

            if !outgoing.is_empty() {
                for (i, edge) in outgoing.iter().enumerate() {
                    let target_label = scg
                        .get_node(edge.target)
                        .map(|n| n.label.as_str())
                        .unwrap_or("???");

                    let edge_symbol = match edge.kind {
                        EdgeKind::DataFlow => "──▶",
                        EdgeKind::ControlFlow => "──↳",
                        EdgeKind::Message => "──✉──▶",
                        EdgeKind::Call => "──📞──▶",
                        EdgeKind::Borrow => "──&──▶",
                        EdgeKind::Derivation => "──δ──▶",
                        EdgeKind::Annotation => "──@──▶",
                    };

                    let prefix = if i == 0 { "    " } else { "    " };
                    lines.push(format!(
                        "{}{} {}{}",
                        prefix, VLINE, edge_symbol, target_label
                    ));

                    if i + 1 < outgoing.len() {
                        lines.push(format!("    {}", VLINE));
                    }
                }
            }

            lines.push(String::new());
        }

        lines.join("\n")
    }

    /// Renders a single node as a Unicode box.
    fn render_node_box(&self, node: &SCGNode) -> String {
        let kind_label = format!("{:?}", node.kind);
        let content_width = node
            .label
            .len()
            .max(kind_label.len())
            .max(self.min_box_width);
        let inner_width = content_width;

        let top = format!(
            "{}{}{}",
            BOX_TL,
            BOX_H.repeat(inner_width + 2),
            BOX_TR
        );
        let label_line = format!(
            "{} {}{} {}",
            BOX_V,
            node.label,
            " ".repeat(inner_width - node.label.len()),
            BOX_V
        );
        let kind_line = format!(
            "{} {}{} {}",
            BOX_V,
            kind_label,
            " ".repeat(inner_width - kind_label.len()),
            BOX_V
        );
        let bottom = format!(
            "{}{}{}",
            BOX_BL,
            BOX_H.repeat(inner_width + 2),
            BOX_BR
        );

        if self.use_color {
            use colored::Colorize;
            let cat = VisualCategory::from_node_kind(node.kind);
            let colored_label = match cat {
                VisualCategory::Allocation => label_line.green().bold().to_string(),
                VisualCategory::Deallocation => label_line.red().bold().to_string(),
                VisualCategory::Access => label_line.blue().bold().to_string(),
                VisualCategory::Computation => {
                    use colored::CustomColor;
                    label_line.custom_color(CustomColor::new(255, 152, 0)).bold().to_string()
                }
                VisualCategory::ControlFlow => label_line.purple().bold().to_string(),
            };
            format!(
                "{}\n{}\n{}\n{}",
                top, colored_label, kind_line.dimmed(), bottom
            )
        } else {
            format!("{}\n{}\n{}\n{}", top, label_line, kind_line, bottom)
        }
    }

    // ── Message rendering ─────────────────────────────────────────────────────

    /// Renders a single message-passing edge as a visual diagram.
    pub fn render_msg(&self, edge: &SCGEdge, scg: &SCG) -> String {
        let source_label = scg
            .get_node(edge.source)
            .map(|n| n.label.as_str())
            .unwrap_or("???");
        let target_label = scg
            .get_node(edge.target)
            .map(|n| n.label.as_str())
            .unwrap_or("???");

        if edge.kind != EdgeKind::Message {
            return format!(
                "// edge {} is not a message edge (found {:?})",
                edge.id, edge.kind
            );
        }

        let sw = source_label.len().max(self.min_box_width);
        let tw = target_label.len().max(self.min_box_width);

        let src_top = format!("{}{}{}", BOX_TL, BOX_H.repeat(sw + 2), BOX_TR);
        let src_mid = format!(
            "{} {}{} {}",
            BOX_V,
            source_label,
            " ".repeat(sw - source_label.len()),
            BOX_V
        );
        let src_bot = format!("{}{}{}", BOX_BL, BOX_H.repeat(sw + 2), BOX_BR);

        let tgt_top = format!("{}{}{}", BOX_TL, BOX_H.repeat(tw + 2), BOX_TR);
        let tgt_mid = format!(
            "{} {}{} {}",
            BOX_V,
            target_label,
            " ".repeat(tw - target_label.len()),
            BOX_V
        );
        let tgt_bot = format!("{}{}{}", BOX_BL, BOX_H.repeat(tw + 2), BOX_BR);

        let connector = " ✉──▶ ";

        format!(
            "{}{}{}\n{}{}{}\n{}{}{}",
            src_top,
            " ".repeat(connector.len()),
            tgt_top,
            src_mid,
            connector,
            tgt_mid,
            src_bot,
            " ".repeat(connector.len()),
            tgt_bot,
        )
    }

    // ── Call-graph rendering ──────────────────────────────────────────────────

    /// Renders the call graph of the SCG as a tree-like diagram.
    pub fn render_call_graph(&self, scg: &SCG) -> String {
        let call_edges: Vec<&SCGEdge> = scg.edges.iter().filter(|e| e.kind == EdgeKind::Call).collect();

        if call_edges.is_empty() {
            return "// <no call edges>\n".to_string();
        }

        let callees: HashSet<NodeId> = call_edges.iter().map(|e| e.target).collect();
        let callers: HashSet<NodeId> = call_edges.iter().map(|e| e.source).collect();

        let roots: Vec<NodeId> = callers.difference(&callees).copied().collect();

        let mut lines: Vec<String> = Vec::new();
        let mut visited: HashSet<NodeId> = HashSet::new();

        let start_nodes = if roots.is_empty() {
            callers.into_iter().collect()
        } else {
            roots
        };

        for root_id in start_nodes {
            self.render_call_tree(scg, &call_edges, root_id, 0, &mut lines, &mut visited);
        }

        lines.join("\n")
    }

    /// Recursively renders a call tree starting from `node_id`.
    fn render_call_tree(
        &self,
        scg: &SCG,
        call_edges: &[&SCGEdge],
        node_id: NodeId,
        depth: usize,
        lines: &mut Vec<String>,
        visited: &mut HashSet<NodeId>,
    ) {
        if visited.contains(&node_id) {
            let label = scg
                .get_node(node_id)
                .map(|n| n.label.as_str())
                .unwrap_or("???");
            lines.push(format!("{}{} (recursive)", "  ".repeat(depth), label));
            return;
        }
        visited.insert(node_id);

        let label = scg
            .get_node(node_id)
            .map(|n| n.label.as_str())
            .unwrap_or("???");

        if depth == 0 {
            lines.push(format!("📞 {}", label));
        } else {
            lines.push(format!("{}└──📞 {}", "  ".repeat(depth), label));
        }

        let callees: Vec<NodeId> = call_edges
            .iter()
            .filter(|e| e.source == node_id)
            .map(|e| e.target)
            .collect();

        for callee_id in callees {
            self.render_call_tree(scg, call_edges, callee_id, depth + 1, lines, visited);
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Builds a map from NodeId to the first region it belongs to.
    #[allow(dead_code)]
    fn build_region_map(&self, scg: &SCG) -> HashMap<NodeId, RegionId> {
        let mut map = HashMap::new();
        for region in &scg.regions {
            for &nid in &region.nodes {
                map.entry(nid).or_insert(region.id);
            }
        }
        map
    }
}

// ── Layout result ─────────────────────────────────────────────────────────────

/// Result of the hierarchical layout computation.
struct LayoutResult {
    /// Node positions as (column, row) pairs.
    positions: HashMap<NodeId, (f64, f64)>,
    /// Number of nodes at each level.
    level_counts: HashMap<usize, usize>,
    /// Total number of levels.
    num_levels: usize,
}

// ── Free functions ────────────────────────────────────────────────────────────

/// Projects an SCG as a Graphviz DOT string (convenience wrapper).
pub fn project_dot(scg: &SCG) -> String {
    VisualProjection::default().project_dot(scg)
}

/// Projects an SCG as a Mermaid diagram string (convenience wrapper).
pub fn project_mermaid(scg: &SCG) -> String {
    VisualProjection::default().project_mermaid(scg)
}

/// Projects an SCG as an SVG string (convenience wrapper).
pub fn project_svg(scg: &SCG) -> String {
    VisualProjection::default().project_svg(scg)
}

// ── Escape helpers ────────────────────────────────────────────────────────────

/// Escapes a string for safe inclusion in a DOT label.
fn dot_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Escapes a string for safe inclusion in SVG text content.
fn svg_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ══════════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EdgeKind, NodeKind, SCGEdge, SCGNode, SCGRegion};

    // ── Test fixtures ─────────────────────────────────────────────────────────

    fn sample_scg() -> SCG {
        SCG {
            nodes: vec![
                SCGNode {
                    id: 1,
                    label: "main".into(),
                    kind: NodeKind::Function,
                    bds: vec![],
                    regions: vec![100],
                },
                SCGNode {
                    id: 2,
                    label: "auth".into(),
                    kind: NodeKind::Function,
                    bds: vec![],
                    regions: vec![100],
                },
                SCGNode {
                    id: 3,
                    label: "token".into(),
                    kind: NodeKind::Value,
                    bds: vec![],
                    regions: vec![100],
                },
            ],
            edges: vec![
                SCGEdge {
                    id: 10,
                    source: 1,
                    target: 2,
                    kind: EdgeKind::Call,
                },
                SCGEdge {
                    id: 11,
                    source: 2,
                    target: 3,
                    kind: EdgeKind::DataFlow,
                },
                SCGEdge {
                    id: 12,
                    source: 1,
                    target: 3,
                    kind: EdgeKind::Message,
                },
            ],
            regions: vec![SCGRegion {
                id: 100,
                name: "auth_region".into(),
                nodes: vec![1, 2, 3],
            }],
        }
    }

    fn rich_scg() -> SCG {
        SCG {
            nodes: vec![
                SCGNode {
                    id: 1,
                    label: "alloc_buf".into(),
                    kind: NodeKind::Allocation,
                    bds: vec![],
                    regions: vec![],
                },
                SCGNode {
                    id: 2,
                    label: "free_buf".into(),
                    kind: NodeKind::Deallocation,
                    bds: vec![],
                    regions: vec![],
                },
                SCGNode {
                    id: 3,
                    label: "read_mem".into(),
                    kind: NodeKind::Access,
                    bds: vec![],
                    regions: vec![],
                },
                SCGNode {
                    id: 4,
                    label: "compute".into(),
                    kind: NodeKind::Computation,
                    bds: vec![],
                    regions: vec![],
                },
                SCGNode {
                    id: 5,
                    label: "branch".into(),
                    kind: NodeKind::Merge,
                    bds: vec![],
                    regions: vec![],
                },
                SCGNode {
                    id: 6,
                    label: "derived".into(),
                    kind: NodeKind::Value,
                    bds: vec![],
                    regions: vec![],
                },
            ],
            edges: vec![
                SCGEdge {
                    id: 20,
                    source: 1,
                    target: 3,
                    kind: EdgeKind::DataFlow,
                },
                SCGEdge {
                    id: 21,
                    source: 3,
                    target: 4,
                    kind: EdgeKind::Derivation,
                },
                SCGEdge {
                    id: 22,
                    source: 4,
                    target: 5,
                    kind: EdgeKind::ControlFlow,
                },
                SCGEdge {
                    id: 23,
                    source: 5,
                    target: 2,
                    kind: EdgeKind::ControlFlow,
                },
                SCGEdge {
                    id: 24,
                    source: 4,
                    target: 6,
                    kind: EdgeKind::Annotation,
                },
            ],
            regions: vec![SCGRegion {
                id: 200,
                name: "memory_ops".into(),
                nodes: vec![1, 2, 3],
            }],
        }
    }

    // ── Test 1: DOT basic structure ───────────────────────────────────────────

    #[test]
    fn project_dot_basic_structure() {
        let scg = sample_scg();
        let dot = project_dot(&scg);

        assert!(dot.starts_with("digraph SCG {"));
        assert!(dot.contains("rankdir=TB"));
        assert!(dot.contains("node_1"));
        assert!(dot.contains("node_2"));
        assert!(dot.contains("node_3"));
        assert!(dot.contains("main"));
        assert!(dot.contains("auth"));
        assert!(dot.contains("token"));
        assert!(dot.ends_with("}\n"));
    }

    // ── Test 2: DOT region clusters ──────────────────────────────────────────

    #[test]
    fn project_dot_region_clusters() {
        let scg = sample_scg();
        let dot = project_dot(&scg);

        assert!(dot.contains("subgraph cluster_region_100"));
        assert!(dot.contains("auth_region"));
        assert!(dot.contains("style=dashed"));
    }

    // ── Test 3: DOT edge labels and styles ───────────────────────────────────

    #[test]
    fn project_dot_edge_labels() {
        let scg = sample_scg();
        let dot = project_dot(&scg);

        assert!(dot.contains("label=\"Call\""));
        assert!(dot.contains("label=\"DataFlow\""));
        assert!(dot.contains("label=\"Message\""));
    }

    // ── Test 4: Mermaid basic structure ──────────────────────────────────────

    #[test]
    fn project_mermaid_basic_structure() {
        let scg = sample_scg();
        let mmd = project_mermaid(&scg);

        assert!(mmd.starts_with("graph TD"));
        assert!(mmd.contains("node_1"));
        assert!(mmd.contains("node_2"));
        assert!(mmd.contains("node_3"));
        assert!(mmd.contains("main"));
        assert!(mmd.contains("Call"));
    }

    // ── Test 5: Mermaid region subgraphs ─────────────────────────────────────

    #[test]
    fn project_mermaid_region_subgraphs() {
        let scg = sample_scg();
        let mmd = project_mermaid(&scg);

        assert!(mmd.contains("subgraph region_100"));
        assert!(mmd.contains("auth_region"));
        assert!(mmd.contains("end"));
    }

    // ── Test 6: SVG basic structure ──────────────────────────────────────────

    #[test]
    fn project_svg_basic_structure() {
        let scg = sample_scg();
        let svg = project_svg(&scg);

        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("<rect"));
        assert!(svg.contains("main"));
        assert!(svg.contains("marker-end=\"url(#arrowhead)\""));
    }

    // ── Test 7: Color coding across all formats ──────────────────────────────

    #[test]
    fn color_coding_all_categories() {
        let scg = rich_scg();
        let dot = project_dot(&scg);
        let mmd = project_mermaid(&scg);
        let svg = project_svg(&scg);

        // Allocation → green
        assert!(dot.contains("#C8E6C9")); // green fill
        // Deallocation → red
        assert!(dot.contains("#FFCDD2")); // red fill
        // Access → blue
        assert!(dot.contains("#BBDEFB")); // blue fill
        // Computation → orange
        assert!(dot.contains("#FFE0B2")); // orange fill
        // ControlFlow → purple
        assert!(dot.contains("#E1BEE7")); // purple fill

        // Mermaid style directives
        assert!(mmd.contains("fill:#C8E6C9")); // green
        assert!(mmd.contains("fill:#FFCDD2")); // red

        // SVG node fills
        assert!(svg.contains("#C8E6C9")); // green
        assert!(svg.contains("#FFCDD2")); // red
    }

    // ── Test 8: Empty SCG handling ───────────────────────────────────────────

    #[test]
    fn empty_scg_all_formats() {
        let scg = SCG::empty();
        let proj = VisualProjection::default();

        let dot = proj.project_dot(&scg);
        assert!(dot.contains("empty"));

        let mmd = proj.project_mermaid(&scg);
        assert!(mmd.contains("empty"));

        let svg = proj.project_svg(&scg);
        assert!(svg.contains("empty"));
    }

    // ── Test 9: Derivation and Annotation edge labels ────────────────────────

    #[test]
    fn derivation_and_annotation_edges() {
        let scg = rich_scg();
        let dot = project_dot(&scg);
        let mmd = project_mermaid(&scg);
        let svg = project_svg(&scg);

        assert!(dot.contains("label=\"Derivation\""));
        assert!(dot.contains("label=\"Annotation\""));
        assert!(mmd.contains("|Derivation|"));
        assert!(mmd.contains("|Annotation|"));
        assert!(svg.contains("Derivation"));
        assert!(svg.contains("Annotation"));
    }

    // ── Test 10: Visual category mapping ─────────────────────────────────────

    #[test]
    fn visual_category_mapping() {
        assert_eq!(
            VisualCategory::from_node_kind(NodeKind::Allocation),
            VisualCategory::Allocation
        );
        assert_eq!(
            VisualCategory::from_node_kind(NodeKind::Value),
            VisualCategory::Allocation
        );
        assert_eq!(
            VisualCategory::from_node_kind(NodeKind::Deallocation),
            VisualCategory::Deallocation
        );
        assert_eq!(
            VisualCategory::from_node_kind(NodeKind::Access),
            VisualCategory::Access
        );
        assert_eq!(
            VisualCategory::from_node_kind(NodeKind::Function),
            VisualCategory::Computation
        );
        assert_eq!(
            VisualCategory::from_node_kind(NodeKind::Effect),
            VisualCategory::Computation
        );
        assert_eq!(
            VisualCategory::from_node_kind(NodeKind::Computation),
            VisualCategory::Computation
        );
        assert_eq!(
            VisualCategory::from_node_kind(NodeKind::Merge),
            VisualCategory::ControlFlow
        );
        assert_eq!(
            VisualCategory::from_node_kind(NodeKind::Module),
            VisualCategory::ControlFlow
        );
    }

    // ── Test 11: ASCII dataflow still works ──────────────────────────────────

    #[test]
    fn render_dataflow_non_empty() {
        let scg = sample_scg();
        let proj = VisualProjection::new(16, false);
        let out = proj.render_dataflow(&scg);
        assert!(out.contains("main"));
        assert!(out.contains("auth"));
        assert!(out.contains("token"));
    }

    // ── Test 12: Call graph rendering ────────────────────────────────────────

    #[test]
    fn render_call_graph() {
        let scg = sample_scg();
        let proj = VisualProjection::new(16, false);
        let out = proj.render_call_graph(&scg);
        assert!(out.contains("main"));
        assert!(out.contains("auth"));
    }

    // ── Test 13: SVG region boundaries ───────────────────────────────────────

    #[test]
    fn svg_region_boundaries() {
        let scg = rich_scg();
        let svg = project_svg(&scg);

        // Region boundary should appear as dashed rectangle
        assert!(svg.contains("stroke-dasharray=\"6,3\""));
        assert!(svg.contains("memory_ops"));
    }

    // ── Test 14: DOT hierarchical layout hint ───────────────────────────────

    #[test]
    fn dot_hierarchical_layout() {
        let scg = sample_scg();
        let dot = project_dot(&scg);

        // Top-down direction
        assert!(dot.contains("rankdir=TB"));
    }

    // ── Test 15: Message rendering ───────────────────────────────────────────

    #[test]
    fn render_msg_edge() {
        let scg = sample_scg();
        let proj = VisualProjection::new(16, false);
        let edge = scg
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Message)
            .unwrap();
        let out = proj.render_msg(edge, &scg);
        assert!(out.contains("✉──▶"));
    }
}
