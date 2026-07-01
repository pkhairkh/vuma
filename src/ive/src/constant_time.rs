//! Constant-Time Verification (Taint Analysis)
//!
//! This module implements a static information flow analysis that verifies
//! no secret-dependent branches or memory accesses exist.
//!
//! # Model
//!
//! - Values are classified as `Public`, `Secret`, or `Unknown`.
//! - Secret values propagate through arithmetic and bitwise ops.
//! - A branch whose condition is `Secret` is a VIOLATION.
//! - A memory access whose address is `Secret` is a VIOLATION.
//! - `ct_select` and `ct_eq` are constant-time by construction.

use std::collections::{HashMap, HashSet};

/// Taint classification for a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Taint {
    Public,
    Secret,
    Unknown,
}

impl Taint {
    pub fn join(self, other: Taint) -> Taint {
        match (self, other) {
            (Taint::Public, Taint::Public) => Taint::Public,
            (Taint::Unknown, _) | (_, Taint::Unknown) => Taint::Unknown,
            _ => Taint::Secret,
        }
    }

    pub fn is_secret(self) -> bool {
        matches!(self, Taint::Secret | Taint::Unknown)
    }
}

/// A constant-time violation.
#[derive(Debug, Clone)]
pub struct ConstantTimeViolation {
    pub node_id: u64,
    pub message: String,
}

/// Verify that a set of nodes in the SCG is constant-time.
///
/// `secret_nodes` are the node IDs that contain secret values.
/// `branch_nodes` are the node IDs that are conditional branches.
/// `access_nodes` are the node IDs that are memory accesses.
///
/// Returns a list of violations.
pub fn verify_constant_time(
    secret_nodes: &HashSet<u64>,
    branch_nodes: &HashSet<u64>,
    access_nodes: &HashSet<u64>,
    edges: &[(u64, u64)],  // (source, target) data flow edges
) -> Vec<ConstantTimeViolation> {
    let mut taint: HashMap<u64, Taint> = HashMap::new();
    let mut violations = Vec::new();

    // Initialize: mark secret nodes
    for &node in secret_nodes {
        taint.insert(node, Taint::Secret);
    }

    // Propagate taint through edges (fixpoint iteration)
    let mut changed = true;
    while changed {
        changed = false;
        for &(src, dst) in edges {
            let src_taint = *taint.get(&src).unwrap_or(&Taint::Public);
            let dst_taint = *taint.get(&dst).unwrap_or(&Taint::Public);
            let new_taint = src_taint.join(dst_taint);
            if new_taint != dst_taint {
                taint.insert(dst, new_taint);
                changed = true;
            }
        }
    }

    // Check branches
    for &node in branch_nodes {
        if let Some(Taint::Secret) | Some(Taint::Unknown) = taint.get(&node) {
            violations.push(ConstantTimeViolation {
                node_id: node,
                message: format!(
                    "SECRET-DEPENDENT BRANCH at node {}: condition depends on secret value",
                    node
                ),
            });
        }
    }

    // Check memory accesses
    for &node in access_nodes {
        if let Some(Taint::Secret) | Some(Taint::Unknown) = taint.get(&node) {
            violations.push(ConstantTimeViolation {
                node_id: node,
                message: format!(
                    "SECRET-DEPENDENT MEMORY ACCESS at node {}: address depends on secret value",
                    node
                ),
            });
        }
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_taint_join() {
        assert_eq!(Taint::Public.join(Taint::Public), Taint::Public);
        assert_eq!(Taint::Public.join(Taint::Secret), Taint::Secret);
        assert_eq!(Taint::Unknown.join(Taint::Public), Taint::Unknown);
    }

    #[test]
    fn test_no_violations_with_no_secrets() {
        let secrets = HashSet::new();
        let branches = HashSet::new();
        let accesses = HashSet::new();
        let edges: Vec<(u64, u64)> = vec![];
        let violations = verify_constant_time(&secrets, &branches, &accesses, &edges);
        assert!(violations.is_empty());
    }

    #[test]
    fn test_secret_branch_detected() {
        let mut secrets = HashSet::new();
        secrets.insert(1);
        let mut branches = HashSet::new();
        branches.insert(2);
        let accesses = HashSet::new();
        let edges = vec![(1, 2)]; // secret flows to branch
        let violations = verify_constant_time(&secrets, &branches, &accesses, &edges);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("BRANCH"));
    }

    #[test]
    fn test_secret_access_detected() {
        let mut secrets = HashSet::new();
        secrets.insert(1);
        let branches = HashSet::new();
        let mut accesses = HashSet::new();
        accesses.insert(3);
        let edges = vec![(1, 2), (2, 3)]; // secret propagates to access
        let violations = verify_constant_time(&secrets, &branches, &accesses, &edges);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("MEMORY ACCESS"));
    }

    #[test]
    fn test_public_branch_ok() {
        let secrets = HashSet::new();
        let mut branches = HashSet::new();
        branches.insert(1);
        let accesses = HashSet::new();
        let edges: Vec<(u64, u64)> = vec![];
        let violations = verify_constant_time(&secrets, &branches, &accesses, &edges);
        assert!(violations.is_empty());
    }
}
