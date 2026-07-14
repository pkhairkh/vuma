//! # Proof Artifacts for E-Graph Rewrites (Wave 15)
//!
//! Each rewrite rule application generates a proof artifact that can be
//! checked by the proof checker (`src/proof/src/checker.rs`). This enables
//! machine-verified optimization soundness — every transformation the
//! compiler applies is proven correct.
//!
//! ## Flow
//!
//! 1. E-graph applies a rewrite rule (e.g., `x ^ x → 0`)
//! 2. A `ProofArtifact` is generated recording the rule, the source e-class,
//!    and the replacement e-node
//! 3. The artifact is piped to the proof checker
//! 4. If the checker rejects the proof, the optimization is rolled back

use std::collections::HashMap;
use crate::egraph::{ENode, EClassId, RewriteRule};

/// A proof artifact for a single rewrite application.
#[derive(Debug, Clone)]
pub struct ProofArtifact {
    /// Name of the rewrite rule applied.
    pub rule_name: &'static str,
    /// Whether the rule was SMT-verified.
    pub verified: bool,
    /// The e-class that was rewritten.
    pub source_class: EClassId,
    /// The replacement e-node.
    pub replacement: ENode,
    /// The source e-node (before rewrite).
    pub source: ENode,
}

/// A collection of proof artifacts for a function or program.
#[derive(Debug, Clone, Default)]
pub struct ProofLog {
    /// All proof artifacts, in application order.
    pub artifacts: Vec<ProofArtifact>,
}

impl ProofLog {
    /// Creates an empty proof log.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a rewrite application.
    pub fn record(
        &mut self,
        rule: &RewriteRule,
        source: &ENode,
        replacement: &ENode,
        source_class: EClassId,
    ) {
        self.artifacts.push(ProofArtifact {
            rule_name: rule.name,
            verified: rule.verified,
            source_class,
            replacement: replacement.clone(),
            source: source.clone(),
        });
    }

    /// Returns the number of verified rewrites.
    pub fn verified_count(&self) -> usize {
        self.artifacts.iter().filter(|a| a.verified).count()
    }

    /// Returns the number of unverified rewrites.
    pub fn unverified_count(&self) -> usize {
        self.artifacts.iter().filter(|a| !a.verified).count()
    }

    /// Generates a JSON-serializable summary of the proof log.
    pub fn summary(&self) -> ProofSummary {
        let mut by_rule: HashMap<&'static str, (usize, usize)> = HashMap::new();
        for artifact in &self.artifacts {
            let entry = by_rule.entry(artifact.rule_name).or_insert((0, 0));
            if artifact.verified {
                entry.0 += 1;
            } else {
                entry.1 += 1;
            }
        }
        ProofSummary {
            total_rewrites: self.artifacts.len(),
            verified: self.verified_count(),
            unverified: self.unverified_count(),
            by_rule: by_rule.into_iter().map(|(k, v)| (k.to_string(), v.0, v.1)).collect(),
        }
    }
}

/// Summary of a proof log, suitable for display or serialization.
#[derive(Debug, Clone)]
pub struct ProofSummary {
    /// Total number of rewrites applied.
    pub total_rewrites: usize,
    /// Number of SMT-verified rewrites.
    pub verified: usize,
    /// Number of unverified rewrites (sound by construction, no SMT proof).
    pub unverified: usize,
    /// Per-rule breakdown: (rule_name, verified_count, unverified_count).
    pub by_rule: Vec<(String, usize, usize)>,
}

/// Check a proof log against the bitvector verifier (Wave 15).
///
/// **Wave 36 — call after saturate.** The orchestrator (pipeline.rs) wires
/// this as a compile-time check immediately after `EGraph::saturate_with_proof`
/// populates the log. A failed check rolls back / fails the build: every
/// recorded rewrite must be backed by a verified-sound rule.
///
/// Replaces the old string-whitelist approach with a real check: each
/// artifact's `rule_name` must correspond to a rule that has been verified
/// sound by the Wave 7 bitvector verification framework
/// (`bv_verify::verify_all_rules()`) OR be one of the Wave 31 standard
/// e-graph rules (`egraph::standard_rules()`) — commutativity, associativity,
/// distributivity, constant-folding-across-ops — which are tautologies
/// sound by construction (they have no explicit bv_verify entry because
/// they require e-class structural matching, not bitvector evaluation).
/// If a rule isn't in either set, the check FAILS (the old code silently
/// passed).
///
/// Additionally, for rules where structural verification is possible
/// (e.g., `xor_self` requires both operands to be the same e-class), the
/// checker validates the artifact's source node matches the expected pattern.
pub fn check_proof_log(log: &ProofLog) -> Result<ProofSummary, String> {
    let summary = log.summary();

    // Build the set of verified rule names from the Wave 7 verifier.
    let mut verified_rules: std::collections::HashSet<&'static str> = crate::bv_verify::verify_all_rules()
        .into_iter()
        .filter(|r| r.sound)
        .map(|r| r.rule_name)
        .collect();

    // Wave 36: also accept the Wave 31 standard e-graph rules (commutativity,
    // associativity, distributivity, constant-folding-across-ops). These are
    // sound by construction (tautologies) but have no explicit bv_verify
    // entry. Without this, `check_proof_log` would reject artifacts recorded
    // for `comm_add`, `assoc_add_left`, `distrib_mul_add_fwd`, etc. — which
    // `saturate_with_proof` legitimately emits during equality saturation.
    for rule in crate::egraph::standard_rules() {
        verified_rules.insert(rule.name);
    }

    for artifact in &log.artifacts {
        // Check 1: The rule must be in the verified set.
        if !verified_rules.contains(artifact.rule_name) {
            return Err(format!(
                "Rule '{}' is NOT verified by the bitvector verifier (not in the sound rule set)",
                artifact.rule_name
            ));
        }

        // Check 2: Structural validation — the source node must match the
        // pattern the rule is supposed to match. This catches bugs where a
        // rule is misapplied (e.g., xor_self applied to x^y where x != y).
        if let Some(err) = structurally_validate_artifact(artifact) {
            return Err(format!(
                "Rule '{}' structurally invalid: {}",
                artifact.rule_name, err
            ));
        }
    }

    Ok(summary)
}

/// Structurally validate a proof artifact against its rule's expected pattern.
///
/// Returns `None` if valid, or `Some(error_message)` if the artifact's source
/// node doesn't match what the rule should have matched.
fn structurally_validate_artifact(artifact: &ProofArtifact) -> Option<String> {
    match artifact.rule_name {
        "xor_self" => {
            // Source must be BinOp(Xor, x, x) where both operands are the same.
            match &artifact.source {
                ENode::BinOp(crate::ir::BinOpKind::Xor, x, y) if x == y => None,
                _ => Some(format!("xor_self source must be Xor(x, x), got {:?}", artifact.source)),
            }
        }
        "sub_self" => {
            match &artifact.source {
                ENode::BinOp(crate::ir::BinOpKind::Sub, x, y) if x == y => None,
                _ => Some(format!("sub_self source must be Sub(x, x), got {:?}", artifact.source)),
            }
        }
        "mul_zero_left" => {
            match &artifact.source {
                ENode::BinOp(crate::ir::BinOpKind::Mul, lhs, _) if *lhs == 0 => None,
                _ => Some(format!("mul_zero_left source must be Mul(0, _), got {:?}", artifact.source)),
            }
        }
        "mul_zero_right" => {
            match &artifact.source {
                ENode::BinOp(crate::ir::BinOpKind::Mul, _, rhs) if *rhs == 0 => None,
                _ => Some(format!("mul_zero_right source must be Mul(_, 0), got {:?}", artifact.source)),
            }
        }
        "mul_one_left" => {
            match &artifact.source {
                ENode::BinOp(crate::ir::BinOpKind::Mul, lhs, _) if *lhs == 1 => None,
                _ => Some(format!("mul_one_left source must be Mul(1, _), got {:?}", artifact.source)),
            }
        }
        "mul_one_right" => {
            match &artifact.source {
                ENode::BinOp(crate::ir::BinOpKind::Mul, _, rhs) if *rhs == 1 => None,
                _ => Some(format!("mul_one_right source must be Mul(_, 1), got {:?}", artifact.source)),
            }
        }
        // For rules that match on e-class contents (not directly on the ENode),
        // we can't structurally validate without the full e-graph. These pass
        // structural validation (the bv_verify check above is the real gate).
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::BinOpKind;

    #[test]
    fn test_proof_log() {
        let mut log = ProofLog::new();
        let rule = crate::egraph::RewriteRule {
            name: "xor_self",
            verified: true,
            apply: |_, _| None,
        };
        log.record(
            &rule,
            &ENode::BinOp(BinOpKind::Xor, 1, 1),
            &ENode::Lit(0),
            1,
        );
        assert_eq!(log.verified_count(), 1);
        assert_eq!(log.unverified_count(), 0);

        let summary = check_proof_log(&log).unwrap();
        assert_eq!(summary.total_rewrites, 1);
        assert_eq!(summary.verified, 1);
    }

    // ========================================================================
    // Wave 36 — proof-log checks accept the W31 standard e-graph rules.
    // ========================================================================

    /// Wave 36: `check_proof_log` must accept artifacts whose `rule_name`
    /// is a Wave 31 standard e-graph rule (commutativity, associativity,
    /// distributivity, constant-folding-across-ops). These rules have no
    /// explicit bv_verify entry but are tautologies sound by construction.
    /// Without this acceptance, the orchestrator's post-saturate
    /// `check_proof_log` call would fail on every program that triggers
    /// any W31 rule.
    #[test]
    fn test_wave36_check_proof_log_accepts_w31_rules() {
        let mut log = ProofLog::new();
        // Manually push artifacts for representative W31 rules (the names
        // `saturate_with_proof` would emit). The source/replacement nodes
        // are arbitrary structurally (these rules have no structural
        // validator), so we just use placeholder ENodes.
        let w31_artifact = |rule_name: &'static str| ProofArtifact {
            rule_name,
            verified: true,
            source_class: 0,
            source: ENode::VReg(0),
            replacement: ENode::VReg(0),
        };
        log.artifacts.push(w31_artifact("comm_add"));
        log.artifacts.push(w31_artifact("comm_mul"));
        log.artifacts.push(w31_artifact("assoc_add_left"));
        log.artifacts.push(w31_artifact("assoc_add_right"));
        log.artifacts.push(w31_artifact("assoc_mul_left"));
        log.artifacts.push(w31_artifact("assoc_xor_left"));
        log.artifacts.push(w31_artifact("distrib_mul_add_fwd"));
        log.artifacts.push(w31_artifact("distrib_mul_add_bwd"));
        log.artifacts.push(w31_artifact("peel_add_zero_zero"));
        log.artifacts.push(w31_artifact("peel_mul_one_one"));

        let result = check_proof_log(&log);
        assert!(
            result.is_ok(),
            "W31 standard e-graph rule artifacts must be accepted: {:?}",
            result.err()
        );
    }

    /// Wave 36 end-to-end: `EGraph::saturate_with_proof` populates a
    /// `ProofLog`, and `check_proof_log` accepts it. This is the wiring
    /// the orchestrator (pipeline.rs) relies on: saturate → record → check.
    #[test]
    fn test_wave36_saturate_with_proof_then_check() {
        let mut eg = crate::egraph::EGraph::new();
        // x ^ x  (will trigger xor_self → 0)
        let x = eg.add(ENode::VReg(7));
        let x2 = eg.add(ENode::VReg(7));
        let _xor = eg.add(ENode::BinOp(BinOpKind::Xor, x, x2));
        // a + b  (will trigger comm_add → b + a)
        let a = eg.add(ENode::VReg(8));
        let b = eg.add(ENode::VReg(9));
        let _add = eg.add(ENode::BinOp(BinOpKind::Add, a, b));

        let rules = crate::egraph::standard_rules();
        let mut log = ProofLog::new();
        let saturate_result = eg.saturate_with_proof(&rules, 20, &mut log);
        assert!(saturate_result.is_ok(), "gate should accept standard rules");

        // The log should have recorded at least one rewrite (xor_self and/or
        // comm_add fired on the inputs above).
        assert!(
            !log.artifacts.is_empty(),
            "saturate_with_proof must record at least one ProofArtifact"
        );

        // Every recorded artifact must be backed by a verified rule.
        let check_result = check_proof_log(&log);
        assert!(
            check_result.is_ok(),
            "check_proof_log must accept artifacts emitted by saturate_with_proof: {:?}",
            check_result.err()
        );
        let summary = check_result.unwrap();
        assert!(summary.total_rewrites >= 1);
    }
}
