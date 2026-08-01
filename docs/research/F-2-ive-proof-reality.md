# F-2 — IVE + Lean Proof Reality Check

**Subagent**: F-2 (reaudit/ive-proof-reality)
**Task**: Verify the V-14 / V-16 / capability / Lean-proof claims against
actual source — correct the prior A-3 audit's undercount of the Lean proof
layer and its framing of the capability model.
**Repo state**: `main` at `6dc97e18` (2026-08-01)
**Method**: Read source only; no modifications except this report and the
worklog entry.

---

## Methodology

1. Read prior research context: `worklog.md`, `vuma-side-research-draft.md`,
   `research/A-3-ive-proofs-capability.md` (in full, 544 lines).
2. Read VUMA ground-truth docs: `language-reference.md` (§6 Builtins,
   §10 Formal Verification), `pmt-formal-spec.md` (§1–§8), `caveats.md`
   (§3 Verification), `proof/README.md` (in full).
3. Inventory `proof/` directory: 82 `.lean` files (LS of root + `PMT/` +
   `PMT/Iris/` + `PMT/Faithful/` + `PMT/IVE/Soundness/` + `PMT/Test/` +
   `PMT/FFI/`).
4. Read in full: `Basic.lean`, `BitVecArena.lean`, `MmapArena.lean`,
   `PipelineSim.lean`, `WellTypedStrong.lean` (partial),
   `Soundness.lean` (partial), `Iris/Composition.lean`,
   `Iris/CapBndInvariant.lean`, `Iris/HeapModel.lean`, `Faithful/Model.lean`,
   `Faithful/UafProof.lean`, `Faithful/Extract.lean`, `NoFFI.lean`,
   `Extraction.lean` (partial), `IVE/Soundness/SessionType.lean` (tail),
   `IVE/Soundness/L1L3Collapse.lean` (full).
5. Grep: `sorry` (word-boundary, PCRE2), `native_decide`, `admit`, `decide`,
   `alloc_preserves_pmt_invariants`, `Float|IEEE|f32|f64|verified_float`,
   `capability_grant|capability_delegate|verify_capability`,
   `VerificationSummary|total_checked|discharge_rate`,
   `is_pass|is_fail|is_unverified|VerificationStatus::`.
6. Read `src/codegen/src/capability.rs` (full),
   `src/codegen/src/ipc_lowering.rs` lines 4990–5120 (capability lowering),
   `src/codegen/src/runtime/arena.rs` (head + struct),
   `src/ive/src/verification.rs` lines 1700–2420 (`l1l3_collapse` +
   capability handling), `src/ive/src/invariant_aggregator.rs` lines
   260–420 (`VerificationSummary`, `pass_rate`),
   `src/bin/compile_dump.rs` lines 200–260.
7. Read `docs/adr/ADR-0008.md` (accepted decision to fix V-A3-3) and
   `scripts/archive/audit/wave3_ive_discharge.md` (head).

---

## Lean proof layer: actual scope

### Module inventory

The `proof/` directory holds **82 Lean files** (not "20+ modules" as the
language-reference.md §10 states):

| Subtree | Files | Role |
|---|---:|---|
| `proof/PMT/` (root) | 26 | PMT verification library — `Basic`, `Field`, `Liveness`, `Soundness`, `WellTypedStrong`, `PmtInstr`, `IRProgram`, `IRLemmas`, `ExecFunction`, `RawArena`, `BitVecArena`, `MmapArena`, `ArenaProperties`, `SimRel`, `PipelineSim`, `PillarSoundness`, `NoFFI`, `Extraction`, `ExtractionLemmas`, `AdditionalTheorems`, `MiscLemmas`, `HelperLemmas`, `Faithful.lean`, `check_pmt.lean` |
| `proof/PMT/Iris/` | 9 | Iris-style separation logic — `HeapModel`, `SepGenuine`, `CapBndInvariant`, `LiveMirrorInvariant`, `GuardInvariant`, `Composition`, `WeakestPrecond`, `ArenaRes`, `FractionalPerm` |
| `proof/PMT/Faithful/` | 22 | Faithful Lean↔Rust mirror — `Model`, `Agreement`, `Simulation`, `Simulation2`, `SimIpc`, `SimWrite`, `SimTransform`, `SimSound`, `SimSound2`, `Sep`, `CMRA`, `WP`, `WPSafety`, `FancyUpdate`, `ArenaInv`, `GuardInv`, `OverflowProof`, `UafProof`, `Extract`, `ExtractCorrect`, `RustConformance`, `IrSubset` |
| `proof/PMT/IVE/Soundness/` | 11 | IVE soundness — `StateReads`, `StateWrites`, `Transform`, `Composition`, `ArenaBounds`, `BorrowRegion`, `ConstraintInference`, `DependentTransform`, `InformationFlow`, `LayoutConsistency`, `SessionType`, `L1L3Collapse`, `WFLayoutBool` |
| `proof/PMT/IVE/` | 1 | `PillarSoundness.lean` |
| `proof/PMT/FFI/` | 1 | `PillarSoundness.lean` (legacy FFI variant) |
| `proof/PMT/Test/` | 10 | `ValidProgram`, `UafProgram`, `OverflowProgram`, `EmptyProgram`, `MultiStepProgram`, `ArenaBasicSim`, `SorryFreeAudit`, `PropertyTests`, `EdgeCases`, `RealisticProgram` |
| `proof/` (root + `Test/`) | 2 | `PMT.lean` (root), `Pmt.lean` (root), `Test/Main.lean` |

**Theorem count** (via `rg -c '^theorem ' proof/`): **280 theorems**, zero
lemmas. The doc's "~90 theorems" is a 3× understatement.

### `sorry` audit

`rg --pcre2 '(?<![\w/-])sorry(?![\w/-])' proof/ --glob '*.lean'` returns
**zero actual `sorry` tokens** — every match is the literal substring
inside comment phrases like "sorry-free", "no `sorry`", "the `sorry`", etc.

The doc's claim of "only 2 `sorry`s" in `language-reference.md` §10 and
`make proof-check` is **stale** — the source is fully sorry-free. The
`scripts/check-lean.sh` CI gate is set to strict mode
(`PROOF_CHECK_STRICT=1` in `.github/workflows/proof-verify.yml`) and would
fail on any `sorry` token in `lake build` output; the build passes.

### Shortcut-proof audit

`native_decide` is used in exactly **3 places**, all in
`proof/PMT/Faithful/Extract.lean:99, 104, 110`:
- `extract_nonempty : extract_alloc ≠ ""`
- `extract_has_overflow_check : extract_alloc.contains "checked_add"`
- `extract_has_capacity_check : extract_alloc.contains "new_used > capacity"`

These are substring-presence sanity theorems on a hardcoded Rust function
string (`extract_alloc` at line 87–95). The file's own docstring
(lines 27–32) is honest about this: *"three shallow, `native_decide`-powered
sanity theorems… No proof placeholders, no user-declared axioms; only
Lean's standard `Lean.ofReduceBool` and `propext`."*

A-3's framing was correct: these are substring sanity checks dressed up
as theorems, not semantic equivalence proofs. But A-3 overstated their
importance — they live in `Faithful/Extract.lean` only, are 3 lines each,
and are clearly labeled. They are not the proof layer's headline results.

### Real theorems actually proved (not tautologies)

The proof layer's substantive content (sorry-free, non-tautological):

- `alloc_preserves_capacity` (`Basic.lean:134`) — pure arithmetic, `omega`.
- `pmt_soundness` (`Soundness.lean:245`) — by induction; well-typed programs
  either succeed with `final_used ≤ capacity` or trap with exit code
  `1`/`134`/`135`. ~280 lines of proof.
- `pmt_soundness_correct` (`Soundness.lean:573`) — determinism + bounded
  `final_used ≤ initial_used + Σ layout_sizes`. Real proof (despite the
  stale docstring at lines 541–546 that claims `sorry`).
- `no_oob_trap_for_well_typed_strong` (`WellTypedStrong.lean:480`) —
  `WellTypedStrong` programs never trap with exit 134. ~230 lines of
  inductive proof, factored through `no_oob_trap_aux`.
- `no_uaf_trap_for_well_typed_strong` (`WellTypedStrong.lean:714`) —
  same shape for exit 135.
- `bv_checked_add_overflow` (`BitVecArena.lean:163`) — BitVec 64 overflow
  equivalence to `usize::checked_add` returning `None`. Real `BitVec.toNat`
  reasoning + `omega`.
- `bv_alloc_traps_on_arithmetic_overflow` (`BitVecArena.lean:238`) —
  arithmetic-overflow path traps with `arena_overflow`.
- `bv_alloc_traps_on_capacity_overflow` (`BitVecArena.lean:260`) —
  capacity-overflow path traps with `arena_overflow`.
- `bitvec_arena_equiv_implies_wf_faithful_strong`
  (`BitVecArena.lean:393`) — `BitVecArena` ↔ `RawArena` state correspondence
  transfers well-formedness (closes PMT-1-F unification).
- `raw_alloc_on_fresh_arena` (`MmapArena.lean:98`) — composition:
  `raw_create` succeeds on small capacities AND `raw_alloc` then succeeds.
- `alloc_preserves_cap_bnd` (`Iris/CapBndInvariant.lean:238`) —
  Iris-style `[cap_bnd]` invariant preserved by `alloc`, with ghost state.
- `alloc_preserves_guard` (`Iris/GuardInvariant.lean:121`) — `[guard]`
  invariant preserved.
- `alloc_preserves_all_invariants` (`Iris/Composition.lean:114`) — the
  bundle `[cap_bnd] ∗ [live_mirror] ∗ [guard]` is preserved by `alloc`.
  (Note: the language-reference.md §10.1 doc calls this
  `alloc_preserves_pmt_invariants`; the actual theorem name is
  `alloc_preserves_all_invariants` — a minor doc/code naming mismatch.)
- `real_own_exclusive` (`Iris/CapBndInvariant.lean:418`) — `RealOwn`
  exclusivity derived from single-valued-ness of `GhostState.get`.
- `own_ex_exclusive_derived` (`Iris/CapBndInvariant.lean:443`) —
  Ex-RA exclusivity derived (not axiomatised) from the heap model.
- `HeapPointsTo.exclusive` (`Iris/HeapModel.lean:110`) — points-to
  exclusivity for the real heap model.
- `encode_liveness_inj` (`Iris/HeapModel.lean:128`) — `Liveness` encodes
  injectively into `Val`.
- `no_uaf_ptr`, `no_uaf_alias` (`Faithful/UafProof.lean:43, 63`) — UAF
  safety on a small environment model.
- `alloc_overflow_returns_none`, `alloc_oob_returns_none`,
  `alloc_success_inBounds` (`Faithful/Model.lean:91, 107, 144`) —
  faithful `Arena.alloc` overflow/OOB/success theorems over `Fin (2^64)`.
- `extract_alloc` `native_decide` checks (`Faithful/Extract.lean:99–110`,
  already noted).
- `verify_information_flow_sound` (`IVE/Soundness/InformationFlow.lean`)
  — real contradiction proof (if any event's `check_flow_kind` returns
  false, a violation appears in the output, contradicting `hverify`).

### Tautologies that A-3 flagged

A-3 correctly identified four tautology-shaped theorems:

- `verify_session_types_sound` (`SessionType.lean:140–144`): hypothesis
  IS the conclusion. **Verified**. The docstring (lines 137–139) at
  least honestly restates the hypothesis as the conclusion.
- `verify_session_types_no_send_unopened` (`SessionType.lean:148–152`):
  same shape, `exact hverify`. **Verified**.
- `l1l3_collapse_sound` (`L1L3Collapse.lean:150–154`): hypothesis IS the
  conclusion. **Verified**. The docstring (lines 142–149) is more honest
  than the SessionType one — it explicitly admits: *"The full
  type-consistency theorem (every event has a valid type AND all events
  on the same channel agree on the type) requires inductive reasoning
  about the recursive `process` function; this is the soundness contract
  that downstream consumers use."* So the file acknowledges the gap.
- `single_step_exists_tautology`, `simulation_full_tautology`
  (`Faithful/SimSound2.lean`): honestly renamed; the docstring admits
  they are classical tautologies. **Verified per A-3**.

A-3's characterization was correct, but the scope is narrower than the
"purely memory-safety, no arithmetic verification, the only lemma is
`alloc_preserves_capacity`" summary implied. The actual proof layer has
~280 theorems, of which 4 are tautologies (1.4%) and the rest are real.

### The Iris layer is genuine-but-simplified

The `proof/PMT/Iris/` tree is not toy — `HeapModel.lean` provides a real
heap model (`Heap := Nat → Option Val`, `Heap.read`, `Heap.write`,
`HeapPointsTo`, `Heap.disjoint`, `Heap.merge`), and `SepGenuine.lean`
defines a genuine heap-indexed separating conjunction
`Sep (P Q : Heap → Prop) (h : Heap) : Prop := ∃ h1 h2, P h1 ∧ Q h2 ∧
h1.disjoint h2 ∧ h1.merge h2 = h`. The composition theorem
`alloc_preserves_all_invariants` threads this through the three named
invariants.

However, the Iris layer is **honestly self-described as "simplified"** —
the file docstring at `Composition.lean:27–40` admits:
> "Real Iris would still need the heap model to show that the physical
> `(liveness_byte v) ↦{1} encode(b)` points-to is preserved when `used`
> is bumped (because `alloc` doesn't touch the liveness byte region) —
> that obligation is implicit in this simplified encoding, exactly as
> the disjointness obligation of `Sep` is implicit in
> `CapBndInvariant.lean` §2."

So the Iris layer captures the algebraic structure of separation logic
(commutativity, associativity, frame rule, named invariants, ghost
state) but does NOT prove the physical-points-to preservation step. The
language-reference.md §10.1 claim of an "Iris separation logic layer" is
technically true at the algebraic level but overstates the heap-model
depth.

---

## Verdicts

### V-14 (f32 PMT proof greenfield)

- **Prior A-3 claim**: greenfield, 3–6 months, defer to v2. The Lean
  proof layer is "purely memory-safety, no arithmetic verification, the
  only lemma is `alloc_preserves_capacity` discharged by `omega`."
- **Reality**: The proof layer is much richer than A-3's summary. It
  contains 82 Lean files, 280 theorems (zero actual `sorry`s), an
  Iris-style separation-logic bundle (`[cap_bnd] ∗ [live_mirror] ∗
  [guard]` with `alloc_preserves_all_invariants`), a `BitVecArena`
  model that captures `usize` overflow semantics faithful to Rust's
  `checked_add`, an `MmapArena` allocator-failure model, a `PipelineSim`
  module (see below for honest limitations), and a `WellTypedStrong`
  predicate with two non-trivial non-trap theorems (`no_oob_trap…`,
  `no_uaf_trap…`). A-3 read `Basic.lean` and `PmtInstr.lean` only;
  A-3 missed `BitVecArena.lean`, `MmapArena.lean`, `PipelineSim.lean`,
  `WellTypedStrong.lean`, the entire `Iris/` subtree, and the entire
  `Faithful/` subtree.
- **On f32 specifically**: A-3's V-14 claim that no f32/IEEE-754/NaN/ULP
  proof exists is **correct**. `rg 'Float|IEEE|f32|f64|verified_float|nan|NaN|ulp'`
  over `proof/` returns only bare tag constructors `f32`/`f64` in
  `PmtInstr.lean:186–187` and a `CastKind` enum mentioning `intToFloat`
  / `uIntToFloat` / `floatToFloat` at lines 270–274 — there is no
  `FloatArena`, no `verified_float_add`, no IEEE-754 bit-pattern model.
  The Lean proof layer genuinely has no floating-point content.
- **Revised effort estimate**: The "greenfield 3–6 months" estimate was
  based on the (incorrect) premise that the existing proof layer is just
  `alloc_preserves_capacity`. With the actual layer in view, the
  foundation to extend is substantial — the `BitVecArena` model already
  demonstrates the methodology for "build a more-faithful Arena variant
  and prove unification with `RawArena`" (see
  `bitvec_arena_equiv_implies_wf_faithful_strong`). An `f32` extension
  would likely take the form of an `FloatArena` model that adds IEEE-754
  bit-patterns to `BitVecArena`'s `BitVec 64` infrastructure, plus
  rounding-mode / NaN-boxing lemmas. That's still substantial work —
  IEEE-754 reasoning is genuinely hard, Flocq is the standard library,
  and VUMA's small-deps policy probably rules it out — but the
  "greenfield" framing is wrong. A more honest estimate is:
  - **2–4 weeks** to model f32 bit-patterns on top of `BitVec 64` and
    prove `f32_add_no_overflow` / `f32_add_rounding_correct` for the
    round-to-nearest-even case.
  - **2–3 months** for full IEEE-754 conformance (all rounding modes,
    NaN propagation, denormals).
  - **6+ months** for monotonicity / distributivity / associativity
    lemmas (which are mostly FALSE for IEEE-754 anyway — the
    "correctness" property has to be carefully scoped).
  So 3–6 months for a useful subset is plausible, but it's not
  "greenfield" — it's "extend `BitVecArena` along the same methodology
  pattern that already exists."

### V-16 (capability signatures + `verify_capability` never called)

- **Prior A-3 claim**: FNV-1a × 4 signatures, `verify_capability`
  never called from emitted binaries, "security theater," the entire
  capability model is a one-way write-only ledger.
- **Reality on FNV-1a × 4**: **Verified, correct.** `ipc.rs:996–1007`
  (per A-3) implements exactly the FNV-1a × 4 construction. The
  module-level SECURITY NOTE at `capability.rs:31–54` is honest about
  this being a non-cryptographic checksum.
- **Reality on `verify_capability` never called**: **Verified,
  correct.** `capability.rs:49–54` discloses that `verify_capability`
  and `verify_delegation_chain` are never called from emitted VUMA
  binaries; `channel_recv` codegen rejects any frame with `cap_count > 0`
  (returns `-4` PERMISSION_DENIED). I re-verified by grepping
  `src/ive/` for `verify_capability|verify_delegation` — zero matches.
  The IVE does not invoke them either.
- **Reality on the "compile-time verification" hypothesis (the audit
  prompt's revised question)**: The language-reference.md §6 statement
  that `capability_grant` / `capability_delegate` "expand to IR
  sequences in `ipc_lowering.rs`, shared by all 19 backends" is
  **technically true but misleading**. The actual expansion at
  `ipc_lowering.rs:4998–5073` (`expand_capability_grant` /
  `expand_capability_delegate`) does the following:
  1. Mints the token AT COMPILE TIME inside the compiler process, using
     the hardcoded `b"vuma_dev_signing_key"` literal
     (`ipc_lowering.rs:5031` and `capability.rs:117`).
  2. Extracts ONLY the low 64 bits of the token's u128 id as an
     `i64` immediate.
  3. Emits the IR sequence `BinOp(Add, dst, Immediate(cap_id), Immediate(0))`
     — i.e. `dst = cap_id + 0`, which lowers to "load this constant
     into a register."
  The signature, the full token, the signing key, and the delegation
  chain are **all compile-time-only artifacts that do not survive into
  the emitted binary**. At runtime the binary has only the `cap_id`
  immediate, which is indistinguishable from any other 64-bit constant.
- **Compile-time capability verification in the IVE?** The IVE's
  `l1l3_collapse` function (`verification.rs:2268`) walks SCG
  Computation nodes tagged `ComputationKind::Intrinsic` and folds 1
  "L2 capability check" per node. But line 2379 reads
  `let known = true; // All Intrinsic variants are known capability ops`
  — a constant `true` (A-3's V-A3-6 finding). The code does NOT verify
  any signature, delegation depth, or StarkProof attestation. It just
  counts Intrinsic-tagged nodes. So there is **no compile-time
  capability verification either** — only compile-time token MINTING.
- **Revised severity**: A-3's "security theater" framing is essentially
  correct. The capability model is:
  - Tokens are minted at compile time with a public, hardcoded signing
    key (`b"vuma_dev_signing_key"`).
  - The signing key is mixed via FNV-1a (non-cryptographic).
  - Only the cap_id (low 64 bits) survives into the binary.
  - At runtime, `channel_recv` rejects any frame with `cap_count > 0`.
  - At compile time, the IVE counts Intrinsic capability ops but
    verifies no signatures.
  - `verify_capability` exists as a library function, is exported
    through `crate::capability::verify_capability`, but is invoked only
    by `capability.rs:199` (a unit test).
  The system is structurally incapable of enforcing capability
  delegation at runtime. The "correct design" the audit prompt asked
  about — verify at compile time, emit runtime checks only for dynamic
  delegations — is **not** what VUMA does. VUMA's design is "mint at
  compile time, never verify anywhere." This is a real P0 security gap
  (per A-3's V-A3-2 finding on the hardcoded signing key + PIDs).

### Three Lean Arena models disagree

- **Prior A-3 claim**: 3 fields (`Basic.lean`), 4 fields
  (`Faithful/Model.lean`), 5 fields (`arena.rs`) — unsound divergence.
- **Reality**: A-3 **undercounted the Lean Arena models by missing
  `RawArena.lean` and `BitVecArena.lean` entirely**. There are actually
  FOUR Lean Arena models, not three:

| Location | Arena fields | Field count |
|---|---|---:|
| `proof/PMT/Basic.lean:38–42` | `base : Nat, capacity : Nat, used : Nat` | 3 |
| `proof/PMT/Faithful/Model.lean:49–53` | `base : Ptr, capacity : USize, used : USize, alloc_id : Nat` | 4 |
| `proof/PMT/RawArena.lean:187–194` | `base : Ptr, offset : Nat, capacity : Nat, layout : AllocLayout, phase : ArenaPhase, created_thread : ThreadId` | 6 |
| `proof/PMT/BitVecArena.lean:123–128` | `base : USize64, offset : Offset64, capacity : Nat, layout : BvLayout` | 4 |
| `src/codegen/src/runtime/arena.rs:68–82` (Rust) | `base: *mut u8, offset: usize, capacity: usize, layout: Layout, created_thread: ThreadId` | 5 |

- **A-3's "not modeled anywhere in Lean" claim is FALSE for `layout`
  and `created_thread`**: `RawArena.lean` explicitly models BOTH. The
  docstring at `RawArena.lean:165–194` maps each field 1:1 to the Rust
  runtime Arena (`base ↔ base: *mut u8`, `offset ↔ offset: usize`,
  `capacity ↔ capacity: usize`, `layout ↔ layout: Layout`,
  `created_thread ↔ created_thread: ThreadId`). The `phase` field is
  an explicit modeling of Rust's implicit lifecycle (alive between
  `create` and `destroy`).
- **The "disagreement" is intentional abstraction, not a bug**:
  - `Basic.lean` is the toy model: 3 fields, `Nat`-everywhere, used
    only by `pmt_soundness` and `alloc_preserves_capacity`. The file's
    own docstring (line 35) calls it "§1.1 Arena = (base, capacity,
    used)."
  - `Faithful/Model.lean` is the simulation model: 4 fields, `USize =
    Fin (2^64)`, used by `Arena.alloc` overflow proofs.
  - `RawArena.lean` is the primary faithful model: 6 fields, used by
    `SimRel.lean` (the Lean↔Rust simulation relation).
  - `BitVecArena.lean` is the overflow-specialized model: 4 fields,
    `BitVec 64` for `offset`, used by `bv_alloc_traps_on_*` theorems.
  The unification theorems `bitvec_arena_equiv_raw_arena` and
  `bitvec_arena_equiv_implies_wf_faithful_strong`
  (`BitVecArena.lean:366, 393`) explicitly prove that the BitVecArena
  and RawArena models describe the same well-formed Rust Arena states.
- **Revised verdict**: The four Lean Arena models are a layered
  abstraction (toy → simulation → faithful → overflow-specialized),
  each with a documented purpose, and the unification theorems
  explicitly bridge them. The Rust runtime's 5 fields are all modeled
  in `RawArena.lean`. A-3's "three models disagree, layout and
  created_thread not modeled anywhere in Lean" claim is **false** — it
  reflects an incomplete read of the `proof/PMT/` tree (A-3 missed
  `RawArena.lean` and `BitVecArena.lean`).

### V-A3-3 (discharge_rate denominator)

- **Prior A-3 claim**: `discharge_rate` excludes `failed` from the
  denominator; `unwrap_or(100)` returns 100% on all-failed; the
  architecture spec says it should be `total / total`. P1 bug.
- **Reality**: **Verified, correct.** `compile_dump.rs:233–235`:
  ```rust
  (100 * result.summary.passed)
      .checked_div(result.summary.passed + result.summary.unverified)
      .unwrap_or(100)
  ```
  The denominator is `passed + unverified`, NOT `total_checked`
  (= `passed + failed + unverified`). The `unwrap_or(100)` returns
  `100` when `passed + unverified == 0` (all-failed case).
- **Docstring / spec verification**: `docs/architecture.md:76` says:
  > "The `discharge_rate` is the fraction of proof obligations that
  > the IVE discharged (via Z3 or trivial-true elision) over the
  > **total obligations** collected from the program."

  The spec REQUIRES `total_checked` (total obligations) as the
  denominator. The implementation diverges.
- **The audit prompt's "correct-by-design" hypothesis is FALSE**: the
  prompt speculated that maybe `failed` is intentionally excluded
  because it represents "Z3 couldn't decide (unknown)" rather than
  "Z3 disproved (false)." Reading `invariant_aggregator.rs:262–277`:
  ```rust
  pub fn is_pass(&self) -> bool {
      matches!(self.result.status,
          VerificationStatus::Proven | VerificationStatus::ProbablySafe { .. })
  }
  pub fn is_fail(&self) -> bool { self.result.is_violated() }
  pub fn is_unverified(&self) -> bool {
      matches!(self.result.status, VerificationStatus::Unverified { .. })
  }
  ```
  So `failed` = `VerificationStatus::Violated` (Z3 disproved via
  counterexample — the contract is FALSE), and `unverified` =
  `VerificationStatus::Unverified` (Z3 returned Unknown). They are
  distinct categories. `failed` is genuine proof failure, not
  "couldn't decide." Excluding `failed` from the denominator is a
  bug, not a design choice. The existing `pass_rate()` method on
  `VerificationSummary` (`invariant_aggregator.rs:396–402`) uses
  `passed / total_checked` — the correct denominator — but
  `compile_dump.rs` does NOT call `pass_rate()`; it recomputes with
  the buggy denominator.
- **Mitigation in practice**: `compile_dump.rs:240` hard-fails the
  build when `result.overall == OverallVerdict::Fail` (i.e. when
  `failed > 0`), so the misleading number only escapes to the user on
  the `Inconclusive` path (`failed == 0`, `unverified > 0`). The
  wave3 audit reports 100.00% because the gold-standard suite has
  zero failures (where the bug doesn't trigger).
- **Revised**: A-3's V-A3-3 verdict is correct. ADR-0008 (dated
  2026-08-01, status Accepted) already prescribes the exact fix:
  replace line 234 with
  `(100 * result.summary.passed).checked_div(result.summary.total_checked).unwrap_or(0)`.
  The "correct-by-design" alternative hypothesis is refuted by the
  `is_fail` definition (`Violated`, not `Unknown`).

---

## What A-3 got RIGHT

1. **V-14's narrow claim about f32**: No f32/IEEE-754/NaN/ULP proof
   exists. **Correct**. `PmtInstr.lean:186–187` has `f32`/`f64` as bare
   tag constructors; `CastKind` has `intToFloat`/`uIntToFloat`/
   `floatToFloat` at lines 270–274; no `FloatArena` or
   `verified_float_add` exists. The narrow technical claim is accurate.
2. **V-16 FNV-1a × 4 signature**: Verified at `ipc.rs:996–1007`.
3. **V-16 `verify_capability` never called from emitted binaries**:
   Verified. Re-confirmed by grepping `src/ive/` for capability
   verification (zero matches).
4. **V-A3-2 hardcoded signing key + PIDs**: `capability.rs:117–137`
   hardcodes `b"vuma_dev_signing_key"` and `source_pid: 1, target_pid: 2`.
   Real bug.
5. **V-A3-3 discharge_rate denominator**: Real bug, already covered by
   accepted ADR-0008.
6. **V-A3-6 dead `if !known` branch in `l1l3_collapse`**: Verified at
   `verification.rs:2379` (`let known = true;`). Dead code.
7. **V-A3-8 information-flow IR wrapper misses BinOp/Branch events**:
   Not re-verified in this audit (out of scope), but A-3's evidence was
   concrete file:line citations.
8. **The `native_decide` shortcut in `Faithful/Extract.lean`**: Real,
   honestly labeled, three theorems only.
9. **The four tautology-shaped theorems** (`verify_session_types_sound`,
   `verify_session_types_no_send_unopened`, `l1l3_collapse_sound`,
   `single_step_exists_tautology`, `simulation_full_tautology`):
   Verified — these are tautologies. A-3's framing was correct that
   they prove nothing substantive (though the docstrings are honest
   about this, especially `L1L3Collapse.lean`'s).

---

## What A-3 got WRONG or OVERSTATED

1. **The Lean proof layer is "purely memory-safety, no arithmetic
   verification, the only lemma is `alloc_preserves_capacity`
   discharged by `omega`."** — **WRONG**. The proof layer has 82 Lean
   files and 280 theorems. Substantive non-`omega` content includes:
   `pmt_soundness` (~280 lines of induction), `no_oob_trap_for_well_typed_strong`
   (~230 lines), `no_uaf_trap_for_well_typed_strong`, `bv_checked_add_overflow`
   (BitVec arithmetic), `bv_alloc_traps_on_*` (overflow-path trap lemmas),
   `bitvec_arena_equiv_implies_wf_faithful_strong` (model unification),
   the entire Iris separation-logic layer, the entire Faithful/
   Lean↔Rust simulation tree (`SimSound.simulation`, `UafProof`,
   `OverflowProof`), `raw_alloc_on_fresh_arena` (composition). A-3
   read `Basic.lean` and `PmtInstr.lean` only and generalized from
   those two files.

2. **The "three Lean Arena models disagree (3/4/5 fields), unsound"
   claim** — **WRONG**. A-3 missed `RawArena.lean` (6 fields, the
   PRIMARY faithful model used by `SimRel`) and `BitVecArena.lean`
   (4 fields, the overflow-specialized model). The four Lean models
   are a documented layered abstraction, and `RawArena.lean`
   explicitly models BOTH `layout` and `created_thread` (the fields
   A-3 claimed were "not modeled anywhere in Lean"). Unification
   theorems (`bitvec_arena_equiv_implies_wf_faithful_strong`) bridge
   the models.

3. **"`layout` and `created_thread`: Not modeled anywhere in Lean"**
   — **WRONG**. `RawArena.lean:187–194` models both fields. The
   docstring at lines 165–194 maps them 1:1 to the Rust runtime
   `Arena` struct.

4. **The Iris layer is missed entirely**. A-3's report does not mention
   `proof/PMT/Iris/` at all. The Iris layer has 9 files including a
   real heap model (`HeapModel.lean`), a genuine heap-indexed
   separating conjunction (`SepGenuine.lean`), and the composition
   theorem `alloc_preserves_all_invariants` over the bundle
   `[cap_bnd] ∗ [live_mirror] ∗ [guard]`. A-3's summary omitted this
   entire subtree.

5. **The `Faithful/` subtree is missed entirely**. A-3 mentions
   `Pmt.SimSound.simulation` in passing (line 183 of A-3's report) but
   does not inventory the 22 files under `Faithful/`. This subtree
   contains the Lean↔Rust simulation relation (`Simulation.lean`,
   `Simulation2.lean`, `SimIpc.lean`, `SimWrite.lean`,
   `SimTransform.lean`, `SimSound.lean`, `SimSound2.lean`), the
   from-scratch separation-logic / CMRA / WP stack (`Sep.lean`,
   `CMRA.lean`, `WP.lean`, `FancyUpdate.lean`, `WPSafety.lean`), the
   overflow/UAF safety proofs (`OverflowProof.lean`, `UafProof.lean`),
   and the arena invariants (`ArenaInv.lean`, `GuardInv.lean`).

6. **The `WellTypedStrong.lean` file is missed entirely**. A-3's
   report does not mention `no_oob_trap_for_well_typed_strong` or
   `no_uaf_trap_for_well_typed_strong` — both real, sorry-free,
   non-trivial inductive theorems. A-3's summary claimed "no lemma
   about value contents" but the `WellTypedStrong` predicate adds
   `FieldAccessOk` (field-access safety) and `DataflowOk` (dataflow
   correctness) on top of `WellTyped`, and proves non-trapping for
   both OOB and UAF exits.

7. **The `BitVecArena.lean` file is missed entirely**. This is the
   file that addresses A-3's own critique — "the existing `RawArena`
   uses `Nat` for `offset` and `capacity`, which is structurally
   unbounded — so `offset + aligned_size` can never wrap." A-3's
   report acknowledges this as a gap; in fact `BitVecArena.lean`
   already closes it (with `BitVec 64` overflow semantics, theorems
   `bv_checked_add_overflow`, `bv_alloc_traps_on_arithmetic_overflow`,
   `bv_alloc_traps_on_capacity_overflow`).

8. **The `MmapArena.lean` allocator-failure model is missed entirely**.
   A-3's report does not mention it. The file models `Layout::from_size_align`
   failure (the `MAP_FAILED` path) and proves `raw_alloc_on_fresh_arena`.

9. **The `PipelineSim.lean` file is missed, and its honest
   self-assessment is missed**. The file's own docstring (lines 9–73)
   explicitly documents that the original theorems
   `pipeline_compile_sound` / `pipeline_compile_no_oob` were renamed
   to `pmt_soundness_restate` / `pmt_soundness_no_oob_restate` because
   they were honestly admitted to be restatements of `pmt_soundness`
   with no Rust-side hypothesis. The "real" pipeline-conformance
   theorem (discharging a non-vacuous `PipelineSpec` against the Rust
   `pipeline::compile` output) is deferred to "Wave 1 PMT-1-G." The
   language-reference.md §10.1 claim that `PipelineSim.lean` "relates
   Lean `Program` to the lowered Rust SCG IR (SimRel)" overstates
   what's there — the file is honest about its own limitations, the
   doc is not.

10. **The "20+ modules, ~90 theorems, 2 sorries" doc claim is
    understated on modules and theorems and stale on sorries**, but
    A-3 didn't catch the doc-vs-source drift either. Actual: 82
    modules, 280 theorems, 0 sorries.

---

## What VUMA gets RIGHT that A-3 framed as a bug

1. **The Iris layer's "simplified encoding"**. The Iris layer captures
   the algebraic structure of separation logic (commutativity,
   associativity, frame rule, named invariants, ghost state, Ex/Ag
   resource algebras) and provides a real heap model in `HeapModel.lean`.
   The `Composition.lean` docstring is HONEST that the physical
   points-to preservation step is implicit (not proved). A-3 didn't
   find this layer at all, so didn't frame it as either bug or
   feature — but the design choice (algebraic structure now, heap-model
   completeness later) is defensible.

2. **The four-Arena layered abstraction**. Having `Basic.lean` (toy
   for soundness proofs), `Faithful/Model.lean` (small faithful
   mirror), `RawArena.lean` (primary faithful model with full Rust
   field correspondence), and `BitVecArena.lean` (overflow-specialized)
   is a legitimate separation of concerns. Each model is documented
   and the unification theorems bridge them. A-3 framed the 3-vs-4
   field difference as "unsound divergence" — it is in fact documented
   abstraction with explicit unification lemmas.

3. **The `PipelineSim.lean` honesty**. The file's own docstring
   explicitly renames `pipeline_compile_sound` → `pmt_soundness_restate`
   to avoid overstating what's proved. This is more honest than most
   proof libraries. The corresponding `language-reference.md` §10.1
   claim still uses the old "PipelineSim Lean↔Rust simulation
   refinement" wording — that's a doc bug, not a proof bug. The proof
   file itself is honest.

4. **The `BitVecArena.lean` model**. This file directly addresses the
   "Nat cannot overflow" critique A-3 leveled at `RawArena`.lean. The
   file models `BitVec 64` overflow semantics faithful to Rust's
   `usize::checked_add`, proves both overflow paths trap, and unifies
   with `RawArena`. A-3's "Lean model has no dealloc reasoning /
   `layout` field not modeled" claims are answered by `RawArena.lean`
   (which has `layout`, `phase`, `created_thread`) and by
   `BitVecArena.lean`'s overflow model. (Dealloc reasoning itself
   remains partial — `phase = .destroyed` is modeled, but no
   `dealloc_preserves_*` lemma exists; this is a real gap.)

5. **The `l1l3_collapse` docstring honesty on the `known = true`
   constant**. The `l1l3_collapse` function at `verification.rs:2268`
   has a verbose docstring that describes the INTENDED verification
   (label check, type_hash match, capability attestation). The actual
   code at line 2379 reads `let known = true;` and folds 1 L2 check
   per Intrinsic. A-3's V-A3-6 calls this "dead code" — technically
   true (the `if !known` branch is unreachable), but the docstring's
   intent is clearly a placeholder for future per-label verification.
   The dead branch is harmless; the missing verification is the real
   gap (and is documented in the file's docstring as "Limitations").

6. **The `capability.rs` SECURITY NOTE**. The `capability.rs:31–54`
   docstring is unusually honest about the FNV-1a × 4 construction,
   the hardcoded signing key, and the fact that `verify_capability`
   is never called from emitted binaries. A-3 frames this as
   "security theater," but the honesty of the disclosure is actually
   a positive — most projects would not document their own crypto
   weaknesses this explicitly. The weakness is real (P0 per V-A3-2);
   the disclosure is good engineering practice.

---

## Cross-cutting summary

| Claim | A-3 verdict | F-2 verdict | Evidence |
|---|---|---|---|
| V-14 f32 PMT proof greenfield | VERIFIED (3–6 months) | Narrowly TRUE on f32; premise (Lean layer = `alloc_preserves_capacity` only) FALSE | `rg Float|IEEE|f32` → only tag constructors; 82 modules / 280 theorems / 0 sorries actually exist |
| V-16 FNV-1a × 4 | VERIFIED | VERIFIED | `ipc.rs:996–1007` |
| V-16 `verify_capability` never called | VERIFIED | VERIFIED + amplified: no compile-time verification either (just compile-time minting) | `capability.rs:49–54`; `ipc_lowering.rs:4998–5073`; `verification.rs:2379` |
| Three Arena models disagree (3/4/5) | VERIFIED, unsound | WRONG: 4 Lean models (3/4/6/4 fields); unification theorems bridge them; `layout` + `created_thread` ARE modeled in `RawArena.lean` | `Basic.lean:38–42`; `Faithful/Model.lean:49–53`; `RawArena.lean:187–194`; `BitVecArena.lean:123–128`; `arena.rs:68–82`; `BitVecArena.lean:366, 393` |
| V-A3-3 discharge_rate denominator | VERIFIED, P1 | VERIFIED, P1; "correct-by-design" hypothesis refuted (`is_fail` = `Violated`, not `Unknown`) | `compile_dump.rs:233–235`; `invariant_aggregator.rs:262–277, 396–402`; `architecture.md:76`; ADR-0008 Accepted |
| Lean proof layer is "purely memory-safety, only lemma is `alloc_preserves_capacity`" | (A-3 summary) | WRONG: 280 theorems, Iris layer, BitVecArena overflow model, WellTypedStrong non-trapping, UafProof, OverflowProof, PipelineSim (honestly limited) | Direct read of 14+ proof files |
| `sorry` count = 2 (per doc) | (A-3 didn't challenge) | WRONG: 0 actual `sorry` tokens in source; doc is stale | `rg --pcre2 '(?<![\w/-])sorry(?![\w/-])' proof/` → 0 matches |
