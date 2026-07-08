//! Integration tests proving the e-graph equality saturation pass actually
//! fires and rewrites instructions (not a no-op).
//!
//! These are integration tests (not unit tests in opt.rs) because the
//! codegen crate's unit-test module has pre-existing compile errors in
//! loongarch64/mod.rs test code that block the whole test binary. As a
//! separate compilation unit, these tests compile and run independently.

use vuma_codegen::ir::{BinOpKind, IRFunction, IRInstr, IRTerminator, IRType, IRValue};
use vuma_codegen::opt::equality_saturation;

/// Build a minimal function: `fn name(v0: i64) -> i64 { <body> }`
fn make_func(name: &str, body: Vec<IRInstr>, result: IRValue) -> IRFunction {
    let mut func = IRFunction::new(name);
    func.params = vec![IRValue::Register(0)];
    func.param_types = vec![IRType::I64];
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = body;
    func.blocks[0].terminator = IRTerminator::Return(vec![result.clone()]);
    func.results = vec![result];
    func.result_types = vec![IRType::I64];
    func
}

#[test]
fn egraph_folds_xor_self_to_zero() {
    // v1 = v0 ^ v0  →  xor_self rule fires → extracts to Lit(0)
    // After extraction: op=Add, lhs=Immediate(0), rhs=Immediate(0)
    let func = make_func(
        "xor_self",
        vec![IRInstr::BinOp {
            op: BinOpKind::Xor,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Register(0),
            ty: None,
        }],
        IRValue::Register(1),
    );

    let result = equality_saturation(func);
    match &result.blocks[0].instructions[0] {
        IRInstr::BinOp { op, lhs, rhs, .. } => {
            assert_eq!(*op, BinOpKind::Add, "xor_self should rewrite op to Add");
            assert!(
                matches!(lhs, IRValue::Immediate(0)),
                "lhs should be Immediate(0), got {:?}",
                lhs
            );
            assert!(
                matches!(rhs, IRValue::Immediate(0)),
                "rhs should be Immediate(0), got {:?}",
                rhs
            );
        }
        other => panic!("expected BinOp after saturation, got {:?}", other),
    }
}

#[test]
fn egraph_folds_sub_self_to_zero() {
    // v1 = v0 - v0  →  sub_self rule fires → extracts to Lit(0)
    let func = make_func(
        "sub_self",
        vec![IRInstr::BinOp {
            op: BinOpKind::Sub,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Register(0),
            ty: None,
        }],
        IRValue::Register(1),
    );

    let result = equality_saturation(func);
    match &result.blocks[0].instructions[0] {
        IRInstr::BinOp { op, lhs, rhs, .. } => {
            assert_eq!(*op, BinOpKind::Add);
            assert!(matches!(lhs, IRValue::Immediate(0)));
            assert!(matches!(rhs, IRValue::Immediate(0)));
        }
        other => panic!("expected rewritten BinOp, got {:?}", other),
    }
}

#[test]
fn egraph_leaves_unrelated_binop_unchanged() {
    // v1 = v0 + 5  →  no rule matches, instruction unchanged.
    // Proves the pass doesn't corrupt non-rewritable code.
    let func = make_func(
        "no_rewrite",
        vec![IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(5),
            ty: None,
        }],
        IRValue::Register(1),
    );

    let result = equality_saturation(func);
    match &result.blocks[0].instructions[0] {
        IRInstr::BinOp { op, lhs, rhs, .. } => {
            assert_eq!(*op, BinOpKind::Add, "op should be unchanged");
            assert!(matches!(lhs, IRValue::Register(0)), "lhs unchanged");
            assert!(matches!(rhs, IRValue::Immediate(5)), "rhs unchanged");
        }
        other => panic!("expected unchanged BinOp, got {:?}", other),
    }
}

#[test]
fn egraph_folds_xor_self_in_block_with_other_instrs() {
    // v1 = v0 ^ v0
    // v2 = v1 + 1
    // The pass should rewrite v1 to 0, leaving v2's Add intact for
    // constant_fold to simplify later.
    let func = make_func(
        "xor_self_then_add",
        vec![
            IRInstr::BinOp {
                op: BinOpKind::Xor,
                dst: IRValue::Register(1),
                lhs: IRValue::Register(0),
                rhs: IRValue::Register(0),
                ty: None,
            },
            IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(2),
                lhs: IRValue::Register(1),
                rhs: IRValue::Immediate(1),
                ty: None,
            },
        ],
        IRValue::Register(2),
    );

    let result = equality_saturation(func);
    // First instruction (the xor) should be rewritten to Add(0, 0).
    match &result.blocks[0].instructions[0] {
        IRInstr::BinOp { op, lhs, rhs, .. } => {
            assert_eq!(*op, BinOpKind::Add);
            assert!(matches!(lhs, IRValue::Immediate(0)));
            assert!(matches!(rhs, IRValue::Immediate(0)));
        }
        other => panic!("expected rewritten first instr, got {:?}", other),
    }
    // Second instruction (the add) should be unchanged.
    match &result.blocks[0].instructions[1] {
        IRInstr::BinOp { op, lhs, rhs, .. } => {
            assert_eq!(*op, BinOpKind::Add);
            assert!(matches!(lhs, IRValue::Register(1)));
            assert!(matches!(rhs, IRValue::Immediate(1)));
        }
        other => panic!("expected unchanged second instr, got {:?}", other),
    }
}

// ---- Value-aware rule tests (require the apply(&ENode, &EGraph) signature) ----

#[test]
fn egraph_strength_reduces_mul_two_to_add() {
    // v1 = v0 * 2  →  should rewrite to  v1 = v0 + v0
    // This is the headline strength-reduction rule that the old `apply(&ENode)`
    // signature could NOT express (it couldn't inspect child e-classes to
    // detect the literal 2).
    let func = make_func(
        "mul_two",
        vec![IRInstr::BinOp {
            op: BinOpKind::Mul,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(2),
            ty: None,
        }],
        IRValue::Register(1),
    );

    let result = equality_saturation(func);
    match &result.blocks[0].instructions[0] {
        IRInstr::BinOp { op, lhs, rhs, .. } => {
            assert_eq!(*op, BinOpKind::Add, "x*2 should rewrite to Add (x+x)");
            assert!(matches!(lhs, IRValue::Register(0)), "lhs should be v0");
            assert!(matches!(rhs, IRValue::Register(0)), "rhs should be v0");
        }
        other => panic!("expected strength-reduced Add, got {:?}", other),
    }
}

#[test]
fn egraph_folds_mul_one_to_identity() {
    // v1 = v0 * 1  →  v1 = v0 (via VReg extraction)
    let func = make_func(
        "mul_one",
        vec![IRInstr::BinOp {
            op: BinOpKind::Mul,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(1),
            ty: None,
        }],
        IRValue::Register(1),
    );

    let result = equality_saturation(func);
    // The mul should be rewritten to Add(v0, 0) — the VReg extraction arm
    // replaces the BinOp with a copy (Add with zero) that constant_fold
    // /DCE will later eliminate.
    match &result.blocks[0].instructions[0] {
        IRInstr::BinOp { op, lhs, rhs, .. } => {
            assert_eq!(*op, BinOpKind::Add);
            assert!(matches!(lhs, IRValue::Register(0)), "lhs should be v0");
            assert!(matches!(rhs, IRValue::Immediate(0)), "rhs should be 0");
        }
        other => panic!("expected identity-rewritten Add, got {:?}", other),
    }
}

#[test]
fn egraph_folds_add_zero_to_identity() {
    // v1 = v0 + 0  →  v1 = v0
    let func = make_func(
        "add_zero",
        vec![IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(0),
            ty: None,
        }],
        IRValue::Register(1),
    );

    let result = equality_saturation(func);
    match &result.blocks[0].instructions[0] {
        IRInstr::BinOp { op, lhs, rhs, .. } => {
            assert_eq!(*op, BinOpKind::Add);
            assert!(matches!(lhs, IRValue::Register(0)));
            assert!(matches!(rhs, IRValue::Immediate(0)));
        }
        other => panic!("expected identity Add, got {:?}", other),
    }
}

#[test]
fn egraph_folds_mul_zero_to_zero() {
    // v1 = v0 * 0  →  v1 = 0
    let func = make_func(
        "mul_zero",
        vec![IRInstr::BinOp {
            op: BinOpKind::Mul,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(0),
            ty: None,
        }],
        IRValue::Register(1),
    );

    let result = equality_saturation(func);
    match &result.blocks[0].instructions[0] {
        IRInstr::BinOp { op, lhs, rhs, .. } => {
            assert_eq!(*op, BinOpKind::Add);
            assert!(matches!(lhs, IRValue::Immediate(0)));
            assert!(matches!(rhs, IRValue::Immediate(0)));
        }
        other => panic!("expected zero-folded Add, got {:?}", other),
    }
}

// ---- Add/Sub/Mul/Div variant tests (the audit found scg_to_ir emits these,
//        not BinOp, so the e-graph never fired on production IR) ----

#[test]
fn egraph_folds_sub_self_variant_to_zero() {
    // IRInstr::Sub (standalone variant, what scg_to_ir emits)
    let func = make_func(
        "sub_self_variant",
        vec![IRInstr::Sub {
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Register(0),
            ty: None,
        }],
        IRValue::Register(1),
    );
    let result = equality_saturation(func);
    match &result.blocks[0].instructions[0] {
        IRInstr::Sub { lhs, rhs, .. } => {
            assert!(matches!(lhs, IRValue::Immediate(0)), "lhs should be 0, got {:?}", lhs);
            assert!(matches!(rhs, IRValue::Immediate(0)), "rhs should be 0, got {:?}", rhs);
        }
        other => panic!("expected Sub after saturation, got {:?}", other),
    }
}

#[test]
fn egraph_folds_mul_zero_variant_to_zero() {
    // IRInstr::Mul with rhs = 0 → should fold to 0
    let func = make_func(
        "mul_zero_variant",
        vec![IRInstr::Mul {
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(0),
            ty: None,
        }],
        IRValue::Register(1),
    );
    let result = equality_saturation(func);
    match &result.blocks[0].instructions[0] {
        IRInstr::Mul { lhs, rhs, .. } => {
            assert!(matches!(lhs, IRValue::Immediate(0)), "x*0 should fold lhs to 0, got {:?}", lhs);
            assert!(matches!(rhs, IRValue::Immediate(0)), "x*0 should fold rhs to 0, got {:?}", rhs);
        }
        other => panic!("expected Mul after saturation, got {:?}", other),
    }
}

#[test]
fn egraph_folds_add_zero_variant_to_identity() {
    // IRInstr::Add with rhs = 0 → should fold to identity (x + 0 = x)
    let func = make_func(
        "add_zero_variant",
        vec![IRInstr::Add {
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(0),
            ty: None,
        }],
        IRValue::Register(1),
    );
    let result = equality_saturation(func);
    match &result.blocks[0].instructions[0] {
        IRInstr::Add { lhs, rhs, .. } => {
            assert!(matches!(lhs, IRValue::Register(0)), "x+0 should keep lhs as v0, got {:?}", lhs);
            assert!(matches!(rhs, IRValue::Immediate(0)), "x+0 rhs should be 0, got {:?}", rhs);
        }
        other => panic!("expected Add after saturation, got {:?}", other),
    }
}
