import PMT.Field

/-!
# PMT Liveness — §5 Liveness Predicate + §6 Guard Page (sorry-free)

A machine-checkable formalization of the liveness (ghost-state) and
guard-page invariants of the PMT (Programs as Memory Transformations)
memory model used by the VUMA compiler. This module encodes the Iris
specification in `docs/architecture/pmt-iris-spec.md` (§5–§6) into
plain Lean 4.

**Scope.** This module defines:
  * §5 — `linear_implies_accessible`, `state_transform`,
    `state_transform_kills_input` (the compile-time ghost-state half
    of liveness; the runtime mirror `[live_mirror]` is implemented by
    `inject_liveness_check_ir` at `src/codegen/src/memory_safety.rs:1513`,
    which HAS shipped — tombstone byte load + compare-to-0 + __uaf_trap).
  * §6 — `GuardPage`, `in_arena_below_guard` (trusted OS contract:
    mmap `PROT_NONE` guard page semantics; faithful to the codegen-emitted
    arena at `src/pipeline.rs:11565-11842`, NOT the testing mirror at
    `arena.rs` which documents the guard page as a TODO).

This module depends on the data model (`Arena`, `CapacityInvariant`)
from `PMT.Basic` and the linearity primitives (`LinearToken`,
`LinearResource`, `Accessible`, `Consumed`, `Liveness`) from
`PMT.Field` (§4). It is in turn imported by `PMT.Soundness`, which
uses `linear_implies_accessible` as the runtime liveness precondition
in `pmt_soundness`. All theorems close without `sorry`.

**Build.** This module is part of the Lake package rooted at
`proof/lakefile.toml`. Build with `lake build` (or `make proof` /
`just proof`); the `lean-proofs` CI job in
`.github/workflows/proof-verify.yml` runs the same command. The
legacy single-file `lean PMT/Liveness.lean` invocation does not work
since the multi-module split.
-/

namespace PMT

/-! ## §5. Liveness — LIVE/DEAD as Ghost State (compile-time half) -/

/-- §5 (PMT-FAITH-6-D, renamed): `linear_implies_accessible` — a linear
resource implies the token is accessible.

This is the ghost-state implication (Lean-side), NOT a Rust check. Rust's
IVE does NOT check liveness on reads (`verify_state_reads` state_read.rs:44-129
checks field bounds + type match only). Liveness is enforced on WRITES
(compile-time, via `consumed_vars`) and at RUNTIME (tombstone byte, via
`inject_liveness_check_ir` at memory_safety.rs:1513 — which HAS shipped).
The Lean ghost-state half pairs with the runtime tombstone. -/
theorem linear_implies_accessible
    (t : LinearToken)
    (hlive : LinearResource t) :
    Accessible t := by
  -- `Accessible t := LinearResource t`, which is `hlive`.
  exact hlive

/-- §5: `state_transform` consumes its input token (flips `live → dead`)
and produces a fresh `live` token for the output variable. Mirrors
`state_transform_kills_input` (`pmt-iris-spec.md` §5):

    {{ state_resource v_in p_in ∗ live v_in }} StateTransform
    {{ v_out, state_resource v_out p_out ∗ live v_out ∗ dead v_in }}. -/
def state_transform (tin : LinearToken) (out_name : String)
    (_hin : LinearResource tin) :
    LinearToken × LinearToken :=
  (⟨tin.var, Liveness.dead⟩, ⟨out_name, Liveness.live⟩)

/-- §5 corollary: after `state_transform`, the input token is `Consumed`
and the output token is `LinearResource`. This is the temporal-safety
half of the liveness theorem (`pmt-formal-spec.md` §5). -/
theorem state_transform_kills_input
    (tin : LinearToken) (out_name : String)
    (hin : LinearResource tin) :
    Consumed (state_transform tin out_name hin).1
    ∧ LinearResource (state_transform tin out_name hin).2 := by
  -- `state_transform` reduces definitionally to a pair of tokens;
  -- `(state_transform ...).1` is the dead input token,
  -- `(state_transform ...).2` is the live output token. Both sides are
  -- provable by `rfl` after definitional reduction.
  exact ⟨rfl, rfl⟩

/-! ## §6. Guard Page (trusted OS contract) -/

/-- §6: The guard page sits at `base + capacity` and is `PROT_NONE`.
Modeled as a pure fact: any access at `addr ≥ base + capacity` is
*physically impossible* (MMU traps). This is in the TCB
(`pmt-iris-spec.md` §8: "mmap PROT_NONE guard page semantics — Trusted").

**PMT-FAITH-6-D (closes FAITH-1-E):** the Rust counterpart is the
CODEGEN-EMITTED arena (`src/pipeline.rs:11565-11842`, which calls
`mprotect(PROT_NONE)` to install the guard page), NOT the Rust-level
testing arena (`src/codegen/src/runtime/arena.rs:14-21`, which documents
the guard page as a TODO "INV-PMT-1"). The Lean `GuardPage` is faithful
to the DEPLOYED arena (codegen path), not the testing mirror.

Stated with `≤` (rather than `≥`) so `omega` sees the arithmetic
without needing to unfold the `GE` notation. -/
def GuardPage (a : Arena) (addr : Nat) : Prop := a.base + a.capacity ≤ addr

/-- §6: Any byte address inside the live arena `[base, base+used)` is
strictly below the guard page. So a well-typed in-arena access never
trips the guard; only an overflow (which would require
`CapacityInvariant` to fail) does. -/
theorem in_arena_below_guard
    (a : Arena) (addr : Nat)
    (hcap : CapacityInvariant a)
    (hin  : a.base ≤ addr ∧ addr < a.base + a.used) :
    ¬ GuardPage a addr := by
  -- `GuardPage a addr := a.base + a.capacity ≤ addr`.
  -- `CapacityInvariant a := a.used ≤ a.capacity`.
  -- Unfold both so `omega` sees raw arithmetic.
  unfold GuardPage
  unfold CapacityInvariant at hcap
  intro hg
  -- `hin.2 : addr < a.base + a.used ≤ a.base + a.capacity` (via `hcap`),
  -- so `addr < a.base + a.capacity`, contradicting `hg : … ≤ addr`.
  omega

end PMT
