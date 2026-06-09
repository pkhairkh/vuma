//! # Visual (CLI-based) Projection
//!
//! Renders SCG structures as ASCII / Unicode art diagrams suitable for terminal
//! output. Uses Unicode box-drawing characters for clean, professional-looking
//! diagrams in the CLI.
//!
//! ## Example dataflow diagram
//!
//! ```text
//! ┌─────────────────┐     ┌─────────────────┐
//! │  auth_handler   │────▶│  session_token  │
//! │  [Function]     │     │  [Value]        │
//! └─────────────────┘     └─────────────────┘
//!         │
//!         │ ✉ message
//!         ▼
//! ┌─────────────────┐
//! │  log_audit      │
//! │  [Effect]       │
//! └─────────────────┘
//! ```
//!
//! ## Unicode box-drawing characters used
//!
//! | Char | Usage                    |
//! |------|--------------------------|
//! | ┌┐└┘ | Box corners              |
//! | │─   | Box sides                |
//! | ├┤┬┴ | Box T-junctions          |
//! | ┼    | Box cross                |
//! | ▶▷   | Directed edge (right)    |
//! | ▼▽   | Directed edge (down)     |
//! | ✉    | Message edge             |
//! | 📞   | Call edge                |

use crate::{EdgeKind, NodeId, SCG, SCGEdge, SCGNode};

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

// ── Visual projection engine ──────────────────────────────────────────────────

/// The visual (CLI-based) projection engine.
///
/// Produces ASCII/Unicode art diagrams of dataflow, message passing, and call
/// graphs for terminal-based inspection of SCG structures.
#[derive(Debug, Clone)]
pub struct VisualProjection {
    /// Minimum width for node boxes.
    pub min_box_width: usize,
    /// Whether to use colour in the output (via `colored` crate).
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

    // ── Dataflow rendering ────────────────────────────────────────────────────

    /// Renders the entire SCG as a dataflow diagram using Unicode box-drawing.
    ///
    /// Nodes are drawn as labelled boxes. Data-flow edges are rendered as
    /// horizontal arrows (`──▶`), message edges as `──✉──▶`, and call edges
    /// as `──📞──▶`. When edges go "downward" (same horizontal column), vertical
    /// arrows are used.
    ///
    /// # Layout strategy
    ///
    /// The current implementation uses a simple topological ordering. A proper
    /// Sugiyama-style layered layout will be added in a future iteration.
    pub fn render_dataflow(&self, scg: &SCG) -> String {
        if scg.nodes.is_empty() {
            return "// <empty SCG>\n".to_string();
        }

        let mut lines: Vec<String> = Vec::new();

        // Render each node as a box, then its outgoing edges.
        for node in &scg.nodes {
            // ── Node box ──────────────────────────────────────────────────
            lines.push(self.render_node_box(node));

            // ── Outgoing edges ────────────────────────────────────────────
            let outgoing: Vec<&SCGEdge> = scg.edges.iter().filter(|e| e.source == node.id).collect();

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
                    };

                    let prefix = if i == 0 { "    " } else { "    " };
                    lines.push(format!(
                        "{}{} {}{}",
                        prefix,
                        VLINE,
                        edge_symbol,
                        target_label
                    ));

                    if i + 1 < outgoing.len() {
                        lines.push(format!("    {}", VLINE));
                    }
                }
            }

            lines.push(String::new()); // blank line between nodes
        }

        lines.join("\n")
    }

    /// Renders a single node as a Unicode box.
    fn render_node_box(&self, node: &SCGNode) -> String {
        let kind_label = format!("{:?}", node.kind);
        let content_width = node.label.len().max(kind_label.len()).max(self.min_box_width);
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

        // Optional colour
        if self.use_color {
            use colored::Colorize;
            format!(
                "{}\n{}\n{}\n{}",
                top,
                label_line.green().bold(),
                kind_line.dimmed(),
                bottom
            )
        } else {
            format!("{}\n{}\n{}\n{}", top, label_line, kind_line, bottom)
        }
    }

    // ── Message rendering ─────────────────────────────────────────────────────

    /// Renders a single message-passing edge as a visual diagram.
    ///
    /// Shows the source node, the message edge, and the target node in a
    /// compact horizontal layout.
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
        let src_mid = format!("{} {}{} {}", BOX_V, source_label, " ".repeat(sw - source_label.len()), BOX_V);
        let src_bot = format!("{}{}{}", BOX_BL, BOX_H.repeat(sw + 2), BOX_BR);

        let tgt_top = format!("{}{}{}", BOX_TL, BOX_H.repeat(tw + 2), BOX_TR);
        let tgt_mid = format!("{} {}{} {}", BOX_V, target_label, " ".repeat(tw - target_label.len()), BOX_V);
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

    // ── Call-graph rendering ───────────────────────────────────────────────────

    /// Renders the call graph of the SCG as a tree-like diagram.
    ///
    /// Only edges of kind [`EdgeKind::Call`] are considered. The output is an
    /// indented tree showing caller → callee relationships.
    pub fn render_call_graph(&self, scg: &SCG) -> String {
        let call_edges: Vec<&SCGEdge> = scg.edges.iter().filter(|e| e.kind == EdgeKind::Call).collect();

        if call_edges.is_empty() {
            return "// <no call edges>\n".to_string();
        }

        // Identify roots: nodes with call edges going out but no incoming call edges.
        let callees: std::collections::HashSet<NodeId> =
            call_edges.iter().map(|e| e.target).collect();
        let callers: std::collections::HashSet<NodeId> =
            call_edges.iter().map(|e| e.source).collect();

        let roots: Vec<NodeId> = callers.difference(&callees).copied().collect();

        let mut lines: Vec<String> = Vec::new();
        let mut visited: std::collections::HashSet<NodeId> = std::collections::HashSet::new();

        let start_nodes = if roots.is_empty() {
            // No clear roots; start from all callers.
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
        visited: &mut std::collections::HashSet<NodeId>,
    ) {
        if visited.contains(&node_id) {
            let label = scg
                .get_node(node_id)
                .map(|n| n.label.as_str())
                .unwrap_or("???");
            lines.push(format!(
                "{}{} (recursive)",
                "  ".repeat(depth),
                label
            ));
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

        // Find callees
        let callees: Vec<NodeId> = call_edges
            .iter()
            .filter(|e| e.source == node_id)
            .map(|e| e.target)
            .collect();

        for callee_id in callees {
            self.render_call_tree(scg, call_edges, callee_id, depth + 1, lines, visited);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EdgeKind, NodeKind, SCGEdge, SCGNode};

    fn sample_scg() -> SCG {
        SCG {
            nodes: vec![
                SCGNode {
                    id: 1,
                    label: "main".into(),
                    kind: NodeKind::Function,
                    bds: vec![],
                    regions: vec![],
                },
                SCGNode {
                    id: 2,
                    label: "auth".into(),
                    kind: NodeKind::Function,
                    bds: vec![],
                    regions: vec![],
                },
                SCGNode {
                    id: 3,
                    label: "token".into(),
                    kind: NodeKind::Value,
                    bds: vec![],
                    regions: vec![],
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
            regions: vec![],
        }
    }

    #[test]
    fn render_dataflow_non_empty() {
        let scg = sample_scg();
        let proj = VisualProjection::new(16, false);
        let out = proj.render_dataflow(&scg);
        assert!(out.contains("main"));
        assert!(out.contains("auth"));
        assert!(out.contains("token"));
    }

    #[test]
    fn render_msg_edge() {
        let scg = sample_scg();
        let proj = VisualProjection::new(16, false);
        let edge = scg.edges.iter().find(|e| e.kind == EdgeKind::Message).unwrap();
        let out = proj.render_msg(edge, &scg);
        assert!(out.contains("✉──▶"));
    }

    #[test]
    fn render_call_graph() {
        let scg = sample_scg();
        let proj = VisualProjection::new(16, false);
        let out = proj.render_call_graph(&scg);
        assert!(out.contains("main"));
        assert!(out.contains("auth"));
    }

    #[test]
    fn render_empty_scg() {
        let scg = SCG::empty();
        let proj = VisualProjection::default();
        let out = proj.render_dataflow(&scg);
        assert!(out.contains("empty"));
    }
}
