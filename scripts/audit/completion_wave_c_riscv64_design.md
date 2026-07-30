# Wave C — riscv64 Register-Based Emitter Design

**Task ID:** CC-a-audit
**Wave:** C (VUMA Regalloc Completion run)
**Prior-run context:**
- R2-a-audit (`c3e7413b`, `scripts/audit/regalloc_endianness_wave2_x86_64_design.md`) produced the x86_64 register-based emitter design doc (568 lines, 10 sections). This document is the riscv64 equivalent, following the same template.
- F2-a-audit (`7083e1c7`, `scripts/audit/followup_wave2_emit_regalloc_design.md` §4 row 4) classified riscv64 as **LOW** readiness for `emit_function_regalloc` wire-up — it needs a *new* register-based emitter (the existing `Emitter::emit_function_regalloc` at `emit.rs:1056` is aarch64-only). F2-a estimated 2–4 weeks for a human developer.
- Wave-1 R1-b-impl/R1-b2-fix/R1-b3-fix (`4c6b8524`, `6a8dbd42`, `8194337b`) on aarch64 established the wire-up pattern this design follows.
- CA-a-test (`15a13de6`) confirmed aarch64 (and aarch64_be via delegation) at 29/30 regalloc + 30/30 stack-slot.
- CB-a-investigate (`1ccefa6b`) root-caused the 1 remaining aarch64 edge case (`try_recv`) to a CSEL operand swap in `emit_ir_instr` Select/CtSelect arms — a per-emit-arm bug, NOT an allocator bug. Equivalent risk exists on riscv64's `Select` lowering and is flagged in §7.10.

**Scope of this document:** produce the design CC-b-impl will follow.

**Files audited (READ-ONLY):**
- `src/codegen/src/riscv64.rs` (13714 LOC) — `Gpr` enum (`:70-103`), `Fpr` enum (`:251-284`), `Instruction` enum (`:511+`), `RiscV64Backend::emit_function_regalloc` (`:4162-4174`, metadata-only), `try_real_regalloc` (`:6542-6571`), `RiscV64Backend::allocate_registers` (`:6607-10311`, inline stack-slot ISel), prologue (`:6818-6855+`), `ss_load_imm` (`:5461`), `ss_load_value` (`:5842`), `ss_emit_slot_addr` (`:5868`), typical `Add` arm (`:7243-7261`).
- `src/codegen/src/emit.rs` (9516 LOC) — `Emitter::emit_function` (`:959`), `emit_function_regalloc` (`:1056`, aarch64-only — confirmed: takes `&AllocationResult`, seeds aarch64 `Register::X0..X7`, uses `STP`/`LDP`), `emit_function_stack_slot` (`:3496`).
- `src/codegen/src/regalloc.rs` (6594 LOC) — `LinearScanAllocator` (`:1214`, aarch64-hardcoded), `TargetAgnosticRegAlloc` (`:2742`, target-agnostic), `AllocationResult` (`:480`, aarch64), `RegAllocResult` (`:3224`, target-agnostic), `GenericSpillSlot` (`:3318`), `GenericSpillCode` (`:3355`), `verify_callee_saved` (`:4860`, aarch64-only), `gen_spill_reload` (`:3145`, uses `PhysicalReg::new(class, 0)` as scratch — RISC-V x0/Zero hazard, see §7.5).
- `src/codegen/src/target_desc.rs` (3018 LOC) — `riscv64_target_desc()` (`:1562-1700`), `frame_pointer()` builder (`:1140-1143`, does NOT set `is_allocatable = false` — same latent bug as x86_64 RBP, see §6 gap 1).
- `src/codegen/src/stack_slot_isel.rs` (baseline path reference, not directly used by riscv64 — riscv64 has its own inline ISel).
- `scripts/audit/regalloc_endianness_wave2_x86_64_design.md` — R2-a-audit x86_64 design doc (template).
- `scripts/audit/followup_wave2_emit_regalloc_design.md` — F2-a design doc §4/§5 (risk assessment).

---

## 1. Current riscv64 Emission Path (stack-slot ISel)

**Entry point:** `RiscV64Backend::allocate_registers` at `riscv64.rs:6607`. The body is **~3,700 lines** of inline stack-slot ISel, ending at `:10311`. This is structurally identical to x86_64's `stack_slot_isel.rs` (4,512 LOC) — but lives inline in the backend module rather than in a sibling file. The final 6 lines:

```rust
        // Step 2: run the real target-agnostic linear-scan allocator and,
        // on success, annotate the AllocatedFunction with its decisions.
        if let Some(alloc) = try_real_regalloc(func) {
            crate::regalloc_emit::annotate_with_regalloc(&mut allocated, &alloc);
        }

        Ok(allocated)
    }
```
(`riscv64.rs:10302-10311`)

**Observations:**

1. **Bytes are stack-slot only.** Every vreg gets an 8-byte stack slot at `[S0 - offset]` (negative FP-relative; `vreg_stack_slots: HashMap<u32, i32>` populated at `:6665-6671`). Each instruction loads its operands into a fixed set of scratch registers (`T0`, `T1`, `T2`, `T3`, `T4`, `T5`), performs the op, and stores the result back. See the `Add` arm at `:7243-7261` (typical pattern: `ss_load_value(lhs, slots, T0); ss_load_value(rhs, slots, T1); encode Add { rd: T0, rs1: T0, rs2: T1 }; ss_store_to_slot(T0, dst_offset)`).

2. **`try_real_regalloc` already works.** At `:6542-6571`, the riscv64 backend constructs `TargetAgnosticRegAlloc::new(target)` (target = riscv64 TargetDesc) and calls `allocate_function(func)` to obtain a `RegAllocResult`. Returns `Some` on success, `None` on failure (with `vuma_log!(debug, ...)`). The result is currently used **only for metadata annotation** of the stack-slot bytes — the encoded bytes do not honour `vreg_to_preg` / `spill_code` / `used_callee_saved`.

3. **`RiscV64Backend::emit_function_regalloc` is metadata-only.** At `:4162-4174`, this method runs `self.allocate_registers(func)?` (the stack-slot ISel) first, then annotates with `RegAllocResult` via `regalloc_emit::annotate_with_regalloc`. It does **not** change bytes. Per the F2-a design doc §7.5 (mirrored in x86_64 §1.3), this method should be renamed `emit_function_with_regalloc_metadata`; the `emit_function_regalloc` name should be reserved for the byte-changing path that CC-b-impl will introduce.

4. **Per-function structural invariants are embedded in the prologue.** The stack-slot ISel's prologue (`:6673-6816` for slot computation; `:6818-6855+` for emission) does far more than `addi sp, sp, -frame_size; sd ra, fs-8(sp); sd s0, fs-16(sp); addi s0, sp, fs`:
   - Computes a 32-byte capability-grant signature at compile time and emits 4 × `sd` stores of the FNV-1a×4 sig chunks (slot `cap_sig_off`, 32 bytes).
   - Populates a 160-byte `cap_siginput_off` byte vector + its 8-byte length slot.
   - Pre-loads the formal-verify folded-check counter into `formal_verify_count_off` (8 bytes).
   - Zeroes the channel sequence counter (`seq_counter_off`), protocol-state slot (`proto_state_off`), circuit-breaker state (`cb_state_off`).
   - Zeroes IRQ routing table (`irq_table_off`, 128 bytes), hot-swap version table (`hotswap_table_off`, 128 bytes), STARK proof table (`stark_table_off`, 224 bytes) + their count slots.
   - The register-based emitter must either (a) replicate this prologue verbatim, or (b) extract it into a shared `emit_prologue_common()` helper. Option (b) is preferred — see §5.5.

5. **No `contains_fork` opt-out exists on riscv64 today.** The Wave-1 `contains_fork` fork-detection helper was added to `AArch64Backend::allocate_registers` (`backend.rs:3226-3238`) but **not** to `RiscV64Backend::allocate_registers`. Because riscv64's byte path is currently stack-slot (no callee-saved prologue beyond `sd ra; sd s0`, no register-state preservation across the clone syscall), this has been harmless. Once riscv64 gains a register-based emitter with `sd s1; sd s2; ...` callee-saved saves, the same fork hazard surfaces and riscv64 needs its own `contains_fork` — but with **the SAME Linux/RISC-V syscall numbers as aarch64** (`clone=220`, `vfork=221` — RISC-V uses the generic Linux unified syscall ABI, unlike x86_64 which uses 56/58). The existing inline `spawn_worker` arm at `riscv64.rs:8070-8077` already emits `clone=220` via `Addi { rd: A7, rs1: Zero, imm: 220 }`, confirming the syscall-number convention.

6. **riscv64 does NOT dispatch through `Emitter::emit_function`.** The riscv64 backend's `allocate_registers` is entirely self-contained — it never touches `emit.rs`'s `Emitter`. This is unlike aarch64, which goes through `Emitter::emit_function(func, None)` → `emit_function_stack_slot` (as the comment at `emit.rs:875-893` documents, the `Emitter` is hard-coded `BackendKind::AArch64`). The riscv64 register-based emitter will similarly live in the `riscv64.rs` module (proposed: split off into `src/codegen/src/riscv64/reg_isel.rs` to keep `riscv64.rs` from growing further — it is already 13,714 LOC), not in `emit.rs`.

7. **No env-var gate exists today.** `grep VUMA_REAL_REGALLOC riscv64.rs` returns no matches. The metadata annotation at `:10306-10308` runs unconditionally on every function. CC-b-impl must introduce `VUMA_REAL_REGALLOC_RISCV64` (default off) gating the byte-changing path; the metadata annotation can stay unconditional.

---

## 2. riscv64 Register File (RISC-V LP64D calling convention)

Source: `target_desc.rs:1562-1700` (`riscv64_target_desc()`) and `riscv64.rs:70-150` (`Gpr` enum), `:251-330` (`Fpr` enum).

### 2.1 Integer register file (32 GPRs, x0–x31)

| Index | ABI Name | Reg | ABI Role                       | Callee-saved? | Allocatable? | Notes |
|------:|----------|-----|--------------------------------|---------------|--------------|-------|
| 0     | zero     | x0  | Hardwired zero                 | (special)     | ❌            | Reads = 0; writes discarded |
| 1     | ra       | x1  | Return address                 | (special)     | ❌            | `link_register()` sets non-allocatable |
| 2     | sp       | x2  | Stack pointer                  | (special)     | ❌            | `stack_pointer()` sets non-allocatable |
| 3     | gp       | x3  | Global pointer                 | (special)     | ❌            | `.not_allocatable()` explicit |
| 4     | tp       | x4  | Thread pointer                 | (special)     | ❌            | `.not_allocatable()` explicit |
| 5     | t0       | x5  | Temp 0                         | ❌ caller     | ✅            | |
| 6     | t1       | x6  | Temp 1                         | ❌ caller     | ✅            | |
| 7     | t2       | x7  | Temp 2                         | ❌ caller     | ✅            | |
| 8     | s0 / fp  | x8  | Saved 0 / Frame pointer        | ✅ callee     | ⚠️ BUG        | See §6 gap 1 — `frame_pointer()` does NOT set `is_allocatable = false` |
| 9     | s1       | x9  | Saved 1                        | ✅ callee     | ✅            | |
| 10    | a0       | x10 | Arg 0 / Return 0               | ❌ caller     | ✅            | `.arg(0).return_reg()` |
| 11    | a1       | x11 | Arg 1 / Return 1               | ❌ caller     | ✅            | `.arg(1).return_reg()` |
| 12    | a2       | x12 | Arg 2                          | ❌ caller     | ✅            | `.arg(2)` |
| 13    | a3       | x13 | Arg 3                          | ❌ caller     | ✅            | `.arg(3)` |
| 14    | a4       | x14 | Arg 4                          | ❌ caller     | ✅            | `.arg(4)` |
| 15    | a5       | x15 | Arg 5                          | ❌ caller     | ✅            | `.arg(5)` |
| 16    | a6       | x16 | Arg 6                          | ❌ caller     | ✅            | `.arg(6)` |
| 17    | a7       | x17 | Arg 7 / Syscall number         | ❌ caller     | ✅            | `.arg(7)` — also used for syscall number |
| 18    | s2       | x18 | Saved 2                        | ✅ callee     | ✅            | |
| 19    | s3       | x19 | Saved 3                        | ✅ callee     | ✅            | |
| 20    | s4       | x20 | Saved 4                        | ✅ callee     | ✅            | |
| 21    | s5       | x21 | Saved 5                        | ✅ callee     | ✅            | |
| 22    | s6       | x22 | Saved 6                        | ✅ callee     | ✅            | |
| 23    | s7       | x23 | Saved 7                        | ✅ callee     | ✅            | |
| 24    | s8       | x24 | Saved 8                        | ✅ callee     | ✅            | |
| 25    | s9       | x25 | Saved 9                        | ✅ callee     | ✅            | |
| 26    | s10      | x26 | Saved 10                       | ✅ callee     | ✅            | |
| 27    | s11      | x27 | Saved 11                       | ✅ callee     | ✅            | |
| 28    | t3       | x28 | Temp 3                         | ❌ caller     | ✅            | |
| 29    | t4       | x29 | Temp 4                         | ❌ caller     | ✅            | |
| 30    | t5       | x30 | Temp 5                         | ❌ caller     | ✅            | |
| 31    | t6       | x31 | Temp 6                         | ❌ caller     | ✅            | |

**Caller-saved GPRs available for allocation (13):** t0, t1, t2, a0, a1, a2, a3, a4, a5, a6, a7, t3, t4, t5, t6 — 15 if we count all of t0-t6 (7) plus a0-a7 (8) = 15 caller-saved allocatable GPRs. (Confirms `Gpr::is_callee_saved()` at `riscv64.rs:158-174` which lists only S0–S11 as callee-saved.)

**Callee-saved GPRs available for allocation (12):** s0, s1, s2, s3, s4, s5, s6, s7, s8, s9, s10, s11 — but s0 (x8) is the frame pointer and must be excluded (§6 gap 1), leaving **11 effective callee-saved GPRs**.

### 2.2 Floating-point register file (32 FPRs, f0–f31)

| Index | ABI Name | Reg | ABI Role                  | Callee-saved? |
|------:|----------|-----|---------------------------|---------------|
| 0–7   | ft0–ft7  | f0–f7   | FP temps                  | ❌ caller     |
| 8–9   | fs0–fs1  | f8–f9   | FP saved                  | ✅ callee     |
| 10–17 | fa0–fa7  | f10–f17 | FP args 0–7 / FP return   | ❌ caller     |
| 18–27 | fs2–fs11 | f18–f27 | FP saved                  | ✅ callee     |
| 28–31 | ft8–ft11 | f28–f31 | FP temps                  | ❌ caller     |

**Caller-saved FPRs (20):** ft0–ft7 (8), fa0–fa7 (8), ft8–ft11 (4).
**Callee-saved FPRs (12):** fs0, fs1, fs2–fs11.

`Fpr::is_callee_saved()` at `riscv64.rs:332-345` correctly lists f8, f9, f18–f27.

### 2.3 Calling convention summary

| Aspect                     | Value                                  |
|----------------------------|----------------------------------------|
| ABI name                   | LP64D (RV64GC with double-precision FP)|
| Integer arg registers      | a0, a1, a2, a3, a4, a5, a6, a7 (in order) |
| FP arg registers           | fa0, fa1, fa2, fa3, fa4, fa5, fa6, fa7 |
| Integer return             | a0 (and a1 for 16-byte structs)        |
| FP return                  | fa0 (and fa1 for 16-byte structs)      |
| Stack alignment at `call`  | 16 bytes                               |
| Callee-saved GPRs          | s0–s11 (s0 = FP)                       |
| Callee-saved FPRs          | fs0–fs11                               |
| Link register              | ra (x1) — saved/restored explicitly    |
| Branch delay slots         | (none)                                 |
| TOC pointer                | (none — gp exists but unused by Linux ABIs) |
| Stack frame leaf           | `addi sp, sp, -N; sd ra, N-8(sp); sd s0, N-16(sp); addi s0, sp, N` |
| Stack frame epilogue       | `ld ra, N-8(sp); ld s0, N-16(sp); addi sp, sp, N; ret` |
| Syscall nr register        | a7 (x17)                               |
| Syscall arg registers      | a0, a1, a2, a3, a4, a5 (note: 6 args, NOT 7 like x86_64) |
| Syscall return             | a0                                     |
| Syscall numbers (clone/vfork) | 220 / 221 — SAME as aarch64 (generic Linux unified syscall ABI) |
| Instruction size           | Fixed 32-bit (4-byte aligned)          |
| Immediate field            | 12-bit signed (-2048..2047) for I-type and S-type |

### 2.4 Three-operand ISA — major simplification vs x86_64

RISC-V arithmetic is **three-operand** (`add rd, rs1, rs2` computes `rd = rs1 + rs2` without clobbering either source). This is the **same as aarch64** and **unlike x86_64's two-operand form**. The register-based emitter can directly emit `Add { rd: dst_gpr, rs1: lhs_gpr, rs2: rhs_gpr }` whenever dst, lhs, rhs are all in registers, **without** the extra `mov dst, lhs` that x86_64 §2.4 / §7.6 requires. This is the single biggest reason the riscv64 emitter is simpler than x86_64's: no two-operand coalescing problem, no `eliminated_copies` field needed in `RegAllocResult`.

### 2.5 Fixed 32-bit instruction encoding — no REX, no variable-length

RISC-V instructions are **always 4 bytes** (base ISA; compressed extension RVC would allow 2-byte forms, but the existing `Instruction::encode()` at `riscv64.rs` produces 4-byte words only). Implications:

- No REX-prefix hazard (unlike x86_64 §2.5).
- No variable-length-instruction fall-through risk (unlike x86_64 §7.2).
- `Instruction::encode()` returns `Vec<u8>` of length 4*N (where N = number of logical instructions, e.g. `lui`+`addi` sequence for a 64-bit immediate is 8 bytes).
- All existing `encode_*` helpers in `riscv64.rs` (the `Instruction` enum's `impl`) already handle the 12-bit immediate range split into `lui`+`addi` pairs (see `ss_load_imm` at `:5461`, which uses `Lui` + `Addi` for large constants).

### 2.6 Zero register (x0) — special case

RISC-V's x0 is **hardwired to zero**: reads always return 0, writes are silently discarded. The register-based emitter must:

- **Never** allocate a vreg to x0 (the `TargetDesc` marks x0 non-allocatable via `hardwired_zero()`, so this is enforced at the allocator level — confirmed at `target_desc.rs:1565`).
- **Never** use x0 as a spill/reload scratch (a `sd Zero, [s0+off]` is a no-op). The `TargetAgnosticRegAlloc::gen_spill_reload` at `regalloc.rs:3151` uses `PhysicalReg::new(class, 0)` as the scratch for spill/reload code annotation — on riscv64, GPR index 0 IS x0/Zero. The emitter MUST NOT honor this `preg` field literally; it must substitute a real scratch (T0 or T1) and consult `alloc.vreg_to_preg[vreg]` for the actual location. See §7.5.
- Use `Zero` as the implicit source for `addi rd, zero, imm` (constant materialisation) and `beq rs1, zero, offset` (compare-to-zero) — these are the **intended** uses and are correct.

### 2.7 No condition codes — comparison patterns

RISC-V has **no NZCV condition flags** (unlike aarch64). Comparisons are encoded as register values:

- `slt rd, rs1, rs2` (set-less-than, signed) — `rd = (rs1 < rs2) ? 1 : 0`.
- `sltu rd, rs1, rs2` (unsigned).
- `slti`/`sltiu` for immediate forms.
- `beq`/`bne`/`blt`/`bge`/`bltu`/`bgeu` branch directly on register values.

The existing `emit_cmp_isel` helper at `riscv64.rs:4196-4279` builds the correct patterns (e.g. `Eq` → `XOR rd, rs1, rs2; SLTIU rd, rd, 1`). The register-based emitter inherits these helpers verbatim.

**Implication for `IRInstr::Select`:** the aarch64 CSEL bug (CB-a-investigate `1ccefa6b`) was an operand-swap in `CSEL` semantics (`if cond then Rd=Rn else Rd=Rm`). RISC-V's equivalent — `XOR`+`SLT` to materialise the cond into a 0/1 register, then `BEQ`/`BNE` to branch — is **branch-based**, not predicated. The register-based `Select` arm will emit a small 3-instruction sequence (compare → branch → mv). There is no CSEL-equivalent operand-swap hazard, but there IS an equivalent hazard: the branch-target order must put the `true_val` move on the path that executes when `cond` is true. See §7.10.

---

## 3. What `emit_function_regalloc` Needs to Do for riscv64

The Wave-1 aarch64 wire-up added 6 things to `AArch64Backend::allocate_registers` (see `backend.rs:3175-3300`). The riscv64 wire-up needs the **same 6 things**, but every concrete encoder call is ISA-specific:

1. **Env-var gate.** Read `VUMA_REAL_REGALLOC_RISCV64` (default off). When unset, run today's stack-slot path. When set, attempt the register-based path.

2. **Fork opt-out.** Detect functions containing `IRInstr::Call { func: "spawn_worker"|"fork" }` OR `IRInstr::Syscall { nr: 220|221 }` (Linux/RISC-V clone/vfork — **same numbers as aarch64**, unlike x86_64 which uses 56/58). For these, fall back to stack-slot — the register-based prologue's callee-saved `sd s1; sd s2; ...; ld ...; ld s1` doesn't interact correctly with `clone()` because the child process runs with a different register state and may take a different code path. (Same hazard as Wave-1 R1-b2-fix on aarch64.)

3. **Run the allocator.** Call `try_real_regalloc(func)` (already exists at `riscv64.rs:6542`). On `Some(alloc)`, proceed; on `None`, fall back to stack-slot.

4. **(Optional) Callee-saved verifier.** If `VUMA_VERIFY_CALLEE_SAVED=1`, run a verifier analogous to `regalloc::verify_callee_saved` (`regalloc.rs:4860`) — but parameterized for riscv64 (caller-saved = t0-t6 + a0-a7 + ft0-ft7 + fa0-fa7 + ft8-ft11; callee-saved = s0-s11 + fs0-fs11; always-allowed = Zero, RA, SP). The existing `verify_callee_saved` is hard-coded to aarch64's `PhysReg::Gpr(Register)` with `r.encoding()` checked against X0–X18 / X19–X28 / X29 / X30 / X31 — **it cannot be called on a `RegAllocResult`** (different `PhysicalReg` type: `crate::backend::PhysicalReg { class, index }` vs aarch64's `regalloc::PhysReg::Gpr(Register)`). See §5.3 for the new riscv64 verifier.

5. **Emit register-based bytes.** Call the new `riscv64::reg_isel::allocate_registers(func, &alloc)` (proposed module, see §5). This produces an `AllocatedFunction` whose `encoded` bytes honour `alloc.vreg_to_preg` (operands stay in registers across instructions where possible), `alloc.spill_code` (boundary spills/reloads), and `alloc.used_callee_saved` (prologue `sd s1, ...; sd s2, ...; ...` / epilogue `ld ...; ld s2, ...; ld s1`).

6. **Fall back on allocator failure.** If `try_real_regalloc` returns `None`, or if the new `reg_isel::allocate_registers` returns `Err`, run today's inline stack-slot ISel (the current body of `allocate_registers` at `riscv64.rs:6607-10311`). This preserves the existing safety guarantee.

**Out of scope for CC-b-impl (deferred to a separate PR):**
- The `EmitResult` API change proposed in F2-a §7.2 (returning `frame_size` + `callee_saved` from the emitter). Non-breaking addition but touches all callers; not needed for correctness, only for debug/unwind info accuracy.
- The `emit_function_regalloc` rename proposed in F2-a §7.5. Cosmetic; can be done in a separate cleanup PR.

---

## 4. Reusable Components From aarch64's `emit_function_regalloc`

The aarch64 implementation at `emit.rs:1056-1354` is **aarch64-only** at the byte level — it uses `Register::X0..X30`, `Instruction::SUB/STP/ADD` (aarch64 enums), `compute_frame_size`, `emit_callee_saved_saves`, `emit_spill_reload`, `emit_terminator_regalloc`, `emit_ir_instr` (the aarch64 greedy emitter). None of these concrete calls port to riscv64.

What IS reusable is the **structural pattern**:

| aarch64 component (location)                                 | riscv64 analogue (proposed)                                 | Reuse level |
|--------------------------------------------------------------|------------------------------------------------------------|-------------|
| Allocator result consumed (`AllocationResult`, aarch64)      | `RegAllocResult` (target-agnostic, already produced)       | ✅ Direct — `RegAllocResult` already has `vreg_to_preg`, `spill_slots`, `total_spill_slots`, `used_callee_saved`, `spill_code`, `coalesced_map`. The fields map 1:1. |
| Position-based spill insertion (`pos += 2` per instr, `spill_code.get(&pos)` for pre-instr, `&(pos+1)` for post-instr) | Same `pos += 2` convention | ✅ Direct — `LiveRangeComputer::compute` (regalloc.rs:863) is shared by both allocators, so positions match. |
| Callee-saved prologue sequence (`emit_callee_saved_saves`)   | `sd s1, fs-N(sp); sd s2, fs-N-8(sp); ...` (one `sd` per callee-saved reg, in increasing index order) | 🔄 Pattern — same idea, different bytes. RISC-V uses scalar `sd` (no STP pair-store). |
| Callee-saved epilogue (`emit_terminator_regalloc`)           | `ld s11, ...; ld s10, ...; ...; ld s1, ...` (reverse order of prologue) | 🔄 Pattern |
| Copy-elision skip (`is_eliminated_copy`, `emit.rs:1256`)     | Skip `IRInstr::Cast { kind: BitCast }` whose src & dst resolve to same `PhysicalReg` via `get_phys_reg` | 🚧 Adapted — `RegAllocResult` has no `eliminated_copies` field (only `AllocationResult` does, see regalloc.rs:500). Use `alloc.get_phys_reg(src) == alloc.get_phys_reg(dst)` check directly. (Simpler than x86_64's adaptation because RISC-V's three-operand form means coalescing is a pure optimisation, not a correctness requirement.) |
| Param-vreg preassignment (X0–X7, `emit.rs:1086-1102`)        | a0–a7 (8 integer arg regs) — must NOT be overridden by `alloc.vreg_to_preg` | 🔄 Pattern — same hazard documented in `emit.rs:1112-1141` (R1-b-impl fix). Reuse the param-vreg skip-set logic verbatim, swapping in the riscv64 arg register set (a0–a7 = x10–x17). |
| Spill-slot frame layout (`spill_area_aligned + callee_saved_size`, `emit.rs:1155-1180`) | Same two-region layout: `[spill area]` ← S0-relative negative offsets; `[callee-saved save area]` lives in the prologue's `sd` slots | 🔄 Pattern — but riscv64's callee-saved save area lives **above** the `sd ra; sd s0` save pair (between SP and the ra/s0 save area on entry), unlike aarch64's STP-into-SP-decremented-area which lives *below* SP. See §5.4 for the riscv64 frame layout. |
| Verifier hook (`verify_callee_saved`, regalloc.rs:4860)      | New `verify_callee_saved_riscv64` (see §5.3)                | 🚧 New — aarch64's is hard-coded to aarch64's `PhysReg` enum and X0–X18/X19–X28/X29-X31 encoding ranges. |
| Fork opt-out (`contains_fork`, backend.rs:3226)              | Same predicate, with riscv64 syscall numbers (220/221 — SAME as aarch64) | ✅ Near-verbatim copy — unlike x86_64 (56/58), riscv64 uses the same Linux unified syscall numbers as aarch64. The `contains_fork` body can be lifted almost unchanged from `backend.rs:3226-3238`. |
| Syscall-position tracking (`regalloc.rs:954`)                | Already shared via `LiveRangeComputer` (G6 fix)            | ✅ Already in place — applies to `TargetAgnosticRegAlloc` too. |
| Three-operand arithmetic (no two-operand constraint)         | Same — RISC-V is three-operand like aarch64                 | ✅ Direct — unlike x86_64 §2.4, no extra `mov dst, lhs` needed for non-commutative ops. The `Add { rd, rs1, rs2 }` form maps 1:1 from IR `BinOp { dst, lhs, rhs }`. |

**The single biggest non-reuse item:** the aarch64 regalloc emitter delegates per-instruction byte emission to `emit_ir_instr` (the greedy emitter), which already supports a `reg_alloc.resolve_reg` mechanism. The riscv64 stack-slot ISel has **no equivalent** — every arm hard-codes `ss_load_value(val, slots, T0)` / `ss_store_to_slot(T0, dst_offset)`. This means the riscv64 reg_isel must either (a) introduce a `resolve_vreg(id) -> RegOrSlot` abstraction and rewrite every arm, or (b) special-case "dst in register" vs "dst spilled" per arm. See §5.

---

## 5. New Components Needed (riscv64-specific)

### 5.1 `src/codegen/src/riscv64/reg_isel.rs` (new module)

**Public API:**

```rust
/// Register-based riscv64 emitter.  Consumes a `RegAllocResult` and
/// produces an `AllocatedFunction` whose `encoded` bytes honour the
/// allocator's register assignments, spill code, and callee-saved set.
///
/// Returns `Err` if any IR instruction is not yet supported by the
/// register-based path; the caller falls back to the inline stack-slot
/// ISel in `riscv64.rs:6607-10311`.
pub fn allocate_registers(
    func: &IRFunction,
    alloc: &crate::regalloc::RegAllocResult,
) -> Result<AllocatedFunction, BackendError>;
```

**Internal structure (mirror the aarch64 pattern, but per-arm rewrite):**

1. **`resolve_vreg(id) -> RegOrSlot`** helper:
   ```rust
   enum RegOrSlot {
       Gpr(Gpr),                     // vreg is in this physical GPR
       Fpr(Fpr),                     // vreg is in this physical FPR
       Spill { offset: i32 },        // vreg is spilled to [s0 + offset] (offset is negative)
       Immediate(i64),               // operand is a constant (for IRValue::Immediate)
   }
   ```
   Look up the vreg in `alloc.vreg_to_preg`; if absent, look up in `alloc.spill_slots`; if absent, the vreg is undefined (panic in debug, fall back to scratch in release).

2. **`PhysicalReg` → `Gpr`/`Fpr` translation.** The riscv64 TargetDesc uses `RegDesc.index` 0..31 for both GPRs and FPRs (`target_desc.rs:1565-1643`). The `Gpr` enum has the **same** discriminant values (`riscv64.rs:70-103`). Translation is trivial:
   ```rust
   fn preg_to_gpr(p: crate::backend::PhysicalReg) -> Option<Gpr> {
       if p.class != crate::backend::RegClass::Gpr { return None; }
       Gpr::from_encoding(p.index)  // already exists at riscv64.rs:112-148
   }
   fn preg_to_fpr(p: crate::backend::PhysicalReg) -> Option<Fpr> {
       if p.class != crate::backend::RegClass::SimdFp { return None; }
       Fpr::from_encoding(p.index)  // already exists at riscv64.rs:293-329
   }
   ```
   **Note:** index 0 for GPRs is `Gpr::Zero` (the hardwired-zero register). `preg_to_gpr` should return `None` for index 0 (or the caller should treat `Gpr::Zero` as "no register assigned" — see §7.5 Zero-register hazard).

3. **Per-IR-instruction arms.** Approximately 30 distinct arms (matching the existing stack-slot ISel's coverage). For each, decide:
   - If dst is in a register and both operands are in registers (or immediates): emit the register-form op directly (e.g. `Instruction::Add { rd: dst_gpr, rs1: lhs_gpr, rs2: rhs_gpr }`).
   - If dst is in a register but lhs is spilled: `ld dst_gpr, [s0 + lhs_off]; op dst_gpr, dst_gpr, <rhs>`.
   - If dst is spilled: emit the stack-slot pattern (load into T0, op, store to dst slot) — this is **identical to today's stack-slot ISel arm**, so the existing code at `riscv64.rs:7243-7261` (Add) etc. can be lifted verbatim into a `dst_spilled` helper.
   - **No two-operand constraint:** RISC-V is three-operand, so `dst != lhs` is fine without an extra `mov` (unlike x86_64 §2.4). This is a significant simplification.

4. **Spill/reload insertion.** At each instruction boundary (`pos` and `pos+1`), walk `alloc.spill_code.get(&pos)` / `&(pos+1)` and emit:
   - `Reload`: `ld <scratch>, [s0 + slot.offset]` (GPR) or `fld <fscratch>, [s0 + slot.offset]` (FPR).
   - `Spill`: `sd <scratch>, [s0 + slot.offset]` (GPR) or `fsd <fscratch>, [s0 + slot.offset]` (FPR).
   - The `GenericSpillCode` enum (target-agnostic) has `preg: PhysicalReg` and `slot: GenericSpillSlot` fields. The `preg` field on riscv64 may be `PhysicalReg::new(Gpr, 0)` (Zero) — **do NOT honor this literally**; instead, use the vreg's actual location from `alloc.vreg_to_preg[vreg]` (the spilled vreg's home register, where the value lives just before the spill / just after the reload), and use a real scratch (T0/T1 for GPRs, FT0/FT1 for FPRs) for the load/store address computation if needed. See §7.5.
   - The `slot.offset` field is already an `i32` displacement from the FP (negative = below FP). Translation: `ld scratch, offset(s0)` where `offset` is the slot's `offset` field (sign-aware). For offsets outside [-2048, 2047], use the existing `ss_emit_slot_addr` helper at `riscv64.rs:5868` (which clobbers T3 as scratch for large offsets).

5. **Prologue.** Order:
   1. `addi sp, sp, -frame_size` (allocate frame — same as stack-slot).
   2. `sd ra, fs-8(sp); sd s0, fs-16(sp); addi s0, sp, fs` (RA + FP setup — same as stack-slot).
   3. **Callee-saved saves:** `sd s1, fs-24(sp); sd s2, fs-32(sp); ...` — only the subset in `alloc.used_callee_saved`, in increasing index order (s1, s2, s3, ..., s11). Each save uses a distinct `[sp + offset]` slot. Track the offsets so the epilogue can restore them in reverse order.
   4. **Per-function structural invariants** (cap sig, formal-verify counter, channel seq counter, proto state, circuit breaker state, IRQ/hotswap/STARK tables — see §1.4): reuse the stack-slot ISel's prologue code verbatim (`riscv64.rs:6673-6816` for slot computation; `:6818-6855+` for emission). **Critical:** the spill-area offsets used by these invariants must NOT collide with `alloc.spill_slots` offsets — see §7.7 for the frame-layout drift risk.

6. **Epilogue.** For each `IRInstr::Ret` (or `IRTerminator::Return`):
   1. Move return value into a0 (if not already there).
   2. **Callee-saved restores:** `ld s11, ...; ld s10, ...; ...; ld s1, ...` (reverse order of prologue saves).
   3. `ld ra, fs-8(sp); ld s0, fs-16(sp); addi sp, sp, fs; ret` (jalr zero, ra, 0).

7. **Argument-register preassignment.** For the first 8 integer params, force the param vreg to live in a0–a7 (in that order) regardless of what `alloc.vreg_to_preg` says. The allocator doesn't know about ABI arg registers (this is the R1-b-impl fix documented at `emit.rs:1112-1141`). Implementation: build a `param_vregs: HashSet<u32>` and skip `vreg_to_preg` lookups for those vregs during the param-loading prologue sequence (which `mv`-copies them from the arg reg into their assigned reg if the allocator picked a different one, or emits nothing if the allocator already picked the arg reg).

### 5.2 Wire-up in `RiscV64Backend::allocate_registers`

**File:** `src/codegen/src/riscv64.rs:6607-10311`.

**Sketch** (mirrors aarch64's `backend.rs:3175-3300`):

```rust
fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
    let real_regalloc = std::env::var("VUMA_REAL_REGALLOC_RISCV64")
        .map(|v| v == "1")
        .unwrap_or(false);
    let verify_callee_saved = std::env::var("VUMA_VERIFY_CALLEE_SAVED")
        .map(|v| v == "1")
        .unwrap_or(false);

    // CC-a-audit: fork opt-out (clone=220, vfork=221 on Linux/RISC-V — SAME as
    // aarch64 because RISC-V uses the generic Linux unified syscall ABI).
    let contains_fork = func.blocks.iter().any(|block| {
        block.instructions.iter().any(|inst| {
            match inst {
                crate::ir::IRInstr::Call { func: fname, .. } => {
                    fname == "spawn_worker" || fname == "fork"
                }
                crate::ir::IRInstr::Syscall { nr, .. } => *nr == 220 || *nr == 221,
                _ => false,
            }
        })
    });

    if real_regalloc && !contains_fork {
        if let Some(alloc) = try_real_regalloc(func) {
            if verify_callee_saved {
                if let Err(msg) = super::reg_isel::verify_callee_saved_riscv64(&alloc) {
                    panic!("verify_callee_saved_riscv64 FAILED for '{}': {}", func.name, msg);
                }
            }
            match super::reg_isel::allocate_registers(func, &alloc) {
                Ok(mut allocated) => {
                    crate::regalloc_emit::annotate_with_regalloc(&mut allocated, &alloc);
                    return Ok(allocated);
                }
                Err(e) => {
                    vuma_log!(warn,
                        "riscv64 reg_isel failed for '{}': {}, falling back to stack-slot ISel",
                        func.name, e);
                    // fall through
                }
            }
        }
    } else if real_regalloc && contains_fork {
        vuma_log!(debug,
            "riscv64 regalloc: function '{}' contains spawn_worker/fork; \
             falling back to stack-slot ISel (fork+regalloc not supported)",
            func.name);
    }

    // Fallback (default path or regalloc failure): inline stack-slot ISel.
    // (The current 3700-line body at riscv64.rs:6607-10311 moves here unchanged.)
    self.allocate_registers_stack_slot(func)
}
```

(`allocate_registers_stack_slot` is a renamed version of the current `allocate_registers` body. CC-b-impl can either rename or extract into a private method.)

### 5.3 `verify_callee_saved_riscv64` (new verifier)

The existing `verify_callee_saved` (`regalloc.rs:4860`) is hard-coded to aarch64's `PhysReg` enum and register encoding ranges. It cannot be reused as-is. Add a sibling:

```rust
/// Verify that every physical register used by the regalloc is either
/// (a) caller-saved, (b) in `used_callee_saved`, or (c) Zero/RA/SP
/// (always-reserved).  Mirrors `regalloc::verify_callee_saved` but for
/// the riscv64 register file and the target-agnostic `RegAllocResult`.
pub fn verify_callee_saved_riscv64(
    result: &crate::regalloc::RegAllocResult,
) -> std::result::Result<(), String> {
    // Allowed GPRs by index (LP64D ABI):
    //   caller-saved: 5, 6, 7 (t0-t2), 10-17 (a0-a7), 28-31 (t3-t6) = 15 regs
    //   always-allowed (reserved): 0 (Zero), 1 (RA), 2 (SP) — never in vreg_to_preg
    //   callee-saved: from result.used_callee_saved (typically 9, 18-27, excluding 8=s0/FP)
    let mut allowed_gprs: HashSet<u32> = [5, 6, 7, 10, 11, 12, 13, 14, 15, 16, 17, 28, 29, 30, 31]
        .into_iter().collect();
    for preg in &result.used_callee_saved {
        if preg.class == crate::backend::RegClass::Gpr {
            allowed_gprs.insert(preg.index);
        }
    }
    // Allowed FPRs by index:
    //   caller-saved: 0-7 (ft0-ft7), 10-17 (fa0-fa7), 28-31 (ft8-ft11) = 20 regs
    //   callee-saved: from result.used_callee_saved (typically 8, 9, 18-27)
    let mut allowed_fprs: HashSet<u32> = [0,1,2,3,4,5,6,7,10,11,12,13,14,15,16,17,28,29,30,31]
        .into_iter().collect();
    for preg in &result.used_callee_saved {
        if preg.class == crate::backend::RegClass::SimdFp {
            allowed_fprs.insert(preg.index);
        }
    }

    let check = |preg: crate::backend::PhysicalReg| -> Option<String> {
        match preg.class {
            crate::backend::RegClass::Gpr => {
                if !allowed_gprs.contains(&preg.index) {
                    return Some(format!(
                        "GPR index {} is not caller-saved, not in used_callee_saved, \
                         and not Zero/RA/SP", preg.index));
                }
            }
            crate::backend::RegClass::SimdFp => {
                if !allowed_fprs.contains(&preg.index) {
                    return Some(format!("FPR index {} out of range", preg.index));
                }
            }
            _ => return Some(format!("unexpected register class {:?}", preg.class)),
        }
        None
    };
    for (&vreg, &preg) in &result.vreg_to_preg {
        if let Some(msg) = check(preg) { return Err(format!("vreg {} -> {:?}: {}", vreg, preg, msg)); }
    }
    for (pos, codes) in &result.spill_code {
        for sc in codes {
            // The spill_code's `preg` field may be the gen_spill_reload scratch
            // (PhysicalReg::new(class, 0) = Zero on riscv64). We do NOT enforce
            // it here — the emitter substitutes a real scratch. See §7.5.
            let _ = sc; // intentionally not checked for riscv64.
        }
    }
    Ok(())
}
```

(`sc.phys_reg()` is a convenience accessor on `GenericSpillCode` that CC-b-impl may need to add — currently the enum's spill/reload variants each carry `preg: PhysicalReg` field; expose it via a method or a `match`. For riscv64 specifically, the spill_code `preg` field is **not** checked because `gen_spill_reload` uses index 0 = Zero as a placeholder; the emitter must substitute a real scratch — see §7.5.)

### 5.4 riscv64 frame layout (proposed)

The aarch64 regalloc emitter uses a layout where the callee-saved area lives **below** SP (between SP and the FP/LR save pair). riscv64's prologue uses `sd ra, fs-8(sp); sd s0, fs-16(sp)` to save RA and S0 **below** SP-after-decrement (i.e., in the freshly-allocated frame). Callee-saved saves (`sd s1, fs-24(sp); sd s2, fs-32(sp); ...`) extend this pattern downward.

**Proposed riscv64 layout (low → high addresses):**

```text
   [spill area]                  ← S0-relative negative offsets; addressed via [s0 + neg_off]
                                  Size = alloc.total_spill_slots * 8, aligned to 16.
                                  PLUS per-function structural-invariant slots
                                  (cap sig, formal-verify counter, etc.).
   [s1, s2, ..., s11 saves]      ← Addressed via [sp + (fs - 24)], [sp + (fs - 32)], ...
                                  Size = N_callee_saved * 8.
   [s0 save]                     ← [sp + (fs - 16)]
   [ra save]                     ← [sp + (fs - 8)]
   [SP after prologue]           ← sp = entry_sp - fs
   [caller's frame]              ← Caller's SP at call time.
```

This layout has the callee-saved saves **below** the spill area (between the spill area and the ra/s0 save pair), and the spill area **above** S0 (in S0-relative negative-offset terms). S0-relative addressing for spill slots is `ld t0, -off(s0)` (negative offset). For callee-saved saves, the SP-relative addressing `sd s1, -24(sp)` is used (the prologue computes SP-relative offsets; the epilogue uses the same).

**Critical:** the per-function structural invariants (cap sig computation, formal-verify counter, channel seq counter, proto state, circuit breaker state, IRQ/hotswap/STARK tables) currently use specific S0-relative offsets computed in the inline stack-slot ISel (`riscv64.rs:6673-6816`). The register-based emitter must use **different** offsets for `alloc.spill_slots` to avoid colliding with these reserved slots. Recommended approach: reserve the existing structural-invariant offsets as a "fixed region" below S0, then place `alloc.spill_slots` *below* that region (i.e., at more-negative offsets).

### 5.5 Prologue refactor (optional, recommended)

The per-function structural-invariant prologue (`riscv64.rs:6673-6816` for slot computation + `:6818-6855+` for emission) is the single most complex part of the existing stack-slot ISel. Refactoring it into a shared `emit_prologue_common(&mut instructions, &func, &slot_table)` helper would let both the stack-slot ISel and the new reg_isel call it. This is a **non-trivial refactor** because the current code is inline in `allocate_registers` and captures many local variables (`frame_size`, `cap_grant_sig`, `cap_grant_sig_input`, `formal_verify_count_off`, etc.).

**Recommendation for CC-b-impl:** take option (a) — copy the prologue code into `reg_isel.rs`. Refactor to a shared helper in a follow-up PR (CC-e-cleanup). This mirrors the x86_64 recommendation in R2-a-audit §7.7.

---

## 6. TargetDesc Readiness

The riscv64 TargetDesc at `target_desc.rs:1562-1700` is **complete enough** to support register-based emission. Specifically:

| Required field                                   | Present? | Source |
|--------------------------------------------------|----------|--------|
| All 32 GPRs with ABI roles                       | ✅       | `:1563-1606` |
| All 32 FPRs                                      | ✅       | `:1608-1643` |
| Caller-saved vs callee-saved classification      | ✅       | `.callee_saved()` on x8, x9, x18-x27 (GPRs) and f8, f9, f18-f27 (FPRs) |
| Stack pointer (x2) marked non-allocatable        | ✅       | `.stack_pointer()` at `:1569` sets `is_allocatable = false` (verified at `:1135-1138`) |
| Link register (x1) marked non-allocatable        | ✅       | `.link_register()` at `:1567` sets `is_allocatable = false` (verified at `:1145-1149`) |
| Zero register (x0) marked non-allocatable        | ✅       | `.hardwired_zero()` at `:1565` — must verify it sets `is_allocatable = false` (the helper is not shown in this audit; CC-b-impl should confirm by reading the `hardwired_zero()` builder body) |
| Global pointer (x3) marked non-allocatable       | ✅       | `.not_allocatable()` at `:1571` |
| Thread pointer (x4) marked non-allocatable       | ✅       | `.not_allocatable()` at `:1573` |
| Frame pointer (x8) marked                        | ⚠️ BUG   | `RegDesc::gpr("x8", 8).frame_pointer().callee_saved()` at `:1579` — `frame_pointer()` builder at `:1140-1143` does NOT chain `.not_allocatable()`. x8 is allocatable AND marked as the frame pointer. Same bug pattern as x86_64 RBP (R2-a-audit §6 gap 1) and aarch64 X29 (`target_desc.rs:1458` — invisible on aarch64 because its `LinearScanAllocator` doesn't consult `TargetDesc`). **Fix required:** add `.not_allocatable()` to the x8 line. |
| Argument register positions                      | ✅       | `.arg(0)` through `.arg(7)` on x10-x17 (GPRs) and f10-f17 (FPRs) |
| Return registers (a0, fa0)                       | ✅       | `.return_reg()` on x10, x11 (GPRs) and f10, f11 (FPRs) |
| Calling convention descriptor                    | ✅       | `:1646-1658` (lp64d, 16-byte alignment, has_link_register=true, no branch delay slots, no TOC) |
| `TargetAgnosticRegAlloc` already produces `RegAllocResult` for riscv64 | ✅ | `try_real_regalloc` at `riscv64.rs:6542-6571` proves this works in production today. |

**Gaps found in this audit:**

1. **x8 (S0/FP) allocatability bug.** `RegDesc::gpr("x8", 8).frame_pointer().callee_saved()` at `target_desc.rs:1579` does not chain `.not_allocatable()`. The `frame_pointer()` builder at `target_desc.rs:1140-1143` does NOT set `is_allocatable = false`. This means `TargetAgnosticRegAlloc::new(target)` at `regalloc.rs:2768-2791` includes x8 in the callee-saved pool (it's marked callee-saved) and the linear-scan allocator may assign vregs to S0. If the register-based emitter also uses S0 as the frame base (the existing convention — `addi s0, sp, fs` in the prologue, `ld ..., neg_off(s0)` in every stack-slot load/store), this is a conflict. **Fix:** add `.not_allocatable()` to the x8 line in `riscv64_target_desc()`: `RegDesc::gpr("x8", 8).frame_pointer().callee_saved().not_allocatable()`.

2. **`hardwired_zero()` not verified.** The x0 entry at `target_desc.rs:1565` uses `.hardwired_zero()`. This audit did not read the builder body (it is not at line 1140+ alongside `frame_pointer`/`link_register`/`stack_pointer`). CC-b-impl must confirm `.hardwired_zero()` sets `is_allocatable = false` — otherwise x0 could be assigned to a vreg (catastrophic, since writes to x0 are silently discarded). The `Gpr::is_allocatable()` helper at `riscv64.rs:153-155` correctly excludes Zero, but that is the backend-side check; the TargetDesc is what `TargetAgnosticRegAlloc::new()` consults at `regalloc.rs:2768-2771`.

3. **No FP-rel spill-slot offsets verified.** The `RegAllocResult.spill_slots` field is a `HashMap<IRValueId, GenericSpillSlot>`. The `GenericSpillSlot` struct (`regalloc.rs:3318-3325`) has an `offset: i32` field documented as "Offset from the frame pointer in bytes (negative = deeper)". For riscv64, this offset should be the S0-relative negative displacement. CC-b-impl must verify the offset is computed correctly by `TargetAgnosticRegAlloc` (it should be — the field is documented and the same path works for aarch64/x86_64).

**Conclusion:** TargetDesc readiness is **HIGH** with the single x8-allocatability fix. No new TargetDesc fields are needed.

---

## 7. Risk Assessment

### 7.1 Fixed 32-bit instruction encoding — **LOW**

RISC-V instructions are always 4 bytes (no compressed extension in the current emitter). The existing `Instruction::encode()` helpers in `riscv64.rs` handle all encoding correctly. The register-based emitter just calls these helpers with whatever `Gpr`/`Fpr` the allocator assigned; no new encoding code is needed. **Mitigation:** existing QEMU smoke tests on riscv64 will exercise all paths immediately once the env var is enabled.

### 7.2 PC-relative addressing — **LOW**

RISC-V uses `auipc` + `addi`/`ld` for PC-relative addressing of global variables. The existing stack-slot ISel's `GetAddress` arm uses `auipc` (verify by grep — the `Instruction::Auipc` variant exists at `riscv64.rs:516`). The register-based emitter inherits the stack-slot ISel's `GetAddress` arm verbatim, so no new code is needed. The single hazard: if the emitter inserts spill/reload instructions between an `auipc` and the label it references, the offset computed by `apply_fixups` will be wrong. **Mitigation:** spills/reloads are S0-relative, not PC-relative, so they don't perturb PC-relative fixup math. Spot-check with `mem_copy_buffer.vuma` (the test that exposed the aarch64 greedy SIGSEGV).

### 7.3 Callee-saved tracking — **HIGH**

This is the same HIGH risk identified for aarch64 in F2-a §5.3 and Wave-1's G1/G2/G4 fixes. The hazards:

1. **Spill-scratch register clobbering callee-saved.** If a spill/reload path uses a callee-saved register as a scratch (the aarch64 path used X15, which is caller-saved; the riscv64 path must NOT use s1-s11 — only t0-t6, a0-a7 are caller-saved scratches). **Mitigation:** the existing `ss_load_imm` / `ss_emit_slot_addr` helpers at `riscv64.rs:5461, 5868` use T0-T5 as scratch; the register-based emitter must follow the same convention.

2. **`used_callee_saved` set incompleteness.** If the linear-scan allocator misses a callee-saved register that a spilled-reload path uses as a scratch, the epilogue will restore garbage into it. **Mitigation:** the new `verify_callee_saved_riscv64` (§5.3) catches this. Wire it behind `VUMA_VERIFY_CALLEE_SAVED=1` for the curated test subset before flipping the default.

3. **Callee-saved register interaction with `clone()`.** The `clone` syscall returns in the child process with all registers in their pre-syscall state. If the parent had `sd s1` in the prologue and the child reaches the epilogue, the child's `ld s1` will load a value that the parent's `sd` put on the child's stack copy — this is actually correct (clone copies the stack). The real hazard is that the child may execute a *different* code path (the `if pid == 0` branch) and clobber callee-saved registers without restoring them. **Mitigation:** the `contains_fork` opt-out (§3 step 2) sidesteps this entirely by falling back to stack-slot for fork-containing functions.

### 7.4 Fork + regalloc — **MEDIUM**

Same as Wave-1 R1-b2-fix on aarch64, but with **the SAME syscall numbers as aarch64** (Linux/RISC-V: clone=220, vfork=221; Linux/aarch64: clone=220, vfork=221; Linux/x86_64: clone=56, vfork=58 — RISC-V uses the generic unified syscall ABI shared with aarch64, NOT the x86_64-specific numbers). The `contains_fork` predicate can be a near-verbatim copy of the aarch64 predicate at `backend.rs:3226-3238`. **Mitigation:** §5.2 sketch uses the correct numbers (220/221). The existing inline `spawn_worker` arm at `riscv64.rs:8070-8077` already emits `clone=220` — confirming the convention. Add a unit test asserting the predicate matches on a fixture with `IRInstr::Syscall { nr: 220, .. }`.

### 7.5 Zero-register (x0) hazard — **HIGH** (riscv64-specific)

`TargetAgnosticRegAlloc::gen_spill_reload` at `regalloc.rs:3145-3174` uses `PhysicalReg::new(interval.class.into(), 0)` as the scratch register for spill/reload code annotation. On riscv64:

- For GPRs, `interval.class.into()` = `RegClass::Gpr`, index 0 = `Gpr::Zero` (x0, the hardwired-zero register).
- A `GenericSpillCode::Spill { preg: Zero, ... }` translated literally would emit `sd Zero, [s0 + off]` — a **no-op** (writes to Zero are discarded). The spilled value is never actually stored.
- A `GenericSpillCode::Reload { preg: Zero, ... }` translated literally would emit `ld Zero, [s0 + off]` — also a **no-op** (writes to Zero are discarded). The reloaded value is never actually loaded into any register.

**This is the single riscv64-specific correctness hazard that has no analogue on aarch64 or x86_64.** On aarch64, index 0 = X0 (a usable caller-saved register). On x86_64, index 0 = RAX (also usable). On riscv64, index 0 = Zero (UNUSABLE).

**Mitigation:** the `riscv64::reg_isel` emitter must:
1. **Not** honor the `preg` field of `GenericSpillCode` literally. Instead, consult `alloc.vreg_to_preg[vreg]` for the vreg's actual home register (where the value lives just before the spill / just after the reload).
2. Use a real scratch (T0 or T1) for the load/store instruction if the home register is the spilled-vreg's pre-spill location. (For a "spill" entry, the vreg's value is currently in its home register; emit `sd <home_reg>, [s0 + slot.off]`. For a "reload" entry, emit `ld <home_reg>, [s0 + slot.off]`.)
3. Add a `verify_callee_saved_riscv64` assertion (§5.3) that **skips** the `spill_code` `preg` field check (since it's a placeholder, not a real register assignment).

**Test:** CC-b-impl must include a regression test that exercises a function with more live vregs than available caller-saved registers (forcing at least one spill/reload), compiled with `VUMA_REAL_REGALLOC_RISCV64=1`, and verify the result under `qemu-riscv64-static`. If the spill is silently dropped (Zero-register bug), the function will read garbage and exit with the wrong code.

### 7.6 Stack-frame layout drift — **MEDIUM**

The stack-slot ISel computes a specific `frame_size` (`riscv64.rs:6815`: `let frame_size = ((current_offset + 15) & !15) as usize;`) that includes the structural-invariant slots but does NOT include a callee-saved save area (it saves only RA and S0 via `sd ra, fs-8(sp); sd s0, fs-16(sp)`). The register-based emitter must compute a *different* `frame_size` that includes the callee-saved save area (size = `used_callee_saved.len() * 8`, aligned to 16). If the `AllocatedFunction.frame_size` field is set to the wrong value, debug/unwind info will be wrong (but QEMU execution will still be correct because the bytes themselves honour the right layout). **Mitigation:** §5.4 documents the proposed layout. CC-b-impl must ensure `allocated.frame_size` is set from the register-based emitter's computed value, not from the stack-slot ISel's helper.

### 7.7 Per-function structural invariants interaction — **MEDIUM**

The stack-slot ISel's prologue embeds compile-time-computed capability-grant signatures, formal-verify counter pre-loads, channel sequence counter initialisation, IRQ/hotswap/STARK table zeroing, etc. (§1.4). The register-based emitter must either (a) replicate this prologue verbatim, or (b) extract a shared helper. Option (b) is preferred but requires refactoring the stack-slot ISel's prologue builder into a callable function — a non-trivial change because the current prologue is inline in `allocate_registers` and captures many local variables. **Mitigation:** for CC-b-impl, take option (a) — copy the prologue code into `reg_isel.rs`. Refactor to a shared helper in a follow-up PR (CC-e-cleanup). Same recommendation as x86_64 §7.7.

### 7.8 SIMD/FP register allocation — **MEDIUM**

`TargetAgnosticRegAlloc` does allocate FPRs (the `caller_saved_fps` / `callee_saved_fps` pools are populated from the TargetDesc), but the existing stack-slot ISel's FP path uses fixed FT0/FT1 for all operations. The register-based emitter must consult `alloc.vreg_to_preg` for FP vregs too. The `Fpr` enum at `riscv64.rs:251-284` is complete (32 registers), and `Fpr::is_callee_saved()` is correct (fs0-fs11). **Mitigation:** start CC-b-impl with **integer-only** register allocation (FP vregs fall back to the stack-slot pattern via `dst_spilled`); add FP register allocation in a follow-up. Same approach as x86_64 §7.8.

### 7.9 Syscall-position tracking (G6) — **LOW (already fixed)**

Wave-1's G6 fix at `regalloc.rs:954` (tracking `IRInstr::Syscall` as a call position) is in `LiveRangeComputer::compute`, which both `LinearScanAllocator` (aarch64) and `TargetAgnosticRegAlloc` (riscv64) use. riscv64 already benefits. **Mitigation:** none needed — verify with a `try_recv`-equivalent riscv64 test in the curated subset. Note: the CB-a-investigate (`1ccefa6b`) root cause for `try_recv` on aarch64 was a CSEL operand swap, NOT a syscall-position bug — the G6 fix is still necessary for the syscall-clobber tracking to mark intervals crossing ECALLs.

### 7.10 `IRInstr::Select` lowering — **MEDIUM** (riscv64-specific)

The CB-a-investigate (`1ccefa6b`) root cause for the aarch64 `try_recv` failure was a CSEL operand swap: `CSEL x13, x14, x9, NE` selected the wrong branch because `rn` was `false_val` instead of `true_val`. RISC-V has no CSEL — the `Select` lowering will be a small branch-based sequence:

```riscv
    # is_error in t0, true_val in t1, false_val in t2
    bnez t0, .Lselect_true      # if is_error != 0, branch to true
    mv t0, t2                    # false path: select false_val
    j .Lselect_end
.Lselect_true:
    mv t0, t1                    # true path: select true_val
.Lselect_end:
    # t0 = result
```

The hazard: the **branch direction** must put the `true_val` move on the path that executes when `cond` is true. This is the same kind of operand-swap bug that aarch64 had — easy to get wrong, and the symptom is silent (wrong branch selected). The existing stack-slot ISel's `Select` arm has the correct ordering (it works on the 30/30 stack-slot path — CA-a-test); the register-based arm must copy that ordering verbatim. **Mitigation:** add a regression test for `try_recv` (or equivalent) on riscv64 with `VUMA_REAL_REGALLOC_RISCV64=1` and verify exit code 77 (not 0). If the test exits 0, suspect a Select operand-swap.

### 7.11 12-bit immediate range — **LOW**

RISC-V I-type and S-type immediates are 12-bit signed (-2048..2047). For larger constants or stack offsets, `lui`+`addi` (for constants) or `ss_emit_slot_addr` (for large S0-relative offsets, clobbering T3 as scratch) is used. The existing helpers at `riscv64.rs:5461, 5868` handle this. The register-based emitter inherits the helpers. **Mitigation:** spot-check with a function that has a frame_size > 2048 (forcing the prologue's `addi sp, sp, -fs` to use the `ss_load_imm`+`sub` path at `riscv64.rs:6837-6854`).

---

## 8. Phased Rollout Plan

### Phase Ca — riscv64 reg_isel skeleton (integer-only, no FP, no SIMD)

1. Create `src/codegen/src/riscv64/reg_isel.rs` with the public API from §5.1.
2. Implement `preg_to_gpr` / `preg_to_fpr` translation (§5.1.2).
3. Implement `verify_callee_saved_riscv64` (§5.3) — with the §7.5 spill_code `preg` field skip.
4. Implement prologue/epilogue (§5.1.5, §5.1.6) with callee-saved `sd`/`ld` from `alloc.used_callee_saved`.
5. Implement per-IR-instruction arms for: `Add`, `Sub`, `Mul`, `Div`, `BinOp`, `UnaryOp`, `Cmp`, `Cast` (integer kinds), `Load`, `Store`, `Offset`, `GetAddress`, `Alloc`, `Free`, `Branch`, `CondBranch`, `Ret`, `Phi` (no-op), `Select` (with §7.10 regression test), `Call` (direct, integer args), `CallIndirect`, `Syscall` (Linux/RISC-V ABI: a7=sysnr, a0-a5=args, a0=return).
6. **Defer to Phase Cc:** all Channel*/StarkProof builtins, AtomicCas, VectorOp, FP-typed Add/Sub/Mul/Div/Cmp/Cast (these fall back to the stack-slot pattern via `dst_spilled` — correct but slow).
7. Wire up `RiscV64Backend::allocate_registers` (§5.2) gated by `VUMA_REAL_REGALLOC_RISCV64=1` (default off).
8. Run curated riscv64 test subset under `qemu-riscv64-static` with the env var on. Triage failures.

**Estimated effort:** 2–3 weeks (CC-b-impl).

### Phase Cb — riscv64 reg_isel FP/SIMD

1. Add FP-typed Add/Sub/Mul/Div/Cmp/Cast arms honouring `alloc.vreg_to_preg` for `RegClass::SimdFp` vregs.
2. Add AtomicCas, VectorOp arms.
3. Re-run curated subset; expect binary size reduction on FP-heavy tests.

**Estimated effort:** 1–2 weeks (CC-c-opt or CC-d-impl).

### Phase Cc — riscv64 reg_isel IPC/capability builtins

1. Add `ChannelOpen`, `ChannelSend`, `ChannelRecv`, `ChannelClose`, `ChannelRecvTimeout`, `ChannelRecvResult`, `StarkProof` arms.
2. Each builtin must consult the per-function structural-invariant slots (cap sig, formal-verify counter, channel seq counter) — reuse the stack-slot ISel's emit code for these arms.
3. Re-run curated IPC subset; verify the formal-verify counter still increments correctly.

**Estimated effort:** 1 week (CC-e-impl).

### Phase Cd — Default-on

1. Run the full 30-test curated matrix (CA-a-test equivalent for riscv64) under regalloc.
2. Verify ≥ 28/30 pass (DoD threshold from Wave-1 R1-c-test).
3. Flip `VUMA_REAL_REGALLOC_RISCV64` default to `1`.
4. Update `docs/caveats.md` to reflect riscv64 now emits register-based bytes.

**Estimated effort:** 2–3 days (CC-f-verify + CC-g-default).

### Phase Ce — x8 (S0/FP) allocatability fix and refactor

1. Add `.not_allocatable()` to the x8 line in `riscv64_target_desc()` (§6 gap 1).
2. Verify `hardwired_zero()` builder sets `is_allocatable = false` (§6 gap 2).
3. Refactor the stack-slot ISel's prologue builder into a shared `emit_prologue_common()` helper (§5.5 / §7.7).

**Estimated effort:** 1 week (CC-h-cleanup).

---

## 9. Concrete Code Changes

| # | File | Change | LOC (est.) | Phase |
|--:|------|--------|-----------:|:------:|
| 1 | `src/codegen/src/riscv64/reg_isel.rs` (NEW) | New module: `allocate_registers`, `preg_to_gpr`, `preg_to_fpr`, `verify_callee_saved_riscv64`, per-IR-instruction arms, prologue/epilogue builders. | ~2000–2500 | Ca |
| 2 | `src/codegen/src/riscv64.rs:6607-10311` | Rename current `allocate_registers` body to `allocate_registers_stack_slot`; rewrite `allocate_registers` per §5.2 sketch (env-var gate, fork opt-out with syscall nr=220/221, reg_isel dispatch, stack-slot fallback). | ~60 | Ca |
| 3 | `src/codegen/src/riscv64.rs:4162-4174` | (Cosmetic, optional) Rename `emit_function_regalloc` → `emit_function_with_regalloc_metadata`; reserve the `emit_function_regalloc` name for the byte-changing path. Update `emit_function_with_regalloc` convenience method at `:4177-4183`. | ~10 | Ca (or defer) |
| 4 | `src/codegen/src/riscv64.rs` (module decl) | Add `mod reg_isel;` (or `pub mod reg_isel;`) to the module's child-module declarations. | 1 | Ca |
| 5 | `src/codegen/src/target_desc.rs:1579` | Add `.not_allocatable()` to the x8 line: `RegDesc::gpr("x8", 8).frame_pointer().callee_saved().not_allocatable()`. | 1 | Ca (recommended) or Ce |
| 6 | `src/codegen/src/target_desc.rs:1565` (x0/hardwired_zero) | Verify `hardwired_zero()` builder sets `is_allocatable = false`; if not, add `.not_allocatable()` to the x0 line. | 0–1 | Ca (verification) |
| 7 | `src/codegen/src/regalloc.rs` (near `GenericSpillCode` enum, ~:3355-area) | Add a `phys_reg(&self) -> crate::backend::PhysicalReg` accessor on `GenericSpillCode` for documentation purposes (the riscv64 verifier skips this field per §7.5, but x86_64's verifier uses it). | ~10 | Ca |
| 8 | `tests/` (NEW test file) | Add unit tests for `verify_callee_saved_riscv64` (positive + negative cases). | ~50 | Ca |
| 9 | `tests/` (NEW integration test) | Add a `try_recv`-equivalent riscv64 test that exercises the Syscall-position tracking on riscv64 (G6 + §7.10 Select operand regression guard). | ~30 | Ca |
| 10 | `tests/` (NEW integration test) | Add a "force spill" test: a function with more live vregs than available caller-saved GPRs (15 on riscv64), compiled with `VUMA_REAL_REGALLOC_RISCV64=1`, asserting correct exit code (Zero-register hazard §7.5 regression guard). | ~30 | Ca |
| 11 | `docs/caveats.md` | Document the new `VUMA_REAL_REGALLOC_RISCV64` env var and the fork opt-out (clone=220/vfork=221). | ~20 | Ca |

**Total LOC for Phase Ca:** ~2200–2700 (dominated by the new `reg_isel.rs`).

---

## 10. Effort Estimate

**F2-a estimate:** 2–4 weeks (3–4 weeks in §6 Phase 2, p. 413).

**This audit's revised estimate:**

| Phase | Effort (developer-weeks) | Notes |
|-------|--------------------------|-------|
| Ca — integer-only skeleton + wire-up + fork opt-out + verifier + Zero-register-hazard mitigation | 2–3 | Bulk of the work: ~30 IR instruction arms in `reg_isel.rs`, each adapting the existing stack-slot arm to honour `vreg_to_preg`. The aarch64 wire-up at `backend.rs:3175-3300` is the template. RISC-V's three-operand form (§2.4) is a **simplification vs x86_64**: no two-operand `mov dst, lhs` insertions, no `eliminated_copies` field needed. The Zero-register hazard (§7.5) is a **complication vs x86_64/aarch64**: the emitter must not honor the spill_code `preg` field literally. Net: roughly the same effort as x86_64 Phase 2a. |
| Cb — FP/SIMD arms | 1–2 | Adds ~10 arms, mostly F/D-extension encoder calls (the `Instruction` enum already has `Fadd.D`/`Fsub.D`/`Fmul.D`/`Fdiv.D`/`Fmv.D`/`Fcvt.D.W` at `target_desc.rs:1677`). |
| Cc — IPC/capability builtin arms | 1 | Adds ~7 arms, mostly verbatim copies of stack-slot arms (the builtins consult structural-invariant slots that already exist). |
| Cd — default-on + verification | 0.5 | Run curated matrix, flip default, update docs. |
| Ce — cleanup (x8 fix, hardwired_zero verify, refactor) | 1 | Optional; can ship without. |
| **Total (Phases Ca–Cd, required for default-on)** | **4.5–6.5** | |
| **Total (Phases Ca–Ce, with cleanup)** | **5.5–7.5** | |

**Achievable in this orchestration run? N.**

The orchestration run is operating under a 10-minute-per-task budget for sub-agents (per CC-a-audit's own constraint). The riscv64 register-based emitter is genuinely 4.5–6.5 developer-weeks of work — it is comparable in size to x86_64 Phase 2a (R2-a-audit estimated the same 4.5–6.5 weeks for x86_64). The CC-b-impl sub-agent should be tasked with **Phase Ca only** (integer-only skeleton + wire-up + verifier + env-var gate, default off), which is itself 2–3 weeks and will require multiple sub-agent invocations to complete iteratively (mirror the Wave-1 R1-a→R1-b→R1-b2→R1-b3→R1-c→R1-f cadence). Default-on (Phase Cd) is a separate task after Phase Ca's curated-subset verification passes.

Per §0.7-6 of the orchestration protocol, this honest estimate should cause the orchestrator to defer the bulk of the work to a human developer OR sequence it across many orchestration waves. The CC-a-audit deliverable (this document) is itself the actionable artefact: CC-b-impl can proceed incrementally off it.

**Key risks that could inflate the estimate:**

- The Zero-register hazard (§7.5) is unique to riscv64 and requires careful emitter-side handling. If the emitter naively honors the `GenericSpillCode.preg` field, every spill/reload silently becomes a no-op — the symptom is wrong exit codes on any function with register pressure, which may be hard to triage without a dedicated regression test.
- The per-function structural invariants (cap sig computation, formal-verify counter) are subtle and have many edge cases; replicating them in `reg_isel.rs` without refactoring the stack-slot ISel first will create maintenance hazard (§7.7).
- The x8 allocatability bug (§6 gap 1) must be fixed in Phase Ca, not deferred — otherwise the linear-scan allocator will assign vregs to S0 and the emitter's frame-base convention will conflict.
- The `IRInstr::Select` lowering (§7.10) carries the same operand-swap hazard as aarch64's CSEL bug (CB-a-investigate `1ccefa6b`). A regression test is mandatory.

**Key simplifications vs x86_64 (could deflate the estimate):**

- RISC-V is three-operand (§2.4) — no two-operand coalescing problem, no `eliminated_copies` field needed.
- RISC-V uses fixed 32-bit instructions (§2.5) — no REX prefix, no variable-length fall-through risk.
- RISC-V uses the same Linux syscall numbers as aarch64 (§7.4: clone=220, vfork=221) — the `contains_fork` predicate is a near-verbatim copy.
- RISC-V has no condition codes (§2.7) — comparison patterns are already register-value-based via the existing `emit_cmp_isel` helper.

Net: the riscv64 effort is roughly equal to x86_64's (the simplifications offset the Zero-register complication).

---

## DoD Check

- [x] Design doc exists at `scripts/audit/completion_wave_c_riscv64_design.md`.
- [x] All 10 required sections present: §1 Current Path, §2 Register File, §3 What emit_function_regalloc Needs, §4 Reusable Components, §5 New Components, §6 TargetDesc Readiness, §7 Risk Assessment, §8 Phased Rollout, §9 Concrete Code Changes, §10 Effort Estimate.
- [x] Concrete line numbers cited for every code path: §1 `riscv64.rs:6607`, `:6542`, `:4162`, `:10306`, `:8070`, `:7243`, `:6818`; §2 `target_desc.rs:1562-1700`, `riscv64.rs:70-103`, `:251-284`; §4 `emit.rs:1056-1354`, `regalloc.rs:4860`, `backend.rs:3226`; §5 `riscv64.rs:6607-10311`, `target_desc.rs:1579`, `regalloc.rs:3355`; §6 `target_desc.rs:1579`, `:1565`, `:1140-1143`; §7 multiple.
- [x] Honest effort estimate: 4.5–6.5 developer-weeks total; Phase Ca alone is 2–3 weeks; **NOT achievable in a single 10-minute orchestration sub-agent run** — recommendation is to sequence CC-b-impl across multiple waves or defer to human developer per §0.7-6.
- [x] No source files edited (READ-ONLY audit — `git status --short` shows only the new markdown added).
- [x] No `git push`.
- [x] No sub-agents spawned.
