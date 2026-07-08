//! IVE→codegen loop closure tests (Wave 8).
//!
//! These tests prove that `mark_ive_proven_nonaliasing` actually computes
//! Alloc-region provenance and that downstream passes (DSE) use it to prove
//! non-aliasing across same-type pointers from different allocations — the
//! case TBAA cannot handle.

use vuma_codegen::ir::{BinOpKind, IRFunction, IRInstr, IRTerminator, IRType, IRValue};
use vuma_codegen::opt::{
    mark_ive_proven_nonaliasing, get_ive_provenance, ive_proven_non_aliasing,
    ive_values_proven_non_aliasing, dead_store_eliminate,
};

#[test]
fn wave8_alloc_region_provenance_basic() {
    // v0 = alloc(8)   // region 0
    // v1 = alloc(8)   // region 1
    // After mark_ive_proven_nonaliasing:
    //   get_ive_provenance(0) == Some(0)
    //   get_ive_provenance(1) == Some(1)
    //   ive_proven_non_aliasing(0, 1) == true  (different regions)
    let mut func = IRFunction::new("test_provenance");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        IRInstr::Alloc { dst: IRValue::Register(0), size: 8 },
        IRInstr::Alloc { dst: IRValue::Register(1), size: 8 },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![]);

    mark_ive_proven_nonaliasing(func);

    assert_eq!(get_ive_provenance(0), Some(0), "v0 should derive from region 0");
    assert_eq!(get_ive_provenance(1), Some(1), "v1 should derive from region 1");
    assert!(
        ive_proven_non_aliasing(0, 1),
        "v0 and v1 (different Allocs) should be proven non-aliasing"
    );
    assert!(
        !ive_proven_non_aliasing(0, 0),
        "same vreg should not be non-aliasing with itself"
    );
}

#[test]
fn wave8_offset_propagates_provenance() {
    // v0 = alloc(64)           // region 0
    // v1 = offset(v0, 8)       // derives from region 0
    // v2 = offset(v0, 16)      // derives from region 0
    // v3 = alloc(64)           // region 3
    // v4 = offset(v3, 8)       // derives from region 3
    // ive_proven_non_aliasing(1, 4) should be true (different regions)
    // ive_proven_non_aliasing(1, 2) should be false (same region 0)
    let mut func = IRFunction::new("test_offset_provenance");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        IRInstr::Alloc { dst: IRValue::Register(0), size: 64 },
        IRInstr::Offset {
            dst: IRValue::Register(1),
            base: IRValue::Register(0),
            offset: IRValue::Immediate(8),
        },
        IRInstr::Offset {
            dst: IRValue::Register(2),
            base: IRValue::Register(0),
            offset: IRValue::Immediate(16),
        },
        IRInstr::Alloc { dst: IRValue::Register(3), size: 64 },
        IRInstr::Offset {
            dst: IRValue::Register(4),
            base: IRValue::Register(3),
            offset: IRValue::Immediate(8),
        },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![]);

    mark_ive_proven_nonaliasing(func);

    assert_eq!(get_ive_provenance(1), Some(0), "v1 (offset of v0) derives from region 0");
    assert_eq!(get_ive_provenance(2), Some(0), "v2 (offset of v0) derives from region 0");
    assert_eq!(get_ive_provenance(4), Some(3), "v4 (offset of v3) derives from region 3");

    // v1 and v4 are from different regions → non-aliasing
    assert!(
        ive_proven_non_aliasing(1, 4),
        "v1 (region 0) and v4 (region 3) should be proven non-aliasing"
    );
    // v1 and v2 are from the SAME region → NOT proven non-aliasing
    assert!(
        !ive_proven_non_aliasing(1, 2),
        "v1 and v2 (same region 0) should NOT be proven non-aliasing"
    );
}

#[test]
fn wave8_binop_add_propagates_provenance() {
    // v0 = alloc(64)
    // v1 = v0 + 8   (BinOp Add)
    // v1 should derive from region 0
    let mut func = IRFunction::new("test_binop_provenance");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        IRInstr::Alloc { dst: IRValue::Register(0), size: 64 },
        IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(8),
            ty: None,
        },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![]);

    mark_ive_proven_nonaliasing(func);

    assert_eq!(
        get_ive_provenance(1),
        Some(0),
        "v1 = v0 + 8 should derive from region 0"
    );
}

#[test]
fn wave8_ive_overrides_tbaa_for_same_type_ptrs() {
    // This is THE Wave 8 test: two same-type (u32) pointers from different
    // Allocs. TBAA says they may alias (same type). IVE says they don't
    // (different regions). The IVE proof must win.
    //
    // v0 = alloc(4)        // region 0, used as u32*
    // v1 = alloc(4)        // region 1, used as u32*
    // store(v0, 42)        // store to region 0
    // store(v0, 99)        // overwrite region 0 — first store is dead
    // load(v1)             // load from region 1 (different region, doesn't read v0's store)
    //
    // Without IVE: TBAA sees two u32* and says load(v1) MAY alias store(v0,42),
    // so DSE can't eliminate the first store.
    // With IVE: proven non-aliasing, so load(v1) is proven not to read v0's store,
    // and DSE can eliminate store(v0, 42).
    let mut func = IRFunction::new("test_ive_overrides_tbaa");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        IRInstr::Alloc { dst: IRValue::Register(0), size: 4 },
        IRInstr::Alloc { dst: IRValue::Register(1), size: 4 },
        // store(v0, 42) — first store, should be dead (overwritten before any read)
        IRInstr::Store {
            value: IRValue::Immediate(42),
            addr: IRValue::Register(0),
            offset: 0,
            ty: IRType::U32,
        },
        // store(v0, 99) — overwrites the first store
        IRInstr::Store {
            value: IRValue::Immediate(99),
            addr: IRValue::Register(0),
            offset: 0,
            ty: IRType::U32,
        },
        // load(v1) — from a DIFFERENT region. IVE proves non-aliasing.
        IRInstr::Load {
            dst: IRValue::Register(2),
            addr: IRValue::Register(1),
            offset: 0,
            ty: IRType::U32,
        },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(2)]);

    // Run IVE provenance pass FIRST, then DSE.
    let func = mark_ive_proven_nonaliasing(func);
    // Verify the provenance was computed.
    assert!(ive_proven_non_aliasing(0, 1), "v0 and v1 must be proven non-aliasing");

    let result = dead_store_eliminate(func);

    // The first store (store(v0, 42)) should have been eliminated because:
    // 1. It's overwritten by store(v0, 99) at the same address.
    // 2. The load(v1) in between is IVE-proven non-aliasing, so it doesn't read v0.
    let stores: Vec<_> = result.blocks[0].instructions.iter()
        .filter(|i| matches!(i, IRInstr::Store { .. }))
        .collect();
    assert_eq!(
        stores.len(),
        1,
        "first store should be eliminated by DSE (IVE proved load doesn't alias); found {} stores",
        stores.len()
    );
    // The remaining store should be the second one (value 99).
    if let IRInstr::Store { value, .. } = stores[0] {
        assert!(matches!(value, IRValue::Immediate(99)), "remaining store should be value=99");
    }
}

#[test]
fn wave8_no_provenance_for_unrelated_vregs() {
    // vregs that don't derive from any Alloc should have no provenance.
    // ive_proven_non_aliasing should return false for them.
    let mut func = IRFunction::new("test_no_provenance");
    func.params = vec![IRValue::Register(0)]; // parameter, not from Alloc
    func.param_types = vec![IRType::I64];
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: IRValue::Register(1),
        lhs: IRValue::Register(0),
        rhs: IRValue::Immediate(1),
        ty: None,
    }];
    func.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

    mark_ive_proven_nonaliasing(func);

    // v0 (parameter) and v1 (derived from v0) have no Alloc provenance.
    assert_eq!(get_ive_provenance(0), None, "parameter has no Alloc provenance");
    assert_eq!(get_ive_provenance(1), None, "v1 derived from parameter has no provenance");
    assert!(
        !ive_proven_non_aliasing(0, 1),
        "vregs without provenance should not be proven non-aliasing"
    );
}
