//! Integration tests for the OriginVerifier (IVE module).
//!
//! Comprehensive test suite covering:
//! - Basic origin verification (valid derivation, dangling pointer, null pointer,
//!   out-of-bounds, valid dereference chain)
//! - Provenance features (graph construction, reachability, unreachable nodes,
//!   cast tracking, forged pointer detection)
//! - Advanced scenarios (stack escape, wild pointer, multiple derivation chains,
//!   cast classification, provenance with offsets)

use vuma_ive::origin::{
    Access, AccessKind, Address, Derivation, DerivationId, DerivationKind, DerivationSource,
    OriginReport, OriginRoot, OriginVerifier, Region, RegionId, TaintLevel,
    ViolationKind,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Shorthand for creating an `Address` from a `u64`.
fn addr(v: u64) -> Address {
    Address::new(v)
}

/// Shorthand for creating a `RegionId`.
fn rid(v: u64) -> RegionId {
    RegionId(v)
}

/// Shorthand for creating a `DerivationId`.
fn did(v: u64) -> DerivationId {
    DerivationId(v)
}

/// Verify the verifier and return the report.
fn verify(verifier: &OriginVerifier) -> OriginReport {
    verifier.verify()
}

// ===========================================================================
// Category 1: Basic Origin (5 tests)
// ===========================================================================

#[test]
fn test_valid_derivation() {
    // Scenario: alloc → offset → access  →  should be Proven (no violations).
    //
    // Region R1 at 0x1000, size 256.
    // D1: Direct from R1, range [0x1000, 0x1100)
    // D2: Offset(+16) from D1, range [0x1010, 0x1020)
    // A1: Read from D2, initialized
    let mut v = OriginVerifier::new();
    v.add_region(Region::new(rid(1), addr(0x1000), 256));
    v.add_derivation(Derivation::new(
        did(1),
        DerivationSource::Region(rid(1)),
        DerivationKind::Direct,
        (addr(0x1000), addr(0x1100)),
    ));
    v.add_derivation(Derivation::new(
        did(2),
        DerivationSource::AnotherDerivation(did(1)),
        DerivationKind::Offset { by: 16 },
        (addr(0x1010), addr(0x1020)),
    ));
    v.add_access(Access::new(
        vuma_ive::origin::AccessId(1),
        did(2),
        AccessKind::Read,
        4,
        "test.vu:10",
        true,
    ));

    let report = verify(&v);

    assert!(report.is_clean(), "Valid alloc→offset→access should produce no violations");
    assert_eq!(report.total_derivations, 2);
    assert_eq!(report.total_accesses, 1);

    // Both derivations should have a valid origin root.
    for node in &report.provenance_forest {
        assert!(node.has_origin(), "Derivation {} should have an origin", node.derivation_id);
        assert_eq!(node.taint, TaintLevel::Trusted);
    }
}

#[test]
fn test_dangling_pointer() {
    // Scenario: alloc → free → offset → access  →  FreedRegionAccess violation.
    //
    // Region R1 is marked as freed (is_allocated = false).
    // D1: Direct from R1
    // A1: Read from D1 → targets a freed region.
    let mut v = OriginVerifier::new();
    let mut region = Region::new(rid(1), addr(0x1000), 256);
    region.is_allocated = false; // freed
    v.add_region(region);
    v.add_derivation(Derivation::new(
        did(1),
        DerivationSource::Region(rid(1)),
        DerivationKind::Direct,
        (addr(0x1000), addr(0x1100)),
    ));
    v.add_access(Access::new(
        vuma_ive::origin::AccessId(1),
        did(1),
        AccessKind::Read,
        4,
        "test.vu:20",
        true,
    ));

    let report = verify(&v);

    assert!(!report.is_clean(), "Access to freed region should produce a violation");
    assert!(
        report.violations.iter().any(|v| matches!(v.kind, ViolationKind::FreedRegionAccess { .. })),
        "Expected FreedRegionAccess violation, got: {:?}",
        report.violations
    );
}

#[test]
fn test_null_pointer() {
    // Scenario: Derivation from a fabricated source at address 0 (null).
    // This should be flagged as a FabricatedPointer violation.
    let mut v = OriginVerifier::new();
    // No region registered — null address has no backing allocation.
    v.add_derivation(Derivation::new(
        did(1),
        DerivationSource::Fabricated { raw_value: 0 },
        DerivationKind::Direct,
        (Address::NULL, addr(0x1)), // tiny range at null
    ));
    v.add_access(Access::new(
        vuma_ive::origin::AccessId(1),
        did(1),
        AccessKind::Read,
        1,
        "test.vu:30",
        true,
    ));

    let report = verify(&v);

    assert!(!report.is_clean(), "Null pointer access should produce a violation");
    assert!(
        report.violations.iter().any(|v| matches!(v.kind, ViolationKind::FabricatedPointer { .. })),
        "Expected FabricatedPointer violation, got: {:?}",
        report.violations
    );

    // The provenance node for D1 should be an orphan.
    let node = report.provenance_forest.iter().find(|n| n.derivation_id == did(1)).unwrap();
    assert!(node.is_orphan(), "Null pointer derivation should be an orphan");
}

#[test]
fn test_out_of_bounds() {
    // Scenario: Offset derivation goes beyond region bounds.
    //
    // Region R1 at 0x1000, size 64 (end = 0x1040).
    // D1: Direct from R1, range [0x1000, 0x1040) — valid.
    // D2: Offset(+80) from D1, range [0x1050, 0x1060) — exceeds region.
    let mut v = OriginVerifier::new();
    v.add_region(Region::new(rid(1), addr(0x1000), 64));
    v.add_derivation(Derivation::new(
        did(1),
        DerivationSource::Region(rid(1)),
        DerivationKind::Direct,
        (addr(0x1000), addr(0x1040)),
    ));
    v.add_derivation(Derivation::new(
        did(2),
        DerivationSource::AnotherDerivation(did(1)),
        DerivationKind::Offset { by: 80 },
        (addr(0x1050), addr(0x1060)), // past end of region
    ));

    let report = verify(&v);

    assert!(!report.is_clean(), "Out-of-bounds derivation should produce a violation");
    assert!(
        report.violations.iter().any(|v| matches!(v.kind, ViolationKind::OutOfBounds { .. })),
        "Expected OutOfBounds violation, got: {:?}",
        report.violations
    );
}

#[test]
fn test_valid_dereference_chain() {
    // Scenario: ptr → ptr → value, all valid.
    //
    // Region R1 at 0x2000, size 128.
    // D1: Direct from R1 (base pointer), range [0x2000, 0x2080)
    // D2: Offset(+8) from D1 (interior pointer), range [0x2008, 0x2010)
    // D3: Cast from D2 (*mut u8 → *mut u32), range [0x2008, 0x200C)
    // A1: Write to D3, initialized
    let mut v = OriginVerifier::new();
    v.add_region(Region::new(rid(1), addr(0x2000), 128));
    v.add_derivation(Derivation::new(
        did(1),
        DerivationSource::Region(rid(1)),
        DerivationKind::Direct,
        (addr(0x2000), addr(0x2080)),
    ));
    v.add_derivation(Derivation::new(
        did(2),
        DerivationSource::AnotherDerivation(did(1)),
        DerivationKind::Offset { by: 8 },
        (addr(0x2008), addr(0x2010)),
    ));
    v.add_derivation(Derivation::new(
        did(3),
        DerivationSource::AnotherDerivation(did(2)),
        DerivationKind::Cast {
            from_repr: "*mut u8".to_string(),
            to_repr: "*mut u32".to_string(),
        },
        (addr(0x2008), addr(0x200C)),
    ));
    v.add_access(Access::new(
        vuma_ive::origin::AccessId(1),
        did(3),
        AccessKind::Write,
        4,
        "test.vu:50",
        true,
    ));

    let report = verify(&v);

    assert!(report.is_clean(), "Valid ptr→ptr→value chain should produce no violations");

    // D3's provenance chain should be [D1, D2, D3].
    let node_d3 = report.provenance_forest.iter().find(|n| n.derivation_id == did(3)).unwrap();
    assert_eq!(node_d3.chain, vec![did(1), did(2), did(3)], "Chain should trace root→leaf");
    assert!(node_d3.has_origin());
    assert_eq!(node_d3.taint, TaintLevel::Trusted);

    // D2's chain should be [D1, D2].
    let node_d2 = report.provenance_forest.iter().find(|n| n.derivation_id == did(2)).unwrap();
    assert_eq!(node_d2.chain, vec![did(1), did(2)]);
}

// ===========================================================================
// Category 2: Provenance (5 tests)
// ===========================================================================

#[test]
fn test_provenance_graph_construction() {
    // Build a provenance graph with 2 regions and 4 derivations, then
    // verify the provenance forest has the correct number of nodes and
    // each node has the correct root.
    let mut v = OriginVerifier::new();
    v.add_region(Region::new(rid(1), addr(0x1000), 256));
    v.add_region(Region::new(rid(2), addr(0x2000), 128));

    // R1 derivations
    v.add_derivation(Derivation::new(
        did(1),
        DerivationSource::Region(rid(1)),
        DerivationKind::Direct,
        (addr(0x1000), addr(0x1100)),
    ));
    v.add_derivation(Derivation::new(
        did(2),
        DerivationSource::AnotherDerivation(did(1)),
        DerivationKind::Offset { by: 32 },
        (addr(0x1020), addr(0x1030)),
    ));

    // R2 derivations
    v.add_derivation(Derivation::new(
        did(3),
        DerivationSource::Region(rid(2)),
        DerivationKind::Direct,
        (addr(0x2000), addr(0x2080)),
    ));
    v.add_derivation(Derivation::new(
        did(4),
        DerivationSource::AnotherDerivation(did(3)),
        DerivationKind::Offset { by: 16 },
        (addr(0x2010), addr(0x2020)),
    ));

    let report = verify(&v);

    assert!(report.is_clean());
    assert_eq!(report.provenance_forest.len(), 4, "Should have 4 provenance nodes");

    // Verify roots
    let root_of = |report: &OriginReport, d: DerivationId| -> Option<OriginRoot> {
        report
            .provenance_forest
            .iter()
            .find(|n| n.derivation_id == d)
            .and_then(|n| n.root.clone())
    };

    // D1 and D2 should trace to R1
    let root_d1 = root_of(&report, did(1)).unwrap();
    let root_d2 = root_of(&report, did(2)).unwrap();
    assert!(matches!(root_d1, OriginRoot::AllocationSite { region_id: RegionId(1), .. }));
    assert!(matches!(root_d2, OriginRoot::AllocationSite { region_id: RegionId(1), .. }));

    // D3 and D4 should trace to R2
    let root_d3 = root_of(&report, did(3)).unwrap();
    let root_d4 = root_of(&report, did(4)).unwrap();
    assert!(matches!(root_d3, OriginRoot::AllocationSite { region_id: RegionId(2), .. }));
    assert!(matches!(root_d4, OriginRoot::AllocationSite { region_id: RegionId(2), .. }));
}

#[test]
fn test_provenance_reachability() {
    // A valid path from allocation to access through a multi-step derivation chain.
    // Verify that every derivation in the chain is reachable from the root.
    let mut v = OriginVerifier::new();
    v.add_region(Region::new(rid(1), addr(0x3000), 512));

    // Chain: D1 (Direct) → D2 (Offset) → D3 (Cast) → D4 (Offset)
    v.add_derivation(Derivation::new(
        did(1),
        DerivationSource::Region(rid(1)),
        DerivationKind::Direct,
        (addr(0x3000), addr(0x3200)),
    ));
    v.add_derivation(Derivation::new(
        did(2),
        DerivationSource::AnotherDerivation(did(1)),
        DerivationKind::Offset { by: 64 },
        (addr(0x3040), addr(0x3060)),
    ));
    v.add_derivation(Derivation::new(
        did(3),
        DerivationSource::AnotherDerivation(did(2)),
        DerivationKind::Cast {
            from_repr: "*mut u8".to_string(),
            to_repr: "*mut u64".to_string(),
        },
        (addr(0x3040), addr(0x3048)),
    ));
    v.add_derivation(Derivation::new(
        did(4),
        DerivationSource::AnotherDerivation(did(3)),
        DerivationKind::Offset { by: 8 },
        (addr(0x3048), addr(0x3050)),
    ));

    let report = verify(&v);

    assert!(report.is_clean(), "All derivations should be valid");

    // Every node should have origin and be trusted.
    for node in &report.provenance_forest {
        assert!(node.has_origin(), "Node {} should have an origin", node.derivation_id);
        assert_eq!(node.taint, TaintLevel::Trusted, "Node {} should be Trusted", node.derivation_id);
    }

    // D4's chain should be [D1, D2, D3, D4].
    let node_d4 = report.provenance_forest.iter().find(|n| n.derivation_id == did(4)).unwrap();
    assert_eq!(node_d4.chain.len(), 4, "D4 chain should have 4 elements (root to leaf)");
    assert_eq!(node_d4.chain[0], did(1), "Chain root should be D1");
    assert_eq!(node_d4.chain[3], did(4), "Chain leaf should be D4");
}

#[test]
fn test_provenance_unreachable() {
    // A derivation that references a non-existent region → orphan with no
    // reachable path from any allocation.
    let mut v = OriginVerifier::new();
    // No region registered — D1 references R99 which does not exist.
    v.add_derivation(Derivation::new(
        did(1),
        DerivationSource::Region(rid(99)),
        DerivationKind::Direct,
        (addr(0x5000), addr(0x5100)),
    ));

    let report = verify(&v);

    assert!(!report.is_clean(), "Unreachable derivation should produce a violation");

    // Should be flagged as an orphan (region R99 doesn't exist).
    assert!(
        report.violations.iter().any(|v| matches!(v.kind, ViolationKind::OrphanValue { .. })),
        "Expected OrphanValue violation for unreachable derivation, got: {:?}",
        report.violations
    );

    // The provenance node should be an orphan.
    let node = report.provenance_forest.iter().find(|n| n.derivation_id == did(1)).unwrap();
    assert!(node.is_orphan(), "Unreachable derivation should be an orphan");
    assert_eq!(node.taint, TaintLevel::Unknown);
}

#[test]
fn test_provenance_with_casts() {
    // Cast records should appear in the provenance chain.
    // Verify that a Cast DerivationKind is preserved in the chain.
    let mut v = OriginVerifier::new();
    v.add_region(Region::new(rid(1), addr(0x4000), 256));

    v.add_derivation(Derivation::new(
        did(1),
        DerivationSource::Region(rid(1)),
        DerivationKind::Direct,
        (addr(0x4000), addr(0x4100)),
    ));
    v.add_derivation(Derivation::new(
        did(2),
        DerivationSource::AnotherDerivation(did(1)),
        DerivationKind::Cast {
            from_repr: "*mut u8".to_string(),
            to_repr: "*mut u32".to_string(),
        },
        (addr(0x4000), addr(0x4004)),
    ));
    v.add_derivation(Derivation::new(
        did(3),
        DerivationSource::AnotherDerivation(did(2)),
        DerivationKind::Cast {
            from_repr: "*mut u32".to_string(),
            to_repr: "*mut u64".to_string(),
        },
        (addr(0x4000), addr(0x4008)),
    ));

    let report = verify(&v);

    assert!(report.is_clean(), "Valid cast chain should produce no violations");

    // Verify the original derivation kinds are preserved in the verifier's
    // internal state (we verify this indirectly through the chain structure).
    let node_d3 = report.provenance_forest.iter().find(|n| n.derivation_id == did(3)).unwrap();
    assert_eq!(node_d3.chain, vec![did(1), did(2), did(3)]);

    // All nodes should be Trusted since they trace to a valid allocation.
    for node in &report.provenance_forest {
        assert_eq!(node.taint, TaintLevel::Trusted);
    }
}

#[test]
fn test_forged_pointer_detection() {
    // Integer-to-pointer without a cast (i.e., DerivationSource::Fabricated).
    // This should be detected as a FabricatedPointer violation.
    let mut v = OriginVerifier::new();
    v.add_region(Region::new(rid(1), addr(0x1000), 256));

    // A legitimate derivation from R1.
    v.add_derivation(Derivation::new(
        did(1),
        DerivationSource::Region(rid(1)),
        DerivationKind::Direct,
        (addr(0x1000), addr(0x1100)),
    ));

    // A forged pointer — an integer 0xDEAD_BEEF cast to a pointer without
    // any backing allocation.
    v.add_derivation(Derivation::new(
        did(2),
        DerivationSource::Fabricated { raw_value: 0xDEAD_BEEF },
        DerivationKind::Direct,
        (addr(0xDEAD_BEEF), addr(0xDEAD_BF00)),
    ));

    // Access through the forged pointer.
    v.add_access(Access::new(
        vuma_ive::origin::AccessId(1),
        did(2),
        AccessKind::Read,
        4,
        "test.vu:100",
        true,
    ));

    let report = verify(&v);

    assert!(!report.is_clean(), "Forged pointer should produce a violation");
    assert!(
        report.violations.iter().any(|v| {
            matches!(v.kind, ViolationKind::FabricatedPointer { derivation_id, raw_value }
                if derivation_id == did(2) && raw_value == 0xDEAD_BEEF)
        }),
        "Expected FabricatedPointer violation for D2 with raw_value=0xDEAD_BEEF, got: {:?}",
        report.violations
    );

    // D2 should be tainted Unknown, D1 should be Trusted.
    let node_d1 = report.provenance_forest.iter().find(|n| n.derivation_id == did(1)).unwrap();
    let node_d2 = report.provenance_forest.iter().find(|n| n.derivation_id == did(2)).unwrap();
    assert_eq!(node_d1.taint, TaintLevel::Trusted);
    assert_eq!(node_d2.taint, TaintLevel::Unknown);
    assert!(node_d2.is_orphan());
}

// ===========================================================================
// Category 3: Advanced (5 tests)
// ===========================================================================

#[test]
fn test_stack_escape() {
    // Stack pointer used after function return.
    // Modelled as: a region that is freed (stack frame deallocated) followed
    // by an access through a derivation that still references it.
    let mut v = OriginVerifier::new();

    // Stack frame region — freed after function returns.
    let mut stack_region = Region::new(rid(1), addr(0x7FFF_0000), 1024);
    stack_region.is_allocated = false;
    v.add_region(stack_region);

    // D1: Direct pointer into the stack frame.
    v.add_derivation(Derivation::new(
        did(1),
        DerivationSource::Region(rid(1)),
        DerivationKind::Direct,
        (addr(0x7FFF_0000), addr(0x7FFF_0400)),
    ));

    // Access after return — uses the dangling stack pointer.
    v.add_access(Access::new(
        vuma_ive::origin::AccessId(1),
        did(1),
        AccessKind::Read,
        8,
        "test.vu:200",
        true,
    ));

    let report = verify(&v);

    assert!(!report.is_clean(), "Stack escape should produce a violation");
    assert!(
        report.violations.iter().any(|v| matches!(v.kind, ViolationKind::FreedRegionAccess { .. })),
        "Expected FreedRegionAccess violation for stack escape, got: {:?}",
        report.violations
    );
}

#[test]
fn test_wild_pointer() {
    // Access to an address outside all regions — modelled as a fabricated
    // pointer to an arbitrary address.
    let mut v = OriginVerifier::new();

    // Legitimate region at 0x1000.
    v.add_region(Region::new(rid(1), addr(0x1000), 256));

    // Wild pointer to 0xBEEF_0000 — not inside any region.
    v.add_derivation(Derivation::new(
        did(1),
        DerivationSource::Fabricated { raw_value: 0xBEEF_0000 },
        DerivationKind::Direct,
        (addr(0xBEEF_0000), addr(0xBEEF_0100)),
    ));

    v.add_access(Access::new(
        vuma_ive::origin::AccessId(1),
        did(1),
        AccessKind::Write,
        4,
        "test.vu:300",
        true,
    ));

    let report = verify(&v);

    assert!(!report.is_clean(), "Wild pointer should produce a violation");
    assert!(
        report.violations.iter().any(|v| matches!(v.kind, ViolationKind::FabricatedPointer { .. })),
        "Expected FabricatedPointer violation for wild pointer, got: {:?}",
        report.violations
    );

    // The wild pointer derivation should be an orphan.
    let node = report.provenance_forest.iter().find(|n| n.derivation_id == did(1)).unwrap();
    assert!(node.is_orphan());
    assert_eq!(node.taint, TaintLevel::Unknown);
}

#[test]
fn test_multiple_derivation_chains() {
    // Two independent derivation chains that converge on the same region,
    // then both are accessed. Both should verify cleanly.
    let mut v = OriginVerifier::new();
    v.add_region(Region::new(rid(1), addr(0x8000), 512));

    // Chain A: D1 → D2
    v.add_derivation(Derivation::new(
        did(1),
        DerivationSource::Region(rid(1)),
        DerivationKind::Direct,
        (addr(0x8000), addr(0x8200)),
    ));
    v.add_derivation(Derivation::new(
        did(2),
        DerivationSource::AnotherDerivation(did(1)),
        DerivationKind::Offset { by: 16 },
        (addr(0x8010), addr(0x8020)),
    ));

    // Chain B: D3 → D4 (same root region, different path)
    v.add_derivation(Derivation::new(
        did(3),
        DerivationSource::Region(rid(1)),
        DerivationKind::Direct,
        (addr(0x8000), addr(0x8200)),
    ));
    v.add_derivation(Derivation::new(
        did(4),
        DerivationSource::AnotherDerivation(did(3)),
        DerivationKind::Offset { by: 64 },
        (addr(0x8040), addr(0x8060)),
    ));

    // Both chains are accessed.
    v.add_access(Access::new(
        vuma_ive::origin::AccessId(1),
        did(2),
        AccessKind::Read,
        4,
        "test.vu:400",
        true,
    ));
    v.add_access(Access::new(
        vuma_ive::origin::AccessId(2),
        did(4),
        AccessKind::Write,
        8,
        "test.vu:401",
        true,
    ));

    let report = verify(&v);

    assert!(report.is_clean(), "Multiple independent chains to same region should be valid");

    // Both chains should trace to the same root region.
    let node_d2 = report.provenance_forest.iter().find(|n| n.derivation_id == did(2)).unwrap();
    let node_d4 = report.provenance_forest.iter().find(|n| n.derivation_id == did(4)).unwrap();

    assert!(matches!(node_d2.root, Some(OriginRoot::AllocationSite { region_id: RegionId(1), .. })));
    assert!(matches!(node_d4.root, Some(OriginRoot::AllocationSite { region_id: RegionId(1), .. })));

    // Chain A: D1→D2, Chain B: D3→D4
    assert_eq!(node_d2.chain, vec![did(1), did(2)]);
    assert_eq!(node_d4.chain, vec![did(3), did(4)]);
}

#[test]
fn test_cast_classification() {
    // Verify that explicit casts (DerivationKind::Cast) and implicit casts
    // (DerivationKind::Arithmetic) are both tracked correctly in the
    // provenance forest, and that both are treated as valid derivations
    // when they trace back to a valid allocation.
    let mut v = OriginVerifier::new();
    v.add_region(Region::new(rid(1), addr(0x6000), 256));

    // Explicit cast: *mut u8 → *mut u32
    v.add_derivation(Derivation::new(
        did(1),
        DerivationSource::Region(rid(1)),
        DerivationKind::Direct,
        (addr(0x6000), addr(0x6100)),
    ));
    v.add_derivation(Derivation::new(
        did(2),
        DerivationSource::AnotherDerivation(did(1)),
        DerivationKind::Cast {
            from_repr: "*mut u8".to_string(),
            to_repr: "*mut u32".to_string(),
        },
        (addr(0x6000), addr(0x6004)),
    ));

    // Implicit arithmetic: pointer arithmetic described as a general operation
    v.add_derivation(Derivation::new(
        did(3),
        DerivationSource::AnotherDerivation(did(1)),
        DerivationKind::Arithmetic {
            description: "ptr.add(align_of::<u64>())".to_string(),
        },
        (addr(0x6008), addr(0x6010)),
    ));

    let report = verify(&v);

    assert!(report.is_clean(), "Both explicit and implicit casts should be valid");

    // All three derivations should trace to R1.
    for node in &report.provenance_forest {
        assert!(matches!(node.root, Some(OriginRoot::AllocationSite { region_id: RegionId(1), .. })));
        assert_eq!(node.taint, TaintLevel::Trusted);
    }

    // Verify chains include the cast steps.
    let node_d2 = report.provenance_forest.iter().find(|n| n.derivation_id == did(2)).unwrap();
    let node_d3 = report.provenance_forest.iter().find(|n| n.derivation_id == did(3)).unwrap();
    assert_eq!(node_d2.chain, vec![did(1), did(2)]);
    assert_eq!(node_d3.chain, vec![did(1), did(3)]);
}

#[test]
fn test_provenance_with_offsets() {
    // Offset derivations are tracked correctly in the provenance chain,
    // including the offset amount in DerivationKind::Offset.
    // Also verify that out-of-bounds offsets are detected while in-bounds
    // offsets are accepted.
    let mut v = OriginVerifier::new();
    v.add_region(Region::new(rid(1), addr(0x9000), 128));

    // D1: base pointer from R1
    v.add_derivation(Derivation::new(
        did(1),
        DerivationSource::Region(rid(1)),
        DerivationKind::Direct,
        (addr(0x9000), addr(0x9080)),
    ));

    // D2: offset +16 (in-bounds)
    v.add_derivation(Derivation::new(
        did(2),
        DerivationSource::AnotherDerivation(did(1)),
        DerivationKind::Offset { by: 16 },
        (addr(0x9010), addr(0x9020)),
    ));

    // D3: offset +32 from D2 (i.e., base+48, in-bounds)
    v.add_derivation(Derivation::new(
        did(3),
        DerivationSource::AnotherDerivation(did(2)),
        DerivationKind::Offset { by: 32 },
        (addr(0x9030), addr(0x9040)),
    ));

    // D4: offset +200 from D1 (out-of-bounds, past region end 0x9080)
    v.add_derivation(Derivation::new(
        did(4),
        DerivationSource::AnotherDerivation(did(1)),
        DerivationKind::Offset { by: 200 },
        (addr(0x90C8), addr(0x90D0)), // 0x9000 + 200 = 0x90C8
    ));

    let report = verify(&v);

    // Should have exactly one OutOfBounds violation for D4.
    assert!(!report.is_clean(), "Out-of-bounds offset should produce a violation");
    let oob_violations: Vec<_> = report
        .violations
        .iter()
        .filter(|v| matches!(v.kind, ViolationKind::OutOfBounds { .. }))
        .collect();
    assert_eq!(oob_violations.len(), 1, "Expected exactly 1 OutOfBounds violation");

    // Verify that D1, D2, D3 are all clean (trusted).
    for did_val in [did(1), did(2), did(3)] {
        let node = report.provenance_forest.iter().find(|n| n.derivation_id == did_val).unwrap();
        assert_eq!(node.taint, TaintLevel::Trusted, "D{} should be Trusted", did_val.0);
    }

    // Verify the chain for D3 includes all offset steps.
    let node_d3 = report.provenance_forest.iter().find(|n| n.derivation_id == did(3)).unwrap();
    assert_eq!(node_d3.chain, vec![did(1), did(2), did(3)]);

    // D4 should still have a valid root (it traces to R1) even though
    // its provenance range is out of bounds — the violation is OutOfBounds,
    // not OrphanValue.
    let node_d4 = report.provenance_forest.iter().find(|n| n.derivation_id == did(4)).unwrap();
    assert!(node_d4.has_origin(), "D4 should still trace to R1 despite being out of bounds");
    assert_eq!(node_d4.taint, TaintLevel::Trusted);
}
