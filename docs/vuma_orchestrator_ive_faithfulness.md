# VUMA IVE Orchestrator — Faithfulness Gap Closure (Wave 5+)

**Pillar**: IVE (Invariant Verification Engine)
**Scope**: Close all faithfulness gaps identified in `proof/FAITHFULNESS_AUDIT.md`. The current Lean theorems prove soundness of **Lean abstractions**, not of the production **Rust IVE**. This orchestrator rewrites the Lean models to faithfully mirror the Rust implementations, so `ive_pillar_sound` proves soundness of the actual Rust IVE.
**Repository**: `https://github.com/pkhairkh/vuma.git` — clone to `/home/z/my-project/vuma`.
**Total tasks**: 11 (4 critical-gap closures + 5 major-gap closures + 1 pillar re-proof + 1 final audit)
**Estimated effort**: ~28 person-weeks (~7 months for one Lean+Rust expert)

**CRITICAL MANDATE**: Every Lean model must match its Rust implementation **exactly** — same parameters, same checks, same failure modes, same data structures. If Rust checks X and Lean checks Y, that's a gap. If Rust uses a HashMap and Lean uses a List, that's a gap. If Rust has a check Lean omits (or vice versa), that's a gap. The goal is **bit-faithful modeling**, not abstraction.

---

## Faithfulness Rules (NON-NEGOTIABLE)

1. **Same signature shape.** If Rust takes `(layouts: &HashMap<String, LayoutInfo>, input_layout: &str, output_layout: &str)`, Lean takes `(layouts : String → Option LayoutInfo) (input_layout : String) (output_layout : String)`. Do NOT replace a HashMap lookup with a pre-resolved struct.

2. **Same failure modes.** If Rust has a "layout not found → invalid" path (lines 62-72), Lean must have the same `Option`-based path. Do NOT collapse multiple Rust lookups into one Lean `Option`.

3. **Same check ordering.** If Rust checks `after_consume` first (line 65), then layout lookup (line 78), then field lookup (line 104), then bounds (line 107), then type match (line 118) — Lean must check in the same order. The error messages and `valid` field must match.

4. **Same data structures.** If Rust's `FieldInfo` has `{name, offset, size, type_name}`, Lean's field representation must carry all 4 fields. Do NOT drop `name` (as the current Lean model does) or `type_name` (as the current model externalizes into a separate `field_types` map).

5. **Same arithmetic.** If Rust uses `saturating_add` / `checked_add` (overflow → reject), Lean must model overflow. If Rust uses `u64`, Lean must use `Nat` with an explicit overflow check (since `Nat` doesn't overflow). Do NOT silently use `Nat` arithmetic where Rust uses wrapping/saturating `u64`.

6. **Same control flow.** If Rust has path-sensitive analysis (Branch/ElseStart/Join with state snapshots), Lean must model the path-sensitivity. Do NOT replace path-sensitive analysis with a flat prefix-scan.

7. **Same variant coverage.** If Rust's `FlowKind` has 4 variants (Assign, BinOp, ChannelSend, Branch), Lean must have 4 variants. Do NOT model only 1.

8. **Rust is the source of truth.** If the Lean theorem would be easier with a different model, **change the Lean model, not the Rust code** (unless the orchestrator spec explicitly says to change Rust). The Rust code is what runs in production.

---

## Other Orchestrators (Awareness)

Same as the original IVE orchestrator spec — PMT and FFI orchestrators work on the same repo. Codedomain ownership is unchanged, EXCEPT that this orchestrator may modify Rust IVE files when the faithfulness rules require it (e.g., to expose a `type_hash` function that Lean needs to call, or to change a return type to match the Lean model). All Rust changes must be coordinated via the worklog.

| Orchestrator | Pillar | Codedomain |
|--------------|--------|------------|
| **PMT** | PMT model | `proof/PMT/*.lean` (except `IVE/`), `proof/PMT/Iris/`, `src/codegen/src/runtime/arena.rs` |
| **IVE** (this doc) | IVE engine + faithful Lean models | `src/ive/**/*.rs`, `proof/PMT/IVE/`, `proof/extracted/`, `tests/pmt_parity_test*.rs` |
| **FFI** | FFI removal (No-FFI path) | `src/ffi.rs`, `src/codegen/src/runtime/{marshal,vuma_context,callback,ffi_scratch,pmt_ops}.rs`, `proof/PMT/NoFFI.lean`, `proof/PMT/FFI/` |

---

## Codedomain Ownership

### IVE-owned (exclusive write — no other orchestrator touches these)

**Rust source** (may be modified when faithfulness requires it):
- `src/ive/src/state_transform.rs` — expose `type_hash` if needed; otherwise read-only.
- `src/ive/src/state_read.rs`, `state_write.rs` — read-only unless a signature change is needed.
- `src/ive/src/arena_bounds.rs`, `borrow_region.rs`, `information_flow.rs`, `session_type.rs` — read-only unless a signature change is needed.
- `src/ive/src/constraint.rs`, `inference.rs`, `verification.rs` — read-only unless a signature change is needed.
- `src/ive/Cargo.toml`

**Lean proofs** (all rewritten in this orchestrator):
- `proof/PMT/IVE/Soundness/Transform.lean` (rewritten — gap T1-T4)
- `proof/PMT/IVE/Soundness/StateReads.lean` (rewritten — gap R1-R4)
- `proof/PMT/IVE/Soundness/StateWrites.lean` (rewritten — gap W1-W4)
- `proof/PMT/IVE/Soundness/ArenaBounds.lean` (rewritten — gap A1-A4)
- `proof/PMT/IVE/Soundness/BorrowRegion.lean` (rewritten — gap B1-B6)
- `proof/PMT/IVE/Soundness/InformationFlow.lean` (rewritten — gap F1-F5)
- `proof/PMT/IVE/Soundness/SessionType.lean` (rewritten — gap S1-S5)
- `proof/PMT/IVE/Soundness/L1L3Collapse.lean` (rewritten — gap L1-L5)
- `proof/PMT/IVE/Soundness/DependentTransform.lean` (rewritten — gap D1-D4)
- `proof/PMT/IVE/Soundness/ConstraintInference.lean` (rewritten — gap C1-C4)
- `proof/PMT/IVE/Soundness/LayoutConsistency.lean` (rewritten — gap LC1-LC4)
- `proof/PMT/IVE/Soundness/Composition.lean` (updated to use new models)
- `proof/PMT/IVE/PillarSoundness.lean` (re-proven with faithful models)
- `proof/PMT/IVE/Soundness/WFLayoutBool.lean` (unchanged — already faithful)

**Tests & audit**:
- `tests/pmt_parity_test.rs` — extended with faithful-model parity tests.
- `tests/pmt_parity_test_full.rs` — updated for new models.
- `proof/FAITHFULNESS_AUDIT.md` — updated to reflect gap closures.
- `proof/AUDIT_IVE.md` — updated for re-audit.

### Shared files (multiple orchestrators touch — see conflict rules below)
- `proof/PMT/Extraction.lean` — IVE owns the `@[export]` annotations (append-only).
- `src/pipeline.rs` — all three; IVE edits only the `verify_pmt` wiring section.
- `Cargo.toml` (workspace + per-crate) — all three; APPEND-ONLY to features/deps.
- `docs/caveats.md` — all three; APPEND-ONLY.

### Conflict resolution rules
Same as the original IVE orchestrator spec.

---

## Coordination Protocol (Pull / Commit / Push Between Waves)

Same as the original IVE orchestrator spec:
1. **Before each wave**: `git pull origin main`, resolve conflicts, `cargo build --release && lake build`.
2. **During the wave**: subagents work on `task/ive-faith-<wave>-<letter>` branches.
3. **After each wave**: commit, push, post worklog entry.
4. **Prerequisite checks**: read worklog before starting; verify dependencies.

---

## Orchestrator Rules

Same as the original IVE orchestrator spec:
1. **Wave-based execution.** Max 6 parallel subagents per wave.
2. **Each subagent prompt is self-contained** (≤400 words, ≤6 files read, ≤3 files modified).
3. **Subagents work on `task/ive-faith-<wave>-<letter>` branches.** Merged to `main` after build passes.
4. **Build verification mandatory**: `cargo build --release`, `cargo test`, `lake build` must all PASS.
5. **Sorry-free mandatory**: `grep -rn "sorry\|admit" proof/ | wc -l` must be 0 after each wave.
6. **Line numbers forbidden** in prompts — use grep patterns or symbol names.
7. **Full faithfulness, not samples** — every task produces a COMPLETE faithful model.
8. **Pull before each wave, push after each wave.**
9. **NEW: Faithfulness check mandatory.** Each subagent must include a "faithfulness checklist" in its worklog entry showing, line-by-line, that the Lean model matches the Rust implementation. The orchestrator reviews these checklists before merging.

---

## Wave 5 — Critical Gap Closures (4 subagents in parallel)

**Goal**: Close the 4 critical gaps where Lean proves a different property than Rust checks.
**Prerequisites**: Wave 4 complete (current main).
**Max parallel**: 4.

---

### Subagent IVE-FAITH-5-A — Rewrite Transform.lean to faithfully model `verify_transform`

```
Task ID: IVE-FAITH-5-A
Wave: 5 (Faithfulness)
Branch: task/ive-faith-5-a

Background: The current Transform.lean takes `StateTransform` with
`in_layout : Layout`, `out_layout : Layout`, `kind : TransformKind` (given).
It checks `WF_Layout` for both layouts. This does NOT match Rust.

Rust `src/ive/src/state_transform.rs::verify_transform` (search "pub fn verify_transform"):
  - Signature: `(layouts: &HashMap<String, LayoutInfo>, input_layout: &str, output_layout: &str)`
  - Step 1: Look up `input_layout` by NAME in `layouts` (not-found → invalid, kind=Copy).
  - Step 2: Look up `output_layout` by NAME (not-found → invalid, kind=Copy).
  - Step 3: If `input_layout == output_layout` (STRING equality) → Identity, valid.
  - Step 4: If `in_info.total_size == out_info.total_size` → Reinterpret, valid.
  - Step 5: Otherwise → Copy, valid.
  - Does NOT check WF_Layout for either layout.

Task: Rewrite Transform.lean to faithfully model the Rust function.

Read (≤5 files): src/ive/src/state_transform.rs (search "pub fn verify_transform"),
proof/PMT/IVE/Soundness/Transform.lean (current, to understand what to replace),
proof/PMT/Basic.lean (Layout, Field structures),
proof/PMT/IVE/Soundness/WFLayoutBool.lean (wf_layout_bool, to reference but NOT use in the check),
proof/PMT/Field.lean (LayoutInfo if it exists, else use Basic.lean's Layout).

Modify (≤2 files): proof/PMT/IVE/Soundness/Transform.lean (rewrite),
proof/PMT/Extraction.lean (update leanVerifyTransform to match new signature — append-only).

Concrete faithful model:
  - Define `LayoutInfo` in Lean mirroring Rust: `{name : String, total_size : Nat, fields : List FieldInfo}`.
  - Define `FieldInfo` mirroring Rust: `{name : String, offset : Nat, size : Nat, type_name : String}`.
  - Define `LayoutRegistry := String → Option LayoutInfo` (models the HashMap).
  - `verify_transform (layouts : LayoutRegistry) (input_layout output_layout : String) : StateTransformVerification`
  - Step 1: `match layouts input_layout with | none => {valid := false, kind := Copy, …} | some in_info => …`
  - Step 2: `match layouts output_layout with | none => {valid := false, kind := Copy, …} | some out_info => …`
  - Step 3: `if input_layout = output_layout then {valid := true, kind := Identity, …}`
  - Step 4: `else if in_info.total_size = out_info.total_size then {valid := true, kind := Reinterpret, …}`
  - Step 5: `else {valid := true, kind := Copy, …}`
  - NO WF_Layout check anywhere.

Theorem `verify_transform_sound`:
  If `(verify_transform layouts input output).valid = true`, then:
  - `layouts input = some in_info` (input layout exists)
  - `layouts output = some out_info` (output layout exists)
  - The kind is determined by the name/size comparison rules above.
  NO WF_Layout conclusion (Rust doesn't check it).

Faithfulness checklist (include in worklog):
  [x] Lean takes layout names (strings), not Layout structs.
  [x] Lean looks up layouts by name in a registry (String → Option LayoutInfo).
  [x] Lean checks layout-not-found for BOTH input and output (2 separate Option paths).
  [x] Lean infers the kind from name equality (Identity) then size equality (Reinterpret) then Copy.
  [x] Lean does NOT check WF_Layout.
  [x] Lean's Identity = string equality, NOT structural Layout equality.
  [x] Lean's Reinterpret = total_size equality only (no field overlap check — Rust comments it out).

Deliverable: Faithful Transform.lean. Build status: lake build PASS, zero sorry.
Worklog entry with faithfulness checklist.

Do NOT: Check WF_Layout (Rust doesn't). Take kind as input (Rust infers it). Use structural Layout equality for Identity.
```

---

### Subagent IVE-FAITH-5-B — Rewrite DependentTransform.lean to faithfully model `verify_dependent_transform`

```
Task ID: IVE-FAITH-5-B
Wave: 5 (Faithfulness)
Branch: task/ive-faith-5-b

Background: The current DependentTransform.lean takes `DependentTransform` with
`in_layout : Layout`, `out_layout : Layout`, `dep_value : Nat`, and checks
`WF_Layout in ∧ WF_Layout out ∧ dep_value ≤ out.total_size`. This does NOT match Rust.

Rust `src/ive/src/state_transform.rs::verify_dependent_transform` (search "pub fn verify_dependent_transform"):
  - Signature: `(elem_size: u64, count: u64, offset: u64, buffer_size: u64) -> bool`
  - Returns `offset.saturating_add(count.saturating_mul(elem_size)) <= buffer_size`.
  - Uses SATURATING arithmetic: overflow → u64::MAX → rejected.
  - This is a Presburger arithmetic check: `offset + count × elem_size ≤ buffer_size`.
  - NO layout well-formedness check. NO WF_Layout.

Task: Rewrite DependentTransform.lean to faithfully model the Rust function.

Read (≤4 files): src/ive/src/state_transform.rs (search "verify_dependent_transform"),
proof/PMT/IVE/Soundness/DependentTransform.lean (current, to replace),
proof/PMT/IVE/Soundness/WFLayoutBool.lean (reference, do NOT use).

Modify (≤1 file): proof/PMT/IVE/Soundness/DependentTransform.lean (rewrite).

Concrete faithful model:
  - Define `verify_dependent_transform (elem_size count offset buffer_size : Nat) : Bool`.
  - Model saturating arithmetic: since Nat doesn't overflow, model Rust's
    `saturating_mul` as: if `count * elem_size` would overflow u64 (i.e.,
    `count * elem_size > 2^64 - 1`), return `2^64 - 1` (which is > any
    realistic buffer_size, so the check fails).
  - Define `saturating_mul_64 (a b : Nat) : Nat := if a * b > 2^64 - 1 then 2^64 - 1 else a * b`.
  - Define `saturating_add_64 (a b : Nat) : Nat := if a + b > 2^64 - 1 then 2^64 - 1 else a + b`.
  - `verify_dependent_transform elem_size count offset buffer_size :=
      saturating_add_64 offset (saturating_mul_64 count elem_size) ≤ buffer_size`.

Theorem `verify_dependent_transform_sound`:
  If `verify_dependent_transform elem_size count offset buffer_size = true`, then
  `offset + count * elem_size ≤ buffer_size` (in Nat, no overflow) AND
  `count * elem_size < 2^64` AND `offset + count * elem_size < 2^64`
  (i.e., no overflow occurred — the saturating arithmetic didn't kick in).

Faithfulness checklist:
  [x] Lean takes 4 Nat params (elem_size, count, offset, buffer_size), matching Rust's 4 u64 params.
  [x] Lean uses saturating arithmetic (models u64 overflow).
  [x] Lean checks `offset + count * elem_size ≤ buffer_size` (Presburger).
  [x] Lean does NOT check WF_Layout.
  [x] Lean does NOT take Layout structs.
  [x] Lean returns Bool (matching Rust's `-> bool`).

Deliverable: Faithful DependentTransform.lean. Build status: lake build PASS, zero sorry.
Worklog entry with faithfulness checklist.

Do NOT: Check WF_Layout. Take Layout structs. Use plain Nat arithmetic without overflow modeling.
```

---

### Subagent IVE-FAITH-5-C — Rewrite ConstraintInference.lean to faithfully model `constraint.rs::check_against`

```
Task ID: IVE-FAITH-5-C
Wave: 5 (Faithfulness)
Branch: task/ive-faith-5-c

Background: The current ConstraintInference.lean has 3 arithmetic constraint types
(le, ge, eq) and `ModelState` with `values : List (String × Nat)`. This does NOT match Rust.

Rust `src/ive/src/constraint.rs` has 5+ constraint types, each with `description: String`:
  - TemporalConstraint { description: String }
  - ResourceFlowConstraint { description: String }
  - SecurityConstraint { description: String }
  - ComplexityConstraint { description: String }
  - LivenessConstraint { description: String }
Each `check_against(&self, model: &ModelState) -> bool` uses STRING CONTAINMENT:
  - Temporal: `self.description.contains(a) && self.description.contains(b)` for each `(a,b)` in `model.temporal_violations`.
  - ResourceFlow: same pattern with `model.blocked_flows`.
  - Security: `self.description.contains(violation)` for each violation in `model.security_violations`.

Rust `ModelState` has:
  - temporal_violations: Vec<(String, String)>
  - blocked_flows: Vec<(String, String)>
  - security_violations: Vec<String>
  (and possibly more — read constraint.rs fully.)

Task: Rewrite ConstraintInference.lean to faithfully model the Rust system.

Read (≤4 files): src/ive/src/constraint.rs (FULL — read all constraint types + ModelState),
src/ive/src/inference.rs (search "pub fn" — understand how constraints are derived),
proof/PMT/IVE/Soundness/ConstraintInference.lean (current, to replace).

Modify (≤1 file): proof/PMT/IVE/Soundness/ConstraintInference.lean (rewrite).

Concrete faithful model:
  - Define `ModelState` mirroring Rust: `{temporal_violations : List (String × String), blocked_flows : List (String × String), security_violations : List String}` (add more fields if Rust has them).
  - Define 5 inductive constraint types matching Rust, each with `description : String`.
  - Define `check_against` for each type using STRING CONTAINMENT (Lean's `String.contains` or a substring check).
  - Define `verify_constraints (constraints : List Constraint) (model : ModelState) : List Constraint` returning unsatisfied constraints (filter where check_against returns false).

Theorem `verify_constraints_sound`:
  If `verify_constraints constraints model = []`, then for every constraint `c ∈ constraints`,
  `c.check_against model = true`. The proof follows the filter logic.

Faithfulness checklist:
  [x] Lean has 5 constraint types matching Rust (Temporal, ResourceFlow, Security, Complexity, Liveness).
  [x] Each has `description : String`.
  [x] `check_against` uses string containment (NOT arithmetic comparison).
  [x] `ModelState` has temporal_violations, blocked_flows, security_violations (matching Rust).
  [x] No arithmetic constraint types (le/ge/eq are REMOVED — they don't exist in Rust).

Deliverable: Faithful ConstraintInference.lean. Build status: lake build PASS, zero sorry.
Worklog entry with faithfulness checklist.

Do NOT: Use arithmetic constraints. Use a value-lookup ModelState. Invent constraint types not in Rust.
```

---

### Subagent IVE-FAITH-5-D — Rewrite L1L3Collapse.lean to faithfully model `l1l3_collapse`

```
Task ID: IVE-FAITH-5-D
Wave: 5 (Faithfulness)
Branch: task/ive-faith-5-d

Background: The current L1L3Collapse.lean checks `type_hash = hash_string ir_type`
with a simple foldl hash. This does NOT match Rust.

Rust `src/ive/src/verification.rs::l1l3_collapse` (search "pub fn l1l3_collapse"):
  - Walks the SCG for ChannelOpen, ChannelSend, ChannelRecv nodes.
  - Tracks `channel_types: HashMap<String, String>` (per-channel element type).
  - For ChannelOpen: checks `ty.is_empty() || type_hash(ty) == 0` (empty/invalid type → failure).
  - For ChannelSend/Recv: checks type consistency against prior ChannelOpen/Send/Recv on same channel.
  - Counts `l1_checks_folded` and `l2_checks_folded`.
  - Returns `L1L3Collapse { l1_checks_folded, l2_checks_folded, failures : Vec<String> }`.
  - Uses `type_hash` function (search "fn type_hash" in verification.rs — it's the ACTUAL hash function, NOT a simple foldl).

Task: Rewrite L1L3Collapse.lean to faithfully model the Rust function.

Read (≤5 files): src/ive/src/verification.rs (search "pub fn l1l3_collapse" AND "fn type_hash"),
src/scg/src/node.rs (search "ChannelOpen\|ChannelSend\|ChannelRecv" — the SCG node payloads),
proof/PMT/IVE/Soundness/L1L3Collapse.lean (current, to replace).

Modify (≤1 file): proof/PMT/IVE/Soundness/L1L3Collapse.lean (rewrite).

Concrete faithful model:
  - Define `type_hash` in Lean to match Rust's `type_hash` EXACTLY (read the Rust function and replicate the algorithm — do NOT use a simple foldl).
  - Define `L1L3Collapse` structure: `{l1_checks_folded : Nat, l2_checks_folded : Nat, failures : List String}`.
  - Define a `ChannelTypeEvent` inductive: `open_event (chan : String) (ty : String) | send_event (chan : String) (ty : String) | recv_event (chan : String) (ty : String)`.
  - Define `l1l3_collapse (events : List ChannelTypeEvent) : L1L3Collapse`:
    - Track `channel_types : String → Option String` (per-channel type).
    - For each event: check `ty.is_empty || type_hash ty = 0` → failure.
    - For Open: check type consistency if channel already declared.
    - For Send/Recv: check type matches prior declaration.
    - Count folded checks.
  - Return the `L1L3Collapse` struct.

Theorem `l1l3_collapse_sound`:
  If `l1l3_collapse events` has `failures = []`, then:
  - Every event has a non-empty type with `type_hash ty ≠ 0`.
  - All events on the same channel agree on the element type.
  (This is the type-consistency guarantee Rust enforces.)

Faithfulness checklist:
  [x] Lean's `type_hash` matches Rust's `type_hash` EXACTLY (same algorithm).
  [x] Lean checks `ty.is_empty || type_hash ty = 0` (empty/invalid type).
  [x] Lean tracks per-channel types (String → Option String).
  [x] Lean checks type consistency across Open/Send/Recv.
  [x] Lean returns `L1L3Collapse` struct with counts + failures.
  [x] Lean does NOT check `type_hash = hash_string ir_type` (that was the old model).

Deliverable: Faithful L1L3Collapse.lean. Build status: lake build PASS, zero sorry.
Worklog entry with faithfulness checklist.

Do NOT: Use a different hash function. Check hash equality instead of type consistency. Return a List instead of the L1L3Collapse struct.
```

---

## Wave 5.5 — Build Verification Gate

Same as original Wave 0.5: `cargo build --release`, `cargo test`, `lake build`, sorry audit. Wave 6 does NOT start until all pass.

---

## Wave 6 — Major Gap Closures (5 subagents in parallel)

**Goal**: Close the 5 major gaps where Lean omits a key Rust check or models the wrong data structure.
**Prerequisites**: Wave 5 complete.
**Max parallel**: 5.

---

### Subagent IVE-FAITH-6-A — Rewrite InformationFlow.lean with all 4 FlowKind variants + implicit flow

```
Task ID: IVE-FAITH-6-A
Wave: 6 (Faithfulness)
Branch: task/ive-faith-6-a

Background: The current InformationFlow.lean models only 1 flow kind (Assign-like)
with `src_label` and `dst_label`. Rust has 4 FlowKind variants including Branch
(IMPLICIT FLOW). This is a SOUNDNESS HOLE — Lean accepts programs Rust rejects.

Rust `src/ive/src/information_flow.rs::verify_information_flow` (search "pub fn verify_information_flow"):
  - FlowKind::Assign { dst_vreg, dst_label, src_label }: checks `src_label.can_flow_to(dst_label)`.
  - FlowKind::BinOp { dst_vreg, dst_label, lhs_label, rhs_label }: computes
    `result_label = lhs_label.join(rhs_label)` (LUB), checks `result_label.can_flow_to(dst_label)`.
  - FlowKind::ChannelSend { channel_label, msg_label }: checks `msg_label.can_flow_to(channel_label)`.
  - FlowKind::Branch { cond_label, branch_var_labels }: IMPLICIT FLOW — for each var_label,
    checks `cond_label.can_flow_to(var_label)`.
  - SecurityLabel::can_flow_to: matches Lean's `flows_to`.
  - SecurityLabel::join: LUB (Public∧x=x; Internal∧Secret=Secret; etc.).
  - Sorts events by at_node.

Task: Rewrite InformationFlow.lean to faithfully model all 4 variants.

Read (≤4 files): src/ive/src/information_flow.rs (FULL — read FlowKind, SecurityLabel, verify_information_flow),
proof/PMT/IVE/Soundness/InformationFlow.lean (current, to replace).

Modify (≤1 file): proof/PMT/IVE/Soundness/InformationFlow.lean (rewrite).

Concrete faithful model:
  - Keep SecurityLabel (public, internal, secret, topsecret) and `flows_to` (already faithful).
  - Add `join : SecurityLabel → SecurityLabel → SecurityLabel` (LUB) matching Rust's `join`.
  - Define `FlowKind` inductive with 4 variants matching Rust:
    `| assign : Nat → SecurityLabel → SecurityLabel → FlowKind` (dst_vreg, dst_label, src_label)
    `| binop : Nat → SecurityLabel → SecurityLabel → SecurityLabel → FlowKind` (dst_vreg, dst_label, lhs_label, rhs_label)
    `| channel_send : SecurityLabel → SecurityLabel → FlowKind` (channel_label, msg_label)
    `| branch : SecurityLabel → List SecurityLabel → FlowKind` (cond_label, branch_var_labels)
  - Define `FlowEvent := {kind : FlowKind, at_node : Nat}`.
  - Define `verify_information_flow (events : List FlowEvent) : List FlowViolation`:
    - Sort by at_node (or assume sorted).
    - For each event, match on kind and apply the appropriate check.
  - `verify_information_flow_sound`: if output is [], every flow respects the lattice
    (including implicit flows via Branch).

Faithfulness checklist:
  [x] All 4 FlowKind variants modeled.
  [x] `join` (LUB) implemented and used in BinOp.
  [x] IMPLICIT FLOW (Branch) checks `cond_label.can_flow_to(var_label)` for each var.
  [x] ChannelSend checks `msg_label.can_flow_to(channel_label)`.
  [x] at_node sorting (or documented assumption).

Deliverable: Faithful InformationFlow.lean. Build status: lake build PASS, zero sorry.
Worklog entry with faithfulness checklist.

Do NOT: Model only 1 flow kind. Omit implicit flow (this is a soundness hole). Omit join.
```

---

### Subagent IVE-FAITH-6-B — Rewrite BorrowRegion.lean with path-sensitivity + all 7 event kinds

```
Task ID: IVE-FAITH-6-B
Wave: 6 (Faithfulness)
Branch: task/ive-faith-6-b

Background: The current BorrowRegion.lean has no path-sensitivity and only 5 event kinds.
Rust has 7 event kinds and full Branch/ElseStart/Join path-sensitivity with state snapshots/merges.

Rust `src/ive/src/borrow_region.rs::verify_linear_channels` (search "pub fn verify_linear_channels"):
  - 7 ChannelEventKind: Open, Use, Close, Branch, ElseStart, Join, FunctionExit.
  - ChannelEvent has `vreg : String` (NOT Nat) and `at_node`.
  - State: `HashMap<String, ChannelLifecycle>` where ChannelLifecycle = Open | Closed.
  - Branch: pushes a snapshot of state onto branch_stack.
  - ElseStart: captures then-branch state, restores pre-branch snapshot.
  - Join: pops frame, merges then/else states. Handle closed on one path but not the other → LEAK (violation).
  - FunctionExit: marks then-branch as returned.
  - Open on already-Open handle → re-init leak (violation).
  - Sorts by at_node.

Task: Rewrite BorrowRegion.lean to faithfully model the path-sensitive analysis.

Read (≤4 files): src/ive/src/borrow_region.rs (FULL — read verify_linear_channels, ChannelEventKind, ChannelLifecycle, BranchFrame),
proof/PMT/IVE/Soundness/BorrowRegion.lean (current, to replace).

Modify (≤1 file): proof/PMT/IVE/Soundness/BorrowRegion.lean (rewrite).

Concrete faithful model:
  - Define `ChannelLifecycle := open | closed`.
  - Define `ChannelEventKind` with 7 variants matching Rust.
  - Define `ChannelEvent := {vreg : String, kind : ChannelEventKind, at_node : Nat}`.
  - Define `BranchFrame := {snapshot : List (String × ChannelLifecycle), then_state : Option (List (String × ChannelLifecycle)), then_returned : Bool}`.
  - Model state as `List (String × ChannelLifecycle)` (a lookup list, modeling HashMap).
  - Define `verify_linear_channels (events : List ChannelEvent) : List LinearVerification`:
    - Sort by at_node.
    - Walk events, maintaining state + branch_stack.
    - Open: check re-init leak (already open → violation).
    - Use: check use-without-open, use-after-close.
    - Close: check close-without-open, double-close.
    - Branch: push snapshot.
    - ElseStart: capture then-state, restore snapshot.
    - Join: merge, detect leaks (closed on one path but not other).
    - FunctionExit: mark then_returned.
  - `verify_linear_channels_sound`: if output is [], no violations (all the above checks passed).

Faithfulness checklist:
  [x] 7 event kinds (Open, Use, Close, Branch, ElseStart, Join, FunctionExit).
  [x] vreg is String (not Nat).
  [x] Path-sensitivity: Branch/ElseStart/Join with state snapshots/merges.
  [x] Leak detection at Join (closed on one path but not other).
  [x] Re-init leak detection (Open on already-Open).
  [x] at_node sorting.

Deliverable: Faithful BorrowRegion.lean. Build status: lake build PASS, zero sorry.
Worklog entry with faithfulness checklist.

Do NOT: Use a flat prefix-scan. Omit Branch/Join/FunctionExit. Use Nat for vreg. Omit leak detection.
```

---

### Subagent IVE-FAITH-6-C — Rewrite SessionType.lean with per-vreg tracking + Open event

```
Task ID: IVE-FAITH-6-C
Wave: 6 (Faithfulness)
Branch: task/ive-faith-6-c

Background: The current SessionType.lean uses a single global session type and has no Open event.
Rust tracks per-vreg session state.

Rust `src/ive/src/session_type.rs::verify_session_types` (search "pub fn verify_session_types"):
  - SessionEventKind: Open { vreg, session_type }, Send { vreg, msg_type }, Recv { vreg, expected_type }, Close { vreg }.
  - State: `HashMap<u32, SessionType>` (per-vreg session type).
  - Open: initializes the vreg's session type. Re-open on already-open vreg → violation.
  - Send: checks state is `Send(expected_t, cont)`, checks `expected_t == msg_type`, advances to `cont`.
  - Recv: checks state is `Recv(expected_t, cont)`, checks `expected_t == expected_type`, advances.
  - Close: checks state is `End`, removes vreg.
  - SessionType: End | Send(String, Box<SessionType>) | Recv(String, Box<SessionType>).
  - Sorts by at_node.

Task: Rewrite SessionType.lean to faithfully model per-vreg tracking.

Read (≤4 files): src/ive/src/session_type.rs (FULL — read verify_session_types, SessionEventKind, SessionType),
proof/PMT/IVE/Soundness/SessionType.lean (current, to replace).

Modify (≤1 file): proof/PMT/IVE/Soundness/SessionType.lean (rewrite).

Concrete faithful model:
  - Keep SessionType inductive (end, send τ s, recv τ s) — already matches Rust.
  - Define SessionEventKind with 4 variants: `open_event (vreg : Nat) (st : SessionType)`, `send_event (vreg : Nat) (msg_type : String)`, `recv_event (vreg : Nat) (expected_type : String)`, `close_event (vreg : Nat)`.
  - Model state as `List (Nat × SessionType)` (per-vreg, modeling HashMap).
  - `verify_session_types (events : List SessionEvent) : List SessionViolation`:
    - Sort by at_node.
    - Walk events, maintaining per-vreg state.
    - Open: check re-open, initialize.
    - Send/Recv: check session type matches, advance.
    - Close: check End, remove.
  - `verify_session_types_sound`: if output is [], no violations.

Faithfulness checklist:
  [x] Per-vreg session state (not single global).
  [x] Open event with session_type parameter.
  [x] vreg field on all events (Nat, matching Rust's u32).
  [x] Re-open detection.
  [x] Send/Recv advance the session type.
  [x] Close checks End.

Deliverable: Faithful SessionType.lean. Build status: lake build PASS, zero sorry.
Worklog entry with faithfulness checklist.

Do NOT: Use a single global session type. Omit the Open event. Omit vreg.
```

---

### Subagent IVE-FAITH-6-D — Rewrite ArenaBounds.lean with SCG walk + Option capacity

```
Task ID: IVE-FAITH-6-D
Wave: 6 (Faithfulness)
Branch: task/ive-faith-6-d

Background: The current ArenaBounds.lean takes a pre-extracted list of ops and always checks capacity.
Rust walks the SCG and uses Option<u64> for capacity (skips check when None).

Rust `src/ive/src/arena_bounds.rs::verify_arena_bounds` (search "pub fn verify_arena_bounds"):
  - Signature: `(pmt_layouts: &HashMap<String, LayoutSpec>, scg: &SCG)`.
  - Walks SCG for ArenaNew and ArenaAlloc nodes.
  - Tracks `arena_capacity: HashMap<u32, Option<u64>>` and `arena_used: HashMap<u32, u64>`.
  - ArenaNew: records capacity as None (unknown, symbolic vreg), used as 0.
  - ArenaAlloc: looks up layout, checks total_size > 0, checks overflow (checked_add),
    checks capacity ONLY if known (Some), propagates state.
  - Uses u64 with checked_add (overflow → reject).

Task: Rewrite ArenaBounds.lean to faithfully model the SCG walk.

Read (≤5 files): src/ive/src/arena_bounds.rs (FULL),
src/scg/src/node.rs (search "ArenaNew\|ArenaAlloc"),
proof/PMT/IVE/Soundness/ArenaBounds.lean (current, to replace).

Modify (≤1 file): proof/PMT/IVE/Soundness/ArenaBounds.lean (rewrite).

Concrete faithful model:
  - Define `ArenaNode` inductive: `| arena_new (result_vreg : Nat) (capacity_vreg : Nat) | arena_alloc (arena_vreg : Nat) (layout_name : String) (result_arena_vreg : Nat) (result_state_vreg : Nat)`.
  - Model the SCG walk as a fold over `List ArenaNode` (in program order).
  - Track state: `capacity_map : Nat → Option (Option Nat)` (vreg → Option capacity), `used_map : Nat → Nat` (vreg → used).
    - Outer Option: vreg not yet seen. Inner Option: capacity known (Some) or unknown (None).
  - ArenaNew: insert (result_vreg → None) [capacity unknown], (result_vreg → 0) [used].
  - ArenaAlloc:
    - Look up layout by name (not-found → violation).
    - Check total_size > 0 (zero → violation).
    - Get used and capacity_opt for arena_vreg.
    - Check overflow: `used + total_size` must not overflow u64 (use saturating/checked add).
    - If capacity is Some cap: check `used + total_size ≤ cap`.
    - If capacity is None: SKIP capacity check (but still check overflow + total_size > 0).
    - Propagate state to result_arena_vreg.
  - `verify_arena_bounds_sound`: if all valid, every alloc has layout-found + total_size > 0 + no overflow + (if capacity known) fits.

Faithfulness checklist:
  [x] Lean walks a list of ArenaNode (modeling SCG walk), not pre-extracted ops.
  [x] Capacity is Option (Option Nat), matching Rust's Option<u64>.
  [x] Capacity check SKIPPED when None (matching Rust).
  [x] Overflow check (saturating/checked add, modeling u64).
  [x] State propagation from arena_vreg to result_arena_vreg.
  [x] used accumulation across allocs on the same arena lineage.

Deliverable: Faithful ArenaBounds.lean. Build status: lake build PASS, zero sorry.
Worklog entry with faithfulness checklist.

Do NOT: Take pre-extracted ops. Always check capacity. Use plain Nat without overflow.
```

---

### Subagent IVE-FAITH-6-E — Rewrite StateReads.lean + StateWrites.lean with field-name lookup

```
Task ID: IVE-FAITH-6-E
Wave: 6 (Faithfulness)
Branch: task/ive-faith-6-e

Background: The current StateReads.lean and StateWrites.lean carry `f : PMT.Field`
(offset+size) and look up fields by (offset, size). Rust carries field NAMES
and looks up by name. The name→field resolution (where "field not found" occurs)
is unmodeled.

Rust `src/ive/src/state_read.rs::verify_state_reads`:
  - reads: `&[(String, String, String)]` — (var, field_name, expected_type).
  - Finds field by NAME: `layout.fields.iter().find(|f| f.name == *field)`.
  - FieldInfo has {name, offset, size, type_name}.

Rust `src/ive/src/state_write.rs::verify_state_writes`:
  - StateWriteOp has {var_name, field_name, value_type, after_consume}.
  - Same name-based field lookup.

Task: Rewrite StateReads.lean and StateWrites.lean to use field-name lookup.

Read (≤5 files): src/ive/src/state_read.rs (FULL), src/ive/src/state_write.rs (FULL),
proof/PMT/IVE/Soundness/StateReads.lean (current), StateWrites.lean (current),
proof/PMT/Basic.lean (Field, Layout).

Modify (≤2 files): proof/PMT/IVE/Soundness/StateReads.lean (rewrite),
proof/PMT/IVE/Soundness/StateWrites.lean (rewrite).

Concrete faithful model for BOTH modules:
  - Define `FieldInfo := {name : String, offset : Nat, size : Nat, type_name : String}` (matching Rust).
  - Define `LayoutInfo := {name : String, total_size : Nat, fields : List FieldInfo}` (matching Rust).
  - Define `LayoutRegistry := String → Option LayoutInfo` (models HashMap<String, LayoutInfo>).
  - StateRead: `{var : String, field_name : String, expected_type : String}` (NO Field struct).
  - StateWrite: `{var : String, field_name : String, value_type : String, after_consume : Bool}` (NO Field struct).
  - `fieldInLayout` finds field by NAME: `layout.fields.any (fun f => f.name == field_name)` (using String BEq).
  - `fieldInBounds` uses the FOUND field's offset+size: `fi.offset + fi.size ≤ layout.total_size`.
  - `fieldTypeMatches` checks `fi.type_name == expected_type` (direct string equality, no separate field_types map).
  - The `field_types` parameter is REMOVED — type_name is part of FieldInfo (matching Rust).

Faithfulness checklist (for BOTH StateReads and StateWrites):
  [x] StateRead/StateWrite carry field_name : String (not Field struct).
  [x] Field lookup is by NAME (not by offset+size).
  [x] FieldInfo has {name, offset, size, type_name} (4 fields, matching Rust).
  [x] type_name is part of FieldInfo (no separate field_types map).
  [x] "Field not found" failure mode modeled (name not in layout.fields).

Deliverable: Faithful StateReads.lean + StateWrites.lean. Build status: lake build PASS, zero sorry.
Worklog entry with faithfulness checklist.

Do NOT: Carry Field struct (use field_name string). Look up by (offset, size). Use separate field_types map.
```

---

## Wave 6.5 — Build Verification Gate

Same as Wave 5.5. Wave 7 does NOT start until all pass.

---

## Wave 7 — LayoutConsistency + Composition Update + Pillar Re-proof (3 subagents)

**Goal**: Close the LayoutConsistency gap, update Composition.lean to use the new faithful models, and re-prove `ive_pillar_sound` with faithful models.
**Prerequisites**: Waves 5-6 complete.
**Max parallel**: 3 (LayoutConsistency is independent; Composition and Pillar depend on all prior waves).

---

### Subagent IVE-FAITH-7-A — Rewrite LayoutConsistency.lean with C-style alignment re-derivation

```
Task ID: IVE-FAITH-7-A
Wave: 7 (Faithfulness)
Branch: task/ive-faith-7-a

Background: The current LayoutConsistency.lean checks wf_layout_bool. Rust re-derives
offsets using C-style alignment rules and compares, plus checks field-list consistency by name.

Rust `src/ive/src/verification.rs::verify_layout_consistency` (search "pub fn verify_layout_consistency"):
  - Calls `rederive_layout(&spec.fields)` which computes offsets using C-style alignment
    (fields aligned to their size, padding inserted).
  - Compares derived_total vs spec.total_size, derived_offset vs field.offset, derived_size vs field.size.
  - Returns Vec<String> of mismatch descriptions.

Rust `verify_layout_field_list_consistency` (search "pub fn verify_layout_field_list_consistency"):
  - Takes TWO layout maps: parser_layouts and ivederived_layouts.
  - Compares field LISTS by NAME: every IVE-derived field name must be in the parser-provided layout.
  - Checks field count.

Task: Rewrite LayoutConsistency.lean to faithfully model both functions.

Read (≤4 files): src/ive/src/verification.rs (search "verify_layout_consistency" AND "rederive_layout" AND "verify_layout_field_list_consistency"),
proof/PMT/IVE/Soundness/LayoutConsistency.lean (current, to replace).

Modify (≤1 file): proof/PMT/IVE/Soundness/LayoutConsistency.lean (rewrite).

Concrete faithful model:
  - Define `rederive_layout (fields : List FieldInfo) : (Nat × List (Nat × Nat))` — computes
    (total_size, [(offset, size), …]) using C-style alignment (read the Rust `rederive_layout` function
    and replicate the alignment algorithm exactly).
  - `verify_layout_consistency (layouts : LayoutRegistry) : List String` — for each layout,
    call rederive_layout, compare, collect mismatches.
  - `verify_layout_field_list_consistency (parser_layouts ivederived_layouts : LayoutRegistry) : List String` —
    compare field lists by name.
  - `verify_layout_consistency_sound`: if output is [], all layouts are consistent.
  - `verify_layout_field_list_consistency_sound`: if output is [], all field lists match.

Faithfulness checklist:
  [x] rederive_layout uses C-style alignment (matching Rust's algorithm).
  [x] verify_layout_consistency compares derived vs provided offsets/sizes/total.
  [x] verify_layout_field_list_consistency takes TWO layout maps and compares by NAME.
  [x] Field count check.
  [x] Returns List String (mismatch descriptions), matching Rust's Vec<String>.

Deliverable: Faithful LayoutConsistency.lean. Build status: lake build PASS, zero sorry.
Worklog entry with faithfulness checklist.

Do NOT: Use wf_layout_bool. Use a single layout map for field_list_consistency. Omit alignment.
```

---

### Subagent IVE-FAITH-7-B — Update Composition.lean to use faithful models

```
Task ID: IVE-FAITH-7-B
Wave: 7 (Faithfulness)
Branch: task/ive-faith-7-b

Background: After Waves 5-6, the per-rule Lean models have new signatures.
Composition.lean must be updated to use them.

Task: Update Composition.lean (FullyVerified structure, fully_verified_implies_pmt_invariants,
fully_verified_no_memory_safety_traps) to use the new faithful model signatures.

Read (≤6 files): proof/PMT/IVE/Soundness/Composition.lean (current),
proof/PMT/IVE/Soundness/Transform.lean (post-5-A), StateReads.lean (post-6-E),
StateWrites.lean (post-6-E), ArenaBounds.lean (post-6-D), BorrowRegion.lean (post-6-B).

Modify (≤1 file): proof/PMT/IVE/Soundness/Composition.lean (update).

Concrete changes:
  - Update FullyVerified to use the new signatures (layout registries instead of env functions,
    field_name strings instead of Field structs, etc.).
  - Update fully_verified_implies_pmt_invariants to delegate to the new soundness theorems.
  - The conclusion should match what the faithful Rust checks guarantee (e.g., Transform no
    longer concludes WF_Layout; it concludes layout-exists + kind-correct).

Faithfulness checklist:
  [x] FullyVerified uses faithful model signatures.
  [x] Composition theorem delegates to faithful per-rule theorems.
  [x] No leftover references to old (unfaithful) model structures.

Deliverable: Updated Composition.lean. Build status: lake build PASS, zero sorry.
Worklog entry.

Do NOT: Keep old signatures. Reference WF_Layout in Transform's conclusion (Rust doesn't check it).
```

---

### Subagent IVE-FAITH-7-C — Re-prove ive_pillar_sound with faithful models

```
Task ID: IVE-FAITH-7-C
Wave: 7 (Faithfulness)
Branch: task/ive-faith-7-c

Background: After Waves 5-7, all per-rule models are faithful. The pillar theorem
must be re-proven to combine them.

Task: Rewrite PillarSoundness.lean to use the faithful models and re-prove ive_pillar_sound.

Read (≤6 files): proof/PMT/IVE/PillarSoundness.lean (current),
Composition.lean (post-7-B), and the 11 faithful soundness modules.

Modify (≤1 file): proof/PMT/IVE/PillarSoundness.lean (rewrite).

Concrete changes:
  - Update IveAccepted to use faithful model acceptance predicates.
  - Update ive_pillar_sound's conclusion to match what the faithful Rust IVE guarantees.
  - The conclusion should be: if Rust IVE accepts (all 12 rules pass), then:
    (1) All reads access registered, in-bounds, type-matched fields (by name).
    (2) All writes are to live, registered, in-bounds, type-matched fields.
    (3) All transforms have existing layouts + correct kind inference.
    (4) All arena allocs have existing layouts + total_size > 0 + no overflow + fits (if capacity known).
    (5) All channel operations are linear (with path-sensitivity + leak detection).
    (6) All information flows respect the lattice (including implicit flows).
    (7) All session types are respected (per-vreg).
    (8) All dependent transforms satisfy Presburger bounds.
    (9) All L1L3 checks have type consistency.
    (10) All constraints are satisfied (string-description-based).
    (11) All layouts are consistent (C-style alignment + field-list by name).
  - NO WF_Layout conclusions where Rust doesn't check WF_Layout.

Faithfulness checklist:
  [x] IveAccepted uses faithful model predicates.
  [x] ive_pillar_sound's conclusion matches Rust's actual guarantees.
  [x] No WF_Layout conclusions for verifiers that don't check it (Transform, DependentTransform).
  [x] Implicit flow in the conclusion (from faithful InformationFlow).
  [x] Path-sensitive leak detection in the conclusion (from faithful BorrowRegion).

Deliverable: Re-proven ive_pillar_sound (sorry-free). Build status: lake build PASS, zero sorry.
Worklog entry.

Do NOT: Include conclusions not supported by the faithful Rust checks. Omit implicit flow. Omit leak detection.
```

---

## Wave 8 — Final Faithfulness Audit (1 subagent)

**Goal**: Independent audit confirming all Lean models faithfully mirror Rust.
**Prerequisites**: Wave 7 complete.

---

### Subagent IVE-FAITH-8-A — Final faithfulness audit

```
Task ID: IVE-FAITH-8-A
Wave: 8 (Faithfulness)
Branch: task/ive-faith-8-a

Background: All gap closures are merged. Need a final audit confirming faithfulness.

Task: For each of the 11 soundness modules, produce a line-by-line faithfulness
checklist comparing Lean vs Rust. Update FAITHFULNESS_AUDIT.md to mark each gap
as CLOSED or STILL-OPEN. Run clean build + sorry audit.

Read (≤3 files per module, but read all 11 modules + their Rust counterparts).

Modify (≤2 files): proof/FAITHFULNESS_AUDIT.md (update gap statuses),
proof/AUDIT_IVE.md (update for re-audit).

Deliverable: Updated FAITHFULNESS_AUDIT.md with all gaps marked CLOSED (or
documented why still open). Clean lake build PASS. Sorry audit 0.
Worklog entry.

Do NOT: Skip any module. Mark a gap closed without line-by-line verification.
```

---

## Worklog Template (IVE-Faith subagents)

```markdown
---
Task ID: IVE-FAITH-<wave>-<letter>
Agent: <subagent name or ID>
Wave: <wave> (Faithfulness)
Task: <one-line scope summary>

Work Log:
- <step 1>
- <step 2>

Faithfulness Checklist:
- [x] <check 1 — line-by-line Rust vs Lean comparison>
- [x] <check 2>
- ...

Stage Summary:
- Git branch: task/ive-faith-<wave>-<letter>
- Files read: <list>
- Files modified: <list>
- Files created: <list>
- Lines added/removed: <counts>
- Build status: lake build [PASS/FAIL] (zero sorry warnings: YES/NO), cargo build --release [PASS/FAIL], cargo test [PASS/FAIL/N/A]
- Theorems stated: <count>
- Theorems proven (sorry-free): <count>
- Axioms used: <list, must be empty unless explicitly justified>
- Faithfulness gaps closed: <list of gap IDs from FAITHFULNESS_AUDIT.md>
- Faithfulness gaps still open: <list, must be empty for this task to be considered complete>
- Outstanding questions / blockers: <list>
- Next-wave dependencies unblocked: <list of Task IDs>
```

---

## IVE-Faith Orchestrator Self-Check (After All Waves Complete)

Run this checklist before declaring faithful verification complete:

- [ ] Wave 5 complete; 4 critical gaps closed (Transform, DependentTransform, ConstraintInference, L1L3Collapse).
- [ ] Wave 5.5 gate passed.
- [ ] Wave 6 complete; 5 major gaps closed (InformationFlow, BorrowRegion, SessionType, ArenaBounds, StateReads/Writes).
- [ ] Wave 6.5 gate passed.
- [ ] Wave 7 complete; LayoutConsistency gap closed, Composition updated, ive_pillar_sound re-proven.
- [ ] Wave 8 complete; all gaps in FAITHFULNESS_AUDIT.md marked CLOSED.
- [ ] Final `lake build` from clean passes with zero warnings.
- [ ] Final `grep -rn "sorry\|admit" proof/PMT/IVE/ | wc -l` = 0.
- [ ] Every module's faithfulness checklist verified by the audit.

When all checkboxes pass: the IVE pillar is 100% mathematically verified **and** the Lean models faithfully mirror the Rust IVE. Post a `IVE FAITHFUL VERIFICATION COMPLETE` entry in the worklog.

---

## What "Faithful 100% IVE Verified" Means After This Document

- All 12 production IVE rules have Lean soundness proofs that **match the Rust implementations exactly** — same parameters, same checks, same failure modes, same data structures.
- `ive_pillar_sound` proves: if the Rust IVE accepts a program, then the faithful Lean `IveAccepted` hypothesis holds, and the program satisfies all memory-safety invariants that the Rust IVE actually enforces.
- No abstraction gaps: the Lean model is a **bit-faithful rendering** of the Rust code, not a simplification.
- The parity test (1,589 fixtures × 12 rules) validates that the Rust IVE and the Lean models agree on all test cases.

**Residual TCB** (unchanged from the original spec):
- Parser, AST→SCG bridge, codegen SCG→IR lowering, optimizer, regalloc, backend instruction selection, ELF/Wasm emission, OS interface, hardware.

The IVE pillar theorem is conditional on the residual TCB. The IVE pillar itself is 100% verified with **faithful models** — no undischarged hypotheses within IVE scope, no abstraction gaps between Lean and Rust.
