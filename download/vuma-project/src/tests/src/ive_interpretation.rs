//! Integration tests for the InterpretationVerifier
//!
//! This test suite covers four categories of interpretation invariant checks:
//!
//! 1. **RepD Compatibility** (5 tests) — size, alignment, structural shape
//! 2. **CapD Transitions** (5 tests) — weakening, strengthening, empty meet
//! 3. **Type Confusion & Pointer Reinterpretation** (5 tests) — cross-kind reads
//! 4. **Uninitialized Reads & RelD** (5 tests) — missing writes, relation preservation

use vuma_ive::interpretation::{
    InterpretationVerifier, InterpretationViolation, LocationId, ProgramPointId, byte_repd,
    capd_with, empty_reld, make_bd, reld_with,
};
use vuma_ive::result::VerificationStatus;
use vuma_bd::capd::Capability;
use vuma_bd::repd::{ByteRep, FuncRep, PtrRep, RepD, StructRep};
use vuma_bd::reld::{Relation, TemporalKind};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Standard read-write capability set used as default in tests.
fn rw_capd() -> vuma_bd::capd::CapD {
    capd_with(&[Capability::Read, Capability::Write])
}

/// Read-only capability set.
fn read_capd() -> vuma_bd::capd::CapD {
    capd_with(&[Capability::Read])
}

/// Create a pointer RepD (size=8, align=8).
fn ptr_repd() -> RepD {
    RepD::Ptr(PtrRep {
        pointee: Box::new(RepD::Byte(ByteRep { size: 1, align: 1 })),
    })
}

/// Create a function RepD (size=8, align=8).
fn func_repd() -> RepD {
    RepD::Func(FuncRep {
        params: vec![],
        result: Box::new(RepD::Byte(ByteRep { size: 1, align: 1 })),
    })
}

/// Create a struct RepD with one i64-like field (size=8, align=8).
fn struct_i64_repd() -> RepD {
    RepD::Struct(StructRep {
        fields: vec![(0, RepD::Byte(ByteRep { size: 8, align: 8 }))],
        total_size: 8,
        align: 8,
    })
}

/// Create a struct RepD with two i32-like fields (size=8, align=4).
fn struct_two_i32_repd() -> RepD {
    RepD::Struct(StructRep {
        fields: vec![
            (0, RepD::Byte(ByteRep { size: 4, align: 4 })),
            (4, RepD::Byte(ByteRep { size: 4, align: 4 })),
        ],
        total_size: 8,
        align: 4,
    })
}

/// Create a struct RepD with three fields (size=8, align=4) — different layout.
fn struct_three_fields_repd() -> RepD {
    RepD::Struct(StructRep {
        fields: vec![
            (0, RepD::Byte(ByteRep { size: 2, align: 2 })),
            (2, RepD::Byte(ByteRep { size: 2, align: 2 })),
            (4, RepD::Byte(ByteRep { size: 4, align: 4 })),
        ],
        total_size: 8,
        align: 4,
    })
}

/// Shorthand for creating a LocationId.
fn loc(id: u64) -> LocationId {
    LocationId(id)
}

/// Shorthand for creating a ProgramPointId.
fn pp(id: u64) -> ProgramPointId {
    ProgramPointId(id)
}

// ===========================================================================
// Category 1: RepD Compatibility (5 tests)
// ===========================================================================

// Test 1: Matching byte RepDs — write and read with identical BDs should pass.
#[test]
fn test_matching_byte_repd() {
    let mut verifier = InterpretationVerifier::new();
    let repd = byte_repd(4, 4);
    let capd = rw_capd();
    let reld = empty_reld();
    let bd = make_bd(repd, capd, reld);

    verifier.record_write(loc(1), bd.clone(), pp(1));
    verifier.record_read(loc(1), bd.clone(), pp(2));

    let result = verifier.verify();
    assert!(
        result.is_proven(),
        "expected Proven for matching byte RepD, got {:?}",
        result.status
    );
}

// Test 2: Size mismatch — different sizes should produce IncompatibleRepD.
#[test]
fn test_size_mismatch() {
    let mut verifier = InterpretationVerifier::new();
    let write_bd = make_bd(byte_repd(8, 1), rw_capd(), empty_reld());
    let read_bd = make_bd(byte_repd(4, 1), rw_capd(), empty_reld());

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    let violations = verifier.verify_detailed();
    assert!(
        !violations.is_empty(),
        "expected violations for size mismatch"
    );

    let has_incompatible_repd = violations.iter().any(|v| {
        matches!(v, InterpretationViolation::IncompatibleRepD { .. })
    });
    assert!(
        has_incompatible_repd,
        "expected IncompatibleRepD violation, got {:?}",
        violations
    );
}

// Test 3: Alignment mismatch — Byte(4,8) written, Byte(4,2) read.
// The alignment divisor rule (write_align % read_align == 0) passes, but
// RepD::compatible() requires exact alignment match, so the result is
// still a RepD incompatibility.
#[test]
fn test_alignment_mismatch() {
    let mut verifier = InterpretationVerifier::new();
    let write_bd = make_bd(byte_repd(4, 8), rw_capd(), empty_reld());
    let read_bd = make_bd(byte_repd(4, 2), rw_capd(), empty_reld());

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    let violations = verifier.verify_detailed();
    assert!(
        !violations.is_empty(),
        "expected violations for alignment mismatch"
    );

    // The violation should be RepD-related (IncompatibleRepD since
    // compatible() requires exact alignment match).
    let has_repd_violation = violations.iter().any(|v| {
        matches!(v, InterpretationViolation::IncompatibleRepD { .. })
            || matches!(v, InterpretationViolation::TypeConfusion { .. })
            || matches!(v, InterpretationViolation::PointerReinterpretation { .. })
    });
    assert!(
        has_repd_violation,
        "expected RepD-related violation, got {:?}",
        violations
    );
}

// Test 4: Matching struct RepDs — identical struct layout should pass.
#[test]
fn test_struct_repd_match() {
    let mut verifier = InterpretationVerifier::new();
    let struct_rep = struct_two_i32_repd();
    let bd = make_bd(struct_rep, rw_capd(), empty_reld());

    verifier.record_write(loc(1), bd.clone(), pp(1));
    verifier.record_read(loc(1), bd.clone(), pp(2));

    let result = verifier.verify();
    assert!(
        result.is_proven(),
        "expected Proven for matching struct RepD, got {:?}",
        result.status
    );
}

// Test 5: Pointer vs integer — write Ptr, read as non-Byte non-Ptr.
// The implementation detects Ptr→Struct as PointerReinterpretation
// (checked before TypeConfusion in the verification priority order).
#[test]
fn test_pointer_vs_integer() {
    let mut verifier = InterpretationVerifier::new();
    let write_bd = make_bd(ptr_repd(), rw_capd(), empty_reld());
    let read_bd = make_bd(struct_i64_repd(), rw_capd(), empty_reld());

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    // Overall result should be Violated
    let result = verifier.verify();
    assert!(
        result.is_violated(),
        "expected Violated for Ptr vs Struct, got {:?}",
        result.status
    );

    // Verify the specific violation type
    let violations = verifier.verify_detailed();
    let has_ptr_reinterp = violations.iter().any(|v| {
        matches!(v, InterpretationViolation::PointerReinterpretation { .. })
    });
    assert!(
        has_ptr_reinterp,
        "expected PointerReinterpretation violation, got {:?}",
        violations
    );
}

// ===========================================================================
// Category 2: CapD Transitions (5 tests)
// ===========================================================================

// Test 6: CapD weakening is safe — write with {Read,Write}, read with {Read}.
#[test]
fn test_capd_weakening_safe() {
    let mut verifier = InterpretationVerifier::new();
    let write_capd = rw_capd();
    let read_capd = read_capd();
    let repd = byte_repd(4, 4);

    let write_bd = make_bd(repd.clone(), write_capd, empty_reld());
    let read_bd = make_bd(repd, read_capd, empty_reld());

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    let result = verifier.verify();
    assert!(
        result.is_proven(),
        "expected Proven for CapD weakening, got {:?}",
        result.status
    );
}

// Test 7: Same CapD on both write and read is safe.
#[test]
fn test_capd_same_safe() {
    let mut verifier = InterpretationVerifier::new();
    let capd = rw_capd();
    let bd = make_bd(byte_repd(4, 4), capd, empty_reld());

    verifier.record_write(loc(1), bd.clone(), pp(1));
    verifier.record_read(loc(1), bd.clone(), pp(2));

    let result = verifier.verify();
    assert!(
        result.is_proven(),
        "expected Proven for same CapD, got {:?}",
        result.status
    );
}

// Test 8: CapD strengthening — read has more capabilities than write.
// With default settings (allow_strengthening_with_proof: true), this
// results in ProbablySafe (pending proof obligation).
#[test]
fn test_capd_strengthening_needs_proof() {
    let mut verifier = InterpretationVerifier::new();
    let write_capd = read_capd();
    let read_capd = rw_capd();
    let repd = byte_repd(4, 4);

    let write_bd = make_bd(repd.clone(), write_capd, empty_reld());
    let read_bd = make_bd(repd, read_capd, empty_reld());

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    let result = verifier.verify();
    assert!(
        matches!(result.status, VerificationStatus::ProbablySafe { .. }),
        "expected ProbablySafe for CapD strengthening, got {:?}",
        result.status
    );
}

// Test 9: Empty capability meet — no shared capabilities between write and read.
#[test]
fn test_capd_empty_meet() {
    let mut verifier = InterpretationVerifier::new();
    let write_capd = capd_with(&[Capability::Read]);
    let read_capd = capd_with(&[Capability::Write]);
    let repd = byte_repd(4, 4);

    let write_bd = make_bd(repd.clone(), write_capd, empty_reld());
    let read_bd = make_bd(repd, read_capd, empty_reld());

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    let violations = verifier.verify_detailed();
    let has_empty_meet = violations.iter().any(|v| {
        matches!(v, InterpretationViolation::EmptyCapabilityMeet { .. })
    });
    assert!(
        has_empty_meet,
        "expected EmptyCapabilityMeet, got {:?}",
        violations
    );
}

// Test 10: Incomparable CapDs — some capabilities added, some removed.
// With strengthening proof disallowed, this is flagged as invalid.
#[test]
fn test_capd_incomparable() {
    let mut verifier = InterpretationVerifier::new().with_strengthening_proof(false);
    let write_capd = capd_with(&[Capability::Read, Capability::Write]);
    let read_capd = capd_with(&[Capability::Read, Capability::Execute]);
    let repd = byte_repd(4, 4);

    let write_bd = make_bd(repd.clone(), write_capd, empty_reld());
    let read_bd = make_bd(repd, read_capd, empty_reld());

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    let violations = verifier.verify_detailed();
    let has_invalid_strengthening = violations.iter().any(|v| {
        matches!(v, InterpretationViolation::InvalidCapDStrengthening { .. })
    });
    assert!(
        has_invalid_strengthening,
        "expected InvalidCapDStrengthening for incomparable CapDs, got {:?}",
        violations
    );
}

// ===========================================================================
// Category 3: Type Confusion & Pointer Reinterpretation (5 tests)
// ===========================================================================

// Test 11: Pointer to integer confusion — write Ptr, read as integer struct.
// Detected as PointerReinterpretation (Ptr→non-Ptr,non-Byte).
#[test]
fn test_pointer_to_integer_confusion() {
    let mut verifier = InterpretationVerifier::new();
    let write_bd = make_bd(ptr_repd(), rw_capd(), empty_reld());
    let read_bd = make_bd(struct_i64_repd(), rw_capd(), empty_reld());

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    let violations = verifier.verify_detailed();
    let has_ptr_reinterp = violations.iter().any(|v| {
        matches!(v, InterpretationViolation::PointerReinterpretation { .. })
    });
    assert!(
        has_ptr_reinterp,
        "expected PointerReinterpretation for Ptr→Struct, got {:?}",
        violations
    );
}

// Test 12: Integer to pointer — write struct, read as Ptr.
// Detected as PointerReinterpretation (non-Ptr,non-Byte→Ptr).
#[test]
fn test_integer_to_pointer_suspicious() {
    let mut verifier = InterpretationVerifier::new();
    let write_bd = make_bd(struct_i64_repd(), rw_capd(), empty_reld());
    let read_bd = make_bd(ptr_repd(), rw_capd(), empty_reld());

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    let violations = verifier.verify_detailed();
    let has_ptr_reinterp = violations.iter().any(|v| {
        matches!(v, InterpretationViolation::PointerReinterpretation { .. })
    });
    assert!(
        has_ptr_reinterp,
        "expected PointerReinterpretation for Struct→Ptr, got {:?}",
        violations
    );
}

// Test 13: Byte is universal — write Ptr, read as Byte → OK.
#[test]
fn test_byte_universal() {
    let mut verifier = InterpretationVerifier::new();
    let write_bd = make_bd(ptr_repd(), rw_capd(), empty_reld());
    let read_bd = make_bd(byte_repd(8, 8), rw_capd(), empty_reld());

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    let result = verifier.verify();
    assert!(
        result.is_proven(),
        "expected Proven for Ptr→Byte (Byte is universal), got {:?}",
        result.status
    );
}

// Test 14: Function pointer confusion — write Func, read as Struct → TypeConfusion.
#[test]
fn test_func_ptr_confusion() {
    let mut verifier = InterpretationVerifier::new();
    let write_bd = make_bd(func_repd(), rw_capd(), empty_reld());
    let read_bd = make_bd(struct_i64_repd(), rw_capd(), empty_reld());

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    let violations = verifier.verify_detailed();
    let has_type_confusion = violations.iter().any(|v| {
        matches!(v, InterpretationViolation::TypeConfusion { .. })
    });
    assert!(
        has_type_confusion,
        "expected TypeConfusion for Func→Struct, got {:?}",
        violations
    );
}

// Test 15: Same-size struct with different field layout → IncompatibleRepD.
#[test]
fn test_same_struct_different_layout() {
    let mut verifier = InterpretationVerifier::new();
    let write_bd = make_bd(struct_two_i32_repd(), rw_capd(), empty_reld());
    let read_bd = make_bd(struct_three_fields_repd(), rw_capd(), empty_reld());

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    let violations = verifier.verify_detailed();
    assert!(
        !violations.is_empty(),
        "expected violations for different struct layouts"
    );

    let has_incompatible_repd = violations.iter().any(|v| {
        matches!(v, InterpretationViolation::IncompatibleRepD { .. })
    });
    assert!(
        has_incompatible_repd,
        "expected IncompatibleRepD for different struct layouts, got {:?}",
        violations
    );
}

// ===========================================================================
// Category 4: Uninitialized Reads & RelD (5 tests)
// ===========================================================================

// Test 16: Read without any prior write → UninitializedRead.
#[test]
fn test_uninitialized_read() {
    let mut verifier = InterpretationVerifier::new();
    let read_bd = make_bd(byte_repd(4, 4), rw_capd(), empty_reld());

    verifier.record_read(loc(1), read_bd, pp(1));

    let violations = verifier.verify_detailed();
    let has_uninit = violations.iter().any(|v| {
        matches!(v, InterpretationViolation::UninitializedRead { .. })
    });
    assert!(
        has_uninit,
        "expected UninitializedRead, got {:?}",
        violations
    );
}

// Test 17: Write then Read at same location → no UninitializedRead.
#[test]
fn test_initialized_after_write() {
    let mut verifier = InterpretationVerifier::new();
    let bd = make_bd(byte_repd(4, 4), rw_capd(), empty_reld());

    verifier.record_write(loc(1), bd.clone(), pp(1));
    verifier.record_read(loc(1), bd.clone(), pp(2));

    let violations = verifier.verify_detailed();
    let has_uninit = violations.iter().any(|v| {
        matches!(v, InterpretationViolation::UninitializedRead { .. })
    });
    assert!(
        !has_uninit,
        "should not have UninitializedRead after write, got {:?}",
        violations
    );

    let result = verifier.verify();
    assert!(
        result.is_proven(),
        "expected Proven for write-then-read, got {:?}",
        result.status
    );
}

// Test 18: Compatible RelDs — same Liveness relation on both sides → Proven.
#[test]
fn test_reld_preservation() {
    let mut verifier = InterpretationVerifier::new();
    let write_reld = reld_with(&[Relation::Liveness]);
    let read_reld = reld_with(&[Relation::Liveness]);

    let write_bd = make_bd(byte_repd(4, 4), rw_capd(), write_reld);
    let read_bd = make_bd(byte_repd(4, 4), rw_capd(), read_reld);

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    let result = verifier.verify();
    assert!(
        result.is_proven(),
        "expected Proven for compatible RelDs, got {:?}",
        result.status
    );
}

// Test 19: Inconsistent composed RelD — Temporal(Outlives) + Temporal(Succeeds)
// is contradictory → RelDNotPreserved.
#[test]
fn test_reld_inconsistent() {
    let mut verifier = InterpretationVerifier::new();
    let write_reld = reld_with(&[Relation::Temporal(TemporalKind::Outlives)]);
    let read_reld = reld_with(&[Relation::Temporal(TemporalKind::Succeeds)]);

    let write_bd = make_bd(byte_repd(4, 4), rw_capd(), write_reld);
    let read_bd = make_bd(byte_repd(4, 4), rw_capd(), read_reld);

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    let violations = verifier.verify_detailed();
    let has_reld_violation = violations.iter().any(|v| {
        matches!(v, InterpretationViolation::RelDNotPreserved { .. })
    });
    assert!(
        has_reld_violation,
        "expected RelDNotPreserved, got {:?}",
        violations
    );
}

// Test 20: Multiple write-read pairs across different locations with mixed results.
#[test]
fn test_multiple_write_read_pairs() {
    let mut verifier = InterpretationVerifier::new();

    // Location 1: matching BDs → OK
    let bd_ok = make_bd(byte_repd(4, 4), rw_capd(), empty_reld());
    verifier.record_write(loc(1), bd_ok.clone(), pp(1));
    verifier.record_read(loc(1), bd_ok.clone(), pp(2));

    // Location 2: size mismatch → IncompatibleRepD
    let write_bd_bad = make_bd(byte_repd(8, 1), rw_capd(), empty_reld());
    let read_bd_bad = make_bd(byte_repd(4, 1), rw_capd(), empty_reld());
    verifier.record_write(loc(2), write_bd_bad, pp(3));
    verifier.record_read(loc(2), read_bd_bad, pp(4));

    // Location 3: uninitialized read
    let read_bd_uninit = make_bd(byte_repd(4, 4), rw_capd(), empty_reld());
    verifier.record_read(loc(3), read_bd_uninit, pp(5));

    let violations = verifier.verify_detailed();

    // Should have at least an IncompatibleRepD and an UninitializedRead
    let has_repd_violation = violations.iter().any(|v| {
        matches!(v, InterpretationViolation::IncompatibleRepD { .. })
            || matches!(v, InterpretationViolation::PointerReinterpretation { .. })
            || matches!(v, InterpretationViolation::TypeConfusion { .. })
    });
    let has_uninit = violations.iter().any(|v| {
        matches!(v, InterpretationViolation::UninitializedRead { .. })
    });

    assert!(
        has_repd_violation,
        "expected RepD-related violation for location 2, got {:?}",
        violations
    );
    assert!(
        has_uninit,
        "expected UninitializedRead for location 3, got {:?}",
        violations
    );

    // Overall result should be Violated
    let result = verifier.verify();
    assert!(
        result.is_violated(),
        "expected Violated overall, got {:?}",
        result.status
    );
}
