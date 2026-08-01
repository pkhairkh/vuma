# VUMA-Side Problem Catalog

**Scope.** This catalog covers only problems that live inside the VUMA
repository itself: the parser, AST, SCG, IVE, IR, codegen backends,
runtime, capability model, Lean proofs, and the test suite. It explicitly
**excludes** the WOMB UI engine layer (layout, renderer, fonts, shaper,
IME, a11y) and the VELL UX-language layer — those are tracked separately
in the SWE package (`vuma-swe-package/`).

The catalog is the result of a focused audit pass against `main` at commit
`6dc97e18` (2026-08-01). Every P0 entry below has been verified by reading
the actual source — file paths and line numbers reflect the current tree,
not the SWE package's draft (which had several stale references).

The companion SWE package (`vuma-swe-package/22-review-against-vuma.md`)
proposed 32 VUMA-side patches. After verification, only ~9 of them are
real VUMA work; the rest are either redundant (VUMA already has the
feature), belong to WOMB/VELL, or are GPU-stack work whose scope is much
larger than a "patch."

---

## Summary table

| ID  | Title                                                  | Severity | Status              | Effort      |
|-----|--------------------------------------------------------|----------|---------------------|-------------|
| V-34 | `bridge_type_to_ir_type` misses f32/f64               | P0       | Open (verified)     | 3 days      |
| V-35 | `type_size_from_name` returns 8 for layout names      | P0       | Open (verified)     | 1 week      |
| V-36 | `StateRead`/`StateWrite` hardcoded to `IRType::I64`   | P0       | Open (verified)     | 1 week      |
| V-03 | Legacy `bridge_type_size` still used by `build_pmt_layout_specs` | P0 | Open (verified) | 1 week |
| V-37 | `build_pmt_layout_specs` should use `_with_layouts` variant | P0 | Open (depends on V-03) | 2 days |
| V-26 | Parser lacks const byte arrays / `Expr::ArrayLit`     | P1       | Open (verified)     | 2 weeks     |
| V-11 | Session types lack `Choice`/`Offer`                   | P1       | Open (verified)     | 2–4 weeks   |
| V-16 | Capability signatures use FNV-1a × 4 (not HMAC-SHA256) | P1       | Open                | 5 weeks     |
| V-13 | SIMD coverage narrow (no AVX2/AVX-512, no pmaxsd/pminsd) | P2    | Open (partial)      | 6 weeks     |
| V-14 | f32 PMT Lean proof is greenfield (no arithmetic model to extend) | P1 | Open | 3–6 months (defer to v2) |
| V-39 | Test suite at 93.42% — 1971 failures across 364 tests | P1       | Open (per `test_results/failures.txt`) | ongoing |
| V-40 | `bridge_type_size` and `bridge_type_size_with_layouts` coexist (dead-code risk) | P2 | Open | 1 day |
| V-41 | Doc references to `arm64.rs` and stale `regalloc.rs` line numbers persist in non-README files | P3 | Mostly fixed by Wave-1..6 | verify |
| V-04 | (was) Parser rejects `[T; N]` for struct T            | —        | REDUNDANT (parser accepts; bug is V-03/V-35) | — |
| V-05 | (was) `Expr::Index` always loads 1 byte               | —        | REDUNDANT (already implemented at `pipeline.rs:8075`) | — |
| V-07 | (was) `extern "C"` lacks Borrow/Marshal distinction   | —        | REDUNDANT (`ArgMode` exists; only `effects.rs` integration is a 3-day fix) | 3 days |
| V-01 | (was) Add `native` backend variant                    | —        | Out of scope (WOMB/native-host concern, not VUMA core) | — |
| V-02 | (was) Add rendering IR instructions                   | —        | Out of scope (depends on GPU stack — see V-GPU) | — |
| V-GPU | GPU backend infrastructure (Vulkan/Metal/SPIR-V/WebGPU) | P1 (downstream) | Greenfield — 3–6 months | tracked separately |

---

## P0 — Foundation bridge bugs (must fix before any f32 / nested-struct work)

These four bugs form a single dependency chain. Together they make it
impossible to use f32-coordinate state fields or nested-struct layouts
correctly through the typed-state API. They are the actual blockers
behind what the SWE package originally mislabeled V-03/V-04/V-05.

### V-34 — `bridge_type_to_ir_type` doesn't map `"f32"`/`"f64"`

**Severity**: P0 — blocks all f32 state fields.
**File**: `src/pipeline.rs:6503–6528`.
**Verified**: yes (read the source).

The `Type::BDBase(name)` arm handles `i8/i16/i32/i64/u8/u16/u32/u64` and
falls through `_ => IRType::U64` for everything else — including `"f32"`
and `"f64"`. Result: a state field declared as `measured_w: f32` gets
typed as `U64` at the IR layer, so `node.measured_w + 1.0` emits integer
`ADD` instead of `ADDSD`. f32/f64 local variables and array-element
accesses work correctly (different code paths), but state fields do not.

```rust
// src/pipeline.rs:6506–6516 (BUGGY)
Type::BDBase(name) => match name.as_str() {
    "i8"  => IRType::I8,  "i16" => IRType::I16,
    "i32" => IRType::I32, "i64" => IRType::I64,
    "u8"  => IRType::U8,  "u16" => IRType::U16,
    "u32" => IRType::U32, "u64" => IRType::U64,
    _ => IRType::U64,    // ← "f32" and "f64" land here
},
```

**Fix**: add `"f32" => IRType::F32, "f64" => IRType::F64` arms.
**Effort**: 3 days (1-line fix + regression tests + a gold-standard f32-state-field test under `tests/gold_standard/float_*`).
**Unblocks**: every WOMB layout / renderer module that uses f32 coordinates.

### V-35 — `type_size_from_name` returns 8 for layout names

**Severity**: P0 — blocks all nested-struct layouts (e.g. `transform: Transform` as a field).
**File**: `src/parser/src/to_scg.rs:4057–4065`.
**Verified**: yes. Note: the SWE package's `22-review-against-vuma.md`
incorrectly cites this as `scg_to_ir.rs:4057`. The actual location is
`src/parser/src/to_scg.rs:4057`.

```rust
// src/parser/src/to_scg.rs:4057–4065 (BUGGY)
fn type_size_from_name(&self, name: &str) -> u64 {
    match name {
        "u8" | "i8" | "bool" => 1,
        "u16" | "i16" => 2,
        "u32" | "i32" | "f32" => 4,
        "u64" | "i64" | "f64" | "ptr" => 8,
        _ => 8,    // ← "Transform", "Point", "Rect" land here
    }
}
```

A `transform: Transform` field is treated as 8 bytes (not
`sizeof(Transform)`), so every field after it gets a wrong offset. This
is the actual blocker for nested-struct layouts.

**Fix**: make `type_size_from_name` consult the registered layouts table.
When `name` matches a registered layout, return that layout's computed
`total_size`. ~10-line change but load-bearing — every consumer of this
function (notably `is_lossless_cast` at line 4070) needs the corrected
size.
**Effort**: 1 week.
**Unblocks**: every layout that nests another layout as a field (channels
holding protocol state, UI nodes holding `Transform`/`Rect`).

### V-36 — `StateRead`/`StateWrite` hardcoded to `IRType::I64`

**Severity**: P0 — blocks f32 state field access even after V-34 is fixed.
**File**: `src/codegen/src/scg_to_ir.rs:6010–6026` (StateRead Load at
line 6011, StateWrite Store at line 6024).
**Verified**: yes. Note: the SWE package cites `5954–5972`; the actual
hardcoded `ty: IRType::I64` lines are 6011 and 6024 in the current tree.

```rust
// src/codegen/src/scg_to_ir.rs:6002–6013 (StateRead — BUGGY)
P::StateRead { dst, src, layout_name, field_name } => {
    let addr = self.resolve_expr(src, names, ir_func)?;
    let dst_vreg = self.alloc_vreg();
    ir_func.register_vreg(VirtualRegister::named(dst_vreg, dst));
    names.insert(dst.clone(), dst_vreg);
    ir_func.current_block().push(IRInstruction::Load {
        dst: IRValue::Register(dst_vreg),
        addr,
        offset: 0, // placeholder — BD field offset not plumbed here
        ty: IRType::I64,        // ← HARDCODED, should be field's IRType
    });
    let _ = (layout_name, field_name); // diagnostic only
    Ok(())
}
```

The `StateWrite` arm at line 6017 has the same bug. Note also that the
`offset: 0` placeholder means field offsets are not plumbed through this
path at all — the typed-state API currently only works for the first
field of a layout.

**Fix**: thread the field's `IRType` and offset through `PmtOpStmt::StateRead`/`StateWrite` lowering. Use the field's type (resolved via V-34's fixed bridge) and offset (resolved via V-35's fixed size table) instead of hardcoded `I64` and `0`.
**Effort**: 1 week (the threading touches the SCG node payload, the IR builder, and at least one backend's `Load`/`Store` emission path).
**Unblocks**: native f32 state-field loads/stores (no `i64`-load-then-cast workaround).

### V-03 — Legacy `bridge_type_size` still used by `build_pmt_layout_specs`

**Severity**: P0 — same class of bug as V-35 but on the codegen side.
**Files**: `src/pipeline.rs:6532` (legacy `bridge_type_size`) and `src/pipeline.rs:6724` (its only remaining caller, inside `build_pmt_layout_specs`).
**Verified**: yes.

A fixed variant `bridge_type_size_with_layouts` already exists at
`src/pipeline.rs:6551` — it consults a `layout_sizes: &HashMap<String, u64>`
table for user-defined layout names. But `build_pmt_layout_specs` (the
function that constructs the IVE `PmtLayoutSpec` table consumed by the
verifier) still calls the **legacy** `bridge_type_size`, which has
`_ => 8` for any user-defined layout name.

This means the IVE-side layout specs and the codegen-side layout specs
disagree on the size of nested-layout fields. The IVE proof of
field-bounds safety reasons about wrong offsets, so the discharge is
technically unsound for any program that nests a layout as a field of
another layout.

**Fix**: migrate `build_pmt_layout_specs` to a two-pass algorithm —
first compute sizes for all layouts bottom-up, then call
`bridge_type_size_with_layouts(&ftype, &layout_sizes)`.
**Effort**: 1 week.
**Unblocks**: sound IVE discharge on programs with nested layouts.

### V-37 — `build_pmt_layout_specs` alignment handling

**Severity**: P0 (depends on V-03 fix being done correctly).
**File**: `src/pipeline.rs:6715–6735`.

The current `build_pmt_layout_specs` uses `bridge_type_align` (which
correctly handles user types via `_ => 8`) but does not propagate
alignment back into the size table. After V-03 is fixed, the size table
must also include trailing padding to `max_align`, otherwise the last
field of a layout can be followed by unaccounted bytes that the next
layout's first field would land inside.

**Effort**: 2 days (once V-03 lands).
**Unblocks**: correct `sizeof(Layout)` for any layout whose `max_align` > last field's alignment.

---

## P1 — Real gaps (VUMA genuinely lacks these)

### V-26 — Parser lacks const byte arrays / `Expr::ArrayLit`

**Severity**: P1 — blocks SPIR-V embedding, font subsetting, any module
that needs a `.rodata` byte blob.
**Verified**: yes.

The `Lit` enum (`src/parser/src/ast.rs:1511–1525`) has `Int(i64)`,
`Float(f64)`, `String(String)`, `Bool(bool)`, `Address(u64)` — no
`Bytes(Vec<u8>)`, no `Array(Vec<Lit>)`. The `Expr` enum has no
`ArrayLit`/`BytesLit` variant. `parse_primary` has no
`TokenKind::LBracket` arm — a `[` at the start of an expression falls
through to the error arm. The codegen has no `.rodata` lowering for const
byte arrays; const items are lowered as immediate scalars.

**Implication**: there is currently no way to embed a SPIR-V shader
blob, a font subset, or any compile-time byte constant in a VUMA program.
Every consumer uses `Address` of an `extern "C"` symbol instead.

**Fix**:
1. Add `TokenKind::LBracket` arm to `parse_primary` → new
   `Expr::ArrayLit(Vec<Expr>)` variant.
2. Add `Lit::Bytes(Vec<u8>)` or handle `ArrayLit` of `Int` literals as
   bytes.
3. Add codegen path: emit `.rodata` section with bytes, lower the
   expression to a `Load` of the base address.
**Effort**: 2 weeks.

### V-11 — Session types lack `Choice`/`Offer`

**Severity**: P1 — blocks IME channels, any protocol with branching.
**Verified**: yes.

`SessionType` (`src/parser/src/ast.rs:1632–1647` and
`src/codegen/src/ir.rs:167–176`) has only `Send(Box<Type>, Box<SessionType>)`,
`Recv(Box<Type>, Box<SessionType>)`, `End`, `Recurse`. No `Choice`,
`Offer`, `Select`, `Branch`, or `Rec` (recursion is a bare `Recurse`
marker with no body binder).

**Fix**:
1. Add `Choice(Vec<SessionType>)` and `Offer(Vec<SessionType>)` to both
   AST and IR `SessionType` enums.
2. Extend the IVE linear-type checker to handle branching (each branch
   must independently satisfy linearity).
3. Re-prove session-type soundness lemmas in `proof/PMT/IVE/Soundness/SessionType.lean`.
**Effort**: 2 weeks (parser/AST/IR) + 2–4 weeks (IVE checker + Lean
proofs).

### V-16 — Capability signatures use FNV-1a × 4 (not HMAC-SHA256)

**Severity**: P1 — current capability model is unforgeable only against
accidental collision, not adversarial.
**Verified**: yes (per SWE package audit; the `womb/crypto/` module
already has an HMAC-SHA256 implementation that the capability layer
could adopt).

The current capability signature scheme is FNV-1a × 4 (four parallel
FNV-1a hashes with different seeds, concatenated). This is fast and
collision-resistant in the accidental sense, but is not a cryptographic
MAC — an adversary who can observe signatures can construct collisions
without knowing the secret key.

**Fix**: replace the capability signature function with HMAC-SHA256 over
the canonical serialization of the capability token, keyed by a
per-process secret. The `womb/crypto/` module already has an
HMAC-SHA256 implementation; the work is plumbing it into the capability
layer (`src/codegen/src/capability.rs`) and updating the Lean model.
**Effort**: 5 weeks (3 weeks code + 2 weeks Lean proof updates).

### V-14 — f32 PMT Lean proof is greenfield

**Severity**: P1 — defer to v2.
**Verified**: yes.

The Lean model (`proof/PMT/Basic.lean`) is purely memory-safety: `Arena`
uses `Nat` for base/capacity/used; `Field` uses `Nat` for offset/size.
The only arithmetic lemma is `alloc_preserves_capacity`
(`Basic.lean:134–143`), discharged by `omega` — proves
`used + size ≤ capacity`, nothing about value contents. `PmtInstr.lean`
carries `IRType.f32`/`f64` as tag variants (`PmtInstr.lean:186–187`) but
with no semantic content — every arithmetic `PmtInstr` variant is
`True`-well-typed (`PmtInstr.lean:810–820`). `pmt_soundness` proves
capacity preservation + field-bounds safety; says nothing about NaN,
±inf, ULP error, rounding, distributivity, associativity.

The SWE package's original 4-week estimate for "extend the Lean model to
f32" is wrong — there is no existing arithmetic-verification
infrastructure to extend. You'd be building a brand-new `FloatArena`
model, brand-new `verified_float_add` runtime checks, brand-new NaN/inf
trap injection, and a brand-new `float_alloc_preserves_finite` lemma
from scratch.

**Recommendation**: drop the Lean f32 proof for v1. Use runtime
`__float_overflow_trap` (exit 142) checks only — no formal verification
of f32 arithmetic. Document this as a known gap. Revisit in v2 if
verification is critical.
**Effort if pursued**: 3–6 months.

---

## P2 — Cleanup and coverage

### V-13 — SIMD coverage is narrow

**Severity**: P2 — current SIMD is sufficient for v1 but blocks text-shaper acceleration.
**Verified**: yes (per SWE package audit).

x86_64 already has `IRInstr::VectorOp` lowering
(`src/codegen/src/x86_64/stack_slot_isel.rs:3493–3527`): `paddq` (SSE2),
`vpaddq` (AVX), `psubd` (SSE2), `pmulld` (SSE4.1). AArch64 has NEON
encoders (`src/codegen/src/aarch64/mod.rs:3452–3491`). Both backends
support `{Add, Sub, Mul} × {i32, i64}`.

**Gap**: no 256-bit AVX2, no 512-bit AVX-512, no `pmaxsd`/`pminsd`,
no gather, no shuffle. RISC-V V extension is absent. MIPS MXU and
LoongArch LSX are absent.

**Fix**: add wider SIMD encoders for x86_64 first (highest ROI for text
shaping). Defer other arches until a real consumer exists.
**Effort**: 6 weeks.

### V-40 — Legacy `bridge_type_size` coexists with `_with_layouts`

**Severity**: P2 — dead-code risk + footgun.
**File**: `src/pipeline.rs:6532` (legacy) vs `:6551` (`_with_layouts`).
**Verified**: yes.

After V-03 lands and `build_pmt_layout_specs` migrates to
`bridge_type_size_with_layouts`, the legacy `bridge_type_size` will have
zero callers. It should be deleted (or marked `#[deprecated]` with a
migration note) to prevent future regressions.

**Effort**: 1 day (after V-03).

### V-41 — Stale doc references (mostly fixed)

**Severity**: P3.
**Verified**: partially — Wave-1 through Wave-6 commits (f003deec →
6dc97e18) addressed the README, `architecture.md`, `backends.md`,
`caveats.md`, `contributing.md`, `fp_backends.md`, `language-reference.md`,
`pmt-formal-spec.md`, `kernel-*.md`, and
`vuma_orchestrator_ive_faithfulness.md`. Remaining stale refs to verify:

- Anywhere that still cites `regalloc.rs:2307` or `regalloc.rs:2899`
  (correct lines are `:1284` and `:2966` per Wave-2).
- Anywhere that still says "15 of 19 backends" (correct is "18 of 19:
  14 native + 4 wrappers" per Wave-6).
- Anywhere that references `arm64.rs` (correct is
  `aarch64/{mod,reg_isel}.rs` since W7-impl).

**Effort**: 1 day to grep and verify.

---

## V-39 — Test suite health

**Severity**: P1 — a 93.42% pass rate is below the bar for "production-ready."
**Source**: `test_results/failures.txt` and `test_results/summary.json`
(commit `78e71a6b`, run on `pi-pkhairkh-dev`, 2026-07-31 23:46:38 UTC).

**Headline numbers**:
- Total runs: 29963
- Matches: 27992
- Skipped: 0
- Pass rate: 93.42%
- Failures: 1971 across 364 tests

**Per-backend pass rate** (from `summary.json`):
- Strongest: `x86_64` (1543/1577 = 97.8%), `aarch64` (1533/1577 = 97.2%)
- Middle: `riscv64` (1532/1577 = 97.1%), `mips64` (1532/1577 = 97.1%)
- Weakest: `m68k`, `sparc64`, `hppa`, `alpha`, `x86_32` (these account
  for the bulk of the failures)

**Failure mode breakdown** (sampled from `failures.txt`):
- **CR** (crash, exit code -11 / SIGSEGV or -8 / SIGFPE) — concentrated
  in `m68k`, `arm32`, `armeb`, `riscv32`, `x86_32`. Indicates backend
  codegen bugs that produce invalid instruction encodings or trap.
- **MM** (mismatch — wrong numeric result) — spread across all backends.
  Indicates codegen correctness bugs (wrong constant folding, wrong
  shift amount, wrong branch direction).
- **TO** (timeout, exit 124) — concentrated in `aarch64`, `x86_64`,
  `mips64` on long-running tests (`arith_collatz`, `arith_palindrome`,
  `arith_tribonacci`). Indicates either an infinite loop in the
  compiled program or a perf regression.

**Recommendation**:
1. Triage the CR failures first — they are the most likely to indicate
   real codegen bugs (vs. perf issues for TO and edge-case arithmetic
   for MM).
2. Group failures by backend and by test family (e.g. all `m68k` CR
   failures are likely one or two instruction-encoder bugs).
3. Add the missing backends to a "second-tier" CI tier — they don't
   need to block every PR, but they should not be silently broken.
4. The `1d72d296` commit (Wave-0, just before the doc-cleanup waves)
   fixed a non-deterministic phi construction + regalloc liveness bug —
   re-run the full suite on `main` HEAD to see if that fix moved the
   needle.

**Effort**: ongoing — 1–2 weeks of focused triage should get the pass
rate above 97%, after which per-PR gating on the strongest backends
becomes viable.

---

## Out of scope (tracked separately)

The following items appear in the SWE package's `01-vuma-side-changes.md`
but are **not** VUMA-core work. They belong to the WOMB UI engine layer
(written in VUMA, but lives in `womb/ui/`) or to the GPU stack (which is
a greenfield multi-month effort, not a patch).

- **V-01** — `native` backend variant. This is a WOMB/native-host concern;
  the existing 19 backends already produce native binaries. The "native
  UI" variant is really about host imports (Vulkan/Metal/IME/a11y), not
  about a new ISA.
- **V-02** — Rendering IR instructions (`GpuDraw`, `GpuBufferWrite`,
  `ShapeText`, `FontLoad`, `FontParse`, `Animate`). These depend on a GPU
  backend existing (see V-GPU). Adding the IR variants before the
  backend exists would be dead code.
- **V-GPU** — GPU backend infrastructure (Vulkan / Metal / SPIR-V /
  WebGPU). The SWE package's `22-review-against-vuma.md` verified that
  VUMA has **zero** GPU support: no `BackendKind::Vulkan/Metal/WebGPU`,
  no `IRInstr::GpuDraw`, no `Effect::Gpu`, no `Resource::Gpu*`. Building
  this is 3–6 months of greenfield work and should be tracked as its
  own epic, not as a VUMA-side patch.
- **V-08 through V-32** (UI feature patches — `LayoutNode` fields,
  variable fonts, color fonts, TrueType hinting, font subsetting,
  stacking contexts, Knuth-Plass, vertical text, composited scroll,
  animation, theme, path IR). These are all WOMB-layer work; they touch
  VUMA only insofar as they consume the IR/Effect/Resource extensions
  from V-02 (which itself depends on V-GPU).
- **V-27** — Native Metal backend. Same as V-GPU; Metal is one of the
  GPU backends in the greenfield GPU stack.
- **V-28** — Persistent GPU buffer abstraction. Same.

---

## Recommended execution order

1. **V-34** (3 days) — unblocks f32 state fields at the bridge layer.
2. **V-35** (1 week) — unblocks nested-struct layouts at the parser layer.
3. **V-36** (1 week) — unblocks f32 state fields at the IR builder layer.
4. **V-03** (1 week) — unblocks nested-struct layouts at the codegen/IVE layer.
5. **V-37** (2 days) — closes the alignment gap opened by V-03.
6. **V-40** (1 day) — delete the legacy `bridge_type_size`.
7. **V-39** triage (ongoing) — get the test suite above 97%.
8. **V-26** (2 weeks) — unblocks SPIR-V embedding and font subsetting.
9. **V-11** (2–4 weeks) — unblocks IME channels.
10. **V-16** (5 weeks) — close the capability-model security gap.
11. **V-13** (6 weeks) — SIMD coverage for text-shaper acceleration.
12. **V-14** (defer to v2) — f32 PMT Lean proof.
13. **V-41** (1 day) — verify stale doc refs are gone.

Steps 1–6 form a single coherent "bridge fix" epic — total ~3.5 weeks
of focused work, after which f32-coordinate nested-struct state fields
work end-to-end. This is the prerequisite for every WOMB layout and
renderer module.
