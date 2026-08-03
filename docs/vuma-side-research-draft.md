# VUMA-Side Research Draft

**Status**: Rough draft. Supersedes nothing — extends
`docs/vuma-side-problem-catalog.md` with the findings of four parallel
deep-audit subagents (Wave A, 2026-08-01).

**Scope**: VUMA-core only (parser, AST, SCG, IVE, IR, codegen backends,
runtime, capability model, Lean proofs, test suite, dependency manifest,
build/CI infrastructure). The WOMB UI-engine layer and the VELL
UX-language layer remain out of scope.

**Method**: Four subagents audited four disjoint slices of the codebase
in parallel, each writing a structured report:

- A-1 — Parser + SCG (`src/parser/`, `src/scg/`) → `research/A-1-parser-scg.md`
- A-2 — IR + Codegen + Backends (`src/codegen/`) → `research/A-2-ir-codegen.md`
- A-3 — IVE + Lean proofs + Capability (`src/ive/`, `proof/`, `capability.rs`) → `research/A-3-ive-proofs-capability.md`
- A-4 — Pipeline + Runtime + Tests + Deps (`src/pipeline.rs`, `tests/`, `test_results/`, `Cargo.toml`, `.github/`) → `research/A-4-pipeline-runtime-tests-deps.md`

This draft consolidates those reports, corrects the original catalog
where the subagents refuted its claims, and groups the now-49-entry
problem space (18 original + 35 newly surfaced, minus duplicates and
redundancies) into thematic clusters.

---

## 1. Corrections to the original catalog

The original `vuma-side-problem-catalog.md` was accurate on every P0
claim it made. The subagents verified each one against the source. Five
claims need correction:

### 1.1 V-37 — REFUTED (subsumed by V-03)

**Catalog claim**: "`build_pmt_layout_specs` does not propagate
alignment back into the size table. After V-03 is fixed, the size table
must also include trailing padding to `max_align`."

**Subagent A-4 finding**: Trailing padding to `max_align` IS computed
at `src/pipeline.rs:6741–6744`:

```rust
// pad total_size to max_align
if max_align > 1 && !offset.is_multiple_of(max_align) {
    offset = (offset + max_align - 1) & !(max_align - 1);
}
```

The real gap is that `build_pmt_layout_specs` runs in a single pass and
calls the legacy `bridge_type_size` (which returns 8 for nested layout
names) — so it can't compute correct sizes for nested layouts even
though the padding logic is correct. This is exactly V-03. V-37 adds
nothing and should be deleted.

### 1.2 V-13 — PARTIALLY REFUTED (AArch64 SIMD weaker than claimed)

**Catalog claim**: "AArch64 has NEON encoders … both backends support
`{Add, Sub, Mul} × {i32, i64}`."

**Subagent A-2 finding**: AArch64 has only the `4S` (4×i32) form for
`VectorOp`. There is no `2D` (2×i64) encoder. The catalog over-stated
AArch64 SIMD coverage.

### 1.3 V-39 weakest-backend list — PARTIALLY REFUTED

**Catalog claim**: "Weakest: `m68k`, `sparc64`, `hppa`, `alpha`,
`x86_32`."

**Subagent A-4 finding**: The actual ranking (from `test_results/summary.json`)
is:

| Rank | Backend    | Pass rate | Failure count |
|------|------------|-----------|---------------|
| 19   | m68k       | 80.47%    | 309           |
| 18   | ppc64      | 81.29%    | 295           |
| 17   | ppc64le    | 81.29%    | 295           |
| 16   | x86_32     | 83.45%    | 261           |
| 15   | sparc64    | 86.05%    | 220           |
| 14   | arm32      | 96.77%    | 51            |
| 13   | armeb      | 96.77%    | 51            |
| ...  | ...        | ...       | ...           |
| 7    | hppa       | 97.59%    | 38            |
| 8    | alpha      | 97.27%    | 43            |

`hppa` and `alpha` are mid-pack (rank 7 and 8), not weakest. `ppc64`
and `ppc64le` (rank 18 and 17, both at 81.29%) were missed entirely.
The catalog's "top-5 weakest" should be `m68k, ppc64, ppc64le, x86_32,
sparc64`.

### 1.4 V-39 baseline is stale

**Subagent A-2 finding**: The `test_results/` snapshot is from commit
`78e71a6b` (2026-07-31 23:46:38 UTC). The phi+regalloc liveness fix
landed at `1d72d296`, ~51 minutes later. The 93.42% figure is
pre-fix. The catalog should mark V-39's baseline as stale and recommend
a re-run on `main` HEAD before treating it as ground truth.

### 1.5 "18 of 19 backends" — STALE (now "all 19")

**Subagent A-2 finding**: Since commit `f714a7a5` ("[19/19] ALL 19
backends now have `reg_isel.rs`"), all 19 backends have a `reg_isel.rs`.
The breakdown is 15 substantive + 4 byte-swap wrappers (6-line files
that delegate to a parent). The catalog's "14 native + 4 wrappers = 18"
count is one short because `aarch64` was already migrated to
`TargetAgnosticRegAlloc` by W7-impl and is now counted among the
substantive `reg_isel.rs` backends.

**Correction**: "19 backends, all with `reg_isel.rs` (15 substantive +
4 byte-swap wrappers)."

---

## 2. Newly surfaced bugs — 35 entries

The four subagents surfaced 35 new bugs. After de-duplication (two
subagents independently found V-NEW-2 / V-A3-1 — the IVE-side
`rederive_layout` reproducing the V-03 bug — and one was a
misunderstanding), 33 distinct new bugs remain. They are renumbered
V-42 through V-74 below, preserving the subagent-of-origin as a
suffix for traceability.

### 2.1 Cluster: Type-bridge silent-miscompile family

This is the single largest bug family. The root cause (per A-1) is the
three-way type representation: parser `Type::BDBase(String)`, canonical
SCG `String`, codegen `IRType`. Every bridge between them is string
matching with `_ => <default>` arms, and every default arm is a silent
miscompile.

| ID | Sev | File:line | Description | Effort |
|----|-----|-----------|-------------|--------|
| V-34 | P0 | `pipeline.rs:6515` | `bridge_type_to_ir_type` `_ => U64` catches f32/f64 | 3 days |
| V-35 | P0 | `to_scg.rs:4057` | `type_size_from_name` `_ => 8` catches layout names | 1 week |
| V-42 | P0 | `to_scg.rs` (`register_layout`) | V-35 propagates to layout field offsets and `total_size` | (subsumed by V-35) |
| V-43 | P1 | `to_scg.rs` (`infer_expr_type`) | Returns variable NAMES not types — defeats V-35 fix for `*ptr` deref sizes | 1 week |
| V-44 | P0 | `to_scg.rs` (`type_alignment`) | `_ => 8` catch-all for `Type::BDBase(name)` (twin of V-35) | 2 days |
| V-46 | P0 | `pipeline.rs:7403` (`resolve_state_array_access`) | `_ => (1, None)` for unknown element types — `[StructType; N]` indexing silently accesses byte `i` instead of `i * sizeof(T)` | 1 week |
| V-03 | P0 | `pipeline.rs:6532, :6724` | Legacy `bridge_type_size` still called by `build_pmt_layout_specs` even though `_with_layouts` exists | 1 week |
| V-NEW-2 | P0 | `ive/verification.rs:268` (`rederive_layout`) | IVE intentionally reproduces V-03 bug for parity — will break IVE after V-03 fix unless migrated in lockstep | 3 days |
| V-NEW-1 | P0 | `pipeline.rs:9228, 9297, 9598` | `allocate(<non-literal>)` silently truncates to 8 bytes | 1 week |

**Architectural fix (per A-1)**: replace the three-way string-based
type representation with a unified type enum. This would eliminate
V-34, V-35, V-42, V-44, V-46, V-03, V-NEW-2 in one stroke. Estimated
2–3 weeks of refactoring (touches parser, SCG, IVE, codegen, every
backend's `lower_instruction`). Defer until after the bridge-fix epic
(V-34 → V-35 → V-36 → V-03 → V-40) lands, then evaluate.

### 2.2 Cluster: Hardcoded IRType / IRInstr family

| ID | Sev | File:line | Description | Effort |
|----|-----|-----------|-------------|--------|
| V-36 | P0 | `scg_to_ir.rs:6011, 6024` | `StateRead`/`StateWrite` hardcoded `IRType::I64` | 1 week |
| V-A2-1 | P0 | `scg_to_ir.rs` (StateInit, ArenaNew, ArenaAlloc) | `Alloc { size: 0 }` — state buffers are zero-sized; root cause of `float_mem/*` failures on 17 backends | 1 week |
| V-A2-2 | P1 | `scg_to_ir.rs` (CastKind) | `inttofloat`/`floattoint`/`uinttofloat`/`floattouint` hardcoded to `I64 ↔ F64` — blocks f32 casts and 32-bit int↔float casts | 1 week |
| V-A2-5 | P2 | `scg_to_ir.rs` (`current_return_type`) | Parsed from function name — clobbers correct type for f32/f64/ptr returns | 3 days |
| V-A2-6 | P2 | `scg_to_ir.rs` (`channel_open` Call-form) | Hardcodes `Channel<I64>` payload type | 3 days |

### 2.3 Cluster: SIMD / vectorization

| ID | Sev | File:line | Description | Effort |
|----|-----|-----------|-------------|--------|
| V-13 | P2 | `x86_64/stack_slot_isel.rs:3493`, `aarch64/mod.rs:3452` | SIMD coverage narrow; AArch64 lacks `2D` (i64) form | 6 weeks |
| V-A2-3 | P1 | `x86_64/stack_slot_isel.rs`, `aarch64/mod.rs` | SIMD lowering hardcodes `Xmm0/1/2` and `V0/1/2`; vectorizer non-functional for real loops | 2 weeks |

### 2.4 Cluster: Silent no-op IR instructions

| ID | Sev | File:line | Description | Effort |
|----|-----|-----------|-------------|--------|
| V-A2-4 | P1 | 14+ backends | `Transform`/`BulkCopy`/`BulkFill`/`StarkProof`/`Channel*` are silent no-ops on most backends; only x86_64 implements `BulkCopy`/`BulkFill` | 3 weeks |

### 2.5 Cluster: Backend-specific softfloat bugs

| ID | Sev | File:line | Description | Effort |
|----|-----|-----------|-------------|--------|
| V-A2-7 | P1 | `hppa/` | F64 softfloat `sub`/`mul`/`div` return 0; `lt` returns 0 for negative operands; F32 entirely stubbed | 2 weeks |
| V-A2-8 | P1 | `m68k/` | F32 softfloat stubs return `0.0` for `Register` operands | 1 week |

### 2.6 Cluster: Register allocator

| ID | Sev | File:line | Description | Effort |
|----|-----|-----------|-------------|--------|
| V-A2-9 | P1 | `regalloc.rs` | Doesn't model syscall arg/dst interference — root cause of the `contains_fork` opt-out (which is correct but over-conservative) | 2 weeks |

### 2.7 Cluster: Capability model + security

| ID | Sev | File:line | Description | Effort |
|----|-----|-----------|-------------|--------|
| V-16 | P1 | `capability.rs` | FNV-1a × 4 signatures (not HMAC-SHA256); `verify_capability` is never called from emitted binaries | 5 weeks |
| V-A3-2 | P0 | `capability.rs:49–54` (`delegate_capability`) | Hardcodes signing key `b"vuma_dev_signing_key"` and PIDs `1, 2` | 1 week |
| V-A3-8 | P1 | `ive/verification.rs` (`verify_information_flow_from_ir`) | Misses indirect flows — no `BinOp`/`Branch` events emitted | 2 weeks |

### 2.8 Cluster: IVE / Lean proofs

| ID | Sev | File:line | Description | Effort |
|----|-----|-----------|-------------|--------|
| V-14 | P1 | `proof/PMT/` | f32 PMT Lean proof is greenfield (no arithmetic model to extend) — defer to v2 | 3–6 months |
| V-A3-1 | P0 | `ive/verification.rs:268–327` (`verify_layout_consistency`) | Structurally blind to V-03 — uses same `_ => 8` catch-all; docstring at `:264–267` admits this | (subsumed by V-NEW-2) |
| V-A3-3 | P1 | `compile_dump.rs:228` (`discharge_rate`) | Excludes `failed` from denominator; `unwrap_or(100)` returns 100% on all-failed | 3 days |
| V-A3-5 | P2 | `proof/PMT/IVE/Soundness/SessionType.lean:27–31` | Lean `SessionType` model behind Rust IVE by 4 variants (`Choice`/`Offer`/`Rec`/`Var` exist in Rust `session_type.rs:38–56` as dead code) | 2 weeks |
| V-A3-6 | P3 | `ive/verification.rs:2379` | `l1l3_collapse` has dead `let known = true` branch | 1 day |
| V-A3-7 | P2 | `effects.rs` | `Effect::ExternCall` is dead code — IVE has zero references to it (refutes the catalog's claim that IVE consumes the Effect enum) | 1 day |

### 2.9 Cluster: Session types

| ID | Sev | File:line | Description | Effort |
|----|-----|-----------|-------------|--------|
| V-11 | P1 | `ast.rs:1632`, `ir.rs:167` | AST/IR `SessionType` lack `Choice`/`Offer`/`Rec`/`Var`. NOTE: IVE-side `session_type.rs:38–56` already has them as dead code — the IVE work is done, only the AST/IR plumbing + parser + Lean proof remain | 2 weeks (down from 2–4) |

### 2.10 Cluster: Parser / AST gaps

| ID | Sev | File:line | Description | Effort |
|----|-----|-----------|-------------|--------|
| V-26 | P1 | `ast.rs:1511`, `parser.rs:2999` | No `Expr::ArrayLit` / `Lit::Bytes` — blocks SPIR-V embedding, font subsetting | 2 weeks |
| V-45 | P3 | `ast.rs:1511–1525` | Stale `Lit::Float` doc comment claims lexer doesn't produce float tokens, but it does (`lexer.rs:1279, 1289, 1317, 1323`) | 1 day |
| V-47 | P2 | `to_scg.rs` (`extract_state_write_target`) | Only handles `AssignTarget::DerefField`; state-typed writes through `Index`/`Deref` silently lose layout info | 1 week |
| V-48 | P2 | `to_scg.rs` (`ConstantFolding`) | Only folds 3 ops + parses constants as `f64` (precision loss); effectively dead code because parser doesn't emit the labels it expects | 1 week |
| V-49 | P2 | `to_scg.rs` (`NodeVisitor::dispatch`) | Only handles 10 of 28 `NodePayload` variants; 18 silently route to `visit_default` | 1 week |

### 2.11 Cluster: CI / build infrastructure

| ID | Sev | File:line | Description | Effort |
|----|-----|-----------|-------------|--------|
| V-A3-4 | P1 | `.github/workflows/lean-rust-parity.yml` | CI tests non-existent FFI bridge (build.rs says bridge is deleted) | 1 day |
| V-NEW-6 | P1 | `scripts/ci_run_tests.sh:61` | Pass criterion is "didn't crash", not "got the right answer" — CI falsely green on wrong-output regressions | 3 days |
| V-NEW-7 | P2 | `.github/workflows/ci.yml` + `proof-verify.yml` | Duplicate `lean-proofs` job wastes ~10 CI minutes per push | 1 day |
| V-NEW-8 | P1 | `.github/workflows/ci.yml` | Full 19-backend × 1577-test matrix NOT in CI — only 7 backends × 47 examples + x86_64 gold-standard are gated. The 6.58% gap from 100% is invisible to CI | 1 week |

### 2.12 Cluster: Stale documentation

| ID | Sev | File:line | Description | Effort |
|----|-----|-----------|-------------|--------|
| V-41 | P3 | multiple | Stale doc refs persist in non-README files (25+ refs found by A-4, not "mostly fixed" as catalog claimed) | 2 days |

---

## 3. Dependency manifest — small-deps audit

**Policy**: "only small dependencies." The audit confirms VUMA honors
this strongly.

**Total external crates: 8** (3 declared + 5 transitive), **0
duplicates** in `Cargo.lock`:

| Crate | Version | Used by | Purpose | Assessment |
|-------|---------|---------|---------|------------|
| `bitflags` | 2.13.1 | `vuma-codegen` | Bitflag macros for `Effect`, `ArgMode` | **Keep** — tiny, no transitive deps |
| `z3` | 0.20.2 | `vuma-ive` | SMT solver bindings (the "V" in VUMA) | **Keep** — hard dependency, required by design |
| `z3-sys` | 0.11.0 | (transitive of `z3`) | FFI to libz3 | (transitive) |
| `log` | 0.4.33 | (transitive of `z3`) | Logging facade | (transitive) |
| `pkg-config` | 0.3.x | (transitive of `z3-sys`) | libz3 discovery | (transitive) |
| `cc` | 1.4.0 | `vuma-codegen` (build-dep) | C compiler driver for build.rs | **DELETE** — unused since Lean FFI removal |
| `find-msvc-tools` | 0.x | (transitive of `cc`) | MSVC discovery | (transitive — delete with `cc`) |
| `shlex` | 1.x | (transitive of `cc`) | Shell quoting for `cc` | (transitive — delete with `cc`) |

**Action items**:

1. **Delete `cc` from `vuma-codegen/build.rs` dependencies** — it was
   used to build a Lean→C→Rust FFI bridge that no longer exists (per
   A-4). Removing it eliminates 3 transitive crates from the lockfile.
2. **Do NOT add `serde`/`serde_json`/`toml`** — Wave 43 already purged
   these in favor of hand-written equivalents. The audit confirms this
   was the right call.
3. **Do NOT add `regex`** — the parser uses hand-written NFAs. Keep it
   that way.
4. **Z3 is the one carve-out** — it's a C FFI binding to a 50MB
   library, but it's the entire point of VUMA (contract discharge).
   Non-negotiable.

**Post-cleanup dep count**: 5 external crates (1 declared `bitflags` +
1 declared `z3` + 3 transitive of `z3`). This is exceptionally lean
for a compiler project of this scope.

---

## 4. Test suite deep analysis

### 4.1 Headline numbers (from `test_results/summary.json`, commit `78e71a6b`)

- Total runs: 29963
- Matches: 27992
- Skipped: 0
- Pass rate: 93.42%
- Failures: 1971 across 364 tests

**Caveat**: This snapshot is pre-`1d72d296` (the phi+regalloc liveness
fix). The real current pass rate is likely higher. A re-run on `main`
HEAD is the first item of business.

### 4.2 Per-backend ranking (corrected)

| Rank | Backend | Pass | Total | Pass% | Failure mode |
|------|---------|------|-------|-------|--------------|
| 1 | wasm32 | 1577 | 1577 | 100.00% | — |
| 2 | s390x | 1576 | 1577 | 99.94% | 1 MM |
| 3 | x86_64 | 1543 | 1577 | 97.78% | mixed MM/TO |
| 4 | aarch64 | 1533 | 1577 | 97.21% | mixed MM/TO |
| 5 | aarch64_be | 1533 | 1577 | 97.21% | (wrapper, inherits aarch64) |
| 6 | riscv64 | 1532 | 1577 | 97.27% | mixed MM/TO |
| 7 | hppa | 1539 | 1577 | 97.59% | F64 softfloat (V-A2-7) |
| 8 | alpha | 1534 | 1577 | 97.27% | MM in arithmetic |
| 9 | loongarch64 | 1532 | 1577 | 97.27% | mixed MM |
| 10 | mips64 | 1532 | 1577 | 97.27% | mixed MM |
| 11 | mips64be | 1532 | 1577 | 97.27% | (wrapper, inherits mips64) |
| 12 | riscv32 | 1526 | 1577 | 96.77% | mixed MM/CR |
| 13 | arm32 | 1526 | 1577 | 96.77% | CR (segfault) |
| 14 | armeb | 1526 | 1577 | 96.77% | (wrapper, inherits arm32) |
| 15 | sparc64 | 1357 | 1577 | 86.05% | CR + MM in arithmetic |
| 16 | x86_32 | 1316 | 1577 | 83.45% | CR + MM |
| 17 | ppc64le | 1282 | 1577 | 81.29% | MM (loop lowering) |
| 18 | ppc64 | 1282 | 1577 | 81.29% | MM (loop lowering) |
| 19 | m68k | 1268 | 1577 | 80.47% | TO (80% of all TOs) + F32 softfloat (V-A2-8) |

### 4.3 Failure-mode breakdown

- **CR (crash, exit -11 SIGSEGV / -8 SIGFPE)**: concentrated in
  `arm32`, `armeb`, `riscv32`, `x86_32`, `m68k`, `hppa`. Indicates
  backend codegen bugs that produce invalid instruction encodings or
  trap. Highest-signal category — each CR is likely a real bug.
- **MM (mismatch — wrong numeric result)**: spread across all
  backends. Indicates codegen correctness bugs (wrong constant
  folding, wrong shift amount, wrong branch direction). The
  `ppc64`/`ppc64le` cluster (295 failures each, almost all MM)
  suggests a shared loop-lowering bug — see §4.4.
- **TO (timeout, exit 124)**: 80% concentrated in `m68k` (243 of 302
  TOs). Indicates either infinite loops in compiled programs or
  extreme perf regressions. `m68k` TOs likely correlate with V-A2-8
  (F32 softfloat returning 0.0 → infinite loop in float-based loop
  bounds).

### 4.4 The ppc64/ppc64le/x86_32/sparc64/m68k loop-lowering cluster

**Subagent A-4 hypothesis**: The `nested_loops` + `control_flow` test
families (235 failures, ~12% of all failures) follow an identical
pattern across 5 backends:

| Backend | Symptom | Hypothesized root cause |
|---------|---------|--------------------------|
| ppc64/ppc64le | return 0 (loop body never executes) | Inverted exit branch |
| x86_32 | return 0 (same) | Inverted exit branch (shared code path?) |
| sparc64 | return `(n+1)×(m+1)` | Off-by-one in loop bound |
| m68k | TO/124 (infinite loop) | Loop condition never becomes false |

A single PR fixing the shared loop-lowering code path for these 5
backends would lift the pass rate from 93.42% to ~94.0% in one shot.
This is the highest-ROI test-suite fix.

### 4.5 CI gaps

1. **Full 19-backend × 1577-test matrix is NOT in CI** — only 7
   backends × 47 examples + x86_64 gold-standard are gated (V-NEW-8).
   The 6.58% gap from 100% is invisible to CI.
2. **`ci_run_tests.sh:61` pass criterion is "didn't crash"** (V-NEW-6)
   — CI is falsely green on wrong-output regressions.
3. **Duplicate `lean-proofs` job** in `ci.yml` + `proof-verify.yml`
   (V-NEW-7) wastes ~10 CI minutes per push.
4. **`lean-rust-parity.yml` tests a non-existent FFI bridge**
   (V-A3-4) — the workflow's doc comments describe a Lean→C→Rust
   linkage that `build.rs` says was deleted.

---

## 5. Architectural observations

### 5.1 The type-bridge anti-pattern (root cause of 8 bugs)

The parser represents types as `Type::BDBase(String)`. The SCG layer
canonicalizes to `String`. The codegen layer uses `IRType` (a typed
enum). Every bridge between these layers is string matching with a
`_ => <default>` arm, and every default arm is a silent miscompile.

This single anti-pattern accounts for V-34, V-35, V-42, V-44, V-46,
V-03, V-NEW-2, and V-NEW-1 — 8 of the 18 P0 bugs.

**Fix**: introduce a unified `VumaType` enum in the parser, lower it
to `ScgType` and `IRType` via total (no-default) matches. This is a
2–3 week refactor that touches every layer but eliminates the entire
bug class. Defer until after the bridge-fix epic lands, then evaluate.

### 5.2 The "Effect enum is dead code" finding

**Subagent A-3 finding**: The catalog claims "IVE marks `Effect::Gpu`
/ `Effect::ExternCall` as impure (no CSE / memoization)." This is
false. `Effect`/`EffectSet`/`analyze_program_effects` have ZERO
references in `src/ive/`. The only consumer is `pipeline.rs:4431`,
which discards the map after counting pure functions for a summary.

**Implication**: The Effect system is currently decorative. Either
wire it into IVE (real work — 2–3 weeks) or delete it (1 day). The
catalog's V-02 proposal (add `Effect::Gpu`, `Effect::ShapeText`,
`Effect::Animate`) is premature until this is resolved.

### 5.3 The "verify_capability is never called" finding

**Subagent A-3 finding**: `capability.rs:49–54` admits that
`verify_capability` is never called from emitted binaries. The
capability model is write-only — capabilities are minted and signed
but never checked at runtime.

**Implication**: The capability model is currently security theater.
V-16 (HMAC-SHA256 upgrade) is necessary but not sufficient — the
runtime check path must also be wired in. This expands V-16 from 5
weeks to ~7 weeks.

### 5.4 The "three Lean Arena models disagree" finding

**Subagent A-3 finding**: There are three separate Lean models of the
PMT arena:

1. `proof/PMT/Basic.lean` — 3 fields (`base`, `capacity`, `used`)
2. `proof/PMT/Faithful/Model.lean` — 4 fields (adds `alloc_id`)
3. The Rust runtime `arena.rs` — 5 fields (adds `layout` + `created_thread`)

The `Faithful` model is closer to the runtime but is not the one used
by `pmt_soundness`. The `Basic` model is what's proved, but it doesn't
reflect the actual runtime layout.

**Implication**: The PMT soundness proof is about a simpler model
than what actually runs. This is a known gap in formal methods
(abstraction is fine if the abstraction is sound), but the gap should
be documented.

### 5.5 The "Lean proofs contain tautologies" finding

**Subagent A-3 finding**: The Lean proof layer is genuinely
`sorry`-free (CI strict mode passes). However, it contains:

- 2 tautology theorems masquerading as soundness results
  (`SessionType.lean:140–144` and `L1L3Collapse.lean:150–154`, both
  `exact hverify` — they assume the conclusion)
- 1 honestly-renamed tautology (`SimSound2.lean`)
- 3 `native_decide` substring-check "theorems" (`Faithful/Extract.lean`)

Only `InformationFlow.lean` and `SimSound.lean` contain real
non-tautological soundness proofs.

**Implication**: The "discharge_rate=N%" headline overstates what's
actually proved. The honest statement is: "capacity preservation +
field-bounds safety are proved; session-type linearity and
information-flow have real proofs; l1l3 collapse and session-type
soundness are tautological."

---

## 6. Revised execution plan

The original catalog recommended: V-34 → V-35 → V-36 → V-03 → V-37
→ V-40. With V-37 refuted and V-NEW-2 / V-A3-1 added, the revised
bridge-fix epic is:

| Step | Bug | Effort | Cumulative |
|------|-----|--------|------------|
| 1 | V-34 (`bridge_type_to_ir_type` f32/f64) | 3 days | 3 days |
| 2 | V-44 (`type_alignment` `_ => 8`) | 2 days | 5 days |
| 3 | V-35 (`type_size_from_name` layouts) | 1 week | 12 days |
| 4 | V-42 (propagates with V-35) | (subsumed) | 12 days |
| 5 | V-46 (`resolve_state_array_access` `_ => (1, None)`) | 1 week | 19 days |
| 6 | V-03 (migrate `build_pmt_layout_specs` to `_with_layouts`) | 1 week | 26 days |
| 7 | V-NEW-2 (migrate `rederive_layout` in IVE in lockstep) | 3 days | 29 days |
| 8 | V-36 (`StateRead`/`StateWrite` IRType threading) | 1 week | 36 days |
| 9 | V-A2-1 (`StateInit`/`ArenaNew`/`ArenaAlloc` `Alloc { size: 0 }`) | 1 week | 43 days |
| 10 | V-NEW-1 (`allocate(<non-literal>)` truncation) | 1 week | 50 days |
| 11 | V-40 (delete legacy `bridge_type_size`) | 1 day | 51 days |

**Total bridge-fix epic**: ~10 weeks (was 3.5 weeks in the original
catalog). The expansion reflects the newly-surfaced bugs in the same
family — V-44, V-46, V-NEW-2, V-A2-1, V-NEW-1 — that must land
together for the fix to actually work end-to-end.

**After the epic**: re-run the full test suite on `main` HEAD to get
an accurate V-39 baseline, then triage the ppc64/ppc64le loop-lowering
cluster (§4.4) for the highest-ROI test-suite fix.

---

## 7. Confidence assessment for ADRs

ADRs will be written in Wave C for decisions where the research gives
high confidence. The following meet that bar:

| ADR | Topic | Confidence | Reason |
|-----|-------|------------|--------|
| ADR-0001 | Fix V-34 by adding f32/f64 arms to `bridge_type_to_ir_type` | High | 1-line fix, verified, low risk |
| ADR-0002 | Fix V-35 by consulting layouts table in `type_size_from_name` | High | Verified, ~10-line change, load-bearing |
| ADR-0003 | Fix V-36 by threading IRType through StateRead/StateWrite | High | Verified, clear fix path |
| ADR-0004 | Migrate `build_pmt_layout_specs` to `bridge_type_size_with_layouts` (V-03) + IVE `rederive_layout` in lockstep (V-NEW-2) | High | Verified, the `_with_layouts` function already exists |
| ADR-0005 | Delete legacy `bridge_type_size` after V-03 (V-40) + delete `cc`/`find-msvc-tools`/`shlex` build-deps | High | Verified zero callers, verified unused |
| ADR-0006 | Defer V-14 (f32 PMT Lean proof) to v2; use runtime `__float_overflow_trap` only | High | Verified greenfield, 3–6 months not justified for v1 |
| ADR-0007 | Wire `verify_capability` into emitted binaries as part of V-16 | Medium-High | Verified never-called, but the wiring touches every backend's call ABI |
| ADR-0008 | Replace `discharge_rate` denominator to include `failed` (V-A3-3) | High | 3-line fix, verified |
| ADR-0009 | Re-run full test suite on `main` HEAD before treating V-39 as ground truth | High | Verified stale baseline |
| ADR-0010 | Adopt "5 external crates max" dependency policy (current: 8, target: 5) | High | Verified, aligns with user's "small deps" mandate |

ADRs will NOT be written for:

- V-13 (SIMD coverage) — needs benchmarking data first
- V-11 (session types) — IVE-side work is done but AST/IR plumbing design needs more thought
- V-26 (const byte arrays) — syntax design needs more thought
- The unified `VumaType` refactor (§5.1) — too large for an ADR, needs a separate RFC
- The "Effect enum is dead code" resolution (§5.2) — needs a decision on wire-vs-delete, which requires more usage analysis

These will be noted in the catalog as "ADR deferred — needs more design work."

---

## 8. Open questions for the next wave

1. **Should the unified `VumaType` refactor (§5.1) be done before or
   after the bridge-fix epic?** Doing it before would eliminate V-34,
   V-35, V-42, V-44, V-46, V-03, V-NEW-2, V-NEW-1 in one stroke but
   delays the bridge-fix epic by 2–3 weeks. Doing it after ships the
   fixes faster but creates throwaway work. Recommendation: after.
2. **Should the Effect enum be wired into IVE or deleted (§5.2)?**
   Wiring is 2–3 weeks of real work; deleting is 1 day. If V-02
   (rendering IR instructions) is going to land, wiring is necessary.
   If V-02 is deferred indefinitely, deleting is cleaner.
3. **Should the `Faithful` Lean Arena model replace `Basic` (§5.4)?**
   The `Faithful` model is closer to the runtime but harder to prove
   over. This is a research question, not an engineering decision.
4. **What's the right CI gating tier?** The full 29963-test matrix is
   too slow for per-PR gating. The current 7-backend × 47-example
   tier is too narrow. A middle ground (e.g. 5 strong backends × 200
   curated tests) needs to be designed.
