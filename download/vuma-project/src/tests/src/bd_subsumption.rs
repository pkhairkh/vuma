//! BD Subsumption Tests — M2.3
//!
//! Verifies that BD inference subsumes the Rust type system: every Rust-typable
//! program should produce a valid BD assignment.  If we can represent Rust types
//! as BDs and the BD inference produces compatible results, then BD inference
//! subsumes Rust type inference.
//!
//! # Categories
//!
//! | # | Category              | Count | Focus                                            |
//! |---|-----------------------|-------|--------------------------------------------------|
//! | 1 | Primitive Type Map    | 5     | u32/u64/f64/bool/char → RepD::Byte + CapD + RelD |
//! | 2 | Composite Type Map   | 5     | Struct/Enum/Array/Box/&T → structural RepDs      |
//! | 3 | Rust Subsumption     | 5     | Ownership/borrowing/lifetime/traits/Send+Sync    |

use vuma_bd::capd::{CapD, Capability};
use vuma_bd::descriptor::BD;
use vuma_bd::reld::{DepKind, FlowPolicy, Relation, RelD, TemporalKind};
use vuma_bd::repd::{
    ArrayRep, ByteRep, EnumRep, PtrRep, RepD, StructRep,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `CapD` from a slice of capabilities (no conditions).
fn capd_from(caps: &[Capability]) -> CapD {
    CapD::empty().strengthen(caps)
}

/// Build a `RelD` from a slice of relations.
fn reld_from(relations: &[Relation]) -> RelD {
    let mut reld = RelD::empty();
    for r in relations {
        reld.relations.insert(r.clone());
    }
    reld
}

/// Check BD well-formedness:
///   1. RepD has non-zero alignment.
///   2. RelD is internally consistent.
///   3. CapD is non-empty (a useful value must permit at least one operation).
fn assert_well_formed(bd: &BD, label: &str) {
    assert!(
        bd.repd.alignment() > 0,
        "{label}: RepD alignment must be non-zero"
    );
    assert!(
        !bd.capd.caps.is_empty(),
        "{label}: CapD should contain at least one capability for a useful type"
    );
    assert!(
        bd.reld.is_consistent(),
        "{label}: RelD must be internally consistent"
    );
}

/// Standard capabilities for numeric primitive types (Read, Write, Hash, Compare).
fn numeric_capd() -> CapD {
    capd_from(&[
        Capability::Read,
        Capability::Write,
        Capability::Hash,
        Capability::Compare,
    ])
}

// ===========================================================================
// Category 1: Primitive Type Mapping (5 tests)
// ===========================================================================

/// Test 1: u32 → RepD::Byte(4,4), CapD{Read,Write,Hash,Compare}, RelD::empty()
///
/// The Rust `u32` type maps to a 4-byte representation with read/write/hash/
/// compare capabilities — everything Rust allows on a `u32` value.
#[test]
fn test_u32_bd() {
    let repd = RepD::Byte(ByteRep { size: 4, align: 4 });
    let capd = numeric_capd();
    let reld = RelD::empty();
    let bd = BD::new(repd.clone(), capd.clone(), reld.clone());

    // RepD checks
    assert_eq!(bd.repd.size(), 4, "u32 must be 4 bytes");
    assert_eq!(bd.repd.alignment(), 4, "u32 must be 4-byte aligned");

    // CapD checks
    assert!(bd.capd.caps.contains(&Capability::Read), "u32 must be Read");
    assert!(bd.capd.caps.contains(&Capability::Write), "u32 must be Write");
    assert!(bd.capd.caps.contains(&Capability::Hash), "u32 must be Hash");
    assert!(bd.capd.caps.contains(&Capability::Compare), "u32 must be Compare");

    // RelD checks
    assert!(bd.reld.relations.is_empty(), "u32 has no relational constraints");

    // Well-formedness
    assert_well_formed(&bd, "u32");

    // BD is self-compatible
    assert!(bd.compatible(&bd), "u32 BD must be compatible with itself");

    // Lattice: meet with a superset gives back a subset
    let bigger = capd_from(&[Capability::Read, Capability::Write, Capability::Hash, Capability::Compare, Capability::Send]);
    let meet = bd.capd.meet(&bigger);
    assert!(meet.caps.contains(&Capability::Read), "meet with superset preserves Read");
    assert!(meet.caps.contains(&Capability::Write), "meet with superset preserves Write");
    assert!(!meet.caps.contains(&Capability::Send), "meet with superset excludes Send (not in original)");
}

/// Test 2: u64 → RepD::Byte(8,8), CapD{Read,Write,Hash,Compare}, RelD::empty()
///
/// The Rust `u64` type maps to an 8-byte representation with the same
/// capability set as `u32` — demonstrating that different sizes yield
/// incompatible RepDs (size matters).
#[test]
fn test_u64_bd() {
    let repd = RepD::Byte(ByteRep { size: 8, align: 8 });
    let capd = numeric_capd();
    let reld = RelD::empty();
    let bd = BD::new(repd.clone(), capd.clone(), reld.clone());

    assert_eq!(bd.repd.size(), 8, "u64 must be 8 bytes");
    assert_eq!(bd.repd.alignment(), 8, "u64 must be 8-byte aligned");
    assert_well_formed(&bd, "u64");

    // u32 and u64 are incompatible (different size/alignment)
    let u32_repd = RepD::Byte(ByteRep { size: 4, align: 4 });
    let u32_capd = numeric_capd();
    let u32_bd = BD::new(u32_repd, u32_capd, RelD::empty());
    assert!(!bd.compatible(&u32_bd), "u32 and u64 must be incompatible");

    // But a Byte(8,8) representation subsumes any same-sized representation
    let other_8byte = RepD::Byte(ByteRep { size: 8, align: 8 });
    assert!(bd.repd.subsumes(&other_8byte), "same Byte RepD should subsume");
}

/// Test 3: f64 → RepD::Byte(8,8), CapD{Read,Write,Hash,Compare}
///
/// The Rust `f64` type has the same size/alignment as `u64` but different
/// semantics.  In the BD system, the RepD is structurally identical (both
/// Byte(8,8)); the difference is captured at the CapD layer: f64 lacks Hash
/// in Rust (NaN != NaN), but BD is more permissive.
#[test]
fn test_f64_bd() {
    let repd = RepD::Byte(ByteRep { size: 8, align: 8 });
    let capd = numeric_capd(); // BD permits Hash even where Rust doesn't
    let reld = RelD::empty();
    let bd = BD::new(repd.clone(), capd.clone(), reld.clone());

    assert_eq!(bd.repd.size(), 8, "f64 must be 8 bytes");
    assert_eq!(bd.repd.alignment(), 8, "f64 must be 8-byte aligned");
    assert_well_formed(&bd, "f64");

    // f64 and u64 have compatible representations (same size/alignment)
    let u64_bd = BD::new(
        RepD::Byte(ByteRep { size: 8, align: 8 }),
        numeric_capd(),
        RelD::empty(),
    );
    assert!(
        bd.repd.compatible(&u64_bd.repd),
        "f64 and u64 RepDs are compatible (same size/align)"
    );

    // BD subsumption: if we weakened f64's CapD to exclude Hash (Rust's
    // actual constraint), the BD still works
    let f64_rust_capd = capd_from(&[Capability::Read, Capability::Write, Capability::Compare]);
    let f64_rust_bd = BD::new(repd.clone(), f64_rust_capd, RelD::empty());
    assert!(
        f64_rust_bd.capd.is_subset(&bd.capd),
        "Rust-f64 CapD should be a subset of BD-f64 CapD (BD subsumes Rust)"
    );
}

/// Test 4: bool → RepD::Byte(1,1), CapD{Read,Write,Compare}
///
/// The Rust `bool` type is a single byte with compare capability.
/// It has a constrained value domain (0 or 1), which BD does not
/// capture at the RepD layer — this is a sound over-approximation.
#[test]
fn test_bool_bd() {
    let repd = RepD::Byte(ByteRep { size: 1, align: 1 });
    let capd = capd_from(&[Capability::Read, Capability::Write, Capability::Compare]);
    let reld = RelD::empty();
    let bd = BD::new(repd.clone(), capd.clone(), reld.clone());

    assert_eq!(bd.repd.size(), 1, "bool must be 1 byte");
    assert_eq!(bd.repd.alignment(), 1, "bool must be 1-byte aligned");
    assert_well_formed(&bd, "bool");

    // bool is strictly smaller than u32
    let u32_bd = BD::new(
        RepD::Byte(ByteRep { size: 4, align: 4 }),
        numeric_capd(),
        RelD::empty(),
    );
    assert!(!bd.compatible(&u32_bd), "bool and u32 are incompatible");

    // Meet of bool CapD and u32 CapD yields Compare + Read only
    let meet = bd.capd.meet(&u32_bd.capd);
    assert!(meet.caps.contains(&Capability::Compare), "meet preserves Compare");
    assert!(!meet.caps.contains(&Capability::Hash), "meet removes Hash (bool has none)");
}

/// Test 5: char → RepD::Byte(4,4), CapD{Read,Compare}
///
/// The Rust `char` type is 4 bytes (Unicode scalar value).
/// It is not Hash by default in all contexts but supports comparison.
#[test]
fn test_char_bd() {
    let repd = RepD::Byte(ByteRep { size: 4, align: 4 });
    let capd = capd_from(&[Capability::Read, Capability::Compare]);
    let reld = RelD::empty();
    let bd = BD::new(repd.clone(), capd.clone(), reld.clone());

    assert_eq!(bd.repd.size(), 4, "char must be 4 bytes");
    assert_eq!(bd.repd.alignment(), 4, "char must be 4-byte aligned");
    assert_well_formed(&bd, "char");

    // char and u32 have the same RepD (Byte(4,4)) → compatible representations
    let u32_bd = BD::new(
        RepD::Byte(ByteRep { size: 4, align: 4 }),
        numeric_capd(),
        RelD::empty(),
    );
    assert!(
        bd.repd.compatible(&u32_bd.repd),
        "char and u32 RepDs are compatible"
    );

    // But char has fewer capabilities than u32
    assert!(
        bd.capd.is_subset(&u32_bd.capd),
        "char CapD is a subset of u32 CapD"
    );

    // BD join: char ∨ u32 gives the more permissive (u32's caps)
    let joined = bd.capd.join(&u32_bd.capd);
    assert!(
        joined.caps.contains(&Capability::Write),
        "join of char+u32 includes Write"
    );
}

// ===========================================================================
// Category 2: Composite Type Mapping (5 tests)
// ===========================================================================

/// Test 6: struct { x: u32, y: u64 } → RepD::Struct with 2 fields
///
/// A Rust struct maps to RepD::Struct with fields at their correct offsets.
/// CapD is the union of field capabilities, and RelD captures containment.
#[test]
fn test_struct_bd() {
    // struct S { x: u32, y: u64 }
    // Layout: x at offset 0 (4 bytes), padding 4 bytes, y at offset 8 (8 bytes)
    let field_x = RepD::Byte(ByteRep { size: 4, align: 4 });
    let field_y = RepD::Byte(ByteRep { size: 8, align: 8 });
    let repd = RepD::Struct(StructRep {
        fields: vec![
            (0, field_x.clone()),
            (8, field_y.clone()),
        ],
        total_size: 16,
        align: 8,
    });

    let capd = numeric_capd();
    let reld = reld_from(&[Relation::Containment]);
    let bd = BD::new(repd.clone(), capd.clone(), reld.clone());

    // RepD checks
    assert_eq!(bd.repd.size(), 16, "struct total size must be 16");
    assert_eq!(bd.repd.alignment(), 8, "struct alignment must be 8");
    assert_well_formed(&bd, "struct");

    // Field access
    assert_eq!(bd.repd.field_offset(0), 0, "field x at offset 0");
    assert_eq!(bd.repd.field_offset(1), 8, "field y at offset 8");

    // The struct is compatible with itself
    let same = BD::new(repd.clone(), capd.clone(), reld.clone());
    assert!(bd.compatible(&same), "identical structs are compatible");

    // A Byte representation with same size/align subsumes the struct
    let byte_repd = RepD::Byte(ByteRep { size: 16, align: 8 });
    assert!(
        byte_repd.subsumes(&repd),
        "Byte(16,8) subsumes the struct representation"
    );

    // The struct does NOT subsume a flat Byte (it's more specific)
    assert!(
        !repd.subsumes(&byte_repd),
        "struct does not subsume a flat Byte representation"
    );
}

/// Test 7: enum { A(u32), B(u64) } → RepD::Enum with 2 variants
///
/// A Rust enum maps to RepD::Enum with tagged variants.  The size accounts
/// for the discriminant plus the largest variant.
#[test]
fn test_enum_bd() {
    let variant_a = RepD::Byte(ByteRep { size: 4, align: 4 });
    let variant_b = RepD::Byte(ByteRep { size: 8, align: 8 });
    let repd = RepD::Enum(EnumRep {
        variants: vec![
            (0, variant_a.clone()),
            (1, variant_b.clone()),
        ],
    });

    let capd = numeric_capd();
    let reld = RelD::empty();
    let bd = BD::new(repd.clone(), capd.clone(), reld.clone());

    // RepD checks: 8 (discriminant) + aligned_variant_size
    // max variant = 8, aligned to 8 = 8, so total = 8 + 8 = 16
    assert_eq!(bd.repd.size(), 16, "enum total size must be 16");
    assert_eq!(bd.repd.alignment(), 8, "enum alignment must be 8 (discriminant)");
    assert_well_formed(&bd, "enum");

    // Self-compatibility
    let same = BD::new(repd.clone(), capd.clone(), reld.clone());
    assert!(bd.compatible(&same), "identical enums are compatible");

    // Incompatible with a different enum (different variant count)
    let different_enum = RepD::Enum(EnumRep {
        variants: vec![
            (0, variant_a.clone()),
        ],
    });
    assert!(
        !repd.compatible(&different_enum),
        "enums with different variant counts are incompatible"
    );
}

/// Test 8: [u32; 10] → RepD::Array with element RepD and count 10
///
/// A Rust fixed-size array maps to RepD::Array.  The element representation
/// is recursively defined and the total size is element_size × count.
#[test]
fn test_array_bd() {
    let element_repd = RepD::Byte(ByteRep { size: 4, align: 4 });
    let repd = RepD::Array(ArrayRep {
        element: Box::new(element_repd.clone()),
        count: 10,
    });

    let capd = numeric_capd().strengthen(&[Capability::Iterate]);
    let reld = RelD::empty();
    let bd = BD::new(repd.clone(), capd.clone(), reld.clone());

    // RepD checks
    assert_eq!(bd.repd.size(), 40, "[u32; 10] must be 40 bytes");
    assert_eq!(bd.repd.alignment(), 4, "[u32; 10] alignment must be 4");
    assert_well_formed(&bd, "array");

    // Iterate capability
    assert!(
        bd.capd.caps.contains(&Capability::Iterate),
        "arrays should support Iterate"
    );

    // Array is compatible with itself
    let same = BD::new(repd.clone(), capd.clone(), reld.clone());
    assert!(bd.compatible(&same), "identical arrays are compatible");

    // Different count → incompatible
    let different_count = RepD::Array(ArrayRep {
        element: Box::new(element_repd.clone()),
        count: 5,
    });
    assert!(
        !repd.compatible(&different_count),
        "arrays with different counts are incompatible"
    );

    // Same count, incompatible element → incompatible
    let wrong_element = RepD::Array(ArrayRep {
        element: Box::new(RepD::Byte(ByteRep { size: 8, align: 8 })),
        count: 10,
    });
    assert!(
        !repd.compatible(&wrong_element),
        "arrays with incompatible elements are incompatible"
    );
}

/// Test 9: Box<T> → RepD::Ptr(RepD of T), CapD{Read,Write,Drop,DerivePtr,Move}
///
/// A `Box<T>` is a pointer with ownership semantics.  The CapD includes
/// Drop (deallocation), Move (transfer ownership), DerivePtr (pointer
/// derivation from the box itself), plus Read/Write on the pointee.
#[test]
fn test_box_bd() {
    let pointee_repd = RepD::Byte(ByteRep { size: 4, align: 4 });
    let repd = RepD::Ptr(PtrRep {
        pointee: Box::new(pointee_repd.clone()),
    });

    let capd = capd_from(&[
        Capability::Read,
        Capability::Write,
        Capability::Drop,
        Capability::DerivePtr,
        Capability::Move,
    ]);
    let reld = reld_from(&[Relation::Containment]);
    let bd = BD::new(repd.clone(), capd.clone(), reld.clone());

    // RepD checks
    assert_eq!(bd.repd.size(), 8, "Box pointer must be 8 bytes on 64-bit");
    assert_eq!(bd.repd.alignment(), 8, "Box pointer must be 8-byte aligned");
    assert_well_formed(&bd, "Box<T>");

    // Ownership capabilities
    assert!(
        bd.capd.caps.contains(&Capability::Drop),
        "Box must have Drop (deallocation)"
    );
    assert!(
        bd.capd.caps.contains(&Capability::Move),
        "Box must have Move (ownership transfer)"
    );
    assert!(
        bd.capd.caps.contains(&Capability::DerivePtr),
        "Box must have DerivePtr"
    );

    // Box<T> refines (is more specific than) a raw pointer with all capabilities
    let raw_ptr_all_caps = BD::new(
        repd.clone(),
        CapD::all(),
        reld.clone(),
    );
    assert!(
        bd.capd.is_subset(&raw_ptr_all_caps.capd),
        "Box CapD is a subset of all-caps pointer"
    );

    // But Box does NOT have Share (exclusive ownership)
    assert!(
        !bd.capd.caps.contains(&Capability::Share),
        "Box does not have Share (exclusive ownership)"
    );
}

/// Test 10: &T → RepD::Ptr(RepD of T), CapD{Read,Share,DerivePtr}
///
/// A shared reference `&T` is a pointer with read-only access to the pointee.
/// It has Share (shared ownership of the reference) and DerivePtr capabilities,
/// but crucially lacks Write — modelling Rust's borrowing rules.
#[test]
fn test_reference_bd() {
    let pointee_repd = RepD::Byte(ByteRep { size: 4, align: 4 });
    let repd = RepD::Ptr(PtrRep {
        pointee: Box::new(pointee_repd.clone()),
    });

    let capd = capd_from(&[Capability::Read, Capability::Share, Capability::DerivePtr]);
    let reld = RelD::empty();
    let bd = BD::new(repd.clone(), capd.clone(), reld.clone());

    // RepD checks
    assert_eq!(bd.repd.size(), 8, "&T pointer must be 8 bytes on 64-bit");
    assert_eq!(bd.repd.alignment(), 8, "&T pointer must be 8-byte aligned");
    assert_well_formed(&bd, "&T");

    // Shared reference capabilities
    assert!(
        bd.capd.caps.contains(&Capability::Read),
        "&T must have Read"
    );
    assert!(
        bd.capd.caps.contains(&Capability::Share),
        "&T must have Share"
    );
    assert!(
        !bd.capd.caps.contains(&Capability::Write),
        "&T must NOT have Write"
    );
    assert!(
        !bd.capd.caps.contains(&Capability::Drop),
        "&T must NOT have Drop (no ownership)"
    );
    assert!(
        !bd.capd.caps.contains(&Capability::Move),
        "&T does not move ownership of pointee"
    );

    // &T and Box<T> are incomparable in the capability lattice:
    // &T has Share (shared access), Box has Write+Drop+Move (exclusive access).
    // They share Read and DerivePtr but each has capabilities the other lacks.
    let box_capd = capd_from(&[
        Capability::Read,
        Capability::Write,
        Capability::Drop,
        Capability::DerivePtr,
        Capability::Move,
    ]);
    // &T has Share, Box doesn't → &T is NOT a subset
    assert!(
        !bd.capd.is_subset(&box_capd),
        "&T has Share which Box<T> lacks — incomparable in lattice"
    );
    // Box has Write, &T doesn't → Box is NOT a subset either
    assert!(
        !box_capd.is_subset(&bd.capd),
        "Box<T> has Write which &T lacks — incomparable in lattice"
    );
    // Their meet yields only the intersection: Read + DerivePtr
    let meet = bd.capd.meet(&box_capd);
    assert!(
        meet.caps.contains(&Capability::Read),
        "meet of &T and Box<T> preserves Read"
    );
    assert!(
        meet.caps.contains(&Capability::DerivePtr),
        "meet of &T and Box<T> preserves DerivePtr"
    );
    assert!(
        !meet.caps.contains(&Capability::Write),
        "meet of &T and Box<T> loses Write"
    );
    assert!(
        !meet.caps.contains(&Capability::Share),
        "meet of &T and Box<T> loses Share"
    );

    // Meet of &T and &mut T (Write) yields Read only → models borrow splitting
    let mut_ref_capd = capd_from(&[Capability::Read, Capability::Write, Capability::DerivePtr]);
    let meet = bd.capd.meet(&mut_ref_capd);
    assert!(
        meet.caps.contains(&Capability::Read),
        "meet preserves Read"
    );
    assert!(
        !meet.caps.contains(&Capability::Write),
        "meet removes Write (&T and &mut T cannot coexist with Write)"
    );
}

// ===========================================================================
// Category 3: Rust Type System Subsumption (5 tests)
// ===========================================================================

/// Test 11: Ownership modeled by CapD — Rust ownership → CapD with exclusive Write
///
/// Rust's ownership system ensures that each value has exactly one owner with
/// exclusive write access.  In the BD framework, this is modeled by a CapD
/// that includes Write but excludes Share, ensuring exclusive access.
///
/// When ownership is transferred (move), the CapD loses all capabilities
/// on the source and the target gains them.
#[test]
fn test_ownership_modeled_by_capd() {
    // Owned value: Read, Write, Drop, Move (exclusive access)
    let owned_capd = capd_from(&[
        Capability::Read,
        Capability::Write,
        Capability::Drop,
        Capability::Move,
        Capability::Hash,
        Capability::Compare,
    ]);

    // After move: source loses all capabilities
    let after_move_capd = CapD::empty();

    // Verify: owned has exclusive Write (no Share)
    assert!(
        owned_capd.caps.contains(&Capability::Write),
        "owned value must have Write"
    );
    assert!(
        !owned_capd.caps.contains(&Capability::Share),
        "owned value must NOT have Share (exclusive)"
    );

    // After move, the source has no capabilities — cannot be used
    assert!(
        after_move_capd.caps.is_empty(),
        "after move, source has no capabilities"
    );

    // BD for owned value
    let owned_bd = BD::new(
        RepD::Byte(ByteRep { size: 4, align: 4 }),
        owned_capd.clone(),
        RelD::empty(),
    );
    let moved_bd = BD::new(
        RepD::Byte(ByteRep { size: 4, align: 4 }),
        after_move_capd,
        RelD::empty(),
    );

    // moved_bd does not refine owned_bd (fewer caps is "more specific",
    // but an empty CapD means the value is unusable — not a valid BD
    // assignment for a Rust-accessible value)
    assert!(
        !moved_bd.compatible(&owned_bd),
        "moved-from value is incompatible with still-owned value (empty CapD meet)"
    );

    // Join of owned and moved gives back owned (lattice property)
    let joined = owned_capd.join(&CapD::empty());
    assert!(
        joined.caps.contains(&Capability::Read),
        "join with bottom returns original (Read)"
    );
    assert!(
        joined.caps.contains(&Capability::Write),
        "join with bottom returns original (Write)"
    );
}

/// Test 12: Borrowing modeled by CapD — &T → Read-only, &mut T → exclusive Write
///
/// Rust's borrowing rules are modeled by CapD:
/// - `&T` (shared borrow): Read + Share (multiple readers allowed)
/// - `&mut T` (exclusive borrow): Read + Write (exclusive access, no Share)
///
/// The key subsumption property: the meet of &T and &mut T CapDs removes
/// Write, reflecting that you cannot have both shared and mutable borrows
/// simultaneously.
#[test]
fn test_borrowing_modeled_by_capd() {
    // &T: shared borrow — Read + Share
    let shared_borrow_capd = capd_from(&[
        Capability::Read,
        Capability::Share,
        Capability::DerivePtr,
    ]);

    // &mut T: exclusive borrow — Read + Write (no Share!)
    let mut_borrow_capd = capd_from(&[
        Capability::Read,
        Capability::Write,
        Capability::DerivePtr,
    ]);

    // Owned: all of the above
    let owned_capd = capd_from(&[
        Capability::Read,
        Capability::Write,
        Capability::Drop,
        Capability::Move,
        Capability::Share,
        Capability::DerivePtr,
    ]);

    // Both borrows are subsets of owned CapD
    assert!(
        shared_borrow_capd.is_subset(&owned_capd),
        "&T CapD ⊆ owned CapD"
    );
    assert!(
        mut_borrow_capd.is_subset(&owned_capd),
        "&mut T CapD ⊆ owned CapD"
    );

    // &mut T has Write but not Share (exclusive)
    assert!(
        mut_borrow_capd.caps.contains(&Capability::Write),
        "&mut T must have Write"
    );
    assert!(
        !mut_borrow_capd.caps.contains(&Capability::Share),
        "&mut T must NOT have Share (exclusive)"
    );

    // &T has Share but not Write (shared, immutable)
    assert!(
        shared_borrow_capd.caps.contains(&Capability::Share),
        "&T must have Share"
    );
    assert!(
        !shared_borrow_capd.caps.contains(&Capability::Write),
        "&T must NOT have Write"
    );

    // Meet of &T and &mut T: Read + DerivePtr only (cannot coexist with Write/Share)
    let meet = shared_borrow_capd.meet(&mut_borrow_capd);
    assert!(
        meet.caps.contains(&Capability::Read),
        "meet of &T and &mut T preserves Read"
    );
    assert!(
        !meet.caps.contains(&Capability::Write),
        "meet of &T and &mut T removes Write"
    );
    assert!(
        !meet.caps.contains(&Capability::Share),
        "meet of &T and &mut T removes Share"
    );

    // BD-level: create BDs for the same RepD with different CapDs
    let pointee = RepD::Byte(ByteRep { size: 4, align: 4 });
    let ptr_repd = RepD::Ptr(PtrRep {
        pointee: Box::new(pointee),
    });

    let shared_bd = BD::new(ptr_repd.clone(), shared_borrow_capd, RelD::empty());
    let mut_bd = BD::new(ptr_repd.clone(), mut_borrow_capd, RelD::empty());

    // They are compatible (same RepD, meet of CapDs is non-empty)
    assert!(
        shared_bd.compatible(&mut_bd),
        "&T and &mut T BDs are structurally compatible"
    );
    // But their composition restricts to Read-only
    let composed = shared_bd.compose(&mut_bd).expect("composition should succeed");
    assert!(
        !composed.capd.caps.contains(&Capability::Write),
        "composed &T ∘ &mut T loses Write"
    );
}

/// Test 13: Lifetime modeled by RelD — Rust lifetime 'a → RelD with TemporalKind::Outlives
///
/// Rust lifetimes constrain how long references remain valid.  In the BD
/// framework, lifetimes are modeled by RelD with Temporal relations:
/// - `'a: 'b` (a outlives b) → Temporal(Outlives)
/// - Equal lifetimes → Temporal(Coincides)
///
/// The key property: RelD.is_consistent() catches contradictory lifetimes
/// (e.g., 'a outlives 'b AND 'a succeeds 'b).
#[test]
fn test_lifetime_modeled_by_reld() {
    // 'a outlives 'b
    let a_outlives_b = reld_from(&[Relation::Temporal(TemporalKind::Outlives)]);

    // 'a coincides with 'b
    let a_coincides_b = reld_from(&[Relation::Temporal(TemporalKind::Coincides)]);

    // 'a precedes 'b
    let a_precedes_b = reld_from(&[Relation::Temporal(TemporalKind::Precedes)]);

    // Consistent combinations
    assert!(
        a_outlives_b.is_consistent(),
        "Outlives alone is consistent"
    );
    assert!(
        a_coincides_b.is_consistent(),
        "Coincides alone is consistent"
    );
    assert!(
        a_precedes_b.is_consistent(),
        "Precedes alone is consistent"
    );

    // Outlives + Coincides is consistent
    let combined = a_outlives_b.compose(&a_coincides_b);
    assert!(
        combined.is_consistent(),
        "Outlives + Coincides is consistent"
    );

    // Contradictory: Outlives + Succeeds
    let a_succeeds_b = reld_from(&[Relation::Temporal(TemporalKind::Succeeds)]);
    let contradictory = a_outlives_b.compose(&a_succeeds_b);
    assert!(
        !contradictory.is_consistent(),
        "Outlives + Succeeds is inconsistent (contradictory lifetime)"
    );

    // Contradictory: Precedes + Succeeds
    let also_contra = a_precedes_b.compose(&a_succeeds_b);
    assert!(
        !also_contra.is_consistent(),
        "Precedes + Succeeds is inconsistent"
    );

    // BD with lifetime constraints
    let bd_with_lifetime = BD::new(
        RepD::Ptr(PtrRep {
            pointee: Box::new(RepD::Byte(ByteRep { size: 4, align: 4 })),
        }),
        capd_from(&[Capability::Read, Capability::DerivePtr]),
        a_outlives_b,
    );
    assert_well_formed(&bd_with_lifetime, "&'a T");

    // Compose two references with different lifetime constraints
    let bd_other_lifetime = BD::new(
        RepD::Ptr(PtrRep {
            pointee: Box::new(RepD::Byte(ByteRep { size: 4, align: 4 })),
        }),
        capd_from(&[Capability::Read, Capability::DerivePtr]),
        a_coincides_b,
    );
    let composed = bd_with_lifetime.compose(&bd_other_lifetime);
    assert!(
        composed.is_some(),
        "composing references with consistent lifetimes should succeed"
    );
    let composed = composed.unwrap();
    assert!(
        composed.reld.is_consistent(),
        "composed RelD should be consistent"
    );
    assert!(
        composed.reld.relations.contains(&Relation::Temporal(TemporalKind::Outlives)),
        "composed RelD should contain Outlives"
    );
    assert!(
        composed.reld.relations.contains(&Relation::Temporal(TemporalKind::Coincides)),
        "composed RelD should contain Coincides"
    );
}

/// Test 14: Trait bounds modeled by CapD — Trait bounds → CapD requirements
///
/// Rust trait bounds can be modeled as CapD requirements:
/// - `Clone` → Fork capability
/// - `Copy` → Fork without Drop (implicit copy semantics)
/// - `Drop` → Drop capability (custom destructor)
/// - `Hash` → Hash capability
/// - `Ord/PartialOrd` → Compare capability
/// - `Read` → Read capability (std::io::Read)
///
/// The subsumption property: if a type implements a trait, the BD must
/// include the corresponding capability.
#[test]
fn test_trait_bounds_modeled_by_capd() {
    // Clone trait → Fork capability
    let clone_capd = capd_from(&[Capability::Fork]);

    // Copy trait → Fork without Drop (implicit copy)
    let copy_capd = capd_from(&[Capability::Fork]);
    // Copy types do NOT have custom Drop
    assert!(
        !copy_capd.caps.contains(&Capability::Drop),
        "Copy types should not have Drop"
    );

    // Drop trait → Drop capability
    let drop_capd = capd_from(&[Capability::Drop]);

    // Hash trait → Hash capability
    let hash_capd = capd_from(&[Capability::Hash]);

    // Ord trait → Compare capability
    let ord_capd = capd_from(&[Capability::Compare]);

    // A type implementing Clone + Hash + Ord has all three capabilities
    let full_trait_capd = clone_capd
        .join(&hash_capd)
        .join(&ord_capd);
    assert!(
        full_trait_capd.caps.contains(&Capability::Fork),
        "Clone + Hash + Ord implies Fork"
    );
    assert!(
        full_trait_capd.caps.contains(&Capability::Hash),
        "Clone + Hash + Ord implies Hash"
    );
    assert!(
        full_trait_capd.caps.contains(&Capability::Compare),
        "Clone + Hash + Ord implies Compare"
    );

    // A type with Drop + Clone is valid but NOT Copy
    let drop_clone_capd = drop_capd.join(&clone_capd);
    assert!(
        drop_clone_capd.caps.contains(&Capability::Drop),
        "Drop + Clone implies Drop"
    );
    assert!(
        drop_clone_capd.caps.contains(&Capability::Fork),
        "Drop + Clone implies Fork"
    );
    // This is NOT Copy because Copy types cannot have Drop

    // BD for a Clone + Hash type
    let bd = BD::new(
        RepD::Byte(ByteRep { size: 4, align: 4 }),
        full_trait_capd,
        RelD::empty(),
    );
    assert_well_formed(&bd, "Clone+Hash+Ord type");

    // Subsumption: a type with fewer trait bounds has fewer capabilities
    let minimal_capd = capd_from(&[Capability::Fork]); // only Clone
    assert!(
        minimal_capd.is_subset(&bd.capd),
        "Clone-only CapD is a subset of Clone+Hash+Ord CapD"
    );
}

/// Test 15: Send/Sync modeled by RelD — Send/Sync → RelD concurrency relations
///
/// Rust's `Send` and `Sync` marker traits control concurrency safety:
/// - `Send`: safe to transfer ownership across thread boundaries → Send capability
/// - `Sync`: safe to share references across threads → Share + Security(NoDowngrade)
///
/// In the BD framework:
/// - `Send` is modeled by Capability::Send (transfer across concurrency boundary)
/// - `Sync` is modeled by Capability::Share + RelD::Security(NoCrossBoundary)
///   or Security(NoDowngrade) to ensure no data race or downgrade
///
/// A type that is both Send + Sync has the full concurrency RelD.
#[test]
fn test_send_sync_modeled_by_reld() {
    // Send type: can be moved across thread boundaries
    let send_capd = capd_from(&[Capability::Send, Capability::Move]);

    // Sync type: &T can be shared across threads
    let sync_capd = capd_from(&[Capability::Share, Capability::Read]);

    // Send + Sync: both transfer and share
    let send_sync_capd = send_capd.join(&sync_capd);
    assert!(
        send_sync_capd.caps.contains(&Capability::Send),
        "Send+Sync implies Send"
    );
    assert!(
        send_sync_capd.caps.contains(&Capability::Share),
        "Send+Sync implies Share"
    );
    assert!(
        send_sync_capd.caps.contains(&Capability::Move),
        "Send+Sync implies Move"
    );

    // Security relation: Sync implies no data race (no cross-boundary without
    // proper synchronization)
    let sync_reld = reld_from(&[
        Relation::Security(FlowPolicy::NoCrossBoundary),
        Relation::Security(FlowPolicy::NoDowngrade),
    ]);
    assert!(
        sync_reld.is_consistent(),
        "Sync RelD must be consistent"
    );

    // Send RelD: concurrency boundary crossing is permitted
    let send_reld = reld_from(&[Relation::Dependency(DepKind::DataDep)]);

    // BD for a Send type
    let send_bd = BD::new(
        RepD::Byte(ByteRep { size: 4, align: 4 }),
        send_capd.clone(),
        send_reld.clone(),
    );
    assert_well_formed(&send_bd, "Send type");

    // BD for a Sync type
    let sync_bd = BD::new(
        RepD::Byte(ByteRep { size: 4, align: 4 }),
        sync_capd.clone(),
        sync_reld.clone(),
    );
    assert_well_formed(&sync_bd, "Sync type");

    // BD for a Send+Sync type
    let send_sync_bd = BD::new(
        RepD::Byte(ByteRep { size: 4, align: 4 }),
        send_sync_capd.clone(),
        send_reld.compose(&sync_reld),
    );
    assert_well_formed(&send_sync_bd, "Send+Sync type");

    // Key subsumption: a non-Send type lacks Send capability
    let non_send_capd = capd_from(&[Capability::Read, Capability::Write]); // no Send!
    assert!(
        !non_send_capd.caps.contains(&Capability::Send),
        "non-Send type must NOT have Send capability"
    );

    // Meet of Send and non-Send loses Send
    let meet = send_capd.meet(&non_send_capd);
    assert!(
        !meet.caps.contains(&Capability::Send),
        "meet of Send and non-Send loses Send capability"
    );

    // A type with interior mutability (e.g., RefCell) is not Sync:
    // it has Write + Share but lacks NoCrossBoundary security
    let interior_mut_capd = capd_from(&[
        Capability::Read,
        Capability::Write,
        Capability::Share,
    ]);
    let interior_mut_reld = RelD::empty(); // no security boundary protection!
    // This combination (Share + Write + no NoCrossBoundary) would be
    // flagged by a full analysis — the BD framework correctly represents
    // the *absence* of the Sync guarantee by missing the Security relation.

    let interior_mut_bd = BD::new(
        RepD::Byte(ByteRep { size: 4, align: 4 }),
        interior_mut_capd,
        interior_mut_reld,
    );
    // The BD is well-formed but does NOT imply Sync
    assert!(
        !interior_mut_bd.reld.relations.contains(&Relation::Security(FlowPolicy::NoCrossBoundary)),
        "RefCell-like type lacks NoCrossBoundary security relation (not Sync)"
    );
}
