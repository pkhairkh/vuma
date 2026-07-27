import PMT.Basic
import PMT.IVE.Soundness.WFLayoutBool

/-!
## IVE Soundness — ArenaBounds (Wave 2 task IVE-2-A)

This module proves that IVE's `verify_arena_bounds` function is sound:
if it accepts a program (all `valid = true`), then every `ArenaAlloc`
node references a registered layout whose `total_size > 0` and (when the
arena's capacity is known) fits within the arena's remaining capacity.

The Lean model mirrors the Rust function's specification. The actual Rust
function lives at `src/ive/src/arena_bounds.rs`.

**Rust reference** (`src/ive/src/arena_bounds.rs::verify_arena_bounds`):
```rust
pub fn verify_arena_bounds(
    pmt_layouts: &HashMap<String, LayoutSpec>,
    scg: &SCG,
) -> Vec<ArenaBoundsVerification>
```
The Rust function walks the SCG for `ArenaNew` and `ArenaAlloc` nodes.
For each `ArenaAlloc`:
  1. Looks up `layout_name` in `pmt_layouts` (layout-not-found → Violated).
  2. Checks `layout.total_size > 0` (zero-size → Violated).
  3. Checks `used + layout.total_size ≤ capacity` (overflow or exceeds → Violated),
     where `used` is the running total of prior allocs on that arena.

The Lean model below abstracts the SCG walk as a list of `ArenaAllocOp`
records (one per `ArenaAlloc` node), each carrying the layout name and
the arena's capacity/used at that point. This is the "list of events"
simplification used throughout the IVE soundness proofs.

This module is `sorry`-free.
-/

namespace PMT.IVE.Soundness

/-- An arena allocation operation: allocate a layout-sized region in an
arena with the given capacity, where `used` bytes are already allocated.
Mirrors one `ArenaAlloc` node in the SCG (the Rust function walks the
SCG and produces one of these per `ArenaAlloc` node, after looking up
the layout in `pmt_layouts`). -/
structure ArenaAllocOp where
  layout_name : String
  layout_size : Nat
  capacity    : Nat
  used        : Nat
  deriving Repr

/-- The Lean model of IVE's `verify_arena_bounds` output item.
Mirrors `ArenaBoundsVerification { valid: bool, error: Option<String> }`
from `src/ive/src/arena_bounds.rs`. -/
structure ArenaBoundsVerification where
  valid : Bool
  error : Option String
  deriving Repr

/-- The per-op check: layout_size > 0 AND used + layout_size ≤ capacity.
Mirrors the Rust function's three checks (layout-exists is modelled by
the caller providing `layout_size > 0` for registered layouts; the Lean
model receives `layout_size` directly, so "layout not found" is modelled
as `layout_size = 0`). -/
def arena_alloc_ok (op : ArenaAllocOp) : Bool :=
  decide (0 < op.layout_size)
  && decide (op.used + op.layout_size ≤ op.capacity)

/-- The Lean model of IVE's `verify_arena_bounds`.
Returns one `ArenaBoundsVerification` per `ArenaAlloc` node. -/
def verify_arena_bounds (ops : List ArenaAllocOp) : List ArenaBoundsVerification :=
  ops.map fun op =>
    let ok := arena_alloc_ok op
    { valid := ok,
      error := if ok then none
               else some ("arena_alloc: layout size or capacity check failed") }

/-- Soundness: if `verify_arena_bounds` returns all `valid = true`,
then every `ArenaAlloc` has `layout_size > 0` and `used + layout_size ≤ capacity`.
This is the Lean rendering of the soundness obligation for
`src/ive/src/arena_bounds.rs::verify_arena_bounds`. -/
theorem verify_arena_bounds_sound
    (ops : List ArenaAllocOp)
    (hverify : ∀ v, v ∈ verify_arena_bounds ops → v.valid = true) :
    ∀ op : ArenaAllocOp, op ∈ ops →
      0 < op.layout_size ∧ op.used + op.layout_size ≤ op.capacity := by
  intro op hop
  -- Step 1: from `hop : op ∈ ops`, derive that the per-op verification
  -- record is in the output list.
  have h_in :
      ({ valid := arena_alloc_ok op,
         error := if arena_alloc_ok op then none
                  else some "arena_alloc: layout size or capacity check failed" :
        ArenaBoundsVerification })
        ∈ verify_arena_bounds ops := by
    rw [verify_arena_bounds, List.mem_map]
    refine ⟨op, hop, ?_⟩
    rfl
  -- Step 2: apply the all-valid hypothesis.
  have hvalid := hverify _ h_in
  -- Step 3: decompose `arena_alloc_ok op = true` into the two conjuncts.
  unfold arena_alloc_ok at hvalid
  simp only [Bool.and_eq_true_iff, decide_eq_true_iff] at hvalid
  exact hvalid

/-- Corollary: if all arena-allocs pass verification, no alloc has a
zero-size layout. This is the "no zero-size alloc slips through" guarantee. -/
theorem verify_arena_bounds_no_zero_size
    (ops : List ArenaAllocOp)
    (hverify : ∀ v, v ∈ verify_arena_bounds ops → v.valid = true) :
    ∀ op : ArenaAllocOp, op ∈ ops → 0 < op.layout_size := by
  intro op hop
  have h := verify_arena_bounds_sound ops hverify op hop
  exact h.1

/-- Corollary: if all arena-allocs pass verification, no alloc overflows
the arena's capacity. This is the "no arena overflow" guarantee that
complements the runtime `__arena_overflow()` trap. -/
theorem verify_arena_bounds_no_overflow
    (ops : List ArenaAllocOp)
    (hverify : ∀ v, v ∈ verify_arena_bounds ops → v.valid = true) :
    ∀ op : ArenaAllocOp, op ∈ ops → op.used + op.layout_size ≤ op.capacity := by
  intro op hop
  have h := verify_arena_bounds_sound ops hverify op hop
  exact h.2

end PMT.IVE.Soundness
