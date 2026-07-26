# IVE IR Divergence Plan — Wave 0 (Task IVE-0-A)

**Status:** PLAN (no code changes — planning task only). **Recommended option: (c) document as known gap with workaround**, with a differential conformance test deferred to Wave 1.

---

## 1. Background

IVE runs its soundness verifiers (`proof/PMT/IVE/Soundness/{Transform,StateReads,StateWrites,Composition}.lean` and the Rust `src/ive/**/*.rs` verifiers) on the **semantic SCG** (`vuma-scg` crate; `src/scg/src/node.rs`). The emitted binary, however, is produced from a **different IR** — the **codegen SCG** (`vuma_codegen::Scg`; `src/codegen/src/scg_to_ir.rs`). The two are bridged from a common parser AST but by **two independent bridges**:

- AST → semantic SCG: `vuma_parser::AstToScg` (in `src/parser/src/to_scg.rs`).
- AST → codegen SCG: `vuma::pipeline::bridge_ast_to_codegen_scg` (in `src/pipeline.rs:9539`).

The old **semantic-SCG → codegen-SCG** bridge (`bridge_scg_to_codegen*`) was **abandoned** — see the `NOTE: The canonical pipeline now uses the DIRECT AST→codegen SCG bridge ...` comment at `src/pipeline.rs:6084-6090` (and the two parallel occurrences at `:7596-7602` and `:8032-8037`): *"This avoids the segfaults / infinite loops that the old `bridge_scg_to_codegen*` path produced."* As a result, IVE verifies a **different IR than the binary producer**.

---

## 2. Concrete AST Construct — `transform a -> b`

Consider the following VUMA source (a parser/transform pattern that appears across `tests/gold_standard/pmt_*`):

```vuma
layout Raw    { bytes:  [u8; 16] }
layout Parsed { tag:    u32,
                payload:[u8; 12] }
transform parse : Raw -> Parsed

fn main() {
  let raw   = state_new(Raw);     // Expr::StateInit { layout_name: "Raw" }
  let p     = parse(raw);        // Stmt::TransformCall { transform_name: "parse", arg, dst }
  let tag   = p.tag;             // Expr::StateRead { state, layout_name, field: "tag" }
  consume(p);                    // Stmt::ForeignConsume-marked extern call
}
```

---

## 3. Representation in BOTH SCGs (divergence highlighted)

| AST construct | Semantic SCG (IVE input — `vuma-scg`) | Codegen SCG (binary producer — `vuma_codegen::Scg`) | **Divergence** |
|---|---|---|---|
| `let raw = state_new(Raw)` | `NodePayload::StateInit(StateInitNode { layout_name: "Raw", result_vreg: r0 })` — typed-state node; carries the layout name + a typed vreg. (See `src/scg/src/node.rs:811`.) | `ScgStatement::Allocation(AllocationNode::Stack { size: total_size("Raw"), align, dst })` — untyped stack-alloc. Size computed by a **separate** layout-size path (`pipeline.rs::build_alloc_sizes` + `build_pmt_layout_specs` at `:9472` / `:10315`). | **Codegen SCG loses the layout_name entirely.** Size comes from a different computation path than the one IVE's verifiers reason about. Layout-size drift between `to_scg.rs::layout_total_size` (semantic side) and `pipeline.rs::build_pmt_layout_specs` (codegen side) → silent miscompilation invisible to IVE. |
| `let p = parse(raw)` (`Stmt::TransformCall`) | `NodeType::Computation` + `ComputationNode { callee: "parse", args: [r0] }` — synthetic call reusing `emit_call_nodes` (`to_scg.rs:2345`). The `StateTransform` payload (`NodeType::StateTransform`/`StateTransformNode`) **exists** in the semantic SCG (`node.rs:860`) but the AST-to-semantic-SCG bridge **does not emit it** for user-declared `transform`s; it emits a generic `Computation` node instead. | `ScgStatement::Call(CallNode { callee: "parse", args: [Var("raw")], dst: "p", ... })` — generic call. `ScgFunction::var_types` (`scg_to_ir.rs:252`) carries only primitive `ScgType` (I8/I32/.../Channel<T>) — **no `State<Raw>` typing** propagates to the call args. | IVE reasons about a typed state-vreg `r0` of layout `Raw`; codegen emits a call to `parse` with an opaque byte-buffer vreg. The "transform" semantic (raw bytes → struct, equal total size, in-place reinterpretation) is **not represented in either SCG as a typed node** — both sides lower it to a generic call. IVE's `Transform.lean` soundness theorem must assume the call preserves the buffer identity by side-channel reasoning, not by inspecting the SCG. |
| `let tag = p.tag` (field access on `State<Parsed>`) | `NodePayload::StateRead(StateReadNode { state_vreg, layout_name: "Parsed", field_name: "tag", result_vreg })` — typed-state field access; IVE verifies the field offset against the BD `LayoutRegistry`. (`node.rs:825`.) | `ScgStatement::StructAccess(StructAccessNode::Load { ptr, field_offset: 0, field_ty: U32 })` — hardcoded byte offset, no layout name. (`scg_to_ir.rs:681-704`.) | IVE verifies "offset 0 is a valid `u32` field of layout `Parsed`"; codegen emits "load 4 bytes at offset 0". If the two layout-size tables disagree on field offsets (e.g. due to padding rules differing between `to_scg.rs` and `pipeline.rs`), the load reads the wrong bytes — and IVE has no way to catch it because it never sees the codegen SCG. |
| `consume(p)` (`#[foreign_consume]`) | `NodePayload::ForeignConsume(ForeignConsumeNode { input_vreg, layout_name: "Parsed" })` — linearity marker; subsequent reads/writes to `input_vreg` flagged as linearity errors. (`node.rs:880`.) | `ScgStatement::ForeignConsume(ForeignConsumeStmt { state_var: "p", layout_name: "Parsed" })` — marker statement that emits **no IR instruction** (`scg_to_ir.rs:1666-1671`). | Codegen SCG's marker is purely diagnostic — emits no IR. The linearity invariant lives in IVE only. If the codegen SCG bridge drops the marker (e.g. for an extern declared without `#[foreign_consume]`), the binary is unchanged but IVE has silently lost the consume marker — and there is no cross-check that both bridges emit the marker for the same externs. |

### Summary of divergence

1. **Typed-state payloads lost.** The semantic SCG has 5 dedicated typed-state payloads (`StateInit`, `StateRead`, `StateWrite`, `StateTransform`, `ForeignConsume`). The codegen SCG has **zero** — it lowers them to generic `AllocationNode`/`StructAccess`/`CallNode`/marker `ForeignConsumeStmt`.
2. **Two independent layout registries.** `to_scg.rs::layout_total_size` (semantic) vs `pipeline.rs::build_pmt_layout_specs` (codegen). Same input → two bridges → two size tables → drift is silent.
3. **Two independent call-lowering paths.** `Stmt::TransformCall` lowers via `emit_call_nodes` on the semantic side and via `bridge_ast_to_codegen_scg` on the codegen side; the "transform" semantic is captured in **neither** as a typed node — both emit generic calls.
4. **No conformance check.** Nothing asserts the two bridges agree on the IVE-relevant subset (layout names + sizes + transform callees + state-var declarations + foreign-consume markers).

---

## 4. Recommended Option: **(c) document as known gap with workaround**

**Justification.** Option (c) is the only choice consistent with this task's hard constraints (`≤2` modified files; no edits to either SCG type; no cargo/lake builds). Option (a) — re-establishing the semantic-SCG → codegen-SCG bridge — was **already tried and abandoned** for concrete cause (segfaults + infinite loops; see `pipeline.rs:6084-6090`), and re-introducing it would require modifying both `src/scg/src/node.rs` and `src/codegen/src/scg_to_ir.rs` plus re-running the full pipeline test suite (forbidden at Wave 0). Option (b) — moving IVE to the codegen SCG — would **lose the typed-state payloads** that are IVE's entire value-add: the codegen SCG has no `StateInit`/`StateRead`/`StateWrite`/`StateTransform`/`ForeignConsume` payloads, so moving IVE there would either require re-adding them to the codegen SCG (re-diverging in a new direction) or rewriting all four IVE soundness theorems in `proof/PMT/IVE/Soundness/*.lean` against untyped allocations (defeating the purpose and exceeding Wave 0's scope). Option (c) instead **documents the gap** (this file + a sub-paragraph appended to `docs/caveats.md`'s "Two parallel SCG IRs — OPEN" entry) and proposes a **differential conformance test** as the Wave-1 workaround: build both SCGs from the same AST, assert agreement on the IVE-relevant subset (layout names + sizes + transform callees + state-var declarations + foreign-consume markers). The test is a tripwire — it catches future divergence between the two bridges without forcing a refactor that Wave 0 cannot afford. Full unification (choosing between (a) and (b)) is deferred to post-Wave-3, once IVE's typed-state soundness theorems for the new state model are complete.

---

## 5. Next Steps

1. **Wave 0 (this task, IVE-0-A):** Plan approved; this file created; `docs/caveats.md`'s "Two parallel SCG IRs — OPEN" row gets an appended sub-paragraph referencing this plan (section name `## 0.5. IVE IR Divergence Plan — Wave 0` to avoid collision with PMT's existing `## 0.` section).
2. **Wave 1 (IVE-1-x, deferred):** Implement `tests/ive_scg_conformance_test.rs` — a differential conformance test that, for every fixture under `tests/gold_standard/`, builds (i) the semantic SCG via `vuma_parser::AstToScg` and (ii) the codegen SCG via `vuma::pipeline::bridge_ast_to_codegen_scg`, then asserts agreement on: layout names + total sizes; transform callee names; state-var declarations (name + layout); foreign-consume markers. The test is `cargo test`-only — no production code changes.
3. **Wave 1 (concurrent):** Wire the conformance test into CI as a **hard gate** (failure = bug in either bridge).
4. **Wave 2:** Audit the two layout-size computation paths (`to_scg.rs::layout_total_size` vs `pipeline.rs::build_pmt_layout_specs`); unify if possible without re-introducing the segfaults that killed the old bridge.
5. **Post-Wave-3:** Re-evaluate (a) re-establishing the bridge with the conformance test as a guard, vs (b) moving IVE to the codegen SCG with typed-state payloads re-added — defer until IVE's soundness theorems for the new state model are complete.

---

## 6. File List (Wave-1 implementation — NOT this task)

| File | Change | Approx LOC |
|---|---|---|
| `tests/ive_scg_conformance_test.rs` | NEW — differential conformance test | ~250 |
| `src/pipeline.rs` | MODIFY — extract the shared layout-registry builder so the test can call it without re-parsing | ~50 |
| `docs/caveats.md` | MODIFY — flip "Two parallel SCG IRs — OPEN" → "MITIGATED (conformance test)" once the test lands | ~3 |
| `src/scg/src/node.rs` | **NOT MODIFIED** (per task rule) | 0 |
| `src/codegen/src/scg_to_ir.rs` | **NOT MODIFIED** (per task rule) | 0 |

---

## 7. Effort Estimate

| Phase | Engineer-days (1 eng, `--jobs 1` builds) |
|---|---|
| Wave 0 (this task — plan + caveats update) | 0.5 |
| Wave 1 (conformance test + pipeline refactor + CI wiring) | 2–3 |
| Wave 2 (layout-size unification audit) | 3–5 |
| Post-Wave-3 (full bridge re-establish OR IVE migration) | 10–15 (deferred; out of Wave-0 scope) |
| **Total to mitigation (Wave 0 + Wave 1)** | **~3 engineer-days** |

---

## 8. Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Conformance test fails to detect drift (false negative) | Medium | High — silent miscompilation persists | Include ALL `tests/gold_standard/**` fixtures; add property-based fuzz fixture if budget allows |
| Conformance test too strict (false positive) | Low | Low — slows CI | Start with subset assertion (layout names + sizes only); expand per-fixture |
| Layout-size tables drift between `to_scg.rs` and `pipeline.rs` during Waves 1–2 | Medium | High — exactly the bug this plan exists to catch | Conformance test is the tripwire; Wave 2 audit is the fix |
| Old `bridge_scg_to_codegen*` segfaults resurface if (a) is later chosen | Medium | High | Conformance test must run against the OLD bridge first (regression), then against the new one |

---

*End of plan. No code changes made. Branch `task/ive-0-a` ready for orchestrator merge.*
