//! Trivial program tests
//!
//! Basic memory safety tests covering allocation, access, freeing,
//! and common violation patterns (use-after-free, double-free, out-of-bounds).

/// Test: allocate a region, write a value, read it back, verify, then free.
///
/// This is the simplest possible safe memory lifecycle:
/// allocate → write → read → verify → free
#[test]
fn test_allocate_read_free() {
    // TODO: Implement using vuma-scg memory model
    // let mut heap = scg::Heap::new();
    // let ptr = heap.allocate(1, std::mem::size_of::<u64>())?;
    // heap.write(ptr, 42u64)?;
    // let value: u64 = heap.read(ptr)?;
    // assert_eq!(value, 42);
    // heap.free(ptr)?;
    // assert!(heap.is_freed(ptr));
    todo!("Implement allocate-read-free test once vuma-scg memory model is available");
}

/// Test: allocate, free, then attempt to read → should flag liveness violation.
///
/// A use-after-free is one of the most critical memory safety bugs.
/// The VUMA system should detect that the pointer is no longer live
/// and flag this as a liveness violation.
#[test]
fn test_use_after_free() {
    // TODO: Implement using vuma-ive liveness checker
    // let mut heap = scg::Heap::new();
    // let ptr = heap.allocate(1, std::mem::size_of::<u64>())?;
    // heap.free(ptr)?;
    // let result = heap.read::<u64>(ptr);
    // assert!(result.is_err());
    // assert!(matches!(result.unwrap_err(), ive::Violation::Liveness(_)));
    todo!("Implement use-after-free test once vuma-ive liveness checker is available");
}

/// Test: allocate, free, then free again → should flag cleanup violation.
///
/// Double-free is a classic memory safety issue that can lead to
/// exploitable heap corruption. VUMA should detect that the region
/// has already been freed and flag a cleanup violation.
#[test]
fn test_double_free() {
    // TODO: Implement using vuma-ive cleanup checker
    // let mut heap = scg::Heap::new();
    // let ptr = heap.allocate(1, std::mem::size_of::<u64>())?;
    // heap.free(ptr)?;
    // let result = heap.free(ptr);
    // assert!(result.is_err());
    // assert!(matches!(result.unwrap_err(), ive::Violation::Cleanup(_)));
    todo!("Implement double-free test once vuma-ive cleanup checker is available");
}

/// Test: allocate N bytes, access offset N+1 → should flag interpretation violation.
///
/// Out-of-bounds access violates the spatial contract of the allocated region.
/// VUMA should detect that the access falls outside the region's bounds
/// and flag an interpretation violation.
#[test]
fn test_out_of_bounds() {
    // TODO: Implement using vuma-ive bounds checker
    // let mut heap = scg::Heap::new();
    // let size = 16u64;
    // let ptr = heap.allocate(1, size as usize)?;
    // let oob_ptr = ptr.offset(size + 1);
    // let result = heap.read::<u8>(oob_ptr);
    // assert!(result.is_err());
    // assert!(matches!(result.unwrap_err(), ive::Violation::Interpretation(_)));
    todo!("Implement out-of-bounds test once vuma-ive bounds checker is available");
}

/// Test: allocate N bytes, access offset N-1 → should prove safe.
///
/// Accessing the last valid byte within an allocated region should
/// be provably safe. This tests that VUMA's proof system correctly
/// identifies in-bounds accesses as safe.
#[test]
fn test_valid_offset() {
    // TODO: Implement using vuma-proof verifier
    // let mut heap = scg::Heap::new();
    // let size = 16u64;
    // let ptr = heap.allocate(1, size as usize)?;
    // let last_ptr = ptr.offset(size - 1);
    // let proof = proof::verify_access(&heap, last_ptr, 1)?;
    // assert!(proof.is_safe());
    // heap.free(ptr)?;
    todo!("Implement valid-offset test once vuma-proof verifier is available");
}

/// Test: base pointer + offset within bounds → prove safe.
///
/// Pointer arithmetic that stays within the allocated region's bounds
/// should be verified as safe by the proof system.
#[test]
fn test_pointer_arithmetic() {
    // TODO: Implement using vuma-proof verifier
    // let mut heap = scg::Heap::new();
    // let size = 64u64;
    // let ptr = heap.allocate(1, size as usize)?;
    // let offset_ptr = ptr.offset(32);
    // let proof = proof::verify_access(&heap, offset_ptr, 4)?;
    // assert!(proof.is_safe());
    // heap.free(ptr)?;
    todo!("Implement pointer-arithmetic test once vuma-proof verifier is available");
}

/// Test: base pointer + offset exceeds bounds → flag violation.
///
/// Pointer arithmetic that produces a pointer outside the allocated
/// region should be flagged as an interpretation violation.
#[test]
fn test_pointer_arithmetic_oob() {
    // TODO: Implement using vuma-ive bounds checker
    // let mut heap = scg::Heap::new();
    // let size = 16u64;
    // let ptr = heap.allocate(1, size as usize)?;
    // let oob_ptr = ptr.offset(size + 8);
    // let result = heap.read::<u64>(oob_ptr);
    // assert!(result.is_err());
    // assert!(matches!(result.unwrap_err(), ive::Violation::Interpretation(_)));
    // heap.free(ptr)?;
    todo!("Implement pointer-arithmetic-oob test once vuma-ive bounds checker is available");
}
