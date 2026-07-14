//! Proof artifacts + provenance tests (Waves 15, 16).
//!
//! Wave 15: proof artifacts use the real bitvector verifier (not a string whitelist).
//! Wave 16: e-graph provenance records rewrite history for debug-info fidelity.

use vuma_codegen::egraph::{EGraph, ENode, RewriteRule, standard_rules, default_cost};
use vuma_codegen::ir::BinOpKind;
use vuma_codegen::proof_artifacts::{ProofArtifact, ProofLog, check_proof_log};

// ── Wave 15: Proof artifacts with real verification ──

#[test]
fn wave15_check_proof_log_accepts_verified_rules() {
    // A proof log with artifacts from verified rules should pass.
    let mut log = ProofLog::new();
    // xor_self is verified by bv_verify.
    log.artifacts.push(ProofArtifact {
        rule_name: "xor_self",
        verified: true,
        source_class: 0,
        source: ENode::BinOp(BinOpKind::Xor, 1, 1), // x ^ x
        replacement: ENode::Lit(0),
    });
    let result = check_proof_log(&log);
    assert!(result.is_ok(), "verified rule should pass: {:?}", result.err());
}

#[test]
fn wave15_check_proof_log_rejects_unverified_rules() {
    // A proof log with an artifact from a rule NOT in the verified set
    // should FAIL. The old code silently passed; the new code must reject.
    let mut log = ProofLog::new();
    log.artifacts.push(ProofArtifact {
        rule_name: "totally_bogus_rule", // not in bv_verify's verified set
        verified: false,
        source_class: 0,
        source: ENode::Lit(0),
        replacement: ENode::Lit(0),
    });
    let result = check_proof_log(&log);
    assert!(result.is_err(), "unverified rule must be rejected");
    assert!(
        result.unwrap_err().contains("NOT verified"),
        "error should mention the rule is not verified"
    );
}

#[test]
fn wave15_check_proof_log_validates_structure() {
    // An artifact claiming to be xor_self but with a source that doesn't
    // match the pattern (x ^ y where x != y) should fail structural validation.
    let mut log = ProofLog::new();
    log.artifacts.push(ProofArtifact {
        rule_name: "xor_self",
        verified: true,
        source_class: 0,
        source: ENode::BinOp(BinOpKind::Xor, 1, 2), // x ^ y (x != y — wrong!)
        replacement: ENode::Lit(0),
    });
    let result = check_proof_log(&log);
    assert!(result.is_err(), "structurally invalid artifact must be rejected");
    assert!(
        result.unwrap_err().contains("structurally invalid"),
        "error should mention structural invalidity"
    );
}

#[test]
fn wave15_check_proof_log_accepts_multiple_verified_rules() {
    let mut log = ProofLog::new();
    log.artifacts.push(ProofArtifact {
        rule_name: "xor_self",
        verified: true,
        source_class: 0,
        source: ENode::BinOp(BinOpKind::Xor, 1, 1),
        replacement: ENode::Lit(0),
    });
    log.artifacts.push(ProofArtifact {
        rule_name: "sub_self",
        verified: true,
        source_class: 1,
        source: ENode::BinOp(BinOpKind::Sub, 2, 2),
        replacement: ENode::Lit(0),
    });
    let result = check_proof_log(&log);
    assert!(result.is_ok(), "multiple verified rules should pass");
}

// ── Wave 16: E-graph provenance ──

#[test]
fn wave16_provenance_records_rewrites() {
    // When the e-graph applies a rewrite, it should record the provenance.
    let mut eg = EGraph::new();
    let x_id = eg.add(ENode::VReg(1_000_000)); // x
    let x_id2 = eg.add(ENode::VReg(1_000_000)); // same x
    let xor_id = eg.add(ENode::BinOp(BinOpKind::Xor, x_id, x_id2)); // x ^ x

    let rules = standard_rules();
    eg.saturate(&rules, 10);

    // The xor_self rule should have fired, recording provenance.
    assert!(
        eg.has_provenance(xor_id),
        "xor_self rewrite should be recorded in provenance"
    );
    let prov = eg.get_provenance(xor_id);
    assert!(!prov.is_empty(), "provenance should have at least one step");
    assert_eq!(prov[0].rule_name, "xor_self");
}

#[test]
fn wave16_provenance_no_rewrites_for_unmatched() {
    // An e-class where no rule fires should have empty provenance.
    let mut eg = EGraph::new();
    let x_id = eg.add(ENode::VReg(0));
    let five_id = eg.add(ENode::Lit(5));
    let add_id = eg.add(ENode::BinOp(BinOpKind::Add, x_id, five_id)); // x + 5 (no rule matches)

    let rules = standard_rules();
    eg.saturate(&rules, 10);

    assert!(
        !eg.has_provenance(add_id),
        "x+5 has no matching rule, provenance should be empty"
    );
}

#[test]
fn wave16_provenance_tracks_source_and_replacement() {
    // The provenance step should record both the source and replacement ENodes.
    let mut eg = EGraph::new();
    let x_id = eg.add(ENode::VReg(1_000_000));
    let x_id2 = eg.add(ENode::VReg(1_000_000));
    let xor_id = eg.add(ENode::BinOp(BinOpKind::Xor, x_id, x_id2));

    let rules = standard_rules();
    eg.saturate(&rules, 10);

    let prov = eg.get_provenance(xor_id);
    if !prov.is_empty() {
        let step = &prov[0];
        // Source should be the Xor node.
        assert!(
            matches!(&step.source, ENode::BinOp(BinOpKind::Xor, _, _)),
            "source should be Xor"
        );
        // Replacement should be Lit(0).
        assert!(
            matches!(&step.replacement, ENode::Lit(0)),
            "replacement should be Lit(0)"
        );
    }
}

#[test]
fn wave16_provenance_enables_debug_traceback() {
    // End-to-end: the provenance allows tracing an optimized instruction
    // back to its source. This is the debug-info fidelity feature.
    let mut eg = EGraph::new();
    let x_id = eg.add(ENode::VReg(42)); // some vreg
    let x_id2 = eg.add(ENode::VReg(42));
    let sub_id = eg.add(ENode::BinOp(BinOpKind::Sub, x_id, x_id2)); // x - x

    let rules = standard_rules();
    eg.saturate(&rules, 10);

    // After saturation, sub_id's e-class should contain Lit(0) (from sub_self).
    // The provenance tells us HOW it got there.
    let best = eg.extract(sub_id, &default_cost);
    assert_eq!(best, ENode::Lit(0), "x-x should extract to Lit(0)");

    // The provenance records the rewrite that produced this.
    let prov = eg.get_provenance(sub_id);
    assert!(
        prov.iter().any(|s| s.rule_name == "sub_self"),
        "provenance should record sub_self rewrite"
    );
}
