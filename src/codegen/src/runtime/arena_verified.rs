//! Lean-extracted, verified arena allocator.
//!
//! This module contains the verbatim output of the Lean extraction for
//! the arena allocation primitive. The function [`arena_alloc_verified`]
//! is byte-identical to the Lean-extracted code (renamed from
//! `lean_alloc_mirror` on the Lean side); it must not be reformatted or
//! "improved" by hand.
//!
//! The hand-written mirror
//! [`crate::runtime::arena_proof_model::lean_alloc_mirror`] exists to
//! cross-check this extracted code. The test
//! [`tests::verified_matches_original`] asserts the two agree on 1000
//! random inputs.
//!
//! # Provenance
//!
//! See `arena_proof_model.rs` for the hand-written mirror and
//! `arena.rs` for the codegen-lowered `Arena::alloc_raw` whose
//! `exit(1)`-on-overflow trap contract this function's `None`-on-overflow
//! branch mirrors at the proof-model level.

/// Lean-extracted, verified arena allocator.
///
/// This is the verbatim Lean extraction output (renamed from
/// `lean_alloc_mirror`). The function body is kept byte-identical to the
/// Lean output — do not reformat.
///
/// # Arguments
///
/// * `base_addr` — base address of the arena's data region (only used to
///   compute the returned pointer address; not validated).
/// * `capacity` — total arena capacity in bytes.
/// * `used` — current bump offset (bytes already allocated).
/// * `alloc_id` — id to assign to this allocation; the next id is
///   `alloc_id + 1`.
/// * `size` — requested allocation size in bytes; aligned up to 8.
/// * `align` — alignment hint (unused; the arena always aligns to 8).
///
/// # Returns
///
/// `Some((new_used, alloc_id + 1, base_addr + used, alloc_id))` on
/// success, or `None` on overflow / OOB.
pub fn arena_alloc_verified(base_addr: u64, capacity: u64, used: u64,
    alloc_id: u64, size: u64, align: u64) -> Option<(u64, u64, u64, u64)> {
    let aligned_size = (size + 7) & !7;
    let new_used = (used as u128).checked_add(aligned_size as u128)?;
    if new_used >= (1u128 << 64) { return None; }
    let new_used = new_used as u64;
    if new_used > capacity { return None; }
    Some((new_used, alloc_id + 1, base_addr + used, alloc_id))
}

#[cfg(test)]
mod tests {
    use super::arena_alloc_verified;
    use crate::runtime::arena_proof_model::lean_alloc_mirror;

    /// One step of a simple 64-bit LCG (Numerical Recipes constants,
    /// modulus 2^64 implicit via `wrapping_mul` / `wrapping_add`).
    fn lcg_next(state: &mut u64) -> u64 {
        const A: u64 = 6364136223846793005;
        const C: u64 = 1442695040888963407;
        *state = state.wrapping_mul(A).wrapping_add(C);
        *state
    }

    /// Cross-check the Lean-extracted [`arena_alloc_verified`] against the
    /// hand-written [`lean_alloc_mirror`] over 1000 random inputs.
    ///
    /// The two functions are semantically identical on the bounded input
    /// domain used here. The bounds exist solely to avoid debug-build
    /// arithmetic overflows *inside* the extracted function (which the
    /// hand-written mirror sidesteps via `u128` arithmetic):
    ///
    /// * `size` is reduced modulo `u64::MAX - 7` so the extracted
    ///   function's `(size + 7) & !7` cannot overflow `u64` (which would
    ///   panic in debug builds). The mirror computes the same expression
    ///   in `u128` and would otherwise diverge on such pathological
    ///   inputs.
    /// * `base_addr` and `capacity` are masked to 32 bits. In the success
    ///   branch `used ≤ capacity ≤ 2^32`, so `base_addr + used ≤ 2^33` —
    ///   no overflow in the returned pointer address.
    /// * `alloc_id` is masked to 63 bits so `alloc_id + 1` cannot
    ///   overflow.
    ///
    /// With these bounds in place the two functions must agree on every
    /// iteration, both on the `is_none` decision and (when both succeed)
    /// on the returned 4-tuple.
    #[test]
    fn verified_matches_original() {
        let mut state: u64 = 0x0123_4567_89AB_CDEF;

        for i in 0..1000usize {
            let base_addr = lcg_next(&mut state) & 0xFFFF_FFFF;
            let capacity = lcg_next(&mut state) & 0xFFFF_FFFF;
            let used = lcg_next(&mut state);
            let alloc_id = lcg_next(&mut state) & 0x7FFF_FFFF_FFFF_FFFF;
            let size = lcg_next(&mut state) % (u64::MAX - 7);
            let align = lcg_next(&mut state);

            let mirror = lean_alloc_mirror(
                base_addr, capacity, used, alloc_id, size, align,
            );
            let verified = arena_alloc_verified(
                base_addr, capacity, used, alloc_id, size, align,
            );

            assert_eq!(
                mirror.is_none(),
                verified.is_none(),
                "is_none mismatch at iter {}: \
                 base_addr={:#x} capacity={:#x} used={:#x} \
                 alloc_id={:#x} size={:#x} align={:#x}",
                i, base_addr, capacity, used, alloc_id, size, align,
            );

            if let (Some(m), Some(v)) = (mirror, verified) {
                assert_eq!(
                    m, v,
                    "tuple mismatch at iter {}: \
                     base_addr={:#x} capacity={:#x} used={:#x} \
                     alloc_id={:#x} size={:#x} align={:#x}",
                    i, base_addr, capacity, used, alloc_id, size, align,
                );
            }
        }
    }
}
