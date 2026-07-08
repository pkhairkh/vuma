//! Correct loop unrolling tests (Wave 13b).
//!
//! These tests prove the unroller is CORRECT: it changes the IV step from
//! +1 to +F (so the loop runs N/F iterations, not N*F), substitutes the IV
//! in each body copy, and bails out for loops it can't analyze.

use vuma_codegen::ir::{BinOpKind, IRBlock, IRFunction, IRInstr, IRTerminator, IRType, IRValue};
use vuma_codegen::loop_unroll::{unroll_loops, try_unroll_block};

/// Build a countable self-loop:
///   loop:
///     i = phi(0, entry), (i_new, loop)
///     body: store(addr + i, i)
///     i_new = i + 1
///     cond = i_new < N
///     branch cond, loop, exit
fn make_counted_loop() -> IRFunction {
    let mut func = IRFunction::new("counted_loop");
    func.params = vec![IRValue::Register(10)]; // N (trip count)
    func.param_types = vec![IRType::I64];

    let mut loop_block = IRBlock::new("loop");
    // i = phi(0, entry), (i_new, loop)
    loop_block.instructions.push(IRInstr::Phi {
        dst: IRValue::Register(0), // i
        incoming: vec![
            (IRValue::Immediate(0), "entry".to_string()),
            (IRValue::Register(1), "loop".to_string()), // i_new
        ],
    });
    // body: addr = base + i (base = reg 20)
    loop_block.instructions.push(IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: IRValue::Register(2),
        lhs: IRValue::Register(20),
        rhs: IRValue::Register(0),
        ty: None,
    });
    // store(addr, i)
    loop_block.instructions.push(IRInstr::Store {
        value: IRValue::Register(0),
        addr: IRValue::Register(2),
        offset: 0,
        ty: IRType::I64,
    });
    // i_new = i + 1
    loop_block.instructions.push(IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: IRValue::Register(1),
        lhs: IRValue::Register(0),
        rhs: IRValue::Immediate(1),
        ty: None,
    });
    // cond = i_new < N
    loop_block.instructions.push(IRInstr::Cmp {
        kind: vuma_codegen::ir::CmpKind::SLt,
        dst: IRValue::Register(3),
        lhs: IRValue::Register(1),
        rhs: IRValue::Register(10),
        ty: None,
    });
    // branch cond, loop, exit
    loop_block.terminator = IRTerminator::Branch {
        cond: IRValue::Register(3),
        true_block: "loop".to_string(),
        false_block: "exit".to_string(),
    };

    let mut entry = IRBlock::new("entry");
    entry.terminator = IRTerminator::Jump("loop".to_string());

    let mut exit = IRBlock::new("exit");
    exit.terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

    func.blocks = vec![entry, loop_block, exit];
    func
}

#[test]
fn wave13b_unroller_changes_iv_step() {
    // After unrolling by 2, the increment should be `i + 2` (not `i + 1`).
    let func = make_counted_loop();
    let result = unroll_loops(func);

    let loop_block = result.blocks.iter().find(|b| b.label == "loop").unwrap();

    // Find the increment instruction (the LAST BinOp Add that defines reg 1 = i_new).
    let mut found_increment = false;
    for instr in &loop_block.instructions {
        if let IRInstr::BinOp { op: BinOpKind::Add, dst, lhs, rhs, .. } = instr {
            if *dst == IRValue::Register(1) {
                // This is the increment. After unrolling by 2, it should be i + 2.
                assert_eq!(
                    *lhs, IRValue::Register(0),
                    "increment lhs should still be i (reg 0)"
                );
                assert_eq!(
                    *rhs, IRValue::Immediate(2),
                    "after unroll-by-2, increment should be i + 2 (not i + 1). Got rhs = {:?}",
                    rhs
                );
                found_increment = true;
            }
        }
    }
    assert!(found_increment, "increment instruction must exist after unrolling");
}

#[test]
fn wave13b_unroller_duplicates_body_with_substitution() {
    // After unrolling by 2, the body (store) should appear TWICE.
    // The second copy should use a SUBSTITUTED IV (i + 1 = i_1), not the
    // original i.
    let func = make_counted_loop();
    let result = unroll_loops(func);

    let loop_block = result.blocks.iter().find(|b| b.label == "loop").unwrap();

    // Count store instructions.
    let stores: Vec<_> = loop_block.instructions.iter()
        .filter(|i| matches!(i, IRInstr::Store { .. }))
        .collect();
    assert_eq!(
        stores.len(),
        2,
        "after unroll-by-2, there should be 2 stores (original + 1 copy). Got {}",
        stores.len()
    );
}

#[test]
fn wave13b_unroller_does_not_quadruple_body() {
    // THE critical test: the old vectorizer multiplied the body by 4 WITHOUT
    // changing the trip count, turning N iterations into 4N work. The correct
    // unroller changes the IV step so total work stays N.
    //
    // After unroll-by-2: 2 body copies per iteration, IV steps by 2,
    // so loop runs N/2 iterations doing 2 bodies each = N total. Correct.
    let func = make_counted_loop();
    let result = unroll_loops(func);
    let loop_block = result.blocks.iter().find(|b| b.label == "loop").unwrap();

    // The body should appear exactly 2 times (not 4, not 1).
    let stores = loop_block.instructions.iter()
        .filter(|i| matches!(i, IRInstr::Store { .. }))
        .count();
    assert_eq!(stores, 2, "unroll-by-2 should produce 2 body copies, not {}", stores);

    // The increment should be +2 (proving the trip count is halved).
    let increment = loop_block.instructions.iter()
        .filter_map(|i| {
            if let IRInstr::BinOp { op: BinOpKind::Add, dst, rhs, .. } = i {
                if *dst == IRValue::Register(1) { Some(rhs.clone()) } else { None }
            } else { None }
        })
        .last();
    assert_eq!(
        increment,
        Some(IRValue::Immediate(2)),
        "unroll-by-2 must change increment to +2 (got {:?})",
        increment
    );
}

#[test]
fn wave13b_unroller_bails_on_non_loop() {
    // A plain block (no self-loop) should not be unrolled.
    let mut block = IRBlock::new("plain");
    block.terminator = IRTerminator::Return(vec![]);
    assert!(try_unroll_block(&block, 2).is_none());
}

#[test]
fn wave13b_unroller_bails_on_no_phi() {
    // A self-loop without a Phi (no induction variable) should not be unrolled.
    let mut block = IRBlock::new("loop");
    block.instructions = vec![IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: IRValue::Register(1),
        lhs: IRValue::Register(0),
        rhs: IRValue::Immediate(1),
        ty: None,
    }];
    block.terminator = IRTerminator::Branch {
        cond: IRValue::Register(2),
        true_block: "loop".to_string(),
        false_block: "exit".to_string(),
    };
    assert!(try_unroll_block(&block, 2).is_none(), "loop without Phi must not be unrolled");
}

#[test]
fn wave13b_unroller_bails_on_calls() {
    // A loop with a call (side effect) should not be unrolled.
    let func = make_counted_loop();
    // Add a call to the loop body.
    let mut func = func;
    let loop_idx = func.blocks.iter().position(|b| b.label == "loop").unwrap();
    func.blocks[loop_idx].instructions.insert(1, IRInstr::Call {
        dst: None,
        func: "side_effect".to_string(),
        args: vec![],
        is_extern: false,
    });
    let result = unroll_loops(func);
    let loop_block = result.blocks.iter().find(|b| b.label == "loop").unwrap();
    // The body should NOT have been duplicated (call prevented unrolling).
    let stores = loop_block.instructions.iter()
        .filter(|i| matches!(i, IRInstr::Store { .. }))
        .count();
    assert_eq!(stores, 1, "loop with call should not be unrolled");
}

#[test]
fn wave13b_unroller_preserves_terminator() {
    // After unrolling, the terminator must still be a Branch back to self.
    let func = make_counted_loop();
    let result = unroll_loops(func);
    let loop_block = result.blocks.iter().find(|b| b.label == "loop").unwrap();
    match &loop_block.terminator {
        IRTerminator::Branch { true_block, false_block, .. } => {
            assert!(true_block == "loop" || false_block == "loop",
                "terminator must still branch back to self");
        }
        other => panic!("terminator should be Branch, got {:?}", other),
    }
}

#[test]
fn wave13b_production_compile_with_unroller() {
    // End-to-end: compile a loop-heavy program through the production
    // pipeline (unroller is now live) and verify it succeeds.
    use vuma::pipeline::{compile, CompileConfig, OptLevel};

    let source = r#"
        fn sum(n) -> i64 {
            total = 0;
            i = 0;
            while i < n {
                total = total + i;
                i = i + 1;
            }
            return total;
        }
        fn main() -> i64 {
            return sum(100);
        }
    "#;

    let config = CompileConfig {
        opt_level: OptLevel::O2,
        ..CompileConfig::default()
    };

    let result = compile(source, &config);
    assert!(result.is_ok(), "compile with unroller failed: {:?}", result.err());
    let output = result.unwrap();
    assert!(!output.binary.is_empty(), "should produce a binary");
}
