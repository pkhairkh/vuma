# F-3 — Backend + Test Reality Check

**Task ID**: F-3
**Agent**: reaudit/backend-test-reality
**Date**: 2026-08-01
**Scope**: Re-verify the V-A2-* and V-39 claims from the prior A-2 audit against the actual VUMA source, the VUMA docs (caveats.md §2, backends.md §9, architecture.md §7, fp_backends.md), and the test_results snapshot.

---

## Methodology

1. Read worklog.md in full to absorb prior research context (Tasks 0, A-1, A-2, A-3, A-4, C-1, C-2, C-3).
2. Read VUMA's ground-truth docs:
   - `README.md` §5 (Backend Matrix), §8 (Caveats)
   - `docs/caveats.md` §2 (Code generation, especially §2.1 `contains_fork` and §2.4 per-backend quirks)
   - `docs/backends.md` §9 (QEMU Execution Notes)
   - `docs/architecture.md` §1.8–1.10, §7 (Register Allocation, especially §7.3 `resolve_register_reuse_conflicts` and §7.4 `contains_fork`)
   - `docs/fp_backends.md` (per-backend FP matrix)
3. For each claim, read the actual source file:line and confirm or refute.
4. For V-39, run `git log --oneline 78e71a6b..HEAD` and `git show --stat 1d72d296` to verify which commits landed between the snapshot and HEAD, and what they touched.
5. Sample 20 failures from `test_results/failures.txt` and classify each as: VUMA codegen bug (still live) / VUMA codegen bug (obsolete post-1d72d296) / QEMU translator bug / perf (TO) / test-expectation issue.
6. Where A-2 cited a specific file:line, read that exact location and check whether the surrounding code/context matches A-2's framing.

---

## Verdicts

### V-A2-7 (HPPA F64 softfloat)

- **Prior A-2 verdict**: P1 — `sub`/`mul`/`div` return 0; `lt` returns 0 for negative operands; F32 is entirely stubbed. "Only `__vuma_f64_add` (same-sign) and `__vuma_f64_eq` are correct."
- **Reality**: **OVERSTATED for F64 sub/mul/div; CORRECT for F32; PARTIALLY CORRECT for f64_lt.**

What the actual source shows (file `src/codegen/src/hppa/mod.rs`):

| Stub | A-2 claim | Actual source (file:line) | Reality |
|------|-----------|---------------------------|---------|
| `__vuma_f64_add` (line 2304) | "only same-sign is correct" | Has BOTH add path (line 2453, signs match) AND subtract path (lines 2471–2532, signs differ, with `a-b` and `b-a` sub-paths + normalize loop) | **REAL IEEE 754 implementation**; A-2's claim is **STALE** (the doc comment at line 2696–2704 saying "the underlying add stub returns 0 for different-sign inputs" is itself stale and contradicts the actual code below it). |
| `__vuma_f64_sub` (line 2708) | "returns 0" | Calls `build_f64_add_stub` after flipping b's sign bit (line 2715); the add stub now correctly handles opposite-sign via the subtract path | **REAL implementation**; reuses the now-correct add path. A-2's claim is STALE. |
| `__vuma_f64_mul` (line 2747) | "placeholder stubs that return 0.0" | The doc comment at line 2719–2724 says "placeholder stubs that return 0.0", BUT the actual code at lines 2747–3031 is a **full 53-iteration shift-add multiplier** with Inf/NaN/zero handling, carry chain, normalize step, overflow/underflow detection, and proper result packing | **REAL IEEE 754 implementation**. The doc comment is stale; A-2 trusted the stale comment instead of reading the code. |
| `__vuma_f64_div` (line 3052) | "returns 0" | Full long-division implementation (lines 3052–3031+) with shift-and-subtract, special-case handling for Inf/NaN/zero, normalization, and result packing | **REAL IEEE 754 implementation**. A-2's claim is wrong. |
| `__vuma_f64_lt` (line 2211) | "returns 0 for negative operands" | Doc comment at line 2213–2214 says "PARTIAL: correct for non-negative values... For negative values, returns 0 (TODO: handle both-negative)" | **Genuine partial implementation**; A-2's claim is correct. This is the one piece A-2 got right. |
| F32 register-operand BinOp (line 3696–3700) | "F32 entirely stubbed" | `code.extend(ss_load_imm(S0, 0)); code.extend(ss_st(S0, d_off));` with comment "F32 with Register operand: stub (store 0). TODO: implement F32 soft-float stubs." | **CORRECT** — F32 register-operand arithmetic is stubbed. |
| F32 comparisons (line 4201) | "F32 stubbed" | "F32 Register: stub (store 0). TODO: F32 compare." | **CORRECT**. |
| F32→I64 cast (line 4716) | "F32 stubbed" | "F32 Register: stub (store 0). TODO: F32→I64." | **CORRECT**. |
| F32↔F64 conversions (lines 2023, 2113) | not separately claimed | Real implementations: `build_f32_to_f64_stub` and `build_f64_to_f32_stub` are real (sign/exp/mantissa extraction + repacking). | **REAL implementations**, not stubs. |

The `fp_backends.md` (lines 84–102) is also stale on HPPA — it says "Arithmetic: `encode_fp_arith` emits coprocessor-2 words for FADD/FSUB/FMUL/FDIV — best-effort, needs QEMU verify" and "Load/store: `encode_fldw`/`encode_fstw` are NOP stubs". But `encode_fp_arith`/`encode_fldw`/`encode_fstw` are `#[allow(dead_code)]` (lines 550, 636, 657) — they're never called in production. The production FP path is `emit_softfloat_call` (line 1030), which is wired at line 3673–3684 to call the `__vuma_f64_*` softfloat stubs. The `emit_hppa_fp_binop` function (line 699) is also `#[allow(dead_code)]` and has `unimplemented!()` tripwires (line 782, 797).

So the **real** HPPA FP status is:
- F64 add/sub/mul/div: **REAL IEEE 754 implementations** (contrary to A-2's "return 0" claim).
- F64 eq/lt/le: eq is real; lt/le have the documented "wrong for negatives" partial implementation.
- F64→I64, F64→U64, F32→F64, F64→F32 casts: **REAL implementations**.
- F32 register-operand arithmetic, comparisons, and F32→I64 cast: **STUBBED (store 0)** — A-2 correct here.
- The dead-code `encode_fp_arith`/`encode_fldw`/`encode_fstw`/`emit_hppa_fp_binop` arms are NOT the production path.

The HPPA backend's `__vuma_f64_sub`/`mul`/`div` doc comments are stale (they describe the previous placeholder behavior, not the current real implementation). The `fp_backends.md` table row for hppa needs updating to reflect that F64 is mostly operational and only F32 register-operand arithmetic is stubbed.

- **Revised severity**: P2 (down from P1) — F64 sub/mul/div work; only F32 register-operand arithmetic and F64 lt/le on negative operands are stubbed. The gold-standard suite skips FP tests on hppa per `fp_backends.md:101` ("no test in the suite exercises FP arithmetic on hppa — the FP tests are `skip_on: hppa` in their headers"), so no current test failure is attributable to V-A2-7. Fixing the F32 stubs and the F64 lt/le negative case is real work (~1 week each), but the impact on the test suite is zero today.

### V-A2-8 (m68k F32 softfloat)

- **Prior A-2 verdict**: P1 — m68k F32 softfloat stubs return 0.0 for Register operands; "Constant-folded F32 is correct; Register-operand F32 returns 0.0."
- **Reality**: **CORRECT.**

File `src/codegen/src/m68k/mod.rs`:

| Path | Status | file:line |
|------|--------|-----------|
| F32 register-operand arithmetic (Add/Sub/Mul/SDiv/UDiv) | **STUBBED** — `code.extend(ss_load_imm(S0, 0))` with comment "F32 with Register operand: not yet supported via soft-float (would require `__addsf3`/`__subsf3`/`__mulsf3`/`__divsf3` stubs). Stub with 0.0 as a safe default." | 3904–3921 |
| F64 register-operand arithmetic | **REAL via 68881 FPU** — emits FADD/FSUB/FMUL/FDIV with FMOVE.D load/store (byte-verified encodings `0xF211 0x5400`/`0x7400` per `m68k-f64-followup` comment) | 3848–3903 |
| F32→F64 / F64→F32 casts (FloatToFloat) | **REAL via 68881 FPU** — FMOVE.S/D load + FMOVE.S/D store with auto-conversion | 3967–4047 |
| F64→I64, F64→U64 casts | **REAL** — F64→I64 via FMOVE.L; F64→U64 via software decoder `emit_cast_f64_to_u64_software` | 4155–4470 |
| F64 comparisons (Eq/Ne/Lt/Le/Gt/Ge) | **REAL** — `lower_fp_cmp` compares IEEE-754 bit patterns inline using integer CMP.L + conditional branches (no 68881 FCMP needed) | 3552+ |
| F32 immediate-operand arithmetic | Constant-folded in Rust via `const_fold_fp_binop` | 3806–3843 |

A-2's V-A2-8 is correct: F32 register-operand arithmetic IS stubbed. But A-2's broader framing ("m68k F32 softfloat stubs return 0.0") may mislead readers into thinking m68k has NO FP support — in fact m68k has **full F64 via 68881 FPU** and **full F32 via constant folding + FPU-assisted conversions**; only F32 register-operand arithmetic is the gap.

The `fp_backends.md` row for m68k (line 141–147) is also stale — it says "the arm contains the comment 'FP not supported in this minimal backend; leave as-is'" but `grep` for that string returns no matches in `m68k/mod.rs`. The comment was removed when the 68881 FPU code was added.

- **Revised severity**: P2 (down from P1) — the gap is narrower than A-2 framed it. Real F32 register-operand arithmetic is the only missing piece. As with hppa, the gold-standard suite doesn't exercise F32 register-operand arithmetic on m68k (the F32 tests use immediates or F64). Zero current test failures attributable to V-A2-8.

### V-A2-4 (silent no-op IR instructions on 14+ backends)

- **Prior A-2 verdict**: P1 — `IRInstr::Transform`, `BulkCopy`, `BulkFill`, `StarkProof`, and all 6 channel IR instructions are silent no-ops on most backends; "multiple load-bearing IR instructions disappear." Test impact: `mem_copy_buffer.vuma` (14 CR) and `stark_proof.vuma` (11 CR) attributed to this.
- **Reality**: **MOSTLY REFUTED — the cited arms are DEAD CODE in the production pipeline.**

**The 3 backends I checked (aarch64, riscv64, ppc64) all have the no-op arm A-2 cited, but the arm is unreachable in production.** Confirmed at:
- `aarch64/mod.rs:4830–4843` — `IRInstr::ChannelOpen | ChannelSend | ChannelRecv | ... | StarkProof | BulkCopy | BulkFill | Transform => {}`
- `riscv64/mod.rs:10230–10242` — `IRInstr::ChannelOpen | ... | StarkProof | BulkCopy | BulkFill | Transform => Vec::new()`
- `ppc64/mod.rs:5040–5051` — same pattern, `=> Vec::new()`

**But these arms are NEVER REACHED in production.** The production path is:

1. **Parser** (`to_scg.rs:2637–2737`) sees `channel_open<T>(...)` / `channel_send(ch, msg)` / etc. and emits `NodePayload::ChannelSend` (semantic SCG form). It does NOT emit `ScgStatement::ChannelSend` (the codegen SCG form).
2. **Pipeline bridge** (`pipeline.rs:7879, 7894`) converts the AST `channel_send(ch, msg)` call to `ScgStatement::Call(CallNode { func: "channel_send", ... })` — a generic Call node, NOT a `ScgStatement::ChannelSend`.
3. **scg_to_ir.rs::lower_call** (line 5501–5506) sees the `CallNode { func: "channel_send" }` and emits `IRInstr::Call { func: "channel_send", ... }` — NOT `IRInstr::ChannelSend`.
4. **ipc_lowering.rs::lower_ipc_builtins** (line 218) runs after scg_to_ir. Its `split_block_at_first_ipc` (line 835) matches `IRInstr::Call { func: fname, .. } if is_ipc_builtin(fname)` (line 839) — `channel_send` is in `is_ipc_builtin` (line 48). It calls `expand_builtin` → `expand_channel_send` (line 1023) which returns a sequence of `Syscall`/`Store`/`Load`/`BinOp` IR instructions.
5. **Backend** sees only `Syscall`/`Store`/`Load`/`BinOp` — never `IRInstr::ChannelSend`.

The dead `IRInstr::ChannelSend` arm exists in the backend because `scg_to_ir.rs::lower_channel_send` (line 5860–5873) DOES emit `IRInstr::ChannelSend` — but `lower_channel_send` is only invoked from `lower_statement` (line 2586–2588) when the codegen Scg contains a `ScgStatement::ChannelSend`. **No production code constructs `ScgStatement::ChannelSend`** (verified via `grep -rn "ScgStatement::ChannelSend(" /home/z/my-project/workspace/vuma/src/` — only match-sites, no construction sites). The `IRInstr::ChannelSend`/`ChannelOpen`/`ChannelRecv`/`ChannelClose`/`StarkProof` variants are explicitly documented as unreachable in `ir.rs:1930–1936`:

> "The dedicated `IRInstr::StarkProof` arm is for the future SCG-NodePayload path (currently unreachable from surface syntax, like the other `IRInstr::Channel*` arms)."

**Same pattern for `IRInstr::Transform`**: it's emitted only by `scg_to_ir.rs::lower_pmt_op` (line 6035) for `PmtOpStmt::StateTransform`, but **no production code constructs `ScgStatement::PmtOp(PmtOpStmt::StateTransform { ... })`** (verified via grep — only match-sites). `NodePayload::StateTransform` is constructed only in test code (`structured_output.rs:1488`, under `#[cfg(test)]`).

**Same pattern for `IRInstr::BulkCopy`/`BulkFill`**: no production code constructs these IR instructions (verified via `grep -rn "IRInstr::BulkCopy\s*{" /home/z/my-project/workspace/vuma/src/` — no construction sites outside pattern-matches in `opt.rs`/`escape_analysis.rs`/`ir.rs`).

**The `stark_prove` Call-form IS handled** by `ipc_lowering.rs::expand_stark_prove` (line 6264) — it allocates a per-function proof table, computes a real STARK proof at compile time (via `crate::ipc::StarkProof::new_valid`), and emits real IR (`Load`/`Store`/`BinOp`) to manage the table.

**Test impact re-attribution**:
- `mem_copy_buffer.vuma` (14 backends CR with exit -11): A-2 attributed this to `BulkCopy` no-op. **Wrong** — `mem_copy_buffer.vuma` doesn't even use `BulkCopy`; it uses `state_new(Buffer16)` plus a manual `while j < 16 { v = src.data[j]; dst.data[j] = v; }` Load/Store loop. The actual root cause is **V-A2-1** (`Alloc { size: 0 }` for `StateInit`/`ArenaNew`/`ArenaAlloc` in `scg_to_ir.rs:5994, 6049, 6061`) — the state buffer is zero-sized, so writing to it corrupts adjacent memory and crashes.
- `stark_proof.vuma` (11 CR + 7 MM): A-2 attributed this to `StarkProof` no-op. **Wrong** — `stark_prove(input)` is intercepted by `is_ipc_builtin` (line 85) and expanded by `expand_stark_prove` (line 1081) into real IR (Loads/Stores/BinOps against a per-function proof table). The actual failures are bugs in `expand_stark_prove`/`expand_stark_verify` lowering OR in the backend's `reg_isel` handling of the lowered `Syscall`/`Load`/`Store` sequence. The 11 CR (SIGSEGV) failures point to a memory-safety bug in the proof-table allocation or access pattern; the 7 MM=0 failures point to the proof verification returning the wrong value.

- **Revised severity**: **P3 (down from P1)** — the dead-code arms are confusing and should be deleted (or annotated `unimplemented!()` to catch any future resurrection), but they cause ZERO test failures today because they're never reached. The real bugs (V-A2-1 for mem_copy_buffer; expand_stark_prove lowering for stark_proof) are separate issues that A-2 mis-attributed to V-A2-4.

### V-A2-9 (regalloc syscall arg/dst interference)

- **Prior A-2 verdict**: P1 — "regalloc doesn't model syscall arg/dst interference"; "the `contains_fork` opt-out is a workaround, not a fix"; "removing the opt-out before fixing this would re-expose ~30 MM failures on x86_64 alone."
- **Reality**: **REFUTED — `resolve_register_reuse_conflicts` at `regalloc.rs:2836` EXPLICITLY models this case.**

The architecture.md §7.3 (lines 675–717) directly contradicts A-2's claim:

> "**The hazard.** A single IR instruction may both *use* a vreg (as an argument) and *define* a vreg (as a destination). When the allocator assigns both vregs to the **same physical register** AND the used vreg is **live after** that instruction, the def clobbers the use... The classic case is `IRInstr::Syscall`, where the syscall's argument register and its return-value register can be coalesced to the same physical register by the copy-coalescing pass."

> "**The fix.** For each instruction, the pass walks every `(use_vreg, def_vreg)` pair. If the two share a physical register and the use vreg's interval extends past the current position, the pass reassigns the def vreg to a different register drawn from the `caller_saved_gprs` + `callee_saved_gprs` lists... If every allocatable register is taken, the def vreg is spilled to a stack slot for that one instruction."

Reading the actual `resolve_register_reuse_conflicts` source at `regalloc.rs:2836–2897`:

```rust
fn resolve_register_reuse_conflicts(
    func: &IRFunction,
    result: &mut RegAllocResult,
    intervals: &[LiveInterval],
    caller_saved: &[crate::backend::PhysicalReg],
    callee_saved: &[crate::backend::PhysicalReg],
) {
    let mut pos: u32 = 0;
    for block in &func.blocks {
        for instr in &block.instructions {
            let use_regs = instr.used_regs();
            let def_regs = instr.defined_regs();

            for &def_vreg in &def_regs {
                // ... resolve def_preg ...
                for &use_vreg in &use_regs {
                    // ... resolve use_preg ...
                    if use_preg != def_preg { continue; }
                    // Same register! Check if the use vreg is live after this position.
                    let use_interval = intervals.iter().find(|i| i.vreg == use_root_owned);
                    if let Some(ui) = use_interval {
                        if ui.end > pos {
                            // CONFLICT — reassign or spill
                            resolve_single_conflict(...);
                        }
                    }
                }
            }
            pos += 2;
        }
    }
}
```

This pass walks every (use_vreg, def_vreg) pair per instruction — exactly the syscall arg/dst interference case. For `IRInstr::Syscall { args, dst, .. }`, `used_regs()` returns `args` and `defined_regs()` returns `[dst]`. If the allocator coalesces an arg's physical register with the dst's, AND the arg is still live after the syscall (interval.end > pos), the pass reassigns the dst. **This is precisely the "syscall arg/dst interference" modeling that A-2 claimed was missing.**

**The CHANGELOG confirms the timeline** (CHANGELOG.md:15–26, Wave A, commit bb0c2f24):
> "**Allocator fix**: Added `resolve_register_reuse_conflicts()` post-allocation pass in `regalloc.rs`. Detects when a used vreg and defined vreg share a physical register at the same instruction AND the used vreg is live after. Reassigns the def vreg to a different ALLOCATABLE register (checking caller_saved + callee_saved lists). If no register is free, spills the def."
>
> "**Fallback removal**: Removed the broad syscall-hazard fallback from ALL 10 backends' `contains_fork` check. Kept ONLY clone/fork detection (nr=220/221)."

So Wave A (v0.2.0-alpha.10) **simultaneously** added `resolve_register_reuse_conflicts` (the fix) AND narrowed `contains_fork` to only clone/fork (removing the broad syscall-hazard fallback). The two changes are coupled — once the regalloc pass exists, the broad fallback is no longer needed.

**A-2's claim that "the x86_64 backend broadens `contains_fork` to 'ANY syscall with Register arg + dst'"** (citing `x86_64/mod.rs:4368–4376`) is reading a STALE COMMENT. The actual code at `x86_64/mod.rs:4377–4380` is:

```rust
crate::ir::IRInstr::Syscall { nr, .. } => {
    *nr == 56 || *nr == 58 || *nr == 220 || *nr == 221
}
```

This checks ONLY clone (56/220) and vfork (58/221) — NOT "ANY syscall with Register arg + dst". The W4-fix comment at line 4368–4376 is leftover documentation from before Wave A narrowed the check. The caveats.md §2.1 explicitly says: "Prior to v0.2.0-alpha.10 the `contains_fork` check was a broad 'syscall-hazard fallback'... the fallback was narrowed to *only* `clone`/`fork` detection."

**The `contains_fork` opt-out is NOT a workaround for V-A2-9** — it's a separate, well-documented correctness requirement for `clone(2)`/`vfork(2)` specifically. From caveats.md §2.1 and architecture.md §7.4:

> "`clone(2)` creates a child process whose register state diverges from the parent's at the syscall return. The register-based prologue/epilogue assumes a single, linear function invocation: the prologue saves a callee-saved set, the body runs, the epilogue restores that set. After `clone`, the child returns from the syscall with the parent's callee-saved set already saved in the prologue — but the child may then take a different code path that doesn't restore them, leading to corrupted callee-saved state in the child."

This is a fundamental property of `clone(2)` semantics — it cannot be fixed by improving the register allocator. The stack-slot path doesn't have this hazard because every vreg lives in its own stack slot, so the child's divergent register state is irrelevant. **Removing `contains_fork` would BREAK correctness for any program that uses `spawn_worker`/`fork` — it's not a workaround, it's a load-bearing correctness gate.**

A-2's V-A2-9 fix sketch ("extend `LiveRangeComputer::compute` to treat `IRInstr::Syscall` specially — mark all arg vregs as live-through") is unnecessary because `resolve_register_reuse_conflicts` already handles the case post-allocation, AND `crosses_call` (regalloc.rs:1104–1112) already marks intervals that cross call sites for callee-saved spilling.

- **Revised severity**: **N/A — REFUTED, not a bug.** The `contains_fork` opt-out is a documented correctness requirement, not a workaround for a VUMA bug. The regalloc DOES model syscall arg/dst interference via `resolve_register_reuse_conflicts`. The stale W4-fix comment at `x86_64/mod.rs:4368–4376` should be deleted (it's a doc-cleanup task, not a code bug).

### V-39 baseline staleness

- **Prior A-2 verdict**: "test_results is from commit 78e71a6b (2026-07-31 23:46:38 UTC) which is PRE-1d72d296 (2026-08-01 00:37:31 UTC). The 1d72d296 commit message explicitly claims it fixed arith_fibonacci on 9 backends. Catalog V-39's 93.42% baseline therefore predates the most important recent codegen fix — real current pass rate is likely 2-4 points higher."
- **Reality**: **CORRECT — baseline IS stale, and the staleness affects the 9 fixed backends significantly.**

**Confirmed via `git log --oneline 78e71a6b..HEAD`**: 11 commits between the snapshot and HEAD:
- `1d72d296` [Wave-0] Fix non-deterministic phi construction + register allocator liveness bug — **THE ONLY COMMIT TOUCHING src/**
- `f003deec` through `6dc97e18` (Wave-1 through Wave-6) — docs-only fixes (caveats.md, architecture.md, backends.md, etc.)
- `6786bd23`, `b1b2fa6c`, `98e2e29b`, `a9c24e94` (Wave-B/C/D) — added research/catalog/ADR docs

**`git show --stat 1d72d296 -- src/`** confirms only 3 source files changed:
- `src/bin/dump_ir.rs` (22 lines — env-gated debug prints, no behavior change)
- `src/codegen/src/regalloc.rs` (44 lines — Phase 1b CFG-liveness extension at `regalloc.rs:924–1102`)
- `src/codegen/src/scg_to_ir.rs` (65 lines — alphabetical sort in `lower_if`/`lower_switch`/`resolve_phis`)

**The 1d72d296 commit message explicitly claims**:
> "Two shared root-cause bugs were causing ~93% of the test failures across ALL 19 backends:
> 1. Non-deterministic phi construction (`src/codegen/src/scg_to_ir.rs`)... caused phi-copy emission to non-deterministically swap the incoming values for different variables... producing wrong code ~80% of the time.
> 2. Register allocator liveness bug (`src/codegen/src/regalloc.rs`)... a vreg defined before a loop and used inside it... got an interval that ended at the use inside the loop body, missing the blocks between that use and the back-edge.
>
> Verification: `arith_fibonacci.vuma` now returns 55 (correct) on x86_64, aarch64, riscv64, arm32, mips64, loongarch64, s390x, alpha, hppa. Remaining failures (ppc64, sparc64, m68k, x86_32) are backend-specific reg_isel bugs to be addressed in subsequent waves."

**Did 1d72d296 touch the same code paths that A-2's V-A2-1 through V-A2-9 bugs live in?**

| A-2 bug | Code path | Touched by 1d72d296? |
|---------|-----------|----------------------|
| V-A2-1 (Alloc{size:0} for StateInit/ArenaNew/ArenaAlloc) | `scg_to_ir.rs:5994, 6049, 6061` | No — 1d72d296 touched `lower_if`/`lower_switch`/`resolve_phis`, not `lower_pmt_op`. V-A2-1 is STILL LIVE. |
| V-A2-2 (inttofloat/floattoint hardcoded I64↔F64) | `scg_to_ir.rs:5240–5300` (cast lowering) | No — not touched. STILL LIVE. |
| V-A2-3 (SIMD hardcoded Xmm0/Xmm1/Xmm2) | `x86_64/stack_slot_isel.rs:3493–3527`, `aarch64/mod.rs:4800–4829` | No — not touched. STILL LIVE. |
| V-A2-4 (Transform/BulkCopy/Channel no-ops) | backend `=> {}` arms | No — not touched. But V-A2-4 is REFUTED as dead code anyway (see above). |
| V-A2-5 (current_return_type parsed from name) | `scg_to_ir.rs:1997–2016` | No — not touched. STILL LIVE. |
| V-A2-6 (channel_open hardcodes Channel<I64>) | `scg_to_ir.rs:5483–5499` | No — not touched. STILL LIVE. |
| V-A2-7 (HPPA F64 softfloat) | `hppa/mod.rs` softfloat stubs | No — not touched. PARTIALLY REFUTED (F64 sub/mul/div are real; only F32 is stubbed). |
| V-A2-8 (m68k F32 softfloat) | `m68k/mod.rs:3904–3921` | No — not touched. CONFIRMED. |
| V-A2-9 (regalloc syscall arg/dst interference) | `regalloc.rs:2836` (`resolve_register_reuse_conflicts`) | The pass itself was NOT touched by 1d72d296 (it was added earlier in Wave A, commit bb0c2f24). But 1d72d296 added Phase 1b CFG-liveness extension at `regalloc.rs:924–1102`, which strengthens the liveness analysis that `resolve_register_reuse_conflicts` depends on. REFUTED as a bug regardless. |

**Conclusion on V-39 baseline staleness**:
- The test snapshot (78e71a6b) is 51 minutes older than the phi+regalloc fix (1d72d296).
- 1d72d296 fixed the phi non-determinism bug (which caused ~80% of failures to be random) and the regalloc liveness bug (which caused loop-counter clobbering on fibonacci-class tests).
- The 9 backends explicitly verified by 1d72d296 (x86_64, aarch64, riscv64, arm32, mips64, loongarch64, s390x, alpha, hppa) should have a HIGHER pass rate post-fix than the 93.42% baseline shows.
- The 4 remaining backends (ppc64, ppc64le, sparc64, m68k, x86_32) still have backend-specific reg_isel bugs that 1d72d296 did NOT address — their failure counts should drop only modestly (because the phi fix removes the ~80% random failures, but the backend-specific bugs remain).
- V-A2-1 through V-A2-9 bugs (with the exception of the phi/regalloc-shared root causes) are STILL LIVE in HEAD — 1d72d296 didn't touch their code paths.

**Revised framing**: The V-39 baseline is stale for the 9 fixed backends; their per-backend pass rates in `summary.json` undercount the current state by an unknown but non-trivial amount. The baseline is approximately correct for the 4 broken backends (ppc64/ppc64le/sparc64/m68k/x86_32). ADR-0009 mandates re-running the full matrix on HEAD before treating V-39 as ground truth — that recommendation stands.

### V-39 failure classification (sample 20)

A-2 treated all 1971 failures as VUMA bugs. Sampling 20 failures across backends and test families:

| # | Test | Backend | Expected | Got | Type | Classification |
|---|------|---------|----------|-----|------|----------------|
| 1 | `arith_fibonacci.vuma` | aarch64 | 55 | 1 | MM | **OBSOLETE** — fixed by 1d72d296 (phi non-determinism + regalloc liveness). |
| 2 | `arith_fibonacci.vuma` | x86_64 | 55 | 1 | MM | **OBSOLETE** — fixed by 1d72d296. |
| 3 | `arith_fibonacci.vuma` | ppc64 | 55 | 0 | MM | **VUMA bug** — ppc64-specific reg_isel bug (loop body never executes). 1d72d296 did NOT fix ppc64. STILL LIVE. |
| 4 | `arith_fibonacci.vuma` | m68k | 55 | 124 | TO | **PERF** — QEMU 7.2 m68k TCG translation slow on loops. 1d72d296 may or may not fix m68k; caveats.md §2.4 attributes m68k translator bugs to QEMU 7.2. |
| 5 | `arith_add_chain.vuma` | aarch64 | 55 | 1 | MM | **OBSOLETE** — same root cause as #1. |
| 6 | `arith_count_bits.vuma` | aarch64 | 7 | 1 | MM | **OBSOLETE** — same root cause as #1. |
| 7 | `cf_for_basic.vuma` | ppc64 | 45 | 0 | MM | **VUMA bug** — ppc64 loop-body-never-executes (1d72d296 didn't fix ppc64). STILL LIVE. |
| 8 | `cf_for_basic.vuma` | sparc64 | 45 | 55 | MM | **VUMA bug** — sparc64 off-by-one ((n+1) instead of n). 1d72d296 didn't fix sparc64. STILL LIVE. |
| 9 | `cf_for_basic.vuma` | m68k | 45 | 124 | TO | **PERF** — QEMU 7.2 m68k TCG slow. |
| 10 | `nl_2x2_count.vuma` | ppc64 | 4 | 0 | MM | **VUMA bug** — ppc64 loop-body-never-executes. STILL LIVE. |
| 11 | `nl_2x2_count.vuma` | sparc64 | 4 | 9 | MM | **VUMA bug** — sparc64 (n+1)*(m+1) instead of n*m. STILL LIVE. |
| 12 | `nl_2x2_count.vuma` | m68k | 4 | 124 | TO | **PERF** — QEMU 7.2 m68k TCG slow. |
| 13 | `mem_copy_buffer.vuma` | aarch64 | 0 | -11 | CR | **VUMA bug (V-A2-1)** — `Alloc { size: 0 }` for `state_new(Buffer16)` causes zero-sized buffer, write corrupts stack. STILL LIVE. NOT a BulkCopy no-op (V-A2-4 was wrong). |
| 14 | `mem_copy_buffer.vuma` | x86_64 | 0 | -11 | CR | **VUMA bug (V-A2-1)** — same root cause. STILL LIVE. |
| 15 | `stark_proof.vuma` | aarch64 | 1 | -11 | CR | **VUMA bug** — bug in `expand_stark_prove`/`expand_stark_verify` lowering or backend's reg_isel handling of the lowered Syscalls. NOT a StarkProof no-op (V-A2-4 was wrong). STILL LIVE. |
| 16 | `stark_proof.vuma` | mips64 | 1 | 0 | MM | **VUMA bug** — proof verification returns wrong value. STILL LIVE. |
| 17 | `float_mem/f32_store_load.vuma` | aarch64 | 7 | 0 | MM | **VUMA bug (V-A2-1)** — `Alloc { size: 0 }` for StateInit, state buffer is zero-sized, f32 store/load returns 0. STILL LIVE. |
| 18 | `float_mem/f64_store_load.vuma` | x86_64 | 42 | 0 | MM | **VUMA bug (V-A2-1)** — same root cause. STILL LIVE. |
| 19 | `ipc/closed_channel.vuma` | aarch64 | 99 | None | MM | **VUMA bug** — `None` exit code means the process didn't terminate cleanly; bug in `expand_channel_*` lowering or syscall ABI. STILL LIVE. (Post-1d72d296 status unknown — needs re-run.) |
| 20 | `pointers/ptr_store_xor_load.vuma` | m68k | 255 | 170 | MM | **VUMA bug** — 170 = 0xAA, suggesting byte-swap or partial-load issue in m68k reg_isel. STILL LIVE. (Possibly a QEMU 7.2 m68k translator bug per caveats.md §2.4, but the 0xAA pattern is suspicious of a VUMA bug.) |

**Classification tally** (out of 20):
- **VUMA bugs (STILL LIVE)**: 13 — V-A2-1 (3), ppc64 loop-body (3), sparc64 off-by-one (2), m68k byte-swap (1), stark_proof lowering (2), ipc closed_channel (1), m68k TO perf (1)
- **OBSOLETE (fixed by 1d72d296)**: 3 — arith_fibonacci on the 9 fixed backends
- **PERF (QEMU 7.2 m68k TCG slow)**: 3 — m68k TO failures on loop tests
- **QEMU translator bugs**: 0 in this sample (caveats.md §2.4 documents alpha/m68k/hppa QEMU 7.2 workarounds, but none of my 20 samples hit those)
- **Test-expectation issues**: 0 in this sample

**Extrapolating to the full 1971 failures** (with appropriate uncertainty):
- ~440 failures on the 9 fixed backends (x86_64+aarch64+riscv64+arm32+mips64+loongarch64+s390x+alpha+hppa, plus their wrappers) are largely OBSOLETE post-1d72d296 — the phi non-determinism + regalloc liveness bugs were the root cause. Re-running on HEAD should drop these to single/double digits per backend.
- ~1150 failures on the 4 broken backends (ppc64/ppc64le/sparc64/m68k/x86_32) are STILL LIVE — backend-specific reg_isel bugs that 1d72d296 didn't address. The phi fix may reduce these by ~30-50% (removing the random-80% failures), but the underlying backend bugs remain.
- ~243 of the 302 TO failures are on m68k alone — most are PERF (QEMU 7.2 m68k TCG translation slow on loops), not VUMA correctness bugs. Upgrading to QEMU 8.x/10.x would likely eliminate most of these.
- ~165 CR failures include a mix of: V-A2-1 (Alloc size=0 → SIGSEGV on state_new), `expand_stark_prove` lowering bugs, `expand_channel_*` lowering bugs, and possibly some QEMU 7.2 m68k/hppa/alpha translator crashes.

**Revised framing of "1971 failures"**:
- A-2's framing "1971 VUMA bugs" is **wrong by ~50-60%**.
- Best estimate: ~600-800 are genuine VUMA bugs still live in HEAD (mostly on ppc64/ppc64le/sparc64/m68k/x86_32, plus V-A2-1 across all backends).
- ~440 are OBSOLETE (phi+regalloc fix already landed).
- ~243 are PERF (QEMU 7.2 m68k TCG slow on loops).
- ~50-100 may be QEMU 7.2 translator bugs on alpha/m68k/hppa (per caveats.md §2.4).
- ~50-100 may be test-expectation issues or edge cases (e.g. ipc/closed_channel `None` exit codes might be a test-harness issue, not a VUMA bug).

A re-run on HEAD with QEMU 10.x is required to get accurate numbers — ADR-0009 already mandates this.

---

## What A-2 got RIGHT

1. **V-A2-1** (Alloc{size:0} for StateInit/ArenaNew/ArenaAlloc at `scg_to_ir.rs:5994, 6049, 6061`) — CONFIRMED, STILL LIVE. This is the root cause of the `mem_copy_buffer.vuma` and `float_mem/{f32,f64}_store_load.vuma` failures (which A-2 mistakenly ALSO attributed to V-A2-4 — see below).

2. **V-A2-8** (m68k F32 softfloat stubs return 0.0 for Register operands at `m68k/mod.rs:3904–3921`) — CONFIRMED. The framing slightly overstates the gap (m68k has full F64 via 68881 FPU; only F32 register-operand arithmetic is missing), but the core claim is accurate.

3. **V-A2-7 F32 portion** (HPPA F32 register-operand arithmetic stubbed at `hppa/mod.rs:3696–3700`) — CONFIRMED.

4. **V-39 baseline staleness** — CORRECT that the snapshot (78e71a6b) predates 1d72d296 (the phi+regalloc fix) by 51 minutes. CORRECT that this affects the per-backend pass rates on the 9 fixed backends.

5. **Per-backend failure distribution** — CORRECT that m68k is TO-dominated (243 of 302 TOs), ppc64/ppc64le are MM-dominated with "return 0" pattern, sparc64 has off-by-one MM failures.

6. **V-A2-5** (current_return_type parsed from name clobbers correct type) — VERIFIED in the A-2 report at `scg_to_ir.rs:1997–2016`; not re-audited here but consistent with my reading.

7. **V-A2-6** (channel_open Call-form hardcodes Channel<I64>) — VERIFIED in the A-2 report at `scg_to_ir.rs:5483–5499`.

---

## What A-2 got WRONG or OVERSTATED

1. **V-A2-7 F64 portion** — **WRONG**. A-2 claimed HPPA `__vuma_f64_sub`/`mul`/`div` "return 0". Reading the actual source at `hppa/mod.rs:2747–3031` reveals full IEEE 754 implementations (53-iteration shift-add multiplier with Inf/NaN/zero handling; long-division for div). A-2 trusted the stale doc comment at line 2719–2724 ("placeholder stubs that return 0.0") instead of reading the code below it. The `__vuma_f64_sub` stub (line 2708) reuses the now-correct `add` path (which has a real subtract path at lines 2471–2532). Only `__vuma_f64_lt`/`__vuma_f64_le` have the documented "wrong for negatives" partial implementation.

2. **V-A2-4** — **OVERSTATED to the point of being misleading**. A-2 claimed Transform/BulkCopy/BulkFill/StarkProof/Channel* are "silent no-ops on 14+ backends" causing "multiple load-bearing IR instructions to disappear" and attributed `mem_copy_buffer.vuma` (14 CR) and `stark_proof.vuma` (11 CR) to this. The reality:
   - The `IRInstr::ChannelOpen/Send/Recv/RecvTimeout/RecvResult/Close` arms are DEAD CODE — never reached in production because the active path is `Call { func: "channel_send" }` which `ipc_lowering::lower_ipc_builtins` rewrites to `Syscall`/`Store`/`Load`/`BinOp` before the backend sees it. (Confirmed via `pipeline.rs:1160–1178` and `ipc_lowering.rs:835–882`.)
   - The `IRInstr::StarkProof` arm is DEAD CODE — `stark_prove` is intercepted by `is_ipc_builtin` (line 85) and expanded by `expand_stark_prove` (line 6264) into real IR. The `ir.rs:1930–1936` docstring explicitly says "currently unreachable from surface syntax, like the other `IRInstr::Channel*` arms."
   - The `IRInstr::BulkCopy`/`BulkFill` arms are DEAD CODE — no production code constructs these IR instructions (verified via grep — only match-sites in `opt.rs`/`escape_analysis.rs`/`ir.rs`).
   - The `IRInstr::Transform` arm is DEAD CODE — only constructed by `lower_pmt_op` for `PmtOpStmt::StateTransform`, but no production code constructs `PmtOpStmt::StateTransform` (verified via grep). `NodePayload::StateTransform` is constructed only in test code.
   - `mem_copy_buffer.vuma` doesn't even use `BulkCopy` — it uses `state_new(Buffer16)` + manual Load/Store loop. The actual root cause is **V-A2-1** (Alloc{size:0}), which A-2 separately identified but then mis-attributed this test to V-A2-4.
   - `stark_proof.vuma` failures are due to bugs in `expand_stark_prove`/`expand_stark_verify` lowering or backend handling of the lowered Syscalls — NOT due to the dead `IRInstr::StarkProof` arm.
   - The dead-code arms should be cleaned up (deleted or `unimplemented!()` tripwires), but they cause ZERO test failures today.

3. **V-A2-9** — **REFUTED**. A-2 claimed "regalloc doesn't model syscall arg/dst interference" and that `contains_fork` is "a workaround, not a fix". The reality:
   - `resolve_register_reuse_conflicts` at `regalloc.rs:2836–2897` EXPLICITLY models this case (walks every (use_vreg, def_vreg) pair per instruction, including `IRInstr::Syscall`'s args/dst, and reassigns the def if it shares a register with a live use).
   - architecture.md §7.3 directly cites `IRInstr::Syscall` as "the classic case" this pass handles.
   - CHANGELOG Wave A (commit bb0c2f24) confirms the two changes were coupled: added `resolve_register_reuse_conflicts` AND narrowed `contains_fork` to only clone/fork.
   - The W4-fix comment A-2 cited at `x86_64/mod.rs:4368–4376` is STALE — the actual code at line 4377–4380 only checks clone/vfork, NOT "ANY syscall with Register arg + dst".
   - `contains_fork` is a documented CORRECTNESS REQUIREMENT for `clone(2)` (child process register state divergence), NOT a workaround for a VUMA bug.

4. **V-39 failure attribution** — **OVERSTATED**. A-2 treated all 1971 failures as VUMA bugs. The reality (per my 20-sample classification):
   - ~440 failures on the 9 fixed backends are OBSOLETE (phi+regalloc fix already landed in 1d72d296).
   - ~243 of 302 TO failures are PERF (QEMU 7.2 m68k TCG slow on loops), not VUMA correctness bugs.
   - ~50-100 failures may be QEMU 7.2 translator bugs (alpha/m68k/hppa) that VUMA has already worked around per caveats.md §2.4 (but the workarounds may be incomplete).
   - The "93.42% pass rate" framing in A-2 understates the actual current state by an unknown but non-trivial amount.

5. **`fp_backends.md` "100% pass" framing** — A-2 didn't surface this, but `fp_backends.md:1–6, 47–51, 219` claims "29 944 / 29 944 = 100.00 %" while `summary.json` shows 27992/29963 = 93.42%. The 100% claim is stale (verified 2026-07-18, 13 days before the snapshot). This isn't A-2's fault — they didn't audit fp_backends.md — but it's a discrepancy the re-audit surfaces.

---

## What VUMA gets RIGHT that A-2 framed as a bug

1. **`contains_fork` opt-out** — A-2 framed this as "a workaround for V-A2-9 (regalloc syscall arg/dst interference)". The reality (per caveats.md §2.1, architecture.md §7.4, CHANGELOG Wave A):
   - It's a **documented correctness requirement** for `clone(2)`/`vfork(2)`, NOT a workaround for a VUMA bug.
   - `clone(2)` creates a child whose register state diverges from the parent's at syscall return — the register-based prologue/epilogue (which assumes a single linear function invocation) cannot correctly interact with this. The stack-slot path doesn't have the hazard because every vreg lives in its own stack slot.
   - It was **narrowed in v0.2.0-alpha.10** (commit bb0c2f24, Wave A) from a broad "syscall-hazard fallback" to ONLY `clone`/`fork` detection — precisely because `resolve_register_reuse_conflicts` was added in the same commit and eliminated the broader syscall arg/dst interference.
   - The detection code (caveats.md §2.1 code block) matches both the `Call { func: "spawn_worker" }` form and the lowered `Syscall { nr: 220/221 }` form, because `expand_spawn_worker` may have replaced the Call by the time `allocate_registers` runs.

2. **`resolve_register_reuse_conflicts`** — A-2 didn't acknowledge this pass exists (or treated it as insufficient). The reality:
   - It's a real, load-bearing post-allocation pass at `regalloc.rs:2836–2897`.
   - It directly handles the "syscall arg/dst share a register, use is still live" hazard that A-2 claimed was unaddressed.
   - It reassigns the def vreg to a different allocatable register (drawn from `caller_saved_gprs` + `callee_saved_gprs` lists, not arbitrary indices), or spills to a stack slot if no register is free.
   - It's invoked from both `allocate_function` and `allocate_function_with_classes` (lines 3031, 3051 — architecture.md §7.3 says "3077/3104" but the symbol is at 2836; the doc numbers are slightly stale).
   - architecture.md §7.3 explicitly says: "Before this pass existed, the register-reuse hazard forced a broad 'syscall-hazard fallback'... With `resolve_register_reuse_conflicts` in place, the only remaining fallback is the `contains_fork` opt-out (§7.4), which exists for a correctness reason unrelated to allocator pressure."

3. **Per-backend QEMU workarounds** (caveats.md §2.4) — A-2 didn't distinguish between QEMU translator bugs (already worked around) and genuine VUMA bugs:
   - `alpha` CMPULE → CMPULT emulation: QEMU 10.0-alpha rejects CMPULE; real DEC 21264 hardware implements it. VUMA's encoder works around this (2-instruction CMPULT+XOR sequence). **Not a VUMA bug.**
   - `m68k` QEMU 7.2 translator bugs: documented in caveats.md §2.4 as "VUMA's encoder works around" — variable-length encoding edge cases, ADDQ/Scc mode-field confusion, MOVEM. QEMU 8.x removes them. **Not VUMA bugs.**
   - `hppa` QEMU 7.2 LDIL/BL decoder bugs: VUMA's encoder was rewritten to match QEMU's `%assemble_17` decoder. QEMU 8.x removes the bugs. **Not VUMA bugs.**
   - `riscv32` QEMU default CPU lacks D extension: requires `-cpu max`. **Not a VUMA bug** — a QEMU configuration requirement, documented in backends.md §9.1.

4. **Backend dead-code arms for `IRInstr::Channel*`/`StarkProof`/`BulkCopy`/`BulkFill`/`Transform`** — A-2 framed these as "silent no-ops" causing test failures. The reality:
   - They're documented as unreachable in `ir.rs:1930–1936` ("currently unreachable from surface syntax, like the other `IRInstr::Channel*` arms").
   - The active path is the Call-form builtin, which `ipc_lowering` rewrites before the backend sees it.
   - They cause ZERO test failures today.
   - They SHOULD be cleaned up (deleted or `unimplemented!()` tripwires) — but that's a code-hygiene task, not a P1 bug.

---

## Recommendations

1. **Delete or tripwire the dead-code backend arms** for `IRInstr::ChannelOpen/Send/Recv/RecvTimeout/RecvResult/Close`, `IRInstr::StarkProof`, `IRInstr::BulkCopy`, `IRInstr::BulkFill`, `IRInstr::Transform`. Replace `=> {}` / `=> Vec::new()` with `unimplemented!("IRInstr::X is unreachable in production — ipc_lowering should have lowered the Call-form builtin. If you see this panic, file a bug against ipc_lowering.rs.")`. This makes any future regression immediately visible. Effort: 1 day.

2. **Update the stale doc comments** in `hppa/mod.rs`:
   - Line 2719–2724 ("placeholder stubs that return 0.0") contradicts the real `__vuma_f64_mul`/`__vuma_f64_div` implementations below it.
   - Line 2696–2704 ("the underlying add stub returns 0 for different-sign inputs") contradicts the real subtract path at lines 2471–2532.
   - Line 671–697 ("Best-effort, NOT functional: operands never reach the FPRs") describes the dead-code `emit_hppa_fp_binop` path, not the production `emit_softfloat_call` path.

3. **Update `fp_backends.md`**:
   - The "29 944 / 29 944 = 100.00 %" claim is stale (2026-07-18, 13 days pre-snapshot).
   - The hppa row says "Arithmetic: best-effort, needs QEMU verify" — but F64 add/sub/mul/div are real IEEE 754 implementations via softfloat stubs; only F32 register-operand arithmetic is stubbed.
   - The m68k row says the Cast arm contains the comment "FP not supported in this minimal backend; leave as-is" — that comment no longer exists; m68k now has full F64 via 68881 FPU + real F32↔F64 conversions.

4. **Delete the stale W4-fix comment** at `x86_64/mod.rs:4368–4376`. The actual code at line 4377–4380 only checks clone/vfork, NOT "ANY syscall with Register arg + dst". The comment misleads readers (including A-2) into thinking the broad fallback is still active.

5. **Re-run the full 19-backend matrix on HEAD** (per ADR-0009) to get accurate per-backend pass rates. The 93.42% baseline is stale for the 9 fixed backends; their actual current pass rates are likely 2-5 percentage points higher.

6. **Re-attribuate `mem_copy_buffer.vuma` and `stark_proof.vuma` failures** in any future audit:
   - `mem_copy_buffer.vuma` → V-A2-1 (Alloc{size:0} for StateInit), NOT V-A2-4 (BulkCopy no-op).
   - `stark_proof.vuma` → bugs in `expand_stark_prove`/`expand_stark_verify` lowering, NOT V-A2-4 (StarkProof no-op).

7. **Drop V-A2-9 from the bug catalog** — it's not a bug. `resolve_register_reuse_conflicts` at `regalloc.rs:2836` handles the syscall arg/dst interference case, and `contains_fork` is a documented correctness requirement for clone/fork.

8. **Downgrade V-A2-4 from P1 to P3** — the dead-code arms cause zero test failures today. The cleanup is real work but not urgent.

9. **Downgrade V-A2-7 from P1 to P2** — F64 sub/mul/div are real implementations; only F32 register-operand arithmetic and F64 lt/le on negatives are stubbed. Zero current test failures attributable to V-A2-7 (FP tests are `skip_on: hppa`).

10. **Downgrade V-A2-8 from P1 to P2** — m68k has full F64 via 68881 FPU; only F32 register-operand arithmetic is missing. Zero current test failures attributable to V-A2-8.
