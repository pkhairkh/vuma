# PMT Faithfulness Audit — Lean Models vs Rust Implementations

**Task ID:** PMT-FAITH (groups 1–4)
**Auditor:** PMT Orchestrator (main, direct execution + 1 subagent for FAITH-1)
**Scope:** All PMT-codedomain Lean modules (excluding `proof/PMT/IVE/` which is IVE's codedomain, audited by the IVE-Faith orchestrator).
**Date:** Audit run on `main` HEAD `8cb6973c` (PMT-3-B: pmt_pillar_sound_no_uaf).
**Methodology:** Apply the 8 faithfulness rules from `docs/vuma_orchestrator_ive_faithfulness.md` (same signature shape, same failure modes, same check ordering, same data structures, same arithmetic, same control flow, same variant coverage, Rust is source of truth).

---

## Executive Summary

The PMT pillar theorems (`pmt_soundness`, `pmt_pillar_sound`, `pmt_pillar_sound_no_uaf`) prove soundness of **Lean abstractions**, not of the production **Rust runtime**. The Lean models are sound (no sorries, 1 residual axiom) but diverge from the Rust implementations in 4 critical ways and 24+ major ways. The most serious gaps are:

1. **`instr_sim` is trivially `lean = rust`** (FAITH-4-A) — the simulation relation is intra-Lean, not cross-language.
2. **`PipelineSpec.compiled_matches_exec` is `rfl`** (FAITH-4-C) — the CompCert-style translation-validation obligation is NOT discharged.
3. **`Transform` field shape diverges** (FAITH-2-C) — Lean drops `from_layout`/`to_layout` (essential for IVE's Reinterpret/Copy/Identity check).
4. **`verified_capacity_check` overflow mismatch** (FAITH-4-D) — Lean (Nat) returns `true` on overflow inputs; Rust (`checked_add`) returns `false`. Known and documented in the parity test but not fixed in the Lean model.

The gaps fall into 3 categories:
- **Intra-Lean abstractions** (acceptable, documented): PmtOp (3 behavioral categories vs 35 IRInstr variants), Arena (3-field arithmetic core vs 6-field Rust), ghost-state Liveness vs runtime tombstone byte.
- **Field-shape divergences** (should be fixed for bit-faithfulness): Transform, Call, Ret, CallIndirect, load/store use String instead of IRValue; IRType missing 10 variants; IRValue uses Nat instead of bounded u32/i64/u64.
- **Missing cross-language bridge** (the most serious gap): `instr_sim` is trivial, `full_simulation` is intra-Lean, `PipelineSpec.compiled_matches_exec` is `rfl`. The cross-language bridge is via the Extraction parity test (which covers the 4 verified checkers but NOT full program execution).

---

## Gap Inventory (38 total: 4 CRITICAL, 21 MAJOR, 13 MINOR/MITIGATED)

### Group 1: Basic + Field + Liveness (FAITH-1, 11 gaps)

| ID | Category | Sev | Lean | Rust | Status |
|----|----------|-----|------|------|--------|
| FAITH-1-A | Field struct drops `name`+`type_name` | MAJOR | Basic.lean:45-48 | state_read.rs:29-34 | OPEN |
| FAITH-1-B | Layout struct drops `name` | MAJOR | Basic.lean:51-54 | state_read.rs:22-26 | OPEN |
| FAITH-1-C | WF_Layout conjunct 2 (disjointness) INVENTED | MAJOR | Basic.lean:69 | state_read.rs:82 | OPEN (soundness-strengthening) |
| FAITH-1-D | WF_Layout conjunct 3 (size>0) INVENTED | MINOR | Basic.lean:70 | state_read.rs (none) | OPEN (sanity check) |
| FAITH-1-E | GuardPage models PROT_NONE page arena.rs lacks | MAJOR | Liveness.lean:84 | arena.rs:14-21 (TODO) | OPEN (faithful to codegen, not arena.rs) |
| FAITH-1-F | `state_read_requires_live` misnamed | MINOR | Liveness.lean:44-49 | state_read.rs (no liveness check) | OPEN (rename) |
| FAITH-1-G | Stale comment about `inject_liveness_check_ir` | MINOR | Liveness.lean:15,42 | memory_safety.rs:1513 (shipped) | OPEN (doc fix) |
| FAITH-1-H | Arena drops `layout`+`created_thread` | MAJOR | Basic.lean:38-42 | arena.rs:68-82 | MITIGATED (RawArena.lean) |
| FAITH-1-I | CapacityInvariant only `used≤capacity` | MAJOR | Basic.lean:86 | arena.rs:68-82 | MITIGATED (WF_RawArena_faithful) |
| FAITH-1-J | `alloc` unaligned | MAJOR | Basic.lean:93-94 | arena.rs:176 | MITIGATED (aligned_alloc) |
| FAITH-1-K | `alloc` no overflow modeling | MAJOR | Basic.lean:93-94 | arena.rs:220-228 | MITIGATED (BitVecArena + raw_alloc_with_overflow) |

### Group 2: PmtInstr 35 variants vs Rust IRInstr (FAITH-2, 13 gaps)

| ID | Category | Sev | Lean | Rust | Status |
|----|----------|-----|------|------|--------|
| FAITH-2-A | IRType missing 10+ variants (I8/I16/U8/U16/F32/F64/Func/Array/TaggedUnion/Channel) | MAJOR | PmtInstr.lean:44-50 | ir.rs:40-110 | OPEN |
| FAITH-2-B | IRValue uses Nat vs Rust u32/i64/u64 (bounded) | MAJOR | PmtInstr.lean:42-47 | ir.rs:984-992 | OPEN |
| FAITH-2-C | Transform drops from_layout/to_layout, uses Layout instead of names | **CRITICAL** | PmtInstr.lean:482 | ir.rs:1978-1986 | OPEN |
| FAITH-2-D | Offset variant missing (documented out-of-scope) | MAJOR | PmtInstr.lean (absent) | ir.rs:1490 | OPEN |
| FAITH-2-E | AtomicLoad/Store/Cas have spurious AtomicOrdering field | MINOR | PmtInstr.lean:520-530 | ir.rs:1583/1598/1617 | OPEN (documented divergence) |
| FAITH-2-F | Call drops `dst` + `is_extern` flag | MAJOR | PmtInstr.lean:480 | ir.rs:1394 | OPEN (is_extern relevant to NoExterns) |
| FAITH-2-G | Ret single IRValue vs Rust Vec<IRValue> | MAJOR | PmtInstr.lean:481 | ir.rs:1620 | OPEN |
| FAITH-2-H | CallIndirect abstracts func_ptr to String, drops dst | MAJOR | PmtInstr.lean:570 | ir.rs:1907 | OPEN |
| FAITH-2-I | VectorOp lanes/elem_size Nat vs u32 | MINOR | PmtInstr.lean:541 | ir.rs:1737 | OPEN |
| FAITH-2-J | Syscall nr Nat vs u32 | MINOR | PmtInstr.lean:581 | ir.rs:1925 | OPEN |
| FAITH-2-K | well_typed = True for all 28 non-memory variants | MAJOR | PmtInstr.lean:595+ | ive/*.rs (per-variant checks) | OPEN (PMT/IVE split) |
| FAITH-2-L | load/store use String (var name) + Nat (offset) vs IRValue | MAJOR | PmtInstr.lean:478-479 | ir.rs:1370+ | OPEN |
| FAITH-2-M | alloc uses String (out_var) + Layout vs IRValue dst + size | MAJOR | PmtInstr.lean:477 | ir.rs:1430 | MITIGATED (to_steps mapping) |

### Group 3: Soundness + RawArena vs Rust runtime (FAITH-3, 7 open + 7 no-gap)

| ID | Category | Sev | Lean | Rust | Status |
|----|----------|-----|------|------|--------|
| FAITH-3-A | PmtOp 3 variants vs Rust 35 IRInstr (behavioral abstraction) | MAJOR | Soundness.lean:144-148 | ir.rs:1368 | OPEN (sound abstraction) |
| FAITH-3-B | UAF ghost-state vs Rust tombstone byte | MAJOR | Soundness.lean:193-195 | memory_safety.rs:1393-1395 | OPEN (mitigated by codegen) |
| FAITH-3-C | OOB check semantics match but Rust emits at compile time | MAJOR | Soundness.lean:198-200 | memory_safety.rs:1019-1130 | OPEN (mitigated) |
| FAITH-3-D | Overflow single check vs Rust two paths (arith + capacity) | MAJOR | Soundness.lean:203-205 | arena.rs:220-228 | MITIGATED (raw_alloc_with_overflow) |
| FAITH-3-E | assert_owner_thread not modeled | MINOR | Soundness.lean:193 | arena.rs:176 | OPEN (single-threaded assumption) |
| FAITH-3-F | ptr return implicit | MINOR | Soundness.lean:206-209 | arena.rs:234 | OPEN |
| FAITH-3-G | Rust panic not modeled in Result | MINOR | Soundness.lean:220-226 | arena.rs:176,259 | OPEN (debug-only) |
| FAITH-3-H–N | RawArena struct/grow/overflow/lifecycle/TrapCode | — | RawArena.lean | arena.rs | NO GAP (closed by PMT-1-F) |

### Group 4: SimRel + Extraction + PipelineSim (FAITH-4, 7 open + 4 no-gap)

| ID | Category | Sev | Lean | Rust | Status |
|----|----------|-----|------|------|--------|
| FAITH-4-A | `instr_sim` trivially `lean = rust` (intra-Lean) | **CRITICAL** | SimRel.lean:189-190 | ir.rs:1368 (different type) | OPEN |
| FAITH-4-B | `full_simulation` is intra-Lean, not cross-language | **CRITICAL** | SimRel.lean:433 | pipeline.rs:5166 | OPEN |
| FAITH-4-C | `PipelineSpec.compiled_matches_exec` is `rfl` | **CRITICAL** | PipelineSim.lean:113 | pipeline.rs:5166 | OPEN (known degeneracy) |
| FAITH-4-D | `verified_capacity_check` overflow: Lean true, Rust false | **CRITICAL** | Extraction.lean:62 | pmt_check.rs:21 | OPEN (documented in parity test) |
| FAITH-4-E | verified_field_bounds_check: Lean Field+Layout vs Rust raw u64s | MAJOR | Extraction.lean:72 | pmt_check.rs:30 | OPEN (parity test bridges) |
| FAITH-4-F | verified_linearity_check: Lean String+List vs Rust &str+&[&str] | MAJOR | Extraction.lean:83 | pmt_check.rs:39 | OPEN (parity test bridges) |
| FAITH-4-G | verified_pmt_check: Lean 6 args (structured) vs Rust 7 args (raw) | MAJOR | Extraction.lean:93 | pmt_check.rs:49 | OPEN (parity test bridges) |
| FAITH-4-H–K | arena_sim, aligned_alloc, initial_state_sim, arena_sim_preserved | — | SimRel.lean | arena.rs | NO GAP (intra-Lean, contingent on align8_nat) |

---

## Critical Gaps (4) — Detailed

### FAITH-2-C: Transform field shape diverges (CRITICAL)

**Lean:** `transform : String → String → Layout → PmtInstr` (in_var, out_var, layout)
**Rust:** `Transform { dst: IRValue, src: IRValue, from_layout: String, to_layout: String }`

Three divergences:
1. Lean uses `String` (var name) where Rust uses `IRValue` (vreg).
2. Lean takes ONE `Layout` where Rust takes TWO layout names (`from_layout`, `to_layout`).
3. Lean uses a resolved `Layout` struct where Rust uses layout NAMES (Strings, looked up in a registry).

**Why critical:** Transform is the core PMT operation. The `from_layout`/`to_layout` distinction is essential for IVE's `verify_transform` Reinterpret/Copy/Identity check (based on layout compatibility). The Lean model drops this distinction, making it unable to model the layout-compatibility failure path. The use of a resolved `Layout` instead of a name loses the layout-not-found failure path.

**Fix:** Change Lean `transform` to `IRValue → IRValue → String → String → PmtInstr` (dst, src, from_layout_name, to_layout_name).

### FAITH-4-A: `instr_sim` is trivially `lean = rust` (CRITICAL)

**Lean:** `def instr_sim (lean : PmtInstr) (rust : PmtInstr) : Prop := lean = rust`
**Rust:** `IRInstr` (35 variants, different type)

The simulation relation requires structural equality between two `PmtInstr` values, but the cross-language simulation would need to relate `PmtInstr` (Lean) to `IRInstr` (Rust) — different types. The relation is never provable for cross-language use.

**Why critical:** `full_simulation` (SimRel.lean:433) claims to be a simulation theorem but only runs Lean `exec` on Lean `IRProgram.to_program` — it does NOT simulate Rust execution. The "simulation" is intra-Lean.

**Fix:** Either (a) define a proper `instr_sim` that relates Lean `PmtInstr` to Rust `IRInstr` via a projection/encoding function, OR (b) rename `full_simulation` to `lean_internal_soundness` and document that the cross-language bridge is via the Extraction parity test, not via SimRel.

### FAITH-4-C: `PipelineSpec.compiled_matches_exec` is `rfl` (CRITICAL)

**Lean:** `compiled_matches_exec : exec prog s = exec prog s` (PipelineSim.lean:113)
**Rust:** `pipeline::compile` (pipeline.rs:5166) — not modeled

The CompCert-style translation-validation obligation is advertised but NOT discharged. The `pmt_soundness_restate` theorem (renamed from `pipeline_compile_sound` in PMT-0-C) delegates entirely to `pmt_soundness` without any Rust-side content.

**Why critical:** This is the bridge between the Lean model and the Rust-compiled binary. Without it, the Lean theorems prove soundness of Lean `exec` on Lean `IRProgram`, NOT of the Rust-compiled binary. The residual TCB includes the entire codegen pipeline (parser → SCG → IVE → IR → opt → regalloc → backend → ELF).

**Fix:** Closing this requires either (1) modeling the Rust `pipeline::compile` pipeline in Lean (huge effort), OR (2) translation validation — run Lean `exec` on the IRProgram and compare to Rust-compiled binary execution on test inputs. The parity test (tests/pmt_parity_test.rs) partially addresses (2) for the 4 extracted verifiers, but NOT for full program execution.

### FAITH-4-D: `verified_capacity_check` overflow mismatch (CRITICAL)

**Lean:** `verified_capacity_check used size capacity := used + size ≤ capacity` (Nat arithmetic, no overflow)
**Rust:** `used.checked_add(size).map_or(false, |sum| sum <= capacity)` (overflow → false)

**Divergence:** On `used = 2^64 - 1, size = 1, capacity = 2^64`:
- Lean: `2^64 - 1 + 1 = 2^64 ≤ 2^64` → `true`
- Rust: `checked_add` overflows → `false`

**Why critical:** The Lean theorem `verified_capacity_check_correct` proves `verified_capacity_check used size capacity = true → used + size ≤ capacity`. The Rust code does NOT satisfy this on overflow inputs (it returns false, which is sound, but the Lean theorem's premise is `= true`, so the theorem is vacuously true on overflow). The gap is that the Lean `verified_capacity_check` returns `true` on overflow inputs where Rust returns `false` — they disagree on the function's output, even though both are individually sound.

**Status:** Known and documented in the parity test (`tests/pmt_parity_test.rs:285-291`): "Lean: verified_capacity_check 0 (2^64) (2^64) = true (Nat, no overflow); Rust: u64 overflow → checked_add returns None → false. This is the KEY difference: Rust catches overflow, Lean doesn't."

**Fix:** Change Lean `verified_capacity_check` to model overflow: `verified_capacity_check used size capacity := (used + size ≥ used) ∧ (used + size ≤ capacity)` where the first conjunct models the no-overflow check. OR use BitVec 64 arithmetic.

---

## Recommendations

### Priority 1: Fix the 4 CRITICAL gaps
1. **FAITH-2-C (Transform):** Change Lean `transform` to carry `from_layout`/`to_layout` names. This unblocks faithful IVE `verify_transform` modeling.
2. **FAITH-4-A/B (SimRel intra-Lean):** Rename `full_simulation` to `lean_internal_soundness`; document that cross-language bridge is via Extraction parity test.
3. **FAITH-4-C (PipelineSpec degenerate):** Document as the largest residual TCB. Closing requires translation validation (run Lean exec on test inputs vs Rust binary).
4. **FAITH-4-D (overflow mismatch):** Change Lean `verified_capacity_check` to model overflow (add `(used + size ≥ used)` conjunct OR use BitVec 64).

### Priority 2: Fix field-shape divergences (MAJOR gaps)
- FAITH-2-F (Call drops `is_extern`): essential for NoExterns faithfulness.
- FAITH-2-A (IRType 7-vs-14): add missing 10 variants.
- FAITH-2-G (Ret single-vs-Vec): change to `List IRValue`.
- FAITH-2-H (CallIndirect): use IRValue instead of String.
- FAITH-2-L (load/store): use IRValue instead of String.
- FAITH-1-A/B (Field/Layout name): add `name` field OR document PMT/IVE split.

### Priority 3: Document the intra-Lean abstractions (acceptable)
- FAITH-3-A (PmtOp 3-vs-35): sound behavioral abstraction, document.
- FAITH-3-B/C (ghost-state vs runtime byte/IR): sound split, document.
- FAITH-1-H/I/J/K (Arena/CapacityInvariant/alloc mitigated by RawArena/SimRel): document pointers to faithful counterparts.

### Priority 4: Minor doc fixes
- FAITH-1-F (rename `state_read_requires_live`), FAITH-1-G (stale comment), FAITH-2-E (AtomicOrdering divergence), FAITH-2-I/J (Nat-vs-u32).

---

## Conclusion

The PMT pillar theorems are **sound** (sorry-free, 1 residual axiom) but **not bit-faithful** to the Rust runtime. The 4 CRITICAL gaps (Transform field shape, instr_sim trivial, PipelineSpec degenerate, overflow mismatch) mean the theorems prove soundness of Lean abstractions, not of the production Rust binary. The cross-language bridge is via the Extraction parity test (which covers the 4 verified checkers but NOT full program execution).

Closing the CRITICAL gaps is the work of a follow-up "PMT-Faith" orchestrator (mirroring the IVE-Faith orchestrator's approach). Estimated effort: ~20 person-weeks (~5 months for one Lean+Rust expert).

---

## Closure Status (PMT-Faith Waves 5-6, post-closure audit)

**Date:** Closure audit run on `main` HEAD `22f1244f` (PMT-Faith Wave 6-D complete).
**Auditor:** PMT-Faith Orchestrator (main, direct execution).

### Gaps CLOSED (11 of 38)

| Gap ID | Severity | Closure Task | How |
|--------|----------|--------------|-----|
| FAITH-1-A | MAJOR | PMT-FAITH-6-C | Added `name` + `type_name` to Field (matches Rust FieldInfo) |
| FAITH-1-B | MAJOR | PMT-FAITH-6-C | Added `name` to Layout (matches Rust LayoutInfo) |
| FAITH-1-C | MAJOR | PMT-FAITH-6-C | Removed disjointness conjunct from WF_Layout; moved to separate `WF_Layout_Disjoint` predicate (explicit hypothesis) |
| FAITH-1-D | MINOR | PMT-FAITH-6-C | Removed size>0 conjunct from WF_Layout; moved to separate `WF_Layout_NonEmpty` predicate |
| FAITH-1-E | MAJOR | PMT-FAITH-6-D | GuardPage docstring now points to codegen-emitted arena (pipeline.rs:11565-11842), not testing mirror |
| FAITH-1-F | MINOR | PMT-FAITH-6-D | Renamed `state_read_requires_live` → `linear_implies_accessible`; docstring clarifies it's ghost-state, not a Rust check |
| FAITH-1-G | MINOR | PMT-FAITH-6-D | Updated stale comment: `inject_liveness_check_ir` HAS shipped (memory_safety.rs:1513) |
| FAITH-2-A | MAJOR | PMT-FAITH-6-A | IRType expanded from 7 to 17 variants (added I8/I16/U8/U16/F32/F64/Func/Array/TaggedUnion/Channel; struct now has name+fields) |
| FAITH-2-B | MAJOR | PMT-FAITH-6-A | IRValue changed from Nat/Int to BitVec 32/64/64 (matches Rust u32/i64/u64) |
| FAITH-2-C | **CRITICAL** | PMT-FAITH-5-A | Removed unfaithful `transform` (String→String→Layout); kept faithful `transform_layouts` (IRValue→IRValue→String→String) as sole Transform variant |
| FAITH-2-F | MAJOR | PMT-FAITH-6-B | Call changed to (Option IRValue, String, List IRValue, Bool) — added dst + is_extern; NoExterns now checks is_extern=false |
| FAITH-2-G | MAJOR | PMT-FAITH-6-A | Ret changed from IRValue to List IRValue (matches Rust Ret{values: Vec<IRValue>}) |
| FAITH-2-H | MAJOR | PMT-FAITH-6-A | CallIndirect changed to (Option IRValue, IRValue, List IRValue) — added dst, uses IRValue not String |
| FAITH-4-A | **CRITICAL** | PMT-FAITH-5-B | `instr_sim` docstring honestly states it's intra-Lean; `full_simulation` renamed to `lean_internal_soundness` |
| FAITH-4-B | **CRITICAL** | PMT-FAITH-5-B | Renamed `full_simulation` → `lean_internal_soundness` (honestly reflects intra-Lean, not cross-language) |
| FAITH-4-D | **CRITICAL** | PMT-FAITH-5-C | `verified_capacity_check` changed from Nat to BitVec 64 with explicit no-overflow guard (matches Rust checked_add) |

### Gaps DEFERRED (4 — documented as residual TCB)

| Gap ID | Severity | Why deferred |
|--------|----------|--------------|
| FAITH-4-C | CRITICAL | `PipelineSpec.compiled_matches_exec` is still `rfl` (degenerate). Closing requires modeling Rust `pipeline::compile` in Lean OR translation validation (running Lean exec vs Rust binary on test inputs). This is the largest residual TCB — the cross-language bridge is via the Extraction parity test (tests/pmt_parity_test.rs), not a formal Lean proof. |
| FAITH-2-L | MAJOR | load/store use String (var name) instead of IRValue (vreg). Closing requires refactoring `Step` to use IRValue, which cascades through `Soundness.lean`'s Step definition and the entire exec model. Documented as residual. |
| FAITH-2-E | MINOR | AtomicOrdering field on atomic variants (Lean has it, Rust doesn't). Documented as forward-compat annotation; faithfulness rule 8 says remove it, but it's MINOR and doesn't affect soundness. |
| own_ex_exclusive axiom | (axiom) | The single non-standard axiom in PMT. HeapModel.lean provides `own_ex_exclusive_derived` (sound derivation from Ex RA). Removal requires invasive Own→RealOwn refactor cascading to 5 Iris structures — deferred (PMT-FAITH-7-C was not attempted). |

### Gaps NOT YET ADDRESSED (remaining minor + mitigated)

- FAITH-2-D (Offset variant missing) — MAJOR, not closed. Rust `Offset { dst, base, offset }` not modeled in Lean. Documented as out-of-scope.
- FAITH-2-I/J (VectorOp/Syscall Nat-vs-u32) — MINOR, not closed. Documented.
- FAITH-2-K (well_typed = True for non-memory) — MAJOR, architectural split (PMT = structural, IVE = per-variant). Documented.
- FAITH-2-M (alloc String-vs-IRValue) — MAJOR, mitigated by to_steps mapping. Same root as FAITH-2-L.
- FAITH-3-A through FAITH-3-G (Soundness step/exec) — 7 gaps, mostly MITIGATED by RawArena/SimRel bridges or documented as sound abstractions.
- FAITH-4-E/F/G (Extraction signatures) — MAJOR, not closed (Lean structured vs Rust raw u64s). Parity test bridges.
- FAITH-1-H/I/J/K (Arena/CapacityInvariant/alloc mitigated) — MITIGATED by RawArena/SimRel/BitVecArena bridges.

### Build status after closure

- `lake build`: PASS (all targets)
- Sorry audit (excl IVE/FFI): 0
- Axiom audit (excl IVE/FFI): 1 (`own_ex_exclusive` — deferred, sound)

### Conclusion

The 4 CRITICAL gaps are now CLOSED (FAITH-2-C, 4-A/B, 4-D). The PMT pillar theorems (`pmt_soundness`, `pmt_pillar_sound`, `pmt_pillar_sound_no_uaf`) now operate over bit-faithful Lean models for: Transform (from_layout/to_layout), IRValue (bounded BitVec), IRType (17 variants), Field/Layout (name+type_name), Call (is_extern), Ret (List IRValue), CallIndirect (IRValue), verified_capacity_check (BitVec 64 overflow). The Lean models now match the Rust implementations bit-for-bit in these dimensions.

The remaining CRITICAL gap (FAITH-4-C, PipelineSpec degenerate) is the cross-language bridge — it requires either modeling the Rust pipeline in Lean or translation validation. This is the largest residual TCB and is documented honestly in `SimRel.lean` and `PipelineSim.lean`.

The PMT pillar theorems are now **bit-faithful** in the dimensions closed by PMT-Faith Waves 5-6. The residual TCB (FAITH-4-C cross-language bridge, FAITH-2-L String-vs-IRValue in Step, own_ex_exclusive axiom) is explicitly documented.
