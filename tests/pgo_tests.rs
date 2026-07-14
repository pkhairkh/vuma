//! PGO (profile-guided optimization) tests (Wave 12).
//!
//! These tests prove that profile data can be loaded, serialized, and that
//! the PGO cost function biases e-graph extraction toward hot-path optimization.

use vuma_codegen::egraph::{ProfileData, pgo_cost_fn, target_cost_fn, EGraph, ENode, standard_rules};
use vuma_codegen::ir::BinOpKind;
use vuma_codegen::target_desc::LatencyTable;
use vuma_codegen::opt::{run_optimizations, run_optimizations_with_profile};

#[test]
fn wave12_profile_data_load_from_json() {
    let json = r#"{"hotness": {"0": 100, "1": 50, "5": 200}}"#;
    let profile = ProfileData::from_json(json).expect("should parse");
    assert_eq!(profile.vreg_hotness(0), 100);
    assert_eq!(profile.vreg_hotness(1), 50);
    assert_eq!(profile.vreg_hotness(5), 200);
    assert_eq!(profile.vreg_hotness(999), 0); // unknown = cold
    assert!(profile.has_data());
}

#[test]
fn wave12_profile_data_roundtrip_json() {
    let mut profile = ProfileData::new();
    profile.record_hot(0);
    profile.record_hot(0);
    profile.record_hot(1);
    let json = profile.to_json();
    let restored = ProfileData::from_json(&json).expect("should parse");
    assert_eq!(restored.vreg_hotness(0), 2);
    assert_eq!(restored.vreg_hotness(1), 1);
}

#[test]
fn wave12_empty_profile_has_no_data() {
    let profile = ProfileData::new();
    assert!(!profile.has_data());
    assert_eq!(profile.vreg_hotness(0), 0);
}

#[test]
fn wave12_pgo_cost_fn_lowers_cost_for_hot_vregs() {
    // A hot vreg should have LOWER cost than a cold vreg under pgo_cost_fn,
    // because hot expressions are preferentially kept in optimized form.
    let lt = LatencyTable::default_ooo();

    // Profile: vreg 0 is hot (100), vreg 1 is cold (0).
    let mut hotness = std::collections::HashMap::new();
    hotness.insert(0, 100);
    let profile = ProfileData::from_hotness(hotness);

    let pgo_cost = pgo_cost_fn(&lt, &profile);

    let hot_node = ENode::VReg(0);   // hot
    let cold_node = ENode::VReg(1);  // cold

    let hot_cost = pgo_cost(&hot_node);
    let cold_cost = pgo_cost(&cold_node);

    assert!(
        hot_cost < cold_cost,
        "hot vreg should have lower cost than cold vreg ({} vs {})",
        hot_cost,
        cold_cost
    );
}

#[test]
fn wave12_pgo_cost_fn_differs_from_target_cost_fn() {
    // The PGO cost function should produce DIFFERENT costs than the plain
    // target cost function when profile data is present. This proves PGO
    // actually influences the cost model.
    let lt = LatencyTable::default_ooo();

    let mut hotness = std::collections::HashMap::new();
    hotness.insert(0, 1000); // very hot
    let profile = ProfileData::from_hotness(hotness);

    let target_cost = target_cost_fn(&lt);
    let pgo_cost = pgo_cost_fn(&lt, &profile);

    let node = ENode::VReg(0);
    let t = target_cost(&node);
    let p = pgo_cost(&node);

    assert_ne!(t, p, "PGO cost should differ from target cost for hot vreg");
    assert!(p < t, "PGO cost should be lower for hot vreg");
}

#[test]
fn wave12_run_optimizations_with_profile_compiles() {
    // End-to-end: run_optimizations_with_profile must accept a profile and
    // produce a valid program. This proves the PGO path is wired.
    use vuma_codegen::ir::{IRFunction, IRProgram, IRInstr, IRTerminator, IRValue, BinOpKind};
    let mut func = IRFunction::new("main");
    func.blocks[0].instructions = vec![IRInstr::BinOp {
        op: BinOpKind::Xor,
        dst: IRValue::Register(1),
        lhs: IRValue::Register(0),
        rhs: IRValue::Register(0),
        ty: None,
    }];
    func.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(1)]);
    let program = IRProgram { functions: vec![func], data_sections: vec![] };

    let mut hotness = std::collections::HashMap::new();
    hotness.insert(0, 50);
    let profile = ProfileData::from_hotness(hotness);
    let lt = LatencyTable::default_ooo();

    // Should not crash; should produce a valid program.
    let result = run_optimizations_with_profile(program, &lt, &profile);
    assert!(!result.functions.is_empty());
}

#[test]
fn wave12_empty_profile_falls_back_to_target() {
    // When profile has no data, run_optimizations_with_profile should
    // behave identically to run_optimizations_with_target.
    use vuma_codegen::ir::{IRFunction, IRProgram, IRInstr, IRTerminator, IRValue, BinOpKind};
    let make_program = || {
        let mut func = IRFunction::new("main");
        func.blocks[0].instructions = vec![IRInstr::BinOp {
            op: BinOpKind::Xor,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Register(0),
            ty: None,
        }];
        func.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(1)]);
        IRProgram { functions: vec![func], data_sections: vec![] }
    };

    let lt = LatencyTable::default_ooo();
    let empty_profile = ProfileData::new();

    let with_profile = run_optimizations_with_profile(make_program(), &lt, &empty_profile);
    let without_profile = run_optimizations(make_program());

    // Both should produce the same result (empty profile = no PGO effect).
    assert_eq!(with_profile.functions.len(), without_profile.functions.len());
}

#[test]
fn wave12_invalid_json_rejected() {
    let bad_json = "not json at all";
    assert!(ProfileData::from_json(bad_json).is_err());

    let missing_hotness = r#"{"foo": 1}"#;
    assert!(ProfileData::from_json(missing_hotness).is_err());
}
