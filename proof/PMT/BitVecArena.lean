import PMT.Basic
import PMT.Soundness  -- for TrapCode

/-!
## BitVecArena — faithful Arena model with usize overflow semantics

This module models the Rust `Arena` using `BitVec 64` for addresses and
offsets, matching `usize` on 64-bit platforms. Unlike the `Nat`-based
`RawArena` (and the toy `Arena` in `PMT.Basic`), this model CAN express
arithmetic overflow, which is the actual failure mode the Rust
`checked_add` defends against.

### Why this module exists

Per the W1-D arena-fidelity audit
(`docs/verification-reports/S2-W1-D-arena-fidelity.md`), the existing
`RawArena` uses `Nat` for `offset` and `capacity`, which is structurally
unbounded — so `offset + aligned_size` can never wrap. The Rust
`Arena::alloc_raw` (at `src/codegen/src/runtime/arena.rs:168`) calls
`self.offset.checked_add(aligned_size)`, which returns `None` precisely
when `usize` arithmetic would overflow. This is **gap 1** of the audit:
any security property derived from "all overflow paths trap" is unsound
with respect to the Rust binary unless the Lean model can express the
arithmetic-overflow branch as distinct from the capacity-overflow
branch.

This module is the first Lean model in this project that CAN express
`usize` arithmetic overflow:

  * Addresses and offsets are `BitVec 64` (mirrors `usize` on 64-bit
    targets — wraparound at `2^64`).
  * `bv_checked_add` returns `Option Offset64`, with `none` exactly
    when `a + b` would wrap modulo `2^64`.
  * `bv_alloc` produces `Except.error TrapCode.arena_overflow` on
    EITHER (a) arithmetic overflow (path 1 of the Rust `checked_add`)
    OR (b) capacity overflow (path 2, the existing `> capacity` guard).

The capacity is still kept as `Nat` — it is a *bound* supplied at
construction time, not a `usize` address, and modelling it as `Nat`
keeps the capacity-check arithmetic decidable by `omega` without
bit-blasting. A future refinement may also bound it.

### Status

The **definitions** and all three **proofs** (`bv_checked_add_overflow`,
`bv_alloc_traps_on_arithmetic_overflow`, `bv_alloc_traps_on_capacity_overflow`)
are complete and `lake build`-clean. Closed in W3-E using only stdlib
`BitVec.toNat_*` lemmas and `omega` (no Mathlib / `bv_decide` dependency).
The key idea: reduce every `BitVec 64` comparison to a `Nat` comparison
on `.toNat`, then dispatch to `omega` with the bound `x.toNat < 2^64`
from `BitVec.isLt`.

### References

  * Rust source: `src/codegen/src/runtime/arena.rs` (`checked_add`
    at line 168, per W1-B audit).
  * Audit: `docs/verification-reports/S2-W1-D-arena-fidelity.md`
    (gap 1: "usize arithmetic overflow is structurally unrepresentable").
  * Prior Lean model: `proof/PMT/RawArena.lean` (the `Nat`-based model
    this file complements, NOT replaces).
  * Related: `proof/PMT/SimRel.lean` (Lean↔Rust simulation — to be
    extended to cover `BitVecArena` in a later wave).

**Build.** Part of the Lake package rooted at `proof/lakefile.toml`.
Build with `lake build PMT.BitVecArena` (or `lake build` from `proof/`).
-/

namespace PMT

/-! ## §1. Bounded address/offset types (mirror `usize` on 64-bit) -/

/-- A 64-bit address — mirrors Rust `*mut u8` cast to `usize`.

Unlike `RawArena.Ptr` (an `abbrev` of unbounded `Nat`), this is a
genuine 64-bit value that wraps modulo `2^64` under `+`, so address
arithmetic CAN overflow, exactly as Rust's `usize` does on 64-bit
targets. -/
abbrev USize64 : Type := BitVec 64

/-- A 64-bit offset — mirrors Rust `usize` offset.

Distinct type alias from `USize64` to prevent accidental mixing of
addresses and offsets at the API boundary (the same way Rust uses
`*mut u8` vs `usize` for the two concepts). -/
abbrev Offset64 : Type := BitVec 64

/-- The maximum value of a 64-bit offset (`2^64 - 1` = `usize::MAX`).

`bv_checked_add` uses this as the upper bound: `a + b` overflows iff
`b > usizeMax - a`, equivalently iff `a + b < a` (unsigned wraparound).
-/
def usizeMax64 : Offset64 := BitVec.allOnes 64

/-! ## §2. Layout (mirrors `std::alloc::Layout`) -/

/-- Layout with size and alignment — mirrors `std::alloc::Layout`.

For now we keep `size` and `align` as `Nat` (the abstract layout
parameters supplied by the caller); the bounded-ness only matters at
the *offset arithmetic* layer. The Rust arena enforces 8-byte alignment
via the bitmask `(size + 7) & !7` (`arena.rs:167`); `bv_alloc` below
uses the equivalent `((size + 7) / 8) * 8`. -/
structure BvLayout where
  size  : Nat
  align : Nat  -- must be a power of 2 (8 in practice)
  deriving Repr

/-! ## §3. BitVecArena structure (mirrors Rust `Arena` 1:1 in shape) -/

/-- Arena with `BitVec 64` addresses — CAN express overflow.

Fields correspond 1:1 to the Rust `Arena` struct
(`src/codegen/src/runtime/arena.rs` lines 26–36 per W1-B audit):

  * `base`     ↔ `base: *mut u8`        (as `usize` for arithmetic)
  * `offset`   ↔ `offset: usize`       (the bump pointer — CAN overflow)
  * `capacity` ↔ `capacity: usize`     (kept as `Nat` here: it is a
    bound, not an address; arithmetic against it does not need to wrap)
  * `layout`   ↔ `layout: Layout`      (size + align)

The key difference from `RawArena`: `offset` is `Offset64` (= `BitVec
64`), not `Nat`. This makes the `checked_add` overflow branch
syntactically representable. -/
structure BitVecArena where
  base     : USize64
  offset   : Offset64
  capacity : Nat
  layout   : BvLayout
  deriving Repr

/-! ## §4. Checked add for `BitVec 64` (mirrors `usize::checked_add`)

Rust's `usize::checked_add(self, rhs)` returns `None` iff
`self + rhs` would overflow `usize::MAX`. We model this by:

  * computing `sum := a + b` (which wraps modulo `2^64` in `BitVec`),
  * checking `b ≤ usizeMax64 - a` (no overflow case — equivalent to
    `a + b ≤ usizeMax64` as an integer sum).

This is faithful to Rust's `checked_add` semantics: `none` is returned
exactly when the integer sum would exceed `2^64 - 1`. -/

/-- `bv_checked_add a b` — returns `some (a + b)` if no overflow, `none`
otherwise. Mirrors Rust's `usize::checked_add`.

The `BitVec 64` `+` is wrapping; we detect the wrap by checking that
`b ≤ usizeMax64 - a` (which is `2^64 - 1 - a` in integer arithmetic,
since `a ≤ 2^64 - 1` always). If `b` exceeds this, then `a + b ≥ 2^64`
as an integer and the `BitVec.add` wraps. -/
def bv_checked_add (a b : Offset64) : Option Offset64 :=
  let sum := a + b
  if b ≤ usizeMax64 - a then some sum else none

/-- Lemma: `bv_checked_add` returns `none` iff `a + b` wraps (i.e.
`a + b < a` in unsigned `BitVec` comparison).

This is the formal characterization of the overflow branch. Closed in
W3-E using stdlib `BitVec.toNat_*` lemmas and `omega` (no Mathlib /
`bv_decide` dependency required): the comparison `b ≤ allOnes 64 - a`
is reduced to `b.toNat ≤ 2^64 - 1 - a.toNat`, and the wraparound test
`a + b < a` is reduced to `(a.toNat + b.toNat) % 2^64 < a.toNat`; both
then fall to `omega` on the underlying `Nat` values, given the
fundamental `BitVec.isLt` bound `x.toNat < 2^64`. -/
theorem bv_checked_add_overflow (a b : Offset64) :
    bv_checked_add a b = none ↔ a + b < a := by
  unfold bv_checked_add usizeMax64
  simp only [BitVec.le_def, BitVec.lt_def]
  -- `(BitVec.allOnes 64 - a).toNat = 2^64 - 1 - a.toNat` (since `a < 2^64`).
  have h_sub : (BitVec.allOnes 64 - a).toNat = 2^64 - 1 - a.toNat := by
    rw [BitVec.toNat_sub, BitVec.toNat_allOnes]
    have ha : a.toNat < 2^64 := BitVec.isLt a
    omega
  rw [h_sub]
  have h_add : (a + b).toNat = (a.toNat + b.toNat) % 2^64 := BitVec.toNat_add a b
  rw [h_add]
  have ha : a.toNat < 2^64 := BitVec.isLt a
  have hb : b.toNat < 2^64 := BitVec.isLt b
  by_cases h : b.toNat ≤ 2^64 - 1 - a.toNat
  · -- No-overflow case: `bv_checked_add = some (a + b)`, so LHS is `False`;
    -- `a.toNat + b.toNat < 2^64`, so the modular sum is unchanged and `≥ a.toNat`,
    -- making RHS `False` too. `False ↔ False` holds.
    rw [if_pos h]
    simp
    omega
  · -- Overflow case: `bv_checked_add = none`, so LHS is `True`;
    -- `a.toNat + b.toNat ≥ 2^64`, so the modular sum wraps to
    -- `a.toNat + b.toNat - 2^64 < a.toNat`, making RHS `True` too.
    rw [if_neg h]
    simp
    omega

/-! ## §5. Bump-allocate on `BitVecArena` (models both overflow paths)

The Rust `Arena::alloc_raw` (`arena.rs:160-185`) has TWO distinct trap
paths:

  1. Arithmetic overflow: `self.offset.checked_add(aligned_size)` returns
     `None` → `arena_overflow_trap` → exit 1.
  2. Capacity overflow: `new_offset > self.capacity` → same trap, same
     exit code, but a different semantic failure (we asked for more
     memory than the arena was given).

In the existing `RawArena` model, path 1 is structurally impossible
(`Nat` cannot overflow), so only path 2 is modelled. In `BitVecArena`,
BOTH paths are syntactically present and produce
`Except.error TrapCode.arena_overflow`. This is the contribution of
this module. -/

/-- `bv_alloc a size` — bump-allocate `size` bytes on `BitVecArena`,
returning either the updated arena or `TrapCode.arena_overflow`.

Mirrors `Arena::alloc_raw` at `arena.rs:160-185`. Alignment is the same
8-byte bitmask as Rust: `((size + 7) / 8) * 8` is the Lean equivalent
of `(size + 7) & !7`. Both overflow paths route to the same trap code,
matching the Rust binary's single `__arena_overflow` exit. -/
def bv_alloc (a : BitVecArena) (size : Nat) : Except TrapCode BitVecArena :=
  let aligned := ((size + 7) / 8) * 8  -- 8-byte alignment, mirrors `(size+7) & !7`
  let aligned_bv := BitVec.ofNat 64 aligned
  match bv_checked_add a.offset aligned_bv with
  | none => .error .arena_overflow  -- path 1: arithmetic overflow (usize::checked_add = None)
  | some new_offset =>
    if new_offset.toNat > a.capacity then
      .error .arena_overflow  -- path 2: capacity overflow (new_offset > capacity)
    else
      .ok { a with offset := new_offset }

/-! ## §6. Trap-path lemmas

The two theorems below assert that EACH overflow path independently
produces `Except.error TrapCode.arena_overflow`. Both are closed in W3-E
using `bv_checked_add_overflow` (arithmetic-overflow path) and a direct
unfold of `bv_checked_add` plus `if_pos` (capacity-overflow path). -/

/-- Lemma: `bv_alloc` traps via the arithmetic-overflow path when
`offset + aligned_size` wraps past `usize::MAX`.

Precondition `hover` is exactly the wraparound condition
`a.offset + aligned_bv < a.offset`. -/
theorem bv_alloc_traps_on_arithmetic_overflow
    (a : BitVecArena) (size : Nat)
    (hover : a.offset + BitVec.ofNat 64 (((size + 7) / 8) * 8) < a.offset) :
    bv_alloc a size = Except.error TrapCode.arena_overflow := by
  -- Step 1: from `hover` and `bv_checked_add_overflow`, derive
  --   `bv_checked_add a.offset aligned_bv = none`.
  -- Step 2: zeta-reduce the `let`-bindings in `bv_alloc`, then rewrite
  --   the scrutinee to `none`; the `match` reduces definitionally to
  --   the first arm, `Except.error .arena_overflow`.
  unfold bv_alloc
  simp only []
  have hcheck : bv_checked_add a.offset (BitVec.ofNat 64 ((size + 7) / 8 * 8)) = none :=
    (bv_checked_add_overflow _ _).mpr hover
  rw [hcheck]

/-- Lemma: `bv_alloc` traps via the capacity-overflow path when the
sum (as a `Nat`) exceeds the arena's capacity.

Preconditions:
  * `hnoarith`: no arithmetic overflow (so `bv_checked_add` returns
    `some new_offset`),
  * `hcap`: `new_offset.toNat > capacity` (so the capacity guard fires). -/
theorem bv_alloc_traps_on_capacity_overflow
    (a : BitVecArena) (size : Nat)
    (hnoarith : a.offset + BitVec.ofNat 64 (((size + 7) / 8) * 8) ≥ a.offset)
    (hcap : (a.offset + BitVec.ofNat 64 (((size + 7) / 8) * 8)).toNat > a.capacity) :
    bv_alloc a size = Except.error TrapCode.arena_overflow := by
  -- Strategy: unfold `bv_alloc` AND `bv_checked_add` together so the
  -- `if` inside `bv_checked_add` is visible, then prove its condition
  -- (`aligned_bv ≤ allOnes 64 - a.offset`) from `hnoarith`, reducing
  -- the match to its second arm with `new_offset = a.offset + aligned_bv`.
  -- The inner capacity guard then fires by `hcap`.
  unfold bv_alloc bv_checked_add usizeMax64
  simp only []
  -- Same `2^64 - 1 - a.offset.toNat` simplification as in `bv_checked_add_overflow`.
  have h_sub : (BitVec.allOnes 64 - a.offset).toNat = 2^64 - 1 - a.offset.toNat := by
    rw [BitVec.toNat_sub, BitVec.toNat_allOnes]
    have ha : a.offset.toNat < 2^64 := BitVec.isLt a.offset
    omega
  -- Unfold `hnoarith` to `Nat`-level via `BitVec.toNat_add` / `toNat_ofNat`.
  -- We keep `hcap` in `BitVec` form (it is used only for the final `if_pos`).
  have hnoarith' :
      (a.offset.toNat + ((size + 7) / 8 * 8) % 2^64) % 2^64 ≥ a.offset.toNat := by
    have := hnoarith
    simp only [BitVec.toNat_add, BitVec.toNat_ofNat] at this
    exact this
  -- The no-overflow condition `aligned_bv ≤ allOnes 64 - a.offset` is the
  -- contrapositive of `hover` (wraparound), derivable by `omega` once both
  -- sides are reduced to `Nat`.
  have hcond : BitVec.ofNat 64 ((size + 7) / 8 * 8) ≤ BitVec.allOnes 64 - a.offset := by
    simp only [BitVec.le_def]
    rw [h_sub, BitVec.toNat_ofNat]
    have ha : a.offset.toNat < 2^64 := BitVec.isLt a.offset
    have hb : ((size + 7) / 8 * 8) % 2^64 < 2^64 := Nat.mod_lt _ (by omega)
    omega
  -- Now the `if` inside `bv_checked_add` reduces to `some (a.offset + aligned_bv)`,
  -- the match takes the second arm, and `if_pos hcap` selects the `.error` branch.
  rw [if_pos hcond]
  simp only []
  rw [if_pos hcap]

end PMT
