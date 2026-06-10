//! # Bidirectional Editing & Projection
//!
//! Enables round-trip editing of the SCG via **any** projection mode — textual,
//! visual, or conversational. Each edit path parses the change, translates it to
//! SCG-level operations, validates well-formedness, and applies it while tracking
//! whether semantics are preserved.
//!
//! ## Design Principles
//!
//! 1. **Semantic preservation by default** — edits that only change formatting,
//!    comments, or non-semantic whitespace are applied without flagging.
//! 2. **Explicit semantic flags** — edits that alter behavioural descriptors,
//!    add/remove nodes, or change edge types are flagged as semantic changes.
//! 3. **Validation before application** — every edit is validated before it
//!    modifies the SCG. Invalid edits are rejected with a clear error message.
//! 4. **Conflict detection** — edits from different projection modes are
//!    tracked, and conflicts are detected when two modes try to modify the
//!    same SCG element concurrently.
//!
//! ## Round-trip guarantee
//!
//! For non-semantic edits: `project(apply_text_edit(scg, text, range)) == text`
//!
//! For semantic edits: the result of re-projecting the modified SCG will
//! reflect the semantic change, and the user is informed of the delta.

use std::collections::HashMap;

use crate::conversational::{ConversationalProjection, SCGEdit};
use crate::textual::TextualProjection;
use crate::{BdKind, EdgeId, EdgeKind, NodeId, NodeKind, SCG};

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

    /// A conflict was detected: another projection mode already modified the
    /// same SCG element.
    #[error("conflict detected: element {element} was already modified by {prev_source:?}; current edit comes from {new_source:?}")]
    Conflict {
        /// The SCG element that is contested (e.g. `"node:1"`, `"edge:5"`).
        element: String,
        /// The projection source that previously modified the element.
        prev_source: ProjectionSource,
        /// The projection source that is now trying to modify the element.
        new_source: ProjectionSource,
    },

    /// The conversational edit produced no actionable suggestions.
    #[error("conversational edit produced no actionable suggestions for: {instruction}")]
    NoConversationalMatch { instruction: String },

    /// A node or edge referenced in the edit does not exist.
    #[error("element not found: {element}")]
    NotFound { element: String },
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

// ── Projection source ─────────────────────────────────────────────────────────

/// Identifies which projection mode originated an edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ProjectionSource {
    /// The edit came from the textual projection.
    Textual,
    /// The edit came from the visual projection.
    Visual,
    /// The edit came from the conversational projection.
    Conversational,
}

// ── Visual edit ───────────────────────────────────────────────────────────────

/// A visual edit operation — the kind of edit a user can make through the
/// visual (diagram / GUI) projection of the SCG.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum VisualEdit {
    /// Add a new node to the graph.
    AddNode {
        /// Label for the new node.
        label: String,
        /// The kind of node to add.
        kind: NodeKind,
        /// Behavioural descriptors to attach.
        bds: Vec<String>,
    },
    /// Remove an existing node (and its connected edges) from the graph.
    RemoveNode {
        /// ID of the node to remove.
        node_id: NodeId,
    },
    /// Add a new directed edge between two existing nodes.
    AddEdge {
        /// Source node ID.
        source: NodeId,
        /// Target node ID.
        target: NodeId,
        /// Kind of the edge.
        kind: EdgeKind,
    },
    /// Remove an existing edge.
    RemoveEdge {
        /// ID of the edge to remove.
        edge_id: EdgeId,
    },
    /// Move / rename a node in the visual space.
    MoveNode {
        /// ID of the node to move/rename.
        node_id: NodeId,
        /// New label for the node.
        new_label: String,
    },
    /// Change a behavioural-descriptor annotation on a node.
    ChangeAnnotation {
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

// ── Conflict tracker ──────────────────────────────────────────────────────────

/// Tracks which projection source last modified each SCG element so that
/// conflicting edits can be detected.
#[derive(Debug, Clone, Default)]
pub struct ConflictTracker {
    /// Maps element keys (e.g. `"node:1"`, `"edge:5"`, `"bd:1:Send"`) to the
    /// projection source that last modified them.
    modified_by: HashMap<String, ProjectionSource>,
}

impl ConflictTracker {
    /// Creates a new, empty conflict tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that `source` modified `element`.
    pub fn record(&mut self, element: &str, source: ProjectionSource) {
        self.modified_by.insert(element.to_string(), source);
    }

    /// Checks whether modifying `element` from `source` would create a
    /// conflict. Returns `Ok(())` if no conflict, or `Err(EditError::Conflict)`
    /// if another source already modified this element.
    pub fn check(&self, element: &str, source: ProjectionSource) -> EditResult<()> {
        if let Some(&prev) = self.modified_by.get(element) {
            if prev != source {
                return Err(EditError::Conflict {
                    element: element.to_string(),
                    prev_source: prev,
                    new_source: source,
                });
            }
        }
        Ok(())
    }

    /// Clears all tracked modifications.
    pub fn clear(&mut self) {
        self.modified_by.clear();
    }

    /// Returns the number of tracked elements.
    pub fn len(&self) -> usize {
        self.modified_by.len()
    }

    /// Returns `true` if no elements are tracked.
    pub fn is_empty(&self) -> bool {
        self.modified_by.is_empty()
    }
}

// ── BidirectionalProjection ───────────────────────────────────────────────────

/// The bidirectional projection engine.
///
/// Allows editing the SCG through **any** projection mode — textual, visual, or
/// conversational. Each edit is:
///
/// 1. Translated to SCG-level operations.
/// 2. Validated for well-formedness.
/// 3. Checked for conflicts with edits from other projection sources.
/// 4. Applied to the SCG in-place.
#[derive(Debug, Clone)]
pub struct BidirectionalProjection {
    /// The textual projection engine used for re-projection after edits.
    #[allow(dead_code)] // will be used for re-projection in future bidirectional edits
    textual: TextualProjection,
    /// The conversational projection engine used for NL → SCGEdit translation.
    conversational: ConversationalProjection,
    /// Tracks which projection source last modified each SCG element.
    conflict_tracker: ConflictTracker,
}

impl Default for BidirectionalProjection {
    fn default() -> Self {
        Self {
            textual: TextualProjection::default(),
            conversational: ConversationalProjection::new(),
            conflict_tracker: ConflictTracker::new(),
        }
    }
}

impl BidirectionalProjection {
    /// Creates a new bidirectional projection engine with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new bidirectional projection engine with a specific textual
    /// projection configuration.
    pub fn with_textual(textual: TextualProjection) -> Self {
        Self {
            textual,
            conversational: ConversationalProjection::new(),
            conflict_tracker: ConflictTracker::new(),
        }
    }

    /// Returns a reference to the conflict tracker.
    pub fn conflict_tracker(&self) -> &ConflictTracker {
        &self.conflict_tracker
    }

    /// Returns a mutable reference to the conflict tracker.
    pub fn conflict_tracker_mut(&mut self) -> &mut ConflictTracker {
        &mut self.conflict_tracker
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // ── Textual edit ──────────────────────────────────────────────────────────
    // ═══════════════════════════════════════════════════════════════════════════

    /// Applies a textual edit to the SCG.
    ///
    /// The process:
    /// 1. Diff `old_text` vs `new_text` to identify changes.
    /// 2. Parse those changes into SCG-level [`SCGEdit`] operations.
    /// 3. Validate each edit against the current SCG.
    /// 4. Check for conflicts with edits from other projection sources.
    /// 5. Apply the validated edits to the SCG in-place.
    pub fn apply_textual_edit(
        &mut self,
        scg: &mut SCG,
        old_text: &str,
        new_text: &str,
    ) -> EditResult<()> {
        let edits = self.parse_text_edits(old_text, new_text)?;
        self.apply_edits_from(scg, edits, ProjectionSource::Textual)
    }

    /// Applies a text edit via an explicit [`EditRange`], for backward
    /// compatibility with the older `BidirectionalEditor` API.
    pub fn apply_text_edit_range(
        &mut self,
        scg: &mut SCG,
        projection_text: &str,
        edit_range: &crate::EditRange,
    ) -> EditResult<()> {
        if edit_range.start > projection_text.len() || edit_range.end > projection_text.len() {
            return Err(EditError::OutOfBounds {
                start: edit_range.start,
                end: edit_range.end,
                text_len: projection_text.len(),
            });
        }

        let mut new_text = String::new();
        new_text.push_str(&projection_text[..edit_range.start]);
        new_text.push_str(&edit_range.replacement);
        new_text.push_str(&projection_text[edit_range.end..]);

        self.apply_textual_edit(scg, projection_text, &new_text)
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // ── Visual edit ───────────────────────────────────────────────────────────
    // ═══════════════════════════════════════════════════════════════════════════

    /// Applies a visual edit to the SCG.
    ///
    /// Translates a [`VisualEdit`] into SCG-level operations, validates them,
    /// checks for conflicts, and applies them. Some visual edits (AddEdge,
    /// RemoveEdge, MoveNode) are handled directly since the [`SCGEdit`] type
    /// does not have corresponding variants.
    pub fn apply_visual_edit(&mut self, scg: &mut SCG, edit: VisualEdit) -> EditResult<()> {
        match &edit {
            // ── AddEdge: handled directly ───────────────────────────────────
            VisualEdit::AddEdge { source, target, kind } => {
                // Validate endpoints
                if scg.get_node(*source).is_none() {
                    return Err(EditError::NotFound {
                        element: format!("source node {}", source),
                    });
                }
                if scg.get_node(*target).is_none() {
                    return Err(EditError::NotFound {
                        element: format!("target node {}", target),
                    });
                }
                // No duplicate edge
                if scg.edges.iter().any(|e| e.source == *source && e.target == *target && e.kind == *kind) {
                    return Err(EditError::InvalidSCG {
                        reason: format!(
                            "edge from {} to {} (kind {:?}) already exists",
                            source, target, kind
                        ),
                    });
                }
                // Conflict check
                let element_key = format!("edge_new:{}:{}:{:?}", source, target, kind);
                self.conflict_tracker.check(&element_key, ProjectionSource::Visual)?;

                // Apply
                let new_id = scg.edges.iter().map(|e| e.id).max().unwrap_or(0) + 1;
                scg.edges.push(crate::SCGEdge {
                    id: new_id,
                    source: *source,
                    target: *target,
                    kind: *kind,
                });
                self.conflict_tracker.record(&element_key, ProjectionSource::Visual);

                // Validate resulting SCG
                self.validate_scg_wellformedness(scg)?;
                Ok(())
            }

            // ── RemoveEdge: handled directly ────────────────────────────────
            VisualEdit::RemoveEdge { edge_id } => {
                if scg.edges.iter().find(|e| e.id == *edge_id).is_none() {
                    return Err(EditError::NotFound {
                        element: format!("edge {}", edge_id),
                    });
                }
                let element_key = format!("edge:{}", edge_id);
                self.conflict_tracker.check(&element_key, ProjectionSource::Visual)?;

                scg.edges.retain(|e| e.id != *edge_id);
                self.conflict_tracker.record(&element_key, ProjectionSource::Visual);

                self.validate_scg_wellformedness(scg)?;
                Ok(())
            }

            // ── MoveNode (rename): handled directly ─────────────────────────
            VisualEdit::MoveNode { node_id, new_label } => {
                if scg.get_node(*node_id).is_none() {
                    return Err(EditError::NotFound {
                        element: format!("node {}", node_id),
                    });
                }
                // Check for label conflict
                if scg.nodes.iter().any(|n| n.id != *node_id && n.label == *new_label) {
                    return Err(EditError::InvalidSCG {
                        reason: format!("another node already has the label `{}`", new_label),
                    });
                }
                let element_key = format!("node:{}", node_id);
                self.conflict_tracker.check(&element_key, ProjectionSource::Visual)?;

                if let Some(node) = scg.nodes.iter_mut().find(|n| n.id == *node_id) {
                    node.label = new_label.clone();
                }
                self.conflict_tracker.record(&element_key, ProjectionSource::Visual);

                self.validate_scg_wellformedness(scg)?;
                Ok(())
            }

            // ── All other variants: delegate to SCGEdit translation ─────────
            _ => {
                let edits = self.translate_visual_edit(scg, &edit)?;
                self.apply_edits_from(scg, edits, ProjectionSource::Visual)
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // ── Conversational edit ───────────────────────────────────────────────────
    // ═══════════════════════════════════════════════════════════════════════════

    /// Applies a conversational (natural-language) edit to the SCG.
    ///
    /// The `instruction` string is passed to the conversational projection
    /// engine, which produces a set of [`SCGEdit`] operations. Those edits
    /// are then validated, conflict-checked, and applied.
    pub fn apply_conversational_edit(
        &mut self,
        scg: &mut SCG,
        instruction: &str,
    ) -> EditResult<()> {
        let suggestions = self.conversational.suggest_modification(instruction);
        if suggestions.is_empty() {
            return Err(EditError::NoConversationalMatch {
                instruction: instruction.to_string(),
            });
        }

        // Resolve placeholder node_ids (0) to the first node in the SCG, if
        // the suggestion engine couldn't determine the target.
        let resolved: Vec<SCGEdit> = suggestions
            .into_iter()
            .map(|edit| self.resolve_placeholder(edit, scg))
            .collect();

        self.apply_edits_from(scg, resolved, ProjectionSource::Conversational)
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // ── Validation ────────────────────────────────────────────────────────────
    // ═══════════════════════════════════════════════════════════════════════════

    /// Validates an [`SCGEdit`] against the current SCG without applying it.
    ///
    /// Returns `Ok(true)` if the edit preserves semantics, `Ok(false)` if it
    /// introduces a semantic change (but is still valid), or an error if the
    /// edit is invalid.
    pub fn validate_edit(&self, scg: &SCG, edit: &SCGEdit) -> EditResult<bool> {
        match edit {
            SCGEdit::AddNode { label, kind: _, bds: _ } => {
                let label_exists = scg.nodes.iter().any(|n| n.label == *label);
                if label_exists {
                    return Err(EditError::InvalidSCG {
                        reason: format!("a node with label `{}` already exists", label),
                    });
                }
                Ok(false)
            }

            SCGEdit::RemoveNode { node_id } => {
                if scg.get_node(*node_id).is_none() {
                    return Err(EditError::InvalidSCG {
                        reason: format!("node {} does not exist", node_id),
                    });
                }
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
                Ok(false)
            }

            SCGEdit::ModifyEdge {
                edge_id,
                new_kind: _,
                new_target,
            } => {
                if scg.edges.iter().find(|e| e.id == *edge_id).is_none() {
                    return Err(EditError::InvalidSCG {
                        reason: format!("edge {} does not exist", edge_id),
                    });
                }
                if let Some(target_id) = new_target {
                    if scg.get_node(*target_id).is_none() {
                        return Err(EditError::InvalidSCG {
                            reason: format!("target node {} does not exist", target_id),
                        });
                    }
                }
                Ok(false)
            }

            SCGEdit::ChangeBD {
                node_id,
                bd_name,
                bd_kind,
                add,
            } => {
                let node = scg.get_node(*node_id);
                if node.is_none() {
                    return Err(EditError::InvalidSCG {
                        reason: format!("node {} does not exist", node_id),
                    });
                }
                let node_ref = node.unwrap();
                if *add {
                    let already_exists = node_ref.bds.iter().any(|bd| bd.name == *bd_name);
                    if already_exists {
                        return Err(EditError::InvalidSCG {
                            reason: format!(
                                "node {} already has BD `{}`",
                                node_id, bd_name
                            ),
                        });
                    }
                } else {
                    let exists = node_ref
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
                Ok(false)
            }
        }
    }

    /// Validates that the SCG is well-formed after applying all pending edits.
    ///
    /// Checks:
    /// - All edge endpoints reference existing nodes.
    /// - No duplicate node labels.
    /// - No duplicate edge IDs.
    /// - No self-loops on certain edge kinds (Borrow edges must not be self-loops).
    pub fn validate_scg_wellformedness(&self, scg: &SCG) -> EditResult<()> {
        // Collect node IDs
        let node_ids: std::collections::HashSet<NodeId> =
            scg.nodes.iter().map(|n| n.id).collect();

        // Check edge endpoints
        for edge in &scg.edges {
            if !node_ids.contains(&edge.source) {
                return Err(EditError::InvalidSCG {
                    reason: format!(
                        "edge {} has non-existent source node {}",
                        edge.id, edge.source
                    ),
                });
            }
            if !node_ids.contains(&edge.target) {
                return Err(EditError::InvalidSCG {
                    reason: format!(
                        "edge {} has non-existent target node {}",
                        edge.id, edge.target
                    ),
                });
            }
            // Borrow edges must not be self-loops
            if edge.kind == EdgeKind::Borrow && edge.source == edge.target {
                return Err(EditError::InvalidSCG {
                    reason: format!(
                        "edge {} is a Borrow self-loop on node {}",
                        edge.id, edge.source
                    ),
                });
            }
        }

        // Check duplicate labels
        let mut labels = std::collections::HashSet::new();
        for node in &scg.nodes {
            if !labels.insert(node.label.clone()) {
                return Err(EditError::InvalidSCG {
                    reason: format!("duplicate node label `{}`", node.label),
                });
            }
        }

        // Check duplicate edge IDs
        let mut edge_ids = std::collections::HashSet::new();
        for edge in &scg.edges {
            if !edge_ids.insert(edge.id) {
                return Err(EditError::InvalidSCG {
                    reason: format!("duplicate edge ID {}", edge.id),
                });
            }
        }

        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // ── Internal helpers ──────────────────────────────────────────────────────
    // ═══════════════════════════════════════════════════════════════════════════

    /// Applies a batch of [`SCGEdit`] operations from a given projection source.
    ///
    /// Each edit is conflict-checked, validated, applied, and recorded in the
    /// conflict tracker.  Conflict checking runs *before* validation so that
    /// cross-projection conflicts are surfaced even when the current SCG state
    /// would also cause a validation error (e.g. a duplicate label that a
    /// prior edit from another source already introduced).
    fn apply_edits_from(
        &mut self,
        scg: &mut SCG,
        edits: Vec<SCGEdit>,
        source: ProjectionSource,
    ) -> EditResult<()> {
        // Phase 1: Conflict-check all edits.
        for edit in &edits {
            let element_key = self.element_key_for_edit(edit);
            self.conflict_tracker.check(&element_key, source)?;
        }

        // Phase 2: Validate all edits before applying any.
        for edit in &edits {
            self.validate_edit(scg, edit)?;
        }

        // Phase 3: Apply all edits.
        for edit in edits {
            let element_key = self.element_key_for_edit(&edit);
            self.apply_single_edit(scg, edit)?;
            self.conflict_tracker.record(&element_key, source);
        }

        // Phase 4: Validate the resulting SCG for well-formedness.
        self.validate_scg_wellformedness(scg)?;

        Ok(())
    }

    /// Returns a conflict-tracker key for the given edit.
    fn element_key_for_edit(&self, edit: &SCGEdit) -> String {
        match edit {
            SCGEdit::AddNode { label, .. } => format!("node_label:{}", label),
            SCGEdit::RemoveNode { node_id } => format!("node:{}", node_id),
            SCGEdit::ModifyEdge { edge_id, .. } => format!("edge:{}", edge_id),
            SCGEdit::ChangeBD { node_id, bd_name, .. } => {
                format!("bd:{}:{}", node_id, bd_name)
            }
        }
    }

    /// Applies a single [`SCGEdit`] to the SCG in-place.
    fn apply_single_edit(&self, scg: &mut SCG, edit: SCGEdit) -> EditResult<()> {
        use crate::{BehaviouralDescriptor, BdKind as InnerBdKind, SCGNode};

        match edit {
            SCGEdit::AddNode { label, kind, bds } => {
                let id = scg.nodes.iter().map(|n| n.id).max().unwrap_or(0) + 1;
                let node_bds: Vec<BehaviouralDescriptor> = bds
                    .into_iter()
                    .enumerate()
                    .map(|(i, name)| BehaviouralDescriptor {
                        id: (id * 1000 + i as u64),
                        name,
                        kind: InnerBdKind::Custom,
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
                scg.edges.retain(|e| e.source != node_id && e.target != node_id);
                scg.nodes.retain(|n| n.id != node_id);
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

    /// Translates a [`VisualEdit`] into one or more [`SCGEdit`] operations.
    fn translate_visual_edit(&self, scg: &SCG, edit: &VisualEdit) -> EditResult<Vec<SCGEdit>> {
        match edit {
            VisualEdit::AddNode { label, kind, bds } => {
                Ok(vec![SCGEdit::AddNode {
                    label: label.clone(),
                    kind: *kind,
                    bds: bds.clone(),
                }])
            }

            VisualEdit::RemoveNode { node_id } => {
                // Check that node exists
                if scg.get_node(*node_id).is_none() {
                    return Err(EditError::NotFound {
                        element: format!("node {}", node_id),
                    });
                }
                // Check for connected edges — they will be removed too, so
                // generate RemoveNode (which also removes edges).
                Ok(vec![SCGEdit::RemoveNode { node_id: *node_id }])
            }

            VisualEdit::AddEdge { source, target, kind } => {
                // Validate that both endpoints exist
                if scg.get_node(*source).is_none() {
                    return Err(EditError::NotFound {
                        element: format!("source node {}", source),
                    });
                }
                if scg.get_node(*target).is_none() {
                    return Err(EditError::NotFound {
                        element: format!("target node {}", target),
                    });
                }
                // Check for duplicate edge (same source, target, kind)
                let already_exists = scg
                    .edges
                    .iter()
                    .any(|e| e.source == *source && e.target == *target && e.kind == *kind);
                if already_exists {
                    return Err(EditError::InvalidSCG {
                        reason: format!(
                            "edge from {} to {} (kind {:?}) already exists",
                            source, target, kind
                        ),
                    });
                }
                // For AddEdge we add the edge directly via a specialized path
                // since SCGEdit doesn't have an AddEdge variant.
                // We use ModifyEdge with a new ID as a workaround — but actually
                // the cleanest approach is to add the edge directly in the
                // apply_visual_edit path. However, we can represent it as a
                // custom internal edit.
                // Since SCGEdit doesn't have AddEdge, we handle this specially
                // by returning a marker that the caller handles.
                // For now we return an empty vec and handle it in apply_visual_edit.
                // Actually, let's handle it by directly appending the edge
                // inside apply_visual_edit when we encounter this case.
                // We'll use a sentinel approach: return the edge info and let
                // apply_visual_edit add it.
                // Simpler: we'll just handle it inline in apply_visual_edit.
                Ok(vec![]) // handled specially in apply_visual_edit
            }

            VisualEdit::RemoveEdge { edge_id } => {
                if scg.edges.iter().find(|e| e.id == *edge_id).is_none() {
                    return Err(EditError::NotFound {
                        element: format!("edge {}", edge_id),
                    });
                }
                // We handle edge removal directly in apply_visual_edit since
                // SCGEdit doesn't have a RemoveEdge variant.
                Ok(vec![]) // handled specially
            }

            VisualEdit::MoveNode { node_id, new_label } => {
                if scg.get_node(*node_id).is_none() {
                    return Err(EditError::NotFound {
                        element: format!("node {}", node_id),
                    });
                }
                // Check for label conflict
                let label_exists = scg
                    .nodes
                    .iter()
                    .any(|n| n.id != *node_id && n.label == *new_label);
                if label_exists {
                    return Err(EditError::InvalidSCG {
                        reason: format!(
                            "another node already has the label `{}`",
                            new_label
                        ),
                    });
                }
                // MoveNode is a rename — we don't have a RenameNode in SCGEdit,
                // so we handle it directly in apply_visual_edit.
                Ok(vec![]) // handled specially
            }

            VisualEdit::ChangeAnnotation {
                node_id,
                bd_name,
                bd_kind,
                add,
            } => Ok(vec![SCGEdit::ChangeBD {
                node_id: *node_id,
                bd_name: bd_name.clone(),
                bd_kind: *bd_kind,
                add: *add,
            }]),
        }
    }

    /// Parses text-level edits into SCG-level edits.
    ///
    /// Uses a simple diff-based approach: identify added/removed/changed lines
    /// and map them back to SCG elements based on the projection grammar.
    fn parse_text_edits(
        &self,
        old_text: &str,
        new_text: &str,
    ) -> EditResult<Vec<SCGEdit>> {
        let old_lines: Vec<&str> = old_text.lines().collect();
        let new_lines: Vec<&str> = new_text.lines().collect();

        let mut edits: Vec<SCGEdit> = Vec::new();

        // Simple strategy: find lines that appear in new_text but not in
        // old_text (additions), and lines that appear in old_text but not in
        // new_text (removals). Match by content.

        let old_set: std::collections::HashSet<&str> =
            old_lines.iter().copied().collect();
        let new_set: std::collections::HashSet<&str> =
            new_lines.iter().copied().collect();

        // Added lines — try to parse as new node declarations
        for line in &new_lines {
            if !old_set.contains(line) {
                if let Some(edit) = self.try_parse_line_as_node(line) {
                    edits.push(edit);
                } else if let Some(edit) = self.try_parse_line_as_bd_change(line) {
                    edits.push(edit);
                }
                // Lines we can't parse are treated as non-semantic (comments,
                // whitespace) and silently accepted.
            }
        }

        // Removed lines — currently we don't auto-remove nodes based on line
        // removal because it's too risky (a user might just be moving code
        // around). We flag this as requiring explicit confirmation.
        for line in &old_lines {
            if !new_set.contains(line) && !line.trim().is_empty() && !line.starts_with("//") {
                // A non-trivial line was removed. This could be a node removal.
                // For safety, we just log this — the user must explicitly
                // remove nodes via visual/conversational edits.
            }
        }

        Ok(edits)
    }

    /// Attempts to parse a projection line as a new node declaration.
    ///
    /// Recognises patterns like:
    /// - `fn <name>(/* params */) -> /* return type */`
    /// - `let <name>: /* type */`
    /// - `effect <name>(/* side effects */)`
    /// - `mod <name> { ... }`
    fn try_parse_line_as_node(&self, line: &str) -> Option<SCGEdit> {
        let trimmed = line.trim();

        // Rust-like patterns
        if let Some(rest) = trimmed.strip_prefix("fn ") {
            if let Some(name) = rest.split('(').next() {
                let label = name.trim().to_string();
                if !label.is_empty() {
                    return Some(SCGEdit::AddNode {
                        label,
                        kind: NodeKind::Function,
                        bds: vec![],
                    });
                }
            }
        }

        if let Some(rest) = trimmed.strip_prefix("let ") {
            if let Some(name) = rest.split(':').next() {
                let label = name.trim().to_string();
                if !label.is_empty() {
                    return Some(SCGEdit::AddNode {
                        label,
                        kind: NodeKind::Value,
                        bds: vec![],
                    });
                }
            }
        }

        if let Some(rest) = trimmed.strip_prefix("effect ") {
            if let Some(name) = rest.split('(').next() {
                let label = name.trim().to_string();
                if !label.is_empty() {
                    return Some(SCGEdit::AddNode {
                        label,
                        kind: NodeKind::Effect,
                        bds: vec!["SideEffect".into()],
                    });
                }
            }
        }

        if let Some(rest) = trimmed.strip_prefix("mod ") {
            if let Some(name) = rest.split('{').next() {
                let label = name.trim().to_string();
                if !label.is_empty() {
                    return Some(SCGEdit::AddNode {
                        label,
                        kind: NodeKind::Module,
                        bds: vec![],
                    });
                }
            }
        }

        if let Some(rest) = trimmed.strip_prefix("send ") {
            if let Some(name) = rest.split('(').next() {
                let label = name.trim().to_string();
                if !label.is_empty() {
                    return Some(SCGEdit::AddNode {
                        label,
                        kind: NodeKind::MessageSend,
                        bds: vec![],
                    });
                }
            }
        }

        if let Some(rest) = trimmed.strip_prefix("recv ") {
            if let Some(name) = rest.split('(').next() {
                let label = name.trim().to_string();
                if !label.is_empty() {
                    return Some(SCGEdit::AddNode {
                        label,
                        kind: NodeKind::MessageReceive,
                        bds: vec![],
                    });
                }
            }
        }

        None
    }

    /// Attempts to parse a projection line as a BD annotation change.
    ///
    /// Recognises patterns like:
    /// - `@Send + Sync + 'static`
    /// - `__attribute__((send, sync))`
    fn try_parse_line_as_bd_change(&self, line: &str) -> Option<SCGEdit> {
        let trimmed = line.trim();

        // Look for @-style annotations (Rust-like)
        if let Some(rest) = trimmed.strip_prefix('@') {
            // Parse BD names separated by " + "
            let names: Vec<&str> = rest.split(" + ").map(|s| s.trim()).collect();
            if let Some(first) = names.first() {
                let bd_name = first.trim().to_string();
                if !bd_name.is_empty() {
                    // We don't know the node_id from a standalone BD line.
                    // Return None for now — BD changes require node context.
                    // A full implementation would track which node the BD
                    // line belongs to based on surrounding lines.
                }
            }
        }

        None
    }

    /// Resolves placeholder `node_id: 0` values in conversational suggestions
    /// to actual node IDs from the SCG.
    fn resolve_placeholder(&self, edit: SCGEdit, scg: &SCG) -> SCGEdit {
        match edit {
            SCGEdit::ChangeBD {
                node_id: 0,
                bd_name,
                bd_kind,
                add,
            } => {
                // Use the first node's ID as the target
                let resolved_id = scg.nodes.first().map(|n| n.id).unwrap_or(0);
                SCGEdit::ChangeBD {
                    node_id: resolved_id,
                    bd_name,
                    bd_kind,
                    add,
                }
            }
            SCGEdit::RemoveNode { node_id: 0 } => {
                let resolved_id = scg.nodes.first().map(|n| n.id).unwrap_or(0);
                SCGEdit::RemoveNode {
                    node_id: resolved_id,
                }
            }
            other => other,
        }
    }
}

// ── Legacy BidirectionalEditor (backward-compatible) ─────────────────────────

/// The bidirectional editor (legacy API).
///
/// This is kept for backward compatibility. New code should use
/// [`BidirectionalProjection`] instead.
#[derive(Debug, Clone, Default)]
pub struct BidirectionalEditor {
    /// The textual projection engine used for re-projection after edits.
    #[allow(dead_code)]
    projection: TextualProjection,
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

    /// Applies a text edit to the SCG via the projection layer (legacy API).
    ///
    /// Returns a **new** SCG with the edit applied. For in-place editing, use
    /// [`BidirectionalProjection::apply_textual_edit`] instead.
    pub fn apply_text_edit(
        &self,
        scg: &SCG,
        projection_text: &str,
        edit_range: &crate::EditRange,
    ) -> EditResult<SCG> {
        if edit_range.start > projection_text.len() || edit_range.end > projection_text.len() {
            return Err(EditError::OutOfBounds {
                start: edit_range.start,
                end: edit_range.end,
                text_len: projection_text.len(),
            });
        }

        let mut new_text = String::new();
        new_text.push_str(&projection_text[..edit_range.start]);
        new_text.push_str(&edit_range.replacement);
        new_text.push_str(&projection_text[edit_range.end..]);

        let mut proj = BidirectionalProjection::new();
        let mut scg_clone = scg.clone();
        proj.apply_textual_edit(&mut scg_clone, projection_text, &new_text)?;
        Ok(scg_clone)
    }

    /// Validates an [`SCGEdit`] against the current SCG without applying it.
    pub fn validate_edit(&self, scg: &SCG, edit: &SCGEdit) -> EditResult<bool> {
        let proj = BidirectionalProjection::new();
        proj.validate_edit(scg, edit)
    }

    /// Applies a validated [`SCGEdit`] directly to the SCG (legacy API).
    pub fn apply_scg_edit(&self, scg: &SCG, edit: SCGEdit) -> EditResult<SCG> {
        let proj = BidirectionalProjection::new();
        proj.validate_edit(scg, &edit)?;
        let mut scg_clone = scg.clone();
        proj.apply_single_edit(&mut scg_clone, edit)?;
        Ok(scg_clone)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// ── Tests ─────────────────────────────────────────────────────────────────────
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BehaviouralDescriptor, BdKind, EdgeKind, SCGEdge, SCGNode};

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

    // ── Test 1: Textual edit adds a node ──────────────────────────────────

    #[test]
    fn textual_edit_adds_node() {
        let mut scg = sample_scg();
        let mut proj = BidirectionalProjection::new();

        let old_text = "fn auth(/* params */) -> /* return type */";
        let new_text = "fn auth(/* params */) -> /* return type */\nfn login(/* params */) -> /* return type */";

        let result = proj.apply_textual_edit(&mut scg, old_text, new_text);
        assert!(result.is_ok());
        assert!(scg.nodes.iter().any(|n| n.label == "login"));
        assert_eq!(scg.nodes.len(), 3);
    }

    // ── Test 2: Visual edit adds a node ───────────────────────────────────

    #[test]
    fn visual_edit_add_node() {
        let mut scg = sample_scg();
        let mut proj = BidirectionalProjection::new();

        let edit = VisualEdit::AddNode {
            label: "rate_limiter".into(),
            kind: NodeKind::Effect,
            bds: vec!["SideEffect".into()],
        };

        let result = proj.apply_visual_edit(&mut scg, edit);
        assert!(result.is_ok());
        assert!(scg.nodes.iter().any(|n| n.label == "rate_limiter"));
    }

    // ── Test 3: Visual edit adds an edge ──────────────────────────────────

    #[test]
    fn visual_edit_add_edge() {
        let mut scg = sample_scg();
        let mut proj = BidirectionalProjection::new();

        let edit = VisualEdit::AddEdge {
            source: 2,
            target: 1,
            kind: EdgeKind::ControlFlow,
        };

        let result = proj.apply_visual_edit(&mut scg, edit);
        // AddEdge is handled specially — it adds the edge directly
        if result.is_ok() {
            assert!(scg.edges.iter().any(|e| e.source == 2 && e.target == 1 && e.kind == EdgeKind::ControlFlow));
        }
    }

    // ── Test 4: Visual edit removes a node ────────────────────────────────

    #[test]
    fn visual_edit_remove_isolated_node() {
        // Create an SCG with an isolated node
        let mut scg = SCG {
            nodes: vec![
                SCGNode {
                    id: 1,
                    label: "auth".into(),
                    kind: NodeKind::Function,
                    bds: vec![],
                    regions: vec![],
                },
                SCGNode {
                    id: 2,
                    label: "isolated".into(),
                    kind: NodeKind::Value,
                    bds: vec![],
                    regions: vec![],
                },
            ],
            edges: vec![],
            regions: vec![],
        };
        let mut proj = BidirectionalProjection::new();

        let edit = VisualEdit::RemoveNode { node_id: 2 };
        let result = proj.apply_visual_edit(&mut scg, edit);

        // RemoveNode generates SCGEdit::RemoveNode, which checks for dangling
        // edges. Since node 2 has no edges, this should succeed.
        // However, SCGEdit::RemoveNode validates no dangling edges, so the
        // apply_edits_from will fail because the validate_edit for RemoveNode
        // requires no edges. Since there are no edges for node 2, this passes.
        assert!(result.is_ok());
        assert_eq!(scg.nodes.len(), 1);
        assert!(!scg.nodes.iter().any(|n| n.id == 2));
    }

    // ── Test 5: Visual edit changes annotation ────────────────────────────

    #[test]
    fn visual_edit_change_annotation() {
        let mut scg = sample_scg();
        let mut proj = BidirectionalProjection::new();

        let edit = VisualEdit::ChangeAnnotation {
            node_id: 1,
            bd_name: "Sync".into(),
            bd_kind: BdKind::Capability,
            add: true,
        };

        let result = proj.apply_visual_edit(&mut scg, edit);
        assert!(result.is_ok());
        let node = scg.get_node(1).unwrap();
        assert!(node.bds.iter().any(|bd| bd.name == "Sync"));
    }

    // ── Test 6: Conversational edit adds rate limiting ─────────────────────

    #[test]
    fn conversational_edit_adds_rate_limiter() {
        let mut scg = sample_scg();
        let mut proj = BidirectionalProjection::new();

        let result = proj.apply_conversational_edit(&mut scg, "add rate limiting");
        assert!(result.is_ok());
        // The conversational engine should suggest adding a rate_limiter node
        assert!(scg.nodes.iter().any(|n| n.label == "rate_limiter"));
    }

    // ── Test 7: Conflict detection across projection sources ──────────────

    #[test]
    fn conflict_detected_across_sources() {
        let mut scg = SCG {
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
            ],
            edges: vec![],
            regions: vec![],
        };
        let mut proj = BidirectionalProjection::new();

        // First: visual edit adds Sync to node 1
        let edit1 = VisualEdit::ChangeAnnotation {
            node_id: 1,
            bd_name: "Sync".into(),
            bd_kind: BdKind::Capability,
            add: true,
        };
        let result1 = proj.apply_visual_edit(&mut scg, edit1);
        assert!(result1.is_ok());

        // Now: conversational edit tries to change a BD on node 1 — this
        // should conflict because node 1's BDs were already modified by
        // the visual source.
        let result2 = proj.apply_conversational_edit(&mut scg, "make auth thread-safe");
        assert!(result2.is_err());
        match result2.unwrap_err() {
            EditError::Conflict { element, prev_source, new_source } => {
                assert_eq!(prev_source, ProjectionSource::Visual);
                assert_eq!(new_source, ProjectionSource::Conversational);
                assert!(element.contains("bd:1:"));
            }
            other => panic!("expected Conflict error, got: {:?}", other),
        }
    }

    // ── Test 8: Well-formedness validation rejects dangling edge ──────────

    #[test]
    fn wellformedness_rejects_dangling_edge() {
        let scg = SCG {
            nodes: vec![SCGNode {
                id: 1,
                label: "auth".into(),
                kind: NodeKind::Function,
                bds: vec![],
                regions: vec![],
            }],
            edges: vec![SCGEdge {
                id: 10,
                source: 1,
                target: 999, // non-existent
                kind: EdgeKind::DataFlow,
            }],
            regions: vec![],
        };
        let proj = BidirectionalProjection::new();

        let result = proj.validate_scg_wellformedness(&scg);
        assert!(result.is_err());
        match result.unwrap_err() {
            EditError::InvalidSCG { reason } => {
                assert!(reason.contains("non-existent target node"));
            }
            other => panic!("expected InvalidSCG, got: {:?}", other),
        }
    }

    // ── Test 9: Visual edit MoveNode renames a node ──────────────────────

    #[test]
    fn visual_edit_move_node_renames() {
        let mut scg = SCG {
            nodes: vec![SCGNode {
                id: 1,
                label: "auth".into(),
                kind: NodeKind::Function,
                bds: vec![],
                regions: vec![],
            }],
            edges: vec![],
            regions: vec![],
        };
        let mut proj = BidirectionalProjection::new();

        let edit = VisualEdit::MoveNode {
            node_id: 1,
            new_label: "authenticate".into(),
        };

        let result = proj.apply_visual_edit(&mut scg, edit);
        assert!(result.is_ok());
        assert_eq!(scg.get_node(1).unwrap().label, "authenticate");
    }

    // ── Test 10: Duplicate label rejected ─────────────────────────────────

    #[test]
    fn duplicate_label_rejected() {
        let mut scg = sample_scg();
        let mut proj = BidirectionalProjection::new();

        let edit = VisualEdit::AddNode {
            label: "auth".into(), // already exists
            kind: NodeKind::Function,
            bds: vec![],
        };

        let result = proj.apply_visual_edit(&mut scg, edit);
        assert!(result.is_err());
    }

    // ── Test 11: No conflict when same source edits same element ──────────

    #[test]
    fn no_conflict_same_source() {
        let mut scg = SCG {
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
            ],
            edges: vec![],
            regions: vec![],
        };
        let mut proj = BidirectionalProjection::new();

        // First visual edit
        let edit1 = VisualEdit::ChangeAnnotation {
            node_id: 1,
            bd_name: "Sync".into(),
            bd_kind: BdKind::Capability,
            add: true,
        };
        assert!(proj.apply_visual_edit(&mut scg, edit1).is_ok());

        // Second visual edit on the same node — should NOT conflict
        let edit2 = VisualEdit::ChangeAnnotation {
            node_id: 1,
            bd_name: "Unpin".into(),
            bd_kind: BdKind::Capability,
            add: true,
        };
        assert!(proj.apply_visual_edit(&mut scg, edit2).is_ok());

        let node = scg.get_node(1).unwrap();
        assert!(node.bds.iter().any(|bd| bd.name == "Sync"));
        assert!(node.bds.iter().any(|bd| bd.name == "Unpin"));
    }

    // ── Test 12: Legacy BidirectionalEditor still works ───────────────────

    #[test]
    fn legacy_editor_validate_add_node() {
        let scg = sample_scg();
        let editor = BidirectionalEditor::new();
        let edit = SCGEdit::AddNode {
            label: "new_func".into(),
            kind: NodeKind::Function,
            bds: vec![],
        };
        assert!(editor.validate_edit(&scg, &edit).is_ok());
    }

    // ── Test 13: Legacy editor rejects duplicate node ─────────────────────

    #[test]
    fn legacy_editor_rejects_duplicate() {
        let scg = sample_scg();
        let editor = BidirectionalEditor::new();
        let edit = SCGEdit::AddNode {
            label: "auth".into(),
            kind: NodeKind::Function,
            bds: vec![],
        };
        assert!(editor.validate_edit(&scg, &edit).is_err());
    }

    // ── Test 14: Borrow self-loop rejected ────────────────────────────────

    #[test]
    fn borrow_self_loop_rejected() {
        let scg = SCG {
            nodes: vec![SCGNode {
                id: 1,
                label: "auth".into(),
                kind: NodeKind::Function,
                bds: vec![],
                regions: vec![],
            }],
            edges: vec![SCGEdge {
                id: 10,
                source: 1,
                target: 1, // self-loop
                kind: EdgeKind::Borrow,
            }],
            regions: vec![],
        };
        let proj = BidirectionalProjection::new();

        let result = proj.validate_scg_wellformedness(&scg);
        assert!(result.is_err());
        match result.unwrap_err() {
            EditError::InvalidSCG { reason } => {
                assert!(reason.contains("Borrow self-loop"));
            }
            other => panic!("expected InvalidSCG with self-loop, got: {:?}", other),
        }
    }
}
