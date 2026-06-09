//! SCG Region Types
//!
//! This module defines memory regions within the Semantic Computation Graph.
//! Regions group nodes into logical memory scopes, enabling reasoning about
//! allocation lifetimes, access boundaries, and security isolation.

use serde::{Deserialize, Serialize};

use crate::node::NodeId;

/// Unique identifier for a region within the SCG.
///
/// `RegionId` is a newtype wrapper around `u64`, providing type safety
/// to distinguish region identifiers from node and edge identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RegionId(pub u64);

impl RegionId {
    /// Creates a new `RegionId` from a `u64` value.
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// Returns the underlying `u64` value.
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for RegionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RegionId({})", self.0)
    }
}

/// Deployment target for a region.
///
/// Specifies where the memory region is physically or logically
/// allocated, which affects access semantics and security properties.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeploymentTarget {
    /// Main program heap memory.
    Heap,
    /// Stack-allocated memory.
    Stack,
    /// GPU device memory.
    Gpu,
    /// Shared memory accessible across processes.
    Shared,
    /// Persisted storage (e.g., memory-mapped file).
    Persisted,
    /// A custom or vendor-specific target identified by name.
    Custom(String),
}

impl std::fmt::Display for DeploymentTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeploymentTarget::Heap => write!(f, "Heap"),
            DeploymentTarget::Stack => write!(f, "Stack"),
            DeploymentTarget::Gpu => write!(f, "Gpu"),
            DeploymentTarget::Shared => write!(f, "Shared"),
            DeploymentTarget::Persisted => write!(f, "Persisted"),
            DeploymentTarget::Custom(name) => write!(f, "Custom({name})"),
        }
    }
}

/// A memory region within the SCG.
///
/// Regions group related allocation, access, and deallocation nodes,
/// providing a scope for memory lifetime analysis and security boundary
/// enforcement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SCGRegion {
    /// The unique identifier of this region.
    pub id: RegionId,
    /// The set of nodes belonging to this region.
    pub nodes: hashbrown::HashSet<NodeId>,
    /// The nesting scope level (0 = top-level, higher = more deeply nested).
    pub scope_level: u32,
    /// Whether this region constitutes a security boundary.
    ///
    /// Security boundaries enforce access restrictions: nodes outside
    /// the boundary cannot directly access memory within it.
    pub security_boundary: bool,
    /// The deployment target specifying where this region's memory resides.
    pub deployment_target: DeploymentTarget,
}

impl SCGRegion {
    /// Creates a new region with the given ID and deployment target.
    ///
    /// The region starts with no nodes, scope level 0, and no security boundary.
    pub fn new(id: RegionId, deployment_target: DeploymentTarget) -> Self {
        Self {
            id,
            nodes: hashbrown::HashSet::new(),
            scope_level: 0,
            security_boundary: false,
            deployment_target,
        }
    }

    /// Creates a new region with a specified scope level.
    pub fn with_scope_level(id: RegionId, deployment_target: DeploymentTarget, scope_level: u32) -> Self {
        Self {
            id,
            nodes: hashbrown::HashSet::new(),
            scope_level,
            security_boundary: false,
            deployment_target,
        }
    }

    /// Creates a new security-boundary region.
    pub fn with_security_boundary(
        id: RegionId,
        deployment_target: DeploymentTarget,
        security_boundary: bool,
    ) -> Self {
        Self {
            id,
            nodes: hashbrown::HashSet::new(),
            scope_level: 0,
            security_boundary,
            deployment_target,
        }
    }

    /// Adds a node to this region.
    pub fn add_node(&mut self, node_id: NodeId) {
        self.nodes.insert(node_id);
    }

    /// Removes a node from this region.
    ///
    /// Returns `true` if the node was present and removed.
    pub fn remove_node(&mut self, node_id: &NodeId) -> bool {
        self.nodes.remove(node_id)
    }

    /// Returns `true` if this region contains the specified node.
    pub fn contains_node(&self, node_id: &NodeId) -> bool {
        self.nodes.contains(node_id)
    }

    /// Returns the number of nodes in this region.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Returns an iterator over the node IDs in this region.
    pub fn iter_nodes(&self) -> impl Iterator<Item = &NodeId> {
        self.nodes.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_region_id_creation_and_display() {
        let id = RegionId::new(3);
        assert_eq!(id.as_u64(), 3);
        assert_eq!(format!("{id}"), "RegionId(3)");
    }

    #[test]
    fn test_deployment_target_display() {
        assert_eq!(format!("{}", DeploymentTarget::Heap), "Heap");
        assert_eq!(format!("{}", DeploymentTarget::Gpu), "Gpu");
        assert_eq!(
            format!("{}", DeploymentTarget::Custom("TPU".to_string())),
            "Custom(TPU)"
        );
    }

    #[test]
    fn test_region_new() {
        let region = SCGRegion::new(RegionId::new(1), DeploymentTarget::Heap);
        assert_eq!(region.id, RegionId::new(1));
        assert!(region.nodes.is_empty());
        assert_eq!(region.scope_level, 0);
        assert!(!region.security_boundary);
    }

    #[test]
    fn test_region_add_remove_nodes() {
        let mut region = SCGRegion::new(RegionId::new(1), DeploymentTarget::Heap);
        let n1 = NodeId::new(10);
        let n2 = NodeId::new(20);

        region.add_node(n1);
        region.add_node(n2);
        assert_eq!(region.node_count(), 2);
        assert!(region.contains_node(&n1));
        assert!(region.contains_node(&n2));

        assert!(region.remove_node(&n1));
        assert_eq!(region.node_count(), 1);
        assert!(!region.contains_node(&n1));

        // Removing again returns false
        assert!(!region.remove_node(&n1));
    }

    #[test]
    fn test_region_security_boundary() {
        let region = SCGRegion::with_security_boundary(
            RegionId::new(2),
            DeploymentTarget::Gpu,
            true,
        );
        assert!(region.security_boundary);
    }
}
