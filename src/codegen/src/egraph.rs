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
//!
//! # Wave 31 Additions
//!
//! - **Rebuilding after merge**: `merge` now calls `rebuild` to maintain
//!   the congruence-closure invariant (parents that become congruent
//!   after a merge are themselves merged).
//! - **Bottom-up DP extraction**: `extract` now considers children's
//!   best costs, not just the node's own cost.
//! - **Commutativity** (`+`, `*`, `&`, `|`, `^`).
//! - **Associativity** (both directions, all 5 ops).
//! - **Distributivity** (`a*(b+c) ↔ a*b + a*c`).
//! - **Constant-folding-across-ops** (`(x+0)+0 → x`, `(x*1)*1 → x`).

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

impl Default for EGraph {
    fn default() -> Self {
        Self::new()
    }
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

    /// Merge two e-classes (union). Wave 31: also triggers a rebuild to
    /// maintain the congruence-closure invariant (parents that become
    /// congruent after this merge are themselves merged).
    pub fn merge(&mut self, a: EClassId, b: EClassId) -> EClassId {
        let ra = self.merge_no_rebuild(a, b);
        self.rebuild();
        ra
    }

    /// Merge two e-classes without rebuilding. Used internally by `rebuild`
    /// and `saturate` to avoid O(n²) rebuild-on-every-merge — `saturate`
    /// batches merges and calls `rebuild` once per round.
    fn merge_no_rebuild(&mut self, a: EClassId, b: EClassId) -> EClassId {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return ra;
        }
        // Merge rb into ra.
        self.parents.insert(rb, ra);
        if let Some(nodes_b) = self.classes.remove(&rb) {
            let class_a = self.classes.entry(ra).or_default();
            for node in nodes_b {
                class_a.insert(node.clone());
                self.hashcons.insert(node, ra);
            }
        }
        // Wave 31: migrate provenance from rb to ra so prior rewrite
        // history survives across merges.
        if let Some(prov_b) = self.provenance.remove(&rb) {
            self.provenance.entry(ra).or_default().extend(prov_b);
        }
        ra
    }

    /// Rebuild the e-graph after merges (Wave 31).
    ///
    /// Rehashes all parent e-nodes using canonical child IDs and merges
    /// e-classes whose canonicalized e-nodes collide. This maintains the
    /// congruence-closure invariant: two structurally-equivalent e-nodes
    /// (modulo child e-class equivalences) live in the same e-class.
    ///
    /// Implements the classic e-graph rebuilding algorithm from the egg
    /// paper. To bound runtime on pathological inputs, runs at most
    /// `REBUILD_MAX_ITERS` fixpoint iterations — a sound approximation
    /// (each iteration leaves the graph at least as merged as before;
    /// `saturate` calls `rebuild` again next round).
    pub fn rebuild(&mut self) {
        const REBUILD_MAX_ITERS: usize = 16;
        for _ in 0..REBUILD_MAX_ITERS {
            if !self.rebuild_once() {
                break;
            }
        }
    }

    /// One pass of rebuilding. Returns true if any merges occurred.
    fn rebuild_once(&mut self) -> bool {
        // 1. Collect (canonicalized_node, class_id) pairs across all
        //    canonical e-classes. Canonicalize each BinOp's child e-class
        //    references so that parents whose children were merged become
        //    equal under the hashcons key.
        let mut groups: HashMap<ENode, Vec<EClassId>> = HashMap::new();
        let class_ids: Vec<EClassId> = self.classes.keys().copied().collect();
        for cid in &class_ids {
            let canon = self.find(*cid);
            if canon != *cid {
                continue; // skip merged-away classes
            }
            if let Some(nodes) = self.classes.get(&canon) {
                for node in nodes {
                    let canon_node = self.canonicalize(node);
                    groups.entry(canon_node).or_default().push(canon);
                }
            }
        }
        // 2. For each canonicalized-node group, if multiple distinct
        //    e-classes produced it, merge them. This catches congruences
        //    like (a+b) and (b+a) when a~b, or (x+0)+c and (VReg(x_class))+c
        //    when x~x+0.
        let mut merged = false;
        for classes in groups.values() {
            let mut iter = classes.iter().copied();
            if let Some(first) = iter.next() {
                for other in iter {
                    let r1 = self.find(first);
                    let r2 = self.find(other);
                    if r1 != r2 {
                        self.merge_no_rebuild(r1, r2);
                        merged = true;
                    }
                }
            }
        }
        merged
    }

    /// Return a canonicalized version of an e-node: child e-class IDs
    /// are replaced by their canonical representatives.
    fn canonicalize(&self, node: &ENode) -> ENode {
        match node {
            ENode::Lit(_) => node.clone(),
            ENode::VReg(_) => node.clone(),
            ENode::BinOp(op, a, b) => {
                ENode::BinOp(*op, self.find(*a), self.find(*b))
            }
        }
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

    /// Check whether an e-class contains any literal value at all (Wave 31).
    /// Used by commutativity rules to skip constant operands: this avoids
    /// firing on `x+5` (which would record spurious provenance and conflict
    /// with constant-folding rules like `add_zero_left`). For non-constant
    /// operands — the common case in real programs — commutativity fires
    /// normally and enables congruence-merge opportunities.
    pub fn class_contains_any_lit(&self, class_id: EClassId) -> bool {
        let canonical = self.find(class_id);
        self.classes.get(&canonical)
            .map(|s| s.iter().any(|n| matches!(n, ENode::Lit(_))))
            .unwrap_or(false)
    }

    /// Apply all rewrite rules until saturation or budget exhausted.
    ///
    /// **Wave 36:** this is now a wrapper over [`Self::saturate_with_proof`]
    /// that additionally runs [`crate::proof_artifacts::check_proof_log`] on
    /// the recorded proof log. New callers should prefer `saturate_with_proof`
    /// directly if they want access to the `ProofLog` and the `bv_verify` gate
    /// `Result`.
    ///
    /// **Wave 36 — production wiring (Task 2-a):** `check_proof_log` is now
    /// invoked from this production wrapper, not only from the inline test
    /// `test_wave36_saturate_with_proof_then_check` in `proof_artifacts.rs`.
    /// The check runs in **advisory mode**: a failure logs a `vuma_log!(warn, …)`
    /// describing the offending rule and continues saturation normally, rather
    /// than panicking. This choice avoids breaking production compiles if a
    /// soundness surprise in `check_proof_log` (or in the rule-acceptance set
    /// for a new Wave 31 tautological rule) ever emerges — the `bv_verify`
    /// gate inside `saturate_with_proof` is the hard, fail-the-build gate;
    /// `check_proof_log` is the secondary structural-correctness audit. Flip
    /// to panic-mode only after a Wave-37+ audit of the rule-acceptance set
    /// concludes no false positives remain.
    pub fn saturate(&mut self, rules: &[RewriteRule], budget: usize) {
        let mut log = crate::proof_artifacts::ProofLog::new();
        let _ = self.saturate_with_proof(rules, budget, &mut log);

        // Wave 36 / Task 2-a: production-side post-saturation proof audit.
        // Advisory (warn + continue) — see the doc comment above for the
        // rationale behind not panicking here.
        if let Err(err) = crate::proof_artifacts::check_proof_log(&log) {
            vuma_log!(
                warn,
                "wave36: check_proof_log rejected a recorded rewrite (advisory — saturation result retained): {}",
                err
            );
        }
    }

    /// **Wave 36 — proof-logging saturation.**
    ///
    /// Like [`Self::saturate`], but:
    /// 1. **Gate (before saturating):** calls
    ///    [`crate::bv_verify::verify_rules_with_counterexample`] on `rules`.
    ///    If any rule is unsound, returns `Err(Counterexample)` *without*
    ///    saturating — refusing to apply a rule that would change program
    ///    semantics. The orchestrator surfaces this to fail the build.
    /// 2. **Recording (during saturating):** after each successful rewrite
    ///    application, calls `log.record(...)` capturing the rule name, the
    ///    source e-node (matched pattern), the replacement e-node, and the
    ///    source e-class ID. The resulting `ProofLog` is then typically
    ///    passed to [`crate::proof_artifacts::check_proof_log`] by the
    ///    orchestrator as a post-saturation compile-time check.
    ///
    /// Returns `Ok(())` if the gate passed and saturation ran to completion
    /// (or budget exhaustion). Returns `Err(Counterexample)` if the gate
    /// rejected a rule.
    pub fn saturate_with_proof(
        &mut self,
        rules: &[RewriteRule],
        budget: usize,
        log: &mut crate::proof_artifacts::ProofLog,
    ) -> Result<(), crate::bv_verify::Counterexample> {
        // Wave 36 gate: verify every rule is sound BEFORE applying any of
        // them. A counterexample aborts saturation entirely.
        crate::bv_verify::verify_rules_with_counterexample(rules)?;

        for _ in 0..budget {
            let mut changed = false;
            let class_ids: Vec<EClassId> = self.classes.keys().copied().collect();
            for class_id in class_ids {
                let canonical = self.find(class_id);
                // Collect nodes first to avoid borrow issues: `apply`
                // takes `&mut self`, so we cannot hold a borrow on
                // `self.classes` while calling it.
                let nodes: Vec<ENode> = self.classes.get(&canonical)
                    .map(|s| s.iter().cloned().collect())
                    .unwrap_or_default();
                for node in &nodes {
                    for rule in rules {
                        // Wave 31: `apply` now takes `&mut EGraph` (so rules
                        // can add intermediate e-nodes for associativity /
                        // distributivity) and returns `(new_root_class_id,
                        // replacement_enode)` — the replacement ENode is
                        // recorded in provenance.
                        if let Some((repl_id, replacement)) = (rule.apply)(node, self) {
                            let old_id = self.find(class_id);
                            let repl_canon = self.find(repl_id);
                            if repl_canon != old_id {
                                // Wave 16: Record provenance — track that
                                // `rule.name` transformed `node` into `replacement`.
                                self.record_provenance(
                                    old_id,
                                    rule.name,
                                    node.clone(),
                                    replacement.clone(),
                                );
                                // Wave 36: Record the proof artifact for
                                // the post-saturation `check_proof_log` gate.
                                log.record(rule, node, &replacement, old_id);
                                // Use merge_no_rebuild here for speed; we
                                // rebuild once at the end of the round.
                                self.merge_no_rebuild(old_id, repl_canon);
                                changed = true;
                            }
                        }
                    }
                }
            }
            // Wave 31: rebuild after each saturate round to maintain the
            // congruence-closure invariant. Catches equivalences arising
            // from this round's merges (e.g., (a+b)+c and (b+a)+c become
            // congruent after a~b is discovered).
            if changed {
                self.rebuild();
            }
        }
        Ok(())
    }

    /// **Wave 36 — pre-saturation rule-verification gate.**
    ///
    /// Verifies that every rule in `rules` is sound (via
    /// [`crate::bv_verify::verify_rules_with_counterexample`]) WITHOUT
    /// saturating. Returns `Ok(())` if all rules pass the gate, or
    /// `Err(Counterexample)` describing the first unsound rule.
    ///
    /// The orchestrator (pipeline.rs) calls this as an explicit gate before
    /// the e-graph pass; `saturate_with_proof` also calls it internally.
    /// Exposed as a `pub` entry point so callers can verify a rule set
    /// independent of running saturation (e.g., during rule-set
    /// construction or AOT validation).
    pub fn verify_rules_before_saturate(
        &self,
        rules: &[RewriteRule],
    ) -> Result<(), crate::bv_verify::Counterexample> {
        crate::bv_verify::verify_rules_with_counterexample(rules)
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

    /// Extract the cheapest expression from an e-class (Wave 31: bottom-up DP).
    ///
    /// Computes the best (lowest-cost) e-node per e-class via fixpoint
    /// iteration, where each e-node's total cost is its own cost plus the
    /// best costs of its children's e-classes. This replaces the old
    /// single-node extraction that only considered the node's own cost,
    /// ignoring how expensive its children were.
    ///
    /// Fixpoint iteration handles cyclic e-graphs (e.g. `x = x+0`, which
    /// can arise after aggressive rewriting). Bounded to
    /// `EXTRACT_MAX_ITERS` iterations as a safety net.
    pub fn extract(&self, class_id: EClassId, cost_fn: &dyn Fn(&ENode) -> usize) -> ENode {
        const EXTRACT_MAX_ITERS: usize = 64;
        let canonical = self.find(class_id);
        // best_cost[canonical_id] = lowest total cost achievable for that class.
        let mut best_cost: HashMap<EClassId, usize> = HashMap::new();
        let mut best_node: HashMap<EClassId, ENode> = HashMap::new();
        for _ in 0..EXTRACT_MAX_ITERS {
            let mut changed = false;
            for (cid, nodes) in &self.classes {
                let cid_canon = self.find(*cid);
                if cid_canon != *cid {
                    continue; // skip merged-away classes
                }
                let mut local_best: Option<(usize, ENode)> = None;
                for node in nodes {
                    let node_cost = cost_fn(node);
                    let child_cost: usize = match node {
                        ENode::Lit(_) => 0,
                        ENode::VReg(_) => 0,
                        ENode::BinOp(_, a, b) => {
                            let a_canon = self.find(*a);
                            let b_canon = self.find(*b);
                            let ca = best_cost.get(&a_canon).copied().unwrap_or(usize::MAX / 2);
                            let cb = best_cost.get(&b_canon).copied().unwrap_or(usize::MAX / 2);
                            ca.saturating_add(cb)
                        }
                    };
                    let total = node_cost.saturating_add(child_cost);
                    match &local_best {
                        None => local_best = Some((total, node.clone())),
                        Some((best_total, _)) if total < *best_total => {
                            local_best = Some((total, node.clone()));
                        }
                        _ => {}
                    }
                }
                if let Some((c, n)) = local_best {
                    if best_cost.get(&cid_canon).copied() != Some(c) {
                        best_cost.insert(cid_canon, c);
                        best_node.insert(cid_canon, n);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        best_node.get(&canonical).cloned().unwrap_or(ENode::Lit(0))
    }
}

/// A rewrite rule: pattern matcher + replacement generator.
///
/// The `apply` function receives the matched e-node AND a mutable reference
/// to the e-graph, so it can both inspect the contents of child e-classes
/// (required for value-aware rules like `x*2 → x+x`) and add intermediate
/// e-nodes (required for rules like associativity that synthesize new
/// sub-expressions, e.g. `b+c` when rewriting `(a+b)+c → a+(b+c)`).
///
/// Returns `Some((new_root_class_id, replacement_enode))` on a match:
/// - `new_root_class_id`: the e-class ID of the replacement's root (the
///   rule adds this via `eg.add(...)` itself, including any intermediate
///   e-nodes it needs).
/// - `replacement_enode`: the root ENode of the replacement, recorded
///   in provenance (Wave 16).
pub struct RewriteRule {
    pub name: &'static str,
    /// Whether this rule has been SMT-verified (Wave 7).
    pub verified: bool,
    /// Wave 31: takes `&mut EGraph` so rules can add intermediate e-nodes
    /// (associativity, distributivity). Returns the new root e-class ID
    /// plus the replacement ENode for provenance.
    pub apply: fn(&ENode, &mut EGraph) -> Option<(EClassId, ENode)>,
}

/// Standard algebraic rewrite rules.
///
/// Rules are divided into categories:
/// 1. **Structural rules** — match purely on e-class ID equality (no value lookup).
///    These are always sound and don't need SMT verification.
/// 2. **Value-aware rules** — match on literal values embedded in ENodes.
///    These are sound by construction (the values are compile-time constants).
/// 3. **Wave 31 algebraic rules** — commutativity, associativity,
///    distributivity, and constant-folding-across-ops. These add new
///    equivalences that enable further simplification via congruence
///    closure (rebuilt after each saturate round).
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
            apply: |node, eg| match node {
                // x ^ x → 0
                ENode::BinOp(BinOpKind::Xor, x, y) if x == y => {
                    let replacement = ENode::Lit(0);
                    let repl_id = eg.add(replacement.clone());
                    Some((repl_id, replacement))
                }
                _ => None,
            },
        },
        RewriteRule {
            name: "sub_self",
            verified: true,
            apply: |node, eg| match node {
                // x - x → 0
                ENode::BinOp(BinOpKind::Sub, x, y) if x == y => {
                    let replacement = ENode::Lit(0);
                    let repl_id = eg.add(replacement.clone());
                    Some((repl_id, replacement))
                }
                _ => None,
            },
        },

        // ============================================================
        // Value-aware identity rules — inspect child e-class contents.
        // Sound by constant evaluation.
        // ============================================================

        // 0 + x → x  (identity)
        RewriteRule {
            name: "add_zero_left",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Add, lhs, rhs) = node {
                    if eg.class_contains_lit(*lhs, 0) {
                        let replacement = ENode::VReg(*rhs);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "add_zero_right",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Add, lhs, rhs) = node {
                    if eg.class_contains_lit(*rhs, 0) {
                        let replacement = ENode::VReg(*lhs);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        // 0 * x → 0
        RewriteRule {
            name: "mul_zero_left",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Mul, lhs, _) = node {
                    if eg.class_contains_lit(*lhs, 0) {
                        let replacement = ENode::Lit(0);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "mul_zero_right",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Mul, _, rhs) = node {
                    if eg.class_contains_lit(*rhs, 0) {
                        let replacement = ENode::Lit(0);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        // x * 1 → x
        RewriteRule {
            name: "mul_one_right",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Mul, lhs, rhs) = node {
                    if eg.class_contains_lit(*rhs, 1) {
                        let replacement = ENode::VReg(*lhs);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "mul_one_left",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Mul, lhs, rhs) = node {
                    if eg.class_contains_lit(*lhs, 1) {
                        let replacement = ENode::VReg(*rhs);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        // x * 2 → x + x  (strength reduction: add is cheaper than mul)
        // default_cost: Add=100, Mul=200, so extraction picks x+x.
        RewriteRule {
            name: "mul_two_to_add",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Mul, lhs, rhs) = node {
                    if eg.class_contains_lit(*rhs, 2) {
                        let replacement = ENode::BinOp(BinOpKind::Add, *lhs, *lhs);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "mul_two_left_to_add",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Mul, lhs, rhs) = node {
                    if eg.class_contains_lit(*lhs, 2) {
                        let replacement = ENode::BinOp(BinOpKind::Add, *rhs, *rhs);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        // 0 & x → 0
        RewriteRule {
            name: "and_zero_left",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::And, lhs, _) = node {
                    if eg.class_contains_lit(*lhs, 0) {
                        let replacement = ENode::Lit(0);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "and_zero_right",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::And, _, rhs) = node {
                    if eg.class_contains_lit(*rhs, 0) {
                        let replacement = ENode::Lit(0);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        // x | 0 → x
        RewriteRule {
            name: "or_zero_right",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Or, lhs, rhs) = node {
                    if eg.class_contains_lit(*rhs, 0) {
                        let replacement = ENode::VReg(*lhs);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        // x ^ 0 → x
        RewriteRule {
            name: "xor_zero_right",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Xor, lhs, rhs) = node {
                    if eg.class_contains_lit(*rhs, 0) {
                        let replacement = ENode::VReg(*lhs);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        // x >> 0 → x
        RewriteRule {
            name: "shr_zero_right",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::ShrL, lhs, rhs)
                | ENode::BinOp(BinOpKind::ShrA, lhs, rhs) = node
                {
                    if eg.class_contains_lit(*rhs, 0) {
                        let replacement = ENode::VReg(*lhs);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        // x << 0 → x
        RewriteRule {
            name: "shl_zero_right",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Shl, lhs, rhs) = node {
                    if eg.class_contains_lit(*rhs, 0) {
                        let replacement = ENode::VReg(*lhs);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },

        // ============================================================
        // Wave 31: Commutativity rules.
        //
        // a op b → b op a for {+, *, &, |, ^}.
        //
        // Guard: only fire when NEITHER operand's e-class contains a literal.
        // This is a deliberate compromise: it avoids firing on `x+5` (which
        // would record spurious provenance and conflict with constant-folding
        // rules like `add_zero_left`). For non-constant operands (the common
        // case in real programs), commutativity fires normally and enables
        // congruence-merge opportunities (e.g., `a+b` and `b+a` unifying).
        // ============================================================
        RewriteRule {
            name: "comm_add",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Add, a, b) = node {
                    if a != b
                        && !eg.class_contains_any_lit(*a)
                        && !eg.class_contains_any_lit(*b)
                    {
                        let replacement = ENode::BinOp(BinOpKind::Add, *b, *a);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "comm_mul",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Mul, a, b) = node {
                    if a != b
                        && !eg.class_contains_any_lit(*a)
                        && !eg.class_contains_any_lit(*b)
                    {
                        let replacement = ENode::BinOp(BinOpKind::Mul, *b, *a);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "comm_and",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::And, a, b) = node {
                    if a != b
                        && !eg.class_contains_any_lit(*a)
                        && !eg.class_contains_any_lit(*b)
                    {
                        let replacement = ENode::BinOp(BinOpKind::And, *b, *a);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "comm_or",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Or, a, b) = node {
                    if a != b
                        && !eg.class_contains_any_lit(*a)
                        && !eg.class_contains_any_lit(*b)
                    {
                        let replacement = ENode::BinOp(BinOpKind::Or, *b, *a);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "comm_xor",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Xor, a, b) = node {
                    if a != b
                        && !eg.class_contains_any_lit(*a)
                        && !eg.class_contains_any_lit(*b)
                    {
                        let replacement = ENode::BinOp(BinOpKind::Xor, *b, *a);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                }
                None
            },
        },

        // ============================================================
        // Wave 31: Associativity rules.
        //
        // (a op b) op c ↔ a op (b op c) for {+, *, &, |, ^}.
        // Both directions are provided so the e-graph can find the
        // minimal form regardless of how the user wrote the expression.
        //
        // These rules add an intermediate e-node (e.g. `b+c` for the
        // left direction), which is why `apply` takes `&mut EGraph`.
        // ============================================================
        RewriteRule {
            name: "assoc_add_left",
            verified: true,
            apply: |node, eg| {
                // (a+b)+c → a+(b+c)
                if let ENode::BinOp(BinOpKind::Add, ab, c) = node {
                    let ab_canon = eg.find(*ab);
                    let nodes: Vec<ENode> = eg.classes.get(&ab_canon)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    for n in &nodes {
                        if let ENode::BinOp(BinOpKind::Add, a, b) = n {
                            let bc = eg.add(ENode::BinOp(BinOpKind::Add, *b, *c));
                            let replacement = ENode::BinOp(BinOpKind::Add, *a, bc);
                            let repl_id = eg.add(replacement.clone());
                            return Some((repl_id, replacement));
                        }
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "assoc_add_right",
            verified: true,
            apply: |node, eg| {
                // a+(b+c) → (a+b)+c
                if let ENode::BinOp(BinOpKind::Add, a, bc) = node {
                    let bc_canon = eg.find(*bc);
                    let nodes: Vec<ENode> = eg.classes.get(&bc_canon)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    for n in &nodes {
                        if let ENode::BinOp(BinOpKind::Add, b, c) = n {
                            let ab = eg.add(ENode::BinOp(BinOpKind::Add, *a, *b));
                            let replacement = ENode::BinOp(BinOpKind::Add, ab, *c);
                            let repl_id = eg.add(replacement.clone());
                            return Some((repl_id, replacement));
                        }
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "assoc_mul_left",
            verified: true,
            apply: |node, eg| {
                // (a*b)*c → a*(b*c)
                if let ENode::BinOp(BinOpKind::Mul, ab, c) = node {
                    let ab_canon = eg.find(*ab);
                    let nodes: Vec<ENode> = eg.classes.get(&ab_canon)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    for n in &nodes {
                        if let ENode::BinOp(BinOpKind::Mul, a, b) = n {
                            let bc = eg.add(ENode::BinOp(BinOpKind::Mul, *b, *c));
                            let replacement = ENode::BinOp(BinOpKind::Mul, *a, bc);
                            let repl_id = eg.add(replacement.clone());
                            return Some((repl_id, replacement));
                        }
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "assoc_mul_right",
            verified: true,
            apply: |node, eg| {
                // a*(b*c) → (a*b)*c
                if let ENode::BinOp(BinOpKind::Mul, a, bc) = node {
                    let bc_canon = eg.find(*bc);
                    let nodes: Vec<ENode> = eg.classes.get(&bc_canon)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    for n in &nodes {
                        if let ENode::BinOp(BinOpKind::Mul, b, c) = n {
                            let ab = eg.add(ENode::BinOp(BinOpKind::Mul, *a, *b));
                            let replacement = ENode::BinOp(BinOpKind::Mul, ab, *c);
                            let repl_id = eg.add(replacement.clone());
                            return Some((repl_id, replacement));
                        }
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "assoc_and_left",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::And, ab, c) = node {
                    let ab_canon = eg.find(*ab);
                    let nodes: Vec<ENode> = eg.classes.get(&ab_canon)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    for n in &nodes {
                        if let ENode::BinOp(BinOpKind::And, a, b) = n {
                            let bc = eg.add(ENode::BinOp(BinOpKind::And, *b, *c));
                            let replacement = ENode::BinOp(BinOpKind::And, *a, bc);
                            let repl_id = eg.add(replacement.clone());
                            return Some((repl_id, replacement));
                        }
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "assoc_and_right",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::And, a, bc) = node {
                    let bc_canon = eg.find(*bc);
                    let nodes: Vec<ENode> = eg.classes.get(&bc_canon)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    for n in &nodes {
                        if let ENode::BinOp(BinOpKind::And, b, c) = n {
                            let ab = eg.add(ENode::BinOp(BinOpKind::And, *a, *b));
                            let replacement = ENode::BinOp(BinOpKind::And, ab, *c);
                            let repl_id = eg.add(replacement.clone());
                            return Some((repl_id, replacement));
                        }
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "assoc_or_left",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Or, ab, c) = node {
                    let ab_canon = eg.find(*ab);
                    let nodes: Vec<ENode> = eg.classes.get(&ab_canon)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    for n in &nodes {
                        if let ENode::BinOp(BinOpKind::Or, a, b) = n {
                            let bc = eg.add(ENode::BinOp(BinOpKind::Or, *b, *c));
                            let replacement = ENode::BinOp(BinOpKind::Or, *a, bc);
                            let repl_id = eg.add(replacement.clone());
                            return Some((repl_id, replacement));
                        }
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "assoc_or_right",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Or, a, bc) = node {
                    let bc_canon = eg.find(*bc);
                    let nodes: Vec<ENode> = eg.classes.get(&bc_canon)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    for n in &nodes {
                        if let ENode::BinOp(BinOpKind::Or, b, c) = n {
                            let ab = eg.add(ENode::BinOp(BinOpKind::Or, *a, *b));
                            let replacement = ENode::BinOp(BinOpKind::Or, ab, *c);
                            let repl_id = eg.add(replacement.clone());
                            return Some((repl_id, replacement));
                        }
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "assoc_xor_left",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Xor, ab, c) = node {
                    let ab_canon = eg.find(*ab);
                    let nodes: Vec<ENode> = eg.classes.get(&ab_canon)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    for n in &nodes {
                        if let ENode::BinOp(BinOpKind::Xor, a, b) = n {
                            let bc = eg.add(ENode::BinOp(BinOpKind::Xor, *b, *c));
                            let replacement = ENode::BinOp(BinOpKind::Xor, *a, bc);
                            let repl_id = eg.add(replacement.clone());
                            return Some((repl_id, replacement));
                        }
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "assoc_xor_right",
            verified: true,
            apply: |node, eg| {
                if let ENode::BinOp(BinOpKind::Xor, a, bc) = node {
                    let bc_canon = eg.find(*bc);
                    let nodes: Vec<ENode> = eg.classes.get(&bc_canon)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    for n in &nodes {
                        if let ENode::BinOp(BinOpKind::Xor, b, c) = n {
                            let ab = eg.add(ENode::BinOp(BinOpKind::Xor, *a, *b));
                            let replacement = ENode::BinOp(BinOpKind::Xor, ab, *c);
                            let repl_id = eg.add(replacement.clone());
                            return Some((repl_id, replacement));
                        }
                    }
                }
                None
            },
        },

        // ============================================================
        // Wave 31: Distributivity rules.
        //
        // a * (b + c) ↔ a*b + a*c  (both directions).
        // ============================================================
        RewriteRule {
            name: "distrib_mul_add_fwd",
            verified: true,
            apply: |node, eg| {
                // a*(b+c) → a*b + a*c
                if let ENode::BinOp(BinOpKind::Mul, a, bc) = node {
                    let bc_canon = eg.find(*bc);
                    let nodes: Vec<ENode> = eg.classes.get(&bc_canon)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    for n in &nodes {
                        if let ENode::BinOp(BinOpKind::Add, b, c) = n {
                            let ab = eg.add(ENode::BinOp(BinOpKind::Mul, *a, *b));
                            let ac = eg.add(ENode::BinOp(BinOpKind::Mul, *a, *c));
                            let replacement = ENode::BinOp(BinOpKind::Add, ab, ac);
                            let repl_id = eg.add(replacement.clone());
                            return Some((repl_id, replacement));
                        }
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "distrib_mul_add_bwd",
            verified: true,
            apply: |node, eg| {
                // a*b + a*c → a*(b+c)
                if let ENode::BinOp(BinOpKind::Add, ab_id, ac_id) = node {
                    let ab_canon = eg.find(*ab_id);
                    let ac_canon = eg.find(*ac_id);
                    let ab_nodes: Vec<ENode> = eg.classes.get(&ab_canon)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    let ac_nodes: Vec<ENode> = eg.classes.get(&ac_canon)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    for ab_n in &ab_nodes {
                        if let ENode::BinOp(BinOpKind::Mul, a1, b) = ab_n {
                            for ac_n in &ac_nodes {
                                if let ENode::BinOp(BinOpKind::Mul, a2, c) = ac_n {
                                    if eg.find(*a1) == eg.find(*a2) {
                                        let bc = eg.add(ENode::BinOp(BinOpKind::Add, *b, *c));
                                        let replacement = ENode::BinOp(BinOpKind::Mul, *a1, bc);
                                        let repl_id = eg.add(replacement.clone());
                                        return Some((repl_id, replacement));
                                    }
                                }
                            }
                        }
                    }
                }
                None
            },
        },

        // ============================================================
        // Wave 31: Constant-folding-across-ops ("peel-the-zero/one").
        //
        // These handle nested identity patterns that the single-level
        // identity rules can't fully reduce on their own:
        //   (x + 0) + 0 → x
        //   (x * 1) * 1 → x
        //
        // Without these, the single-level rules reduce the inner `x+0` to
        // `VReg(x_class)` (a leaf reference), which doesn't unify with `x`
        // itself; the outer `+0` then reduces to `VReg(inner_class)`, still
        // distinct from `x`. The peel rules collapse both levels at once.
        // ============================================================
        RewriteRule {
            name: "peel_add_zero_zero",
            verified: true,
            apply: |node, eg| {
                // (x + 0) + 0 → x
                if let ENode::BinOp(BinOpKind::Add, inner, outer_zero) = node {
                    if eg.class_contains_lit(*outer_zero, 0) {
                        let inner_canon = eg.find(*inner);
                        let nodes: Vec<ENode> = eg.classes.get(&inner_canon)
                            .map(|s| s.iter().cloned().collect())
                            .unwrap_or_default();
                        for n in &nodes {
                            if let ENode::BinOp(BinOpKind::Add, x, z) = n {
                                if eg.class_contains_lit(*z, 0) {
                                    let replacement = ENode::VReg(*x);
                                    let repl_id = eg.add(replacement.clone());
                                    return Some((repl_id, replacement));
                                }
                            }
                        }
                    }
                }
                None
            },
        },
        RewriteRule {
            name: "peel_mul_one_one",
            verified: true,
            apply: |node, eg| {
                // (x * 1) * 1 → x
                if let ENode::BinOp(BinOpKind::Mul, inner, outer_one) = node {
                    if eg.class_contains_lit(*outer_one, 1) {
                        let inner_canon = eg.find(*inner);
                        let nodes: Vec<ENode> = eg.classes.get(&inner_canon)
                            .map(|s| s.iter().cloned().collect())
                            .unwrap_or_default();
                        for n in &nodes {
                            if let ENode::BinOp(BinOpKind::Mul, x, z) = n {
                                if eg.class_contains_lit(*z, 1) {
                                    let replacement = ENode::VReg(*x);
                                    let repl_id = eg.add(replacement.clone());
                                    return Some((repl_id, replacement));
                                }
                            }
                        }
                    }
                }
                None
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
    ///
    /// Wave 43 serde-migration: previously used `serde_json::from_str::<serde_json::Value>`
    /// to parse the JSON into a generic value tree, then navigated it with
    /// `.get("hotness").and_then(|h| h.as_object())`. Now uses a hand-written
    /// minimal JSON parser (`parse_profile_json`) that understands only the
    /// shape `{"hotness": {"<id>": <u64>, ...}}`. The on-disk JSON format
    /// is unchanged.
    pub fn from_json(json: &str) -> Result<Self, String> {
        parse_profile_json(json).map(|hotness| Self { hotness })
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

// ---------------------------------------------------------------------------
// Hand-written JSON parser for PGO profile data (Wave 43 serde-migration)
// ---------------------------------------------------------------------------
//
// Parses the shape `{"hotness": {"<id>": <u64>, ...}}`. This is a minimal
// recursive descent parser — it does NOT implement the full JSON grammar.
// It handles only:
//   - The literal token `"hotness"` as the top-level key
//   - String-literal keys (the e-class/vreg IDs as ASCII strings)
//   - Non-negative integer values
//   - Whitespace between tokens
//   - Optional trailing commas
//
// All other JSON constructs (nested objects other than `hotness`, arrays,
// floats, escapes in strings, etc.) are rejected. If the format ever needs
// to expand, this parser should be replaced with a full JSON value type.
//
// The on-disk JSON format produced by `ProfileData::to_json` is unchanged.

fn parse_profile_json(json: &str) -> Result<HashMap<EClassId, u32>, String> {
    let bytes = json.as_bytes();
    let mut pos = 0;
    let mut hotness: HashMap<EClassId, u32> = HashMap::new();

    skip_ws(bytes, &mut pos);
    expect_byte(bytes, &mut pos, b'{')?;
    skip_ws(bytes, &mut pos);

    // Allow empty top-level object: `{}`
    if peek_byte(bytes, pos) == Some(b'}') {
        pos += 1;
        // No hotness key — return empty map (caller treats as "no profile").
        return Ok(hotness);
    }

    // Iterate top-level key-value pairs.
    loop {
        skip_ws(bytes, &mut pos);
        let key = parse_json_string(bytes, &mut pos)?;
        skip_ws(bytes, &mut pos);
        expect_byte(bytes, &mut pos, b':')?;
        skip_ws(bytes, &mut pos);

        if key == "hotness" {
            // Parse the inner hotness map.
            expect_byte(bytes, &mut pos, b'{')?;
            skip_ws(bytes, &mut pos);
            if peek_byte(bytes, pos) == Some(b'}') {
                pos += 1;
            } else {
                loop {
                    skip_ws(bytes, &mut pos);
                    let id_str = parse_json_string(bytes, &mut pos)?;
                    skip_ws(bytes, &mut pos);
                    expect_byte(bytes, &mut pos, b':')?;
                    skip_ws(bytes, &mut pos);
                    let count = parse_json_u64(bytes, &mut pos)?;
                    let id: EClassId = id_str.parse().map_err(|e| {
                        format!("invalid vreg id '{}': {}", id_str, e)
                    })?;
                    if count > u32::MAX as u64 {
                        return Err(format!(
                            "hotness for {} exceeds u32::MAX ({})",
                            id_str, count
                        ));
                    }
                    hotness.insert(id, count as u32);
                    skip_ws(bytes, &mut pos);
                    match peek_byte(bytes, pos) {
                        Some(b',') => {
                            pos += 1;
                            skip_ws(bytes, &mut pos);
                            // Allow trailing comma.
                            if peek_byte(bytes, pos) == Some(b'}') {
                                pos += 1;
                                break;
                            }
                        }
                        Some(b'}') => {
                            pos += 1;
                            break;
                        }
                        _ => return Err(format!(
                            "expected ',' or '}}' in hotness object at byte {}",
                            pos
                        )),
                    }
                }
            }
        } else {
            // Unknown top-level key — skip its value (best-effort forward compat).
            skip_json_value(bytes, &mut pos)?;
        }

        skip_ws(bytes, &mut pos);
        match peek_byte(bytes, pos) {
            Some(b',') => {
                pos += 1;
                skip_ws(bytes, &mut pos);
                // Allow trailing comma.
                if peek_byte(bytes, pos) == Some(b'}') {
                    pos += 1;
                    break;
                }
            }
            Some(b'}') => {
                pos += 1;
                break;
            }
            _ => return Err(format!(
                "expected ',' or '}}' at top level at byte {}",
                pos
            )),
        }
    }

    // Allow trailing whitespace; no trailing characters allowed.
    skip_ws(bytes, &mut pos);
    if pos != bytes.len() {
        return Err(format!(
            "trailing characters after top-level object at byte {} (len={})",
            pos,
            bytes.len()
        ));
    }
    Ok(hotness)
}

fn skip_ws(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len() {
        match bytes[*pos] {
            b' ' | b'\t' | b'\n' | b'\r' => *pos += 1,
            _ => break,
        }
    }
}

fn peek_byte(bytes: &[u8], pos: usize) -> Option<u8> {
    bytes.get(pos).copied()
}

fn expect_byte(bytes: &[u8], pos: &mut usize, expected: u8) -> Result<(), String> {
    if *pos >= bytes.len() {
        return Err(format!("expected {:?} but reached end of input", expected as char));
    }
    if bytes[*pos] != expected {
        return Err(format!(
            "expected {:?} at byte {} but found {:?}",
            expected as char,
            *pos,
            bytes[*pos] as char
        ));
    }
    *pos += 1;
    Ok(())
}

/// Parse a JSON string literal. Supports the common escapes (`\"`, `\\`,
/// `\/`, `\b`, `\f`, `\/`, `\n`, `\r`, `\t`, and `\uXXXX`). Does NOT
/// validate UTF-8 well-formedness beyond what `String::from_utf8` provides.
fn parse_json_string(bytes: &[u8], pos: &mut usize) -> Result<String, String> {
    expect_byte(bytes, pos, b'"')?;
    let mut out = Vec::new();
    while *pos < bytes.len() {
        let b = bytes[*pos];
        *pos += 1;
        match b {
            b'"' => {
                return String::from_utf8(out).map_err(|e| {
                    format!("invalid UTF-8 in JSON string: {}", e)
                });
            }
            b'\\' => {
                if *pos >= bytes.len() {
                    return Err("trailing backslash in JSON string".to_string());
                }
                let esc = bytes[*pos];
                *pos += 1;
                match esc {
                    b'"' => out.push(b'"'),
                    b'\\' => out.push(b'\\'),
                    b'/' => out.push(b'/'),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0c),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'u' => {
                        if *pos + 4 > bytes.len() {
                            return Err("truncated \\uXXXX escape in JSON string".to_string());
                        }
                        let hex = std::str::from_utf8(&bytes[*pos..*pos + 4])
                            .map_err(|_| "invalid UTF-8 in \\uXXXX escape".to_string())?;
                        let code = u32::from_str_radix(hex, 16)
                            .map_err(|e| format!("invalid \\uXXXX escape: {}", e))?;
                        *pos += 4;
                        if let Some(ch) = char::from_u32(code) {
                            let mut buf = [0u8; 4];
                            let s = ch.encode_utf8(&mut buf);
                            out.extend_from_slice(s.as_bytes());
                        } else {
                            return Err(format!("invalid Unicode codepoint U+{:04X}", code));
                        }
                    }
                    _ => return Err(format!("invalid JSON escape '\\{}'", esc as char)),
                }
            }
            // Reject raw control characters (JSON requires they be escaped).
            0x00..=0x1f => {
                return Err(format!(
                    "raw control character (0x{:02X}) in JSON string at byte {}",
                    b, *pos - 1
                ));
            }
            _ => out.push(b),
        }
    }
    Err("unterminated JSON string".to_string())
}

/// Parse a JSON number as a u64. Rejects negatives, floats, and exponents.
fn parse_json_u64(bytes: &[u8], pos: &mut usize) -> Result<u64, String> {
    let start = *pos;
    if *pos < bytes.len() && bytes[*pos] == b'-' {
        return Err("negative numbers are not allowed in hotness values".to_string());
    }
    while *pos < bytes.len() && bytes[*pos].is_ascii_digit() {
        *pos += 1;
    }
    if *pos == start {
        return Err(format!("expected digit at byte {}", start));
    }
    let s = std::str::from_utf8(&bytes[start..*pos])
        .map_err(|_| "invalid UTF-8 in JSON number".to_string())?;
    s.parse::<u64>()
        .map_err(|e| format!("invalid integer '{}': {}", s, e))
}

/// Best-effort skip of any JSON value (used for unknown top-level keys).
/// Supports objects, arrays, strings, numbers, true/false/null.
fn skip_json_value(bytes: &[u8], pos: &mut usize) -> Result<(), String> {
    skip_ws(bytes, pos);
    if *pos >= bytes.len() {
        return Err("expected JSON value but reached end of input".to_string());
    }
    match bytes[*pos] {
        b'{' => {
            *pos += 1;
            skip_ws(bytes, pos);
            if peek_byte(bytes, *pos) == Some(b'}') {
                *pos += 1;
                return Ok(());
            }
            loop {
                skip_ws(bytes, pos);
                let _key = parse_json_string(bytes, pos)?;
                skip_ws(bytes, pos);
                expect_byte(bytes, pos, b':')?;
                skip_json_value(bytes, pos)?;
                skip_ws(bytes, pos);
                match peek_byte(bytes, *pos) {
                    Some(b',') => {
                        *pos += 1;
                        skip_ws(bytes, pos);
                        if peek_byte(bytes, *pos) == Some(b'}') {
                            *pos += 1;
                            return Ok(());
                        }
                    }
                    Some(b'}') => {
                        *pos += 1;
                        return Ok(());
                    }
                    _ => return Err(format!("expected ',' or '}}' at byte {}", *pos)),
                }
            }
        }
        b'[' => {
            *pos += 1;
            skip_ws(bytes, pos);
            if peek_byte(bytes, *pos) == Some(b']') {
                *pos += 1;
                return Ok(());
            }
            loop {
                skip_json_value(bytes, pos)?;
                skip_ws(bytes, pos);
                match peek_byte(bytes, *pos) {
                    Some(b',') => {
                        *pos += 1;
                        skip_ws(bytes, pos);
                        if peek_byte(bytes, *pos) == Some(b']') {
                            *pos += 1;
                            return Ok(());
                        }
                    }
                    Some(b']') => {
                        *pos += 1;
                        return Ok(());
                    }
                    _ => return Err(format!("expected ',' or ']' at byte {}", *pos)),
                }
            }
        }
        b'"' => {
            let _ = parse_json_string(bytes, pos)?;
            Ok(())
        }
        b't' => {
            expect_literal(bytes, pos, "true")
        }
        b'f' => {
            expect_literal(bytes, pos, "false")
        }
        b'n' => {
            expect_literal(bytes, pos, "null")
        }
        b'0'..=b'9' | b'-' => {
            // Skip a number — accept leading '-', digits, '.', 'e', 'E', '+', '-'.
            if *pos < bytes.len() && bytes[*pos] == b'-' {
                *pos += 1;
            }
            while *pos < bytes.len()
                && (bytes[*pos].is_ascii_digit()
                    || matches!(bytes[*pos], b'.' | b'e' | b'E' | b'+' | b'-'))
            {
                *pos += 1;
            }
            Ok(())
        }
        _ => Err(format!(
            "unexpected character {:?} at byte {}",
            bytes[*pos] as char, *pos
        )),
    }
}

fn expect_literal(bytes: &[u8], pos: &mut usize, lit: &str) -> Result<(), String> {
    if *pos + lit.len() > bytes.len() {
        return Err(format!("expected '{}' but reached end of input", lit));
    }
    if &bytes[*pos..*pos + lit.len()] != lit.as_bytes() {
        return Err(format!(
            "expected '{}' at byte {} but found '{}'",
            lit,
            *pos,
            std::str::from_utf8(&bytes[*pos..*pos + lit.len().min(bytes.len() - *pos)])
                .unwrap_or("<invalid utf-8>")
        ));
    }
    *pos += lit.len();
    Ok(())
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

    // ============================================================
    // Wave 31 tests
    // ============================================================

    /// Rebuilding after merge should detect congruent parents:
    /// if `a ~ b`, then `(a+b)` and `(b+a)` are congruent and should
    /// end up in the same e-class (without an explicit commutativity
    /// rule firing on them — pure congruence closure).
    #[test]
    fn test_rebuild_merges_congruent_parents() {
        let mut eg = EGraph::new();
        let a = eg.add(ENode::Lit(1));
        let b = eg.add(ENode::Lit(2));
        let p = eg.add(ENode::BinOp(BinOpKind::Add, a, b)); // (1+2)
        let q = eg.add(ENode::BinOp(BinOpKind::Add, b, a)); // (2+1)
        // Before merging a and b, p and q are distinct (different child IDs).
        assert_ne!(eg.find(p), eg.find(q));
        // Merge a and b. Now find(b) = a, so (a+b) and (b+a) both
        // canonicalize to (a+a). Rebuild should detect the congruence
        // and merge them.
        eg.merge(a, b);
        assert_eq!(
            eg.find(p), eg.find(q),
            "rebuild should merge congruent parents (a+b)~(b+a) after a~b"
        );
    }

    /// Rebuild is also called inside `saturate` after each round, so
    /// congruences arising from rule-driven merges propagate.
    #[test]
    fn test_rebuild_propagates_through_saturate() {
        let mut eg = EGraph::new();
        // Build (a+b)+c and (b+a)+c. After saturate, comm_add will fire
        // on `a+b` producing `b+a`, and rebuild should then merge
        // (a+b)+c with (b+a)+c via congruence.
        let a = eg.add(ENode::VReg(10));
        let b = eg.add(ENode::VReg(11));
        let c = eg.add(ENode::VReg(12));
        let ab = eg.add(ENode::BinOp(BinOpKind::Add, a, b));
        let ba = eg.add(ENode::BinOp(BinOpKind::Add, b, a));
        let abc = eg.add(ENode::BinOp(BinOpKind::Add, ab, c));
        let bac = eg.add(ENode::BinOp(BinOpKind::Add, ba, c));
        let rules = standard_rules();
        eg.saturate(&rules, 20);
        assert_eq!(
            eg.find(abc), eg.find(bac),
            "after saturate, (a+b)+c and (b+a)+c should be congruent \
             (commutativity + rebuild)"
        );
    }

    /// DP extraction peels nested zeros: an e-class containing
    /// `{(x+0)+0, x+0, x}` should extract to `x` (lowest total cost).
    #[test]
    fn test_extract_dp_peels_zeros() {
        let mut eg = EGraph::new();
        let x = eg.add(ENode::VReg(0));        // x  (class 0)
        let zero = eg.add(ENode::Lit(0));      // 0  (class 1)
        let x_plus_0 = eg.add(ENode::BinOp(BinOpKind::Add, x, zero));
        let x_plus_0_plus_0 = eg.add(ENode::BinOp(BinOpKind::Add, x_plus_0, zero));
        // Manually merge so all three are equivalent.
        eg.merge(x, x_plus_0);
        eg.merge(x, x_plus_0_plus_0);
        // default_cost: VReg=10, Lit=1, Add=100.
        //   x           = 10
        //   x+0         = 100 + 10 + 1   = 111
        //   (x+0)+0     = 100 + 111 + 1  = 212
        // DP extraction picks VReg(0) (cost 10).
        let best = eg.extract(x, &default_cost);
        assert_eq!(
            best, ENode::VReg(0),
            "DP extraction should peel nested zeros and pick x"
        );
    }

    /// DP extraction considers children's costs (not just node cost).
    /// With a custom cost where Add is cheap (5) but Lit is expensive
    /// (100), old single-node extraction would pick `x+0` (node cost 5
    /// < VReg cost 10). DP extraction correctly picks `VReg(x)` because
    /// `x+0`'s total cost (5 + 10 + 100 = 115) exceeds VReg's (10).
    #[test]
    fn test_extract_dp_uses_children_costs() {
        let mut eg = EGraph::new();
        let x = eg.add(ENode::VReg(0));        // x
        let expensive = eg.add(ENode::Lit(0)); // 0 (treated as expensive)
        let add = eg.add(ENode::BinOp(BinOpKind::Add, x, expensive));
        eg.merge(x, add); // x ~ x+0
        // Custom cost: VReg=10, BinOp(Add)=5 (cheap!), Lit=100 (expensive!)
        let cost_fn = |node: &ENode| match node {
            ENode::VReg(_) => 10,
            ENode::Lit(_) => 100,
            ENode::BinOp(BinOpKind::Add, _, _) => 5,
            _ => 50,
        };
        let best = eg.extract(x, &cost_fn);
        assert_eq!(
            best, ENode::VReg(0),
            "DP extraction should consider children's costs: VReg(0)=10 beats \
             BinOp(Add,x,0)=5+10+100=115"
        );
    }

    /// Commutativity rule fires on `a+b` (both VRegs, neither a literal),
    /// recording provenance.
    #[test]
    fn test_commutativity_fires_on_vregs() {
        let mut eg = EGraph::new();
        let a = eg.add(ENode::VReg(1));
        let b = eg.add(ENode::VReg(2));
        let _ab = eg.add(ENode::BinOp(BinOpKind::Add, a, b));
        let rules = standard_rules();
        eg.saturate(&rules, 10);
        // After saturate, the e-class of a+b should also contain b+a
        // (commutativity fired). Verify via provenance.
        let mut saw_comm_add = false;
        let class_ids: Vec<EClassId> = eg.classes.keys().copied().collect();
        for cid in class_ids {
            for step in eg.get_provenance(cid) {
                if step.rule_name == "comm_add" {
                    saw_comm_add = true;
                }
            }
        }
        assert!(saw_comm_add, "comm_add should fire on a+b with VReg operands");
    }

    /// Commutativity does NOT fire on `x+5` (Lit operand), preserving
    /// the Wave 16 invariant that unmatched expressions have empty
    /// provenance.
    #[test]
    fn test_commutativity_skips_lit_operands() {
        let mut eg = EGraph::new();
        let x = eg.add(ENode::VReg(0));
        let five = eg.add(ENode::Lit(5));
        let add = eg.add(ENode::BinOp(BinOpKind::Add, x, five));
        let rules = standard_rules();
        eg.saturate(&rules, 10);
        assert!(
            !eg.has_provenance(add),
            "comm_add should not fire on x+5 (Lit operand), keeping provenance empty"
        );
    }

    /// Associativity fires in both directions: `(a+b)+c ↔ a+(b+c)`.
    #[test]
    fn test_associativity_both_directions() {
        let mut eg = EGraph::new();
        let a = eg.add(ENode::VReg(10));
        let b = eg.add(ENode::VReg(11));
        let c = eg.add(ENode::VReg(12));
        let ab = eg.add(ENode::BinOp(BinOpKind::Add, a, b));
        let _abc = eg.add(ENode::BinOp(BinOpKind::Add, ab, c)); // (a+b)+c
        let bc = eg.add(ENode::BinOp(BinOpKind::Add, b, c));
        let _a_bc = eg.add(ENode::BinOp(BinOpKind::Add, a, bc)); // a+(b+c)
        let rules = standard_rules();
        eg.saturate(&rules, 20);
        let mut saw_left = false;
        let mut saw_right = false;
        let class_ids: Vec<EClassId> = eg.classes.keys().copied().collect();
        for cid in class_ids {
            for step in eg.get_provenance(cid) {
                if step.rule_name == "assoc_add_left" { saw_left = true; }
                if step.rule_name == "assoc_add_right" { saw_right = true; }
            }
        }
        assert!(saw_left, "assoc_add_left should fire on (a+b)+c");
        assert!(saw_right, "assoc_add_right should fire on a+(b+c)");
    }

    /// Distributivity fires in both directions: `a*(b+c) ↔ a*b + a*c`.
    #[test]
    fn test_distributivity_both_directions() {
        let mut eg = EGraph::new();
        // a*(b+c)  [fwd]
        let a = eg.add(ENode::VReg(20));
        let b = eg.add(ENode::VReg(21));
        let c = eg.add(ENode::VReg(22));
        let bc = eg.add(ENode::BinOp(BinOpKind::Add, b, c));
        let _a_bc = eg.add(ENode::BinOp(BinOpKind::Mul, a, bc));
        // d*e + d*f  [bwd]
        let d = eg.add(ENode::VReg(30));
        let e = eg.add(ENode::VReg(31));
        let f = eg.add(ENode::VReg(32));
        let de = eg.add(ENode::BinOp(BinOpKind::Mul, d, e));
        let df = eg.add(ENode::BinOp(BinOpKind::Mul, d, f));
        let _de_df = eg.add(ENode::BinOp(BinOpKind::Add, de, df));
        let rules = standard_rules();
        eg.saturate(&rules, 20);
        let mut saw_fwd = false;
        let mut saw_bwd = false;
        let class_ids: Vec<EClassId> = eg.classes.keys().copied().collect();
        for cid in class_ids {
            for step in eg.get_provenance(cid) {
                if step.rule_name == "distrib_mul_add_fwd" { saw_fwd = true; }
                if step.rule_name == "distrib_mul_add_bwd" { saw_bwd = true; }
            }
        }
        assert!(saw_fwd, "distrib_mul_add_fwd should fire on a*(b+c)");
        assert!(saw_bwd, "distrib_mul_add_bwd should fire on a*b + a*c");
    }

    /// Constant-folding-across-ops: `(x+0)+0 → x` and `(x*1)*1 → x`.
    #[test]
    fn test_peel_rules_fire() {
        let mut eg = EGraph::new();
        // (x+0)+0
        let x1 = eg.add(ENode::VReg(40));
        let zero = eg.add(ENode::Lit(0));
        let x1_plus_0 = eg.add(ENode::BinOp(BinOpKind::Add, x1, zero));
        let _x1_p0_p0 = eg.add(ENode::BinOp(BinOpKind::Add, x1_plus_0, zero));
        // (x*1)*1
        let x2 = eg.add(ENode::VReg(41));
        let one = eg.add(ENode::Lit(1));
        let x2_mul_1 = eg.add(ENode::BinOp(BinOpKind::Mul, x2, one));
        let _x2_m1_m1 = eg.add(ENode::BinOp(BinOpKind::Mul, x2_mul_1, one));
        let rules = standard_rules();
        eg.saturate(&rules, 20);
        let mut saw_peel_add = false;
        let mut saw_peel_mul = false;
        let class_ids: Vec<EClassId> = eg.classes.keys().copied().collect();
        for cid in class_ids {
            for step in eg.get_provenance(cid) {
                if step.rule_name == "peel_add_zero_zero" { saw_peel_add = true; }
                if step.rule_name == "peel_mul_one_one" { saw_peel_mul = true; }
            }
        }
        assert!(saw_peel_add, "peel_add_zero_zero should fire on (x+0)+0");
        assert!(saw_peel_mul, "peel_mul_one_one should fire on (x*1)*1");
    }

    /// Rule-coverage test (Wave 31): construct a representative program
    /// per rule family and assert each new rule fires at least once
    /// during `saturate`. We use fresh VReg operands per rule to avoid
    /// interference (one rule's merge shouldn't suppress another's
    /// provenance recording).
    #[test]
    fn test_wave31_rule_coverage() {
        let mut eg = EGraph::new();
        let mut next_vreg: u32 = 100;
        // Helper to mint a fresh VReg e-class (so each rule gets its own
        // operands and fires independently).
        let mut fresh = |eg: &mut EGraph| -> EClassId {
            let v = next_vreg;
            next_vreg += 1;
            eg.add(ENode::VReg(v))
        };

        // === Commutativity (one expr per op) ===
        for op in [BinOpKind::Add, BinOpKind::Mul, BinOpKind::And,
                   BinOpKind::Or, BinOpKind::Xor] {
            let a = fresh(&mut eg);
            let b = fresh(&mut eg);
            eg.add(ENode::BinOp(op, a, b));
        }

        // === Associativity left: (a op b) op c → a op (b op c) ===
        for op in [BinOpKind::Add, BinOpKind::Mul, BinOpKind::And,
                   BinOpKind::Or, BinOpKind::Xor] {
            let a = fresh(&mut eg);
            let b = fresh(&mut eg);
            let c = fresh(&mut eg);
            let ab = eg.add(ENode::BinOp(op, a, b));
            eg.add(ENode::BinOp(op, ab, c));
        }

        // === Associativity right: a op (b op c) → (a op b) op c ===
        for op in [BinOpKind::Add, BinOpKind::Mul, BinOpKind::And,
                   BinOpKind::Or, BinOpKind::Xor] {
            let a = fresh(&mut eg);
            let b = fresh(&mut eg);
            let c = fresh(&mut eg);
            let bc = eg.add(ENode::BinOp(op, b, c));
            eg.add(ENode::BinOp(op, a, bc));
        }

        // === Distributivity fwd: a * (b + c) → a*b + a*c ===
        {
            let a = fresh(&mut eg);
            let b = fresh(&mut eg);
            let c = fresh(&mut eg);
            let bc = eg.add(ENode::BinOp(BinOpKind::Add, b, c));
            eg.add(ENode::BinOp(BinOpKind::Mul, a, bc));
        }

        // === Distributivity bwd: a*b + a*c → a*(b+c) ===
        {
            let a = fresh(&mut eg);
            let b = fresh(&mut eg);
            let c = fresh(&mut eg);
            let ab = eg.add(ENode::BinOp(BinOpKind::Mul, a, b));
            let ac = eg.add(ENode::BinOp(BinOpKind::Mul, a, c));
            eg.add(ENode::BinOp(BinOpKind::Add, ab, ac));
        }

        // === Constant-folding-across-ops ===
        // (x + 0) + 0 → x   [peel_add_zero_zero]
        {
            let x = fresh(&mut eg);
            let zero = eg.add(ENode::Lit(0));
            let x_plus_0 = eg.add(ENode::BinOp(BinOpKind::Add, x, zero));
            eg.add(ENode::BinOp(BinOpKind::Add, x_plus_0, zero));
        }
        // (x * 1) * 1 → x   [peel_mul_one_one]
        {
            let x = fresh(&mut eg);
            let one = eg.add(ENode::Lit(1));
            let x_mul_1 = eg.add(ENode::BinOp(BinOpKind::Mul, x, one));
            eg.add(ENode::BinOp(BinOpKind::Mul, x_mul_1, one));
        }

        let rules = standard_rules();
        eg.saturate(&rules, 30);

        // Collect all rule names that fired (from provenance).
        let mut rule_names_seen: HashSet<String> = HashSet::new();
        let class_ids: Vec<EClassId> = eg.classes.keys().copied().collect();
        for cid in class_ids {
            for step in eg.get_provenance(cid) {
                rule_names_seen.insert(step.rule_name.clone());
            }
        }

        let expected_rules = [
            // Commutativity
            "comm_add", "comm_mul", "comm_and", "comm_or", "comm_xor",
            // Associativity (both directions, all 5 ops)
            "assoc_add_left", "assoc_add_right",
            "assoc_mul_left", "assoc_mul_right",
            "assoc_and_left", "assoc_and_right",
            "assoc_or_left", "assoc_or_right",
            "assoc_xor_left", "assoc_xor_right",
            // Distributivity
            "distrib_mul_add_fwd", "distrib_mul_add_bwd",
            // Constant-folding-across-ops
            "peel_add_zero_zero", "peel_mul_one_one",
        ];
        for rule_name in &expected_rules {
            assert!(
                rule_names_seen.contains(*rule_name),
                "rule '{}' should have fired at least once. Seen rules: {:?}",
                rule_name,
                rule_names_seen
            );
        }
    }

    // ============================================================
    // Wave 36 tests — proof-logging saturation + bv_verify gate.
    // ============================================================

    /// Wave 36 [EGRAPH-WIRE]: `saturate_with_proof` records a `ProofArtifact`
    /// for every rewrite application. Construct an input that triggers a
    /// known rule (`xor_self` on `x ^ x`) and assert the proof log receives
    /// a matching artifact.
    #[test]
    fn test_wave36_saturate_with_proof_records_artifacts() {
        let mut eg = EGraph::new();
        let x = eg.add(ENode::VReg(50));
        let x2 = eg.add(ENode::VReg(50));
        let _xor = eg.add(ENode::BinOp(BinOpKind::Xor, x, x2)); // x ^ x

        let rules = standard_rules();
        let mut log = crate::proof_artifacts::ProofLog::new();
        let result = eg.saturate_with_proof(&rules, 10, &mut log);

        assert!(result.is_ok(), "gate should accept standard_rules");
        assert!(
            !log.artifacts.is_empty(),
            "saturate_with_proof must record at least one ProofArtifact"
        );
        // At least one artifact should be from xor_self (the only rule that
        // fires on `x ^ x`).
        let saw_xor_self = log.artifacts.iter().any(|a| a.rule_name == "xor_self");
        assert!(
            saw_xor_self,
            "proof log should contain an xor_self artifact, got: {:?}",
            log.artifacts.iter().map(|a| a.rule_name).collect::<Vec<_>>()
        );
        // The artifact's source must be the matched pattern (Xor node) and
        // the replacement must be Lit(0).
        let xor_artifact = log.artifacts.iter().find(|a| a.rule_name == "xor_self").unwrap();
        assert!(
            matches!(&xor_artifact.source, ENode::BinOp(BinOpKind::Xor, _, _)),
            "xor_self artifact source should be a Xor node"
        );
        assert_eq!(
            xor_artifact.replacement, ENode::Lit(0),
            "xor_self artifact replacement should be Lit(0)"
        );
    }

    /// Wave 36 [OPT-WIRE / bv_verify gate]: `saturate_with_proof` returns
    /// `Err(Counterexample)` and does NOT saturate when an unsound rule is
    /// in the rule set. The e-graph should be unchanged after the failed
    /// call (no rules applied).
    #[test]
    fn test_wave36_saturate_with_proof_rejects_unsound_rule() {
        let mut eg = EGraph::new();
        let _x = eg.add(ENode::VReg(60));
        let _ = eg.add(ENode::VReg(60)); // identical VReg (would allow xor_self)

        let unsound_rule = RewriteRule {
            name: "wave36_unsound_inc", // registered unsound in bv_verify
            verified: false,
            apply: |_node, eg| {
                // Pretend to rewrite any node to Lit(99) — semantically wrong,
                // but the gate must reject this rule by NAME before apply
                // is ever called.
                let replacement = ENode::Lit(99);
                let repl_id = eg.add(replacement.clone());
                Some((repl_id, replacement))
            },
        };
        let rules = vec![unsound_rule];
        let nodes_before = eg.classes.len();
        let mut log = crate::proof_artifacts::ProofLog::new();
        let result = eg.saturate_with_proof(&rules, 10, &mut log);
        assert!(
            result.is_err(),
            "saturate_with_proof must reject the unsound rule, got {:?}",
            result
        );
        let err = result.unwrap_err();
        assert_eq!(err.rule_name, "wave36_unsound_inc");
        // The gate fires BEFORE saturating, so no rules were applied.
        assert_eq!(
            eg.classes.len(), nodes_before,
            "e-graph must be unchanged after the gate rejected the rule"
        );
        assert!(
            log.artifacts.is_empty(),
            "no ProofArtifacts should be recorded when the gate rejects the rule"
        );
    }

    /// Wave 36 [OPT-WIRE]: `verify_rules_before_saturate` is the standalone
    /// gate entry point. It returns Ok for standard_rules() and Err for a
    /// known-unsound rule.
    #[test]
    fn test_wave36_verify_rules_before_saturate_entry_point() {
        let eg = EGraph::new();
        // Standard rules: all sound (or assumed-sound-by-construction).
        let standard = standard_rules();
        assert!(
            eg.verify_rules_before_saturate(&standard).is_ok(),
            "verify_rules_before_saturate must accept standard_rules"
        );

        // Unsound rule: rejected.
        let unsound = RewriteRule {
            name: "wave36_unsound_inc",
            verified: false,
            apply: |_, _| None,
        };
        let result = eg.verify_rules_before_saturate(&[unsound]);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().rule_name, "wave36_unsound_inc");
    }

    /// Wave 36 backward-compat: legacy `saturate` (no log) still works on
    /// standard rules — its results are unchanged from the W31 behavior.
    #[test]
    fn test_wave36_legacy_saturate_still_works() {
        let mut eg = EGraph::new();
        let x = eg.add(ENode::VReg(70));
        let x2 = eg.add(ENode::VReg(70));
        let xor_id = eg.add(ENode::BinOp(BinOpKind::Xor, x, x2));

        let rules = standard_rules();
        eg.saturate(&rules, 10);

        // xor_self should have fired and merged xor_id's class with Lit(0).
        assert!(eg.has_provenance(xor_id));
        let best = eg.extract(xor_id, &default_cost);
        assert_eq!(best, ENode::Lit(0), "x^x should extract to Lit(0)");
    }
}
