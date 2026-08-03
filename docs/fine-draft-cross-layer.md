# Cross-Layer Integration — Fine Draft (Final Engineering Plan)

**Status**: Final draft. Companion to `fine-draft-vuma.md`, `fine-draft-womb.md`,
`fine-draft-veee.md`.
**Scope**: Cross-layer concerns — bootstrap dependencies, shared
infrastructure, timeline coupling, risk register.
**Date**: 2026-08-01.

---

## 1. Executive summary

The VUMA ecosystem is a three-layer architecture (ADR-0013):

| Layer | What | Language | Owner | Timeline |
|-------|------|----------|-------|----------|
| **VUMA** | Compiler, codegen, runtime, PMT verification | Rust + Lean | VUMA team | Months 1-18 |
| **WOMB** | UI engine libraries | VUMA | UI engine team | Months 3-18 |
| **VEEE** | UX language + compiler | Rust (compiler) | VEEE team | Months 18-26 |

Each layer has its own fine draft. This document covers the **cross-layer
concerns** that no single-layer draft can address:

1. **Bootstrap dependencies** — VUMA's capability model depends on
   WOMB's `hmac.vuma`; VEEE's compiler depends on VUMA's AST API.
2. **Shared infrastructure** — the e-graph, the PMT arena, the Z3
   verifier, the 19-backend codegen are shared across layers.
3. **Timeline coupling** — WOMB can't start UI work until VUMA's
   bridge-fix epic lands; VEEE can't start until VUMA v1 + WOMB v1.
4. **Risk register** — the 6 cross-layer risks that could derail the
   plan, with mitigations.
5. **Definition of Done** — what "VUMA v1," "WOMB v1," and "VEEE v0.1"
   mean, concretely.

---

## 2. Bootstrap dependencies

The three layers are NOT independent — there are circular and
cross-layer dependencies that must be managed.

### 2.1 VUMA → WOMB (capability model bootstrap)

**The dependency**: VUMA's capability model
(`src/codegen/src/capability.rs`) needs HMAC-SHA-256 to sign
capability tokens. Per ADR-0007 (promoted by ADR-0024), VUMA will
hand-write HMAC-SHA-256 in pure Rust
(`src/codegen/src/hmac_sha256.rs`, ~300 LOC). BUT the WOMB layer
ALREADY has a working HMAC-SHA-256 in
`womb/crypto/mac_kdf/hmac.vuma` (193 LOC, RFC 2104, F-2 verified).

**The question**: Should VUMA's `hmac_sha256.rs` be the canonical
implementation, or should VUMA compile and use
`womb/crypto/mac_kdf/hmac.vuma`?

**Decision (ADR-0007 as revised by ADR-0024)**: VUMA hand-writes its
own `hmac_sha256.rs` in Rust. The WOMB `hmac.vuma` serves as the
**reference implementation** — the Rust hand-translation is
parity-tested against it (following the `tests/pmt_parity_test.rs`
pattern). This avoids a bootstrap dependency where the VUMA compiler
(Rust) would need to compile a `.vuma` file to get its own capability
signing key working.

**Risk**: If `hmac_sha256.rs` and `hmac.vuma` diverge, capability
tokens signed by the compiler won't validate at runtime. Mitigated by:
parity tests in every CI run.

### 2.2 VEEE → VUMA (AST API dependency)

**The dependency**: VEEE's compiler (`veeec`) lowers `.veee` source to
VUMA AST (`vuma_parser::ast::AstProgram`). The integration point is
`vuma::pipeline::compile_modules`. VEEE depends on VUMA's AST being a
**stable interface**.

**The risk**: If VUMA's AST changes (e.g. new node types, changed
constructors), VEEE's lowering breaks. This is a tight coupling.

**Mitigation**: VUMA's AST is the parser's output, documented in
`docs/language-reference.md`. It changes rarely (only when the VUMA
language itself changes). VEEE's compiler pins a VUMA version and
updates only when VUMA releases a new version. This is the same model
Rust uses (rustc's AST changes each edition, tools update accordingly).

### 2.3 VEEE → WOMB (runtime library dependency)

**The dependency**: VEEE programs call WOMB UI primitives
(`womb/ui/layout/flex.vuma`, `womb/ui/render/vector.vuma`, etc.).
VEEE's standard library is a thin wrapper over WOMB.

**The risk**: If WOMB's module APIs change, VEEE programs break.

**Mitigation**: WOMB module APIs are VUMA `transform` signatures.
These are typed and verified by IVE. A breaking change is a type
error, caught at compile time. VEEE pins a WOMB version and updates
when WOMB releases.

### 2.4 WOMB → VUMA (verification dependency)

**The dependency**: WOMB modules are VUMA source. They get PMT
verification (Z3 + Lean) for free — but only if VUMA's verification
is sound. V-03 (IVE `rederive_layout` parity bug, ADR-0004) means
IVE is currently UNSOUND for nested layouts. WOMB UI modules that
use nested layouts (e.g. `layout SceneNode = { children: [SceneNode; N], ... }`)
would be "verified" incorrectly until V-03 is fixed.

**The mitigation**: ADR-0004 (migrate `build_pmt_layout_specs` +
`rederive_layout` in lockstep) must land BEFORE WOMB UI modules that
use nested layouts. This is a hard prerequisite.

### 2.5 The circular dependency (VUMA ↔ WOMB)

There's a theoretical circular dependency: VUMA's capability model
uses WOMB's HMAC, and WOMB's code is compiled by VUMA. In practice
this is broken by:
- VUMA's HMAC is hand-written Rust (not compiled from WOMB).
- WOMB's HMAC is VUMA source (compiled by VUMA).
- The two are parity-tested but NOT linked.

This is the same pattern as a self-hosted compiler: the bootstrap
compiler is hand-written, the self-hosted compiler is written in the
language it compiles. No actual circularity.

---

## 3. Shared infrastructure

These VUMA-side components are shared across all three layers and
must NOT be duplicated:

### 3.1 E-graph (`src/codegen/src/egraph.rs`, 3235 lines)

- **VUMA uses it**: optimization-time equivalent-expression detection.
- **VEEE uses it**: signal-change reactivity optimization (the
  `state_store_load_forward` rewrite is exactly what VEEE needs).
- **WOMB doesn't use it directly** (WOMB is VUMA source, not Rust).

**Decision**: VEEE's compiler adds VEEE-specific rewrite rules
ALONGSIDE VUMA's existing rules. No fork. The e-graph is a VUMA-layer
component that VEEE extends.

### 3.2 PMT arena (`src/codegen/src/runtime/arena.rs`)

- **VUMA owns it**: the memory-safety verified arena.
- **WOMB uses it**: all WOMB state (UI trees, event rings, font caches)
  lives in PMT arenas.
- **VEEE uses it**: VEEE `signal` lowers to `State<T>` which lives in
  the arena.

**Decision**: One arena implementation, owned by VUMA. WOMB and VEEE
are consumers.

### 3.3 Z3 verifier (`src/ive/`, `z3 = "0.20"`)

- **VUMA owns it**: contract discharge, session-type linearity,
  information-flow lattice, PMT invariants.
- **WOMB benefits**: WOMB code is verified by VUMA's IVE (free).
- **VEEE benefits**: VEEE programs are verified by VUMA's IVE (free,
  because VEEE lowers to VUMA AST).

**Decision**: One Z3 integration, owned by VUMA. VEEE's type system
(monotonicity, incremental computation) lowers to VUMA contracts that
Z3 discharges — VEEE doesn't have its own verifier.

### 3.4 19-backend codegen

- **VUMA owns it**: 19 CPU ISA backends + wasm32.
- **WOMB doesn't touch it** (WOMB is VUMA source).
- **VEEE uses it**: VEEE programs compile to VUMA AST → VUMA's 19
  backends. VEEE doesn't have its own codegen (ADR-0023: no Cranelift;
  ADR-0022: GPU via hand-written SPIR-V, not VUMA codegen).

**Decision**: One codegen, owned by VUMA. The GPU path (SPIR-V) is
VEEE's concern but uses VUMA's V-26 (const byte arrays) for embedding.

### 3.5 Lean formal spec (`proof/PMT/`, 82 files, 280 theorems, 0 sorries)

- **VUMA owns it**: the formal specification of the PMT memory model.
- **WOMB benefits**: WOMB code is covered by the Lean spec (because
  it's VUMA source).
- **VEEE benefits**: VEEE programs are covered by the Lean spec
  (because they lower to VUMA AST).

**Decision**: One Lean spec, owned by VUMA. V-14 (f32 PMT Lean proof)
is deferred to v2 per ADR-0006.

---

## 4. Timeline coupling

### 4.1 Critical path

```
VUMA Phase 1: V-34 fix (DONE, commit a58dee80)
    │
    ▼
VUMA Phase 2: Security P0 (V-16 + V-A3-2, ADR-0007/0024, 7 weeks)
    │
    ├──► WOMB Phase 1: V-WOMB-1 fix (1 day, parallel, no dep)
    │
    ▼
VUMA Phase 3: V-03 IVE soundness (ADR-0004, 2 weeks)
    │
    ├──► WOMB Phase 2: womb/sync/spsc.vuma (1 week, parallel)
    │
    ▼
VUMA Phase 4: Backend stabilization (V-S390X-1, W1-sparc64, W1-x86_32, ~4 weeks)
    │
    ├──► WOMB Phase 3: womb/ui/event/ + womb/ui/layout/ (12 weeks, parallel)
    │
    ▼
VUMA Phase 5: Parser gaps (V-26 const byte arrays, V-11 session types, ~4 weeks)
    │
    ├──► WOMB Phase 4: womb/ui/text/ (6+2+10+5+2 = 25 weeks, parallel)
    │
    ▼
VUMA Phase 6: CI hardening (V-NEW-6, V-NEW-8, ~2 weeks)
    │
    ├──► WOMB Phase 5: womb/ui/render/ (3 + 6 = 9 weeks, needs V-26)
    │
    ▼
VUMA Phase 7: Cleanup (V-40, V-A3-7, V-A2-4, ~2 weeks)
    │
    ├──► WOMB Phase 6: womb/ui/ime/ + womb/ui/a11y/ (needs V-11, ~10 weeks)
    │
    ▼
VUMA v1 (month 18)
    │
    ├──► WOMB Phase 7-8: animation + theme + integration (~6 weeks)
    │
    ▼
WOMB v1 (month 18)
    │
    ▼
VEEE E-0: Language design (10 weeks, month 18-20)
    │
    ▼
VEEE E-1: Compiler skeleton (8 weeks, month 20-22)
    │
    ▼
VEEE E-2: Incremental computation + monotonicity (8 weeks, month 22-24)
    │
    ▼
VEEE E-3: Standard library + GPU path (8 weeks, month 24-26)
    │
    ▼
VEEE v0.1 (month 26)
```

### 4.2 Parallelism opportunities

- **VUMA Phase 2 (security) + WOMB Phase 1 (V-WOMB-1)**: parallel, no deps.
- **VUMA Phase 3 (V-03) + WOMB Phase 2 (spsc.vuma)**: parallel, no deps.
- **VUMA Phase 4 (backends) + WOMB Phase 3 (event + layout)**: parallel, WOMB only needs V-34 (DONE).
- **VUMA Phase 5 (parser) + WOMB Phase 4 (text)**: parallel, WOMB text needs V-34 (DONE) + V-A2-3 (can defer).
- **VUMA Phase 6 (CI) + WOMB Phase 5 (render)**: parallel, render needs V-26 (from VUMA Phase 5).
- **VEEE E-0 (language design) + WOMB Phase 7-8 (animation + theme)**: parallel.

### 4.3 Blocking dependencies (hard prerequisites)

| WOMB/VEEE work | Requires VUMA fix | Status |
|----------------|-------------------|--------|
| WOMB womb/ui/layout/ with f32 fields | V-34 | **DONE** (a58dee80) |
| WOMB womb/ui/render/ with SPIR-V embedding | V-26 | Deferred (2 weeks) |
| WOMB womb/ui/layout/ with nested layouts | V-03 (IVE soundness) | Deferred (ADR-0004, 2 weeks) |
| WOMB womb/ui/text/ with SIMD acceleration | V-A2-3 (vectorizer fix) | Deferred (2 weeks) |
| WOMB womb/ui/ime/ with session types | V-11 (Choice/Offer) | Deferred (2 weeks) |
| VEEE GPU kernels | V-26 (const byte arrays) | Deferred (2 weeks) |
| VEEE signals → State<T> | V-34 | **DONE** |
| VEEE monotone collections → nested layouts | V-03 | Deferred |

### 4.4 Total timeline

- **Single-engineer**: ~26 months (VUMA 18 + VEEE 8, WOMB parallel)
- **3-person** (1 VUMA + 1 WOMB + 1 VEEE, starting month 3): ~18 months
- **5-person** (2 VUMA + 2 WOMB + 1 VEEE, starting month 3): ~14 months

---

## 5. Risk register

### R-1: V-03 IVE unsoundness blocks WOMB nested layouts

**Probability**: High (V-03 is confirmed, fix is 2 weeks but not started)
**Impact**: High (every WOMB UI module that nests layouts is affected)
**Mitigation**: Land ADR-0004 BEFORE WOMB Phase 3. If delayed, WOMB
can use flat layouts (no nesting) as a workaround.

### R-2: V-26 (const byte arrays) delays WOMB renderer + VEEE GPU

**Probability**: Medium (2-week fix, but deferred)
**Impact**: High (WOMB render + VEEE GPU both blocked)
**Mitigation**: WOMB can load SPIR-V from disk via `womb/fs/file.vuma`
as a temporary workaround. VEEE GPU is months 24-26, so V-26 has time.

### R-3: s390x regression (V-S390X-1) indicates regalloc liveness bug

**Probability**: Medium (16 new failures from 1d72d296)
**Impact**: Medium (s390x is one of 19 backends; -1.02pp)
**Mitigation**: File V-S390X-1, assign to backend team. Not a blocker
for WOMB or VEEE (they don't target s390x specifically).

### R-4: W1-sparc64 + W1-x86_32 are mid-flight

**Probability**: High (both regressed, fixes incomplete)
**Impact**: Medium (transient; blocks accurate test measurement)
**Mitigation**: Cherry-pick the correct parts (sparc64 COND_ fixes,
x86_32 Call handler), revert the incomplete parts (sparc64 branch-based
Cmp, x86_32 mprotect arg conversion). Re-run test suite.

### R-5: VEEE's incremental computation may not compose with PMT

**Probability**: Low (VEEE lowers to State<T>, which is PMT-verified)
**Impact**: High (if they don't compose, VEEE's value proposition weakens)
**Mitigation**: Prototype VEEE signal → State<T> lowering early (VEEE
E-1, month 20). If it doesn't work, fall back to a less-verified model
(runtime reactivity, no PMT).

### R-6: Hand-written HMAC-SHA-256 may have bugs

**Probability**: Low (SHA-256 is well-specified, ~150 LOC)
**Impact**: High (capability model security)
**Mitigation**: Parity-test against `womb/crypto/mac_kdf/hmac.vuma`
(following `tests/pmt_parity_test.rs` pattern). Use NIST FIPS 180-4
test vectors as regression tests.

---

## 6. Definition of Done

### 6.1 VUMA v1 (month 18)

- [x] V-34 fixed (commit a58dee80)
- [ ] V-16 + V-A3-2 + V-A3-6 fixed (ADR-0007/0024, hand-written HMAC-SHA256 + IVE verifier wiring)
- [ ] V-03 + V-NEW-2 fixed (ADR-0004, IVE soundness for nested layouts)
- [ ] V-26 fixed (const byte arrays for SPIR-V embedding)
- [ ] V-11 fixed (session types Choice/Offer in AST/IR)
- [ ] V-S390X-1 fixed (s390x regression from 1d72d296)
- [ ] W1-sparc64 + W1-x86_32 stabilized
- [ ] Test suite pass rate ≥ 97% on all 19 backends with QEMU 10.0+
- [ ] Lean proofs: 0 sorries, `lake build` passes in CI
- [ ] Dependency count: 5 external crates (after ADR-0005)
- [ ] CI: full 19-backend × 1577-test matrix gated (V-NEW-8)
- [ ] CI: pass criterion is "correct exit code" not "didn't crash" (V-NEW-6)
- [ ] All 25 ADRs either Accepted or Superseded (no Proposed remaining)

### 6.2 WOMB v1 (month 18, parallel with VUMA)

- [ ] V-WOMB-1 fixed (broken womb/net imports, ADR-0020)
- [ ] womb/sync/spsc.vuma generalized from irq_ring.vuma (ADR-0019)
- [ ] womb/ui/event/ (SPSC UiEvent ring + dispatch)
- [ ] womb/ui/layout/ (Flexbox f32 + stacking + position + Knuth-Plass)
- [ ] womb/ui/render/ (vector renderer + gpu_dispatch + SPIR-V embedding)
- [ ] womb/ui/text/ (font parser + shaper v1/v2 + bidi + hinting + subsetting)
- [ ] womb/ui/ime/ (composition state machine + IBus/IMM32/IMK bridges)
- [ ] womb/ui/a11y/ (SemanticsNode + AT-SPI/UIA/NSA11y bridges)
- [ ] womb/ui/animation.vuma
- [ ] womb/ui/theme.vuma
- [ ] C host runtime (~500 LOC, SDL2 + Vulkan/Metal + libcurl + libwebsockets)
- [ ] All WOMB UI modules compile under VUMA with PMT verification passing
- [ ] Integration test: a VUMA program that renders "Hello World" with a
      real font, on x86_64 + aarch64 + wasm32

### 6.3 VEEE v0.1 (month 26)

- [ ] VEEE compiler (`veeec`) parses `.veee` source
- [ ] VEEE type checker verifies monotonicity + builds dependency graph
- [ ] VEEE lowers to VUMA AST (integration: `vuma::pipeline::compile_modules`)
- [ ] VEEE `signal` lowers to `State<T>` + `mark_dirty`
- [ ] VEEE `monotone set` lowers to `requires`/`ensures` Z3 discharges
- [ ] VEEE `render`/`schedule` lowers to SceneNode tree + WOMB renderer call
- [ ] VEEE GPU kernels (hand-written GLSL → glslangValidator → SPIR-V → embed)
- [ ] VEEE standard library (thin wrapper over WOMB)
- [ ] VEEE LSP (extends VUMA's LSP module)
- [ ] Example: a VEEE program that renders a counter with a button,
      compiles to VUMA, verifies, and runs on x86_64 + wasm32
- [ ] Documentation: VEEE language reference + tutorial

---

## 7. Open cross-layer questions

### Q-1: Should VUMA's capability HMAC use WOMB's `hmac.vuma` or hand-written Rust?

**Current decision (ADR-0007/0024)**: Hand-written Rust
(`src/codegen/src/hmac_sha256.rs`), parity-tested against
`womb/crypto/mac_kdf/hmac.vuma`.

**Open question**: BC-1 raised this as open question 9.6 — should we
propose ADR-0026 that supersedes ADR-0007 on the compile-time-vs-runtime
split? The VUMA team should weigh in.

**Recommendation**: Keep the hand-written Rust for v1 (avoids bootstrap
complexity). Revisit in v2 when VUMA can self-compile reliably.

### Q-2: Should VEEE have its own type checker or reuse VUMA's?

**Current decision (ADR-0014)**: VEEE lowers to VUMA AST, VUMA's type
checker runs. VEEE's type checker only verifies VEEE-specific features
(monotonicity, incremental computation).

**Open question**: Where exactly is the boundary? If VEEE's
monotonicity checker rejects a program, does VUMA's type checker also
see it? (No — VEEE's checker runs first, VUMA only sees the lowered AST.)

**Recommendation**: Document the boundary in the VEEE language
reference. VEEE's checker is authoritative for VEEE-specific features;
VUMA's checker is authoritative for VUMA-level types (i32, f32, State<T>, etc.).

### Q-3: How does VEEE interop with hand-written VUMA code?

**Current decision**: VEEE programs can call VUMA `transform`s directly
(VEEE's standard library is a thin wrapper over WOMB, which is VUMA).

**Open question**: Can a VUMA program call a VEEE-compiled function?
(The VEEE compiler produces VUMA AST, which VUMA can link — so yes,
in principle.)

**Recommendation**: Support bidirectional interop. VEEE → VUMA is
natural (VEEE lowers to VUMA). VUMA → VEEE requires the VEEE compiler
to run as a build step, producing VUMA `.vuma` files that the VUMA
build links. Document in the VEEE language reference.

### Q-4: Should the unified VumaType refactor happen before or after VEEE?

**Current status**: The unified `VumaType` refactor (replacing the
three-way string-based type representation) would eliminate V-34,
V-35, V-42, V-44, V-46, V-03, V-NEW-2, V-NEW-1 in one stroke. It's a
2-3 week refactor touching every layer.

**Open question**: If it happens after VEEE v0.1, VEEE's lowering
needs to handle both the old (string-based) and new (typed enum) type
representations. If before, VEEE targets only the new representation.

**Recommendation**: Do it BEFORE VEEE E-1 (month 20). The refactor
stabilizes the AST type representation, which is exactly the interface
VEEE targets. Doing it after VEEE would force a VEEE update.

### Q-5: What's the CI gating tier?

**Current state**: CI runs 7 backends × 47 examples + x86_64
gold-standard. The full 19-backend × 1577-test matrix is NOT in CI
(V-NEW-8). The pass criterion is "didn't crash" not "correct exit
code" (V-NEW-6).

**Open question**: What's the right gating tier? Full matrix is too
slow (~2-3 hours) for per-PR gating.

**Recommendation**: Three-tier CI:
1. **Per-PR**: 5 strong backends (x86_64, aarch64, riscv64, wasm32,
   s390x) × 100 curated tests, "correct exit code" criterion. ~5 min.
2. **Per-merge to main**: 19 backends × 47 examples. ~30 min.
3. **Nightly**: full 19-backend × 1577-test matrix. ~2-3 hours.

---

## 8. References

### Fine drafts (this set)
- `docs/fine-draft-vuma.md` (1,444 lines) — VUMA layer
- `docs/fine-draft-womb.md` (1,607 lines) — WOMB layer
- `docs/fine-draft-veee.md` (1,827 lines) — VEEE layer
- `docs/fine-draft-cross-layer.md` (this document)

### ADRs (25 total)
- ADR-0001 through ADR-0010: original decisions (some revised by ADR-0011)
- ADR-0011: meta-ADR (Wave F re-audit corrections, V-34 reverted to P0)
- ADR-0012 through ADR-0018: VEEE + three-layer architecture (0015, 0018 superseded)
- ADR-0019 through ADR-0021: WOMB + Effect enum cleanup
- ADR-0022 through ADR-0025: hand-written SPIR-V, dev codegen, ADR-0007 promotion, SIMD

### Research reports
- `docs/research/A-1-parser-scg.md` through `A-4-*.md` (Wave A)
- `docs/research/F-1-type-bridge-reality.md` through `F-3-*.md` (Wave F re-audit)
- `docs/research/J-1-womb-layer.md` (WOMB inventory)
- `docs/research/K-1-veee-rename-design.md` (VEEE design)
- `docs/research/AD-AE-s390x-w1-investigation.md` (s390x + W1 analysis)

### Test reports
- `docs/test-report-waves-s-z.md` (empirical testing on 155.138.203.27)

### Catalog
- `docs/vuma-side-problem-catalog.md` (master bug catalog, 631 lines)
- `docs/vuma-side-research-draft.md` (v1, superseded)
- `docs/vuma-side-research-draft-v2.md` (v2, corrected by Wave F)
- `docs/dependency-manifest.md` (5-crate policy)

### SWE package (historical)
- `vuma-swe-package/00-problem-catalog.md` through `26-new-plans-three-layers.md`
