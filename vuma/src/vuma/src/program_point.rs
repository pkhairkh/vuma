//! Program location tracking for memory operations.
//!
//! A [`ProgramPoint`] identifies a specific location in source code where a
//! memory event (allocation, access, free, etc.) occurs. It is the backbone
//! of provenance tracking in VUMA, enabling the system to trace every memory
//! operation back to its origin in the source program.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;

/// Opaque identifier for an AST / HIR node within the compiler.
///
/// The meaning of a [`NodeId`] is defined by the front-end that feeds data
/// into VUMA. It is treated as opaque here so that the core remains
/// front-end-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u64);

impl Ord for NodeId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl PartialOrd for NodeId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "node#{}", self.0)
    }
}

/// A precise location in source code.
///
/// Contains the file path, line, column, and an optional AST node identifier.
/// [`ProgramPoint`] implements [`Ord`] so that events can be sorted by source
/// position, which is essential for constructing the happens-before relation.
///
/// # Examples
///
/// ```
/// use vuma_core::program_point::{ProgramPoint, NodeId};
///
/// let pp = ProgramPoint {
///     file: "main.vu".into(),
///     line: 42,
///     col: 8,
///     node_id: Some(NodeId(107)),
/// };
/// assert_eq!(format!("{}", pp), "main.vu:42:8 [node#107]");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProgramPoint {
    /// Source file path (relative or absolute).
    pub file: String,
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number.
    pub col: u32,
    /// Optional AST / HIR node identifier for fine-grained correlation.
    pub node_id: Option<NodeId>,
}

impl ProgramPoint {
    /// Create a new [`ProgramPoint`] without a node identifier.
    pub fn new(file: impl Into<String>, line: u32, col: u32) -> Self {
        Self {
            file: file.into(),
            line,
            col,
            node_id: None,
        }
    }

    /// Attach a [`NodeId`] to this program point.
    pub fn with_node(mut self, node_id: NodeId) -> Self {
        self.node_id = Some(node_id);
        self
    }
}

impl fmt::Display for ProgramPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.col)?;
        if let Some(nid) = self.node_id {
            write!(f, " [{}]", nid)?;
        }
        Ok(())
    }
}

impl Ord for ProgramPoint {
    fn cmp(&self, other: &Self) -> Ordering {
        // Lexicographic: file → line → col → node_id
        self.file
            .cmp(&other.file)
            .then_with(|| self.line.cmp(&other.line))
            .then_with(|| self.col.cmp(&other.col))
            .then_with(|| self.node_id.cmp(&other.node_id))
    }
}

impl PartialOrd for ProgramPoint {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_without_node() {
        let pp = ProgramPoint::new("foo.vu", 10, 5);
        assert_eq!(format!("{}", pp), "foo.vu:10:5");
    }

    #[test]
    fn display_with_node() {
        let pp = ProgramPoint::new("bar.vu", 1, 1).with_node(NodeId(99));
        assert_eq!(format!("{}", pp), "bar.vu:1:1 [node#99]");
    }

    #[test]
    fn ordering() {
        let a = ProgramPoint::new("a.vu", 1, 1);
        let b = ProgramPoint::new("a.vu", 1, 2);
        let c = ProgramPoint::new("a.vu", 2, 1);
        let d = ProgramPoint::new("b.vu", 1, 1);
        assert!(a < b);
        assert!(b < c);
        assert!(c < d);
    }
}
