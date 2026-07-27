import PMT.Basic

/-!
## IVE Soundness — DependentTransform (FAITHFUL model, Wave 5 task IVE-FAITH-5-B)

This module is a **bit-faithful** Lean rendering of the Rust function
`src/ive/src/state_transform.rs::verify_dependent_transform`. It replaces
the previous (unfaithful) model that checked `WF_Layout`. The Rust function
checks a Presburger arithmetic bound: `offset + count × elem_size ≤ buffer_size`
with saturating arithmetic.

**Rust reference** (`src/ive/src/state_transform.rs::verify_dependent_transform`):
```rust
pub fn verify_dependent_transform(
    elem_size: u64,
    count: u64,
    offset: u64,
    buffer_size: u64,
) -> bool {
    offset.saturating_add(count.saturating_mul(elem_size)) <= buffer_size
}
```

**Rust logic** (faithfully mirrored below):
  - `count.saturating_mul(elem_size)`: if `count * elem_size` overflows u64, returns `u64::MAX`.
  - `offset.saturating_add(...)`: if `offset + ...` overflows u64, returns `u64::MAX`.
  - Returns `true` iff the result `≤ buffer_size`.
  - This is a Presburger arithmetic check (linear: `offset + count × elem_size`).
  - NO layout well-formedness check. NO WF_Layout.

This module is `sorry`-free.
-/

namespace PMT.IVE.Soundness

/-- The maximum value of u64. Used to model Rust's saturating arithmetic. -/
def u64_max : Nat := 2^64 - 1

/-- Saturating multiplication for u64: if `a * b` overflows u64, returns u64_max.
Mirrors Rust's `u64::saturating_mul`. -/
def saturating_mul_64 (a b : Nat) : Nat :=
  if a * b > u64_max then u64_max else a * b

/-- Saturating addition for u64: if `a + b` overflows u64, returns u64_max.
Mirrors Rust's `u64::saturating_add`. -/
def saturating_add_64 (a b : Nat) : Nat :=
  if a + b > u64_max then u64_max else a + b

/-- The Lean model of IVE's `verify_dependent_transform`. **Faithful** to the
Rust function at `src/ive/src/state_transform.rs::verify_dependent_transform`:
  - Takes 4 Nat params (elem_size, count, offset, buffer_size) matching Rust's 4 u64 params.
  - Uses saturating arithmetic (models u64 overflow).
  - Checks `saturating_add_64 offset (saturating_mul_64 count elem_size) ≤ buffer_size`.
  - Returns Bool (matching Rust's `-> bool`).
  - NO WF_Layout check. NO Layout structs. -/
def verify_dependent_transform
    (elem_size count offset buffer_size : Nat) : Bool :=
  saturating_add_64 offset (saturating_mul_64 count elem_size) ≤ buffer_size

/-- Soundness: if `verify_dependent_transform` returns `true` AND no overflow
occurred in the multiplication or addition, then the Presburger bound holds:
`offset + count * elem_size ≤ buffer_size`.

This is the Lean rendering of the soundness obligation for
`src/ive/src/state_transform.rs::verify_dependent_transform`.

**Why the no-overflow hypothesis is needed**: Rust uses saturating arithmetic.
If `count * elem_size` overflows u64, `saturating_mul` returns `u64::MAX`,
which is almost always > `buffer_size`, so the function rejects. But if
`buffer_size = u64::MAX` (unrealistic), the function accepts despite the
overflow. The no-overflow hypothesis excludes this edge case, giving the
useful soundness guarantee: the actual (non-saturated) access is in bounds.

**No `WF_Layout` conclusion** — Rust does not check it. -/
theorem verify_dependent_transform_sound
    (elem_size count offset buffer_size : Nat)
    (hverify : verify_dependent_transform elem_size count offset buffer_size = true)
    (h_no_mul_overflow : count * elem_size ≤ u64_max)
    (h_no_add_overflow : offset + count * elem_size ≤ u64_max) :
    offset + count * elem_size ≤ buffer_size := by
  unfold verify_dependent_transform at hverify
  -- No multiplication overflow: saturating_mul_64 returns count * elem_size.
  have h_sat_mul : saturating_mul_64 count elem_size = count * elem_size := by
    unfold saturating_mul_64
    rw [if_neg (by omega)]
  rw [h_sat_mul] at hverify
  -- No addition overflow: saturating_add_64 returns offset + count * elem_size.
  have h_sat_add : saturating_add_64 offset (count * elem_size) = offset + count * elem_size := by
    unfold saturating_add_64
    rw [if_neg (by omega)]
  rw [h_sat_add] at hverify
  -- hverify : decide (offset + count * elem_size ≤ buffer_size) = true
  exact decide_eq_true_iff.mp hverify

/-- Corollary: if `verify_dependent_transform` returns `true` and both
`count` and `elem_size` are ≤ u64_max (so their product is ≤ u64_max², but
we need the stronger `count * elem_size ≤ u64_max`), and `offset` is small
enough that no addition overflow occurs, then the Presburger bound holds.

This is a convenience wrapper that bundles the no-overflow hypotheses. -/
theorem verify_dependent_transform_sound'
    (elem_size count offset buffer_size : Nat)
    (hverify : verify_dependent_transform elem_size count offset buffer_size = true)
    (h_count : count ≤ u64_max)
    (h_elem : elem_size ≤ u64_max)
    (h_offset : offset ≤ u64_max)
    (h_mul_no_overflow : count * elem_size ≤ u64_max)
    (h_add_no_overflow : offset + count * elem_size ≤ u64_max) :
    offset + count * elem_size ≤ buffer_size :=
  verify_dependent_transform_sound elem_size count offset buffer_size hverify h_mul_no_overflow h_add_no_overflow

end PMT.IVE.Soundness
