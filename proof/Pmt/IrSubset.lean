import Init.Prelude
import Init.Data.Fin.Basic

/-!
# `IrSubset` — IR instructions and their small-step semantics over the Arena

This file models the subset of the Rust SCG IR (`src/scg/src/ir.rs`)
consisting of the three instructions

```rust
pub enum Instr {
    Alloc     { dst: String, size: usize, align: usize },
    Free      { src: String },
    StateRead { dst: String, src: String, field: String },
}
```

as the Lean inductive `IrInstr`, and gives a small-step operational
semantics `Step : IrInstr → Arena → Env → Arena → Env → Prop` over the
bump-allocator `Arena` model from `Arena.lean`.

The `USize` / `Ptr` / `Arena` definitions are pasted in directly (no
`import` of the sibling modules) so this file is self-contained, exactly
as in `Arena.lean`. Everything lives in `namespace Pmt`.

Note on `size`/`align` types. Rust's `usize` is modelled here as
`USize = Fin (2^64)` (the convention used throughout the `Pmt` layer), and
`Arena.alloc` takes `size align : USize`. The `Step.alloc_*` constructors
feed the *same* `size align` into both `Arena.alloc` and `IrInstr.alloc`,
so `IrInstr.alloc`'s `size`/`align` fields are typed `USize` (not `Nat`):
this is the only choice under which the six constructors type-check, and
it is the faithful mirror of Rust `usize`.
-/

namespace Pmt

/-! ## `USize` — `usize` on a 64-bit target, modelled as `Fin (2^64)`. -/

/-- `usize` on a 64-bit target is exactly the finite ordinal `Fin (2^64)`.

    Marked `@[reducible]` so the `Fin` `Ord`/`Decidable` instances (used by
    the `<` comparison in `Arena.alloc`) resolve by unfolding. -/
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

-- The bump allocator does not consult `align`; it is carried as an
-- IR-level parameter for parity with Rust's `Alloc { size, align }`, so
-- we disable the `unusedVariables` linter for this definition.
set_option linter.unusedVariables false in
/-- Checked bump allocation. Returns `none` when `USize.add a.used size`
    overflows *or* the resulting bump offset meets/exceeds `capacity`; on
    success bumps `used`, increments `alloc_id`, and returns a pointer with
    `addr = base.addr + used.val` (old offset) and `provenance = alloc_id`. -/
def Arena.alloc (a : Arena) (size align : USize) : Option (Arena × Ptr) :=
  match USize.add a.used size with
  | none => none
  | some new_used =>
    if new_used < a.capacity then
      some ({ a with used := new_used, alloc_id := a.alloc_id + 1 },
            { addr := a.base.addr + a.used.val, provenance := a.alloc_id })
    else none

/-! ## `IrInstr` — the three-instruction IR subset. -/

/-- The IR subset: `alloc`, `free`, and `stateRead`, mirroring the Rust
    `Instr` enum. `size`/`align` are `USize` (Rust `usize`). -/
inductive IrInstr where
  | alloc     (dst : String) (size align : USize)
  | free      (src : String)
  | stateRead (dst src field : String)

/-! ## `Env` — a name → pointer environment. -/

/-- A variable environment maps each name to the pointer it currently
    holds (or `none` if unbound). -/
def Env := String → Option Ptr

/-! ## `Step` — small-step semantics.

    Each instruction has an explicit *ok* and *err* constructor. The error
    constructors are first-class (not merely the absence of the ok case):
    `alloc_err` / `free_err` / `read_err` each leave both the arena and the
    environment unchanged. This is what the downstream simulation proof
    needs to case-split on the outcome of every instruction. -/

set_option linter.unusedVariables false in
/-- `Step i a env a' env'` holds when executing `i` in arena `a` under
    environment `env` yields arena `a'` and environment `env'`. -/
inductive Step : IrInstr → Arena → Env → Arena → Env → Prop where
  | alloc_ok  : ∀ a a' env p dst size align,
      Arena.alloc a size align = some (a', p) →
      Step (IrInstr.alloc dst size align) a env a'
        (fun x => if x = dst then some p else env x)
  | alloc_err : ∀ a env dst size align,
      Arena.alloc a size align = none →
      Step (IrInstr.alloc dst size align) a env a env
  | free_ok   : ∀ a env src p,
      env src = some p →
      Step (IrInstr.free src) a env a
        (fun x => if x = src then none else env x)
  | free_err  : ∀ a env src,
      env src = none →
      Step (IrInstr.free src) a env a env
  | read_ok   : ∀ a env src dst field p base off (sz : Nat),
      env src = some p →
      p.addr = base + off →
      Step (IrInstr.stateRead dst src field) a env a
        (fun x => if x = dst then some p else env x)
  | read_err  : ∀ a env src dst field,
      env src = none →
      Step (IrInstr.stateRead dst src field) a env a env

end Pmt
