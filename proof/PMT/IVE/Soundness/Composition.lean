import PMT.IVE.Soundness.Transform
import PMT.IVE.Soundness.StateReads
import PMT.IVE.Soundness.StateWrites

/-! ## IVE Soundness — Composition

This module proves the composition theorem: if all three IVE verifiers
(`verify_state_reads`, `verify_state_writes`, `verify_transform`) accept
a program, then the program satisfies all PMT memory-safety invariants.

This is the capstone theorem for IVE soundness. It ties together the
three independent soundness results proven in
`PMT.IVE.Soundness.StateReads`,
`PMT.IVE.Soundness.StateWrites`, and
`PMT.IVE.Soundness.Transform` into a single guarantee:

  **If the program is `FullyVerified`, then**
    (1) every read accesses a registered, in-bounds field, and
    (2) every write accesses a registered, in-bounds field of a *live*
        (non-consumed) variable, and
    (3) every transform produces well-formed layouts.

The corollary `fully_verified_no_memory_safety_traps` distills the
memory-safety-critical conjuncts (no `.oob`, no `.uaf`) for downstream
consumers (the simulation relation and extraction).

This module is `sorry`-free.
-/

namespace PMT.IVE.Soundness

/-- A program is "fully verified" if all three IVE verifiers accept it.

This is the conjunction of the three per-verifier acceptance predicates:
  - `reads_ok`      : `verify_state_reads env reads` returns all-`valid`.
  - `writes_ok`     : `verify_state_writes env consumed writes` returns all-`valid`.
  - `transforms_ok` : `verify_transform t` returns `valid = true` for every `t ∈ transforms`.

The `consumed` parameter carries the set of variables killed by earlier
`StateTransform` / `ForeignConsume` nodes (mirrors Rust's
`consumed_vars: HashSet<String>` threaded through `VerificationEngine::verify_pmt`
at `src/ive/src/verification.rs:474-506`). -/
structure FullyVerified
    (env : String → PMT.Layout)
    (consumed : List String)
    (reads : List StateRead)
    (writes : List StateWrite)
    (transforms : List StateTransform) : Prop where
  reads_ok      : ∀ v, v ∈ verify_state_reads env reads → v.valid = true
  writes_ok     : ∀ v, v ∈ verify_state_writes env consumed writes → v.valid = true
  transforms_ok : ∀ t, t ∈ transforms → (verify_transform t).valid = true

/-- Composition theorem: a fully-verified program satisfies all PMT
memory-safety invariants.

This is the capstone soundness result for IVE: the conjunction of the
three per-verifier soundness theorems
(`verify_state_reads_sound`, `verify_state_writes_sound`,
`verify_transform_sound`).

The three conjuncts of the conclusion correspond to the three
verifiers:
  (1) reads   : field registered in layout ∧ field's byte range in bounds.
  (2) writes  : field registered in layout ∧ byte range in bounds ∧
                target variable is live (not in `consumed`).
  (3) transforms : both `in_layout` and `out_layout` are well-formed.

The `hwf_env` hypothesis is the Lean-side analog of Rust's
`layouts.get(&layout_name)` returning `Some` with a well-formed
`LayoutInfo` — see `PMT.IVE.Soundness.StateReads` for the same
simplification. -/
theorem fully_verified_implies_pmt_invariants
    (env : String → PMT.Layout)
    (consumed : List String)
    (reads : List StateRead)
    (writes : List StateWrite)
    (transforms : List StateTransform)
    (hwf_env : ∀ var, PMT.WF_Layout (env var))
    (hfv : FullyVerified env consumed reads writes transforms) :
    -- All reads access registered, in-bounds fields.
    (∀ r : StateRead, r ∈ reads →
      r.f ∈ (env r.var).fields
      ∧ r.f.offset + r.f.size ≤ (env r.var).total_size)
    -- All writes access registered, in-bounds fields in live variables.
    ∧ (∀ w : StateWrite, w ∈ writes →
      w.f ∈ (env w.var).fields
      ∧ w.f.offset + w.f.size ≤ (env w.var).total_size
      ∧ w.var ∉ consumed)
    -- All transforms have well-formed layouts.
    ∧ (∀ t : StateTransform, t ∈ transforms →
      PMT.WF_Layout t.in_layout ∧ PMT.WF_Layout t.out_layout) := by
  refine ⟨?_, ?_, ?_⟩
  · -- Reads: delegate to `verify_state_reads_sound`.
    exact verify_state_reads_sound env reads hwf_env hfv.reads_ok
  · -- Writes: delegate to `verify_state_writes_sound`.
    exact verify_state_writes_sound env consumed writes hwf_env hfv.writes_ok
  · -- Transforms: delegate to `verify_transform_sound` per-transform.
    intro t ht
    have h := hfv.transforms_ok t ht
    have h_sound := verify_transform_sound t h
    exact ⟨h_sound.1, h_sound.2.1⟩

/-- Corollary: a fully-verified program never traps with `.oob` or `.uaf`.

This distils the three memory-safety-critical conjuncts out of
`fully_verified_implies_pmt_invariants`:
  (1) No read traps with `.oob`  — read byte ranges always fit.
  (2) No write traps with `.uaf` — writes never target consumed vars.
  (3) No write traps with `.oob` — write byte ranges always fit.

Together, these are the "no memory-safety traps" guarantee that the
PMT-simulation layer and the extraction consume. -/
theorem fully_verified_no_memory_safety_traps
    (env : String → PMT.Layout)
    (consumed : List String)
    (reads : List StateRead)
    (writes : List StateWrite)
    (transforms : List StateTransform)
    (hwf_env : ∀ var, PMT.WF_Layout (env var))
    (hfv : FullyVerified env consumed reads writes transforms) :
    -- (1) No read traps with `.oob`.
    (∀ r : StateRead, r ∈ reads →
      r.f.offset + r.f.size ≤ (env r.var).total_size)
    -- (2) No write traps with `.uaf` (writes to consumed variables).
    ∧ (∀ w : StateWrite, w ∈ writes → w.var ∉ consumed)
    -- (3) No write traps with `.oob`.
    ∧ (∀ w : StateWrite, w ∈ writes →
      w.f.offset + w.f.size ≤ (env w.var).total_size) := by
  have h := fully_verified_implies_pmt_invariants
    env consumed reads writes transforms hwf_env hfv
  refine ⟨?_, ?_, ?_⟩
  · -- (1) No read `.oob`: second conjunct of the reads invariant.
    intro r hr
    have h_read := h.1 r hr
    exact h_read.2
  · -- (2) No write `.uaf`: third conjunct of the writes invariant.
    intro w hw
    have h_write := h.2.1 w hw
    exact h_write.2.2
  · -- (3) No write `.oob`: second conjunct of the writes invariant.
    intro w hw
    have h_write := h.2.1 w hw
    exact h_write.2.1

end PMT.IVE.Soundness
