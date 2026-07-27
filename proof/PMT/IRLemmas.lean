import PMT.IRProgram
import PMT.ExecFunction
import PMT.SimRel

/-! ## IR Lemmas — supporting lemmas for IRProgram/IRFunction/IRBlock

These lemmas support the simulation relation proofs and the extraction
pipeline. They establish basic properties of the IR data structures and
unbundle the three simulation layers (`program_sim` → `function_sim` →
`block_sim` → `instr_sim_intra_lean`).

All theorems in this file close without `sorry`, except §3
(`to_program_length`), which is left as a documented TODO for
`List.length_flatMap` reasoning — see the comment on the theorem.

**References.**
  * `PMT.IRProgram` — IR types (`IRProgram`, `IRFunction`, `IRBlock`,
    `IRTerminator`, `DataSection`) and `well_typed` predicates.
  * `PMT.ExecFunction` — flattening `IRProgram.to_program` / etc.
  * `PMT.SimRel` — `instr_sim_intra_lean`, `block_sim`, `function_sim`,
    `program_sim` definitions.

**Build.** Part of the Lake package rooted at `proof/lakefile.toml`.
Build with `lake build PMT.IRLemmas`.
-/

namespace PMT

/-! ## §1. Structural lemmas about `IRProgram.to_program` -/

/-- §1: An empty `IRProgram` (no functions) flattens to `[]`.

This duplicates `IRProgram.to_program_empty` (in `PMT.ExecFunction`)
under a more descriptive name. Kept here so simulation-relation callers
have a one-stop import for IR structural lemmas. -/
theorem IRProgram.empty_no_functions :
    IRProgram.to_program ⟨[], []⟩ = [] := by
  rfl

/-- §2: `to_program` of a single-function program is that function's body.

`IRProgram.to_program` only flattens the *first* function in
`p.functions` (see `PMT.ExecFunction` §4). When there is exactly one
function, this is just that function's `to_program`. -/
theorem IRProgram.to_program_single_function
    (f : IRFunction) (data : List DataSection) :
    IRProgram.to_program ⟨[f], data⟩ = f.to_program := by
  rfl

/- §3: `to_program` length equals the sum of per-block instruction
counts (over the *first* function only — `IRProgram.to_program` ignores
subsequent functions, see `PMT.ExecFunction` §4).

This is a **TODO**: the actual length of `IRProgram.to_program p` is the
length of `f.to_program` (where `f = p.functions.head?`), which is the
length of `f.blocks.flatMap IRBlock.to_steps`, which in turn is
`Σ b, (b.instructions.flatMap PmtInstr.to_steps).length` — NOT simply
`Σ b, b.instructions.length`, because some `PmtInstr` variants
(`call`/`ret`) flatten to a non-singleton (or zero) number of `Step`s.

The precise theorem requires aligning `PmtInstr.to_steps` with the
instruction-count metric. For now, we state a simpler length fact
that IS provable: the to_program of an empty program is empty.

See `IRProgram.empty_no_functions` above for that simpler fact. -/
-- The full length-preservation theorem is deferred
-- once PmtInstr.to_steps is refined to consult `env`. It would require
-- List.length_flatMap reasoning and is intentionally omitted to keep
-- the library sorry-free.

/-! ## §4. WellTypedness propagation -/

/-- §4: A well-typed `IRProgram` has well-typed blocks (under the same
`env`). This unrolls `IRProgram.well_typed` (every function is
well-typed) and `IRFunction.well_typed` (every block is well-typed). -/
theorem IRProgram.well_typed_implies_blocks_well_typed
    (p : IRProgram) (env : String → Layout)
    (hwf : p.well_typed env) :
    ∀ f : IRFunction, f ∈ p.functions →
      ∀ b : IRBlock, b ∈ f.blocks → b.well_typed env := by
  intro f hf b hb
  -- hwf : p.well_typed env  ≡  ∀ f ∈ p.functions, f.well_typed env
  have hfunc : f.well_typed env := hwf f hf
  -- hfunc : f.well_typed env  ≡  (∀ b ∈ f.blocks, b.well_typed env) ∧ f.blocks ≠ []
  unfold IRFunction.well_typed at hfunc
  obtain ⟨hblocks, _⟩ := hfunc
  exact hblocks b hb

/-! ## §5-§7. Simulation-relation unbundling

Each `*_sim_implies_*_sim` lemma peels one layer off the simulation
relation, exposing the length equality and the per-index `*_sim`
hypothesis. These are used by the extraction/soundness pipeline to
go from `program_sim lean rust` down to individual `instr_sim_intra_lean`
hypotheses without re-doing the `And` destructuring at each call site.
-/

/-- §5: `block_sim` implies instruction-level simulation at every index.

`block_sim lean rust` bundles label-equality, instruction-length
equality, per-index `instr_sim_intra_lean`, and terminator-equality. This lemma
returns just the length-equality and per-index `instr_sim_intra_lean`.

Note: due to Lean's parsing of `∀ x, P ∧ Q` (the `∀` extends right), the
`terminator` equality actually lives *inside* the per-index `∀` in the
definition of `block_sim`. We extract just the `instr_sim_intra_lean` part. -/
theorem block_sim_implies_instr_sim
    (lean rust : IRBlock)
    (hsim : block_sim lean rust) :
    lean.instructions.length = rust.instructions.length
    ∧ ∀ i : Nat, i < lean.instructions.length →
        instr_sim_intra_lean (lean.instructions.get! i) (rust.instructions.get! i) := by
  unfold block_sim at hsim
  rcases hsim with ⟨_, hlen, hisim_with_term⟩
  refine ⟨hlen, ?_⟩
  intro i hi
  exact (hisim_with_term i hi).1

/-- §6: `function_sim` implies block-level simulation at every index.

`function_sim lean rust` bundles name-equality, block-length equality,
and per-index `block_sim`. This lemma returns just the length-equality
and per-index `block_sim`. -/
theorem function_sim_implies_block_sim
    (lean rust : IRFunction)
    (hsim : function_sim lean rust) :
    lean.blocks.length = rust.blocks.length
    ∧ ∀ i : Nat, i < lean.blocks.length →
        block_sim (lean.blocks.get! i) (rust.blocks.get! i) := by
  unfold function_sim at hsim
  rcases hsim with ⟨_, hlen, hbsim⟩
  exact ⟨hlen, hbsim⟩

/-- §7: `program_sim` implies function-level simulation at every index.

`program_sim lean rust` bundles function-length equality and per-index
`function_sim`. This lemma exposes both directly. -/
theorem program_sim_implies_function_sim
    (lean rust : IRProgram)
    (hsim : program_sim lean rust) :
    lean.functions.length = rust.functions.length
    ∧ ∀ i : Nat, i < lean.functions.length →
        function_sim (lean.functions.get! i) (rust.functions.get! i) := by
  unfold program_sim at hsim
  rcases hsim with ⟨hlen, hfsim⟩
  exact ⟨hlen, hfsim⟩

end PMT
