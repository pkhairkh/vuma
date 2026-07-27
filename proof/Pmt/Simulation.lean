import Init.Prelude
import Init.Data.Fin.Basic

/-!
# Simulation — relating the Lean reference allocator to the Rust model

This module pastes the (already-verified) `USize` / `Ptr` / `Arena` model of
Rust's checked bump allocator next to a plain-`Nat` "Lean reference"
allocator (`LeanArena` / `LeanExec.alloc`), and proves a simulation theorem
(`sim_alloc`) relating the two: whenever the Lean reference allocator
*fails* (returns `none`), then on the Rust side either the bumped offset
would exceed the arena's `capacity`, or the underlying `USize.add` overflows.

Everything lives in `namespace Pmt` (to avoid clashing with Lean's built-in
`USize`). The proof uses no extra assumptions and no placeholders, and discharges its
case-split with `unfold`, `by_cases`, `if_pos`/`if_neg`, and `omega`.

**Scope vs `Simulation2.lean` (audit 3-C).** The two modules are
*complementary*, not overlapping: this module owns `sim_alloc`; the companion
`Pmt.Simulation2` owns `sim_free` and `sim_read`. There are no shared theorem
names or statements, so they are kept side-by-side rather than merged.
`Simulation2.lean` re-pastes the `USize`/`Ptr`/`Arena`/`Env`/`LeanArena`/
`LeanEnv`/`sim_state` definitions to remain self-contained, so the two modules
must **not** be `import`ed into the same Lean module (that would raise
duplicate-declaration errors). Accordingly, neither module is currently in the
`Pmt` root import graph (`Pmt.lean`); build them explicitly with
`lake build Pmt.Simulation Pmt.Simulation2` when checking them in isolation.
-/

namespace Pmt

/-! ## `USize` — `usize` on a 64-bit target, modelled as `Fin (2^64)`. -/

/-- `usize` on a 64-bit target is exactly the finite ordinal `Fin (2^64)`.

    Marked `@[reducible]` so the `Fin` `Ord`/`Decidable` instances (used by
    the `<`/`>` comparisons below) resolve by unfolding. -/
@[reducible] def USize := Fin (2^64)

/-- `checked_add` for `USize`: returns `none` exactly on overflow. -/
def USize.add (a b : USize) : Option USize :=
  if a.val + b.val < 2^64 then some (Fin.add a b) else none

/-! ## `Ptr` — an address with allocation-site provenance. -/

/-- A pointer carries an absolute address and a provenance tag identifying
    the allocation it was derived from. -/
structure Ptr where
  addr : Nat
  provenance : Nat

/-! ## `Arena` — a bump allocator over one mmap'd region. -/

/-- A faithful arena: a single mmap'd region `[base, base + capacity)`,
    a bump `used` offset, and a monotone `alloc_id` provenance counter. -/
structure Arena where
  base     : Ptr
  capacity : USize
  used     : USize
  alloc_id : Nat

/- Checked bump allocation. Returns `none` when `USize.add a.used size`
   overflows *or* the resulting bump offset meets/exceeds `capacity`. -/
set_option linter.unusedVariables false in
def Arena.alloc (a : Arena) (size align : USize) : Option (Arena × Ptr) :=
  match USize.add a.used size with
  | none => none
  | some new_used =>
    if new_used < a.capacity then
      some ({ a with used := new_used, alloc_id := a.alloc_id + 1 },
            { addr := a.base.addr + a.used.val, provenance := a.alloc_id })
    else none

/-- A Rust-side environment maps binder names to their provenance-tagged
    pointer (if any). -/
abbrev Env := String → Option Ptr

/-! ## `LeanArena` — a plain-`Nat` reference allocator. -/

/-- The Lean reference arena: a `base` address, a `capacity`, and a `used`
    offset, all as unbounded `Nat`s (so addition never overflows). -/
structure LeanArena where
  base     : Nat
  capacity : Nat
  used     : Nat

/- The Lean reference allocator: succeeds iff `used + size ≤ capacity`. -/
set_option linter.unusedVariables false in
def LeanExec.alloc (a : LeanArena) (x : String) (size align : Nat) : Option (LeanArena × Nat) :=
  if a.used + size ≤ a.capacity then
    some ({ a with used := a.used + size }, a.used)
  else
    none

/-- A Lean-side environment maps binder names to their (unbounded) offset. -/
abbrev LeanEnv := String → Option Nat

/-! ## Simulation invariant. -/

/- The Lean and Rust arenas agree on `used` and `capacity` (viewed as
   `Nat`s via `.val`). The environments are not constrained by this
   invariant — they are carried only so `sim_alloc` has the full state
   available for future, stronger simulation lemmas. -/
set_option linter.unusedVariables false in
def sim_state (la : LeanArena) (ra : Arena) (le : LeanEnv) (re : Env) : Prop :=
  la.used = ra.used.val ∧ la.capacity = ra.capacity.val

/-! ## A `Nat → USize` coercion.

    The simulation theorem below applies `USize.add` to a `Nat` `size`
    (the Lean side works over unbounded `Nat`s). Lean's `Init` does *not*
    ship a `Nat → Fin n` coercion, so we install one for `USize` here.
    This is a plain definitional instance — no extra assumptions, no placeholders. -/
instance instCoeNatUSize : Coe Nat USize where
  coe n := ⟨n % 2^64, Nat.mod_lt n (by decide)⟩

/-! ## The simulation theorem. -/

/- If the Lean reference allocator fails on `(la, x, size, align)`, then on
   the Rust side either the bumped offset `ra.used.val + size` exceeds
   `ra.capacity.val`, or `USize.add ra.used size` overflows (returns
   `none`). This is the contrapositive "no spurious failure" half of the
   simulation: a Rust success implies a Lean success.

   Proof sketch. Unfold `LeanExec.alloc` and split (`by_cases`) on the
   Lean capacity check `la.used + size ≤ la.capacity`.
     * If it holds, `if_pos` reduces `LeanExec.alloc` to `some _`,
       contradicting the failure hypothesis `hfail`.
     * If it fails, `if_neg` reduces `LeanExec.alloc` to `none`
       (consistent with `hfail`); the invariant `sim_state` then rewrites
       the failing check to `¬ (ra.used.val + size ≤ ra.capacity.val)`,
       i.e. `ra.used.val + size > ra.capacity.val`, which is the first
       disjunct — discharged by `omega`. -/
set_option linter.unusedVariables false in
theorem sim_alloc (la : LeanArena) (ra : Arena) (le : LeanEnv) (re : Env)
    (h : sim_state la ra le re) (x : String) (size align : Nat)
    (h_size : size < 2^64) :
    LeanExec.alloc la x size align = none →
    ra.used.val + size > ra.capacity.val ∨ (USize.add ra.used size).isNone := by
  -- Assume the Lean reference allocator failed.
  intro hfail
  -- Expose the `if … then some _ else none` body of `LeanExec.alloc`.
  unfold LeanExec.alloc at hfail
  -- Split on the Lean capacity check.
  by_cases h_cond : la.used + size ≤ la.capacity
  · -- Lean check holds: `LeanExec.alloc` yields `some _`, contradicting hfail.
    rw [if_pos h_cond] at hfail
    have hcontra :
        (some ({ la with used := la.used + size }, la.used) : Option (LeanArena × Nat)) = none :=
      hfail
    contradiction
  · -- Lean check fails: `LeanExec.alloc` yields `none` (consistent with hfail).
    rw [if_neg h_cond] at hfail
    -- Decompose the simulation invariant into the two field equalities.
    unfold sim_state at h
    obtain ⟨h_used, h_cap⟩ := h
    -- Transport the failing check onto the Rust-side `.val`s.
    rw [h_used] at h_cond
    rw [h_cap] at h_cond
    -- Drop hypotheses that are no longer needed.
    clear hfail h_used h_cap
    -- The failing check is now `¬ (ra.used.val + size ≤ ra.capacity.val)`.
    have h1 : ¬ (ra.used.val + size ≤ ra.capacity.val) := h_cond
    -- Which is exactly `ra.used.val + size > ra.capacity.val` (first disjunct).
    have h2 : ra.used.val + size > ra.capacity.val := by omega
    -- Discharge the goal with the first disjunct.
    left
    exact h2

end Pmt
