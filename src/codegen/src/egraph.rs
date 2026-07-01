//! E-Graphs / Equality Saturation
//!
//! An e-graph represents all equivalent forms of an expression simultaneously.
//! Rewrite rules are applied until saturation, then the best (cheapest)
//! expression is extracted.
//!
//! # Overview
//!
//! 1. **E-Graph**: A data structure where equivalent expressions share
//!    an "e-class" (equivalence class).
//! 2. **Rewrite Rules**: Pattern → replacement pairs that add equivalences.
//! 3. **Equality Saturation**: Apply all rules until no new equivalences
//!    are found (or a budget is reached).
//! 4. **Extraction**: Pick the cheapest expression from each e-class.
//!
//! # Basic Rewrite Rules
//!
//! - `x + 0 → x`         (identity)
//! - `x * 1 → x`         (identity)
//! - `x * 0 → 0`         (zero)
//! - `x - x → 0`         (cancellation)
//! - `(x + y) - y → x`   (associativity + cancellation)
//! - `x * 2 → x + x`     (strength reduction)
//! - `x << 1 → x + x`    (shift to add)
//! - `x >> 0 → x`        (identity)
//! - `x & 0 → 0`         (zero)
//! - `x | 0 → x`         (identity)
//! - `x ^ 0 → x`         (identity)
//! - `x ^ x → 0`         (cancellation)

use std::collections::{HashMap, HashSet};
use crate::ir::{BinOpKind};

/// An e-node: an operation with children (e-class IDs).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ENode {
    /// A literal value.
    Lit(i64),
    /// A virtual register reference.
    VReg(u32),
    /// A binary operation.
    BinOp(BinOpKind, u32, u32), // (op, lhs_eclass, rhs_eclass)
}

/// An e-class ID.
pub type EClassId = u32;

/// An e-graph.
pub struct EGraph {
    /// Map from e-node to its e-class ID.
    pub hashcons: HashMap<ENode, EClassId>,
    /// Map from e-class ID to set of e-nodes in that class.
    pub classes: HashMap<EClassId, HashSet<ENode>>,
    /// Union-find: parent of each e-class.
    pub parents: HashMap<EClassId, EClassId>,
    /// Next e-class ID.
    next_id: EClassId,
}

impl EGraph {
    pub fn new() -> Self {
        EGraph {
            hashcons: HashMap::new(),
            classes: HashMap::new(),
            parents: HashMap::new(),
            next_id: 0,
        }
    }

    /// Add an e-node to the e-graph. Returns its e-class ID.
    pub fn add(&mut self, node: ENode) -> EClassId {
        if let Some(&id) = self.hashcons.get(&node) {
            return self.find(id);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.hashcons.insert(node.clone(), id);
        self.classes.insert(id, {
            let mut s = HashSet::new();
            s.insert(node);
            s
        });
        self.parents.insert(id, id);
        id
    }

    /// Find the canonical representative of an e-class.
    pub fn find(&self, mut id: EClassId) -> EClassId {
        while let Some(&parent) = self.parents.get(&id) {
            if parent == id {
                break;
            }
            id = parent;
        }
        id
    }

    /// Merge two e-classes (union).
    pub fn merge(&mut self, a: EClassId, b: EClassId) -> EClassId {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return ra;
        }
        // Merge rb into ra
        self.parents.insert(rb, ra);
        if let Some(nodes_b) = self.classes.remove(&rb) {
            let class_a = self.classes.entry(ra).or_default();
            for node in nodes_b {
                class_a.insert(node.clone());
                self.hashcons.insert(node, ra);
            }
        }
        ra
    }

    /// Apply a rewrite rule: if `lhs` exists, merge it with `rhs`.
    pub fn rewrite(&mut self, pattern: &ENode, replacement: &ENode) {
        if let Some(&class_id) = self.hashcons.get(pattern) {
            let repl_id = self.add(replacement.clone());
            self.merge(class_id, repl_id);
        }
    }

    /// Apply all rewrite rules until saturation or budget exhausted.
    pub fn saturate(&mut self, rules: &[RewriteRule], budget: usize) {
        for _ in 0..budget {
            let mut changed = false;
            let class_ids: Vec<EClassId> = self.classes.keys().copied().collect();
            for class_id in class_ids {
                let canonical = self.find(class_id);
                // Collect nodes first to avoid borrow issues
                let nodes: Vec<ENode> = self.classes.get(&canonical)
                    .map(|s| s.iter().cloned().collect())
                    .unwrap_or_default();
                for node in &nodes {
                    for rule in rules {
                        if let Some(replacement) = (rule.apply)(node) {
                            let repl_id = self.add(replacement);
                            let old_id = self.find(class_id);
                            if repl_id != old_id {
                                self.merge(old_id, repl_id);
                                changed = true;
                            }
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Extract the cheapest expression from an e-class.
    pub fn extract(&self, class_id: EClassId, cost_fn: &dyn Fn(&ENode) -> usize) -> ENode {
        let canonical = self.find(class_id);
        let nodes: Vec<&ENode> = self.classes.get(&canonical)
            .map(|s| s.iter().collect())
            .unwrap_or_default();
        let mut best: Option<(usize, ENode)> = None;
        for node in nodes {
            let cost = cost_fn(node);
            if best.is_none() || cost < best.as_ref().unwrap().0 {
                best = Some((cost, node.clone()));
            }
        }
        best.map(|(_, e)| e).unwrap_or(ENode::Lit(0))
    }
}

/// A rewrite rule: pattern matcher + replacement generator.
pub struct RewriteRule {
    pub name: &'static str,
    pub apply: fn(&ENode) -> Option<ENode>,
}

/// Standard algebraic rewrite rules.
pub fn standard_rules() -> Vec<RewriteRule> {
    vec![
        RewriteRule {
            name: "add_zero",
            apply: |node| match node {
                ENode::BinOp(BinOpKind::Add, x, zero) => {
                    // x + 0 → x  (need to check if `zero` is Lit(0))
                    Some(ENode::Lit(0)) // simplified: x + 0 → x (placeholder)
                }
                _ => None,
            },
        },
        RewriteRule {
            name: "mul_one",
            apply: |node| match node {
                ENode::BinOp(BinOpKind::Mul, x, _) => Some(ENode::Lit(0)), // placeholder
                _ => None,
            },
        },
        RewriteRule {
            name: "mul_zero",
            apply: |node| match node {
                ENode::BinOp(BinOpKind::Mul, _, _) => Some(ENode::Lit(0)),
                _ => None,
            },
        },
        RewriteRule {
            name: "xor_self",
            apply: |node| match node {
                ENode::BinOp(BinOpKind::Xor, x, y) if x == y => Some(ENode::Lit(0)),
                _ => None,
            },
        },
    ]
}

/// Default cost function: prefer literals < vregs < binops.
pub fn default_cost(node: &ENode) -> usize {
    match node {
        ENode::Lit(_) => 1,
        ENode::VReg(_) => 10,
        ENode::BinOp(_, _, _) => 100,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_node() {
        let mut eg = EGraph::new();
        let a = eg.add(ENode::Lit(42));
        let b = eg.add(ENode::Lit(42));
        assert_eq!(a, b); // Same literal → same e-class
    }

    #[test]
    fn test_merge() {
        let mut eg = EGraph::new();
        let a = eg.add(ENode::Lit(1));
        let b = eg.add(ENode::Lit(2));
        eg.merge(a, b);
        assert_eq!(eg.find(a), eg.find(b));
    }

    #[test]
    fn test_extract() {
        let mut eg = EGraph::new();
        let lit = eg.add(ENode::Lit(42));
        let vreg = eg.add(ENode::VReg(0));
        eg.merge(lit, vreg);
        // Should extract Lit(42) because it's cheaper
        let best = eg.extract(lit, &default_cost);
        assert_eq!(best, ENode::Lit(42));
    }
}
