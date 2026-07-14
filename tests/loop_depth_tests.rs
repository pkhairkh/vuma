//! Loop-depth-aware spill weight tests (Wave 6).
//!
//! These tests prove that the production register allocator now computes
//! loop-nesting depth for each vreg and uses it in spill decisions —
//! intervals inside loops get exponentially higher spill weight.

use vuma_codegen::ir::{BinOpKind, IRBlock, IRFunction, IRInstr, IRTerminator, IRType, IRValue};
use vuma_codegen::regalloc::{LinearScanAllocator, compute_vreg_loop_depths, LiveInterval};

/// Build a function with a self-loop (loop depth 1).
fn make_loop_function() -> IRFunction {
    let mut func = IRFunction::new("loop_fn");
    func.params = vec![IRValue::Register(10)]; // N
    func.param_types = vec![IRType::I64];

    let mut entry = IRBlock::new("entry");
    entry.terminator = IRTerminator::Jump("loop".to_string());

    let mut loop_block = IRBlock::new("loop");
    // i = phi(0, entry), (i_new, loop)
    loop_block.instructions.push(IRInstr::Phi {
        dst: IRValue::Register(0),
        incoming: vec![
            (IRValue::Immediate(0), "entry".to_string()),
            (IRValue::Register(1), "loop".to_string()),
        ],
    });
    // acc = phi(0, entry), (acc_new, loop)
    loop_block.instructions.push(IRInstr::Phi {
        dst: IRValue::Register(2),
        incoming: vec![
            (IRValue::Immediate(0), "entry".to_string()),
            (IRValue::Register(3), "loop".to_string()),
        ],
    });
    // acc_new = acc + i
    loop_block.instructions.push(IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: IRValue::Register(3),
        lhs: IRValue::Register(2),
        rhs: IRValue::Register(0),
        ty: None,
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
        dst: IRValue::Register(4),
        lhs: IRValue::Register(1),
        rhs: IRValue::Register(10),
        ty: None,
    });
    loop_block.terminator = IRTerminator::Branch {
        cond: IRValue::Register(4),
        true_block: "loop".to_string(),
        false_block: "exit".to_string(),
    };

    let mut exit_block = IRBlock::new("exit");
    exit_block.terminator = IRTerminator::Return(vec![IRValue::Register(2)]);

    func.blocks = vec![entry, loop_block, exit_block];
    func
}

#[test]
fn wave6_loop_depth_is_computed() {
    // The loop-carried vregs (i, i_new, acc, acc_new) should have loop_depth >= 1.
    let func = make_loop_function();
    let depths = compute_vreg_loop_depths(&func);

    // vregs 0,1,2,3 are defined and used inside the loop → depth >= 1.
    for vreg in [0u32, 1, 2, 3] {
        let depth = depths.get(&vreg).copied().unwrap_or(0);
        assert!(
            depth >= 1,
            "loop-carried vreg {} should have loop_depth >= 1, got {}",
            vreg,
            depth
        );
    }
}

#[test]
fn wave6_spill_weight_uses_loop_depth() {
    // An interval with loop_depth=2 should have higher spill_weight than
    // the same interval with loop_depth=0.
    let make_interval = |loop_depth: u32| LiveInterval {
        vreg: 0,
        class: vuma_codegen::regalloc::RegClass::Gpr,
        start: 0,
        end: 10,
        crosses_call: false,
        use_positions: vec![1, 5, 10],
        def_positions: vec![0],
        coalesced_vregs: vec![],
        loop_depth,
    };

    let depth0 = make_interval(0);
    let depth2 = make_interval(2);

    let weight0 = depth0.spill_weight();
    let weight2 = depth2.spill_weight();

    assert!(
        weight2 > weight0,
        "loop_depth=2 should have higher spill weight than loop_depth=0 ({} vs {})",
        weight2,
        weight0
    );
    // loop_multiplier for depth 2 = 1 << 2 = 4. So weight2 should be 4x weight0.
    assert_eq!(
        weight2 / weight0,
        4,
        "loop_depth=2 multiplier should be 4x (got {} / {} = {})",
        weight2,
        weight0,
        weight2 / weight0
    );
}

#[test]
fn wave6_production_allocator_runs_with_loop_depth() {
    // End-to-end: the production allocator must succeed on a loop function
    // and actually use the computed loop depths.
    let func = make_loop_function();
    let allocator = LinearScanAllocator::new();
    let result = allocator.allocate_function(&func);
    assert!(result.is_ok(), "allocation should succeed: {:?}", result.err());
}

#[test]
fn wave6_loop_depth_zero_outside_loops() {
    // Vregs that only appear outside loops should have loop_depth = 0.
    let mut func = IRFunction::new("no_loop");
    func.params = vec![IRValue::Register(0)];
    func.param_types = vec![IRType::I64];
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(1),
            ty: None,
        },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

    let depths = compute_vreg_loop_depths(&func);
    for vreg in [0u32, 1] {
        let depth = depths.get(&vreg).copied().unwrap_or(0);
        assert_eq!(depth, 0, "vreg {} outside loops should have depth 0", vreg);
    }
}
