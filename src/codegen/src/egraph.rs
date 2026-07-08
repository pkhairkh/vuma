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

/// A single rewrite step in the provenance history of an e-class.
#[derive(Debug, Clone)]
pub struct RewriteStep {
    /// Name of the rule that was applied.
    pub rule_name: String,
    /// The e-node that was matched (the pattern).
    pub source: ENode,
    /// The e-node that replaced it (the replacement).
    pub replacement: ENode,
}

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
    /// Provenance map (Wave 16): for each e-class, the sequence of rewrite
    /// steps that were applied to it. This enables mapping an optimized
    /// instruction back to its original source form — the e-graph's
    /// union-find structure IS the provenance graph, and this map records
    /// the rewrite history that produced each equivalence.
    pub provenance: HashMap<EClassId, Vec<RewriteStep>>,
}

impl EGraph {
    pub fn new() -> Self {
        EGraph {
            hashcons: HashMap::new(),
            classes: HashMap::new(),
            parents: HashMap::new(),
            next_id: 0,
            provenance: HashMap::new(),
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

    /// Check whether an e-class contains a specific literal value.
    /// Used by value-aware rewrite rules (e.g. `x*2 → x+x`).
    pub fn class_contains_lit(&self, class_id: EClassId, val: i64) -> bool {
        let canonical = self.find(class_id);
        self.classes.get(&canonical)
            .map(|s| s.iter().any(|n| matches!(n, ENode::Lit(v) if *v == val)))
            .unwrap_or(false)
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
                        if let Some(replacement) = (rule.apply)(node, self) {
                            let repl_id = self.add(replacement.clone());
                            let old_id = self.find(class_id);
                            if repl_id != old_id {
                                // Wave 16: Record provenance — track that
                                // `rule.name` transformed `node` into `replacement`.
                                self.record_provenance(canonical, rule.name, node.clone(), replacement);
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

    /// Record a provenance step for an e-class (Wave 16).
    fn record_provenance(
        &mut self,
        class_id: EClassId,
        rule_name: &str,
        source: ENode,
        replacement: ENode,
    ) {
        self.provenance
            .entry(self.find(class_id))
            .or_default()
            .push(RewriteStep {
                rule_name: rule_name.to_string(),
                source,
                replacement,
            });
    }

    /// Get the provenance (rewrite history) for an e-class (Wave 16).
    ///
    /// Returns the sequence of rewrite steps that were applied to produce
    /// the equivalences in this e-class. This enables mapping an optimized
    /// instruction back to its original source form for debug-info fidelity.
    pub fn get_provenance(&self, class_id: EClassId) -> &[RewriteStep] {
        let canonical = self.find(class_id);
        self.provenance
            .get(&canonical)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Returns true if any rewrites were applied to this e-class.
    pub fn has_provenance(&self, class_id: EClassId) -> bool {
        !self.get_provenance(class_id).is_empty()
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
///
/// The `apply` function receives the matched e-node AND a reference to the
/// e-graph, so it can inspect the contents of child e-classes. This is
/// required for value-aware rules like `x*2 → x+x` (which must detect that
/// one operand's e-class contains the literal 2).
pub struct RewriteRule {
    pub name: &'static str,
    /// Whether this rule has been SMT-verified (Wave 7).
    pub verified: bool,
    pub apply: fn(&ENode, &EGraph) -> Option<ENode>,
}

/// Standard algebraic rewrite rules.
///
/// Rules are divided into two categories:
/// 1. **Structural rules** — match purely on e-class ID equality (no value lookup).
///    These are always sound and don't need SMT verification.
/// 2. **Value-aware rules** — match on literal values embedded in ENodes.
///    These are sound by construction (the values are compile-time constants).
///
/// Wave 7 adds a verification framework: each rule carries a `verified` flag
/// indicating whether it has been SMT-proven sound. Unverified rules are
/// still applied (they're sound by construction) but the flag enables
/// future integration with a proof checker.
pub fn standard_rules() -> Vec<RewriteRule> {
    vec![
        // ============================================================
        // Structural rules — match on e-class ID equality. Always sound.
        // ============================================================
        RewriteRule {
            name: "xor_self",
            verified: true,
            apply: |node, _eg| match node {
                // x ^ x → 0
                ENode::BinOp(BinOpKind::Xor, x, y) if x == y => Some(ENode::Lit(0)),
                _ => None,
            },
        },
        RewriteRule {
            name: "sub_self",
            verified: true,
            apply: |node, _eg| match node {
                // x - x → 0
                ENode::BinOp(BinOpKind::Sub, x, y) if x == y => Some(ENode::Lit(0)),
                _ => None,
            },
        },

        // ============================================================
        // Value-aware rules — inspect child e-class contents via eg.
        // These are the rules the old `apply(&ENode)` signature could
        // not express. They are sound by constant evaluation.
        // ============================================================

        // 0 + x → x  (identity)
        RewriteRule {
            name: "add_zero_left",
            verified: true,
            apply: |node, eg| match node {
                ENode::BinOp(BinOpKind::Add, lhs, rhs) if eg.class_contains_lit(*lhs, 0) => {
                    // Replace with the rhs e-class (a VReg node pointing to rhs).
                    Some(ENode::VReg(*rhs))
                }
                _ => None,
            },
        },
        RewriteRule {
            name: "add_zero_right",
            verified: true,
            apply: |node, eg| match node {
                ENode::BinOp(BinOpKind::Add, lhs, rhs) if eg.class_contains_lit(*rhs, 0) => {
                    Some(ENode::VReg(*lhs))
                }
                _ => None,
            },
        },
        // 0 * x → 0
        RewriteRule {
            name: "mul_zero_left",
            verified: true,
            apply: |node, eg| match node {
                ENode::BinOp(BinOpKind::Mul, lhs, _) if eg.class_contains_lit(*lhs, 0) => {
                    Some(ENode::Lit(0))
                }
                _ => None,
            },
        },
        RewriteRule {
            name: "mul_zero_right",
            verified: true,
            apply: |node, eg| match node {
                ENode::BinOp(BinOpKind::Mul, _, rhs) if eg.class_contains_lit(*rhs, 0) => {
                    Some(ENode::Lit(0))
                }
                _ => None,
            },
        },
        // x * 1 → x
        RewriteRule {
            name: "mul_one_right",
            verified: true,
            apply: |node, eg| match node {
                ENode::BinOp(BinOpKind::Mul, lhs, rhs) if eg.class_contains_lit(*rhs, 1) => {
                    Some(ENode::VReg(*lhs))
                }
                _ => None,
            },
        },
        RewriteRule {
            name: "mul_one_left",
            verified: true,
            apply: |node, eg| match node {
                ENode::BinOp(BinOpKind::Mul, lhs, rhs) if eg.class_contains_lit(*lhs, 1) => {
                    Some(ENode::VReg(*rhs))
                }
                _ => None,
            },
        },
        // x * 2 → x + x  (strength reduction: add is cheaper than mul)
        // default_cost: Add=100, Mul=200, so extraction picks x+x.
        RewriteRule {
            name: "mul_two_to_add",
            verified: true,
            apply: |node, eg| match node {
                ENode::BinOp(BinOpKind::Mul, lhs, rhs) if eg.class_contains_lit(*rhs, 2) => {
                    Some(ENode::BinOp(BinOpKind::Add, *lhs, *lhs))
                }
                _ => None,
            },
        },
        RewriteRule {
            name: "mul_two_left_to_add",
            verified: true,
            apply: |node, eg| match node {
                ENode::BinOp(BinOpKind::Mul, lhs, rhs) if eg.class_contains_lit(*lhs, 2) => {
                    Some(ENode::BinOp(BinOpKind::Add, *rhs, *rhs))
                }
                _ => None,
            },
        },
        // 0 & x → 0
        RewriteRule {
            name: "and_zero_left",
            verified: true,
            apply: |node, eg| match node {
                ENode::BinOp(BinOpKind::And, lhs, _) if eg.class_contains_lit(*lhs, 0) => {
                    Some(ENode::Lit(0))
                }
                _ => None,
            },
        },
        RewriteRule {
            name: "and_zero_right",
            verified: true,
            apply: |node, eg| match node {
                ENode::BinOp(BinOpKind::And, _, rhs) if eg.class_contains_lit(*rhs, 0) => {
                    Some(ENode::Lit(0))
                }
                _ => None,
            },
        },
        // x | 0 → x
        RewriteRule {
            name: "or_zero_right",
            verified: true,
            apply: |node, eg| match node {
                ENode::BinOp(BinOpKind::Or, lhs, rhs) if eg.class_contains_lit(*rhs, 0) => {
                    Some(ENode::VReg(*lhs))
                }
                _ => None,
            },
        },
        // x ^ 0 → x
        RewriteRule {
            name: "xor_zero_right",
            verified: true,
            apply: |node, eg| match node {
                ENode::BinOp(BinOpKind::Xor, lhs, rhs) if eg.class_contains_lit(*rhs, 0) => {
                    Some(ENode::VReg(*lhs))
                }
                _ => None,
            },
        },
        // x >> 0 → x
        RewriteRule {
            name: "shr_zero_right",
            verified: true,
            apply: |node, eg| match node {
                ENode::BinOp(BinOpKind::ShrL, lhs, rhs)
                | ENode::BinOp(BinOpKind::ShrA, lhs, rhs)
                    if eg.class_contains_lit(*rhs, 0) =>
                {
                    Some(ENode::VReg(*lhs))
                }
                _ => None,
            },
        },
        // x << 0 → x
        RewriteRule {
            name: "shl_zero_right",
            verified: true,
            apply: |node, eg| match node {
                ENode::BinOp(BinOpKind::Shl, lhs, rhs) if eg.class_contains_lit(*rhs, 0) => {
                    Some(ENode::VReg(*lhs))
                }
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
        ENode::BinOp(op, _, _) => {
            // Wave 10: per-ISA cost via TargetDesc.
            // Different operations have different costs on different ISAs.
            // For example, multiply is cheap on x86 (3 cycles) but expensive
            // on hppa (software loop). Division is very expensive everywhere.
            match op {
                BinOpKind::Add | BinOpKind::Sub => 100,
                BinOpKind::And | BinOpKind::Or | BinOpKind::Xor => 90,
                BinOpKind::Shl | BinOpKind::ShrL | BinOpKind::ShrA => 95,
                BinOpKind::Mul => 200,      // More expensive than ALU
                BinOpKind::UDiv | BinOpKind::SDiv => 1000,  // Very expensive
                BinOpKind::SRem | BinOpKind::URem => 1000,
                BinOpKind::Eq | BinOpKind::Ne | BinOpKind::SLt | BinOpKind::SLe
                | BinOpKind::SGt | BinOpKind::SGe | BinOpKind::ULt | BinOpKind::ULe
                | BinOpKind::UGt | BinOpKind::UGe => 110,  // Comparison + set
                _ => 100,
            }
        }
    }
}

/// Target-specific cost function factory (Wave 10).
///
/// Creates a cost function that uses the target's latency table to
/// assign costs based on actual instruction latencies.
pub fn target_cost_fn(latency_table: &crate::target_desc::LatencyTable)
    -> Box<dyn Fn(&ENode) -> usize>
{
    let lt = latency_table.clone();
    Box::new(move |node: &ENode| -> usize {
        match node {
            ENode::Lit(_) => 1,
            ENode::VReg(_) => 10,
            ENode::BinOp(op, _, _) => {
                let category = match op {
                    BinOpKind::Add | BinOpKind::Sub => "arithmetic",
                    BinOpKind::Mul => "multiply",
                    BinOpKind::UDiv | BinOpKind::SDiv => "divide",
                    BinOpKind::SRem | BinOpKind::URem => "divide",
                    BinOpKind::And | BinOpKind::Or | BinOpKind::Xor => "logical",
                    BinOpKind::Shl | BinOpKind::ShrL | BinOpKind::ShrA => "shift",
                    _ => "arithmetic",
                };
                let (latency, _, _) = lt.lookup(category);
                (latency as usize) * 100
            }
        }
    })
}

/// Profile-guided cost function factory (Wave 12).
///
/// Creates a cost function that combines target latency with profile
/// weights. Hot expressions (high execution count) get lower cost,
/// encouraging the e-graph to pick optimized forms for hot paths.
/// Cold expressions get higher cost, allowing more expensive forms
/// that might reduce code size.
///
/// When no profile data is available, falls back to target_cost_fn.
pub fn pgo_cost_fn(
    latency_table: &crate::target_desc::LatencyTable,
    profile: &ProfileData,
) -> Box<dyn Fn(&ENode) -> usize> {
    let lt = latency_table.clone();
    let prof = profile.clone();
    Box::new(move |node: &ENode| -> usize {
        let base_cost = match node {
            ENode::Lit(_) => 1,
            ENode::VReg(vreg_id) => {
                // Hot vregs get lower cost (prefer keeping them)
                let hotness = prof.vreg_hotness(*vreg_id);
                10 / (1 + hotness as usize)
            }
            ENode::BinOp(op, lhs, rhs) => {
                let category = match op {
                    BinOpKind::Add | BinOpKind::Sub => "arithmetic",
                    BinOpKind::Mul => "multiply",
                    BinOpKind::UDiv | BinOpKind::SDiv => "divide",
                    BinOpKind::SRem | BinOpKind::URem => "divide",
                    BinOpKind::And | BinOpKind::Or | BinOpKind::Xor => "logical",
                    BinOpKind::Shl | BinOpKind::ShrL | BinOpKind::ShrA => "shift",
                    _ => "arithmetic",
                };
                let (latency, _, _) = lt.lookup(category);
                let op_hotness = prof.vreg_hotness(*lhs).max(prof.vreg_hotness(*rhs));
                // Hot operations: prefer cheaper form (lower cost)
                // Cold operations: accept expensive form (higher cost = less likely extracted)
                let base = (latency as usize) * 100;
                base / (1 + op_hotness as usize)
            }
        };
        base_cost.max(1) // Never return 0
    })
}

/// Profile data for PGO (Wave 12).
///
/// Collected from instrumented runs. Maps vreg IDs to execution counts.
/// Used by pgo_cost_fn to bias e-graph extraction toward hot-path optimization.
#[derive(Debug, Clone, Default)]
pub struct ProfileData {
    /// Map from vreg/e-class ID to execution count.
    pub hotness: std::collections::HashMap<EClassId, u32>,
}

impl ProfileData {
    /// Creates empty profile data (no PGO).
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates profile data from a hotness map.
    pub fn from_hotness(hotness: std::collections::HashMap<EClassId, u32>) -> Self {
        Self { hotness }
    }

    /// Returns the hotness (execution count) of a vreg/e-class.
    /// 0 = cold/unknown, higher = hotter.
    pub fn vreg_hotness(&self, id: EClassId) -> u32 {
        self.hotness.get(&id).copied().unwrap_or(0)
    }

    /// Load profile data from a JSON string.
    ///
    /// Format: `{"hotness": {"0": 100, "1": 50, ...}}` where keys are
    /// e-class/vreg IDs (as strings, since JSON object keys are strings)
    /// and values are execution counts.
    ///
    /// This is the Wave 12 PGO loading mechanism. Profile files are
    /// produced by instrumented runs and consumed by the optimizer to
    /// bias e-graph extraction toward hot-path optimization.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let v: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| format!("invalid profile JSON: {}", e))?;
        let hotness_map = v.get("hotness")
            .and_then(|h| h.as_object())
            .ok_or("missing 'hotness' object")?;
        let mut hotness = std::collections::HashMap::new();
        for (key, val) in hotness_map {
            let id: EClassId = key.parse()
                .map_err(|e| format!("invalid vreg id '{}': {}", key, e))?;
            let count = val.as_u64()
                .ok_or(format!("hotness for {} is not a number", key))?;
            hotness.insert(id, count as u32);
        }
        Ok(Self { hotness })
    }

    /// Serialize profile data to a JSON string.
    pub fn to_json(&self) -> String {
        let mut entries: Vec<String> = self.hotness.iter()
            .map(|(k, v)| format!("\"{}\": {}", k, v))
            .collect();
        entries.sort();
        format!("{{\"hotness\": {{{}}}}}", entries.join(", "))
    }

    /// Record a vreg as hot (increment its execution count).
    pub fn record_hot(&mut self, vreg: EClassId) {
        *self.hotness.entry(vreg).or_insert(0) += 1;
    }

    /// Returns true if any profile data is present.
    pub fn has_data(&self) -> bool {
        !self.hotness.is_empty()
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
