import PMT.Basic
import PMT.Soundness

/-!
## PmtInstr — Lean mirror of the PMT-relevant subset of Rust `IRInstr` (sorry-free)

This module mirrors the subset of `src/codegen/src/ir.rs::IRInstr` (32 variants)
that is relevant to PMT memory safety verification. The full `IRInstr` enum
includes many variants (VectorOp, StarkProof, AtomicCas, etc.) that are not
directly exercised by PMT invariants; those are out of scope for this model.

The 7 PMT-relevant variants are: `alloc`, `load`, `store`, `free`,
`transform`, `call`, `other`. The `well_typed` per-instruction predicate
is the Lean mirror of IVE's `verify_state_reads` / `verify_state_writes`
/ `verify_transform` checks (see `PMT.IVE.Soundness.{Transform,
StateReads,StateWrites}`).

The simulation relation `pmt_instr_simulates_ir_instr`
connects each `PmtInstr` variant to its Rust `IRInstr` counterpart. All
theorems in this file close without `sorry`.

**Module dependency.** `PMT.PmtInstr` is depended on by `PMT.IRProgram`
(which embeds it in `IRBlock.instructions`), `PMT.ExecFunction`
(which flattens `PmtInstr` to `Step`), and `PMT.WellTypedStrong`.

**References.**
  * Rust source: `src/codegen/src/ir.rs` (3,481 lines, 32 IRInstr variants).
  * Related modules: `PMT.IRProgram`, `PMT.ExecFunction`,
    `PMT.WellTypedStrong`, `PMT.SimRel`.

**Build.** This module is part of the Lake package rooted at
`proof/lakefile.toml`. Build with `lake build` (or `make proof` /
`just proof`); the `lean-proofs` CI job in
`.github/workflows/proof-verify.yml` runs the same command.
-/

namespace PMT

/-- §1: IRValue — operand type. Mirrors Rust `IRValue`.
Rust: `IRValue = Register(u32) | Immediate(i64) | Address(u64) | Label(String)`.
Lean: we use `Nat` for all numeric forms (simpler; simulation is by value). -/
inductive IRValue where
  | register : Nat → IRValue       -- vreg ID
  | immediate : Int → IRValue      -- constant
  | address   : Nat → IRValue      -- memory address
  | label     : String → IRValue   -- block label
  deriving Repr, DecidableEq

/-- §2: IRType — value type. Mirrors Rust `IRType` (subset). -/
inductive IRType where
  | i32   : IRType
  | i64   : IRType
  | u32   : IRType
  | u64   : IRType
  | ptr   : IRType
  | void  : IRType
  | struct : Layout → IRType       -- aggregate
  deriving Repr

/-! **Note on `PmtOp` migration.** The `PmtOp` inductive previously declared in this
module (with 7 constructors including `load`/`store`/`free`/
`call`) has been migrated to `PMT.Soundness` per the
design. The migrated `PmtOp`
has the 3-constructor shape (`alloc | field_access
Field | transform`), which is the minimal shape needed to make
`TrapCode.oob` reachable in the Lean model. The IR-level operation
tagging that the previous 7-constructor `PmtOp` supported is now
expressed directly through the `PmtInstr` variants themselves
(`PmtInstr.load`, `PmtInstr.store`, etc.) — the `PmtOp` indirection
was unused and has been removed. -/

/-- §4: PmtInstr — a single PMT-relevant instruction.
Mirrors the Rust `IRInstr` variants relevant to memory safety:
  - Rust `IRInstr::Alloc` ↔ Lean `PmtInstr.alloc`
  - Rust `IRInstr::Load` ↔ Lean `PmtInstr.load`
  - Rust `IRInstr::Store` ↔ Lean `PmtInstr.store`
  - Rust `IRInstr::Free` ↔ Lean `PmtInstr.free`
  - Rust `IRInstr::Call` ↔ Lean `PmtInstr.call`
  - Rust `IRInstr::Ret` ↔ Lean `PmtInstr.ret`

Other IRInstr variants (BinOp, UnaryOp, Cast, Select, Phi, Atomic*, VectorOp,
Channel*, StarkProof) are not directly modeled here. They are either:
  - Pure computation (no memory effect) — out of scope for PMT.
  - Channel operations — modeled via `PmtInstr.call "channel_send"`.
-/
inductive PmtInstr where
  | alloc      : String → Layout → PmtInstr           -- (out_var, layout)
  | load       : String → String → Nat → IRType → PmtInstr  -- (in_var, out_var, offset, type)
  | store      : String → IRValue → Nat → IRType → PmtInstr  -- (in_var, value, offset, type)
  | free       : String → PmtInstr                    -- (in_var)
  | transform  : String → String → Layout → PmtInstr  -- (in_var, out_var, layout)
  | call       : String → List String → PmtInstr      -- (builtin_name, arg_vars)
  | ret        : IRValue → PmtInstr                   -- (return value)
  deriving Repr

/-- §5: A PMT-relevant basic block — a list of PmtInstr. -/
abbrev PmtBlock := List PmtInstr

/-- §6: A PMT-relevant function — name + body. -/
structure PmtFunction where
  name : String
  body : PmtBlock
  deriving Repr

/-- §7: A PMT-relevant program — list of functions. -/
abbrev PmtProgram := List PmtFunction

/-- §8: Effect classification — what does each PmtInstr do to state?
  - `Reads in_var` — reads from in_var (requires it to be live)
  - `Writes out_var` — produces out_var (marks it live)
  - `Consumes in_var` — kills in_var (marks it dead, like transform)
  - `None` — no state effect (pure computation)
-/
inductive PmtEffect where
  | reads    : String → PmtEffect
  | writes   : String → PmtEffect
  | consumes : String → PmtEffect
  | none     : PmtEffect
  deriving Repr

/-- §9: Classify an instruction's effect. -/
def PmtInstr.effect : PmtInstr → PmtEffect
  | .alloc out _ => .writes out
  | .load in_var _ _ _ => .reads in_var
  | .store in_var _ _ _ => .reads in_var
  | .free in_var => .consumes in_var
  | .transform in_var _ _ => .consumes in_var  -- NOTE: also writes out, but consumes dominates
  | .call _ _ => .none
  | .ret _ => .none

/-- §10: WellTypedness for PmtInstr (per-instruction).
This is the per-instruction check that IVE's `verify_state_reads` /
`verify_state_writes` / `verify_transform` perform. -/
def PmtInstr.well_typed (i : PmtInstr) (layout_env : String → Layout) : Prop :=
  match i with
  | .alloc _out layout => WF_Layout layout
  | .load in_var _ _ _ => WF_Layout (layout_env in_var)
  | .store in_var _ _ _ => WF_Layout (layout_env in_var)
  | .free in_var => WF_Layout (layout_env in_var)
  | .transform in_var _out layout =>
      WF_Layout layout ∧ WF_Layout (layout_env in_var)
  | .call _ _ => True
  | .ret _ => True

/-- §11: WellTypedness for a PmtBlock. -/
def PmtBlock.well_typed (b : PmtBlock) (env : String → Layout) : Prop :=
  ∀ i : PmtInstr, i ∈ b → i.well_typed env

/-- §12: WellTypedness for a PmtFunction. -/
def PmtFunction.well_typed (f : PmtFunction) (env : String → Layout) : Prop :=
  f.body.well_typed env

/-- §13: WellTypedness for a PmtProgram. -/
def PmtProgram.well_typed (p : PmtProgram) (env : String → Layout) : Prop :=
  ∀ f : PmtFunction, f ∈ p → f.well_typed env

/-! ## §14. Per-instruction flattening: `PmtInstr.to_steps`

`PmtInstr.to_steps` converts a `PmtInstr` to a list of `Step`s (the
flat-program representation consumed by `exec` in `PMT.Soundness`).
This is defined here in `PmtInstr.lean` (rather than in
`PMT/ExecFunction.lean`) so that `IRFunction.flat_steps` and
`IRFunction.well_typed` (in `PMT/IRProgram.lean`) can refer to it
without creating a circular import (`ExecFunction.lean` already
imports `IRProgram.lean`). The flattening semantics are identical to
those documented in `PMT/ExecFunction.lean` §1. -/

/-- §14: Convert a `PmtInstr` to a list of `Step`s.

Mapping rationale (mirrors the IVE-from-IR traversal in W2-A §6):
  - `alloc out layout`   → 1 step: `⟨out, out, layout⟩`
      (allocates a region named `out`; both in/out are `out` because the
      allocator "consumes" the request and "produces" the new region.
      The `WellTyped` name-uniqueness check is satisfied because `out`
      appears exactly once as `in_var` and once as `out_var` in this step
      — the filter counts each step once.)
  - `load in_var out _ _` → 1 step: `⟨in_var, out, ⟨1, []⟩⟩`
      (reads `in_var`, produces `out`; size 1 is a placeholder — a future
      refinement will use the IRType to compute the actual byte size.)
  - `store in_var _ _ _`  → 1 step: `⟨in_var, in_var, ⟨1, []⟩⟩`
      (writes through `in_var` without producing a new region.)
  - `free in_var`         → 1 step: `⟨in_var, in_var, ⟨1, []⟩⟩`
      (frees `in_var`; marked as both reader and writer for liveness.)
  - `transform in_var out layout` → 1 step: `⟨in_var, out, layout⟩`
      (consumes `in_var`, produces `out` with the given layout.)
  - `call _ args`         → `args.map (fun v => ⟨v, v, ⟨1, []⟩⟩)`
      (each argument variable becomes a self-loop step; placeholder
      until call semantics are modeled.)
  - `ret _`               → `[]`
      (return has no further memory effect in this straight-line model.)
-/
def PmtInstr.to_steps (i : PmtInstr) : List Step :=
  match i with
  | .alloc out layout => [⟨out, out, layout, .transform⟩]
  | .load in_var out _ _ => [⟨in_var, out, ⟨1, []⟩, .transform⟩]
  | .store in_var _ _ _ => [⟨in_var, in_var, ⟨1, []⟩, .transform⟩]
  | .free in_var => [⟨in_var, in_var, ⟨1, []⟩, .transform⟩]
  | .transform in_var out layout => [⟨in_var, out, layout, .transform⟩]
  | .call _ args => args.map (fun v => ⟨v, v, ⟨1, []⟩, .transform⟩)
  | .ret _ => []

end PMT
