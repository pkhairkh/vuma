import PMT.PmtInstr
import PMT.Soundness

/-!
## IRProgram — Lean mirror of Rust `IRProgram` / `IRFunction` / `IRBlock` (sorry-free)

This module mirrors the top-level IR types from `src/codegen/src/ir.rs`:
  - `IRProgram`     ↔ Rust `IRProgram`     (functions + data sections)
  - `IRFunction`    ↔ Rust `IRFunction`    (name, params, blocks, source_file)
  - `IRBlock`       ↔ Rust `IRBlock`       (label, instrs, terminator, preds, succs)
  - `IRTerminator`  ↔ Rust `IRTerminator`  (7 variants: jump/branch/ret/unreachable/switch/invoke/tailCall)
  - `DataSection`   ↔ Rust `DataSection`   (name + data bytes)

The simulation relation connects Lean `IRProgram` to Rust `IRProgram`.
All theorems in this file close without `sorry`.

**References.**
  * Rust source: `src/codegen/src/ir.rs:1-200` (type definitions).
  * Related modules: `PMT.PmtInstr` (instruction type),
    `PMT.ExecFunction` (flattening to `Program`), `PMT.SimRel`.

Design notes:
  - `instructions : List PmtInstr` uses the simplified `PmtInstr` from
    `PMT.PmtInstr` (now 22 variants: 7 memory + 12 arithmetic + 3
    control-flow), NOT the full 32-variant `IRInstr`. The Lean/Rust
    simulation relation maps the 10 other `IRInstr` variants through
    `PmtInstr.call` / `PmtInstr.other`.
  - `predecessors`/`successors` use `List String` instead of Rust's
    `HashSet<String>` — sufficient for our soundness proofs since IVE
    traverses flat (`for func, block, instr`).
  - `IRTerminator.ret` (not `return`) — `return` is a Lean 4 contextual
    keyword in `do`-notation; using `ret` avoids parser ambiguity.
  - **Control flow vs. terminators (PMT-1-B).** Rust `IRInstr` has
    `Branch`/`CondBranch` as instruction-level variants (in
    `IRBlock.instructions`), *alongside* the `IRTerminator::Jump`/
    `IRTerminator::Branch` variants (in `IRBlock.terminator`). The Lean
    model mirrors this redundancy: `PmtInstr.branch` / `PmtInstr.cond_branch`
    live in `IRBlock.instructions`, while `IRTerminator.jump` /
    `IRTerminator.branch` live in `IRBlock.terminator`. The helper
    `PmtInstr.successor_labels` (§6.5) classifies the CFG-target labels
    contributed by each control-flow instruction; `IRBlock.successors_from_instrs`
    (§6.6) collects these across a block. Both are unused by the current
    straight-line flattening (`IRFunction.to_program`) but are provided
    for the future control-flow-aware refinement noted in `ExecFunction.lean` §3.

**Build.** This module is part of the Lake package rooted at
`proof/lakefile.toml`. Build with `lake build` (or `make proof` /
`just proof`); the `lean-proofs` CI job in
`.github/workflows/proof-verify.yml` runs the same command.
-/

namespace PMT

/-! ## §1. IRTerminator — block terminator. Mirrors Rust `IRTerminator` (7 variants). -/

/-- §1: `IRTerminator` — block terminator. Mirrors Rust `IRTerminator`
(ir.rs:2568). The 7 variants:

  | Lean            | Rust          | Args                                            |
  |-----------------|---------------|-------------------------------------------------|
  | `jump tgt`      | `Jump tgt`    | target label                                    |
  | `branch c t f`  | `Branch`      | cond, then_label, else_label                    |
  | `ret vals`      | `Return vals` | return values (renamed: `return` is a Lean kw)  |
  | `unreachable`   | `Unreachable` | —                                               |
  | `switch d ts`   | `Switch`      | discr, targets (default omitted)                |
  | `invoke f a n u`| `Invoke`      | func, args, normal, unwind                      |
  | `tailCall f a`  | `TailCall`    | func, args                                      |

Note: Rust `Switch` has a separate `default` label; we model it as
`(Int, String)` targets with the convention that the *last* entry is
the default (offset `0` is unused). The simulation relation documents
this mapping. -/
inductive IRTerminator where
  | jump        : String → IRTerminator                       -- target label
  | branch      : IRValue → String → String → IRTerminator    -- cond, then_label, else_label
  | ret         : List IRValue → IRTerminator                 -- return values
  | unreachable : IRTerminator
  | switch      : IRValue → List (Int × String) → IRTerminator
  | invoke      : String → List IRValue → String → String → IRTerminator
  | tailCall    : String → List IRValue → IRTerminator
  deriving Repr

/-! ## §2. IRBlock — basic block. Mirrors Rust `IRBlock`. -/

/-- §2: `IRBlock` — basic block. Mirrors Rust `IRBlock` (ir.rs:2726):
```rust
pub struct IRBlock {
    pub label: String,
    pub instructions: Vec<IRInstr>,
    pub terminator: IRTerminator,
    pub predecessors: HashSet<String>,
    pub successors: HashSet<String>,
    pub source_line: u32,
}
```
Simplifications:
  - `instructions : List PmtInstr` (not full `IRInstr`)
  - `predecessors`/`successors : List String` (not `HashSet`)
  - `source_line` omitted (not used by IVE-from-IR). -/
structure IRBlock where
  label        : String
  instructions : List PmtInstr       -- simplified: use PmtInstr, not full IRInstr
  terminator   : IRTerminator
  predecessors : List String         -- simplified: List, not HashSet
  successors   : List String
  deriving Repr

/-! ## §3. IRFunction — function. Mirrors Rust `IRFunction`. -/

/-- §3: `IRFunction` — function. Mirrors Rust `IRFunction` (ir.rs:2801):
```rust
pub struct IRFunction {
    pub name: String,
    pub params: Vec<IRValue>,
    pub results: Vec<IRValue>,
    pub param_types: Vec<IRType>,
    pub result_types: Vec<IRType>,
    pub vregs: HashMap<u32, VirtualRegister>,
    pub blocks: Vec<IRBlock>,         // first block is entry
    pub source_file: String,
}
```
Simplifications:
  - `vregs` omitted — `IRValue.register : Nat → IRValue` captures the
    ID; the vreg metadata (def site, type) is checked via `well_typed`.
  - `blocks : List IRBlock` (first = entry block). -/
structure IRFunction where
  name         : String
  params       : List IRValue
  param_types  : List IRType
  results      : List IRValue
  result_types : List IRType
  blocks       : List IRBlock        -- first = entry block
  source_file  : String
  deriving Repr

/-! ## §4. DataSection — data section. Mirrors Rust `DataSection`. -/

/-- §4: `DataSection` — data section. Mirrors Rust `DataSection`
(ir.rs:3006): `{ name, kind, align, data: Vec<u8> }`.

Simplifications:
  - `kind : DataSectionKind` (ReadOnly | Data | Bss) omitted — not
    used by PMT soundness; may be added later if the simulation
    relation needs to distinguish `.rodata` (immutable) from `.data`.
  - `align : u32` omitted — alignment is enforced via `WF_Layout`
    (PMT.Basic §1.2) on the consumer side, not on the section itself.
  - `data : List Nat` — bytes as Nat list (avoids `ByteArray` for
    simplicity; the simulation relation will use `ByteArray` for the
    Rust-side mirror). -/
structure DataSection where
  name : String
  size : Nat
  data : List Nat                   -- simplified: bytes as Nat list
  deriving Repr

/-! ## §5. IRProgram — top-level program. Mirrors Rust `IRProgram`. -/

/-- §5: `IRProgram` — top-level program. Mirrors Rust `IRProgram`
(ir.rs:3033):
```rust
pub struct IRProgram {
    pub functions: Vec<IRFunction>,
    pub data_sections: Vec<DataSection>,
}
```
A program is just functions + static data — no module table, no
foreign decls, no metadata. -/
structure IRProgram where
  functions     : List IRFunction
  data_sections : List DataSection
  deriving Repr

/-! ## §6. IRFunction helpers -/

/-! ## §6.5. Control-flow resolution helpers (PMT-1-B)

The three control-flow `PmtInstr` variants added in PMT-1-B (`phi`,
`branch`, `cond_branch`) carry no memory effect — they all flatten to
`[]` under `PmtInstr.to_steps`. Their semantics are instead resolved
at the CFG level: each contributes zero or more block labels to the
enclosing `IRBlock`'s successor set. The two helpers below expose
that CFG-level information so that the future control-flow-aware
flattening (noted in `ExecFunction.lean` §3) and any CFG-consistency
strengthening of `IRBlock.well_typed` (§9) can consult it without
re-implementing the constructor case-split.

These helpers are not used by the current `pmt_soundness` proof (which
operates on the flattened `Program = List Step`, to which
control-flow instructions contribute nothing). They are provided for
forward-compatibility with the control-flow-aware refinement. -/

/-- §6.5: `PmtInstr.successor_labels` — the CFG successor block labels
contributed by a `PmtInstr`. Returns:
  * `[]` for every non-control-flow instruction (memory, arithmetic, `phi`),
  * `[target]` for `branch target`,
  * `[true_target, false_target]` for `cond_branch cond true_target false_target`.

`phi` returns `[]` because it consumes predecessors (its `incoming`
pairs reference predecessor blocks) rather than producing successors —
the predecessor-side classification is `PmtInstr.predecessor_labels`
(§6.5.1). -/
def PmtInstr.successor_labels : PmtInstr → List String
  | .branch target => [target]
  | .cond_branch _ true_target false_target => [true_target, false_target]
  | _ => []

/-- §6.5.1: `PmtInstr.predecessor_labels` — the CFG predecessor block
labels referenced by a `PmtInstr`. Returns:
  * the list of `incoming` predecessor labels for `phi`,
  * `[]` for every other instruction.

This is the predecessor-side analog of `PmtInstr.successor_labels`. -/
def PmtInstr.predecessor_labels : PmtInstr → List String
  | .phi _ incoming => incoming.map (·.2)
  | _ => []

/-- §6.5.2: `PmtInstr.is_terminator` — `true` for the two control-flow
variants that end a basic block (`branch`, `cond_branch`). Returns
`false` for `phi` (which is typically the *first* instruction of a
block, not the last) and for every non-control-flow instruction. -/
def PmtInstr.is_terminator : PmtInstr → Bool
  | .branch _ => true
  | .cond_branch _ _ _ => true
  | _ => false

/-- §6.6: `IRBlock.successors_from_instrs` — collect the CFG successor
labels contributed by the control-flow instructions in a block's
`instructions` list. This is the instruction-derived analog of
`IRBlock.successors` (which is the explicit, declared successor list).

The two need not agree in general — the explicit `successors` field
may include targets reached via the `IRTerminator` that are not
expressible as `PmtInstr` control-flow variants. A future
CFG-consistency strengthening of `IRBlock.well_typed` (§9) may require
`successors_from_instrs ⊆ successors`. -/
def IRBlock.successors_from_instrs (b : IRBlock) : List String :=
  b.instructions.flatMap PmtInstr.successor_labels

/-- §6: Entry block of a function (first block). Returns `none` if the
function has no blocks (which violates `IRFunction.well_typed`). -/
def IRFunction.entry (f : IRFunction) : Option IRBlock :=
  f.blocks.head?

/-- §7: Find a block by label in a function. Returns `none` if no block
has the given label. -/
def IRFunction.find_block (f : IRFunction) (label : String) : Option IRBlock :=
  f.blocks.find? (·.label = label)

/-- §8: All instructions in a function (flattened across blocks).
Mirrors the IVE-from-IR traversal pattern:
`for func in &program.functions { for block in &func.blocks { for instr in &block.instructions { … } } }`. -/
def IRFunction.all_instrs (f : IRFunction) : List PmtInstr :=
  f.blocks.flatMap (·.instructions)

/-! ## §9-§11. WellTypedness predicates -/

/-- §8.1: `IRFunction.flat_steps` — the flattened `List Step` of a function,
obtained by concatenating `PmtInstr.to_steps` over `f.all_instrs`.

This is the IR-side mirror of `IRFunction.to_program` (defined in
`PMT/ExecFunction.lean`). The two are *definitionally equal* modulo
`List.flatMap_assoc` (see `to_program_preserves_well_typed_full`'s
proof for the rewrite). Defining it here — in `IRProgram.lean`, where
the IR types live — lets `IRFunction.well_typed` refer to the
post-flattening step list directly, which is what makes the
name-uniqueness precondition (§10, conjuncts 3 & 4) line up exactly
with the `WellTyped` name-uniqueness conclusion (§5.1, conjuncts 2 & 3)
of `IRProgram.to_program_preserves_well_typed_full`. -/
def IRFunction.flat_steps (f : IRFunction) : List Step :=
  f.all_instrs.flatMap PmtInstr.to_steps

/-- §8.2: `IRFunction.in_vars_unique` — the `in_var`s of the flattened
step list are pairwise distinct. Mirrors the runtime SSA-like
discipline: each variable name appears as the reader of at most one
`Step`. -/
def IRFunction.in_vars_unique (f : IRFunction) : Prop :=
  f.flat_steps.Pairwise (fun s1 s2 => s1.in_var ≠ s2.in_var)

/-- §8.3: `IRFunction.out_vars_unique` — the `out_var`s of the flattened
step list are pairwise distinct. Mirrors the runtime single-definition
discipline: each variable name is produced by at most one `Step`. -/
def IRFunction.out_vars_unique (f : IRFunction) : Prop :=
  f.flat_steps.Pairwise (fun s1 s2 => s1.out_var ≠ s2.out_var)

/-- §9: `IRBlock.well_typed` — every instruction is well-typed (delegates
to `PmtInstr.well_typed`) AND every predecessor label is either the
block's own label or in its successors list (CFG consistency check).

This is the block-level predicate that may be strengthened
further (e.g., to use `PmtInstr.well_typed` more aggressively). -/
def IRBlock.well_typed (b : IRBlock) (env : String → Layout) : Prop :=
  -- (a) Every instruction in the block is well-typed.
  (∀ i : PmtInstr, i ∈ b.instructions → i.well_typed env)
  -- (b) Every predecessor label is either the block's own label or
  --     in its successors list (CFG consistency, simplified).
  ∧ (∀ p : String, p ∈ b.predecessors → p = b.label ∨ p ∈ b.successors)

/-- §10: `IRFunction.well_typed` — every block is well-typed, the
function has at least one block (entry block exists), AND the flattened
step list satisfies the SSA-like name-uniqueness discipline:
  * (c) `in_var`s of `f.flat_steps` are pairwise distinct (each name is
    read by at most one step — matches the runtime linear-use invariant).
  * (d) `out_var`s of `f.flat_steps` are pairwise distinct (each name is
    produced by at most one step — matches the runtime single-definition
    invariant, mirroring the per-function uniqueness of `IRValue.register`
    IDs in the Rust IVE-from-IR pass).

**Conjuncts (c) and (d) are NEW.** They close the
precondition gap: without them,
`IRProgram.well_typed` only enforces the `WF_Layout` half of `WellTyped`
(the first conjunct), and the theorem
`to_program_preserves_well_typed_full` is unprovable (see the concrete
counterexamples in the gap report). With them, the theorem closes
sorry-free (see `PMT/ExecFunction.lean` §5.1).

The conjuncts are defined in terms of the *flattened* `to_steps` output
of `f.all_instrs`, not in terms of the IR's own variable occurrences —
the relevant uniqueness is on the post-flattening step list, which is
what `WellTyped` quantifies over. -/
def IRFunction.well_typed (f : IRFunction) (env : String → Layout) : Prop :=
  -- (a) Every block in the function is well-typed.
  (∀ b : IRBlock, b ∈ f.blocks → b.well_typed env)
  -- (b) The function has at least one block (entry block exists).
  ∧ f.blocks ≠ []
  -- (c) After flattening, no two steps share the same in_var.
  ∧ f.in_vars_unique
  -- (d) After flattening, no two steps share the same out_var.
  ∧ f.out_vars_unique

/-- §11: `IRProgram.well_typed` — every function in the program is
well-typed. This is the top-level predicate that the simulation
relation uses as the Lean-side precondition. -/
def IRProgram.well_typed (p : IRProgram) (env : String → Layout) : Prop :=
  ∀ f : IRFunction, f ∈ p.functions → f.well_typed env

end PMT
