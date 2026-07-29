//! IVE→codegen loop closure tests.
//!
//! These tests prove that `mark_ive_proven_nonaliasing` computes Alloc-region
//! provenance and returns it explicitly, and that `dead_store_eliminate`
//! receives it as a parameter and uses it to prove non-aliasing across
//! same-type pointers from different allocations.

use std::collections::{HashMap, HashSet};

use vuma_codegen::ir::{
    BinOpKind, IRFunction, IRInstr, IRTerminator, IRType, IRValue, VregMeta, VumaGrade,
};
use vuma_codegen::opt::{
    build_happens_before_dag, dead_state_eliminate, dead_store_eliminate,
    dead_store_eliminate_with_linearity, ive_proven_non_aliasing_with,
    mark_ive_proven_nonaliasing, schedule_instructions, schedule_with_provenance,
};
use vuma_codegen::regalloc::build_grade_interference;

#[test]
fn wave8_alloc_region_provenance_basic() {
    let mut func = IRFunction::new("test_provenance");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 8,
        },
        IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 8,
        },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![]);

    let (_func, provenance) = mark_ive_proven_nonaliasing(func);

    assert_eq!(
        provenance.get(&0),
        Some(&0),
        "v0 should derive from region 0"
    );
    assert_eq!(
        provenance.get(&1),
        Some(&1),
        "v1 should derive from region 1"
    );
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
        IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 64,
        },
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
        IRInstr::Alloc {
            dst: IRValue::Register(3),
            size: 64,
        },
        IRInstr::Offset {
            dst: IRValue::Register(4),
            base: IRValue::Register(3),
            offset: IRValue::Immediate(8),
        },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![]);

    let (_func, provenance) = mark_ive_proven_nonaliasing(func);

    assert_eq!(
        provenance.get(&1),
        Some(&0),
        "v1 (offset of v0) derives from region 0"
    );
    assert_eq!(
        provenance.get(&2),
        Some(&0),
        "v2 (offset of v0) derives from region 0"
    );
    assert_eq!(
        provenance.get(&4),
        Some(&3),
        "v4 (offset of v3) derives from region 3"
    );
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
        IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 64,
        },
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
    // THE Test: two same-type (u32) pointers from different Allocs.
    // TBAA says they may alias (same type). IVE says they don't (different regions).
    let mut func = IRFunction::new("test_ive_overrides_tbaa");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 4,
        },
        IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 4,
        },
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
    assert!(
        ive_proven_non_aliasing_with(&provenance, 0, 1),
        "v0 and v1 must be proven non-aliasing"
    );

    let result = dead_store_eliminate(func, &provenance);

    let stores: Vec<_> = result.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i, IRInstr::Store { .. }))
        .collect();
    assert_eq!(
        stores.len(),
        1,
        "first store should be eliminated by DSE (IVE proved load doesn't alias); found {} stores",
        stores.len()
    );
    if let IRInstr::Store { value, .. } = stores[0] {
        assert!(
            matches!(value, IRValue::Immediate(99)),
            "remaining store should be value=99"
        );
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
    assert!(
        provenance.get(&0).is_none(),
        "parameter has no Alloc provenance"
    );
    assert!(
        provenance.get(&1).is_none(),
        "v1 derived from parameter has no provenance"
    );
    assert!(
        !ive_proven_non_aliasing_with(&provenance, 0, 1),
        "vregs without provenance should not be proven non-aliasing"
    );
}


// ===========================================================================
// Wave 1-B: linearity-driven DCE tests
// ===========================================================================
//
// Wave 0-A added `dead_store_eliminate_with_linearity` (consumed-state store
// kill) and `dead_state_eliminate` (whole-lifecycle removal of consumed
// state). These tests exercise both entry points directly with a hand-built
// `consumed_vregs` set, mirroring the data the IVE linearity report will
// feed in once Wave 4-A routes it through the pipeline.

#[test]
fn test_linearity_dce_removes_dead_state_store() {
    // Alloc(v0); Store(addr=v0); v0 is IVE-proven consumed -> the store
    // writes to destroyed state with no observable effect and must be
    // eliminated by the linearity-aware DSE.
    let mut func = IRFunction::new("lin_dce_dead_store");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 8,
        },
        IRInstr::Store {
            value: IRValue::Immediate(42),
            addr: IRValue::Register(0),
            offset: 0,
            ty: IRType::U32,
        },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![]);

    let (func, provenance) = mark_ive_proven_nonaliasing(func);

    let mut consumed: HashSet<u32> = HashSet::new();
    consumed.insert(0);

    let result = dead_store_eliminate_with_linearity(func, &provenance, Some(&consumed));

    let stores: Vec<_> = result.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i, IRInstr::Store { .. }))
        .collect();
    assert_eq!(
        stores.len(),
        0,
        "store to consumed state v0 should be eliminated by linearity DCE; found {} stores",
        stores.len()
    );
}

#[test]
fn test_linearity_dce_keeps_live_state_store() {
    // Same IR as above, but v0 is NOT consumed -> the store must survive
    // (the state is still live and the store may be observed).
    let mut func = IRFunction::new("lin_dce_live_store");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 8,
        },
        IRInstr::Store {
            value: IRValue::Immediate(42),
            addr: IRValue::Register(0),
            offset: 0,
            ty: IRType::U32,
        },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![]);

    let (func, provenance) = mark_ive_proven_nonaliasing(func);

    // v0 is NOT in the consumed set: linearity data present, but v0 is live.
    let consumed: HashSet<u32> = HashSet::new();

    let result = dead_store_eliminate_with_linearity(func, &provenance, Some(&consumed));

    let stores: Vec<_> = result.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i, IRInstr::Store { .. }))
        .collect();
    assert_eq!(
        stores.len(),
        1,
        "store to live (unconsumed) state v0 must be kept; found {} stores",
        stores.len()
    );
}

#[test]
fn test_dead_state_eliminate_removes_consumed_alloc() {
    // Alloc(v0); v0 consumed -> dead_state_eliminate removes the Alloc (the
    // whole materialisation lifecycle of the consumed state is dead).
    let mut func = IRFunction::new("dse_consumed_alloc");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![IRInstr::Alloc {
        dst: IRValue::Register(0),
        size: 8,
    }];
    func.blocks[0].terminator = IRTerminator::Return(vec![]);

    let mut consumed: HashSet<u32> = HashSet::new();
    consumed.insert(0);

    let result = dead_state_eliminate(func, &consumed);

    let allocs: Vec<_> = result.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i, IRInstr::Alloc { .. }))
        .collect();
    assert_eq!(
        allocs.len(),
        0,
        "Alloc of consumed state v0 should be removed by dead_state_eliminate; found {} allocs",
        allocs.len()
    );
}


// ===========================================================================
// Wave 2-B: grade-based register-allocation interference tests
// ===========================================================================
//
// Wave 2-A added `build_grade_interference(func)` to regalloc.rs. It derives
// interference edges purely from `IRFunction::vreg_meta` grades (Wave 0-B),
// independent of plain liveness:
//
// - Exclusive + Exclusive, live simultaneously -> interfere.
// - Exclusive + Shared -> interfere (over-constraint, sound regardless of
//   liveness).
// - Shared + Shared -> do NOT interfere (they may share a register).
// - Any pair involving a vreg whose grade is `None` (or absent) -> skipped;
//   such pairs fall back to liveness interference via `build_merged_interference`.
//
// These three tests exercise the core grade rules directly against a
// hand-built `vreg_meta` table. The IR is constructed so the two vregs are
// live simultaneously (overlapping live intervals), isolating the grade rule
// as the deciding factor.

/// Two `Exclusive` vregs that are live simultaneously must interfere.
#[test]
fn test_exclusive_vregs_interfere() {
    // v0: defined at instr 0 (pos 0), used at instr 2 (pos 4) -> [0, 4]
    // v1: defined at instr 1 (pos 2), used at instr 3 (pos 6) -> [2, 6]
    // Strict overlap: 0 < 6 && 2 < 4 -> overlap. Both Exclusive -> edge.
    let mut func = IRFunction::new("grade_excl_interfere");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 8,
        },
        IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 8,
        },
        IRInstr::Store {
            value: IRValue::Immediate(1),
            addr: IRValue::Register(0),
            offset: 0,
            ty: IRType::U32,
        },
        IRInstr::Store {
            value: IRValue::Immediate(2),
            addr: IRValue::Register(1),
            offset: 0,
            ty: IRType::U32,
        },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![]);

    func.vreg_meta.insert(0, VregMeta { grade: Some(VumaGrade::Exclusive) });
    func.vreg_meta.insert(1, VregMeta { grade: Some(VumaGrade::Exclusive) });

    let edges = build_grade_interference(&func);

    assert!(
        edges.contains(&(0, 1)),
        "two Exclusive vregs live simultaneously must interfere; got {:?}",
        edges
    );
}

/// Two `Shared` vregs must NOT interfere, even when live simultaneously --
/// they are permitted to share a physical register.
#[test]
fn test_shared_vregs_dont_interfere() {
    // Same overlapping live intervals as the Exclusive case, but both vregs
    // are graded Shared -> no grade-based edge may be emitted.
    let mut func = IRFunction::new("grade_shared_no_interfere");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 8,
        },
        IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 8,
        },
        IRInstr::Store {
            value: IRValue::Immediate(1),
            addr: IRValue::Register(0),
            offset: 0,
            ty: IRType::U32,
        },
        IRInstr::Store {
            value: IRValue::Immediate(2),
            addr: IRValue::Register(1),
            offset: 0,
            ty: IRType::U32,
        },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![]);

    func.vreg_meta.insert(0, VregMeta { grade: Some(VumaGrade::Shared) });
    func.vreg_meta.insert(1, VregMeta { grade: Some(VumaGrade::Shared) });

    let edges = build_grade_interference(&func);

    assert!(
        !edges.contains(&(0, 1)),
        "two Shared vregs must not interfere (they may share a register); got {:?}",
        edges
    );
    assert!(
        edges.is_empty(),
        "Shared+Shared should emit no grade-based edges; got {:?}",
        edges
    );
}

/// Vregs whose grade is `None` (unknown) are skipped by the grade-based
/// pass -- it must return nothing for them (they fall back to liveness
/// interference via `build_merged_interference`, not here).
#[test]
fn test_unknown_grade_no_interference() {
    // Both vregs carry an explicit `None` grade and are live simultaneously.
    // The grade-based pass must not emit any edge for them.
    let mut func = IRFunction::new("grade_unknown_no_interfere");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 8,
        },
        IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 8,
        },
        IRInstr::Store {
            value: IRValue::Immediate(1),
            addr: IRValue::Register(0),
            offset: 0,
            ty: IRType::U32,
        },
        IRInstr::Store {
            value: IRValue::Immediate(2),
            addr: IRValue::Register(1),
            offset: 0,
            ty: IRType::U32,
        },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![]);

    func.vreg_meta.insert(0, VregMeta { grade: None });
    func.vreg_meta.insert(1, VregMeta { grade: None });

    let edges = build_grade_interference(&func);

    assert!(
        edges.is_empty(),
        "vregs with unknown (None) grade must produce no grade-based interference; got {:?}",
        edges
    );
    assert!(
        !edges.contains(&(0, 1)),
        "None-grade pair must not appear in grade interference; got {:?}",
        edges
    );
}

// ===========================================================================
// Wave 3-B: proof-directed instruction scheduler tests
//
// These exercise `schedule_instructions` / `schedule_with_provenance`
// (Wave 3-A, opt.rs) and the happens-before DAG (`build_happens_before_dag`,
// Wave 0-C).  The scheduler list-schedules each block honouring the DAG and
// a barrier policy; memory ops float to the top, ties broken by smallest
// original index ("if unsure, keep original order").
// ===========================================================================

/// A real memory dependency (Store then Load of the *same* address) must be
/// preserved: the DAG carries a Store -> Load edge, so after scheduling the
/// Store still precedes the Load.
#[test]
fn test_scheduler_preserves_dependencies() {
    let mut func = IRFunction::new("sched_preserve_deps");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        // v0 = alloc (region root 0)
        IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 8,
        },
        // store imm 1 -> [v0+0]   (global idx 1)
        IRInstr::Store {
            value: IRValue::Immediate(1),
            addr: IRValue::Register(0),
            offset: 0,
            ty: IRType::U32,
        },
        // v1 = load [v0+0]        (global idx 2) -- RAW on the Store's memory
        IRInstr::Load {
            dst: IRValue::Register(1),
            addr: IRValue::Register(0),
            offset: 0,
            ty: IRType::U32,
        },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![]);

    // Realistic IVE provenance: v0 derives from region 0.  Store and Load
    // address the *same* region (0), so they are NOT proven non-aliasing and
    // the DAG must keep the Store -> Load memory edge.
    let (func, provenance) = mark_ive_proven_nonaliasing(func);
    assert_eq!(
        provenance.get(&0),
        Some(&0),
        "v0 should derive from region 0"
    );

    // The DAG must record the Store(1) -> Load(2) memory dependency.
    let dag = build_happens_before_dag(&func, &provenance);
    assert!(
        dag.contains(&(1, 2)),
        "Store->Load on the same address must produce a DAG edge; got {:?}",
        dag
    );

    let scheduled = schedule_with_provenance(&func, &provenance);
    let instrs = &scheduled.blocks[0].instructions;

    let store_pos = instrs
        .iter()
        .position(|i| matches!(i, IRInstr::Store { .. }))
        .expect("Store must survive scheduling");
    let load_pos = instrs
        .iter()
        .position(|i| matches!(i, IRInstr::Load { .. }))
        .expect("Load must survive scheduling");
    assert!(
        store_pos < load_pos,
        "Store must remain before Load after scheduling (store@{} >= load@{}); order: {:?}",
        store_pos,
        load_pos,
        instrs
    );
}

/// Two `Alloc`s from *different* IVE regions are fully independent: the DAG
/// emits no edge between them, so either program order is a valid schedule.
#[test]
fn test_scheduler_reorders_independent() {
    let mut func = IRFunction::new("sched_reorder_independent");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        // v0 = alloc (region 0)
        IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 8,
        },
        // v1 = alloc (region 1) -- independent of v0
        IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 8,
        },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![]);

    // IVE proves v0 and v1 originate from different regions.
    let (func, provenance) = mark_ive_proven_nonaliasing(func);
    assert_eq!(provenance.get(&0), Some(&0));
    assert_eq!(provenance.get(&1), Some(&1));
    assert!(
        ive_proven_non_aliasing_with(&provenance, 0, 1),
        "v0 and v1 (different Allocs) must be proven non-aliasing"
    );

    // No vreg RAW/WAW (distinct vregs), no memory ops, no Free ->
    // the two Allocs share NO DAG edge, so either order is valid.
    let dag = build_happens_before_dag(&func, &provenance);
    assert!(
        !dag.contains(&(0, 1)) && !dag.contains(&(1, 0)),
        "independent Allocs must have no DAG edge between them; got {:?}",
        dag
    );
    assert!(
        dag.is_empty(),
        "two independent Allocs should yield an edge-free DAG; got {:?}",
        dag
    );

    // Scheduling must still produce a well-formed function containing both
    // Allocs (the multiset of instructions is preserved; the tie-break picks
    // the original order, which is one of the valid orders).
    let scheduled = schedule_with_provenance(&func, &provenance);
    let mut defined: Vec<u32> = scheduled.blocks[0]
        .instructions
        .iter()
        .filter_map(|i| match i {
            IRInstr::Alloc { dst, .. } => dst.as_register(),
            _ => None,
        })
        .collect();
    defined.sort();
    assert_eq!(
        defined,
        vec![0, 1],
        "both independent Allocs must survive scheduling; got order {:?}",
        scheduled.blocks[0].instructions
    );

    // Sanity: the bare schedule_instructions entry point agrees (same DAG,
    // no provenance-driven edges to drop).
    let scheduled2 = schedule_instructions(&func, &dag);
    let mut defined2: Vec<u32> = scheduled2.blocks[0]
        .instructions
        .iter()
        .filter_map(|i| match i {
            IRInstr::Alloc { dst, .. } => dst.as_register(),
            _ => None,
        })
        .collect();
    defined2.sort();
    assert_eq!(defined2, vec![0, 1]);
}

/// When aliasing is *unknown* (no IVE provenance), the scheduler must play it
/// safe: a conservative Store -> Store memory edge is emitted and the original
/// Store order is preserved.  With IVE provenance proving the two stores target
/// different regions, that same edge is dropped -- proving the safety gate is
/// what keeps the order when unsafe.
#[test]
fn test_scheduler_keeps_original_when_unsafe() {
    let mut func = IRFunction::new("sched_keep_when_unsafe");
    func.blocks[0].label = "entry".to_string();
    func.blocks[0].instructions = vec![
        // v0 = alloc (region 0)            global idx 0
        IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 8,
        },
        // v1 = alloc (region 1)            global idx 1
        IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 8,
        },
        // store imm 1 -> [v0+0]            global idx 2
        IRInstr::Store {
            value: IRValue::Immediate(1),
            addr: IRValue::Register(0),
            offset: 0,
            ty: IRType::U32,
        },
        // store imm 2 -> [v1+0]            global idx 3
        IRInstr::Store {
            value: IRValue::Immediate(2),
            addr: IRValue::Register(1),
            offset: 0,
            ty: IRType::U32,
        },
    ];
    func.blocks[0].terminator = IRTerminator::Return(vec![]);

    // --- Unsafe case: NO IVE provenance -> aliasing unknown ---------------
    // The DAG must conservatively order the two Stores (Store(2) -> Store(3)).
    let empty_prov: HashMap<u32, u32> = HashMap::new();
    let dag_unsafe = build_happens_before_dag(&func, &empty_prov);
    assert!(
        dag_unsafe.contains(&(2, 3)),
        "unknown aliasing must force a Store->Store DAG edge; got {:?}",
        dag_unsafe
    );

    // --- Safe case: IVE proves v0,v1 from different regions --------------
    // The non-aliasing exemption drops the Store->Store memory edge.
    let mut prov = HashMap::new();
    prov.insert(0, 0);
    prov.insert(1, 1);
    let dag_safe = build_happens_before_dag(&func, &prov);
    assert!(
        !dag_safe.contains(&(2, 3)),
        "proven non-aliasing must drop the Store->Store edge; got {:?}",
        dag_safe
    );

    // --- Run the scheduler under the UNSAFE (empty) provenance ----------
    // The conservative edge forces the original Store order to be kept.
    let scheduled = schedule_with_provenance(&func, &empty_prov);
    let instrs = &scheduled.blocks[0].instructions;

    let mut store_v0_pos = None;
    let mut store_v1_pos = None;
    for (i, instr) in instrs.iter().enumerate() {
        if let IRInstr::Store {
            addr: IRValue::Register(r),
            ..
        } = instr
        {
            if *r == 0 {
                store_v0_pos = Some(i);
            } else if *r == 1 {
                store_v1_pos = Some(i);
            }
        }
    }
    let store_v0_pos = store_v0_pos.expect("Store to v0 must survive scheduling");
    let store_v1_pos = store_v1_pos.expect("Store to v1 must survive scheduling");
    assert!(
        store_v0_pos < store_v1_pos,
        "under unknown aliasing the original Store order must be preserved \
         (store[v0]@{} >= store[v1]@{}); order: {:?}",
        store_v0_pos,
        store_v1_pos,
        instrs
    );

    // All four original instructions must still be present.
    assert_eq!(
        instrs.len(),
        4,
        "scheduler must preserve the instruction multiset; got {:?}",
        instrs
    );
}
