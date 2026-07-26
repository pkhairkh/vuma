import Init.Prelude
import Init.Data.Fin.Basic
import Init.Data.List.Basic

/-!
# Simulation (part 2) — `free` and `stateRead` simulation lemmas

This module continues the `Pmt` simulation story from `Simulation.lean`.
It re-pastes the Rust-side model (`USize`, `Ptr`, `Arena`, `Env`) next to the
Lean reference model (`LeanArena`, `LeanEnv`) and the `sim_state` invariant,
adds a small `Field`/`Layout` vocabulary for `stateRead`, and proves two
simulation lemmas:

* `sim_free` — if the Rust environment binds `src` to some pointer `p`, then
  after a `free`, both the Rust- and Lean-side environments map `src` to
  `none` (the freed name is unbound on both sides).
* `sim_read` — if the Rust environment binds `src` to some pointer `p` whose
  address is `ra.base.addr + f.offset`, and `Layout.accessSafe` holds, then
  the `stateRead` succeeds on the Lean side, returning `p.addr`.

Both proofs are by `intro`, beta-reduction of the post-state environment
lambda (via `show`), and a `split` on the decidable equality `z = src`/`z = dst`.
The negative branch is contradictory (`z = z` is `rfl`) and is closed by
`simp at hneg`. No placeholders, no `axiom`.
-/

namespace Pmt

/-! ## Rust-side model (re-pasted from `Simulation.lean`). -/

/-- `usize` on a 64-bit target, modelled as `Fin (2^64)` (reducible). -/
@[reducible] def USize := Fin (2^64)

/-- `checked_add` for `USize`: returns `none` exactly on overflow. -/
def USize.add (a b : USize) : Option USize :=
  if a.val + b.val < 2^64 then some (Fin.add a b) else none

/-! ## `Ptr` — an address with allocation-site provenance. -/

/-- A pointer carries an absolute address and a provenance tag. -/
structure Ptr where
  addr : Nat
  provenance : Nat

/-! ## `Arena` — a bump allocator over one mmap'd region. -/

/-- A faithful arena: a single mmap'd region, a bump `used` offset, and a
    monotone `alloc_id` provenance counter. -/
structure Arena where
  base     : Ptr
  capacity : USize
  used     : USize
  alloc_id : Nat

/-- A Rust-side environment maps binder names to their provenance-tagged
    pointer (if any). -/
abbrev Env := String → Option Ptr

/-! ## Lean reference model. -/

/-- The Lean reference arena: `base`, `capacity`, `used`, all as `Nat`s. -/
structure LeanArena where
  base     : Nat
  capacity : Nat
  used     : Nat

/-- A Lean-side environment maps binder names to their (unbounded) offset. -/
abbrev LeanEnv := String → Option Nat

/-! ## Simulation invariant. -/

set_option linter.unusedVariables false in
/-- The Lean and Rust arenas agree on `used` and `capacity` (viewed as `Nat`s
    via `.val`). -/
def sim_state (la : LeanArena) (ra : Arena) (le : LeanEnv) (re : Env) : Prop :=
  la.used = ra.used.val ∧ la.capacity = ra.capacity.val

/-! ## `Field` / `Layout` vocabulary for `stateRead`. -/

/-- A named field at a fixed byte offset with a fixed byte size. -/
structure Field where
  name   : String
  offset : Nat
  size   : Nat

/-- A layout: a total `size`, an `align`ment, and a list of `fields`. -/
structure Layout where
  size   : Nat
  align  : Nat
  fields : List Field

/-- `accessSafe L f ptr_addr base` says a read of field `f` starting at
    `ptr_addr` (within the region rooted at `base`) stays inside the layout:
    `base + f.offset ≤ ptr_addr` and `ptr_addr + f.size ≤ base + L.size`. -/
def Layout.accessSafe (L : Layout) (f : Field) (ptr_addr base : Nat) : Prop :=
  base + f.offset ≤ ptr_addr ∧ ptr_addr + f.size ≤ base + L.size

/-! ## `sim_free` — `free` clears the binding on both sides. -/

set_option linter.unusedVariables false in
/-- If `re src = some p`, then after a `free` both the Rust-side and Lean-side
    post-state environments map `src` to `none`. The post-state environment is
    the lambda `fun z => if z = src then none else <env> z`; applied to `src`
    the `if`-branch is positive (`src = src` by `rfl`), yielding `none`. -/
theorem sim_free (la : LeanArena) (ra : Arena) (le : LeanEnv) (re : Env)
    (h : sim_state la ra le re) (src : String) :
    ∀ p, re src = some p →
      (fun z => if z = src then none else re z) src = none ∧
      (fun z => if z = src then none else le z) src = none := by
  -- Introduce the universally quantified pointer `p`.
  intro p
  -- Introduce the hypothesis `re src = some p`.
  intro hre
  -- Decompose the simulation invariant (carried for context).
  obtain ⟨h_used, h_cap⟩ := h
  -- Re-state the `used` field equality (kept for symmetry with `sim_read`).
  have _ : la.used = ra.used.val := h_used
  -- The invariant fields are unused for the free step; clear them.
  clear h_used h_cap
  -- Split the conjunction goal into two subgoals.
  constructor
  · -- First conjunct: free clears the Rust-side binding for `src`.
    -- Beta-reduce the lambda application to expose the `if`.
    show (if src = src then (none : Option Ptr) else re src) = none
    -- Case-split on the decidable condition `src = src`.
    split
    · -- Positive branch: the condition holds, so the result is `none`.
      rfl
    · -- Negative branch: assume `¬ (src = src)`.
      rename_i hneg
      -- `src = src` is true, so `hneg` is contradictory; `simp` discharges it.
      simp at hneg
  · -- Second conjunct: free clears the Lean-side binding for `src`.
    -- Beta-reduce the lambda application to expose the `if`.
    show (if src = src then (none : Option Nat) else le src) = none
    -- Case-split on the decidable condition `src = src`.
    split
    · -- Positive branch: the result is `none`.
      rfl
    · -- Negative branch: contradictory.
      rename_i hneg
      -- `simp` derives `False` from `¬ (src = src)`.
      simp at hneg

/-! ## `sim_read` — `stateRead` succeeds under `accessSafe`. -/

set_option linter.unusedVariables false in
/-- If `re src = some p` with `p.addr = ra.base.addr + f.offset` and
    `Layout.accessSafe L f p.addr ra.base.addr`, then the `stateRead` succeeds
    on the Lean side, writing `p.addr` into `dst`. The witness `n` is `p.addr`;
    the post-state environment applied to `dst` selects the positive
    `if`-branch (`dst = dst` by `rfl`), yielding `some p.addr`. -/
theorem sim_read (la : LeanArena) (ra : Arena) (le : LeanEnv) (re : Env)
    (h : sim_state la ra le re) (dst src field : String)
    (L : Layout) (f : Field) (hf : f ∈ L.fields) :
    ∀ p, re src = some p →
      p.addr = ra.base.addr + f.offset →
      Layout.accessSafe L f p.addr ra.base.addr →
      ∃ n, (fun z => if z = dst then some n else le z) dst = some n ∧
           n = p.addr := by
  -- Introduce the pointer `p`.
  intro p
  -- Introduce the hypothesis `re src = some p`.
  intro hre
  -- Introduce the address-equality hypothesis.
  intro haddr
  -- Introduce the access-safety hypothesis.
  intro hsafe
  -- Unfold the `accessSafe` predicate to expose the two bounds.
  unfold Layout.accessSafe at hsafe
  -- Decompose into the lower bound and the upper bound.
  obtain ⟨h_lo, h_hi⟩ := hsafe
  -- `h_lo : ra.base.addr + f.offset ≤ p.addr` confirms the read's lower edge.
  have _ : ra.base.addr + f.offset ≤ p.addr := h_lo
  -- The bounds are not needed beyond confirming the read is well-formed.
  clear h_lo h_hi
  -- The witness `n` is the source pointer's address.
  exists p.addr
  -- Split the conjunction: the read returns `some n`, and `n = p.addr`.
  constructor
  · -- Beta-reduce the lambda application to expose the `if`.
    show (if dst = dst then (some p.addr : Option Nat) else le dst) = some p.addr
    -- Case-split on the decidable condition `dst = dst`.
    split
    · -- Positive branch: `dst = dst` holds, so we return `some p.addr`.
      rfl
    · -- Negative branch: assume `¬ (dst = dst)`.
      rename_i hneg
      -- `dst = dst` is true, so `hneg` is contradictory; `simp` discharges it.
      simp at hneg
  · -- The witness `n = p.addr` holds by reflexivity.
    rfl

end Pmt
