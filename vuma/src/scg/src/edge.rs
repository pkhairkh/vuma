//! SCG Edge Types
//!
//! This module defines all edge types used in the Semantic Computation Graph.
//! Edges represent relationships between nodes: data flow, control flow,
//! derivation chains, and annotations.

use serde::{Deserialize, Serialize};

use crate::node::NodeId;

/// Unique identifier for an edge within the SCG.
///
/// `EdgeId` is a newtype wrapper around `u64`, providing type safety
/// to prevent accidental confusion with `NodeId` or other identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EdgeId(pub u64);

impl EdgeId {
    /// Creates a new `EdgeId` from a `u64` value.
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// Returns the underlying `u64` value.
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for EdgeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EdgeId({})", self.0)
    }
}

/// Classification of an edge's semantic role within the SCG.
///
/// Each variant corresponds to a distinct kind of relationship
/// between two nodes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeKind {
    /// A data flow edge: the target node consumes a value produced by the source.
    DataFlow,
    /// A control flow edge: execution may transfer from source to target.
    ControlFlow,
    /// A derivation edge: the target is derived from or depends on the source
    /// in a semantic sense (e.g., a deallocation is derived from an allocation).
    Derivation,
    /// An annotation edge: the source annotates or provides metadata about the target.
    Annotation,
}

impl std::fmt::Display for EdgeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EdgeKind::DataFlow => write!(f, "DataFlow"),
            EdgeKind::ControlFlow => write!(f, "ControlFlow"),
            EdgeKind::Derivation => write!(f, "Derivation"),
            EdgeKind::Annotation => write!(f, "Annotation"),
        }
    }
}

/// Core data associated with every SCG edge.
///
/// `EdgeData` is the universal edge payload stored in the graph.
/// It carries the edge's identity, source and target nodes,
/// kind classification, and an optional label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeData {
    /// The unique identifier of this edge.
    pub id: EdgeId,
    /// The source node of this edge.
    pub source: NodeId,
    /// The target node of this edge.
    pub target: NodeId,
    /// The semantic classification of this edge.
    pub kind: EdgeKind,
    /// An optional textual label describing the relationship.
    pub label: Option<String>,
}

impl EdgeData {
    /// Creates a new `EdgeData` with the given fields.
    ///
    /// The `label` is set to `None` by default.
    pub fn new(id: EdgeId, source: NodeId, target: NodeId, kind: EdgeKind) -> Self {
        Self {
            id,
            source,
            target,
            kind,
            label: None,
        }
    }

    /// Creates a new `EdgeData` with a label.
    pub fn with_label(
        id: EdgeId,
        source: NodeId,
        target: NodeId,
        kind: EdgeKind,
        label: impl Into<String>,
    ) -> Self {
        Self {
            id,
            source,
            target,
            kind,
            label: Some(label.into()),
        }
    }

    /// Sets or replaces the label on this edge, returning the modified edge.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_edge_id_creation_and_display() {
        let id = EdgeId::new(7);
        assert_eq!(id.as_u64(), 7);
        assert_eq!(format!("{id}"), "EdgeId(7)");
    }

    #[test]
    fn test_edge_kind_display() {
        assert_eq!(format!("{}", EdgeKind::DataFlow), "DataFlow");
        assert_eq!(format!("{}", EdgeKind::ControlFlow), "ControlFlow");
        assert_eq!(format!("{}", EdgeKind::Derivation), "Derivation");
        assert_eq!(format!("{}", EdgeKind::Annotation), "Annotation");
    }

    #[test]
    fn test_edge_data_new() {
        let edge = EdgeData::new(
            EdgeId::new(1),
            NodeId::new(10),
            NodeId::new(20),
            EdgeKind::DataFlow,
        );
        assert_eq!(edge.id, EdgeId::new(1));
        assert_eq!(edge.source, NodeId::new(10));
        assert_eq!(edge.target, NodeId::new(20));
        assert!(edge.label.is_none());
    }

    #[test]
    fn test_edge_data_with_label() {
        let edge = EdgeData::with_label(
            EdgeId::new(2),
            NodeId::new(5),
            NodeId::new(6),
            EdgeKind::ControlFlow,
            "then_branch",
        );
        assert_eq!(edge.label, Some("then_branch".to_string()));
    }

    #[test]
    fn test_edge_data_builder_label() {
        let edge = EdgeData::new(
            EdgeId::new(3),
            NodeId::new(1),
            NodeId::new(2),
            EdgeKind::Annotation,
        )
        .label("metadata");
        assert_eq!(edge.label, Some("metadata".to_string()));
    }
}
