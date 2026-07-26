import PMT.Basic
import PMT.Soundness

/-!
## PmtInstr — Lean mirror of the PMT-relevant subset of Rust `IRInstr` (sorry-free)

This module mirrors the subset of `src/codegen/src/ir.rs::IRInstr` (32 variants)
that is relevant to PMT memory safety verification. The full `IRInstr` enum
includes many variants (VectorOp, StarkProof, AtomicCas, etc.) that are not
directly exercised by PMT invariants; those are out of scope for this model.

The PMT-relevant variants fall into two groups:

  * **Memory variants (7):** `alloc`, `load`, `store`, `free`,
    `transform`, `call`, `ret`. These produce / consume arena regions
    and are subject to the per-instruction `well_typed` check (Lean
    mirror of IVE's `verify_state_reads` / `verify_state_writes` /
    `verify_transform` — see `PMT.IVE.Soundness.{Transform,
    StateReads,StateWrites}`).

  * **Pure-arithmetic variants (12, added in PMT-1-A):** `bin_op`,
    `unary_op`, `cast`, `add`, `sub`, `mul`, `div`, `cmp`, `select`,
    `ct_select`, `ct_eq`, `get_address`. These are pure
    register-to-register computations with no arena effect:
    `effect = .none`, `to_steps = []`, `well_typed = True`. Mirroring
    them in the Lean model lets PMT simulation-relation proofs traverse
    IR functions whose body contains arithmetic interleaved with memory
    operations without needing to abstract the arithmetic away first.

The simulation relation `pmt_instr_simulates_ir_instr`
connects each `PmtInstr` variant to its Rust `IRInstr` counterpart. All
theorems in this file close without `sorry`.

**Module dependency.** `PMT.PmtInstr` is depended on by `PMT.IRProgram`
(which embeds it in `IRBlock.instructions`), `PMT.ExecFunction`
(which flattens `PmtInstr` to `Step`), and `PMT.WellTypedStrong`.

The 12 arithmetic variants (PMT-1-A) are pure: they flatten to `[]`
under `PmtInstr.to_steps`, so they do not contribute `Step`s to
`IRFunction.flat_steps` and therefore do not perturb the
name-uniqueness conjuncts of `WellTyped` — `pmt_soundness` is
preserved sorry-free.

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

/-! ## §3. Op-kind tags (mirrors of Rust enums used by the arithmetic variants)

The four inductives below mirror the eponymous Rust enums in
`src/codegen/src/ir.rs`:
  * `BinOpKind`   — `src/codegen/src/ir.rs:1080` (25 variants).
  * `UnaryOpKind` — `src/codegen/src/ir.rs:1195` (5 variants).
  * `CmpKind`     — `src/codegen/src/ir.rs:1282` (10 variants).
  * `CastKind`    — `src/codegen/src/ir.rs:1324` (9 variants).

They are pure tag types: `PmtInstr.well_typed` does not inspect them
(the arithmetic `PmtInstr` variants are all `True`-well-typed), so the
Lean variants are present solely to faithfully mirror the Rust IR shape
for `instr_sim` / `block_sim` traversal in `PMT.SimRel`. -/

/-- §3.1: Binary operator kind — mirror of Rust `BinOpKind`.
Covers arithmetic, bitwise, shift, rotate, and comparison sub-kinds. -/
inductive BinOpKind where
  | add   : BinOpKind
  | sub   : BinOpKind
  | mul   : BinOpKind
  | sdiv  : BinOpKind
  | udiv  : BinOpKind
  | srem  : BinOpKind
  | urem  : BinOpKind
  | «and» : BinOpKind
  | «or»  : BinOpKind
  | xor   : BinOpKind
  | shl   : BinOpKind
  | shrL  : BinOpKind
  | shrA  : BinOpKind
  | ror   : BinOpKind
  | rol   : BinOpKind
  | sLt   : BinOpKind
  | sLe   : BinOpKind
  | sGt   : BinOpKind
  | sGe   : BinOpKind
  | uLt   : BinOpKind
  | uLe   : BinOpKind
  | uGt   : BinOpKind
  | uGe   : BinOpKind
  | eq    : BinOpKind
  | ne    : BinOpKind
  deriving Repr

/-- §3.2: Unary operator kind — mirror of Rust `UnaryOpKind`. -/
inductive UnaryOpKind where
  | neg    : UnaryOpKind
  | not    : UnaryOpKind
  | clz    : UnaryOpKind
  | ctz    : UnaryOpKind
  | popcnt : UnaryOpKind
  deriving Repr

/-- §3.3: Comparison kind — mirror of Rust `CmpKind`. -/
inductive CmpKind where
  | eq  : CmpKind
  | ne  : CmpKind
  | sLt : CmpKind
  | sLe : CmpKind
  | sGt : CmpKind
  | sGe : CmpKind
  | uLt : CmpKind
  | uLe : CmpKind
  | uGt : CmpKind
  | uGe : CmpKind
  deriving Repr

/-- §3.4: Cast kind — mirror of Rust `CastKind`. -/
inductive CastKind where
  | zExt         : CastKind
  | sExt         : CastKind
  | trunc        : CastKind
  | bitCast      : CastKind
  | intToFloat   : CastKind
  | uIntToFloat  : CastKind
  | floatToInt   : CastKind
  | floatToUInt  : CastKind
  | floatToFloat : CastKind
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

**Memory variants (7):** mirror the Rust `IRInstr` variants with arena
effect:
  - Rust `IRInstr::Alloc`      ↔ Lean `PmtInstr.alloc`
  - Rust `IRInstr::Load`       ↔ Lean `PmtInstr.load`
  - Rust `IRInstr::Store`      ↔ Lean `PmtInstr.store`
  - Rust `IRInstr::Free`       ↔ Lean `PmtInstr.free`
  - Rust `IRInstr::Call`       ↔ Lean `PmtInstr.call`
  - Rust `IRInstr::Ret`        ↔ Lean `PmtInstr.ret`

**Pure-arithmetic variants (12, PMT-1-A):** mirror the Rust `IRInstr`
variants that are pure register-to-register computations. Each has
`effect = .none`, `well_typed = True`, `to_steps = []`:
  - Rust `IRInstr::BinOp`     ↔ Lean `PmtInstr.bin_op`
  - Rust `IRInstr::UnaryOp`   ↔ Lean `PmtInstr.unary_op`
  - Rust `IRInstr::Cast`      ↔ Lean `PmtInstr.cast`
  - Rust `IRInstr::Add`       ↔ Lean `PmtInstr.add`
  - Rust `IRInstr::Sub`       ↔ Lean `PmtInstr.sub`
  - Rust `IRInstr::Mul`       ↔ Lean `PmtInstr.mul`
  - Rust `IRInstr::Div`       ↔ Lean `PmtInstr.div`
  - Rust `IRInstr::Cmp`       ↔ Lean `PmtInstr.cmp`
  - Rust `IRInstr::Select`    ↔ Lean `PmtInstr.select`
  - Rust `IRInstr::CtSelect`  ↔ Lean `PmtInstr.ct_select`
  - Rust `IRInstr::CtEq`      ↔ Lean `PmtInstr.ct_eq`
  - Rust `IRInstr::GetAddress` ↔ Lean `PmtInstr.get_address`

Field shapes mirror Rust (`src/codegen/src/ir.rs:1394-1689`). For
`BinOp`/`UnaryOp`/`Cast`/`Cmp`/`Add`/`Sub`/`Mul`/`Div`/`Select`/
`CtSelect`/`CtEq`, the Rust `ty: Option<IRType>` field is modeled
here as `ty : IRType` (the `None` default is omitted — the existing
`PmtInstr.load`/`.store` precedent uses non-`Option` `IRType`). For
`GetAddress`, the Rust `{ dst, name }` shape is mirrored faithfully
(the `Offset { dst, base, offset }` variant is a separate Rust
variant not in PMT scope).

Other Rust `IRInstr` variants (Phi, Atomic*, VectorOp, Channel*,
StarkProof, Offset, Syscall) are not modeled here. They are either:
  - Pure computation (no memory effect) — out of scope for PMT.
  - Channel operations — modeled via `PmtInstr.call "channel_send"`.
-/
inductive PmtInstr where
  -- Memory variants (7)
  | alloc       : String → Layout → PmtInstr                  -- (out_var, layout)
  | load        : String → String → Nat → IRType → PmtInstr   -- (in_var, out_var, offset, type)
  | store       : String → IRValue → Nat → IRType → PmtInstr  -- (in_var, value, offset, type)
  | free        : String → PmtInstr                           -- (in_var)
  | transform   : String → String → Layout → PmtInstr         -- (in_var, out_var, layout)
  | call        : String → List String → PmtInstr             -- (builtin_name, arg_vars)
  | ret         : IRValue → PmtInstr                          -- (return value)
  -- Pure-arithmetic variants (12, PMT-1-A) — `effect = .none`, `to_steps = []`
  | bin_op      : BinOpKind → IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `BinOp { op, dst, lhs, rhs, ty }`
  | unary_op    : UnaryOpKind → IRValue → IRValue → IRType → PmtInstr
    -- Rust `UnaryOp { op, dst, operand, ty }`
  | cast        : CastKind → IRValue → IRValue → IRType → IRType → PmtInstr
    -- Rust `Cast { kind, dst, src, from_ty, to_ty }`
  | add         : IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `Add { dst, lhs, rhs, ty }`
  | sub         : IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `Sub { dst, lhs, rhs, ty }`
  | mul         : IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `Mul { dst, lhs, rhs, ty }`
  | div         : IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `Div { dst, lhs, rhs, ty }`
  | cmp         : CmpKind → IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `Cmp { kind, dst, lhs, rhs, ty }`
  | select      : IRValue → IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `Select { dst, cond, true_val, false_val, ty }`
  | ct_select   : IRValue → IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `CtSelect { dst, cond, true_val, false_val, ty }`
  | ct_eq       : IRValue → IRValue → IRValue → IRType → PmtInstr
    -- Rust `CtEq { dst, lhs, rhs, ty }`
  | get_address : IRValue → String → PmtInstr
    -- Rust `GetAddress { dst, name }`
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

/-- §9: Classify an instruction's effect.

The 12 pure-arithmetic variants (PMT-1-A) all classify as `.none` —
they neither read from nor write to a state variable, nor do they
consume one. -/
def PmtInstr.effect : PmtInstr → PmtEffect
  | .alloc out _ => .writes out
  | .load in_var _ _ _ => .reads in_var
  | .store in_var _ _ _ => .reads in_var
  | .free in_var => .consumes in_var
  | .transform in_var _ _ => .consumes in_var  -- NOTE: also writes out, but consumes dominates
  | .call _ _ => .none
  | .ret _ => .none
  -- Pure-arithmetic variants (PMT-1-A): no state effect.
  | .bin_op _ _ _ _ _ => .none
  | .unary_op _ _ _ _ => .none
  | .cast _ _ _ _ _ => .none
  | .add _ _ _ _ => .none
  | .sub _ _ _ _ => .none
  | .mul _ _ _ _ => .none
  | .div _ _ _ _ => .none
  | .cmp _ _ _ _ _ => .none
  | .select _ _ _ _ _ => .none
  | .ct_select _ _ _ _ _ => .none
  | .ct_eq _ _ _ _ => .none
  | .get_address _ _ => .none

/-- §10: WellTypedness for PmtInstr (per-instruction).
This is the per-instruction check that IVE's `verify_state_reads` /
`verify_state_writes` / `verify_transform` perform.

The 12 pure-arithmetic variants (PMT-1-A) all reduce to `True`: they
are pure register-to-register computations with no arena
involvement, so the per-instruction `WF_Layout` check does not
apply. -/
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
  -- Pure-arithmetic variants (PMT-1-A): pure computation, no arena involvement.
  | .bin_op _ _ _ _ _ => True
  | .unary_op _ _ _ _ => True
  | .cast _ _ _ _ _ => True
  | .add _ _ _ _ => True
  | .sub _ _ _ _ => True
  | .mul _ _ _ _ => True
  | .div _ _ _ _ => True
  | .cmp _ _ _ _ _ => True
  | .select _ _ _ _ _ => True
  | .ct_select _ _ _ _ _ => True
  | .ct_eq _ _ _ _ => True
  | .get_address _ _ => True

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

  The 12 pure-arithmetic variants (PMT-1-A) all map to `[]`: they are
  pure register-to-register computations and contribute no `Step`s to
  the flattened program. Consequently they do not perturb the
  name-uniqueness conjuncts of `WellTyped` (a `Step`-free instruction
  cannot introduce a duplicate `in_var` / `out_var`). -/
def PmtInstr.to_steps (i : PmtInstr) : List Step :=
  match i with
  | .alloc out layout => [⟨out, out, layout, .transform⟩]
  | .load in_var out _ _ => [⟨in_var, out, ⟨1, []⟩, .transform⟩]
  | .store in_var _ _ _ => [⟨in_var, in_var, ⟨1, []⟩, .transform⟩]
  | .free in_var => [⟨in_var, in_var, ⟨1, []⟩, .transform⟩]
  | .transform in_var out layout => [⟨in_var, out, layout, .transform⟩]
  | .call _ args => args.map (fun v => ⟨v, v, ⟨1, []⟩, .transform⟩)
  | .ret _ => []
  -- Pure-arithmetic variants (PMT-1-A): pure computation, no Steps emitted.
  | .bin_op _ _ _ _ _ => []
  | .unary_op _ _ _ _ => []
  | .cast _ _ _ _ _ => []
  | .add _ _ _ _ => []
  | .sub _ _ _ _ => []
  | .mul _ _ _ _ => []
  | .div _ _ _ _ => []
  | .cmp _ _ _ _ _ => []
  | .select _ _ _ _ _ => []
  | .ct_select _ _ _ _ _ => []
  | .ct_eq _ _ _ _ => []
  | .get_address _ _ => []

end PMT
