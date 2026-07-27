# FFI Faithfulness Gap Audit (Post-Verification)

**Audit date**: Post-FFI-2-A audit of FFI pillar claims vs. actual implementation.
**Auditor**: FFI orchestrator (main agent).
**Scope**: Cross-check every claim in the FFI orchestrator spec and `proof/AUDIT_FFI.md`
against the actual implementation in `src/` and `proof/`.

## Methodology

For each claim in the FFI spec, I verified:
  1. The Rust implementation matches the claim.
  2. The Lean model matches the Rust implementation (model faithfulness).
  3. The audit report (`proof/AUDIT_FFI.md`) accurately describes the state.

## Summary of gaps found

| # | Severity | Gap | Affected claim |
|---|----------|-----|----------------|
| 1 | **HIGH** | 5 active extern calls remain in `src/pipeline.rs` AST-bridge path: `mmap`, `mprotect`, `mremap`, `munmap`, `__arena_overflow` | "No foreign function calls in VUMA (No-FFI path)" |
| 2 | **HIGH** | Lean `SyscallName` allowlist has 6 syscalls; Rust `SyscallName` enum has 16+ | "6 syscalls routed through IRInstr::Syscall" |
| 3 | **HIGH** | Lean `NoExterns` predicate does NOT check `.syscall` instructions | `ffi_pillar_sound` Conjunct 2 is vacuously true |
| 4 | **HIGH** | ~~Lean `PmtInstr` is missing `bulk_copy`, `bulk_fill` (FFI-1-A additions)~~ **CLOSED by FFI-3-A** | Lean cannot model programs using memcpy/memset replacements |
| 5 | **HIGH** | ~~Lean `PmtInstr.transform` signature differs from Rust `IRInstr::Transform`~~ **CLOSED by FFI-3-B** | Lean `transform` ≠ Rust `Transform` (different fields) — distinct variants now coexist
| 6 | **MEDIUM** | `proof/AUDIT_FFI.md` falsely claims "zero active extern block declarations" | Audit report inaccuracy |
| 7 | **MEDIUM** | `proof/AUDIT_FFI.md` falsely claims `is_extern: true` is only for user-source `extern "C"` blocks | Audit report inaccuracy |
| 8 | **LOW** | `__vuma_state_*` / `__vuma_arena_*` prefix-guard in `x86_64/mod.rs:4343-4353` is dead code | Cleanup needed |
| 9 | **LOW** | `src/codegen/src/runtime/pmt_ops.rs` (8 functions + `__oob_trap`) is dead code in production | Cleanup needed |
| 10 | **LOW** | `proof/AUDIT_FFI.md` claims `__oob_trap` is "used by codegen Transform lowering"; it is not | Audit report inaccuracy |
| 11 | **INFO** | Lean `PmtInstr` is missing 4 control-flow variants (`invoke`, `resume`, `switch`, `tail_call`) — CLOSED by FFI-3-C | Pre-existing model gap (not FFI-introduced) |

---

## Gap #1 — 5 active extern calls remain in pipeline.rs (HIGH)

**Claim** (from spec & `proof/AUDIT_FFI.md`):
> "No foreign function calls in VUMA (No-FFI path)."
> "There are zero active `extern "C" { fn ... }` extern block declarations remaining in `src/` for VUMA's own use."

**Reality**: `src/pipeline.rs` actively emits 5 distinct extern calls from the AST-bridge path that are NOT routed through `IRInstr::Syscall`:

| Function | Pipeline.rs lines | Reaches backend as extern? |
|----------|-------------------|----------------------------|
| `mmap` | 11590 | Yes — `IRInstr::Call { is_extern: true }` |
| `mprotect` | 11614, 11844 | Yes |
| `mremap` | 11821 | Yes |
| `munmap` | 11893 | Yes |
| `__arena_overflow` | 11735 | Yes |

(`AtomicLoad`/`AtomicStore`/`AtomicCas` are also emitted as `CallNode { is_extern: true }` but `scg_to_ir.rs::lower_call` intercepts them and emits proper `IRInstr::AtomicLoad/Store/Cas` — so those don't reach the backend as externs.)

**Implication**: A VUMA program that uses the arena (any program with `state_new(Layout)` or `arena_new(cap)`) emits `mmap`/`mprotect`/`munmap` as extern calls. These are NOT in the Lean `NoExterns.builtin_callees` list and NOT in the Lean `SyscallName.allowlist`. Such a program would **violate** `NoExterns P` and thus fail the FFI pillar's hypothesis — meaning `ffi_pillar_sound` says nothing about realistic VUMA programs.

**Fix**: Route these 5 externs through `IRInstr::Syscall` (matching the Lean model's allowlist) OR add them to the Lean `SyscallName.allowlist`. The Rust `SyscallName` enum already has `Mmap`, `Munmap`, `Mprotect`, `Brk` — but the Lean side doesn't have `Mprotect` or `Mremap`.

---

## Gap #2 — Lean vs Rust syscall allowlist mismatch (HIGH)

**Claim** (from spec & `proof/AUDIT_FFI.md`):
> "6 syscalls (write, read, exit, mmap, munmap, brk) routed through IRInstr::Syscall"

**Reality**:
- Lean `proof/PMT/FFI/PillarSoundness.lean` `SyscallName` enum: **6 variants** (Write, Read, Exit, Mmap, Munmap, Brk).
- Rust `src/ffi.rs` `SyscallName` enum: **16+ variants** (Read, Write, Open, Close, Exit, ExitGroup, Mmap, Munmap, Brk, Ioctl, Fcntl, Getpid, Kill, Mprotect, ClockGettime, ...).

**Implication**: The Lean `ffi_pillar_sound` theorem's `SyscallName.allowlist` is a strict undercount of what the actual Rust runtime permits. A VUMA program could call `ioctl(...)`, `kill(...)`, or `mprotect(...)` via `IRInstr::Syscall` and the Lean model would not flag it as an FFI violation. The Lean theorem's "every syscall callee is in the allowlist" claim is unverifiable against reality because the Lean allowlist doesn't match the Rust allowlist.

**Fix**: Either (a) trim the Rust `SyscallName` enum to 6 variants (matching Lean) and route the others through different mechanisms, or (b) extend the Lean `SyscallName` to mirror the Rust enum (16+ variants).

---

## Gap #3 — Lean NoExterns does not check .syscall instructions (HIGH)

**Claim**: `ffi_pillar_sound` Conjunct 2: "Every syscall callee is in the `SyscallName.allowlist`."

**Reality**: The Lean `NoExterns` predicate's match arms are:
```lean
match i with
| .call name _ => name ∈ builtin_callees
| .call_indirect _ _ => False
| _ => True   -- ← .syscall falls through to True
```

Actual syscalls in VUMA go through `IRInstr::Syscall { nr, args, dst }` (a separate Rust IR variant), which corresponds to `PmtInstr.syscall : Nat → List IRValue → Option IRValue → PmtInstr` in Lean. The Lean `NoExterns` predicate does NOT check this variant.

The `ffi_pillar_sound` Conjunct 2 checks `.call name _` instructions where `name ∈ syscall_callees`. But syscalls don't appear as `.call` instructions in the Lean model — they appear as `.syscall nr args dst`. So Conjunct 2's antecedent `name ∈ syscall_callees` is **never satisfied** for any realistic program. Conjunct 2 is **vacuously true**.

**Implication**: A VUMA program with `syscall(99, ...)` (an arbitrary kernel syscall number outside the allowlist) satisfies `NoExterns P` and `ffi_pillar_sound`'s conclusion — even though it actually executes an arbitrary kernel syscall.

**Fix**: Extend `NoExterns` to check `.syscall nr _ _` instructions, requiring `nr ∈ SyscallName.allowlist` (mapped to syscall numbers). And rewrite `ffi_pillar_sound` Conjunct 2 to range over `.syscall` instructions, not `.call` instructions.

---

## Gap #4 — Lean PmtInstr missing BulkCopy/BulkFill (HIGH) — CLOSED by FFI-3-A

**Status**: **CLOSED by FFI-3-A**. Lean `PmtInstr` now has `bulk_copy : IRValue → IRValue → IRValue → PmtInstr` and `bulk_fill : IRValue → IRValue → IRValue → PmtInstr` variants, field-for-field mirrors of Rust `IRInstr::BulkCopy { dst, src, len }` and `IRInstr::BulkFill { dst, val, len }`. Both variants flatten to `[]` in `PmtInstr.to_steps` (opaque memory writes; no PMT arena interaction) — proven by `PmtInstr.to_steps_bulk_copy` and `PmtInstr.to_steps_bulk_fill` in `PMT/ExecFunction.lean`. The exhaustive `cases i with` blocks in `PMT/ExecFunction.lean` (§1.8, §1.9) and `PMT/FFI/PillarSoundness.lean` (§2) were updated with the 2 new arms. `lake build` passes with zero new sorries.

**Claim** (from spec):
> "memcpy → IRInstr::BulkCopy, memset → IRInstr::BulkFill"

**Reality**: FFI-1-A added `IRInstr::BulkCopy { dst, src, len }` and `IRInstr::BulkFill { dst, val, len }` to the Rust IR. But the Lean `PmtInstr` model was never updated to include corresponding variants. FFI-1-A's worklog flagged this with a COORDINATION note asking PMT to add them — no PMT wave ever did.

**Implication**: The Lean `IRProgram` model **cannot represent** any VUMA program that uses `memcpy` or `memset` (which are now lowered to `BulkCopy`/`BulkFill`). For such programs, there's no Lean-side `IRProgram` translation, so `no_ffi_program_sound` and `ffi_pillar_sound` cannot be applied.

The `no_ffi_program_sound` theorem's hypothesis `P : IRProgram` requires a Lean representation of P — but if P uses `BulkCopy`/`BulkFill`, no such representation exists. The theorem is vacuous for such programs.

**Fix**: Add `PmtInstr.bulk_copy : IRValue → IRValue → IRValue → PmtInstr` and `PmtInstr.bulk_fill : IRValue → IRValue → IRValue → PmtInstr` to Lean, with `to_steps` lemmas (likely both flatten to `[]` like the other memory ops, or to a sequence of Load/Store steps). Then update `NoExterns.builtin_callees` if needed.

---

## Gap #5 — Lean PmtInstr.transform signature differs from Rust IRInstr.Transform (HIGH) — **CLOSED by FFI-3-B**

**Claim** (from spec):
> "StateTransform → IRInstr::Transform (NEW variant)"

**Reality**:
- Rust `IRInstr::Transform { dst: IRValue, src: IRValue, from_layout: String, to_layout: String }` (FFI-1-C addition) — 4 fields, two layout names.
- Lean `PmtInstr.transform : String → String → Layout → PmtInstr` (pre-existing PMT-1-A variant) — 3 args, one Layout.

These are **different instructions**:
- Lean's `transform` is a single-layout state transform (in_var, out_var, layout) — pre-existing.
- Rust's `Transform` is a two-layout reinterpretation (dst, src, from_layout, to_layout) — FFI-1-C addition.

The Lean model never received the FFI-1-C `Transform` variant. The pre-existing `transform` is a different concept.

**Implication**: Same as Gap #4 — Lean cannot represent programs using `IRInstr::Transform`, so `no_ffi_program_sound` cannot be applied to them.

**Fix**: Add a new `PmtInstr.transform_layouts : IRValue → IRValue → String → String → PmtInstr` (or rename the existing `transform` and add a new one) to mirror the Rust `Transform`.

**Status (FFI-3-B, closed)**: Added `PmtInstr.transform_layouts : IRValue → IRValue → String → String → PmtInstr` to `proof/PMT/PmtInstr.lean` — a field-for-field mirror of Rust `IRInstr::Transform { dst: IRValue, src: IRValue, from_layout: String, to_layout: String }` (4 fields, same order). The pre-existing `PmtInstr.transform : String → String → Layout → PmtInstr` (PMT-1-A) was left UNTOUCHED — it remains a distinct variant for the single-layout state transform (`__vuma_state_transform`). Added `.effect = .none`, `.well_typed = True`, `.to_steps = []` arms (the Rust lowering is a pointer copy with no PMT arena state interaction). Added `PmtInstr.to_steps_transform_layouts` reflection lemma (closes by `rfl`), and added case arms in the two exhaustive `cases i with` blocks in `proof/PMT/ExecFunction.lean` (`to_steps_preserves_WF_Layout` §1.8 and `to_steps_op_transform` §1.9) and in the exhaustive `cases i with` block of `ffi_pillar_sound` in `proof/PMT/FFI/PillarSoundness.lean`. `lake build` PASS, 108/108 steps, zero new sorries.

---

## Gap #6 — AUDIT_FFI.md false claim: "zero active extern block declarations" (MEDIUM)

**Claim** (from `proof/AUDIT_FFI.md`):
> "There are zero active `extern "C" { fn ... }` extern block declarations remaining in `src/` for VUMA's own use. The only externals remaining are the syscall ABI (routed through `IRInstr::Syscall`, documented as residual TCB)."

**Reality**: 5 active extern calls remain in `src/pipeline.rs` (Gap #1). They are emitted as `CallNode { is_extern: true, func: "mmap" / "mprotect" / "mremap" / "munmap" / "__arena_overflow" }` from the AST-bridge path. These reach the backend as `IRInstr::Call { is_extern: true }` — actual extern calls in the compiled binary.

**Fix**: Either (a) fix the implementation (Gap #1) or (b) correct the audit report to acknowledge the remaining externs.

---

## Gap #7 — AUDIT_FFI.md false claim about is_extern usage (MEDIUM)

**Claim** (from `proof/AUDIT_FFI.md`):
> "The `is_extern: true` value is now only used for explicit `extern "C" { fn ... }` blocks in user VUMA source — the 8 PMT/arena ops no longer set it."

**Reality**: 9 active `is_extern: true` emissions remain in `src/pipeline.rs` (Gap #1 lists 5 that reach the backend as externs; the other 4 are AtomicLoad/Store/Cas which `lower_call` intercepts). These are compiler-emitted, not user-source.

**Fix**: Correct the audit report.

---

## Gap #8 — Dead prefix-guard in x86_64/mod.rs (LOW)

**Claim**: The `__vuma_state_*` / `__vuma_arena_*` prefix-guard at `src/codegen/src/x86_64/mod.rs:4343-4353` was a "stop-the-bleeding" measure for FFI-0-A. FFI-1-C removed the emission of these symbols (they're now `PmtOp`).

**Reality**: After FFI-1-C, no VUMA program emits `__vuma_state_*` / `__vuma_arena_*` symbols. The prefix-guard never fires. It's dead code.

**Fix**: Remove the prefix-guard. The fallback-to-warning logic for other unresolved externs can remain (with the PMT-specific guard stripped out).

---

## Gap #9 — Dead pmt_ops.rs in production (LOW)

**Claim** (from FFI-1-C deprecation): "The 8 functions in this module are kept as reference implementations of the runtime semantics ... but they are no longer invoked by the codegen pipeline."

**Reality**: Confirmed — `grep -rn "pmt_ops::" src/` returns no production-code references (only docstring mentions). The 8 deprecated functions and `__oob_trap` are dead code in production binaries. They're still compiled into the binary (taking up space) and their unit tests still run.

**Fix**: Delete `src/codegen/src/runtime/pmt_ops.rs` entirely. The `__oob_trap` function is not needed in Rust form because every backend emits its own `__oob_trap` stub at the codegen level (`xor eax, eax; ret` etc.). Update `runtime/mod.rs` to remove `pub mod pmt_ops;`.

---

## Gap #10 — AUDIT_FFI.md false claim about __oob_trap (LOW)

**Claim** (from FFI-1-C worklog & `proof/AUDIT_FFI.md`):
> "The `__oob_trap` function (used by the codegen `Transform` lowering for runtime size-mismatch traps) is NOT deprecated."

**Reality**: The `Transform` lowering in `src/codegen/src/x86_64/stack_slot_isel.rs:4419-4428` does a simple pointer copy with NO trap call:
```rust
IRInstr::Transform { dst, src, .. } => {
    let mut code = Vec::new();
    code.extend(load_value(src, Gpr::Rax));
    if let Some(dst_id) = dst.as_register() {
        code.extend(store_vreg(dst_id, Gpr::Rax));
    }
    instr_opcode = Some("transform".to_string());
    code
}
```

The comment claims "If sizes differed at runtime, we would call `__oob_trap`; the SCG layer forbids that case so no runtime check is emitted." — but no `__oob_trap` call is emitted even defensively. And even if it were, it would be a codegen-emitted stub (per backend), not the Rust `pmt_ops::__oob_trap` function.

**Fix**: Either (a) emit a defensive `__oob_trap` call in the Transform lowering (using the codegen stub mechanism, not Rust `pmt_ops`), or (b) correct the comment and audit report.

---

## Gap #11 — Lean PmtInstr missing 4 control-flow variants (INFO) — CLOSED by FFI-3-C

**Status**: CLOSED by FFI-3-C. The 4 control-flow variants
(`invoke`, `resume`, `switch`, `tail_call`) have been added to Lean
`PmtInstr` as field-for-field mirrors of the Rust
`IRTerminator::{Invoke, Resume, Switch, TailCall}` variants. Each
new variant flattens to `[]` under `PmtInstr.to_steps` (control flow
is resolved at the CFG level, not as PMT `Step`s — same precedent as
`branch` / `cond_branch` / `phi` from PMT-1-B). `effect = .none`,
`well_typed = True`. Reflection lemmas `to_steps_switch` /
`to_steps_invoke` / `to_steps_tail_call` / `to_steps_resume` are
`rfl`-closed; the exhaustive `cases i with` blocks in
`PmtInstr.to_steps_preserves_WF_Layout` (§1.8),
`PmtInstr.to_steps_op_transform` (§1.9), and `ffi_pillar_sound`
Conjunct 1 (FFI-owned) all received the 4 new arms. `lake build`
passes sorry-free.

**Pre-existing gap** (not FFI-introduced):
- Rust `IRInstr` has `Invoke`, `Resume`, `Switch`, `TailCall` variants (4 control-flow instructions, likely for exception handling and tail-call optimization).
- Lean `PmtInstr` does not model these.

**Implication**: Programs using these 4 instructions cannot be represented in Lean. This is a pre-existing PMT model gap, not introduced by FFI work — but it does mean the FFI pillar's `no_ffi_program_sound` theorem cannot be applied to such programs.

**Fix**: Out of FFI scope. PMT orchestrator should add these variants if needed.

---

## Combined implications

The FFI orchestrator spec's claim that "**FFI pillar is 100 % mathematically verified**" is **overstated**. The accurate statement is:

> The FFI pillar's two theorems (`no_ffi_program_sound` and `ffi_pillar_sound`) are proven sorry-free, but they apply only to VUMA programs whose IR uses only the variants modeled in Lean `PmtInstr`. Programs using `BulkCopy`, `BulkFill`, `Transform`, `Invoke`, `Resume`, `Switch`, or `TailCall` cannot be represented in Lean and thus cannot be verified. Furthermore, the Lean `NoExterns` predicate does not check `.syscall` instructions, so the FFI pillar does not actually constrain the syscall ABI to the 6-syscall allowlist (the Rust runtime permits 16+ syscalls). Finally, 5 extern calls (`mmap`, `mprotect`, `mremap`, `munmap`, `__arena_overflow`) remain in active use in `src/pipeline.rs`, contradicting the "no foreign function calls" claim.

## Recommended follow-up waves

To genuinely close the FFI pillar:

1. **FFI-3-A (model sync)**: Add `bulk_copy`, `bulk_fill`, `transform_layouts` (the FFI-1-C `Transform` variant) to Lean `PmtInstr`. Update `NoExterns.builtin_callees` if needed.
2. **FFI-3-B (NoExterns strengthening)**: Extend Lean `NoExterns` to check `.syscall nr _ _` instructions, requiring `nr` corresponds to an entry in `SyscallName.allowlist`.
3. **FFI-3-C (syscall allowlist sync)**: Either trim Rust `SyscallName` to match Lean (6 variants) or extend Lean to match Rust (16+ variants). Document which side is canonical.
4. **FFI-3-D (pipeline.rs AST-bridge cleanup)**: Route the 5 remaining externs (`mmap`, `mprotect`, `mremap`, `munmap`, `__arena_overflow`) through `IRInstr::Syscall` (or add them to `NoExterns.builtin_callees` if they should be treated as built-in traps).
5. **FFI-3-E (dead code cleanup)**: Remove `pmt_ops.rs` entirely; remove the `__vuma_*` prefix-guard from `x86_64/mod.rs`.
6. **FFI-3-F (audit report correction)**: Update `proof/AUDIT_FFI.md` to accurately reflect the remaining gaps.

Until these land, the FFI pillar should be described as **"conditionally verified"** (conditional on the model gaps documented above), not "100 % verified".
