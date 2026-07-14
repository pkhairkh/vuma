//! IVE→codegen loop closure tests (Wave 8 — proper, no thread-local).
//!
//! These tests prove that `mark_ive_proven_nonaliasing` computes Alloc-region
//! provenance and returns it explicitly, and that `dead_store_eliminate`
//! receives it as a parameter and uses it to prove non-aliasing across
//! same-type pointers from different allocations.

use vuma_codegen::ir::{BinOpKind, IRFunction, IRInstr, IRTerminator, IRType, IRValue};
use vuma_codegen::opt::{
    mark_ive_proven_nonaliasing, dead_store_eliminate,
    ive_proven_non_aliasing_with,
};

#[test]
fn wave8_alloc_region_provenance_basic() {
    let mut func = IRFunction::new("test_provenance");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        IRInstr::Alloc { dst: IRValue::Register(0), size: 8 },
        IRInstr::Alloc { dst: IRValue::Register(1), size: 8 },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![]);

    let (_func, provenance) = mark_ive_proven_nonaliasing(func);

    assert_eq!(provenance.get(&0), Some(&0), "v0 should derive from region 0");
    assert_eq!(provenance.get(&1), Some(&1), "v1 should derive from region 1");
    assert!(
        ive_proven_non_aliasing_with(&provenance, 0, 1),
        "v0 and v1 (different Allocs) should be proven non-aliasing"
    );
    assert!(
        !ive_proven_non_aliasing_with(&provenance, 0, 0),
        "same vreg should not be non-aliasing with itself"
    );
}

#[test]
fn wave8_offset_propagates_provenance() {
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

    let (_func, provenance) = mark_ive_proven_nonaliasing(func);

    assert_eq!(provenance.get(&1), Some(&0), "v1 (offset of v0) derives from region 0");
    assert_eq!(provenance.get(&2), Some(&0), "v2 (offset of v0) derives from region 0");
    assert_eq!(provenance.get(&4), Some(&3), "v4 (offset of v3) derives from region 3");
    assert!(
        ive_proven_non_aliasing_with(&provenance, 1, 4),
        "v1 (region 0) and v4 (region 3) should be proven non-aliasing"
    );
    assert!(
        !ive_proven_non_aliasing_with(&provenance, 1, 2),
        "v1 and v2 (same region 0) should NOT be proven non-aliasing"
    );
}

#[test]
fn wave8_binop_add_propagates_provenance() {
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

    let (_func, provenance) = mark_ive_proven_nonaliasing(func);
    assert_eq!(
        provenance.get(&1),
        Some(&0),
        "v1 = v0 + 8 should derive from region 0"
    );
}

#[test]
fn wave8_ive_overrides_tbaa_for_same_type_ptrs() {
    // THE Wave 8 test: two same-type (u32) pointers from different Allocs.
    // TBAA says they may alias (same type). IVE says they don't (different regions).
    let mut func = IRFunction::new("test_ive_overrides_tbaa");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        IRInstr::Alloc { dst: IRValue::Register(0), size: 4 },
        IRInstr::Alloc { dst: IRValue::Register(1), size: 4 },
        IRInstr::Store {
            value: IRValue::Immediate(42),
            addr: IRValue::Register(0),
            offset: 0,
            ty: IRType::U32,
        },
        IRInstr::Store {
            value: IRValue::Immediate(99),
            addr: IRValue::Register(0),
            offset: 0,
            ty: IRType::U32,
        },
        IRInstr::Load {
            dst: IRValue::Register(2),
            addr: IRValue::Register(1),
            offset: 0,
            ty: IRType::U32,
        },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(2)]);

    // Run IVE provenance FIRST, then DSE with the provenance.
    let (func, provenance) = mark_ive_proven_nonaliasing(func);
    assert!(ive_proven_non_aliasing_with(&provenance, 0, 1), "v0 and v1 must be proven non-aliasing");

    let result = dead_store_eliminate(func, &provenance);

    let stores: Vec<_> = result.blocks[0].instructions.iter()
        .filter(|i| matches!(i, IRInstr::Store { .. }))
        .collect();
    assert_eq!(
        stores.len(), 1,
        "first store should be eliminated by DSE (IVE proved load doesn't alias); found {} stores",
        stores.len()
    );
    if let IRInstr::Store { value, .. } = stores[0] {
        assert!(matches!(value, IRValue::Immediate(99)), "remaining store should be value=99");
    }
}

#[test]
fn wave8_no_provenance_for_unrelated_vregs() {
    let mut func = IRFunction::new("test_no_provenance");
    func.params = vec![IRValue::Register(0)];
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

    let (_func, provenance) = mark_ive_proven_nonaliasing(func);
    assert!(provenance.get(&0).is_none(), "parameter has no Alloc provenance");
    assert!(provenance.get(&1).is_none(), "v1 derived from parameter has no provenance");
    assert!(
        !ive_proven_non_aliasing_with(&provenance, 0, 1),
        "vregs without provenance should not be proven non-aliasing"
    );
}
