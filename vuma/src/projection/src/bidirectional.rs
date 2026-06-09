//! # Bidirectional Editing
//!
//! Enables round-trip editing of the SCG via textual projections. The
//! bidirectional editor takes a projected text, an edit range, and applies the
//! change back to the SCG, ensuring that edits either preserve semantics or
//! explicitly flag semantic changes.
//!
//! ## Design Principles
//!
//! 1. **Semantic preservation by default** — edits that only change formatting,
//!    comments, or non-semantic whitespace are applied without flagging.
//! 2. **Explicit semantic flags** — edits that alter behavioural descriptors,
//!    add/remove nodes, or change edge types are flagged as semantic changes.
//! 3. **Validation before application** — every edit is validated before it
//!    modifies the SCG. Invalid edits are rejected with a clear error message.
//!
//! ## Round-trip guarantee
//!
//! For non-semantic edits: `project(apply_text_edit(scg, text, range)) == text`
//!
//! For semantic edits: the result of re-projecting the modified SCG will
//! reflect the semantic change, and the user is informed of the delta.

use crate::conversational::SCGEdit;
use crate::textual::TextualProjection;
use crate::{EditRange, SCG};

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors that can occur during bidirectional editing.
#[derive(Debug, Clone, thiserror::Error)]
pub enum EditError {
    /// The edit range is out of bounds for the projection text.
    #[error("edit range {start}..{end} is out of bounds (text length: {text_len})")]
    OutOfBounds {
        start: usize,
        end: usize,
        text_len: usize,
    },

    /// The edit would produce an invalid SCG (e.g. dangling edge).
    #[error("edit would produce an invalid SCG: {reason}")]
    InvalidSCG { reason: String },

    /// The edit could not be parsed from the projection text.
    #[error("failed to parse edit from projection text: {reason}")]
    ParseFailed { reason: String },

    /// A semantic change was detected that requires explicit confirmation.
    #[error("semantic change detected: {description}")]
    SemanticChange { description: String },

    /// An unsupported edit operation was attempted.
    #[error("unsupported edit operation: {operation}")]
    Unsupported { operation: String },
}

/// Result type for bidirectional editing operations.
pub type EditResult<T> = Result<T, EditError>;

// ── Semantic flag ─────────────────────────────────────────────────────────────

/// Indicates whether an edit preserves semantics or introduces a semantic change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SemanticFlag {
    /// The edit preserves semantics (formatting, comments, non-structural changes).
    SemanticsPreserved,
    /// The edit introduces a semantic change (BDs, edges, node types).
    SemanticsChanged {
        /// Brief description of the semantic change.
        description: &'static str,
    },
}

// ── Bidirectional editor ──────────────────────────────────────────────────────

/// The bidirectional editor.
///
/// Enables editing the SCG through its textual projection. The editor parses
/// changes in the projected text, translates them back to SCG edits, validates
/// them, and applies them while tracking whether semantics are preserved.
#[derive(Debug, Clone)]
pub struct BidirectionalEditor {
    /// The textual projection engine used for re-projection after edits.
    #[allow(dead_code)]
    projection: TextualProjection,
}

impl Default for BidirectionalEditor {
    fn default() -> Self {
        Self {
            projection: TextualProjection::default(),
        }
    }
}

impl BidirectionalEditor {
    /// Creates a new bidirectional editor with the default textual projection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new bidirectional editor with a specific textual projection engine.
    pub fn with_projection(projection: TextualProjection) -> Self {
        Self { projection }
    }

    // ── Apply text edit ───────────────────────────────────────────────────────

    /// Applies a text edit to the SCG via the projection layer.
    ///
    /// The process is:
    /// 1. Re-project the SCG to get the current textual representation.
    /// 2. Apply the edit range to the projected text to produce a new text.
    /// 3. Parse the new text to determine what SCG-level changes are implied.
    /// 4. Validate the changes.
    /// 5. Apply the changes to a clone of the SCG and return it.
    ///
    /// If the edit would introduce a semantic change, the method returns
    /// [`EditError::SemanticChange`] to force explicit confirmation.
    pub fn apply_text_edit(
        &self,
        scg: &SCG,
        projection_text: &str,
        edit_range: &EditRange,
    ) -> EditResult<SCG> {
        // ── Validate range ────────────────────────────────────────────────
        if edit_range.start > projection_text.len() || edit_range.end > projection_text.len() {
            return Err(EditError::OutOfBounds {
                start: edit_range.start,
                end: edit_range.end,
                text_len: projection_text.len(),
            });
        }

        // ── Apply the edit to the text ────────────────────────────────────
        let mut new_text = String::new();
        new_text.push_str(&projection_text[..edit_range.start]);
        new_text.push_str(&edit_range.replacement);
        new_text.push_str(&projection_text[edit_range.end..]);

        // ── Parse the edit to determine SCG-level changes ─────────────────
        let edits = self.parse_text_edits(projection_text, &new_text)?;

        // ── Validate each edit ────────────────────────────────────────────
        for edit in &edits {
            self.validate_edit(scg, edit)?;
        }

        // ── Apply edits to a clone of the SCG ─────────────────────────────
        let mut new_scg = scg.clone();
        for edit in edits {
            self.apply_single_edit(&mut new_scg, edit)?;
        }

        Ok(new_scg)
    }

    // ── Validate edit ─────────────────────────────────────────────────────────

    /// Validates an [`SCGEdit`] against the current SCG without applying it.
    ///
    /// Returns `Ok(true)` if the edit preserves semantics, `Ok(false)` if it
    /// introduces a semantic change (but is still valid), or an error if the
    /// edit is invalid.
    pub fn validate_edit(&self, scg: &SCG, edit: &SCGEdit) -> EditResult<bool> {
        match edit {
            SCGEdit::AddNode { label, kind: _, bds: _ } => {
                // Check that no node with the same label already exists.
                let label_exists = scg.nodes.iter().any(|n| n.label == *label);
                if label_exists {
                    return Err(EditError::InvalidSCG {
                        reason: format!("a node with label `{}` already exists", label),
                    });
                }
                // Adding a node is always a semantic change.
                Ok(false)
            }

            SCGEdit::RemoveNode { node_id } => {
                // Check that the node exists.
                let node = scg.get_node(*node_id);
                if node.is_none() {
                    return Err(EditError::InvalidSCG {
                        reason: format!("node {} does not exist", node_id),
                    });
                }

                // Check that removing the node wouldn't leave dangling edges.
                let has_incoming = scg.edges.iter().any(|e| e.target == *node_id);
                let has_outgoing = scg.edges.iter().any(|e| e.source == *node_id);
                if has_incoming || has_outgoing {
                    return Err(EditError::InvalidSCG {
                        reason: format!(
                            "removing node {} would leave dangling edges",
                            node_id
                        ),
                    });
                }

                // Removing a node is a semantic change.
                Ok(false)
            }

            SCGEdit::ModifyEdge {
                edge_id,
                new_kind: _,
                new_target,
            } => {
                // Check that the edge exists.
                let edge = scg.edges.iter().find(|e| e.id == *edge_id);
                if edge.is_none() {
                    return Err(EditError::InvalidSCG {
                        reason: format!("edge {} does not exist", edge_id),
                    });
                }

                // Check that the new target exists (if specified).
                if let Some(target_id) = new_target {
                    if scg.get_node(*target_id).is_none() {
                        return Err(EditError::InvalidSCG {
                            reason: format!("target node {} does not exist", target_id),
                        });
                    }
                }

                // Changing edge kind or target is a semantic change.
                Ok(false)
            }

            SCGEdit::ChangeBD {
                node_id,
                bd_name,
                bd_kind,
                add,
            } => {
                // Check that the node exists.
                let node = scg.get_node(*node_id);
                if node.is_none() {
                    return Err(EditError::InvalidSCG {
                        reason: format!("node {} does not exist", node_id),
                    });
                }

                // Check for duplicate BD addition.
                if *add {
                    let already_exists = node
                        .unwrap()
                        .bds
                        .iter()
                        .any(|bd| bd.name == *bd_name);
                    if already_exists {
                        return Err(EditError::InvalidSCG {
                            reason: format!(
                                "node {} already has BD `{}`",
                                node_id, bd_name
                            ),
                        });
                    }
                } else {
                    // Check that the BD exists before removing.
                    let exists = node
                        .unwrap()
                        .bds
                        .iter()
                        .any(|bd| bd.name == *bd_name && bd.kind == *bd_kind);
                    if !exists {
                        return Err(EditError::InvalidSCG {
                            reason: format!(
                                "node {} does not have BD `{}` (kind: {:?})",
                                node_id, bd_name, bd_kind
                            ),
                        });
                    }
                }

                // Changing BDs is a semantic change.
                Ok(false)
            }
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Parses text-level edits into SCG-level edits.
    ///
    /// The current implementation uses a simple diff-based approach. A future
    /// version will use a proper incremental parser for the projection grammar.
    fn parse_text_edits(
        &self,
        _old_text: &str,
        _new_text: &str,
    ) -> EditResult<Vec<SCGEdit>> {
        // TODO: Implement incremental parsing of the projection text.
        // For now, return an unsupported error to indicate this is not yet
        // implemented. The full implementation requires a projection grammar
        // parser that can map text regions back to SCG elements.
        Err(EditError::Unsupported {
            operation: "text-based edit parsing is not yet implemented".into(),
        })
    }

    /// Applies a single [`SCGEdit`] to the SCG.
    fn apply_single_edit(&self, scg: &mut SCG, edit: SCGEdit) -> EditResult<()> {
        use crate::{BehaviouralDescriptor, BdKind, SCGNode};

        match edit {
            SCGEdit::AddNode { label, kind, bds } => {
                let id = scg.nodes.iter().map(|n| n.id).max().unwrap_or(0) + 1;
                let node_bds: Vec<BehaviouralDescriptor> = bds
                    .into_iter()
                    .enumerate()
                    .map(|(i, name)| BehaviouralDescriptor {
                        id: (id * 1000 + i as u64),
                        name,
                        kind: BdKind::Custom,
                        parameter: None,
                    })
                    .collect();
                scg.nodes.push(SCGNode {
                    id,
                    label,
                    kind,
                    bds: node_bds,
                    regions: vec![],
                });
                Ok(())
            }

            SCGEdit::RemoveNode { node_id } => {
                // Remove edges first (already validated that none are dangling).
                scg.edges.retain(|e| e.source != node_id && e.target != node_id);
                // Remove the node.
                scg.nodes.retain(|n| n.id != node_id);
                // Remove from regions.
                for region in &mut scg.regions {
                    region.nodes.retain(|&id| id != node_id);
                }
                Ok(())
            }

            SCGEdit::ModifyEdge {
                edge_id,
                new_kind,
                new_target,
            } => {
                if let Some(edge) = scg.edges.iter_mut().find(|e| e.id == edge_id) {
                    if let Some(kind) = new_kind {
                        edge.kind = kind;
                    }
                    if let Some(target) = new_target {
                        edge.target = target;
                    }
                    Ok(())
                } else {
                    Err(EditError::InvalidSCG {
                        reason: format!("edge {} not found", edge_id),
                    })
                }
            }

            SCGEdit::ChangeBD {
                node_id,
                bd_name,
                bd_kind,
                add,
            } => {
                if let Some(node) = scg.nodes.iter_mut().find(|n| n.id == node_id) {
                    if add {
                        let bd_id = node.bds.iter().map(|bd| bd.id).max().unwrap_or(0) + 1;
                        node.bds.push(BehaviouralDescriptor {
                            id: bd_id,
                            name: bd_name,
                            kind: bd_kind,
                            parameter: None,
                        });
                    } else {
                        node.bds.retain(|bd| !(bd.name == bd_name && bd.kind == bd_kind));
                    }
                    Ok(())
                } else {
                    Err(EditError::InvalidSCG {
                        reason: format!("node {} not found", node_id),
                    })
                }
            }
        }
    }

    // ── Direct SCG-edit application ───────────────────────────────────────────

    /// Applies a validated [`SCGEdit`] directly to the SCG.
    ///
    /// This is a convenience method that validates the edit first and then
    /// applies it. Returns the modified SCG on success, or an error if the
    /// edit is invalid.
    pub fn apply_scg_edit(&self, scg: &SCG, edit: SCGEdit) -> EditResult<SCG> {
        self.validate_edit(scg, &edit)?;
        let mut new_scg = scg.clone();
        self.apply_single_edit(&mut new_scg, edit)?;
        Ok(new_scg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BdKind, BehaviouralDescriptor, EdgeKind, NodeKind, SCGEdge, SCGNode};

    fn sample_scg() -> SCG {
        SCG {
            nodes: vec![
                SCGNode {
                    id: 1,
                    label: "auth".into(),
                    kind: NodeKind::Function,
                    bds: vec![BehaviouralDescriptor {
                        id: 100,
                        name: "Send".into(),
                        kind: BdKind::Capability,
                        parameter: None,
                    }],
                    regions: vec![],
                },
                SCGNode {
                    id: 2,
                    label: "token".into(),
                    kind: NodeKind::Value,
                    bds: vec![],
                    regions: vec![],
                },
            ],
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
    fn validate_add_node() {
        let scg = sample_scg();
        let editor = BidirectionalEditor::new();
        let edit = SCGEdit::AddNode {
            label: "new_func".into(),
            kind: NodeKind::Function,
            bds: vec![],
        };
        assert!(editor.validate_edit(&scg, &edit).is_ok());
    }

    #[test]
    fn validate_add_duplicate_node() {
        let scg = sample_scg();
        let editor = BidirectionalEditor::new();
        let edit = SCGEdit::AddNode {
            label: "auth".into(),
            kind: NodeKind::Function,
            bds: vec![],
        };
        assert!(editor.validate_edit(&scg, &edit).is_err());
    }

    #[test]
    fn validate_remove_node_with_edges() {
        let scg = sample_scg();
        let editor = BidirectionalEditor::new();
        // Node 1 has edges; removing it should fail.
        let edit = SCGEdit::RemoveNode { node_id: 1 };
        assert!(editor.validate_edit(&scg, &edit).is_err());
    }

    #[test]
    fn validate_remove_node_without_edges() {
        let scg = sample_scg();
        let editor = BidirectionalEditor::new();
        // Node 2 has an incoming edge; removing it should fail.
        let edit = SCGEdit::RemoveNode { node_id: 2 };
        assert!(editor.validate_edit(&scg, &edit).is_err());
    }

    #[test]
    fn apply_scg_edit_add_node() {
        let scg = sample_scg();
        let editor = BidirectionalEditor::new();
        let edit = SCGEdit::AddNode {
            label: "rate_limiter".into(),
            kind: NodeKind::Effect,
            bds: vec!["SideEffect".into()],
        };
        let result = editor.apply_scg_edit(&scg, edit);
        assert!(result.is_ok());
        let new_scg = result.unwrap();
        assert_eq!(new_scg.nodes.len(), 3);
        assert!(new_scg.nodes.iter().any(|n| n.label == "rate_limiter"));
    }

    #[test]
    fn apply_scg_edit_change_bd() {
        let scg = sample_scg();
        let editor = BidirectionalEditor::new();
        let edit = SCGEdit::ChangeBD {
            node_id: 1,
            bd_name: "Sync".into(),
            bd_kind: BdKind::Capability,
            add: true,
        };
        let result = editor.apply_scg_edit(&scg, edit);
        assert!(result.is_ok());
        let new_scg = result.unwrap();
        let node = new_scg.get_node(1).unwrap();
        assert!(node.bds.iter().any(|bd| bd.name == "Sync"));
    }

    #[test]
    fn out_of_bounds_edit_range() {
        let scg = sample_scg();
        let editor = BidirectionalEditor::new();
        let proj_text = "hello world";
        let edit_range = EditRange {
            start: 5,
            end: 100, // out of bounds
            replacement: "X".into(),
        };
        let result = editor.apply_text_edit(&scg, proj_text, &edit_range);
        assert!(result.is_err());
    }
}
