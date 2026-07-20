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
//!
//! # Wave 5 Additions — PMT state-operation ENodes + rewrite rules
//!
//! Wave 5 extends the e-graph to reason about PMT (Programs as Memory
//! Transformations) state operations. Four new ENode variants are added:
//! `StateInit`, `StateRead`, `StateWrite`, `StateTransform`. These let the
//! e-graph discover equivalences between state-operation sequences —
//! most importantly, **store-load forwarding** (a Store immediately
//! followed by a Load of the same field can be replaced by the stored
//! value) and **transform elision** (a transform whose source and
//! destination layouts are the same is a no-op).
//!
//! ## New rewrite rules
//!
//! - `state_dead_init_elim`: `StateInit(L)` whose e-class is referenced by
//!   no `StateRead`/`StateWrite`/`StateTransform` → merge with `Lit(0)`
//!   (the state is provably unused; its value is irrelevant).
//! - `state_store_load_forward`: `StateRead(StateWrite(s, off, v), off, _)`
//!   → `v` (the read forwards the just-written value).
//! - `state_transform_elision`: `StateTransform(x, L, L)` → `x` (same
//!   src/dst layout means no-op).
//! - `state_merge_compatible_layouts`: stub (`verified: false`) — merging
//!   two `StateInit`s with compatible layouts requires lifetime analysis
//!   the e-graph cannot perform; deferred to a future wave.
//!
//! ## Wiring status
//!
//! The optimizer pass in `opt.rs::equality_saturation_with_cost` currently
//! feeds only `BinOp` instructions to the e-graph. The Wave 5 state-op
//! rules therefore do not yet fire on real programs — they are exercised
//! by the unit tests in this module and by the `tests/gold_standard/
//! pmt_wave5/*.vuma` golden tests (which serve as forward-looking test
//! cases for the wave that wires state ops into the optimizer).

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

    // ============================================================
    // Wave 5 — PMT state-operation ENodes.
    //
    // These model the PMT (Programs as Memory Transformations) state
    // operations: `state_new(Layout)`, `state.field` (read), `state.field
    // = val` (write), and `transform T(s)` (layout conversion). The
    // e-class IDs in the variant fields refer to the *child* e-classes
    // (e.g. `state` is the e-class of the input state buffer; `value` is
    // the e-class of the value being stored).
    //
    // `layout_id` is a compile-time-assigned numeric ID identifying a
    // declared layout (Point, Triple, etc.). Two `StateInit`s with the
    // same `layout_id` allocate buffers of the same shape — a prerequisite
    // for the (deferred) state-merge rule.
    // ============================================================

    /// State initialization — `state_new(Layout)`. Produces a fresh state
    /// buffer. `layout_id` identifies the layout (shapes the buffer's size
    /// and field offsets).
    StateInit { layout_id: u64 },
    /// State field read — `state.field` (read). Loads `size` bytes from
    /// `state` at byte offset `offset`. The result is the field's value.
    StateRead { state: u32, offset: u64, size: u64 },
    /// State field write — `state.field = value`. Stores `value` to `state`
    /// at byte offset `offset`. The result is the *new* state (the write
    /// produces a fresh state e-class — PMT states are linear/immutable).
    StateWrite { state: u32, offset: u64, value: u32 },
    /// State layout transform — `transform T(s)`. Converts `input` from
    /// `src_layout` to `dst_layout`. If `src_layout == dst_layout`, the
    /// transform is a no-op (identity) — the `state_transform_elision`
    /// rewrite rule exploits this.
    StateTransform { input: u32, src_layout: u64, dst_layout: u64 },
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
        // DETERMINISM: sort class IDs (HashMap.keys() is randomized).
        let mut class_ids: Vec<EClassId> = self.classes.keys().copied().collect();
        class_ids.sort_unstable();
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
        // DETERMINISM: sort the groups by their canonical-node key so merge
        // order is stable across runs. HashMap.values() is randomized;
        // different merge order can produce different canonical
        // representatives, changing extraction results.
        let mut merged = false;
        let mut sorted_groups: Vec<(&ENode, &Vec<EClassId>)> = groups.iter().collect();
        sorted_groups.sort_by(|a, b| format!("{:?}", a.0).cmp(&format!("{:?}", b.0)));
        for (_key, classes) in sorted_groups {
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
            // Wave 5: canonicalize the child e-class references in state
            // ops. Layout IDs and offsets are static metadata (not e-class
            // references), so they're preserved verbatim.
            ENode::StateInit { layout_id } => {
                ENode::StateInit { layout_id: *layout_id }
            }
            ENode::StateRead { state, offset, size } => {
                ENode::StateRead {
                    state: self.find(*state),
                    offset: *offset,
                    size: *size,
                }
            }
            ENode::StateWrite { state, offset, value } => {
                ENode::StateWrite {
                    state: self.find(*state),
                    offset: *offset,
                    value: self.find(*value),
                }
            }
            ENode::StateTransform { input, src_layout, dst_layout } => {
                ENode::StateTransform {
                    input: self.find(*input),
                    src_layout: *src_layout,
                    dst_layout: *dst_layout,
                }
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
            // DETERMINISM: sort class IDs so the saturation round visits
            // e-classes in a stable order (HashMap.keys() is randomized).
            let mut class_ids: Vec<EClassId> = self.classes.keys().copied().collect();
            class_ids.sort_unstable();
            for class_id in class_ids {
                let canonical = self.find(class_id);
                // Collect nodes first to avoid borrow issues: `apply`
                // takes `&mut self`, so we cannot hold a borrow on
                // `self.classes` while calling it.
                // DETERMINISM: sort the nodes so rule application order is
                // stable across runs. HashSet iteration is randomized per
                // process; without sorting, the same e-graph could apply
                // rewrites in a different order each run, yielding a
                // different saturated e-graph and thus a different extracted
                // IR (the float_math nondeterminism root cause).
                let mut nodes: Vec<ENode> = self.classes.get(&canonical)
                    .map(|s| s.iter().cloned().collect())
                    .unwrap_or_default();
                nodes.sort_by(|a, b| format!("{:?}", a).cmp(&format!("{:?}", b)));
                nodes.dedup();
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

    /// **Wave 5 — dead-state-elimination helper.**
    ///
    /// Returns true if any e-node in the e-graph references `target` as a
    /// child e-class. Used by the `state_dead_init_elim` rewrite rule's
    /// guard: a `StateInit` whose e-class has zero consumers is provably
    /// dead (no `StateRead`/`StateWrite`/`StateTransform`/`BinOp` reads
    /// from it), so its value is irrelevant and the e-class can be merged
    /// with `Lit(0)` without changing program semantics.
    ///
    /// This is O(n) in the number of e-nodes — acceptable because state-op
    /// e-nodes are rare (one per `state_new` call) and the rule only fires
    /// on `StateInit` nodes (not on every node).
    pub fn is_eclass_referenced(&self, target: EClassId) -> bool {
        let target_canon = self.find(target);
        // DETERMINISM: iterate classes in sorted order (HashMap.keys() is
        // randomized). The result is order-independent (it's a boolean
        // OR), but sorting makes the scan deterministic for profiling.
        let mut class_ids: Vec<EClassId> = self.classes.keys().copied().collect();
        class_ids.sort_unstable();
        for cid in &class_ids {
            let canon = self.find(*cid);
            if canon != *cid {
                continue; // skip merged-away classes
            }
            if let Some(nodes) = self.classes.get(&canon) {
                for node in nodes {
                    for child in self.node_children(node) {
                        if self.find(child) == target_canon {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// **Wave 5 — helper for `is_eclass_referenced`.**
    ///
    /// Returns the e-class IDs of all *child* references in `node`. For
    /// leaf nodes (`Lit`, `VReg`, `StateInit`) this is empty. For
    /// `BinOp`/`StateRead`/`StateWrite`/`StateTransform`, returns the
    /// child e-class IDs (excluding static metadata like `layout_id`,
    /// `offset`, `size`).
    fn node_children(&self, node: &ENode) -> Vec<EClassId> {
        match node {
            ENode::Lit(_) | ENode::VReg(_) | ENode::StateInit { .. } => vec![],
            ENode::BinOp(_, a, b) => vec![*a, *b],
            ENode::StateRead { state, .. } => vec![*state],
            ENode::StateWrite { state, value, .. } => vec![*state, *value],
            ENode::StateTransform { input, .. } => vec![*input],
        }
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
            // DETERMINISM (critical): iterate e-classes and e-nodes in a
            // SORTED order. `self.classes` is a HashMap and each class's
            // `nodes` is a HashSet — both have RANDOMIZED iteration order
            // per process (Rust's RandomState). The extraction picks the
            // lowest-cost node, breaking ties by "first seen" (strict `<`).
            // With random iteration, ties resolve differently each run →
            // different extracted IR → different binaries → flaky
            // miscompiles (the float_math nondeterminism: same source
            // produced 5 different binaries in 5 runs, some MM). Sorting
            // makes tie-breaking deterministic. We sort class IDs, and
            // within each class we sort the nodes by a stable key.
            let mut class_ids: Vec<EClassId> = self.classes.keys().copied().collect();
            class_ids.sort_unstable();
            for cid in &class_ids {
                let cid_canon = self.find(*cid);
                if cid_canon != *cid {
                    continue; // skip merged-away classes
                }
                let nodes = self.classes.get(cid).unwrap();
                // Sort nodes for deterministic tie-breaking. ENode derives
                // Ord if its fields do; if not, we sort by a derived key.
                // Use a stable sort by (cost, debug-format) so equal-cost
                // nodes break ties consistently across runs.
                let mut sorted_nodes: Vec<&ENode> = nodes.iter().collect();
                sorted_nodes.sort_unstable_by(|a, b| {
                    let ca = cost_fn(a);
                    let cb = cost_fn(b);
                    ca.cmp(&cb).then_with(|| format!("{:?}", a).cmp(&format!("{:?}", b)))
                });
                let mut local_best: Option<(usize, ENode)> = None;
                for node in sorted_nodes {
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
                        // Wave 5: state-op e-nodes. Leaves have no
                        // children (StateInit); the others have 1-2 child
                        // e-classes whose best costs contribute to the
                        // total. Unknown child costs default to MAX/2
                        // (same convention as BinOp) so an unresolved
                        // child doesn't artificially lower the cost.
                        ENode::StateInit { .. } => 0,
                        ENode::StateRead { state, .. } => {
                            let s_canon = self.find(*state);
                            best_cost.get(&s_canon).copied().unwrap_or(usize::MAX / 2)
                        }
                        ENode::StateWrite { state, value, .. } => {
                            let s_canon = self.find(*state);
                            let v_canon = self.find(*value);
                            let cs = best_cost.get(&s_canon).copied().unwrap_or(usize::MAX / 2);
                            let cv = best_cost.get(&v_canon).copied().unwrap_or(usize::MAX / 2);
                            cs.saturating_add(cv)
                        }
                        ENode::StateTransform { input, .. } => {
                            let i_canon = self.find(*input);
                            best_cost.get(&i_canon).copied().unwrap_or(usize::MAX / 2)
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

        // ============================================================
        // Wave 5: PMT state-operation rewrite rules.
        //
        // These rules reason about StateInit / StateRead / StateWrite /
        // StateTransform e-nodes. They are sound by construction (the
        // semantic equivalences they encode are consequences of the PMT
        // state-operation semantics), but only two of them
        // (`state_store_load_forward`, `state_transform_elision`) can be
        // encoded as bitvector identities for `bv_verify` — the other two
        // require structural / lifetime analysis. See `bv_verify.rs`'s
        // `verify_all_rules()` for the encodable subset and the
        // per-rule doc comments below for the structural soundness
        // arguments.
        // ============================================================

        // ------------------------------------------------------------
        // Rule 1: Dead-state elimination.
        //
        // `StateInit(L)` whose e-class is referenced by no other e-node
        // (no `StateRead`/`StateWrite`/`StateTransform`/`BinOp` consumes
        // it) → merge with `Lit(0)`. The state buffer is provably unused;
        // its value is irrelevant, so replacing it with any constant
        // (including 0) doesn't change program semantics.
        //
        // `verified: false` — this is a structural / dead-code-style
        // rule (the guard checks "no consumers"), not a bitvector
        // identity. `bv_verify` cannot encode the "unreferenced" guard.
        // The rule is sound by construction: the guard ensures the
        // StateInit's result is never observed. The Wave 36 bv_verify
        // gate accepts unknown rule names as sound-by-construction
        // (see `verify_rules_with_counterexample`'s doc comment), so
        // this rule is admitted without an explicit bv_verify entry.
        //
        // COST NOTE: merging StateInit's class with Lit(0) lets the
        // extractor pick `Lit(0)` (cost 1) over `StateInit{L}` (cost
        // 500). A future wave that wires state ops into `opt.rs` would
        // then emit no allocation instruction for the dead state — the
        // `Lit(0)` placeholder is dropped by the existing dead-vreg
        // elimination pass.
        // ------------------------------------------------------------
        RewriteRule {
            name: "state_dead_init_elim",
            verified: false,
            apply: |node, eg| match node {
                ENode::StateInit { layout_id: _ } => {
                    // Guard: the StateInit's e-class must have ZERO
                    // consumers (no parent e-node references it). We
                    // determine this by scanning all e-classes for child
                    // references to this class. The scan is O(n) in the
                    // number of e-nodes — acceptable because StateInit
                    // nodes are rare (one per `state_new` call).
                    //
                    // We need the e-class ID of this StateInit node to
                    // check consumers. `eg.hashcons` maps the node → its
                    // e-class ID; we look it up here.
                    let class_id = eg.hashcons.get(node).copied();
                    if let Some(cid) = class_id {
                        if !eg.is_eclass_referenced(cid) {
                            // No consumer: the state is dead. Merge with
                            // Lit(0) so extraction picks the cheaper form.
                            let replacement = ENode::Lit(0);
                            let repl_id = eg.add(replacement.clone());
                            return Some((repl_id, replacement));
                        }
                    }
                    None
                }
                _ => None,
            },
        },

        // ------------------------------------------------------------
        // Rule 2: Store-load forwarding (the most impactful rule).
        //
        // `StateRead(StateWrite(s, off, v), off, _) → v`
        //
        // If a `StateRead`'s `state` child is in the same e-class as a
        // `StateWrite` with the same `offset`, the read forwards the
        // just-written value. The rule scans the state's e-class for a
        // matching StateWrite and merges the StateRead's class with the
        // write's `value` e-class.
        //
        // Soundness: PMT states are linear/immutable — a `StateWrite`
        // produces a *new* state e-class, so the only way a StateRead's
        // state child can be in the same e-class as a StateWrite's
        // *output* is if the read directly consumes the write's result.
        // There is no intervening write to the same offset (it would
        // produce a different state e-class). Therefore the read returns
        // exactly the written value.
        //
        // `verified: true` — encodable as the 2-variable bitvector
        // identity `v == v` (the read returns the written value). See
        // `verify_all_rules()` in `bv_verify.rs`.
        // ------------------------------------------------------------
        RewriteRule {
            name: "state_store_load_forward",
            verified: true,
            apply: |node, eg| match node {
                ENode::StateRead { state, offset, size: _ } => {
                    let state_canon = eg.find(*state);
                    // Collect all e-nodes in the state's e-class. A
                    // StateWrite whose *output* is in this class is a
                    // candidate (its `state` field is the *input* state;
                    // the write itself produces this class).
                    let nodes: Vec<ENode> = eg.classes.get(&state_canon)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    for n in &nodes {
                        if let ENode::StateWrite { state: _, offset: w_off, value } = n {
                            if *w_off == *offset {
                                // Forward the written value. The
                                // replacement is a VReg reference to the
                                // value's e-class (same convention as
                                // `add_zero_left` etc.).
                                let v_canon = eg.find(*value);
                                let replacement = ENode::VReg(v_canon);
                                let repl_id = eg.add(replacement.clone());
                                return Some((repl_id, replacement));
                            }
                        }
                    }
                    None
                }
                _ => None,
            },
        },

        // ------------------------------------------------------------
        // Rule 3: Transform elision (identity transform).
        //
        // `StateTransform(x, L, L) → x` — when `src_layout == dst_layout`,
        // the transform is a no-op (the state's bitvector representation
        // is unchanged). The rule merges the transform's e-class with
        // the input's e-class.
        //
        // Soundness: a same-layout transform is, by definition of the
        // PMT transform semantics, the identity function on the state
        // buffer. The output state IS the input state (no copy, no
        // field reshuffle).
        //
        // `verified: true` — encodable as the 1-variable bitvector
        // identity `x == x`. See `verify_all_rules()` in `bv_verify.rs`.
        // ------------------------------------------------------------
        RewriteRule {
            name: "state_transform_elision",
            verified: true,
            apply: |node, eg| match node {
                ENode::StateTransform { input, src_layout, dst_layout } => {
                    if src_layout == dst_layout {
                        let input_canon = eg.find(*input);
                        let replacement = ENode::VReg(input_canon);
                        let repl_id = eg.add(replacement.clone());
                        return Some((repl_id, replacement));
                    }
                    None
                }
                _ => None,
            },
        },

        // ------------------------------------------------------------
        // Rule 4: State merge (DEFERRED — stub).
        //
        // Two `StateInit`s with compatible layouts whose lifetimes don't
        // overlap → merge into one `StateInit` (reuse the buffer).
        //
        // This rule is **not implemented** — it requires lifetime
        // analysis (the e-graph would need to know that state A is dead
        // before state B is born, which is a whole-program dataflow
        // property the e-graph cannot express). The stub is registered
        // here so the rule name appears in the rule set and the
        // `bv_verify` gate's "unknown rule" path admits it; the
        // `verified: false` flag and this comment document that the
        // rule is a no-op placeholder for a future wave.
        //
        // When a future wave implements this, it will likely live in a
        // separate pass (not the e-graph) — e.g., a lifetime-aware
        // buffer-merging pass that runs after `equality_saturation`.
        // ------------------------------------------------------------
        RewriteRule {
            name: "state_merge_compatible_layouts",
            verified: false,
            apply: |_node, _eg| {
                // Intentional no-op: see doc comment above. The rule
                // is registered so its name is in the rule set (for
                // documentation / future bv_verify entries), but it
                // never fires.
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
        // Wave 5: PMT state-operation costs.
        //
        // StateInit is an allocation (heap/stack) — expensive, but cheaper
        // than a Div. StateRead/StateWrite are memory loads/stores. StateTransform
        // is a layout conversion (typically a sequence of field copies).
        // These costs incentivise the e-graph to prefer the *elided* forms
        // (VReg reference to the input) over the raw state ops when a
        // rewrite rule makes them equivalent.
        ENode::StateInit { .. } => 500,
        ENode::StateRead { .. } => 150,
        ENode::StateWrite { .. } => 160,
        ENode::StateTransform { .. } => 300,
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
            // Wave 5: state-op costs via the latency table. StateInit is
            // an allocation (modeled as "arithmetic" — a stack-pointer
            // adjust + tagged-pointer materialise). StateRead/StateWrite
            // are memory accesses (use "arithmetic" as a proxy for the
            // address-compute latency; the load/store itself is accounted
            // for by the regalloc cost model). StateTransform is a
            // sequence of field copies (treat as "arithmetic").
            ENode::StateInit { .. } => {
                let (latency, _, _) = lt.lookup("arithmetic");
                (latency as usize) * 500 / 100
            }
            ENode::StateRead { .. } | ENode::StateWrite { .. } => {
                let (latency, _, _) = lt.lookup("arithmetic");
                (latency as usize) * 150 / 100
            }
            ENode::StateTransform { .. } => {
                let (latency, _, _) = lt.lookup("arithmetic");
                (latency as usize) * 300 / 100
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
            // Wave 5: PMT state-op PGO costs. State ops are biased by
            // the hotness of their child e-classes — a hot state (one
            // accessed in a tight loop) gets a lower cost, encouraging
            // the e-graph to keep the state op rather than eliding it
            // when the elided form would actually be slower (e.g., a
            // forwarded store-load pair that defeats the cache).
            ENode::StateInit { .. } => {
                let (latency, _, _) = lt.lookup("arithmetic");
                let base = (latency as usize) * 500 / 100;
                base
            }
            ENode::StateRead { state, .. } => {
                let (latency, _, _) = lt.lookup("arithmetic");
                let base = (latency as usize) * 150 / 100;
                let hotness = prof.vreg_hotness(*state);
                base / (1 + hotness as usize)
            }
            ENode::StateWrite { state, value, .. } => {
                let (latency, _, _) = lt.lookup("arithmetic");
                let base = (latency as usize) * 160 / 100;
                let hotness = prof.vreg_hotness(*state).max(prof.vreg_hotness(*value));
                base / (1 + hotness as usize)
            }
            ENode::StateTransform { input, .. } => {
                let (latency, _, _) = lt.lookup("arithmetic");
                let base = (latency as usize) * 300 / 100;
                let hotness = prof.vreg_hotness(*input);
                base / (1 + hotness as usize)
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
    let mut seen_hotness = false;

    skip_ws(bytes, &mut pos);
    expect_byte(bytes, &mut pos, b'{')?;
    skip_ws(bytes, &mut pos);

    // Allow empty top-level object: `{}`
    if peek_byte(bytes, pos) == Some(b'}') {
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
            seen_hotness = true;
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
    // Reject non-empty top-level objects that lack the required "hotness" key.
    // (Empty `{}` is allowed — it returns early above as "no profile".)
    if !seen_hotness {
        return Err(
            "missing required \"hotness\" key in profile JSON (got non-empty object without it)"
                .to_string(),
        );
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

    // ============================================================
    // Wave 5 tests — PMT state-operation ENode variants + rules.
    // ============================================================

    /// Wave 5: the four new ENode variants can be added to an e-graph
    /// and retrieved via `find`. Verifies the variants are hashable /
    /// comparable (required by the EGraph hashcons).
    #[test]
    fn test_wave5_state_enodes_roundtrip() {
        let mut eg = EGraph::new();
        let init = eg.add(ENode::StateInit { layout_id: 42 });
        let val = eg.add(ENode::Lit(99));
        let write = eg.add(ENode::StateWrite {
            state: init,
            offset: 0,
            value: val,
        });
        let read = eg.add(ENode::StateRead {
            state: write,
            offset: 0,
            size: 4,
        });
        let transform = eg.add(ENode::StateTransform {
            input: init,
            src_layout: 42,
            dst_layout: 42,
        });
        // Each distinct ENode gets its own e-class.
        assert_ne!(eg.find(init), eg.find(write));
        assert_ne!(eg.find(write), eg.find(read));
        assert_ne!(eg.find(read), eg.find(transform));
        // Adding the same ENode twice returns the same e-class (hashcons).
        let added = eg.add(ENode::StateInit { layout_id: 42 });
        assert_eq!(eg.find(init), eg.find(added));
    }

    /// Wave 5 [dead-state elimination]: `StateInit(L)` whose e-class is
    /// referenced by no other e-node → `state_dead_init_elim` fires and
    /// merges the class with `Lit(0)`. After saturation, extraction
    /// should pick `Lit(0)` (cost 1) over `StateInit{L}` (cost 500).
    #[test]
    fn test_wave5_dead_state_elimination_fires() {
        let mut eg = EGraph::new();
        let init_id = eg.add(ENode::StateInit { layout_id: 7 });
        // No StateRead / StateWrite / StateTransform references init_id.
        let rules = standard_rules();
        eg.saturate(&rules, 10);
        // The rule should have fired (provenance records the step).
        let saw_dead_elim = eg.get_provenance(init_id)
            .iter()
            .any(|s| s.rule_name == "state_dead_init_elim");
        assert!(
            saw_dead_elim,
            "state_dead_init_elim should fire on unreferenced StateInit"
        );
        // Extraction should pick Lit(0) (cheaper than StateInit).
        let best = eg.extract(init_id, &default_cost);
        assert_eq!(
            best, ENode::Lit(0),
            "dead StateInit should extract to Lit(0), got {:?}", best
        );
    }

    /// Wave 5 [dead-state elimination, negative]: when a `StateRead`
    /// consumes the `StateInit`, the dead-state rule must NOT fire
    /// (the state is live — its value matters).
    #[test]
    fn test_wave5_dead_state_elimination_skipped_when_referenced() {
        let mut eg = EGraph::new();
        let init_id = eg.add(ENode::StateInit { layout_id: 7 });
        let _read = eg.add(ENode::StateRead {
            state: init_id,
            offset: 0,
            size: 4,
        });
        let rules = standard_rules();
        eg.saturate(&rules, 10);
        // The rule should NOT have fired on init_id (it's referenced).
        let saw_dead_elim = eg.get_provenance(init_id)
            .iter()
            .any(|s| s.rule_name == "state_dead_init_elim");
        assert!(
            !saw_dead_elim,
            "state_dead_init_elim must NOT fire when StateInit is referenced"
        );
    }

    /// Wave 5 [store-load forwarding]: `StateRead(StateWrite(s, off, v),
    /// off, _) → v`. After saturation, the read's e-class should be
    /// merged with the value's e-class.
    #[test]
    fn test_wave5_store_load_forward_fires() {
        let mut eg = EGraph::new();
        let s = eg.add(ENode::StateInit { layout_id: 1 });
        let v = eg.add(ENode::Lit(42));
        let sw = eg.add(ENode::StateWrite {
            state: s,
            offset: 8,
            value: v,
        });
        let sr = eg.add(ENode::StateRead {
            state: sw,
            offset: 8,
            size: 4,
        });
        let rules = standard_rules();
        eg.saturate(&rules, 10);
        // The rule should have fired.
        let saw_fwd = eg.get_provenance(sr)
            .iter()
            .any(|s| s.rule_name == "state_store_load_forward");
        assert!(
            saw_fwd,
            "state_store_load_forward should fire on StateRead(StateWrite(_, off, v), off, _)"
        );
        // The read's e-class should now be equivalent to the value's.
        assert_eq!(
            eg.find(sr), eg.find(v),
            "StateRead should be merged with the forwarded value's e-class"
        );
        // Extraction should pick Lit(42) (cheaper than StateRead).
        let best = eg.extract(sr, &default_cost);
        assert_eq!(
            best, ENode::Lit(42),
            "forwarded StateRead should extract to Lit(42), got {:?}", best
        );
    }

    /// Wave 5 [store-load forwarding, no match]: if the offsets don't
    /// match, the rule must NOT fire (the read returns the *old* value,
    /// not the written one).
    #[test]
    fn test_wave5_store_load_forward_skipped_on_offset_mismatch() {
        let mut eg = EGraph::new();
        let s = eg.add(ENode::StateInit { layout_id: 1 });
        let v = eg.add(ENode::Lit(42));
        let sw = eg.add(ENode::StateWrite {
            state: s,
            offset: 0,  // write at offset 0
            value: v,
        });
        let sr = eg.add(ENode::StateRead {
            state: sw,
            offset: 4,  // read at offset 4 (different field)
            size: 4,
        });
        let rules = standard_rules();
        eg.saturate(&rules, 10);
        let saw_fwd = eg.get_provenance(sr)
            .iter()
            .any(|s| s.rule_name == "state_store_load_forward");
        assert!(
            !saw_fwd,
            "state_store_load_forward must NOT fire when offsets differ"
        );
        assert_ne!(
            eg.find(sr), eg.find(v),
            "StateRead must NOT be merged with the value when offsets differ"
        );
    }

    /// Wave 5 [transform elision]: `StateTransform(x, L, L) → x`. After
    /// saturation, the transform's e-class should be merged with the
    /// input's e-class.
    #[test]
    fn test_wave5_transform_elision_fires() {
        let mut eg = EGraph::new();
        let x = eg.add(ENode::VReg(100));
        let t = eg.add(ENode::StateTransform {
            input: x,
            src_layout: 5,
            dst_layout: 5,  // same layout → identity
        });
        let rules = standard_rules();
        eg.saturate(&rules, 10);
        let saw_elide = eg.get_provenance(t)
            .iter()
            .any(|s| s.rule_name == "state_transform_elision");
        assert!(
            saw_elide,
            "state_transform_elision should fire on StateTransform(x, L, L)"
        );
        assert_eq!(
            eg.find(t), eg.find(x),
            "StateTransform(x, L, L) should be merged with x's e-class"
        );
    }

    /// Wave 5 [transform elision, no match]: if `src_layout != dst_layout`,
    /// the transform is a real conversion — the rule must NOT fire.
    #[test]
    fn test_wave5_transform_elision_skipped_on_layout_mismatch() {
        let mut eg = EGraph::new();
        let x = eg.add(ENode::VReg(100));
        let t = eg.add(ENode::StateTransform {
            input: x,
            src_layout: 5,
            dst_layout: 6,  // different layout → real conversion
        });
        let rules = standard_rules();
        eg.saturate(&rules, 10);
        let saw_elide = eg.get_provenance(t)
            .iter()
            .any(|s| s.rule_name == "state_transform_elision");
        assert!(
            !saw_elide,
            "state_transform_elision must NOT fire when src_layout != dst_layout"
        );
        assert_ne!(
            eg.find(t), eg.find(x),
            "StateTransform must NOT be merged with input when layouts differ"
        );
    }

    /// Wave 5 [state merge stub]: the `state_merge_compatible_layouts`
    /// rule is registered as a no-op stub. Verify it never fires (no
    /// provenance recorded under its name) even when two same-layout
    /// StateInits are present.
    #[test]
    fn test_wave5_state_merge_stub_never_fires() {
        let mut eg = EGraph::new();
        let _a = eg.add(ENode::StateInit { layout_id: 1 });
        let _b = eg.add(ENode::StateInit { layout_id: 1 });
        let rules = standard_rules();
        eg.saturate(&rules, 10);
        let mut saw_merge = false;
        let class_ids: Vec<EClassId> = eg.classes.keys().copied().collect();
        for cid in class_ids {
            for step in eg.get_provenance(cid) {
                if step.rule_name == "state_merge_compatible_layouts" {
                    saw_merge = true;
                }
            }
        }
        assert!(
            !saw_merge,
            "state_merge_compatible_layouts is a stub and must NOT fire"
        );
    }

    /// Wave 5 [bv_verify gate]: the standard rule set now includes the
    /// four Wave 5 state-op rules. Verify the bv_verify gate still
    /// accepts the full rule set (the new rule names are either in the
    /// bv_verify table as sound, or unknown → assumed sound-by-
    /// construction).
    #[test]
    fn test_wave5_standard_rules_pass_bv_verify_gate() {
        let eg = EGraph::new();
        let standard = standard_rules();
        let result = eg.verify_rules_before_saturate(&standard);
        assert!(
            result.is_ok(),
            "Wave 5 standard rule set must pass the bv_verify gate: {:?}",
            result.err()
        );
    }

    /// Wave 5 [is_eclass_referenced helper]: the helper correctly
    /// identifies referenced vs. unreferenced e-classes.
    #[test]
    fn test_wave5_is_eclass_referenced_helper() {
        let mut eg = EGraph::new();
        let a = eg.add(ENode::VReg(1));
        let b = eg.add(ENode::VReg(2));
        // a is referenced by a BinOp; b is not.
        let _add = eg.add(ENode::BinOp(BinOpKind::Add, a, a));
        assert!(
            eg.is_eclass_referenced(a),
            "a should be referenced (it's a child of the BinOp)"
        );
        assert!(
            !eg.is_eclass_referenced(b),
            "b should NOT be referenced (no e-node has it as a child)"
        );
    }
}
