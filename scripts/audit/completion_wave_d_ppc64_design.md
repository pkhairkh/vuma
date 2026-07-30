# Wave D — ppc64 Register-Based Emitter Design

**Task ID:** CD-a-audit
**Wave:** D (VUMA Regalloc Completion run)
**Prior-run context:**
- R2-a-audit (`c3e7413b`, `scripts/audit/regalloc_endianness_wave2_x86_64_design.md`) produced the x86_64 register-based emitter design doc (568 lines, 10 sections). Template for this document.
- CC-a-audit (`2390dd62`, `scripts/audit/completion_wave_c_riscv64_design.md`) produced the riscv64 equivalent (696 lines, 10 sections). Cross-referenced for three-operand ISA parallels (RISC-V and PPC are both three-operand, unlike x86_64).
- F2-a-audit (`7083e1c7`, `scripts/audit/followup_wave2_emit_regalloc_design.md` §4 row 5) classified ppc64 as **LOW** readiness for `emit_function_regalloc` wire-up — it needs a *new* register-based emitter (the existing `Emitter::emit_function_regalloc` at `emit.rs:1056` is aarch64-only). F2-a estimated 2–4 weeks for a human developer.
- Wave-1 R1-b-impl/R1-b2-fix/R1-b3-fix (`4c6b8524`, `6a8dbd42`, `8194337b`) on aarch64 established the wire-up pattern this design follows.

**Scope of this document:** produce the design CD-b-impl will follow. ppc64le inherits automatically (delegation pattern, see §6.1).

**Files audited (READ-ONLY):**
- `src/codegen/src/ppc64/mod.rs` (7222 LOC) — `Gpr` enum (`:54-87`), `Fpr` enum (`:202-235`), `CrField` enum (`:369-378`), encoding helpers (`encode_d_form` `:428`, `encode_x_form` `:446`, `encode_xo_form` `:457`, `encode_word` `:423`), `PPC64Backend` struct (`:1924`), `new()`/`new_le()` (`:1932`/`:1941`), `try_real_regalloc` (`:3011-3040`), `PPC64Backend::allocate_registers` (`:3084-4869`, ~1785 lines of inline stack-slot ISel), prologue (`:3177-3255`), `ss_load_imm` (`:2194`), `ss_store_to_slot` (`:2388`), `ss_load_value` (`:2645`), `ss_emit_cmp` (`:2666`), Syscall arm (`:4642-4694`), syscall stub table (`:5346+`).
- `src/codegen/src/ppc64/disasm.rs` (1161 LOC) — `Instruction::decode` (read-only reference; not in the emit path).
- `src/codegen/src/ppc64le.rs` (593 LOC) — `PPC64LEBackend` wraps `PPC64Backend`; `allocate_registers` delegates at `:400-406`; `encode_function`/`encode_program`/`return_stub`/`trampoline` byte-swap BE→LE (`:408-434`).
- `src/codegen/src/emit.rs` (9516 LOC) — `Emitter::emit_function` (`:959`), `emit_function_regalloc` (`:1056`, aarch64-only — confirmed: takes `&AllocationResult`, seeds aarch64 `Register::X0..X7`, uses `STP`/`LDP`), `emit_function_stack_slot` (`:3496`). ppc64 does NOT dispatch through `Emitter::emit_function` (see §1.6).
- `src/codegen/src/regalloc.rs` (6594 LOC) — `LinearScanAllocator` (`:1214`, aarch64-hardcoded), `TargetAgnosticRegAlloc` (`:2742`, target-agnostic — used by ppc64 via `try_real_regalloc`), `AllocationResult` (`:480`, aarch64), `RegAllocResult` (`:3224`, target-agnostic), `GenericSpillSlot` (`:3318`), `GenericSpillCode` (`:3355`), `gen_spill_reload` (`:3145`, uses `PhysicalReg::new(class, 0)` as scratch — ppc64 R0 hazard, see §7.5), `verify_callee_saved` (`:4860`, aarch64-only — hard-coded X0–X18/X19–X28/X29/X30/X31).
- `src/codegen/src/target_desc.rs` (3018 LOC) — `ppc64_target_desc()` (`:2320-2508`), `frame_pointer()` builder (`:1140-1143`, does NOT set `is_allocatable = false` — same latent bug as x86_64 RBP / riscv64 x8, see §6 gap 1), `toc_pointer()` builder, `stack_pointer()` builder.
- `src/codegen/src/syscall_abi.rs` — `translate_or_warn(BackendKind::PowerPC64, nr)`: generic 220 → ppc64 native 120 (clone); generic 221 → 11 (execve). IR uses generic numbers; native translation happens at emit time.
- `src/codegen/src/backend.rs:3214-3238` — aarch64 `contains_fork` (checks generic `nr == 220 || nr == 221`).
- `scripts/audit/regalloc_endianness_wave2_x86_64_design.md` — R2-a-audit x86_64 design doc (template).
- `scripts/audit/completion_wave_c_riscv64_design.md` — CC-a-audit riscv64 design doc (three-operand ISA parallel).

---

## 1. Current ppc64 Emission Path (stack-slot ISel)

**Entry point:** `PPC64Backend::allocate_registers` at `ppc64/mod.rs:3084`. The body is **~1785 lines** of inline stack-slot ISel, ending at `:4869` with `Ok(allocated)`. The final 6 lines:

```rust
        // Step 2: run the real target-agnostic linear-scan allocator and,
        // on success, annotate the AllocatedFunction with its decisions.
        if let Some(alloc) = try_real_regalloc(func) {
            crate::regalloc_emit::annotate_with_regalloc(&mut allocated, &alloc);
        }

        Ok(allocated)
    }
```
(`ppc64/mod.rs:4864-4869`)

**Observations:**

1. **Bytes are stack-slot only.** Every vreg gets an 8-byte stack slot at `[R31 - offset]` (negative FP-relative; `vreg_stack_slots: HashMap<u32, i32>` populated at `:3165-3172`; R31 is set up as the old-SP / frame-top pointer via `ADDI R31, R1, frame_size` at `:3243-3255`). Each instruction loads its operands into a fixed set of scratch registers (`R3`, `R4`, `R5`, `R6`, `R12` — see the `Add` arm at `:3415+`), performs the op, and stores the result back. The typical pattern is `ss_load_value(lhs, slots, R3); ss_load_value(rhs, slots, R4); encode Add { rt: R3, ra: R3, rb: R4 }; ss_store_to_slot(R3, dst_offset)`.

2. **`try_real_regalloc` already works.** At `:3011-3040`, the ppc64 backend constructs `TargetAgnosticRegAlloc::new(target)` (target = ppc64 TargetDesc from `TargetDescRegistry::new().get("ppc64")`) and calls `allocate_function(func)` to obtain a `RegAllocResult`. Returns `Some` on success, `None` on failure (with `vuma_log!(debug, ...)`). The result is currently used **only for metadata annotation** of the stack-slot bytes — the encoded bytes do not honour `vreg_to_preg` / `spill_code` / `used_callee_saved`.

3. **No `emit_function_regalloc` method exists on `PPC64Backend`.** Unlike x86_64 (`mod.rs:3962-3975`) and riscv64 (`riscv64.rs:4162-4174`), which have a metadata-only `emit_function_regalloc` method, ppc64's inline stack-slot ISel runs unconditionally inside `allocate_registers` itself and appends the metadata annotation at the end. There is no separate method to rename. CD-b-impl introduces the byte-changing path directly in `allocate_registers` (env-var gated), with the metadata annotation retained as a fallback.

4. **Per-function structural invariants are embedded in the prologue.** The stack-slot ISel's prologue (`:3177-3255`) does more than `STDU R1, -fs(R1); MFLR R0; STD R0, fs-24(R1); STD R31, fs-16(R1); ADDI R31, R1, fs`:
   - Computes a stack layout with reserved save-area slots: `[R31 - 0]` back chain, `[R31 - 16]` saved R31, `[R31 - 24]` saved LR (in the **callee's own frame** — see the comment at `:3136-3143` documenting the bug fix that moved LR from caller's frame to callee's frame), `[R31 - 32]` unused, then vreg slots starting at `[R31 - 40]`.
   - The LR-save-at-`fs-24(R1)` fix (`:3202-3226`) is critical: the previous code saved LR at `SP + fs + 16` (the caller's `SP + 16`), which overwrote one of the caller's vreg slots and silently corrupted caller vregs on every call (breaking bsearch, quicksort, matrix, mem_nested_alloc).
   - The register-based emitter must either (a) replicate this prologue verbatim, or (b) extract it into a shared `emit_prologue_common()` helper. Option (b) is preferred — see §5.5.

5. **No `contains_fork` opt-out exists on ppc64 today.** The Wave-1 `contains_fork` fork-detection helper was added to `AArch64Backend::allocate_registers` (`backend.rs:3226-3238`) but **not** to `PPC64Backend::allocate_registers`. Because ppc64's byte path is currently stack-slot (no callee-saved prologue beyond `STD R31`, no register-state preservation across the `SC` syscall), this has been harmless. Once ppc64 gains a register-based emitter with `STD R14; STD R15; ...` callee-saved saves, the same fork hazard surfaces and ppc64 needs its own `contains_fork`. The predicate checks the **generic** (asm-generic unified) IR-level syscall number — `nr == 220` (clone) or `nr == 221` (execve, defensive) — **the SAME as aarch64 and riscv64**, because the IR uses generic numbers and `translate_or_warn(BackendKind::PowerPC64, 220)` converts to ppc64-native 120 at emit time (`syscall_abi.rs:553`). See §7.4 for the full discussion and the discrepancy with the x86_64 design doc.

6. **ppc64 does NOT dispatch through `Emitter::emit_function`.** The ppc64 backend's `allocate_registers` is entirely self-contained — it never touches `emit.rs`'s `Emitter`. This is unlike aarch64, which goes through `Emitter::emit_function(func, None)` → `emit_function_stack_slot` (the `Emitter` is hard-coded `BackendKind::AArch64`). The ppc64 register-based emitter will similarly live in the `ppc64/` module tree (proposed: `src/codegen/src/ppc64/reg_isel.rs`), not in `emit.rs`.

7. **No env-var gate exists today.** `grep VUMA_REAL_REGALLOC ppc64/mod.rs` returns no matches. The metadata annotation at `:4864-4866` runs unconditionally on every function. CD-b-impl must introduce `VUMA_REAL_REGALLOC_PPC64` (default off) gating the byte-changing path; the metadata annotation can stay unconditional.

8. **ppc64le inherits via delegation.** `PPC64LEBackend::allocate_registers` at `ppc64le.rs:400-406` is a one-line delegation: `self.inner.allocate_registers(func)`. Any byte-changing register-based path added to `PPC64Backend::allocate_registers` is automatically inherited by ppc64le. The ppc64le wrapper's `encode_function` (`:408-414`) and `encode_program` (`:416-422`) then byte-swap the BE instruction words to LE. **No ppc64le-side changes are needed for the register-based emitter.** See §6.1.

---

## 2. ppc64 Register File (PPC SVR4 ELFv2 ABI)

Source: `target_desc.rs:2320-2508` (`ppc64_target_desc()`) and `ppc64/mod.rs:54-410` (`Gpr`, `Fpr`, `CrField` enums).

### 2.1 GPR file (32 registers, R0–R31)

| Index | Reg  | ABI Role                          | Callee-saved? | Allocatable? | Notes |
|------:|------|-----------------------------------|---------------|--------------|-------|
| 0     | R0   | Volatile / scratch                | ❌ caller     | ✅ (per `Gpr::is_allocatable` `:98`, but see R0 hazard §7.5) | Special: `MFLR R0`, `MTLR R0`, `LI R0, syscall_nr` — used in prologue/syscall |
| 1     | R1   | Stack pointer                     | (special)     | ❌            | `stack_pointer()` at `:2325` |
| 2     | R2   | TOC pointer                       | (special)     | ❌            | `toc_pointer()` at `:2327`; callee must preserve R2 across calls (ELFv2) |
| 3     | R3   | Integer arg 0 / return value      | ❌ caller     | ✅            | `.arg(0).return_reg()` at `:2329` |
| 4     | R4   | Integer arg 1                     | ❌ caller     | ✅            | |
| 5     | R5   | Integer arg 2                     | ❌ caller     | ✅            | |
| 6     | R6   | Integer arg 3                     | ❌ caller     | ✅            | |
| 7     | R7   | Integer arg 4                     | ❌ caller     | ✅            | |
| 8     | R8   | Integer arg 5                     | ❌ caller     | ✅            | |
| 9     | R9   | Integer arg 6                     | ❌ caller     | ✅            | |
| 10    | R10  | Integer arg 7                     | ❌ caller     | ✅            | |
| 11    | R11  | Volatile (env pointer for indirect calls) | ❌ caller | ✅            | |
| 12    | R12  | Volatile (func pointer for indirect calls, ELFv2) | ❌ caller | ✅            | Used by `CallIndirect` arm at `:4724-4727` |
| 13    | R13  | Thread pointer                    | (special)     | ❌            | `.not_allocatable()` at `:2341` |
| 14    | R14  | (callee-saved)                    | ✅ callee     | ✅            | |
| 15    | R15  | (callee-saved)                    | ✅ callee     | ✅            | |
| 16–30 | R16–R30 | (callee-saved)                 | ✅ callee     | ✅            | |
| 31    | R31  | (callee-saved, frame pointer)     | ✅ callee     | (FP)          | `frame_pointer().callee_saved()` at `:2361`; used as old-SP / frame-top base in stack-slot ISel |

**Caller-saved GPRs available for allocation (11):** R0, R3, R4, R5, R6, R7, R8, R9, R10, R11, R12. (R0 is technically allocatable per `Gpr::is_allocatable` at `:98-100`, but has special meaning in `MFLR`/`MTLR`/`LI R0, nr`/`SC` — the register-based emitter must avoid assigning vregs to R0 if any syscall or call occurs in the function. **Recommended:** mark R0 as non-allocatable in the TargetDesc for safety, or exclude R0 from the regalloc pool in the emitter — see §6 gap 3.)

**Callee-saved GPRs available for allocation (18):** R14–R31. (R31 is also the frame pointer — same latent allocatability bug as x86_64 RBP / riscv64 x8; see §6 gap 1.)

### 2.2 FPR file (32 registers, F0–F31)

| Index | Reg    | ABI Role                       | Callee-saved? |
|------:|--------|--------------------------------|---------------|
| 0     | F0     | FP return (secondary)          | ❌ caller     |
| 1     | F1     | FP arg 0 / FP return (primary) | ❌ caller     |
| 2–13  | F2–F13 | FP args 1–12                   | ❌ caller     |
| 14–31 | F14–F31| (callee-saved)                 | ✅ callee     |

**Caller-saved FPRs (14):** F0–F13. **Callee-saved FPRs (18):** F14–F31. Both correctly marked in `target_desc.rs:2363-2396` and mirrored by `Fpr::is_callee_saved()` at `:244-266`.

### 2.3 Condition Register (8 fields, CR0–CR7)

The PPC64 CR is a 32-bit register divided into 8 4-bit fields (CR0–CR7). Each field has 4 bits: LT (bit 0), GT (bit 1), EQ (bit 2), SO (bit 3). Compare instructions write to a specified CR field; conditional branches test a specified bit.

| Field | ABI Role                                   | Callee-saved? |
|-------|--------------------------------------------|---------------|
| CR0   | Default for integer compares               | ❌ caller     |
| CR1   | Default for FP compares (result of FCMPU)  | ❌ caller     |
| CR2   | (callee-saved)                             | ✅ callee     |
| CR3   | (callee-saved)                             | ✅ callee     |
| CR4   | (callee-saved)                             | ✅ callee     |
| CR5   | Volatile                                   | ❌ caller     |
| CR6   | Volatile (string ops)                      | ❌ caller     |
| CR7   | Volatile                                   | ❌ caller     |

Per `CrField::is_callee_saved()` at `:407-409`: CR2, CR3, CR4 are callee-saved. **TargetDesc gap:** `target_desc.rs:2435-2442` registers CR0–CR7 as `cond_reg` but does NOT mark CR2/CR3/CR4 with `.callee_saved()`. The `CrField::is_callee_saved()` enum helper is correct, but the TargetDesc (which `TargetAgnosticRegAlloc` consults) does not propagate this. See §6 gap 2.

### 2.4 Special registers (not in the GPR/FPR file)

| Register | Role | Save convention |
|----------|------|-----------------|
| LR (Link Register) | Return address | Callee-saved (callee must save if it makes a call). Saved via `MFLR R0; STD R0, slot(R1)` in prologue. |
| CTR (Count Register) | Branch-to-count target for `BCCTR`/`BCTRL` | Caller-saved (volatile across calls). Used by `CallIndirect` at `:4727-4730`. |
| XER | Fixed-point exception register (SO, OV, CA bits) | Mostly caller-saved; XER.SO mirrors CR0.SO. |
| VRSAVE | AltiVec register-save bitmask | callee-saved (AltiVec ABI). Not used by VUMA (no SIMD). |

The TargetDesc registers LR and CTR as `special_reg` at `:2444-2445` — these are not allocatable by the linear-scan allocator (which only consumes GPR/FPR pools).

### 2.5 Calling convention summary (ELFv2)

| Aspect                     | Value                                  |
|----------------------------|----------------------------------------|
| Integer arg registers      | R3, R4, R5, R6, R7, R8, R9, R10 (in order, 8 registers) |
| FP arg registers           | F1, F2, ..., F13 (up to 13 registers)  |
| Integer return             | R3 (R4 for 128-bit)                    |
| FP return                  | F1 (F2 for complex)                    |
| Stack alignment at `call`  | 16 bytes                               |
| Callee-saved GPRs          | R14–R31 (18 registers)                 |
| Callee-saved FPRs          | F14–F31 (18 registers)                 |
| Callee-saved CR fields     | CR2, CR3, CR4                          |
| Link register              | LR (callee must save if it calls)      |
| Branch delay slots         | (none)                                 |
| TOC pointer                | R2 (callee must preserve; ELFv2 caller passes own TOC in R12 for indirect calls) |
| Stack frame leaf           | `STDU R1, -fs(R1); MFLR R0; STD R0, fs-24(R1); STD R31, fs-16(R1); ADDI R31, R1, fs` |
| Stack frame epilogue       | `LD R1, 0(R1)` (stack-restore via back chain); `MTLR R0; BLR` (or `LD R31, -16(R1); BCLR 20, 0, 0`) |
| Syscall nr register        | R0 (Linux ppc64)                       |
| Syscall arg registers      | R3, R4, R5, R6, R7, R8 (6 args, same as calling convention) |
| Syscall return             | R3 (positive errno with CR0.SO=1 on failure — converted to `-errno` via `BC +2; NEG R3, R3` at `:4685-4686`) |
| Function descriptors       | (none — ELFv2 uses direct code pointers; R12 carries the func pointer for indirect calls) |

### 2.6 Three-operand ISA (no two-operand constraint)

PPC64 arithmetic is **three-operand** (`add RT, RA, RB` computes `RT = RA + RB`, distinct from both RA and RB). This is the same as aarch64 and riscv64, and UNLIKE x86_64's two-operand `add dst, src`. **No `mov dst, lhs` insertions needed; no `eliminated_copies` field needed.** This is a significant simplification vs x86_64 — the linear-scan allocator's `vreg_to_preg` mapping can be honoured directly without post-processing for two-operand coalescing.

### 2.7 Fixed 32-bit instruction encoding (no variable-length)

PPC64 instructions are fixed 4 bytes, big-endian stored in memory (ppc64) or little-endian (ppc64le — handled by `swap_instruction_words` at `ppc64le.rs:383-389`). No REX prefix, no variable-length fall-through risk. The `encode_word` helper at `:423` writes `word.to_be_bytes()`; the ppc64le wrapper flips each 4-byte chunk after emission.

### 2.8 D-form / DS-form displacement range

PPC64 load/store instructions use D-form (16-bit signed displacement, `-32768..32767`) or DS-form (14-bit signed displacement with 2-bit zero extension, must be 4-byte aligned). For frame offsets outside the D-form range, the existing `ss_store_to_slot` (`:2388-2438`) and `ss_load_from_slot` helpers fall back to `ADDI R12, R31, neg_off; STD src, 0(R12)` (clobbering R12 as scratch). The register-based emitter inherits these helpers.

### 2.9 Big-endian load/store hazard (ppc64 only, not ppc64le)

The stack-slot ISel has a ppc64-specific workaround at `:3303-3410` ("big-endian U8-load workaround"): when a function has exactly ONE U8 load, ZERO stores, ZERO comparisons, and a non-pointer integer return type wider than U8 — AND the load's address vreg was defined by an Add/Offset/BinOp(Add) whose offset operand is a VARIABLE — the load width is upgraded to the function's return type. This is because on big-endian ppc64, a U8 load (LBZ) of a U32/U64-stored value reads the HIGH byte (which is 0 for small positive values), silently zeroing every array read. The register-based emitter must inherit this workaround verbatim. (ppc64le is little-endian and unaffected.)

---

## 3. What `emit_function_regalloc` Needs to Do for ppc64

The Wave-1 aarch64 wire-up added 6 things to `AArch64Backend::allocate_registers` (see `backend.rs:3175-3300`). The ppc64 wire-up needs the **same 6 things**, but every concrete encoder call is ISA-specific:

1. **Env-var gate.** Read `VUMA_REAL_REGALLOC_PPC64` (default off). When unset, run today's stack-slot path. When set, attempt the register-based path. (ppc64le inherits — no separate `VUMA_REAL_REGALLOC_PPC64LE` env var needed; the ppc64le backend delegates to `PPC64Backend::allocate_registers` which checks the ppc64 env var.)

2. **Fork opt-out.** Detect functions containing `IRInstr::Call { func: "spawn_worker"|"fork" }` OR `IRInstr::Syscall { nr: 220|221 }` (generic asm-generic unified syscall numbers — `220` = clone, `221` = execve (defensive catch); `translate_or_warn(BackendKind::PowerPC64, 220)` converts to ppc64-native `120` at emit time per `syscall_abi.rs:553`). For these, fall back to stack-slot — the register-based prologue's callee-saved `STD R14; STD R15; ...; LD ...; LD ...` doesn't interact correctly with `clone()` because the child process runs with a different register state. (Same hazard as Wave-1 R1-b2-fix on aarch64.) **Note:** the protocol's mention of "120/189" refers to ppc64-native numbers, but the IR-level `nr` is generic — the predicate checks generic 220/221, matching aarch64 and riscv64. See §7.4 for the full discussion.

3. **Run the allocator.** Call `try_real_regalloc(func)` (already exists at `:3011-3040`). On `Some(alloc)`, proceed; on `None`, fall back to stack-slot.

4. **(Optional) Callee-saved verifier.** If `VUMA_VERIFY_CALLEE_SAVED=1`, run a verifier analogous to `regalloc::verify_callee_saved` (`regalloc.rs:4860`) — but parameterized for ppc64 (caller-saved GPRs = 0, 3–12; callee-saved = 14–31 from `used_callee_saved`; always-allowed = 1 (R1/SP), 2 (R2/TOC), 13 (R13/thread); caller-saved FPRs = 0–13; callee-saved FPRs = 14–31). The existing `verify_callee_saved` is hard-coded to aarch64's `PhysReg::Gpr(Register)` with `r.encoding()` checked against X0–X18 / X19–X28 / X29 / X30 / X31 — **it cannot be called on a `RegAllocResult`** (different `PhysicalReg` type: `crate::backend::PhysicalReg { class, index }` vs aarch64's `regalloc::PhysReg::Gpr(Register)`). See §5.3 for the new ppc64 verifier.

5. **Emit register-based bytes.** Call the new `ppc64::reg_isel::allocate_registers(func, &alloc)` (proposed module, see §5). This produces an `AllocatedFunction` whose `encoded` bytes honour `alloc.vreg_to_preg` (operands stay in registers across instructions where possible), `alloc.spill_code` (boundary spills/reloads), and `alloc.used_callee_saved` (prologue `STD R14, slot(R1); STD R15, slot+8(R1); ...` / epilogue `LD R14, slot(R1); ...`).

6. **Fall back on allocator failure.** If `try_real_regalloc` returns `None`, or if the new `reg_isel::allocate_registers` returns `Err`, run today's inline stack-slot ISel (the body at `:3084-4869`). This preserves the existing safety guarantee.

**Out of scope for CD-b-impl (deferred to a separate PR):**
- The `EmitResult` API change proposed in F2-a §7.2 (returning `frame_size` + `callee_saved` from the emitter). Non-breaking addition but touches all callers; not needed for correctness.
- No `emit_function_regalloc` rename needed (ppc64 has no such method today, unlike x86_64/riscv64 — see §1.3).

---

## 4. Reusable Components From aarch64's `emit_function_regalloc`

The aarch64 implementation at `emit.rs:1056-1354` is **aarch64-only** at the byte level — it uses `Register::X0..X30`, `Instruction::SUB/STP/ADD` (aarch64 enums), `compute_frame_size`, `emit_callee_saved_saves`, `emit_spill_reload`, `emit_terminator_regalloc`, `emit_ir_instr` (the aarch64 greedy emitter). None of these concrete calls port to ppc64.

What IS reusable is the **structural pattern**:

| aarch64 component (location)                                 | ppc64 analogue (proposed)                                 | Reuse level |
|--------------------------------------------------------------|------------------------------------------------------------|-------------|
| Allocator result consumed (`AllocationResult`, aarch64)      | `RegAllocResult` (target-agnostic, already produced by `try_real_regalloc`) | ✅ Direct — `RegAllocResult` already has `vreg_to_preg`, `spill_slots`, `total_spill_slots`, `used_callee_saved`, `spill_code`, `coalesced_map`. The fields map 1:1. |
| Position-based spill insertion (`pos += 2` per instr, `spill_code.get(&pos)` for pre-instr, `&(pos+1)` for post-instr) | Same `pos += 2` convention | ✅ Direct — `LiveRangeComputer::compute` (regalloc.rs:863) is shared by both allocators, so positions match. |
| Callee-saved prologue sequence (`emit_callee_saved_saves`)   | `STD R14, slot(R1); STD R15, slot+8(R1); ...; STD R31, slot+N(R1)` (in increasing index order, using D-form `STD` with `R1`-relative offsets) | 🔄 Pattern — same idea, different bytes. ppc64 uses `STD` (DS-form) not `STP`; saves go into the callee's own frame (below the back chain / LR save), not above RBP like x86_64. |
| Calallee-saved epilogue (`emit_terminator_regalloc`)         | `LD R31, slot+N(R1); ...; LD R15, slot+8(R1); LD R14, slot(R1)` (reverse order — though on ppc64 order doesn't matter since each `LD` targets a distinct register, unlike x86_64 `push`/`pop` which is stack-order-dependent) | 🔄 Pattern |
| Copy-elision skip (`is_eliminated_copy`, `emit.rs:1256`)     | Skip `IRInstr::Cast { kind: BitCast }` whose src & dst resolve to same `PhysicalReg` | 🚧 Adapted — `RegAllocResult` has no `eliminated_copies` field (only `AllocationResult` does). Either (a) skip only when src/dst resolve to same preg via `get_phys_reg`, or (b) add `eliminated_copies` to `RegAllocResult`. Option (a) is simpler. (Same as x86_64 §4.) |
| Param-vreg preassignment (X0–X7, `emit.rs:1086-1102`)        | R3–R10 (8 integer arg regs) — must NOT be overridden by `alloc.vreg_to_preg` | 🔄 Pattern — same hazard documented in `emit.rs:1112-1141` (R1-b-impl fix). Reuse the param-vreg skip-set logic verbatim, swapping in the ppc64 arg register set. |
| Spill-slot frame layout (`spill_area_aligned + callee_saved_size`, `emit.rs:1155-1180`) | Same two-region layout: `[spill area]` ← R31-relative negative offsets; `[callee-saved save area]` ← inside the callee's own frame at fixed slots below LR save | 🔄 Pattern — ppc64's frame layout is documented at `:3128-3155`. The callee-saved saves go INTO the callee's frame (via `STD Rn, slot(R1)` where slot is inside `[R1, R1+fs)`), not above the frame like x86_64's `push`. See §5.4. |
| Verifier hook (`verify_callee_saved`, regalloc.rs:4860)      | New `verify_callee_saved_ppc64` (see §5.3)                | 🚧 New — aarch64's is hard-coded to aarch64's `PhysReg` enum and X0–X18/X19–X28 encoding ranges. |
| Fork opt-out (`contains_fork`, backend.rs:3226)              | Same predicate, checking generic `nr == 220 || nr == 221` (same as aarch64 — see §7.4) | ✅ Near-verbatim copy — ppc64 uses the same generic syscall numbers as aarch64/riscv64 (unlike x86_64 which the design doc incorrectly claims uses 56/58 — see §7.4). |
| Syscall-position tracking (`regalloc.rs:954`)                | Already shared via `LiveRangeComputer` (G6 fix)            | ✅ Already in place — applies to `TargetAgnosticRegAlloc` too. |
| Three-operand form (aarch64 `ADD Xd, Xn, Xm`)                | ppc64 `ADD RT, RA, RB` (three-operand) — same shape, no two-operand coalescing | ✅ Direct — unlike x86_64's two-operand constraint (§2.6). |

**The single biggest non-reuse item:** the aarch64 regalloc emitter delegates per-instruction byte emission to `emit_ir_instr` (the greedy emitter), which already supports a `reg_alloc.resolve_reg` mechanism. The ppc64 stack-slot ISel has **no equivalent** — every arm hard-codes `ss_load_value(id, R3)` / `ss_store_to_slot(R3, offset)`. This means the ppc64 reg_isel must either (a) introduce a `resolve_vreg(id) -> RegOrSlot` abstraction and rewrite every arm, or (b) special-case "dst in register" vs "dst spilled" per arm. See §5. (Same situation as x86_64 §4 and riscv64 §4.)

---

## 5. New Components Needed (ppc64-specific)

### 5.1 `src/codegen/src/ppc64/reg_isel.rs` (new module)

**Public API:**

```rust
/// Register-based ppc64 emitter.  Consumes a `RegAllocResult` and
/// produces an `AllocatedFunction` whose `encoded` bytes honour the
/// allocator's register assignments, spill code, and callee-saved set.
///
/// Returns `Err` if any IR instruction is not yet supported by the
/// register-based path; the caller falls back to the inline stack-slot ISel.
pub fn allocate_registers(
    func: &IRFunction,
    alloc: &crate::regalloc::RegAllocResult,
) -> Result<AllocatedFunction, BackendError>;
```

**Internal structure (mirror the aarch64/riscv64 pattern, but per-arm rewrite):**

1. **`resolve_vreg(id) -> RegOrSlot`** helper:
   ```rust
   enum RegOrSlot {
       Gpr(Gpr),                     // vreg is in this physical GPR
       Fpr(Fpr),                     // vreg is in this physical FPR
       Spill { offset: i32 },        // vreg is spilled to [R31 - offset] (negative R31-relative)
       Immediate(i64),               // operand is a constant (for IRValue::Immediate)
   }
   ```
   Look up the vreg in `alloc.vreg_to_preg`; if absent, look up in `alloc.spill_slots`; if absent, the vreg is undefined (panic in debug, fall back to scratch in release).

2. **`PhysicalReg` → `Gpr`/`Fpr` translation.** The ppc64 TargetDesc uses `RegDesc.index` 0..31 for GPRs and 0..31 for FPRs (`target_desc.rs:2321-2442`). The `Gpr` enum has the **same** discriminant values (`ppc64/mod.rs:54-87`). Translation is trivial:
   ```rust
   fn preg_to_gpr(p: crate::backend::PhysicalReg) -> Option<Gpr> {
       if p.class != crate::backend::RegClass::Gpr { return None; }
       // ppc64 Gpr discriminants are 0..31 (R0..R31), matching TargetDesc.index
       match p.index {
           0  => Some(Gpr::R0),   1  => Some(Gpr::R1),   2  => Some(Gpr::R2),
           3  => Some(Gpr::R3),   4  => Some(Gpr::R4),   5  => Some(Gpr::R5),
           6  => Some(Gpr::R6),   7  => Some(Gpr::R7),   8  => Some(Gpr::R8),
           9  => Some(Gpr::R9),   10 => Some(Gpr::R10),  11 => Some(Gpr::R11),
           12 => Some(Gpr::R12),  13 => Some(Gpr::R13),  14 => Some(Gpr::R14),
           15 => Some(Gpr::R15),  16 => Some(Gpr::R16),  17 => Some(Gpr::R17),
           18 => Some(Gpr::R18),  19 => Some(Gpr::R19),  20 => Some(Gpr::R20),
           21 => Some(Gpr::R21),  22 => Some(Gpr::R22),  23 => Some(Gpr::R23),
           24 => Some(Gpr::R24),  25 => Some(Gpr::R25),  26 => Some(Gpr::R26),
           27 => Some(Gpr::R27),  28 => Some(Gpr::R28),  29 => Some(Gpr::R29),
           30 => Some(Gpr::R30),  31 => Some(Gpr::R31),
           _  => None,
       }
   }
   ```
   Similarly `preg_to_fpr` for `RegClass::SimdFp` indices 0..31 → `Fpr::F0..F31`. **Note:** the emitter should treat `Gpr::R0`, `Gpr::R1`, `Gpr::R2`, `Gpr::R13` as reserved (R0 = syscall/MFLR scratch, R1 = SP, R2 = TOC, R13 = thread) — if the allocator assigns a vreg to one of these, fall back to the stack-slot path (or use a scratch-copy approach). See §7.5 R0 hazard.

3. **Per-IR-instruction arms.** Approximately 30 distinct arms (matching the existing stack-slot ISel's coverage). For each, decide:
   - If dst is in a register and both operands are in registers (or immediates): emit the register-form op directly (e.g. `Instruction::Add { rt: dst_gpr, ra: lhs_gpr, rb: rhs_gpr }.encode()`).
   - If dst is in a register but lhs is spilled: `LD dst_gpr, [R31 - lhs_off]; op dst_gpr, dst_gpr, <rhs>` (use `ss_load_from_slot` helper at `:2388`-area).
   - If dst is spilled: emit the stack-slot pattern (load into R3, op, store to dst slot) — this is **identical to today's stack-slot ISel arm**, so the existing code at `:3415+` (Add) etc. can be lifted verbatim into a `dst_spilled` helper.
   - **No two-operand constraint:** ppc64 is three-operand (`Add RT, RA, RB`), so `dst != lhs` is fine without an extra `MR dst, lhs` (unlike x86_64 §2.4). Same simplification as riscv64.

4. **Spill/reload insertion.** At each instruction boundary (`pos` and `pos+1`), walk `alloc.spill_code.get(&pos)` / `&(pos+1)` and emit:
   - `Reload`: `LD <scratch>, [R31 - slot.offset]` (GPR) or `LFD <fscratch>, [R31 - slot.offset]` (FPR).
   - `Spill`: `STD <scratch>, [R31 - slot.offset]` (GPR) or `STFD <fscratch>, [R31 - slot.offset]` (FPR).
   - The `GenericSpillCode` enum (target-agnostic) has `preg: PhysicalReg` and `slot: GenericSpillSlot` fields. The `preg` field on ppc64 may be `PhysicalReg::new(Gpr, 0)` (R0) — **do NOT honor this literally for the scratch register**; instead, use the vreg's actual location from `alloc.vreg_to_preg[vreg]` for the value being spilled/reloaded, and use a real scratch (R3/R4 for GPRs, F0/F1 for FPRs) for the load/store if the slot offset exceeds D-form range. See §7.5 R0 hazard (analogous to riscv64's Zero-register hazard, but less severe because R0 IS a real register — it's just reserved for syscall/MFLR use).
   - The `slot.offset` field is already an `i32` displacement. For ppc64, the displacement is R31-relative (negative = below R31 = inside the callee's frame). Translation: `LD scratch, -offset(R31)` where `-offset` is the negated slot offset. For offsets outside `[-32768, 32767]`, use the existing `ss_load_from_slot`/`ss_store_to_slot` helpers at `:2388+` (which clobber R12 as scratch for large offsets).

5. **Prologue.** Order (mirror the stack-slot ISel at `:3177-3255` with callee-saved additions):
   1. `STDU R1, -frame_size(R1)` (allocate frame — same as stack-slot; sets back chain at `[R1+0]`).
   2. `MFLR R0; STD R0, fs-24(R1)` (save LR in callee's own frame at `SP + fs - 24` — see `:3202-3226` for the bug-fix rationale).
   3. `STD R31, fs-16(R1); ADDI R31, R1, fs` (save old R31, set up R31 = old SP / frame-top base).
   4. **Callee-saved saves (NEW for reg_isel):** for each `preg` in `alloc.used_callee_saved` (sorted by index, GPRs only initially — FPRs deferred to Phase Db), emit `STD Rn, save_off(R1)` where `save_off` is a distinct slot inside `[R1, R1+fs)` (e.g. starting at `fs-32` and growing downward). Track the offsets so the epilogue can restore them. **Important:** the save area must NOT collide with the vreg spill slots or the structural-invariant slots — see §5.4.
   5. **(Optional, if CR2/CR3/CR4 are used) CR save:** `MFCR R0; STD R0, cr_save_off(R1)` — only if the regalloc allocates CR fields (currently the TargetDesc doesn't mark CR2/3/4 as callee-saved, so the allocator won't allocate them — see §6 gap 2). Defer to Phase Dc.
   6. **Per-function structural invariants** (cap sig, formal-verify counter, channel seq counter, proto state, circuit breaker state): reuse the stack-slot ISel's prologue code verbatim. **Critical:** the spill-area offsets used by these invariants must NOT collide with `alloc.spill_slots` offsets — see §7.7 for the frame-layout drift risk.

6. **Epilogue.** For each `IRInstr::Ret` (or `IRTerminator::Return`):
   1. Move return value into R3 (if not already there).
   2. **Callee-saved restores (NEW for reg_isel):** `LD Rn, save_off(R1)` for each saved register (reverse order not required on ppc64 since each `LD` targets a distinct register, but emit in reverse for symmetry with the prologue).
   3. `LD R0, fs-24(R1); MTLR R0` (restore LR).
   4. `LD R31, fs-16(R1)` (restore old R31).
   5. `LD R1, 0(R1)` (stack-restore via back chain) — or `ADDI R1, R1, fs`.
   6. `BLR` (`BCLR 20, 0, 0` — branch to LR unconditionally).

7. **Argument-register preassignment.** For the first 8 integer params, force the param vreg to live in R3–R10 (in that order) regardless of what `alloc.vreg_to_preg` says. The allocator doesn't know about ABI arg registers (this is the R1-b-impl fix documented at `emit.rs:1112-1141`). Implementation: build a `param_vregs: HashSet<u32>` and skip `vreg_to_preg` lookups for those vregs during the param-loading prologue sequence (which `MR`-copies them from the arg reg into their assigned reg if the allocator picked a different one, or emits nothing if the allocator already picked the arg reg). The existing stack-slot ISel stores params from R3–R10 to stack slots at `:3257-3281` — the register-based emitter instead keeps them in their assigned registers (or spills if the allocator decided to).

### 5.2 Wire-up in `PPC64Backend::allocate_registers`

**File:** `src/codegen/src/ppc64/mod.rs:3084-4869`.

**Sketch** (mirrors aarch64's `backend.rs:3175-3300`):

```rust
fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
    let func_name = func.name.clone();
    let real_regalloc = std::env::var("VUMA_REAL_REGALLOC_PPC64")
        .map(|v| v == "1")
        .unwrap_or(false);
    let verify_callee_saved = std::env::var("VUMA_VERIFY_CALLEE_SAVED")
        .map(|v| v == "1")
        .unwrap_or(false);

    // CD-a-audit: fork opt-out. The IR uses GENERIC (asm-generic unified)
    // syscall numbers; translate_or_warn(BackendKind::PowerPC64, 220)
    // converts to ppc64-native 120 (clone) at emit time per
    // syscall_abi.rs:553. The predicate checks generic 220/221, SAME as
    // aarch64 and riscv64 (NOT the ppc64-native 120/189 — those are the
    // post-translation numbers the kernel sees, not the IR-level nr).
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
                if let Err(msg) = super::reg_isel::verify_callee_saved_ppc64(&alloc) {
                    panic!("verify_callee_saved_ppc64 FAILED for '{}': {}", func.name, msg);
                }
            }
            match super::reg_isel::allocate_registers(func, &alloc) {
                Ok(mut allocated) => {
                    // Metadata annotation is still useful (separate target-agnostic
                    // view for downstream consumers).  Keep as-is.
                    crate::regalloc_emit::annotate_with_regalloc(&mut allocated, &alloc);
                    return Ok(allocated);
                }
                Err(e) => {
                    vuma_log!(warn,
                        "ppc64 reg_isel failed for '{}': {}, falling back to stack-slot ISel",
                        func.name, e);
                    // fall through
                }
            }
        }
    } else if real_regalloc && contains_fork {
        vuma_log!(debug,
            "ppc64 regalloc: function '{}' contains spawn_worker/fork; \
             falling back to stack-slot ISel (fork+regalloc not supported)",
            func.name);
    }

    // Fallback (default path or regalloc failure): inline stack-slot ISel.
    // (The current ~1785-line body at ppc64/mod.rs:3084-4869 moves here unchanged.)
    // ... existing inline stack-slot ISel body ...
    // ... ending with:
    if let Some(alloc) = try_real_regalloc(func) {
        crate::regalloc_emit::annotate_with_regalloc(&mut allocated, &alloc);
    }
    Ok(allocated)
}
```

### 5.3 `verify_callee_saved_ppc64` (new verifier)

The existing `verify_callee_saved` (`regalloc.rs:4860`) is hard-coded to aarch64's `PhysReg` enum and register encoding ranges. It cannot be reused as-is. Add a sibling:

```rust
/// Verify that every physical register used by the regalloc is either
/// (a) caller-saved, (b) in `used_callee_saved`, or (c) R1 (SP) / R2 (TOC) /
/// R13 (thread) (always-reserved).  Mirrors `regalloc::verify_callee_saved`
/// but for the ppc64 register file and the target-agnostic `RegAllocResult`.
pub fn verify_callee_saved_ppc64(
    result: &crate::regalloc::RegAllocResult,
) -> std::result::Result<(), String> {
    // Allowed GPRs by index (PPC SVR4 ELFv2 ABI):
    //   caller-saved: 0 (R0), 3-12 (R3-R10 args/return, R11-R12 volatile)
    //   always-reserved: 1 (R1/SP), 2 (R2/TOC), 13 (R13/thread)
    //   callee-saved: from result.used_callee_saved (14-31 typically)
    let mut allowed_gprs: HashSet<u32> = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]
        .into_iter().collect();
    for preg in &result.used_callee_saved {
        if preg.class == crate::backend::RegClass::Gpr {
            allowed_gprs.insert(preg.index);
        }
    }
    // Caller-saved FPRs: 0-13 (F0-F13). Callee-saved: 14-31 (F14-F31).
    let mut allowed_fprs: HashSet<u32> = (0..=13).collect();
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
                         and not R1/R2/R13", preg.index));
                }
            }
            crate::backend::RegClass::SimdFp => {
                if !allowed_fprs.contains(&preg.index) {
                    return Some(format!(
                        "FPR index {} is not caller-saved and not in used_callee_saved",
                        preg.index));
                }
            }
            _ => return Some(format!("unexpected register class {:?}", preg.class)),
        }
        None
    };
    for (&vreg, &preg) in &result.vreg_to_preg {
        if let Some(msg) = check(preg) { return Err(format!("vreg {} -> {}: {}", vreg, preg, msg)); }
    }
    for (pos, codes) in &result.spill_code {
        for sc in codes {
            let preg = sc.phys_reg();
            if let Some(msg) = check(preg) { return Err(format!("spill@{} {:?}: {}", pos, sc, msg)); }
        }
    }
    Ok(())
}
```

(`sc.phys_reg()` is a convenience accessor on `GenericSpillCode` that CD-b-impl may need to add — currently the enum's spill/reload variants each carry `preg: PhysicalReg` field; expose it via a method or a `match`. Same accessor needed by x86_64's `verify_callee_saved_x86_64` and riscv64's verifier.)

### 5.4 ppc64 frame layout (proposed)

The aarch64 regalloc emitter uses a layout where the callee-saved area lives **below** SP (between SP and the FP/LR save pair). ppc64 cannot use the same layout because its frame convention places the back chain at `[SP+0]`, LR save at `[SP+fs-24]` (callee's own frame), and R31 save at `[SP+fs-16]` — the callee-saved saves must fit inside `[SP, SP+fs)` alongside the vreg spill slots.

**Proposed ppc64 layout (low → high addresses, after prologue):**

```text
   [SP + 0]                      ← Back chain (old R1) — set by STDU.
   [SP + 8 .. SP + fs - 24]      ← Vreg spill area + structural-invariant slots.
                                    Addressed via [R31 - offset] (negative R31-relative).
                                    Size = alloc.total_spill_slots * 8 + invariant_slots,
                                    aligned to 16.
   [SP + fs - 24]                ← Saved LR (MFLR R0; STD R0, fs-24(R1)).
   [SP + fs - 16]                ← Saved R31 (STD R31, fs-16(R1)).
   [SP + fs - callee_saved_size] ← Callee-saved GPR save area (NEW for reg_isel).
                                    Size = used_callee_saved.len() * 8, aligned to 16.
                                    Each register saved via STD Rn, slot(R1).
   [SP + fs]                     ← R31 = old SP / frame-top base (ADDI R31, R1, fs).
   [caller's frame]              ← Caller's RSP at call time.
```

**Critical:** the per-function structural invariants (cap sig, formal-verify counter, channel seq counter, proto state, circuit breaker state) currently use specific R31-relative offsets computed in the stack-slot ISel (`:3128-3155` — reserved slots at `[R31-0]`, `[R31-8]`, `[R31-16]`, `[R31-24]`, `[R31-32]`, then vreg slots from `[R31-40]`). The register-based emitter must use **different** offsets for `alloc.spill_slots` to avoid colliding with these reserved slots. Recommended approach: reserve the existing structural-invariant offsets as a "fixed region" (offsets 0..32 from R31), then place `alloc.spill_slots` *below* that region (offsets 40+ from R31), then place the callee-saved save area *above* the spill area but *below* the LR/R31 saves (i.e. at offsets `fs - 32 - callee_saved_size` growing downward).

### 5.5 Shared `emit_prologue_common()` helper (deferred)

The stack-slot ISel's prologue (`:3177-3255`) and the proposed reg_isel prologue share most of their structure (STDU, MFLR/STD LR, STD R31, ADDI R31). The differences are: (a) reg_isel adds callee-saved `STD Rn` saves, (b) reg_isel does NOT store params from R3–R10 to stack slots (they stay in registers). For CD-b-impl, take option (a) — copy the prologue code into `reg_isel.rs`. Refactor to a shared helper in a follow-up PR (CD-e-cleanup). Same recommendation as x86_64 §5.5 / riscv64 §5.5.

---

## 6. TargetDesc Readiness

The ppc64 TargetDesc at `target_desc.rs:2320-2508` is **mostly complete** but has 3 gaps that CD-b-impl must address.

| Required field                                   | Present? | Source |
|--------------------------------------------------|----------|--------|
| All 32 GPRs with ABI roles (R0–R31)              | ✅       | `:2321-2361` |
| All 32 FPRs (F0–F31)                             | ✅       | `:2362-2396` |
| VSX/AltiVec upper halves (VS32–VS63)             | ✅       | `:2397-2433` (but unused — no SIMD encoder, see §7.8) |
| CR fields (CR0–CR7)                              | ✅       | `:2434-2442` (but callee-saved markers missing, see gap 2) |
| Special registers (LR, CTR)                      | ✅       | `:2444-2445` |
| Caller-saved vs callee-saved GPR classification  | ✅       | `callee_saved()` on R14–R31 (`:2343-2361`) |
| Stack pointer (R1) marked non-allocatable        | ✅       | `stack_pointer()` at `:2325` |
| TOC pointer (R2) marked reserved                 | ✅       | `toc_pointer()` at `:2327` (sets non-allocatable) |
| Thread pointer (R13) marked non-allocatable      | ✅       | `.not_allocatable()` at `:2341` |
| Frame pointer (R31) marked                       | ⚠️       | `frame_pointer().callee_saved()` at `:2361` — `frame_pointer()` does NOT set `is_allocatable = false`; R31 remains allocatable — see gap 1 |
| Argument register positions                      | ✅       | `.arg(0)` through `.arg(7)` on R3–R10 (`:2329-2336`) |
| Return register (R3)                             | ✅       | `.return_reg()` on R3 (`:2329`) and F1 (`:2365`) |
| Calling convention descriptor (ELFv2)            | ✅       | `:2448-2464` (16-byte alignment, has LR, has TOC, no branch delay slots) |
| `TargetAgnosticRegAlloc` already produces `RegAllocResult` for ppc64 | ✅ | `try_real_regalloc` at `:3011-3040` proves this works in production today. |

**Gaps found in this audit:**

1. **R31 allocatability bug.** `RegDesc::gpr("R31", 31).frame_pointer().callee_saved()` at `target_desc.rs:2361` does not chain `.not_allocatable()`. The `frame_pointer()` builder at `target_desc.rs:1140-1143` does NOT set `is_allocatable = false`. This means `TargetAgnosticRegAlloc::new(target)` at `regalloc.rs:2768-2791` includes R31 in the callee-saved pool (it's marked callee-saved) and the linear-scan allocator may assign vregs to R31. If the register-based emitter also uses R31 as the frame-top base (the existing convention at `:3243-3255`), this is a conflict. **Fix:** add `.not_allocatable()` to the R31 line in `ppc64_target_desc()`. Same bug pattern as x86_64 RBP (`:1945`) and riscv64 x8 (`:1579`).

2. **CR field callee-saved markers missing.** `target_desc.rs:2435-2442` registers CR0–CR7 as `cond_reg` but does NOT mark CR2/CR3/CR4 with `.callee_saved()`. The `CrField::is_callee_saved()` enum helper at `ppc64/mod.rs:407-409` is correct (CR2/CR3/CR4), but the TargetDesc (which `TargetAgnosticRegAlloc` consults) does not propagate this. If the allocator ever allocates CR fields (currently the `cond_reg` class is likely excluded from the allocatable pool by `TargetAgnosticRegAlloc::new` — verify in CD-b-impl), the prologue must save CR2/CR3/CR4. **Fix:** add `.callee_saved()` to the CR2, CR3, CR4 lines in `ppc64_target_desc()`. (Low priority for CD-b-impl — CR fields are unlikely to be allocated in Phase Da; defer to Phase Dc.)

3. **R0 allocatability ambiguity.** `target_desc.rs:2323` registers R0 as `RegDesc::gpr("R0", 0)` with no `not_allocatable()` marker. The `Gpr::is_allocatable()` helper at `ppc64/mod.rs:98-100` returns `true` for R0 (it only excludes R0/R1/R2/R13... wait, the helper excludes `R0 | R1 | R2 | R13` — so R0 IS excluded from allocatable by the enum helper, but the TargetDesc does NOT mark R0 as non-allocatable). This is an inconsistency: the enum helper says R0 is non-allocatable, but the TargetDesc (which the regalloc consults) says R0 is allocatable. **Fix:** add `.not_allocatable()` to the R0 line in `ppc64_target_desc()` to match the enum helper and reserve R0 for syscall/MFLR use. (Alternatively, keep R0 allocatable but exclude it from the reg_isel's pool via a filter — see §7.5.)

4. **No FP-rel spill-slot offsets.** The `RegAllocResult.spill_slots` field is a `HashMap<IRValueId, GenericSpillSlot>`. The `GenericSpillSlot` struct (`regalloc.rs:3318`) has an `offset` field but it is unclear whether that offset is computed relative to the frame pointer or is just an index. CD-b-impl must verify the offset semantics and, if needed, translate slot indices to R31-relative offsets in the emitter. (Same gap as x86_64 §6 gap 2 and riscv64 §6 gap 3.)

### 6.1 ppc64le inheritance (automatic)

`PPC64LEBackend::allocate_registers` at `ppc64le.rs:400-406` delegates to `self.inner.allocate_registers(func)` — any byte-changing register-based path added to `PPC64Backend::allocate_registers` is automatically inherited by ppc64le. The ppc64le wrapper's `encode_function` (`:408-414`) byte-swaps each 4-byte instruction word BE→LE via `swap_instruction_words` (`:383-389`), and `encode_program` (`:416-422`) converts the full BE ELF to LE via `swap_be_elf_to_le` (`:207-372`). **No ppc64le-side changes are needed for the register-based emitter.** The `VUMA_REAL_REGALLOC_PPC64` env var gates both ppc64 and ppc64le (since ppc64le delegates).

**Conclusion:** TargetDesc readiness is **HIGH** with the R31-allocatability fix (gap 1) and the R0-allocatability fix (gap 3). Gap 2 (CR field markers) is low priority and can be deferred. No new TargetDesc fields are needed.

---

## 7. Risk Assessment

### 7.1 Fixed 32-bit instruction encoding — **LOW**

PPC64 instructions are fixed 4 bytes, big-endian stored (ppc64) or little-endian (ppc64le — handled by the wrapper). No REX prefix, no variable-length fall-through risk. The existing `encode_word` helper at `:423` writes `word.to_be_bytes()`; the ppc64le wrapper flips each 4-byte chunk after emission. The register-based emitter inherits this. **Mitigation:** none needed — the ppc64le wrapper's byte-swap is already battle-tested by the stack-slot ISel.

### 7.2 R31-relative addressing and D-form displacement range — **LOW**

ppc64 load/store instructions use D-form (16-bit signed displacement, `-32768..32767`) or DS-form (14-bit signed, 4-byte aligned). For frame offsets outside the D-form range, the existing `ss_store_to_slot` (`:2388-2438`) and `ss_load_from_slot` helpers fall back to `ADDI R12, R31, neg_off; STD src, 0(R12)` (clobbering R12 as scratch). The register-based emitter inherits these helpers. **Mitigation:** spot-check with a function that has a frame_size > 32768 (forcing the prologue's `STDU` and the spill-slot `STD` to use the `ADDI R12` workaround). Functions with > 4000 vregs would trigger this — the curated test suite is unlikely to have such functions, but a synthetic regression test is cheap.

### 7.3 Callee-saved tracking — **HIGH**

This is the same HIGH risk identified for aarch64 in F2-a §5.3 and Wave-1's G1/G2/G4 fixes. The hazards:

1. **Spill-scratch register clobbering callee-saved.** If a spill/reload path uses a callee-saved register as a scratch (the aarch64 path used X0; the ppc64 path must NOT use R14–R31 — only R0, R3–R12 are caller-saved scratches). **Mitigation:** the ppc64 stack-slot ISel uses R3/R4/R5/R6/R12 as scratch (see `:3257-3281` param-store loop using `arg_regs[i]`; the `ss_store_to_slot` helper clobbers R12 for large offsets at `:2401-2415`). The register-based emitter must follow the same convention.

2. **`used_callee_saved` set incompleteness.** If the linear-scan allocator misses a callee-saved register that a spilled-reload path uses as a scratch, the epilogue will restore garbage into it. **Mitigation:** the new `verify_callee_saved_ppc64` (§5.3) catches this. Wire it behind `VUMA_VERIFY_CALLEE_SAVED=1` for the curated test subset before flipping the default.

3. **Callee-saved register interaction with `clone()`.** The `clone` syscall returns in the child process with all registers in their pre-syscall state. If the parent had `STD R14, slot(R1)` in the prologue and the child reaches the epilogue, the child's `LD R14, slot(R1)` will load a value that the parent's store put on the child's stack copy — this is actually correct (clone copies the stack). The real hazard is that the child may execute a *different* code path (the `if pid == 0` branch) and clobber callee-saved registers without restoring them. **Mitigation:** the `contains_fork` opt-out (§3 step 2) sidesteps this entirely by falling back to stack-slot for fork-containing functions.

### 7.4 Fork + regalloc — **MEDIUM**

Same as Wave-1 R1-b2-fix on aarch64. The IR uses **generic** (asm-generic unified) syscall numbers: `nr == 220` (clone), `nr == 221` (execve — defensive catch). `translate_or_warn(BackendKind::PowerPC64, 220)` converts to ppc64-native `120` at emit time per `syscall_abi.rs:553`. The `contains_fork` predicate checks generic 220/221 — **the SAME as aarch64 (`backend.rs:3234`) and riscv64 (`riscv64.rs` CC-a-audit §3)**, because the IR-level `nr` is generic, not native.

**Discrepancy with the x86_64 design doc:** R2-a-audit (`scripts/audit/regalloc_endianness_wave2_x86_64_design.md` §3, §7.4) claims x86_64's predicate checks native `nr == 56 || nr == 58`. This is **likely incorrect** — x86_64 also uses generic IR numbers (`translate(BackendKind::X86_64, 220) → 56` per `syscall_abi.rs:419`), so the IR-level `nr` is 220 (generic), and the predicate should check 220, not 56. The aarch64 backend (the working reference) checks generic 220. R2-b-impl for x86_64 should verify this and correct the predicate if needed. **For ppc64, CD-b-impl uses generic 220/221 — matching aarch64 and riscv64.**

**Mitigation:** §5.2 sketch uses generic 220/221. Add a unit test asserting the predicate matches on a fixture with `IRInstr::Syscall { nr: 220, .. }` (generic clone) — this catches both the predicate and any future drift in `translate_or_warn`.

### 7.5 R0 hazard (ppc64-specific, analogous to riscv64 Zero-register hazard) — **MEDIUM**

`TargetAgnosticRegAlloc::gen_spill_reload` (`regalloc.rs:3145-3174`) uses `PhysicalReg::new(class, 0)` as the scratch register for spill/reload code. On ppc64 GPRs, index 0 = `Gpr::R0`. Unlike riscv64's `x0` (which is hardwired zero — writes discarded, reads return 0), ppc64's R0 IS a real register — but it has special meaning in `MFLR R0` / `MTLR R0` / `LI R0, syscall_nr` / `SC`. If the emitter naively honors the `GenericSpillCode.preg` field (R0) for a spill `STD R0, slot(R1)`, it will silently clobber the R0 value that the next `MFLR R0` or `LI R0, nr` depends on.

**Mitigation:** the emitter must NOT honor the `preg` field of `GenericSpillCode` literally when it's `PhysicalReg::new(Gpr, 0)` — instead, use the vreg's actual location from `alloc.vreg_to_preg[vreg]` for the value being spilled/reloaded, and use a real scratch (R3/R4 for GPRs, F0/F1 for FPRs) for the load/store. Alternatively (and more robustly): mark R0 as non-allocatable in the TargetDesc (§6 gap 3) so the linear-scan allocator never assigns vregs to R0, and the only R0 usage is explicit `MFLR`/`LI`/`SC` in the prologue/syscall/epilogue. The latter is recommended.

This is less severe than riscv64's Zero-register hazard (§7.5 of CC-a-audit) because R0 IS a real register — the symptom is a corrupted R0 value (wrong syscall number, wrong LR restore) rather than a silent no-op. But it's still a correctness hazard. Regression test: a function with a syscall followed by a spill that lands on R0 — verify the syscall still executes with the correct number.

### 7.6 TOC register (R2) preservation — **MEDIUM**

The ELFv2 ABI requires the callee to preserve R2 (TOC pointer) across calls. The current stack-slot ISel does NOT save/restore R2 because it makes no calls that would clobber R2 (all calls go through `BL` which preserves R2 in ELFv2 for local calls, or through `CallIndirect` which sets up R12 → `MTCTR` → `BCTRL` and the callee is responsible for its own R2). However, if the register-based emitter assigns a vreg to R2 (which the TargetDesc prevents — R2 is `toc_pointer()` and non-allocatable), or if the emitter's prologue/epilogue clobbers R2 (which it shouldn't — R2 is never touched by `STDU`/`MFLR`/`STD`/`ADDI`), R2 will be corrupted. **Mitigation:** verify R2 is marked non-allocatable in the TargetDesc (`toc_pointer()` at `:2327` — confirmed: the builder sets `is_allocatable = false`). The emitter must not use R2 as a scratch. Add a verifier assertion that R2 never appears in `vreg_to_preg` or `spill_code`.

### 7.7 Link Register (LR) save/restore — **MEDIUM**

The LR save must happen in the **callee's own frame** at `[SP + fs - 24]`, NOT in the caller's frame at `[SP + fs + 16]`. The stack-slot ISel has a documented bug fix at `:3136-3143` and `:3202-3226` that moved LR from the caller's frame to the callee's frame — the previous code silently corrupted caller vregs on every call. The register-based emitter must inherit this fix verbatim. **Mitigation:** the prologue sketch in §5.1.5 step 2 uses `STD R0, fs-24(R1)` (callee's frame). Add a regression test with a caller that has 3+ vregs (so that a vreg lands at `caller_fs - 16`, the slot that would be corrupted by the old bug) and verify the caller's vregs survive the call.

### 7.8 CR field allocation — **LOW (deferred)**

The TargetDesc registers CR0–CR7 as `cond_reg` (`:2435-2442`) but the `TargetAgnosticRegAlloc` likely excludes `cond_reg` class from the allocatable pool (verify in CD-b-impl). Even if CR fields are allocated, the existing `ss_emit_cmp` helper at `:2666` uses CR0 unconditionally — the register-based emitter would need to consult `alloc.vreg_to_preg` for CR vregs and use the assigned CR field. **Mitigation:** defer CR field allocation to Phase Dc (post-default-on). Phase Da uses CR0 only (matching the stack-slot ISel). If the allocator does allocate CR fields, the verifier (§5.3) should flag them as unsupported and fall back to stack-slot.

### 7.9 Per-function structural invariants interaction — **MEDIUM**

The stack-slot ISel's prologue embeds compile-time-computed capability-grant signatures, formal-verify counter pre-loads, channel sequence counter initialisation, etc. (§1.4). The register-based emitter must either (a) replicate this prologue verbatim, or (b) extract a shared helper. Option (b) is preferred but requires refactoring the stack-slot ISel's prologue builder into a callable function — a non-trivial change because the current prologue is inline in `allocate_registers` and captures many local variables (frame_size, vreg_stack_slots, alloc_offsets, etc.). **Mitigation:** for CD-b-impl, take option (a) — copy the prologue code into `reg_isel.rs`. Refactor to a shared helper in a follow-up PR (CD-e-cleanup). Same recommendation as x86_64 §7.7 / riscv64 §7.7.

### 7.10 SIMD/FP register allocation — **MEDIUM**

`TargetAgnosticRegAlloc` does allocate FPRs (the `caller_saved_fps` / `callee_saved_fps` pools are populated from the TargetDesc), but the existing stack-slot ISel's FP path uses fixed F0/F1 for all operations (see the `BinOp` FP arm at `:3429-3448` which loads lhs into F0 and rhs into F1 via scratch slots). The register-based emitter must consult `alloc.vreg_to_preg` for FP vregs too. The `Fpr` enum at `:202-235` is complete (32 registers), and `Fpr::is_callee_saved()` is correct (F14–F31). **Mitigation:** start CD-b-impl with **integer-only** register allocation (FP vregs fall back to the stack-slot pattern via `dst_spilled`); add FP register allocation in a follow-up. Same approach as x86_64 §7.8 / riscv64 §7.8. **ppc64 has no SIMD/AltiVec encoder** — `IRInstr::VectorOp { .. }` returns `Vec::new()` at `:4747`. VSX/VMX register allocation (VS32–VS63) is out of scope indefinitely.

### 7.11 Big-endian U8-load workaround — **MEDIUM (ppc64 only, not ppc64le)**

The stack-slot ISel has a ppc64-specific workaround at `:3303-3410` (see §2.9): single-U8-load functions with variable-offset addressing get their load width upgraded to the return type, because big-endian ppc64 reads the HIGH byte (zero for small positive values) on a U8 load of a wider-stored value. The register-based emitter must inherit this workaround verbatim — otherwise the same bsearch/quicksort/matrix failures will recur on big-endian ppc64. (ppc64le is little-endian and unaffected — the workaround is a no-op there because the LE U8 load reads the LOW byte, which equals the value.) **Mitigation:** copy the `single_load_upgrade_ty` pre-pass into `reg_isel.rs` and apply it before the per-instruction emit loop. Add a regression test with `bsearch.vuma` on ppc64 (BE) under `VUMA_REAL_REGALLOC_PPC64=1` and verify the exit code matches the stack-slot baseline.

### 7.12 Syscall-position tracking (G6) — **LOW (already fixed)**

Wave-1's G6 fix at `regalloc.rs:954` (tracking `IRInstr::Syscall` as a call position) is in `LiveRangeComputer::compute`, which both `LinearScanAllocator` (aarch64) and `TargetAgnosticRegAlloc` (ppc64) use. ppc64 already benefits. **Mitigation:** none needed — verify with a `try_recv`-equivalent ppc64 test in the curated subset.

### 7.13 `IRInstr::Select` lowering — **MEDIUM**

The CB-a-investigate (`1ccefa6b`) root cause for the aarch64 `try_recv` failure was a CSEL operand swap. ppc64 has no CSEL — the `Select` lowering will be a small branch-based sequence using CR0:

```asm
    # cond in R3, true_val in R4, false_val in R5
    CMPDI CR0, R3, 0          # compare cond with 0
    BC 12, 2, +3               # if EQ (cond == 0), skip to false path (BD=3)
    MR R3, R4                  # true path: select true_val
    B +2                       # skip false path (BD=2)
    MR R3, R5                  # false path: select false_val
    # R3 = result
```

The hazard: the **branch direction** must put the `true_val` move on the path that executes when `cond` is true. This is the same kind of operand-swap bug that aarch64 had — easy to get wrong, and the symptom is silent (wrong branch selected). The existing stack-slot ISel's `Select` arm has the correct ordering (it works on the stack-slot path); the register-based arm must copy that ordering verbatim. **Mitigation:** add a regression test for `try_recv` (or equivalent) on ppc64 with `VUMA_REAL_REGALLOC_PPC64=1` and verify exit code 77 (not 0). If the test exits 0, suspect a Select operand-swap. Same risk as riscv64 §7.10.

### 7.14 ppc64le byte-swap interaction — **LOW**

The ppc64le wrapper byte-swaps every 4-byte instruction word BE→LE after emission (`ppc64le.rs:383-389`). This is transparent to the register-based emitter — the emitter writes BE bytes (via `encode_word` = `word.to_be_bytes()`), and the wrapper flips them. The only hazard: if the emitter inserts spill/reload instructions between a branch and its target, the branch fixup math (which uses byte offsets, not instruction words) must account for the 4-byte alignment of all ppc64 instructions. Since all ppc64 instructions are exactly 4 bytes, byte offsets are always 4-byte aligned, and the fixup math is straightforward. **Mitigation:** none needed — the existing `branch_fixups` mechanism at `:4809-4830` handles this correctly.

---

## 8. Phased Rollout Plan

### Phase Da — ppc64 reg_isel skeleton (integer-only, no FP, no SIMD)

1. Create `src/codegen/src/ppc64/reg_isel.rs` with the public API from §5.1.
2. Implement `preg_to_gpr` / `preg_to_fpr` translation (§5.1.2).
3. Implement `verify_callee_saved_ppc64` (§5.3) — with the §7.5 R0-hazard skip.
4. Implement prologue/epilogue (§5.1.5, §5.1.6) with callee-saved `STD`/`LD` from `alloc.used_callee_saved`.
5. Implement per-IR-instruction arms for: `Add`, `Sub`, `Mul`, `Div`, `BinOp`, `UnaryOp`, `Cmp`, `Cast` (integer kinds), `Load`, `Store`, `Offset`, `GetAddress`, `Alloc`, `Free`, `Branch`, `CondBranch`, `Ret`, `Phi` (no-op), `Select` (with §7.13 regression test), `Call` (direct, integer args), `CallIndirect`, `Syscall` (Linux ppc64 ABI: R0=sysnr, R3-R8=args, R3=return, with positive-errno→negative-errno conversion at `:4685-4686`).
6. Inherit the big-endian U8-load workaround (§7.11) verbatim from the stack-slot ISel.
7. **Defer to Phase Db:** all Channel*/StarkProof builtins, AtomicCas, VectorOp, FP-typed Add/Sub/Mul/Div/Cmp/Cast (these fall back to the stack-slot pattern via `dst_spilled` — correct but slow).
8. Wire up `PPC64Backend::allocate_registers` (§5.2) gated by `VUMA_REAL_REGALLOC_PPC64=1` (default off).
9. Run curated ppc64 test subset under `qemu-ppc64-static` (BE) AND `qemu-ppc64le-static` (LE — via the ppc64le wrapper delegation) with the env var on. Triage failures.

**Estimated effort:** 2.5–3.5 weeks (CD-b-impl). Slightly more than x86_64/riscv64 Phase 2a/Ca due to: (a) the big-endian U8-load workaround inheritance, (b) the LR-save-in-callee-frame fix inheritance, (c) the R0 hazard mitigation, (d) the dual-endian verification (ppc64 BE + ppc64le LE).

### Phase Db — ppc64 reg_isel FP/SIMD

1. Add FP-typed Add/Sub/Mul/Div/Cmp/Cast arms honouring `alloc.vreg_to_preg` for `RegClass::SimdFp` vregs (F0–F31).
2. Add AtomicCas, VectorOp arms (VectorOp will remain a no-op — no AltiVec encoder).
3. Re-run curated subset; expect binary size reduction on FP-heavy tests.

**Estimated effort:** 1–2 weeks (CD-c-opt or CD-d-impl).

### Phase Dc — ppc64 reg_isel IPC/capability builtins + CR field allocation

1. Add `ChannelOpen`, `ChannelSend`, `ChannelRecv`, `ChannelClose`, `ChannelRecvTimeout`, `ChannelRecvResult`, `StarkProof` arms.
2. Each builtin must consult the per-function structural-invariant slots (cap sig, formal-verify counter, channel seq counter) — reuse the stack-slot ISel's emit code for these arms.
3. (Optional) Add CR field allocation: mark CR2/CR3/CR4 as `.callee_saved()` in the TargetDesc (§6 gap 2), add CR save/restore in the prologue (`MFCR R0; STD R0, cr_save_off(R1)` / `LD R0, cr_save_off(R1); MTCR R0`), and consult `alloc.vreg_to_preg` for CR vregs in `ss_emit_cmp`.
4. Re-run curated IPC subset; verify the formal-verify counter still increments correctly.

**Estimated effort:** 1.5 weeks (CD-e-impl).

### Phase Dd — Default-on

1. Run the full 30-test curated matrix (CA-a-test equivalent for ppc64 + ppc64le) under regalloc.
2. Verify ≥ 28/30 pass (DoD threshold from Wave-1 R1-c-test) on BOTH ppc64 (BE) and ppc64le (LE).
3. Flip `VUMA_REAL_REGALLOC_PPC64` default to `1`.
4. Update `docs/caveats.md` to reflect ppc64/ppc64le now emit register-based bytes.

**Estimated effort:** 2–3 days (CD-f-verify + CD-g-default).

### Phase De — R31/R0 allocatability fix and refactor

1. Add `.not_allocatable()` to the R31 line in `ppc64_target_desc()` (§6 gap 1).
2. Add `.not_allocatable()` to the R0 line in `ppc64_target_desc()` (§6 gap 3) — or keep R0 allocatable but exclude it from the reg_isel pool via a filter.
3. Add `.callee_saved()` to CR2/CR3/CR4 in `ppc64_target_desc()` (§6 gap 2) — if Phase Dc CR allocation is pursued.
4. Refactor the stack-slot ISel's prologue builder into a shared `emit_prologue_common()` helper (§5.5 / §7.9).

**Estimated effort:** 1 week (CD-h-cleanup).

---

## 9. Concrete Code Changes

| # | File | Change | LOC (est.) | Phase |
|--:|------|--------|-----------:|:------:|
| 1 | `src/codegen/src/ppc64/reg_isel.rs` (NEW) | New module: `allocate_registers`, `preg_to_gpr`, `preg_to_fpr`, `verify_callee_saved_ppc64`, per-IR-instruction arms, prologue/epilogue builders, big-endian U8-load workaround inheritance. | ~2200–2800 | Da |
| 2 | `src/codegen/src/ppc64/mod.rs:3084-4869` | Rename current `allocate_registers` body to `allocate_registers_stack_slot` (private method); rewrite `allocate_registers` per §5.2 sketch (env-var gate, fork opt-out with generic syscall nr=220/221, reg_isel dispatch, stack-slot fallback). | ~70 | Da |
| 3 | `src/codegen/src/ppc64/mod.rs` (module decl) | Add `mod reg_isel;` (or `pub mod reg_isel;`) to the module's child-module declarations. | 1 | Da |
| 4 | `src/codegen/src/target_desc.rs:2361` | Add `.not_allocatable()` to the R31 line: `RegDesc::gpr("R31", 31).frame_pointer().callee_saved().not_allocatable()`. | 1 | Da (recommended) or De |
| 5 | `src/codegen/src/target_desc.rs:2323` | Add `.not_allocatable()` to the R0 line: `RegDesc::gpr("R0", 0).not_allocatable()` (§6 gap 3 / §7.5 R0 hazard). | 1 | Da (recommended) or De |
| 6 | `src/codegen/src/target_desc.rs:2437-2439` | Add `.callee_saved()` to CR2, CR3, CR4 lines (§6 gap 2). | 3 | Dc (or defer) |
| 7 | `src/codegen/src/regalloc.rs` (near `GenericSpillCode` enum, ~:3355-area) | Add a `phys_reg(&self) -> crate::backend::PhysicalReg` accessor on `GenericSpillCode` for use by `verify_callee_saved_ppc64` (same accessor needed by x86_64 and riscv64 verifiers — coordinate with R2-b-impl / CC-b-impl). | ~10 | Da |
| 8 | `tests/` (NEW test file) | Add unit tests for `verify_callee_saved_ppc64` (positive + negative cases, including R0/R1/R2/R13 reserved-reg checks). | ~60 | Da |
| 9 | `tests/` (NEW integration test) | Add a `try_recv`-equivalent ppc64 test that exercises the Syscall-position tracking on ppc64 (G6 + §7.13 Select operand regression guard). Run on BOTH ppc64 (BE) and ppc64le (LE). | ~40 | Da |
| 10 | `tests/` (NEW integration test) | Add a "force spill" test: a function with more live vregs than available caller-saved GPRs (11 on ppc64: R0, R3–R12; or 10 if R0 is excluded per §7.5), compiled with `VUMA_REAL_REGALLOC_PPC64=1`, asserting correct exit code (R0-hazard §7.5 regression guard). | ~35 | Da |
| 11 | `tests/` (NEW integration test) | Add a "big-endian U8-load workaround" regression test: `bsearch.vuma` (or equivalent) on ppc64 (BE) with `VUMA_REAL_REGALLOC_PPC64=1`, asserting exit code matches stack-slot baseline (§7.11). | ~30 | Da |
| 12 | `tests/` (NEW integration test) | Add a "caller-vreg survival" test: a caller with 3+ vregs (so one lands at `caller_fs - 16`) calling a callee, verifying the caller's vregs survive the call (§7.7 LR-save regression guard). | ~30 | Da |
| 13 | `docs/caveats.md` | Document the new `VUMA_REAL_REGALLOC_PPC64` env var and the fork opt-out (generic clone=220/execve=221; ppc64-native 120/11 via translate_or_warn). Note ppc64le inherits. | ~25 | Da |

**Total LOC for Phase Da:** ~2500–3100 (dominated by the new `reg_isel.rs`).

---

## 10. Effort Estimate

**F2-a estimate:** 2–4 weeks (3–4 weeks in §6 Phase 2, p. 413).

**This audit's revised estimate:**

| Phase | Effort (developer-weeks) | Notes |
|-------|--------------------------|-------|
| Da — integer-only skeleton + wire-up + fork opt-out + verifier + R0-hazard mitigation + BE U8-load workaround inheritance + dual-endian verification | 2.5–3.5 | Bulk of the work: ~30 IR instruction arms in `reg_isel.rs`, each adapting the existing stack-slot arm to honour `vreg_to_preg`. The aarch64 wire-up at `backend.rs:3175-3300` is the template. ppc64's three-operand form (§2.6) is a **simplification vs x86_64**: no two-operand `mov dst, lhs` insertions, no `eliminated_copies` field needed (same as riscv64). The R0 hazard (§7.5) is a **complication vs x86_64/aarch64** but less severe than riscv64's Zero-register hazard (R0 is a real register, just reserved). The big-endian U8-load workaround (§7.11) and the LR-save-in-callee-frame fix (§7.7) are ppc64-specific inheritance burdens. The dual-endian verification (ppc64 BE + ppc64le LE) adds ~20% testing effort vs x86_64/riscv64. Net: ~15-25% more effort than x86_64 Phase 2a. |
| Db — FP/SIMD arms | 1–2 | Adds ~10 arms, mostly FPR encoder calls (the `Instruction` enum already has `Fadd`/`Fsub`/`Fmul`/`Fdiv`/`Fmov`/`Fcmpu`/`Lfd`/`Stfd` at `ppc64/mod.rs` Instruction enum). No AltiVec/VSX (out of scope). |
| Dc — IPC/capability builtin arms + CR field allocation | 1.5 | Adds ~7 builtin arms, mostly verbatim copies of stack-slot arms. CR field allocation is optional (can defer). |
| Dd — default-on + verification | 0.5 | Run curated matrix on BOTH ppc64 + ppc64le, flip default, update docs. |
| De — cleanup (R31/R0 fix, CR markers, refactor) | 1 | Optional; can ship without. |
| **Total (Phases Da–Dd, required for default-on)** | **5.5–7.5** | |
| **Total (Phases Da–De, with cleanup)** | **6.5–8.5** | |

**Achievable in this orchestration run? N.**

The orchestration run is operating under a 10-minute-per-task budget for sub-agents (per CD-a-audit's own constraint). The ppc64 register-based emitter is genuinely 5.5–7.5 developer-weeks of work — it is **~15-25% larger than x86_64 Phase 2a** (R2-a-audit estimated 4.5–6.5 weeks for x86_64) due to the ppc64-specific inheritance burdens (big-endian U8-load workaround, LR-save-in-callee-frame fix, R0 hazard, dual-endian verification). The CD-b-impl sub-agent should be tasked with **Phase Da only** (integer-only skeleton + wire-up + verifier + env-var gate, default off), which is itself 2.5–3.5 weeks and will require multiple sub-agent invocations to complete iteratively (mirror the Wave-1 R1-a→R1-b→R1-b2→R1-b3→R1-c→R1-f cadence). Default-on (Phase Dd) is a separate task after Phase Da's curated-subset verification passes on BOTH ppc64 and ppc64le.

Per §0.7-6 of the orchestration protocol, this honest estimate should cause the orchestrator to defer the bulk of the work to a human developer OR sequence it across many orchestration waves. The CD-a-audit deliverable (this document) is itself the actionable artefact: CD-b-impl can proceed incrementally off it.

**Key risks that could inflate the estimate:**

- The big-endian U8-load workaround (§7.11) is subtle and has many edge cases; replicating it in `reg_isel.rs` without refactoring the stack-slot ISel first will create maintenance hazard (§7.9). If the workaround is missed, bsearch/quicksort/matrix will silently fail on ppc64 (BE) but pass on ppc64le (LE) — a confusing symptom.
- The LR-save-in-callee-frame fix (§7.7) must be inherited verbatim; missing it will silently corrupt caller vregs on every call (the original bug took significant debugging to root-cause).
- The R0 hazard (§7.5) is ppc64-specific and requires careful emitter-side handling. If the emitter naively honors the `GenericSpillCode.preg` field, syscall numbers and LR restores will be corrupted.
- The R31 allocatability bug (§6 gap 1) must be fixed in Phase Da, not deferred — otherwise the linear-scan allocator will assign vregs to R31 and the emitter's frame-base convention will conflict.
- The `IRInstr::Select` lowering (§7.13) carries the same operand-swap hazard as aarch64's CSEL bug (CB-a-investigate `1ccefa6b`). A regression test is mandatory.
- The dual-endian verification (ppc64 BE + ppc64le LE) doubles the test matrix — a test that passes on ppc64le may fail on ppc64 (BE) due to the U8-load workaround, and vice versa.

**Key simplifications vs x86_64 (could deflate the estimate):**

- ppc64 is three-operand (§2.6) — no two-operand coalescing problem, no `eliminated_copies` field needed (same as riscv64).
- ppc64 uses fixed 32-bit instructions (§2.7) — no REX prefix, no variable-length fall-through risk.
- ppc64 uses the same generic Linux syscall numbers as aarch64/riscv64 (§7.4: generic 220/221) — the `contains_fork` predicate is a near-verbatim copy of aarch64's (despite the protocol's mention of "120/189" — those are native numbers, not IR-level).
- ppc64 has no condition codes in the GPR sense (§2.3) — CR fields are separate; comparisons write to CR0 and branches test CR bits, but this is already handled by the existing `ss_emit_cmp` helper at `:2666`.
- ppc64le inherits automatically (§6.1) — no separate ppc64le implementation needed.

**Net:** the ppc64 effort is ~15-25% larger than x86_64's (the simplifications are offset by the ppc64-specific inheritance burdens and dual-endian verification). The R0 hazard is less severe than riscv64's Zero-register hazard (R0 is a real register, not hardwired zero).

---

## DoD Check

- [x] Design doc exists at `scripts/audit/completion_wave_d_ppc64_design.md`.
- [x] All 10 required sections present: §1 Current Path, §2 Register File, §3 What emit_function_regalloc Needs, §4 Reusable Components, §5 New Components, §6 TargetDesc Readiness, §7 Risk Assessment, §8 Phased Rollout, §9 Concrete Code Changes, §10 Effort Estimate.
- [x] Concrete line numbers cited for every code path: §1 `ppc64/mod.rs:3084`, `:3011`, `:4864`, `:3177`, `:3257`, `:3128`; §2 `target_desc.rs:2320-2508`, `ppc64/mod.rs:54-87`, `:202-235`, `:369-378`, `:98`, `:407`; §4 `emit.rs:1056-1354`, `regalloc.rs:4860`, `backend.rs:3226`, `regalloc.rs:3145`; §5 `ppc64/mod.rs:3084-4869`, `target_desc.rs:2361`, `regalloc.rs:3355`, `ppc64le.rs:400-406`; §6 `target_desc.rs:2361`, `:2323`, `:2437-2439`; §7 multiple.
- [x] Honest effort estimate: 5.5–7.5 developer-weeks total; Phase Da alone is 2.5–3.5 weeks; **NOT achievable in a single 10-minute orchestration sub-agent run** — recommendation is to sequence CD-b-impl across multiple waves or defer to human developer per §0.7-6.
- [x] No source files edited (READ-ONLY audit — `git status --short` shows only the new markdown added).
- [x] No `git push`.
- [x] No sub-agents spawned.
