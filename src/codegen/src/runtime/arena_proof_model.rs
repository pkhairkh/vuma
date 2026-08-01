//! Rust mirror of Lean `FArena.alloc` — proof-model cross-check.
//!
//! This module provides a pure-Rust mirror of the Lean `FArena.alloc`
//! function (and the codegen-lowered `Arena::alloc_raw` in
//! [`super::arena`]). The mirror is *total*: it returns [`Option`] instead
//! of trapping, and uses `u128` for the overflow check, so it can be
//! exhaustively cross-checked against the real arena's behavior on small
//! inputs without risking the `exit(1)` trap that `Arena::alloc_raw`
//! raises on overflow / OOB.
//!
//! ## Branch structure
//!
//! [`lean_alloc_mirror`] has exactly three control-flow branches that
//! produce a final value:
//!
//! 1. **Overflow → [`None`]** — `(used as u128) + aligned_size` does not
//!    fit in `u64` (`>= 1u128 << 64`). The `checked_add?` on the `u128`
//!    sum is total (two `u64` inputs cannot overflow `u128`) but kept
//!    for defensiveness; the `>= 1u128 << 64` check is the meaningful
//!    overflow guard. This mirrors the `usize::checked_add`-returns-
//!    [`None`] trap in `Arena::alloc_raw`.
//!
//! 2. **OOB → [`None`]** — `new_used > capacity`. Mirrors the
//!    `if new_offset > self.capacity { trap }` check in `Arena::alloc_raw`.
//!
//! 3. **Success → [`Some`]`((new_used, alloc_id + 1, base_addr + used,
//!    alloc_id))`** — the new bump offset, the next allocation id, the
//!    allocated pointer address (`base_addr + used`), and the id of
//!    *this* allocation.
//!
//! ## Alignment
//!
//! Like `Arena::alloc_raw`, the mirror aligns `size` up to a multiple of
//! 8 bytes via `(size + 7) & !7` *before* the overflow/capacity checks.
//! The alignment is computed in `u128` so the `+ 7` cannot overflow on a
//! pathological `u64::MAX` input (which the real arena would trap on
//! during the alignment step itself in debug builds).

/// Mirror of Lean `FArena.alloc`.
///
/// See the [module docs](self) for the branch structure and alignment
/// rules.
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
/// * `_align` — alignment hint (unused; the real arena always aligns to
///   8 bytes, so the mirror ignores this parameter the same way).
///
/// # Returns
///
/// `Some((new_used, new_alloc_id, ptr_addr, this_alloc_id))` on success,
/// or [`None`] on overflow / OOB. See [module docs](self).
///
/// # Branches
///
/// Exactly three, per the proof-model spec:
///
/// 1. `checked_add?` / `>= 1u128 << 64` — overflow → [`None`]
/// 2. `> capacity` — OOB → [`None`]
/// 3. fallthrough — success → [`Some`]
pub fn lean_alloc_mirror(
    base_addr: u64,
    capacity: u64,
    used: u64,
    alloc_id: u64,
    size: u64,
    _align: u64,
) -> Option<(u64, u64, u64, u64)> {
    // Align `size` up to 8 bytes — mirrors `Arena::alloc_raw`'s
    // `let aligned_size = (size + 7) & !7;`. Computed in `u128` so the
    // `+ 7` cannot overflow on a `u64::MAX` input.
    let aligned_size = (((size as u128) + 7) & !7u128) as u64;

    // Branch 1: overflow → None. Mirrors `usize::checked_add` returning
    // `None` in `Arena::alloc_raw`. The `checked_add?` is total (two
    // `u64` inputs cannot overflow `u128`) but kept for defensiveness;
    // the `>= 1u128 << 64` check is the meaningful `u64`-range guard.
    let new_used = (used as u128).checked_add(aligned_size as u128)?;
    if new_used >= (1u128 << 64) {
        return None;
    }
    let new_used = new_used as u64;

    // Branch 2: OOB → None. Mirrors `if new_offset > self.capacity` in
    // `Arena::alloc_raw`.
    if new_used > capacity {
        return None;
    }

    // Branch 3: success → Some. The allocated pointer is at
    // `base_addr + used` (the bump pointer *before* the allocation);
    // `alloc_id` is the id of this allocation; `alloc_id + 1` is the
    // next allocation's id.
    Some((new_used, alloc_id + 1, base_addr + used, alloc_id))
}

#[cfg(test)]
mod tests {
    use super::lean_alloc_mirror;
    use crate::runtime::arena::Arena;

    /// Exhaustively cross-check [`lean_alloc_mirror`] against the real
    /// `Arena::alloc_raw` on a small input domain.
    ///
    /// # Domain
    ///
    /// Cartesian product of:
    /// * `capacity ∈ {0, 1, 2, 4, 8, 16, 64, 256, 4096}`
    /// * `used ∈ {0..256}`
    /// * `size ∈ {0..4096}`
    /// * `align ∈ {1, 8, 16}`
    ///
    /// That is `9 × 256 × 4096 × 3 = 28,311,552` cases (well over the
    /// 500-case floor specified by the task).
    ///
    /// # Assertions
    ///
    /// For each combination:
    ///
    /// 1. Compute `mirror = lean_alloc_mirror(...)`.
    /// 2. Independently derive `real_is_none` by replicating
    ///    `Arena::alloc_raw`'s decision logic *inline* (without calling
    ///    `alloc_raw` — it traps via `exit(1)` on failure, which cannot
    ///    be caught in-process). Assert `real_is_none == mirror.is_none()`.
    /// 3. If both are `Some`, create (or reuse) a real `Arena` with the
    ///    given capacity, set its offset to `used` via
    ///    `set_offset_for_testing`, call `alloc_raw(size)`, and assert:
    ///    * the returned pointer is at offset `used` from the arena's
    ///      base (matching the mirror's `base_addr + used`), and
    ///    * the arena's new `used()` equals the mirror's `new_used`.
    ///
    /// # Why the failure cases are not call-real
    ///
    /// `Arena::alloc_raw` funnels every fault path (overflow, OOM,
    /// invalid layout) through `arena_overflow_trap`, which calls
    /// `std::process::exit(1)` (see `arena.rs:107`). `exit` does not
    /// unwind, so the trap cannot be caught with `catch_unwind` or
    /// `#[should_panic]`. The integration test
    /// `tests/arena_overflow_trap_tests.rs` covers the trap-via-subprocess
    /// path; this unit test instead verifies the mirror's failure
    /// prediction against an independent inline replication of the
    /// decision logic.
    ///
    /// # Performance
    ///
    /// To avoid allocating ~1.5M arenas (one per `Some` case), a single
    /// arena is created per `capacity` value and reused across all
    /// `(used, size, align)` combinations by resetting its offset via
    /// `set_offset_for_testing` before each `alloc_raw` call.
    #[test]
    fn mirror_matches_real_alloc_exhaustive_small() {
        let base_addr: u64 = 0x1000_0000;
        let alloc_id: u64 = 0;
        let mut cases: u64 = 0;

        for &capacity in &[0u64, 1, 2, 4, 8, 16, 64, 256, 4096] {
            // One arena per capacity; reused across all (used, size,
            // align) combinations by resetting the offset each iteration.
            let mut arena = Arena::create(capacity as usize);
            let real_base = arena.base() as u64;

            for used in 0u64..256u64 {
                for size in 0u64..4096u64 {
                    for &align in &[1u64, 8, 16] {
                        cases += 1;

                        let mirror = lean_alloc_mirror(
                            base_addr, capacity, used, alloc_id, size, align,
                        );

                        // Independently derive `real_is_none` by
                        // replicating `Arena::alloc_raw`'s decision logic
                        // inline. We cannot call `alloc_raw` directly on
                        // the failure cases because it traps via
                        // `exit(1)` (see `arena.rs:107`).
                        //
                        // This is a *separate* code path from
                        // `lean_alloc_mirror`: it lives in the test
                        // module, uses `u64`-typed `checked_add` (rather
                        // than `u128`), and recomputes `aligned_size`
                        // independently. A bug in the mirror that is not
                        // also present here will surface as an
                        // `is_none`-mismatch assertion failure.
                        let aligned_size = if size <= u64::MAX - 7 {
                            (size + 7) & !7
                        } else {
                            // `size + 7` would overflow `u64`; the real
                            // arena would panic here in debug. Treat as
                            // a failure to match the mirror's `u128`
                            // safety net (which forces `aligned_size` to
                            // `u64::MAX & !7`, then trips the overflow
                            // branch).
                            u64::MAX & !7
                        };
                        let real_is_none = match used.checked_add(aligned_size) {
                            None => true,
                            Some(v) => v > capacity,
                        };

                        assert_eq!(
                            real_is_none,
                            mirror.is_none(),
                            "is_none mismatch: cap={} used={} size={} align={}",
                            capacity,
                            used,
                            size,
                            align,
                        );

                        if let Some((new_used, _new_id, ptr_addr, _old_id)) =
                            mirror
                        {
                            // Mirror predicts success — the real arena
                            // must also succeed. Reset the offset to
                            // `used` and call `alloc_raw`.
                            arena.set_offset_for_testing(used as usize);
                            let real_ptr =
                                arena.alloc_raw(size as usize) as u64;

                            // Both pointers should be at offset `used`
                            // from their respective bases.
                            assert_eq!(
                                real_ptr.wrapping_sub(real_base),
                                used,
                                "real ptr offset mismatch: \
                                 cap={} used={} size={} align={}",
                                capacity,
                                used,
                                size,
                                align,
                            );
                            assert_eq!(
                                ptr_addr.wrapping_sub(base_addr),
                                used,
                                "mirror ptr offset mismatch: \
                                 cap={} used={} size={} align={}",
                                capacity,
                                used,
                                size,
                                align,
                            );
                            // New used must match.
                            assert_eq!(
                                arena.used() as u64,
                                new_used,
                                "new_used mismatch: \
                                 cap={} used={} size={} align={}",
                                capacity,
                                used,
                                size,
                                align,
                            );
                        }
                    }
                }
            }
        }

        assert!(cases >= 500, "expected ≥500 cases, got {}", cases);
    }
}
