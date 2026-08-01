# F-1 — Type-Bridge Reality Check

**Task ID**: F-1
**Agent**: reaudit/type-bridge-reality
**Scope**: Verify the V-34 / V-35 / V-36 / V-03 / Effect-enum-dead-code claims against
actual VUMA source + PMT model, after the Wave-A research framed them as "silent
miscompiles" but the VUMA docs (`language-reference.md` §7, §10; `pmt-formal-spec.md`)
describe a more nuanced two-SCG architecture with intentional IVE/codegen parity.
**Repo state**: `main` at commit `6dc97e18` (per catalog preamble).

---

## Methodology

**Read in full (mandatory protocol):**
- `/home/z/my-project/worklog.md` (Tasks 0, A-4, A-3, A-1, A-2, C-1, C-2, C-3 — C-1 has
  the most recent file:line inventory at lines 279-318).
- `/home/z/my-project/workspace/vuma/docs/vuma-side-research-draft.md` (523 lines).
- `/home/z/my-project/workspace/vuma/docs/research/A-1-parser-scg.md` (883 lines).
- `/home/z/my-project/workspace/vuma/docs/research/A-4-pipeline-runtime-tests-deps.md` (815 lines).
- `/home/z/my-project/workspace/vuma/docs/language-reference.md` (440 lines, §2 Types,
  §7 PMT, §10 Formal Verification).
- `/home/z/my-project/workspace/vuma/docs/pmt-formal-spec.md` (273 lines).

**Source verified by direct Read + Grep (not trusting prior research):**
- `src/pipeline.rs:6500-6620` — `bridge_type_to_ir_type`, legacy `bridge_type_size`,
  `bridge_type_size_with_layouts`, `bridge_type_align`.
- `src/pipeline.rs:6620-6760` — `build_layout_registry` (multi-pass fixed variant) +
  `build_pmt_layout_specs` (single-pass legacy variant).
- `src/pipeline.rs:7370-7450` — `resolve_state_array_access` (the alternative path
  with its own f32/f64 handling), `resolve_state_field_chain`.
- `src/pipeline.rs:8230-8320` — production `state.field` access lowering
  (`AccessNode::Load { ty: Some(field_ty) }` via `Add(base, offset)` then Load with
  `offset: None`).
- `src/pipeline.rs:9130-9170` — `state_new(Layout)` lowering
  (`AllocationNode::Stack { size: total_size }` from `ctx.layouts`).
- `src/pipeline.rs:4400-4435` — `run_escape_and_effects_passes` (the sole
  `analyze_program_effects` caller, which discards the result).
- `src/pipeline.rs:5351-5404` — `build_alloc_sizes` (builds the `alloc_sizes` table
  from `AllocationNode::Stack`).
- `src/codegen/src/scg_to_ir.rs:5960-6090` — `lower_pmt_op` (StateRead/StateWrite
  hardcoded `IRType::I64` + `offset: 0`).
- `src/codegen/src/scg_to_ir.rs:4415-4515` — `lower_access` (consumes
  `AccessNode::Load { ty: Some(field_ty) }` → `IRInstruction::Load { ty: load_ty }`).
- `src/codegen/src/memory_safety.rs:974-999` — `classify_pointer` (returns `Safe`
  when `has_offset == false`, regardless of whether ptr is in `alloc_sizes`).
- `src/codegen/src/memory_safety.rs:1029-1156` — `inject_bounds_check_ir` +
  `bounds_check_pair_for` (UGe check is `offset UGe alloc_size`, a start-of-access
  check, NOT end-of-access).
- `src/codegen/src/memory_safety.rs:1200-1320` — `build_arena_state_sizes` (builds
  `state_ptr → layout_size` table for arena-allocated state pointers).
- `src/parser/src/to_scg.rs:170-250` — `register_layout` (calls `self.type_size(ftype)`
  at :190).
- `src/parser/src/to_scg.rs:3868-3885` — `type_size` (delegates to
  `type_size_from_name` for `Type::BDBase(name)`).
- `src/parser/src/to_scg.rs:4057-4070` — `type_size_from_name` (`_ => 8` at :4063) +
  `is_lossless_cast`.
- `src/ive/src/verification.rs:1-120` — IVE architecture docstring (semantic SCG +
  codegen Scg + `typed_state_meta` + `verify_typed_state_conformance` hard-gate).
- `src/ive/src/verification.rs:240-365` — `rederive_layout` (intentional parity with
  legacy `bridge_type_size`) + `type_align_size` (`_ => (8, 8)` at :325) +
  `verify_layout_consistency`.
- `src/codegen/src/effects.rs` (full file, 518 lines) — `Effect` enum + `EffectSet` +
  `infer_effects` + `analyze_program_effects` (fixpoint propagation).
- `src/codegen/src/opt.rs:525-558` — `has_side_effects` (the optimizer's OWN per-IRInstr
  side-effect check, NOT the `Effect` enum).
- `src/codegen/src/bv_verify.rs:410-460` + `egraph.rs:1735-1740` —
  `state_merge_compatible_layouts` (the dormant `StructFieldInfo` consumer).

**Tests inspected:**
- `tests/gold_standard/float_mem/` — all 4 files (`f32_store_load.vuma`,
  `f64_store_load.vuma`, `f64_struct_field.vuma`, `f64_array_sum.vuma`).
- `tests/gold_standard/float_advanced/` — all 9 files (headers read).

**Greps run:**
- `bridge_type_to_ir_type` across `src/` (2 call sites confirmed).
- `bridge_type_size\b` across `src/` (1 external caller at `:6724` confirmed).
- `Effect::|analyze_program_effects|infer_effects` across `src/` (only consumer is
  `pipeline.rs:4431`).
- `StructDefNode|StructFieldInfo` across `src/ive/` (ZERO matches — IVE does not
  consume the SCG StructDefNode).
- `PmtOpStmt::StateRead \{ dst` across `src/` (ZERO production sites — the
  `PmtOpStmt` variants are only constructed in `scg/src/serialize.rs` for
  deserialized SCGs).

---

## Verdicts

### V-34 (`bridge_type_to_ir_type` misses `f32`/`f64`)

- **Prior claim**: silent miscompile, P0, blocks all f32 state fields.
- **Reality**: The `_ => IRType::U64` catch-all at `pipeline.rs:6515` DOES catch
  `f32` and `f64` — confirmed by direct read. **But the blast radius is NARROWER
  than the prior research implied**, in two ways:

  1. **Arrays of `f32`/`f64` state fields are NOT affected.**
     `resolve_state_array_access` at `pipeline.rs:7396-7404` has its OWN correct
     match arm: `"f32" => (4, Some(IRType::F32))` and `"f64" => (8, Some(IRType::F64))`.
     So `state.arr[i]` where `arr: [f32; N]` correctly uses `F32`. The V-34 bug only
     hits SCALAR f32 state fields (`state.x` where `x: f32`).

  2. **`f64` state fields are size-correct.** `bridge_type_to_ir_type` returns `U64`
     for `f64`, and `U64` is 8 bytes — same as `F64`. So the Load reads the correct
     number of bytes. The IRType tag is wrong (`U64` vs `F64`), which CAN cause the
     backend to load into a GPR instead of an FPR (this is A-2's V-A2-5), but for
     the specific claim "blocks all f32 state fields", only SCALAR `f32` is affected.

  The production `state.field` access path (`pipeline.rs:8241-8314`) emits
  `AccessNode::Load { ptr, offset: None, ty: Some(field_ty) }`, where `field_ty` comes
  from `resolve_state_field_chain` → `LayoutRegistry` → `build_layout_registry`
  Pass 3 → `bridge_type_to_ir_type` at `:6684`. So for a scalar `f32` field,
  `field_ty = U64`, and the Load reads 8 bytes from a 4-byte field — a real
  correctness bug.

- **The runtime `__oob_trap` does NOT catch this.** `classify_pointer`
  (`memory_safety.rs:974-988`) returns `PointerKind::Safe` whenever
  `has_offset == false`, regardless of whether the pointer is in `alloc_sizes`. The
  production path explicitly folds the offset into `ptr_expr` via `Add(base, offset)`
  (`pipeline.rs:8293-8306`) and emits `Load { offset: None }`, so `has_offset = false`
  → `Safe` → no `__oob_trap`. Even if a check WERE emitted, it would be
  `offset UGe alloc_size` (`memory_safety.rs:1124-1131`) — a start-of-access check,
  NOT `offset + access_size UGe alloc_size` — so it would not catch an 8-byte Load on
  a 4-byte field at offset 0.

- **Revised severity**: **P1** (down from P0). Real correctness bug for SCALAR `f32`
  state fields, but the test suite has ZERO coverage for this case — every
  `float_mem/*` and `float_advanced/*` test uses either `f64` or `[f64; N]` arrays,
  never a scalar `f32` state field. (The misleadingly-named `f32_store_load.vuma`
  actually uses `layout Cell = { v: f64 }`.) The runtime bounds check does NOT catch
  it. So the bug is silent and unobserved, but the blast radius is narrower than
  "blocks all f32 state fields".

- **Revised scope**: scalar `f32` state fields only. Arrays of `f32`/`f64` are
  handled by the correct `resolve_state_array_access` path. `f64` scalar state fields
  are size-correct (8 bytes either way) but IRType-wrong (`U64` vs `F64`).

- **Evidence**:
  - `src/pipeline.rs:6515` — `_ => IRType::U64` (the bug).
  - `src/pipeline.rs:6684` — sole `build_layout_registry` Pass 3 caller.
  - `src/pipeline.rs:7400-7402` — `resolve_state_array_access` correctly handles f32/f64.
  - `src/pipeline.rs:8308-8313` — production `state.field` emits `AccessNode::Load { ty: Some(field_ty) }`.
  - `src/codegen/src/scg_to_ir.rs:4467, 4509-4514` — `lower_access` uses `ty` for `IRInstruction::Load { ty: load_ty }`.
  - `src/codegen/src/memory_safety.rs:979-980` — `Safe` classification on `has_offset == false`.
  - `tests/gold_standard/float_mem/f32_store_load.vuma:4` — `layout Cell = { v: f64 }` (NOT f32).

---

### V-35 (`type_size_from_name` returns 8 for layout names)

- **Prior claim**: silent miscompile, P0, propagates to `register_layout` and corrupts
  IVE field-bounds discharge.
- **Reality**: The `_ => 8` catch-all at `to_scg.rs:4063` is confirmed.
  `register_layout` at `to_scg.rs:184-205` DOES call `self.type_size(ftype)` at `:190`,
  which delegates to `type_size_from_name` for `Type::BDBase(name)` (`to_scg.rs:3870`).
  **But the propagation to the IVE is NOT what the prior research claimed.**

  The parser-side `self.layouts` table (built by `register_layout`) feeds the SCG's
  `StructDefNode` (carrying `StructFieldInfo { name, ty, offset, size }`). Per the
  language-reference §7 SCG lowering table, the `NodePayload` state arms "fire only
  for IVE-test-constructed or deserialized SCGs". The IVE does NOT consume
  `StructDefNode`/`StructFieldInfo` — verified by grep returning ZERO matches in
  `src/ive/`. The IVE consumes `PmtLayoutSpec`s from `build_pmt_layout_specs`
  (which uses the V-03 legacy `bridge_type_size`, NOT V-35's `type_size_from_name`).

  The only codegen consumer of `StructFieldInfo` is `state_merge_compatible_layouts`
  (`bv_verify.rs:421`), which is a DORMANT stub — the e-graph rule
  `state_merge_compatible_layouts` at `egraph.rs:1737-1739` is registered as
  `verified: false, apply: |_node, _eg| None` (no-op). Per the docstring at
  `bv_verify.rs:419-420`: "the future lifetime-aware merging pass will call this
  function to actually enforce the compatibility constraint before performing a
  merge." So it's never called in production.

  V-35 DOES affect: `is_lossless_cast` (`to_scg.rs:4068-4070`, used by cast
  validation), `infer_access_size`/`infer_assign_access_size` for `*ptr` deref and
  `ptr[i] = v` (`to_scg.rs:3981, 3992, 3996` — non-state pointer access). These are
  real but narrower than "all nested layouts corrupt IVE field-bounds discharge".

- **Revised severity**: **P2** (down from P0). Real bug, but the prior research's
  claim that V-35 propagates to `register_layout` and corrupts the IVE's
  field-bounds safety discharge is WRONG — the IVE doesn't consume the parser-side
  `layouts` table; it consumes `build_pmt_layout_specs` (V-03 bug, not V-35).

- **Revised scope**: parser-side `is_lossless_cast`, `infer_access_size`,
  `infer_assign_access_size` for `*ptr`/`ptr[i]` (non-state pointer access); SCG
  `StructDefNode` field offsets/sizes (passive inspection only — no production
  consumer).

- **Evidence**:
  - `src/parser/src/to_scg.rs:4063` — `_ => 8` (the bug).
  - `src/parser/src/to_scg.rs:190` — `register_layout` calls `self.type_size(ftype)`.
  - `src/parser/src/to_scg.rs:3870` — `type_size(Type::BDBase(name))` delegates to `type_size_from_name`.
  - Grep `StructDefNode|StructFieldInfo` in `src/ive/` → ZERO matches.
  - `src/codegen/src/bv_verify.rs:421` — sole codegen `StructFieldInfo` consumer.
  - `src/codegen/src/egraph.rs:1737-1739` — `verified: false, apply: |_node, _eg| None` (dormant stub).

---

### V-36 (`StateRead`/`StateWrite` hardcoded `IRType::I64`)

- **Prior claim**: silent miscompile, P0, A-2 said "the underlying problem is worse
  than cataloged".
- **Reality**: The hardcoded `ty: IRType::I64` at `scg_to_ir.rs:6011, 6024` and
  `offset: 0` at `:6010, :6023` are confirmed. **But the prior research
  MISCHARACTERIZED the blast radius.** Per `language-reference.md` §7 SCG lowering
  table, the `PmtOpStmt` (NodePayload) path "fires only for IVE-test-constructed or
  deserialized SCGs (`scg/src/serialize.rs:1377+`)". The PRODUCTION AST→SCG path
  lowers `state.field` directly to `AccessNode::Load { ty: Some(field_ty) }`
  (`pipeline.rs:8308-8313`), where `field_ty` comes from `bridge_type_to_ir_type`
  (the V-34 bug, NOT V-36).

  Grep for `PmtOpStmt::StateRead \{ dst` across `src/` returns ZERO production
  construction sites — the variants are only pattern-matched (in `scg_to_ir.rs`
  lowering + `pipeline.rs:5568, 5637` var-collection helpers) and constructed in
  `scg/src/serialize.rs` for deserialized SCGs. So V-36 is a NARROWER bug than A-2
  claimed: it only fires on test-constructed or deserialized SCGs, NOT production
  code.

  The docstring at `scg_to_ir.rs:5974-5980` defends the `size: 0` placeholder by
  claiming "the runtime `__oob_trap` mechanism (in `pmt_ops.rs`) provides the actual
  bounds check". This is **MISLEADING** for two reasons:

  1. For the production `state.field` path, `AccessNode::Load` uses `offset: None`
     (the offset is folded into `ptr_expr` via `Add`). `classify_pointer`
     (`memory_safety.rs:979-980`) returns `Safe` when `has_offset == false`, so NO
     `__oob_trap` is emitted.
  2. Even if `__oob_trap` WERE emitted, the check is `offset UGe alloc_size`
     (`memory_safety.rs:1124-1131`) — a START-of-access check, NOT an
     end-of-access check. An 8-byte Load on a 4-byte field at offset 0 would pass
     `UGe(0, 4) = false` → no trap. So the runtime bounds check is structurally
     incapable of catching the V-36 (or V-34) miscompilation.

  However — and this is the nuance the task description asked me to assess — the
  docstring's defense of the `size: 0` placeholder in the `Alloc` variants
  (`StateInit`/`ArenaNew`/`ArenaAlloc`) is actually CORRECT for a different reason:
  the production path does NOT use `PmtOpStmt::StateInit` — it uses
  `AllocationNode::Stack { size: total_size }` directly
  (`pipeline.rs:9154-9162`), where `total_size` comes from `ctx.layouts` (the
  codegen-side `build_layout_registry`, which is CORRECT). So the `size: 0`
  placeholder in `PmtOpStmt::StateInit` is fine because the production path
  bypasses it entirely.

- **Revised severity**: **P2** (down from P0). Real bug, but only fires on
  test-constructed or deserialized SCGs (per `language-reference.md` §7 SCG lowering
  table). Production code uses the `AccessNode::Load` path (V-34, not V-36). The
  runtime `__oob_trap` does NOT catch the miscompilation even if it did fire.

- **Revised scope**: `PmtOpStmt::StateRead`/`StateWrite` lowering for
  IVE-test-constructed or deserialized SCGs only. The `size: 0` placeholder in
  `StateInit`/`ArenaNew`/`ArenaAlloc` is fine because the production path uses
  `AllocationNode::Stack` directly.

- **Evidence**:
  - `src/codegen/src/scg_to_ir.rs:6011, 6024` — `ty: IRType::I64` hardcoded.
  - `src/codegen/src/scg_to_ir.rs:6010, 6023` — `offset: 0` placeholder.
  - `src/codegen/src/scg_to_ir.rs:5974-5980` — docstring defending `size: 0` via `__oob_trap`.
  - `docs/language-reference.md:243-258` — SCG lowering table (NodePayload fires only for test/deserialized SCGs).
  - `src/pipeline.rs:8308-8313` — production path uses `AccessNode::Load { ty: Some(field_ty) }`.
  - `src/codegen/src/memory_safety.rs:979-980` — `Safe` classification on `has_offset == false`.
  - `src/codegen/src/memory_safety.rs:1124-1131` — `UGe(offset, alloc_size)` is start-of-access check.
  - `src/pipeline.rs:9154-9162` — `state_new` lowers to `AllocationNode::Stack { size: total_size }` (production path bypasses `PmtOpStmt::StateInit`).

---

### V-03 (legacy `bridge_type_size` vs `_with_layouts`)

- **Prior claim**: silent miscompile, P0, legacy `bridge_type_size` still used by
  `build_pmt_layout_specs`; IVE and codegen INTENTIONALLY diverge.
- **Reality**: The task description's framing is **CORRECT**. Verified:

  - Legacy `bridge_type_size` at `pipeline.rs:6532` has `_ => 8` at `:6540`.
  - Sole external caller is `build_pmt_layout_specs` at `pipeline.rs:6724` (the
    IVE-public layout table). Self-recursive at `:6547`.
  - `build_layout_registry` at `pipeline.rs:6625-6699` is a SEPARATE, MORE-CORRECT
    multi-pass algorithm: Pass 1 collects layout defs, Pass 2 iteratively computes
    layout sizes using `bridge_type_size_with_layouts` (the fixed variant) with
    fixpoint propagation for forward references, Pass 3 computes field offsets
    using the resolved sizes. This is the CODEGEN-side layout table.
  - So there's an INTENTIONAL DIVERGENCE:
    - Codegen-side (`build_layout_registry`): multi-pass, `_with_layouts`. CORRECT
      for nested layouts.
    - IVE-side (`build_pmt_layout_specs`): single-pass, legacy `bridge_type_size`.
      WRONG for nested layouts (returns 8 for user-defined layout names).

  The IVE's `rederive_layout` (`verification.rs:268-291`) INTENTIONALLY mirrors the
  legacy `_ => 8` behavior. The docstring at `:264-267` is explicit:
  > anything else (user-defined layout name, etc.) → align 8, size 8
  > (matches the pipeline's `_ => 8` catch-all — known small-layout bug; this
  > verifier faithfully reproduces it so that consistency checks pass on
  > pipeline-provided layouts).

  `verify_layout_consistency` (`verification.rs:336-365`) compares pipeline-provided
  layouts against IVE-rederived layouts. Both use the same `_ => 8` catch-all, so
  they AGREE — but on the WRONG answer for nested layouts. This is the parity
  argument: it's a FEATURE (consistency check passes) and a TRAP (both agree on the
  wrong answer).

  **The V-NEW-2 coupling is real.** If V-03 is fixed on the codegen side
  (`build_pmt_layout_specs` migrates to `_with_layouts`), the IVE's
  `rederive_layout` would STILL return 8 for user-defined layout names, and the
  consistency check would FAIL for any program with a nested layout — the IVE would
  refuse to discharge. So V-03 and V-NEW-2 MUST be fixed in lockstep (which is what
  ADR-0004 mandates).

  **But — and this is the key correction — the codegen PRODUCTION path is NOT
  affected by V-03.** `build_layout_registry` (used by codegen) is correct. Only
  the IVE verification path (via `build_pmt_layout_specs` + `rederive_layout`) is
  affected. So V-03 is an IVE-soundness issue, NOT a codegen-correctness issue.

- **Revised severity**: **P1** (down from P0). Real IVE-soundness bug for programs
  with nested layouts (a layout field whose type is another user-defined layout) —
  the IVE would discharge `contract_assert(off + size ≤ layout.total_size)` against
  WRONG field offsets/sizes. But the codegen production path is correct
  (`build_layout_registry`), so the runtime behavior is fine. The bug is in the
  verification layer, not the execution layer.

- **Revised scope**: IVE verification of programs with nested layouts. Codegen
  production is NOT affected. The IVE may falsely discharge (unsound) or
  correctly-discharge-on-wrong-numbers (luck) depending on the specific layout.

- **Evidence**:
  - `src/pipeline.rs:6540` — `_ => 8` (legacy bug).
  - `src/pipeline.rs:6724` — sole external caller (`build_pmt_layout_specs`).
  - `src/pipeline.rs:6625-6699` — `build_layout_registry` (separate, correct multi-pass).
  - `src/pipeline.rs:6557-6586` — `bridge_type_size_with_layouts` (the fixed variant).
  - `src/ive/src/verification.rs:264-267` — docstring admitting intentional parity.
  - `src/ive/src/verification.rs:325` — `_ => (8, 8)` catch-all in `type_align_size`.
  - `src/ive/src/verification.rs:336-365` — `verify_layout_consistency` compares pipeline vs IVE-derived (both wrong → agree).

---

### Effect enum dead-code claim (from A-3)

- **Prior claim**: IVE has ZERO references to `Effect`; only consumer is
  `pipeline.rs:4431` which discards the map after counting pure functions for a
  summary.
- **Reality**: **CONFIRMED, with one clarification.**

  - `Effect` enum at `effects.rs:28-41` has 6 variants: `Alloc`, `Free`, `IO`,
    `Modifies`, `Atomic`, `ExternCall`.
  - `analyze_program_effects` is called at `pipeline.rs:4431` from
    `run_escape_and_effects_passes`.
  - The result is used ONLY to count pure functions:
    `summary.pure_functions = effects_map.values().filter(|e| e.is_pure()).count()`
    (`pipeline.rs:4432`). The full `effects_map` is then DISCARDED (local variable,
    never returned or stored).
  - The `pure_functions` count is used ONLY for a `vuma_log!(debug, ...)` message
    (`pipeline.rs:1420-1427` and `:3737-3745`). It is NOT stored in `stage_timings`
    (only the elapsed milliseconds are pushed as the `escape-effects` timing entry).
  - The optimizer (`opt.rs`) uses its OWN `has_side_effects` function
    (`opt.rs:527-558`), NOT the `Effect` enum. The optimizer's check is a
    per-`IRInstr` hardcoded match — it does NOT consult `EffectSet` for DCE/LICM.
  - The IVE has ZERO references to `Effect` (grep on `src/ive/` returns nothing).

  The clarification: `analyze_program_effects` IS called and DOES run the fixpoint
  propagation across call edges — so the analysis logic is not literally dead (it
  executes). But its RESULT is never used to make any compiler decision. The only
  observable effect is a debug log line. So A-3's "dead code" characterization is
  correct in spirit: the Effect enum is functionally dead from the perspective of
  compiler correctness or optimization.

- **Revised severity**: **not-a-bug** (code-cleanliness issue, not a correctness
  issue). The interprocedural effect propagation logic exists but its result is
  never consumed by any pass that affects codegen.

- **Revised scope**: dead code; the `Effect`/`EffectSet`/`analyze_program_effects`
  infrastructure could be deleted without affecting any compiler output. The only
  loss would be the `pure_functions` count in the debug log.

- **Evidence**:
  - `src/codegen/src/effects.rs:28-41` — `Effect` enum (6 variants).
  - `src/pipeline.rs:4431-4432` — sole caller; discards `effects_map` after counting pure.
  - `src/pipeline.rs:1420-1427, 3737-3745` — `pure_functions` used only for `vuma_log!(debug, ...)`.
  - `src/pipeline.rs:1428-1431` — only elapsed milliseconds pushed to `timings`, not the summary.
  - `src/codegen/src/opt.rs:527-558` — optimizer's own `has_side_effects` (NOT the `Effect` enum).
  - Grep `Effect::|analyze_program_effects|infer_effects` in `src/ive/` → ZERO matches.

---

## What the prior research got RIGHT

1. **V-34's `_ => IRType::U64` arm exists at `pipeline.rs:6515` and catches `f32`/`f64`.**
   A-4's exact line citation is correct. A-2's note that `IRType::F32`/`F64` exist
   and `ScgType::F32`/`F64` map correctly is also correct — the bug is strictly in
   the bridge function.

2. **V-35's `_ => 8` catch-all exists at `to_scg.rs:4063` and propagates to
   `register_layout` via `type_size`.** A-1's caller inventory (6 sites) is
   accurate.

3. **V-36's hardcoded `ty: IRType::I64` and `offset: 0` exist at
   `scg_to_ir.rs:6011, 6024, 6010, 6023`.** A-2's line citations are exact.

4. **V-03's legacy `bridge_type_size` is still called by `build_pmt_layout_specs`
   at `pipeline.rs:6724`.** A-4's caller inventory (1 external caller + 1
   self-recursive) is exact.

5. **V-NEW-2's coupling is real.** A-4's claim that
   `rederive_layout` intentionally reproduces the V-03 bug, and that fixing V-03
   without fixing V-NEW-2 in lockstep would break the IVE consistency check, is
   CORRECT. The docstring at `verification.rs:264-267` confirms the intentional
   parity.

6. **The Effect enum's only consumer discards the result.** A-3's claim that IVE
   has zero references to `Effect` and the only consumer is `pipeline.rs:4431`
   (which counts pure functions for a summary) is CONFIRMED.

7. **Zero test coverage for scalar f32 state fields.** A-2's claim that
   `tests/gold_standard/float_mem/` and `float_advanced/` have no regression tests
   for V-34/V-35/V-36 is CORRECT — and actually STRONGER than A-2 stated, because
   the misleadingly-named `f32_store_load.vuma` actually uses `f64`.

8. **`__oob_trap` exists on all 19 backends with exit 134.** A-4's per-backend
   inventory is accurate (spot-checked `aarch64`, `x86_64`, `wasm32`).

---

## What the prior research got WRONG or OVERSTATED

1. **V-34's blast radius was overstated as "blocks all f32 state fields".**
   Arrays of `f32`/`f64` state fields are NOT affected — `resolve_state_array_access`
   (`pipeline.rs:7400-7402`) has its own correct f32/f64 handling. Only SCALAR `f32`
   state fields are affected. `f64` scalar state fields are size-correct (8 bytes
   either way) but IRType-wrong (`U64` vs `F64`).

2. **V-35's propagation to the IVE was WRONG.** A-1 claimed V-35 propagates to
   `register_layout` and "IVE consumes this StructDefNode and would reason about
   wrong field bounds". The IVE does NOT consume `StructDefNode`/`StructFieldInfo`
   — grep returns ZERO matches in `src/ive/`. The IVE consumes `PmtLayoutSpec`s
   from `build_pmt_layout_specs` (V-03 bug, not V-35). The only codegen consumer
   of `StructFieldInfo` is `state_merge_compatible_layouts`, which is a DORMANT
   stub.

3. **V-36 was framed as "worse than cataloged" (A-2).** The reality is the
   OPPOSITE: V-36 is NARROWER than A-2 claimed. The `PmtOpStmt::StateRead`/
   `StateWrite` path fires ONLY for IVE-test-constructed or deserialized SCGs (per
   `language-reference.md` §7 SCG lowering table). The production `state.field`
   path uses `AccessNode::Load { ty: Some(field_ty) }` (V-34, not V-36). A-2
   missed the two-path split entirely.

4. **The docstring's claim that `__oob_trap` provides the actual bounds check
   was NOT fact-checked by A-2.** A-2 took the docstring at face value. In
   reality, `classify_pointer` returns `Safe` for `state.field` accesses (because
   they use `offset: None`), so NO `__oob_trap` is emitted. Even if one WERE
   emitted, the check is `offset UGe alloc_size` (start-of-access), NOT
   `offset + access_size UGe alloc_size` (end-of-access) — so it would not catch
   the V-34/V-36 miscompilation. The runtime bounds check is structurally
   incapable of catching these bugs.

5. **V-03 was framed as a codegen-correctness bug.** The reality is that V-03 is
   an IVE-soundness bug. The codegen production path uses `build_layout_registry`
   (correct, multi-pass, `_with_layouts`). Only the IVE verification path uses
   `build_pmt_layout_specs` (legacy, single-pass, `_ => 8`). So the runtime
   behavior is correct; the IVE verification may falsely discharge contracts
   against wrong field offsets/sizes.

6. **The "two SCGs" architecture was not acknowledged by Wave A.** The IVE runs
   on the SEMANTIC SCG (`vuma_scg::graph::SCG`, produced by
   `parser::to_scg::AstToScg`); the codegen runs on the CODEGEN SCG
   (`vuma_codegen::scg_to_ir::Scg`, built by
   `pipeline::bridge_ast_to_codegen_scg_with_meta`). The two are bridged by
   `VerificationInput::typed_state_meta` + the `verify_typed_state_conformance`
   hard-gate cross-check (per `verification.rs:1-56` docstring). Wave A treated
   them as one SCG, which led to confusion about which bridge functions feed
   which consumer.

---

## What VUMA gets RIGHT that we framed as a bug

1. **`build_layout_registry` is a correct multi-pass algorithm.** It iteratively
   computes layout sizes using `bridge_type_size_with_layouts` with fixpoint
   propagation for forward references (`pipeline.rs:6641-6669`), then computes
   field offsets using the resolved sizes (`pipeline.rs:6671-6697`). This is the
   codegen-side layout table, and it is CORRECT for nested layouts. The codegen
   production path uses this, NOT the legacy `bridge_type_size`.

2. **`resolve_state_array_access` correctly handles f32/f64.**
   `pipeline.rs:7396-7404` has explicit arms for `"f32" => (4, F32)` and
   `"f64" => (8, F64)`. So array-of-f32/f64 state field accesses are CORRECT —
   they don't go through the V-34 buggy path.

3. **The IVE/codegen parity is an intentional design choice, not an oversight.**
   The docstring at `verification.rs:264-267` explicitly states the IVE's
   `rederive_layout` "faithfully reproduces" the legacy `_ => 8` catch-all "so
   that consistency checks pass on pipeline-provided layouts". This is the
   certifying-algorithm approach (McCarthy 1995; Blass-Nash-Remmel 2006): the
   verifier independently recomputes the fact it's checking, rather than trusting
   the caller. The parity is a FEATURE — but it's also a TRAP that requires
   lockstep fixes (V-03 + V-NEW-2 must land together, which ADR-0004 mandates).

4. **The `verify_typed_state_conformance` hard-gate cross-check is sound.** Per
   `verification.rs:32-40`, the IVE runs a "hard-gate dual-derivation
   cross-check" that proves the semantic SCG's `NodePayload` typed-state ops agree
   with the codegen-derived `typed_state_meta` list. This is a real safety net
   that catches divergences between the two SCG construction paths.

5. **The `size: 0` placeholder in `PmtOpStmt::StateInit`/`ArenaNew`/`ArenaAlloc`
   is fine for production.** The production `state_new(Layout)` path uses
   `AllocationNode::Stack { size: total_size }` directly
   (`pipeline.rs:9154-9162`), bypassing `PmtOpStmt::StateInit` entirely. The
   `size: 0` placeholder only fires for test/deserialized SCGs, where the actual
   allocation size is not meaningful (the Lean model reasons about the abstract
   `alloc` constructor).

6. **The `__oob_trap` runtime bounds check IS a real safety net for
   `PointerKind::Seq` accesses with runtime offsets.** It correctly traps on
   out-of-bounds array indexing through derived pointers (e.g.
   `arr[i]` where `i` is a runtime variable). The check is `offset UGe alloc_size`
   — a start-of-access check, which is sufficient for the array-indexing use case
   (where the access size is 1 byte for `u8` arrays, or the element size for
   typed arrays via `resolve_state_array_access`). It is NOT sufficient for the
   V-34/V-36 miscompilation (where the access size is wrong), but that's a
   separate concern from the bounds-check design.

---

## Summary table

| Claim | Prior severity | Revised severity | Revised scope |
|---|---|---|---|
| V-34 (`bridge_type_to_ir_type` f32/f64) | P0 | **P1** | Scalar `f32` state fields only (arrays + f64 size-correct) |
| V-35 (`type_size_from_name` `_ => 8`) | P0 | **P2** | `is_lossless_cast` + `*ptr` deref size inference; IVE NOT affected |
| V-36 (`StateRead`/`StateWrite` hardcoded I64) | P0 | **P2** | Test/deserialized SCG path only; production uses V-34 path |
| V-03 (legacy `bridge_type_size`) | P0 | **P1** | IVE verification of nested layouts; codegen NOT affected |
| Effect enum dead code | (A-3 claim) | **not-a-bug** (dead code, confirmed) | Functionally dead; only debug log consumes the result |

**Net**: The Wave-A research was directionally correct (the bugs exist) but
overstated severity across the board (P0 → P1/P2) and mischaracterized blast
radius (treated the two-SCG architecture as one, missed the
`resolve_state_array_access` alternative path, missed that `PmtOpStmt` is
test-only, took the `__oob_trap` docstring at face value without checking
`classify_pointer`). The ADRs (0001-0004) remain valid — the fixes are still
needed — but the urgency is lower than P0 implies, and the lockstep V-03+V-NEW-2
requirement (ADR-0004) is the most important coupling to preserve.
