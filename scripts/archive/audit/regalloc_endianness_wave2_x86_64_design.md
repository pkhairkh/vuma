# Wave 2 — x86_64 Register-Based Emitter Design

**Task ID:** R2-a-audit
**Wave:** 2
**Prior-run context:** F2-a-audit (`7083e1c7`, `scripts/audit/followup_wave2_emit_regalloc_design.md` §4) classified x86_64 as **LOW** readiness for `emit_function_regalloc` wire-up — it needs a *new* register-based emitter (the existing `Emitter::emit_function_regalloc` at `emit.rs:1056` is aarch64-only). F2-a estimated 2–4 weeks for a human developer.
**Scope of this document:** produce the design R2-b-impl will follow.
**Files audited (READ-ONLY):**
- `src/codegen/src/x86_64/mod.rs` (5692 LOC) — `Gpr`/`Xmm` enums, encoder helpers, `X86_64Backend::allocate_registers` (`:4141`), `X86_64Backend::emit_function_regalloc` (`:3962`), `try_real_regalloc` (`:4066`).
- `src/codegen/src/x86_64/stack_slot_isel.rs` (4512 LOC) — baseline stack-slot ISel.
- `src/codegen/src/emit.rs` (9516 LOC) — `Emitter::emit_function` (`:959`), `emit_function_regalloc` (`:1056`, aarch64-only), `emit_function_stack_slot` (`:3496`).
- `src/codegen/src/regalloc.rs` (6594 LOC) — `LinearScanAllocator` (`:1214`, aarch64-hardcoded), `TargetAgnosticRegAlloc` (`:2742`), `AllocationResult` (`:480`, aarch64), `RegAllocResult` (`:3224`, target-agnostic), `verify_callee_saved` (`:4860`, aarch64-only).
- `src/codegen/src/target_desc.rs` (3018 LOC) — `x86_64_target_desc()` (`:1932`).
- `src/codegen/src/regalloc_emit.rs` (267 LOC) — `annotate_with_regalloc`, `run_regalloc`.
- `src/codegen/src/backend.rs:3162` — `AArch64Backend::allocate_registers` (the Wave-1 wire-up reference).
- `scripts/audit/followup_wave2_emit_regalloc_design.md` — F2-a design doc.

---

## 1. Current x86_64 Emission Path (stack-slot ISel)

**Entry point:** `X86_64Backend::allocate_registers` at `x86_64/mod.rs:4141`. The body is 14 lines:

```rust
fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
    // Step 1: baseline stack-slot ISel — always produces correct bytes.
    let mut allocated = stack_slot_isel::allocate_registers(func)?;

    // Step 2: run the real target-agnostic linear-scan allocator and,
    // on success, annotate the AllocatedFunction with its decisions.
    if let Some(alloc) = try_real_regalloc(func) {
        crate::regalloc_emit::annotate_with_regalloc(&mut allocated, &alloc);
    }

    Ok(allocated)
}
```

**Observations:**

1. **Bytes are stack-slot only.** Every vreg gets an 8-byte stack slot at `[rbp - offset]`. Each instruction loads its operands into a fixed set of scratch registers (`RAX`, `RCX`, `RDX`, `R10`, `R11`), performs the op, and stores the result back. See `stack_slot_isel.rs:1277-1287` (`load_vreg` / `store_vreg` closures) and the `Add` arm at `:1594-1675` (typical pattern: `load_value(lhs, Rax); load_value(rhs, Rcx); encode_add_reg_reg(Rax, Rcx); store_vreg(dst_id, Rax)`).

2. **`try_real_regalloc` already works.** At `mod.rs:4066-4095`, the x86_64 backend constructs a `TargetAgnosticRegAlloc::new(target)` (target = x86_64 TargetDesc) and calls `allocate_function(func)` to obtain a `RegAllocResult`. It returns `Some` on success, `None` on failure (with a `vuma_log!(debug, ...)` message). The result is currently used **only for metadata annotation** of the stack-slot bytes — the encoded bytes do not honour `vreg_to_preg` / `spill_code` / `used_callee_saved`.

3. **`X86_64Backend::emit_function_regalloc` is metadata-only.** At `mod.rs:3962-3975`, this method runs the stack-slot ISel first, then annotates with `RegAllocResult` via `regalloc_emit::annotate_with_regalloc`. This is the per-backend overload (F2-a §1.1.B); it does **not** change bytes. The F2-a design doc §7.5 recommends renaming it `emit_function_with_regalloc_metadata` and reserving the `emit_function_regalloc` name for the byte-changing path that R2-b-impl will introduce.

4. **Per-function structural invariants are embedded in the prologue.** The stack-slot ISel's prologue (`stack_slot_isel.rs:1385-1470+`) does much more than `push rbp; mov rbp, rsp; sub rsp, frame_size`: it computes a 32-byte capability-grant signature at compile time and emits 4 × `mov rax, imm64; mov [rbp+cap_sig_off], rax` stores; it pre-loads the formal-verify folded-check counter; it zeroes the channel sequence counter, protocol-state slot, and circuit-breaker state slots. The register-based emitter must either (a) replicate this prologue verbatim, or (b) extract it into a shared `emit_prologue_common()` helper. Option (b) is preferred — see §5.

5. **No `contains_fork` opt-out exists on x86_64 today.** The Wave-1 `contains_fork` fork-detection helper was added to `AArch64Backend::allocate_registers` (`backend.rs:3226-3238`) but **not** to `X86_64Backend::allocate_registers`. Because x86_64's byte path is currently stack-slot (no callee-saved prologue, no register-state preservation across the clone syscall), this has been harmless. Once x86_64 gains a register-based emitter, the same fork hazard surfaces and x86_64 needs its own `contains_fork` — but with **different syscall numbers** (Linux/x86_64: `clone=56`, `vfork=58`; Linux/aarch64: `clone=220`, `vfork=221`).

6. **x86_64 does NOT dispatch through `Emitter::emit_function`.** The x86_64 backend calls `stack_slot_isel::allocate_registers(func)` directly — it never touches `emit.rs`'s `Emitter`. This is unlike aarch64, which goes through `Emitter::emit_function(func, None)` → `emit_function_stack_slot`. The x86_64 register-based emitter will similarly live in the `x86_64/` module tree (proposed: `x86_64/reg_isel.rs`), not in `emit.rs`.

---

## 2. x86_64 Register File (System V AMD64 ABI)

Source: `target_desc.rs:1932-2040` (`x86_64_target_desc()`) and `x86_64/mod.rs:51-150` (`Gpr`, `Xmm` enums).

### 2.1 GPR file (16 registers)

| Index | Reg | ABI Role                       | Callee-saved? | Allocatable? | Notes |
|------:|-----|--------------------------------|---------------|--------------|-------|
| 0     | RAX | Integer return value           | ❌ caller     | ✅            | Scratch / div quotient |
| 1     | RCX | Integer arg 4                  | ❌ caller     | ✅            | Shift count reg |
| 2     | RDX | Integer arg 3                  | ❌ caller     | ✅            | Div remainder (RDX:RAX pair) |
| 3     | RBX | (callee-saved)                 | ✅ callee     | ✅            | |
| 4     | RSP | Stack pointer                  | (special)     | ❌            | Reserved |
| 5     | RBP | Frame pointer                  | ✅ callee     | (FP)          | Used as spill-area base in stack-slot ISel |
| 6     | RSI | Integer arg 2                  | ❌ caller     | ✅            | |
| 7     | RDI | Integer arg 1                  | ❌ caller     | ✅            | |
| 8     | R8  | Integer arg 5                  | ❌ caller     | ✅            | |
| 9     | R9  | Integer arg 6                  | ❌ caller     | ✅            | |
| 10    | R10 | Scratch / syscall nr (Linux)   | ❌ caller     | ✅            | |
| 11    | R11 | Scratch                        | ❌ caller     | ✅            | |
| 12    | R12 | (callee-saved)                 | ✅ callee     | ✅            | |
| 13    | R13 | (callee-saved)                 | ✅ callee     | ✅            | |
| 14    | R14 | (callee-saved)                 | ✅ callee     | ✅            | |
| 15    | R15 | (callee-saved)                 | ✅ callee     | ✅            | |

**Caller-saved GPRs available for allocation (8):** RAX, RCX, RDX, RSI, RDI, R8, R9, R10, R11. (R11 — 9 caller-saved; some sources count 10 including RAX, this matches `Gpr::is_callee_saved()` at `mod.rs:82-87` which excludes RAX/RCX/RDX/RSI/RDI/R8-R11/RSP from callee-saved.)

**Callee-saved GPRs available for allocation (5):** RBX, R12, R13, R14, R15. (RBP is also callee-saved but is used as the frame pointer.)

### 2.2 SIMD/FP register file (16 XMM registers)

| Index | Reg    | ABI Role                  | Callee-saved? |
|------:|--------|---------------------------|---------------|
| 0–7   | XMM0–7 | FP args 0–7 / FP return   | ❌ caller     |
| 8–15  | XMM8–15| Scratch                   | ❌ caller     |

**All 16 XMM registers are caller-saved** under System V AMD64 (this is a notable difference from aarch64 where V8–V15 are callee-saved). The `x86_64_target_desc()` correctly marks `callee_saved_fps: vec![]` at `target_desc.rs:1989`.

### 2.3 Calling convention summary

| Aspect                     | Value                                  |
|----------------------------|----------------------------------------|
| Integer arg registers      | RDI, RSI, RDX, RCX, R8, R9 (in order)  |
| FP arg registers           | XMM0–XMM7                              |
| Integer return             | RAX                                    |
| FP return                  | XMM0, XMM1                             |
| Stack alignment at `call`  | 16 bytes                               |
| Callee-saved GPRs          | RBX, RBP, R12–R15                      |
| Callee-saved XMMs          | (none)                                 |
| Link register              | (none — return address on stack)       |
| Branch delay slots         | (none)                                 |
| TOC pointer                | (none)                                 |
| Stack frame leaf           | `push rbp; mov rbp, rsp; sub rsp, N`   |
| Stack frame epilogue       | `mov rsp, rbp; pop rbp; ret`           |
| Syscall nr register        | RAX (Linux x86_64)                     |
| Syscall arg registers      | RDI, RSI, RDX, R10, R8, R9 (note R10 instead of RCX) |
| Syscall return             | RAX                                    |

### 2.4 Two-operand ISA constraint

x86_64 arithmetic is **two-operand** (`add dst, src` computes `dst = dst + src`, clobbering the old dst). This is a fundamental difference from aarch64's three-operand `add dst, src1, src2`. The register-based emitter must insert `mov dst, lhs; op dst, rhs` whenever `dst != lhs`, or rely on the linear-scan allocator to coalesce `dst` and `lhs` into the same physical register (the `eliminated_copies` mechanism in `AllocationResult` does this, but `RegAllocResult` — the target-agnostic type x86_64 consumes — does **not** have `eliminated_copies`; see §4).

### 2.5 REX prefix handling

Registers R8–R15 require a REX prefix (REX.B for the dst field, REX.R for the src field, REX.X for the index field, REX.W for 64-bit operand size). The existing `Gpr::needs_rex()` helper at `mod.rs:77-79` returns `*self as u8 >= 8`, and every `encode_*` helper already handles REX emission internally (see `encode_add_reg_reg` at `mod.rs:445`, `encode_mov_reg_reg` at `:316`, etc.). **No new REX code is needed in the register-based emitter** — it just calls the existing `encode_*` functions with whatever `Gpr` the allocator assigned.

The one subtle REX hazard is **SPL/BPL/SIL/DIL**: low-byte access to RSP/RBP/RSI/RDI requires REX (even though the registers themselves don't need REX for normal operations). The existing `encode_mov_mem8_reg8` at `mod.rs:1131` handles this; the register-based emitter inherits the handling.

---

## 3. What `emit_function_regalloc` Needs to Do for x86_64

The Wave-1 aarch64 wire-up added 6 things to `AArch64Backend::allocate_registers` (see `backend.rs:3175-3300`). The x86_64 wire-up needs the **same 6 things**, but every concrete encoder call is ISA-specific:

1. **Env-var gate.** Read `VUMA_REAL_REGALLOC_X86_64` (default off). When unset, run today's stack-slot path. When set, attempt the register-based path.

2. **Fork opt-out.** Detect functions containing `IRInstr::Call { func: "spawn_worker"|"fork" }` OR `IRInstr::Syscall { nr: 56|58 }` (Linux/x86_64 clone/vfork — **different from aarch64's 220/221**). For these, fall back to stack-slot — the register-based prologue's callee-saved `push rbx; push r12; ...; pop ...; pop rbx` doesn't interact correctly with `clone()` because the child process runs with a different register state. (Same hazard as Wave-1 R1-b2-fix on aarch64.)

3. **Run the allocator.** Call `try_real_regalloc(func)` (already exists at `mod.rs:4066`). On `Some(alloc)`, proceed; on `None`, fall back to stack-slot.

4. **(Optional) Callee-saved verifier.** If `VUMA_VERIFY_CALLEE_SAVED=1`, run a verifier analogous to `regalloc::verify_callee_saved` (`regalloc.rs:4860`) — but parameterized for x86_64 (caller-saved = RAX, RCX, RDX, RSI, RDI, R8–R11; callee-saved = RBX, R12–R15, RBP; always-allowed = RSP). The existing `verify_callee_saved` is hard-coded to aarch64's `PhysReg::Gpr(Register)` with `r.encoding()` checked against X0–X18 / X19–X28 / X29 / X30 / X31 — **it cannot be called on a `RegAllocResult`** (different `PhysicalReg` type: `crate::backend::PhysicalReg { class, index }` vs aarch64's `regalloc::PhysReg::Gpr(Register)`). See §5.3 for the new x86_64 verifier.

5. **Emit register-based bytes.** Call the new `x86_64::reg_isel::allocate_registers(func, &alloc)` (proposed module, see §5). This produces an `AllocatedFunction` whose `encoded` bytes honour `alloc.vreg_to_preg` (operands stay in registers across instructions where possible), `alloc.spill_code` (boundary spills/reloads), and `alloc.used_callee_saved` (prologue `push rbx; push r12; ...` / epilogue `pop ...; pop rbx`).

6. **Fall back on allocator failure.** If `try_real_regalloc` returns `None`, or if the new `reg_isel::allocate_registers` returns `Err`, run today's `stack_slot_isel::allocate_registers(func)`. This preserves the existing safety guarantee.

**Out of scope for R2-b-impl (deferred to a separate PR):**
- The `EmitResult` API change proposed in F2-a §7.2 (returning `frame_size` + `callee_saved` from the emitter). This is a non-breaking addition but touches all callers; not needed for correctness, only for debug/unwind info accuracy.
- The `emit_function_regalloc` rename proposed in F2-a §7.5. Cosmetic; can be done in a separate cleanup PR.

---

## 4. Reusable Components From aarch64's `emit_function_regalloc`

The aarch64 implementation at `emit.rs:1056-1354` is **aarch64-only** at the byte level — it uses `Register::X0..X30`, `Instruction::SUB/STP/ADD` (aarch64 enums), `compute_frame_size`, `emit_callee_saved_saves`, `emit_spill_reload`, `emit_terminator_regalloc`, `emit_ir_instr` (the aarch64 greedy emitter). None of these concrete calls port to x86_64.

What IS reusable is the **structural pattern**:

| aarch64 component (location)                                 | x86_64 analogue (proposed)                                 | Reuse level |
|--------------------------------------------------------------|------------------------------------------------------------|-------------|
| Allocator result consumed (`AllocationResult`, aarch64)      | `RegAllocResult` (target-agnostic, already produced)       | ✅ Direct — `RegAllocResult` already has `vreg_to_preg`, `spill_slots`, `total_spill_slots`, `used_callee_saved`, `spill_code`, `coalesced_map`. The fields map 1:1. |
| Position-based spill insertion (`pos += 2` per instr, `spill_code.get(&pos)` for pre-instr, `&(pos+1)` for post-instr) | Same `pos += 2` convention | ✅ Direct — `LiveRangeComputer::compute` (regalloc.rs:863) is shared by both allocators, so positions match. |
| Callee-saved prologue sequence (`emit_callee_saved_saves`)   | `push rbx; push r12; push r13; push r14; push r15` (in *decreasing* index order for symmetry with epilogue) | 🔄 Pattern — same idea, different bytes. |
| Callee-saved epilogue (`emit_terminator_regalloc`)           | `pop r15; pop r14; pop r13; pop r12; pop rbx; mov rsp, rbp; pop rbp; ret` (reverse order) | 🔄 Pattern |
| Copy-elision skip (`is_eliminated_copy`, `emit.rs:1256`)     | Skip `IRInstr::Cast { kind: BitCast }` whose src & dst resolve to same `PhysicalReg` | 🚧 Adapted — `RegAllocResult` has no `eliminated_copies` field (only `AllocationResult` does). Either (a) skip only when src/dst resolve to same preg via `get_phys_reg`, or (b) add `eliminated_copies` to `RegAllocResult`. Option (a) is simpler. |
| Param-vreg preassignment (X0–X7, `emit.rs:1086-1102`)        | RDI/RSI/RDX/RCX/R8/R9 (6 integer arg regs) — must NOT be overridden by `alloc.vreg_to_preg` | 🔄 Pattern — same hazard documented in `emit.rs:1112-1141` (R1-b-impl fix). Reuse the param-vreg skip-set logic verbatim, swapping in the x86_64 arg register set. |
| Spill-slot frame layout (`spill_area_aligned + callee_saved_size`, `emit.rs:1155-1180`) | Same two-region layout: `[spill area] ← RBP-relative; [callee-saved save area]` | 🔄 Pattern — but x86_64's callee-saved save area lives *above* RBP (via `push` instructions in the prologue), unlike aarch64's STP-into-SP-decremented-area which lives *below* SP. See §5.4 for the x86_64 frame layout. |
| Verifier hook (`verify_callee_saved`, regalloc.rs:4860)      | New `verify_callee_saved_x86_64` (see §5.3)                | 🚧 New — aarch64's is hard-coded to aarch64's `PhysReg` enum and X0–X18/X19–X28 encoding ranges. |
| Fork opt-out (`contains_fork`, backend.rs:3226)              | Same predicate, but with x86_64 syscall numbers (56/58, not 220/221) | 🚧 New — straightforward port. |
| Syscall-position tracking (`regalloc.rs:954`)                | Already shared via `LiveRangeComputer` (G6 fix)            | ✅ Already in place — applies to `TargetAgnosticRegAlloc` too. |

**The single biggest non-reuse item:** the aarch64 regalloc emitter delegates per-instruction byte emission to `emit_ir_instr` (the greedy emitter), which already supports a `reg_alloc.resolve_reg` mechanism. The x86_64 stack-slot ISel has **no equivalent** — every arm hard-codes `load_vreg(id, Rax)` / `store_vreg(id, Rax)`. This means the x86_64 reg_isel must either (a) introduce a `resolve_vreg(id) -> RegOrSlot` abstraction and rewrite every arm, or (b) special-case "dst in register" vs "dst spilled" per arm. See §5.

---

## 5. New Components Needed (x86_64-specific)

### 5.1 `src/codegen/src/x86_64/reg_isel.rs` (new module)

**Public API:**

```rust
/// Register-based x86_64 emitter.  Consumes a `RegAllocResult` and
/// produces an `AllocatedFunction` whose `encoded` bytes honour the
/// allocator's register assignments, spill code, and callee-saved set.
///
/// Returns `Err` if any IR instruction is not yet supported by the
/// register-based path; the caller falls back to `stack_slot_isel`.
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
       Xmm(Xmm),                     // vreg is in this physical XMM
       Spill { offset: i32 },        // vreg is spilled to [rbp + offset]
       Immediate(i64),               // operand is a constant (for IRValue::Immediate)
   }
   ```
   Look up the vreg in `alloc.vreg_to_preg`; if absent, look up in `alloc.spill_slots`; if absent, the vreg is undefined (panic in debug, fall back to scratch in release).

2. **`PhysicalReg` → `Gpr`/`Xmm` translation.** The x86_64 TargetDesc uses `RegDesc.index` 0..15 for GPRs in the order RAX, RCX, RDX, RBX, RSP, RBP, RSI, RDI, R8..R15 (`target_desc.rs:1935-1961`). The `Gpr` enum has the **same** discriminant values (`mod.rs:51-68`). Translation:
   ```rust
   fn preg_to_gpr(p: crate::backend::PhysicalReg) -> Option<Gpr> {
       if p.class != crate::backend::RegClass::Gpr { return None; }
       match p.index {
           0  => Some(Gpr::Rax),  1  => Some(Gpr::Rcx),  2  => Some(Gpr::Rdx),
           3  => Some(Gpr::Rbx),  /* 4=RSP excluded (non-allocatable) */
           5  => Some(Gpr::Rbp),  6  => Some(Gpr::Rsi),  7  => Some(Gpr::Rdi),
           8  => Some(Gpr::R8),   9  => Some(Gpr::R9),   10 => Some(Gpr::R10),
           11 => Some(Gpr::R11),  12 => Some(Gpr::R12),  13 => Some(Gpr::R13),
           14 => Some(Gpr::R14),  15 => Some(Gpr::R15),
           _  => None,
       }
   }
   ```
   Similarly `preg_to_xmm` for `RegClass::SimdFp` indices 0..15 → `Xmm::Xmm0..Xmm15`.

3. **Per-IR-instruction arms.** Approximately 30 distinct arms (see §1 count). For each, decide:
   - If dst is in a register and both operands are in registers (or immediates): emit the register-form op directly (e.g. `encode_add_reg_reg(dst_gpr, src_gpr)`).
   - If dst is in a register but lhs is spilled: `mov dst_gpr, [rbp+lhs_off]; op dst_gpr, <rhs>`.
   - If dst is spilled: emit the stack-slot pattern (load into RAX, op, store to dst slot) — this is **identical to today's stack_slot_isel arm**, so the existing code can be lifted verbatim into a `dst_spilled` helper.
   - **Two-operand constraint:** if `dst != lhs` and the op is non-commutative (Sub, Div, SAr), insert `mov dst, lhs` first. For commutative ops (Add, Mul, And, Or, Xor), the emitter can swap operand order to avoid the move.

4. **Spill/reload insertion.** At each instruction boundary (`pos` and `pos+1`), walk `alloc.spill_code.get(&pos)` / `&(pos+1)` and emit:
   - `Reload`: `mov <preg>, [rbp + slot.offset]` (GPR) or `movsd <xmm>, [rbp + slot.offset]` (XMM).
   - `Spill`: `mov [rbp + slot.offset], <preg>` (GPR) or `movsd [rbp + slot.offset], <xmm>` (XMM).
   - The `GenericSpillCode` enum (target-agnostic) has the same shape as `SpillCode` (aarch64): `{ preg: PhysicalReg, slot: GenericSpillSlot, ... }`. Translation: preg → `Gpr`/`Xmm` via §5.1.2; slot.offset is already an `i32` displacement from RBP.

5. **Prologue.** Order:
   1. `push rbp; mov rbp, rsp` (frame setup — same as stack-slot).
   2. `sub rsp, frame_size` (allocate spill area + alignment padding).
   3. **Callee-saved saves:** `push rbx; push r12; push r13; push r14; push r15` — only the subset in `alloc.used_callee_saved`. The saves go ABOVE RBP (in the caller's frame), unlike aarch64 where they go BELOW SP. **Important:** the order of pushes determines the offsets at which the saved values live, and these offsets must be tracked so the epilogue can pop them in reverse order.
   4. **Per-function structural invariants** (cap sig, formal-verify counter, channel seq counter, proto state, circuit breaker state — see §1.4): reuse the stack-slot ISel's prologue code verbatim, either by extracting it into `emit_prologue_common()` or by calling the stack-slot ISel's prologue helper directly. **Critical:** the spill-area offsets used by these invariants must NOT collide with `alloc.spill_slots` offsets — see §7.4 for the frame-layout drift risk.

6. **Epilogue.** For each `IRInstr::Ret`:
   1. Move return value into RAX (if not already there).
   2. **Callee-saved restores:** `pop r15; pop r14; pop r13; pop r12; pop rbx` (reverse order of prologue pushes).
   3. `mov rsp, rbp; pop rbp; ret`.

7. **Argument-register preassignment.** For the first 6 integer params, force the param vreg to live in RDI/RSI/RDX/RCX/R8/R9 (in that order) regardless of what `alloc.vreg_to_preg` says. The allocator doesn't know about ABI arg registers (this is the R1-b-impl fix documented at `emit.rs:1112-1141`). Implementation: build a `param_vregs: HashSet<u32>` and skip `vreg_to_preg` lookups for those vregs during the param-loading prologue sequence (which `mov`-copies them from the arg reg into their assigned reg if the allocator picked a different one, or emits nothing if the allocator already picked the arg reg).

### 5.2 Wire-up in `X86_64Backend::allocate_registers`

**File:** `src/codegen/src/x86_64/mod.rs:4141-4154`.

**Sketch** (mirrors aarch64's `backend.rs:3175-3300`):

```rust
fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
    let real_regalloc = std::env::var("VUMA_REAL_REGALLOC_X86_64")
        .map(|v| v == "1")
        .unwrap_or(false);
    let verify_callee_saved = std::env::var("VUMA_VERIFY_CALLEE_SAVED")
        .map(|v| v == "1")
        .unwrap_or(false);

    // R2-a-audit: fork opt-out (clone=56, vfork=58 on Linux/x86_64 — DIFFERENT
    // from aarch64's 220/221).
    let contains_fork = func.blocks.iter().any(|block| {
        block.instructions.iter().any(|inst| {
            match inst {
                crate::ir::IRInstr::Call { func: fname, .. } => {
                    fname == "spawn_worker" || fname == "fork"
                }
                crate::ir::IRInstr::Syscall { nr, .. } => *nr == 56 || *nr == 58,
                _ => false,
            }
        })
    });

    if real_regalloc && !contains_fork {
        if let Some(alloc) = try_real_regalloc(func) {
            if verify_callee_saved {
                if let Err(msg) = super::reg_isel::verify_callee_saved_x86_64(&alloc) {
                    panic!("verify_callee_saved_x86_64 FAILED for '{}': {}", func.name, msg);
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
                        "x86_64 reg_isel failed for '{}': {}, falling back to stack-slot ISel",
                        func.name, e);
                    // fall through
                }
            }
        }
    } else if real_regalloc && contains_fork {
        vuma_log!(debug,
            "x86_64 regalloc: function '{}' contains spawn_worker/fork; \
             falling back to stack-slot ISel (fork+regalloc not supported)",
            func.name);
    }

    // Fallback (default path or regalloc failure): stack-slot ISel.
    let mut allocated = stack_slot_isel::allocate_registers(func)?;
    if let Some(alloc) = try_real_regalloc(func) {
        crate::regalloc_emit::annotate_with_regalloc(&mut allocated, &alloc);
    }
    Ok(allocated)
}
```

### 5.3 `verify_callee_saved_x86_64` (new verifier)

The existing `verify_callee_saved` (`regalloc.rs:4860`) is hard-coded to aarch64's `PhysReg` enum and register encoding ranges. It cannot be reused as-is. Add a sibling:

```rust
/// Verify that every physical register used by the regalloc is either
/// (a) caller-saved, (b) in `used_callee_saved`, or (c) RAX/RSP/RBP
/// (always-allowed).  Mirrors `regalloc::verify_callee_saved` but for
/// the x86_64 register file and the target-agnostic `RegAllocResult`.
pub fn verify_callee_saved_x86_64(
    result: &crate::regalloc::RegAllocResult,
) -> std::result::Result<(), String> {
    // Allowed GPRs by index (System V AMD64):
    //   caller-saved: 0 (RAX), 1 (RCX), 2 (RDX), 6 (RSI), 7 (RDI),
    //                 8 (R8), 9 (R9), 10 (R10), 11 (R11)
    //   always-allowed: 4 (RSP), 5 (RBP)
    //   callee-saved: from result.used_callee_saved (3, 12, 13, 14, 15 typically)
    let mut allowed_gprs: HashSet<u32> = [0, 1, 2, 4, 5, 6, 7, 8, 9, 10, 11].into_iter().collect();
    for preg in &result.used_callee_saved {
        if preg.class == crate::backend::RegClass::Gpr {
            allowed_gprs.insert(preg.index);
        }
    }
    // All XMMs are caller-saved.
    let allowed_xmms: HashSet<u32> = (0..=15).collect();

    let check = |preg: crate::backend::PhysicalReg| -> Option<String> {
        match preg.class {
            crate::backend::RegClass::Gpr => {
                if !allowed_gprs.contains(&preg.index) {
                    return Some(format!(
                        "GPR index {} is not caller-saved, not in used_callee_saved, \
                         and not RSP/RBP", preg.index));
                }
            }
            crate::backend::RegClass::SimdFp => {
                if !allowed_xmms.contains(&preg.index) {
                    return Some(format!("XMM index {} out of range", preg.index));
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

(`sc.phys_reg()` is a convenience accessor on `GenericSpillCode` that R2-b-impl may need to add — currently the enum's spill/reload variants each carry `preg: PhysicalReg` field; expose it via a method or a `match`.)

### 5.4 x86_64 frame layout (proposed)

The aarch64 regalloc emitter uses a layout where the callee-saved area lives **below** SP (between SP and the FP/LR save pair). x86_64 cannot use the same layout because `push` decrements SP and stores *above* the new SP — the callee-saved area ends up **above** the spill area, which is the opposite of aarch64.

**Proposed x86_64 layout (low → high addresses):**

```text
   [spill area]                  ← RBP-relative negative offsets; addressed via [rbp - off]
                                  Size = alloc.total_spill_slots * 8, aligned to 16.
   [RBP]                         ← Frame pointer.
   [return address]              ← Pushed by `call`.
   [callee-saved saves]          ← push rbx; push r12; ... (in the caller's frame).
                                  Size = N_callee_saved * 8.
   [caller's frame]              ← Caller's RSP at call time.
```

This layout has the callee-saved saves **above** RBP (in the caller's frame), and the spill area **below** RBP. RBP-relative addressing for spill slots is `mov rax, [rbp - off]` (negative offset). For callee-saved restores, the `pop` instructions unwind the stack in reverse order.

**Critical:** the per-function structural invariants (cap sig, formal-verify counter, channel seq counter, proto state, circuit breaker state) currently use specific RBP-relative offsets computed in the stack-slot ISel (`stack_slot_isel.rs:1180-1247`). The register-based emitter must use **different** offsets for `alloc.spill_slots` to avoid colliding with these reserved slots. Recommended approach: reserve the existing structural-invariant offsets as a "fixed region" below RBP, then place `alloc.spill_slots` *below* that region.

---

## 6. TargetDesc Readiness

The x86_64 TargetDesc at `target_desc.rs:1932-2040` is **complete enough** to support register-based emission. Specifically:

| Required field                                   | Present? | Source |
|--------------------------------------------------|----------|--------|
| All 16 GPRs with ABI roles                       | ✅       | `:1933-1980` |
| All 16 XMM registers                             | ✅       | `:1963-1979` |
| Caller-saved vs callee-saved classification      | ✅       | `callee_saved()` builder calls on RBX/RBP/R12-R15 |
| Stack pointer (RSP) marked non-allocatable       | ✅       | `stack_pointer()` at `:1943` sets `is_allocatable = false` |
| Frame pointer (RBP) marked                       | ✅       | `frame_pointer()` at `:1945` (note: `frame_pointer()` does NOT set `is_allocatable = false`; RBP remains allocatable — the linear-scan allocator might pick it, which is a bug if the emitter also uses RBP as the frame base. **R2-b-impl must mark RBP as non-allocatable in `x86_64_target_desc()` via `.not_allocatable()` or exclude RBP from `TargetAgnosticRegAlloc`'s pool.**) |
| Argument register positions                      | ✅       | `.arg(0)` through `.arg(5)` on RDI/RSI/RDX/RCX/R8/R9 |
| Return register (RAX)                            | ✅       | `.return_reg()` on RAX |
| Calling convention descriptor                    | ✅       | `:1982-1994` (systemv, 16-byte alignment, no link register) |
| `TargetAgnosticRegAlloc` already produces `RegAllocResult` for x86_64 | ✅ | `try_real_regalloc` at `mod.rs:4066-4095` proves this works in production today. |

**Gaps found in this audit:**

1. **RBP allocatability bug.** `RegDesc::gpr("RBP", 5).frame_pointer().callee_saved()` at `target_desc.rs:1945` does not chain `.not_allocatable()`. The `frame_pointer()` builder at `target_desc.rs:1140-1143` does NOT set `is_allocatable = false`. This means `TargetAgnosticRegAlloc::new(target)` at `regalloc.rs:2768-2791` includes RBP in the caller-saved-or-callee-saved pool (it's marked callee-saved) and the linear-scan allocator may assign vregs to RBP. If the register-based emitter also uses RBP as the frame base (the existing convention), this is a conflict. **Fix:** add `.not_allocatable()` to the RBP line in `x86_64_target_desc()`. (Also affects aarch64's `X29` registration at `target_desc.rs:1420`-area, but aarch64's `LinearScanAllocator` is hard-coded and doesn't consult `TargetDesc`, so this latent bug is invisible there.)

2. **No FP-rel spill-slot offsets.** The `RegAllocResult.spill_slots` field is a `HashMap<IRValueId, GenericSpillSlot>`. The `GenericSpillSlot` struct (not yet read in this audit — defined around `regalloc.rs:3310`) has an `offset` field but it is unclear whether that offset is computed relative to the frame pointer or is just an index. R2-b-impl must verify the offset semantics and, if needed, translate slot indices to RBP-relative offsets in the emitter. (For comparison, the aarch64 `SpillSlot` at `regalloc.rs:759` has an explicit `offset: i32` field documented as negative for below-FP slots.)

**Conclusion:** TargetDesc readiness is **HIGH** with the single RBP-allocatability fix. No new TargetDesc fields are needed.

---

## 7. Risk Assessment

### 7.1 REX prefix correctness — **LOW**

The existing `encode_*` helpers in `x86_64/mod.rs` (60+ functions) already handle REX prefix emission correctly, including the SPL/BPL/SIL/DIL low-byte case. The register-based emitter just calls these helpers with whatever `Gpr` the allocator assigned; no new REX code is needed. The single edge case to verify: REX.W is required for 64-bit operand size on `mov`, `add`, `sub`, etc. — the existing helpers already emit it (e.g. `encode_mov_reg_reg` at `mod.rs:316` uses 0x48 REX.W prefix). **Mitigation:** the existing QEMU smoke tests on x86_64 will exercise R8–R15 paths immediately once the env var is enabled.

### 7.2 RIP-relative addressing — **LOW**

x86_64 uses RIP-relative addressing for global variables (`lea rax, [rip + disp32]`). The existing `encode_lea_rip_rel` at `mod.rs:686` handles this. The register-based emitter inherits the stack-slot ISel's `GetAddress` arm verbatim (`stack_slot_isel.rs:2539`), so no new code is needed. The single hazard: if the emitter inserts spill/reload instructions between a `lea rip+disp` and the label it references, the disp32 value computed by `apply_fixups` will be wrong. **Mitigation:** spills/reloads are RBP-relative, not RIP-relative, so they don't perturb RIP-relative fixup math. Spot-check with `mem_copy_buffer.vuma` (the test that exposed the aarch64 greedy SIGSEGV).

### 7.3 Callee-saved tracking — **HIGH**

This is the same HIGH risk identified for aarch64 in F2-a §5.3 and Wave-1's G1/G2/G4 fixes. The hazards:

1. **Spill-scratch register clobbering callee-saved.** If a spill/reload path uses a callee-saved register as a scratch (the aarch64 path used X0; the x86_64 path must NOT use RBX/R12–R15 — only RAX/RCX/RDX/R10/R11 are caller-saved scratches). **Mitigation:** the `x86_64/stack_slot_isel.rs:9-14` already documents the scratch register convention (RAX, RCX, RDX, R10, R11). The register-based emitter must follow the same convention.

2. **`used_callee_saved` set incompleteness.** If the linear-scan allocator misses a callee-saved register that a spilled-reload path uses as a scratch, the epilogue will restore garbage into it. **Mitigation:** the new `verify_callee_saved_x86_64` (§5.3) catches this. Wire it behind `VUMA_VERIFY_CALLEE_SAVED=1` for the curated test subset before flipping the default.

3. **Callee-saved register interaction with `clone()`.** The `clone` syscall returns in the child process with all registers in their pre-syscall state. If the parent had `push rbx` in the prologue and the child reaches the epilogue, the child's `pop rbx` will pop a value that the parent's push put on the child's stack copy — this is actually correct (clone copies the stack). The real hazard is that the child may execute a *different* code path (the `if pid == 0` branch) and clobber callee-saved registers without restoring them. **Mitigation:** the `contains_fork` opt-out (§3 step 2) sidesteps this entirely by falling back to stack-slot for fork-containing functions.

### 7.4 Fork + regalloc — **MEDIUM**

Same as Wave-1 R1-b2-fix on aarch64, but with **different syscall numbers** (Linux/x86_64: clone=56, vfork=58; Linux/aarch64: clone=220, vfork=221). The `contains_fork` predicate must use the x86_64 numbers — copying the aarch64 predicate verbatim would silently miss clone calls on x86_64. **Mitigation:** §5.2 sketch uses the correct numbers (56/58). Add a unit test asserting the predicate matches on a fixture with `IRInstr::Syscall { nr: 56, .. }`.

### 7.5 Stack-frame layout drift — **MEDIUM**

The stack-slot ISel computes a specific `frame_size` (`stack_slot_isel.rs:1258-1263`) that includes the structural-invariant slots but does NOT include a callee-saved save area (it pushes no callee-saved regs). The register-based emitter must compute a *different* `frame_size` that includes the callee-saved save area (size = `used_callee_saved.len() * 8`, aligned to 16). If the `AllocatedFunction.frame_size` field is set to the wrong value, debug/unwind info will be wrong (but QEMU execution will still be correct because the bytes themselves honour the right layout). **Mitigation:** §5.4 documents the proposed layout. R2-b-impl must ensure `allocated.frame_size` is set from the register-based emitter's computed value, not from `stack_slot_isel`'s helper.

### 7.6 Two-operand ISA constraint — **MEDIUM**

The linear-scan allocator does not model x86_64's two-operand constraint. It will freely assign `dst` and `lhs` to different physical registers, forcing the emitter to insert a `mov dst, lhs` before each non-commutative op. This is correct but inflates code size and negates some of the register-allocation win. **Mitigation:** (a) accept the extra moves for R2-b-impl (correctness first), (b) future R2-c-opt task: teach the linear-scan allocator to coalesce `dst` and `lhs` for two-operand ISAs by adding an `eliminated_copies` field to `RegAllocResult` (mirroring `AllocationResult`'s field).

### 7.7 Per-function structural invariants interaction — **MEDIUM**

The stack-slot ISel's prologue embeds compile-time-computed capability-grant signatures, formal-verify counter pre-loads, channel sequence counter initialisation, etc. (§1.4). The register-based emitter must either (a) replicate this prologue verbatim, or (b) extract a shared helper. Option (b) is preferred but requires refactoring `stack_slot_isel.rs`'s prologue builder into a callable function — a non-trivial change because the current prologue is inline in `allocate_registers` and captures many local variables (frame_size, cap_grant_sig, cap_grant_sig_input, formal_verify_count_off, etc.). **Mitigation:** for R2-b-impl, take option (a) — copy the prologue code into `reg_isel.rs`. Refactor to a shared helper in a follow-up PR.

### 7.8 SIMD/FP register allocation — **MEDIUM**

`TargetAgnosticRegAlloc` does allocate XMM registers (the `caller_saved_fps` / `callee_saved_fps` pools are populated from the TargetDesc), but the existing stack-slot ISel's FP path uses fixed XMM0/XMM1 for all operations (`stack_slot_isel.rs:1626-1651`). The register-based emitter must consult `alloc.vreg_to_preg` for FP vregs too. The `Xmm` enum at `mod.rs:150` is complete (16 registers), and `Gpr::is_callee_saved()` is mirrored implicitly (all XMMs are caller-saved). **Mitigation:** start R2-b-impl with **integer-only** register allocation (FP vregs fall back to the stack-slot pattern via `dst_spilled`); add FP register allocation in a follow-up.

### 7.9 Syscall-position tracking (G6) — **LOW (already fixed)**

Wave-1's G6 fix at `regalloc.rs:954` (tracking `IRInstr::Syscall` as a call position) is in `LiveRangeComputer::compute`, which both `LinearScanAllocator` (aarch64) and `TargetAgnosticRegAlloc` (x86_64) use. x86_64 already benefits. **Mitigation:** none needed — verify with a `try_recv`-equivalent x86_64 test in the curated subset.

---

## 8. Phased Rollout Plan

### Phase 2a — x86_64 reg_isel skeleton (integer-only, no FP, no SIMD)

1. Create `src/codegen/src/x86_64/reg_isel.rs` with the public API from §5.1.
2. Implement `preg_to_gpr` / `preg_to_xmm` translation.
3. Implement `verify_callee_saved_x86_64` (§5.3).
4. Implement prologue/epilogue (§5.1.5, §5.1.6) with callee-saved `push`/`pop` from `alloc.used_callee_saved`.
5. Implement per-IR-instruction arms for: `Add`, `Sub`, `Mul`, `Div`, `BinOp`, `UnaryOp`, `Cmp`, `Cast` (integer kinds), `Load`, `Store`, `Offset`, `GetAddress`, `Alloc`, `Free`, `Branch`, `CondBranch`, `Ret`, `Phi` (no-op), `Select`, `Call` (direct, integer args), `CallIndirect`, `Syscall` (Linux x86_64 ABI).
6. **Defer to Phase 2c:** all Channel*/StarkProof builtins, AtomicCas, VectorOp, FP-typed Add/Sub/Mul/Div/Cmp/Cast (these fall back to the stack-slot pattern via `dst_spilled` — correct but slow).
7. Wire up `X86_64Backend::allocate_registers` (§5.2) gated by `VUMA_REAL_REGALLOC_X86_64=1` (default off).
8. Run curated x86_64 test subset under `qemu-x86_64-static` with the env var on. Triage failures.

**Estimated effort:** 2–3 weeks (R2-b-impl).

### Phase 2b — x86_64 reg_isel FP/SIMD

1. Add FP-typed Add/Sub/Mul/Div/Cmp/Cast arms honouring `alloc.vreg_to_preg` for `RegClass::SimdFp` vregs.
2. Add AtomicCas, VectorOp arms.
3. Re-run curated subset; expect binary size reduction on FP-heavy tests.

**Estimated effort:** 1–2 weeks (R2-c-opt or R2-d-impl).

### Phase 2c — x86_64 reg_isel IPC/capability builtins

1. Add `ChannelOpen`, `ChannelSend`, `ChannelRecv`, `ChannelClose`, `ChannelRecvTimeout`, `ChannelRecvResult`, `StarkProof` arms.
2. Each builtin must consult the per-function structural-invariant slots (cap sig, formal-verify counter, channel seq counter) — reuse the stack-slot ISel's emit code for these arms.
3. Re-run curated IPC subset; verify the formal-verify counter still increments correctly.

**Estimated effort:** 1 week (R2-e-impl).

### Phase 2d — Default-on

1. Run the full 30-test curated matrix (`scripts/audit/regalloc_endianness_wave1_aarch64_regalloc_30test.md` equivalent for x86_64) under regalloc.
2. Verify ≥ 28/30 pass (DoD threshold from Wave-1 R1-c-test).
3. Flip `VUMA_REAL_REGALLOC_X86_64` default to `1`.
4. Update `docs/caveats.md` §2.1 to reflect x86_64 now emits register-based bytes.

**Estimated effort:** 2–3 days (R2-f-verify + R2-g-default).

### Phase 2e — RBP allocatability fix and refactor

1. Add `.not_allocatable()` to the RBP line in `x86_64_target_desc()` (§6 gap 1).
2. Refactor the stack-slot ISel's prologue builder into a shared `emit_prologue_common()` helper (§5.1.5 / §7.7).
3. Add `eliminated_copies` to `RegAllocResult` for two-operand coalescing (§7.6).

**Estimated effort:** 1 week (R2-h-cleanup).

---

## 9. Concrete Code Changes

| # | File | Change | LOC (est.) | Phase |
|--:|------|--------|-----------:|:------:|
| 1 | `src/codegen/src/x86_64/reg_isel.rs` (NEW) | New module: `allocate_registers`, `preg_to_gpr`, `preg_to_xmm`, `verify_callee_saved_x86_64`, per-IR-instruction arms, prologue/epilogue builders. | ~2000–2500 | 2a |
| 2 | `src/codegen/src/x86_64/mod.rs:4141-4154` | Rewrite `X86_64Backend::allocate_registers` per §5.2 sketch (env-var gate, fork opt-out with syscall nr=56/58, reg_isel dispatch, stack-slot fallback). | ~50 | 2a |
| 3 | `src/codegen/src/x86_64/mod.rs:3962-3975` | (Cosmetic, optional) Rename `emit_function_regalloc` → `emit_function_with_regalloc_metadata`; reserve the `emit_function_regalloc` name for the byte-changing path. Update `emit_function_with_regalloc` convenience method at `:3981-3987`. | ~10 | 2a (or defer) |
| 4 | `src/codegen/src/x86_64/mod.rs` (module decl) | Add `mod reg_isel;` (or `pub mod reg_isel;`) to the module's child-module declarations. | 1 | 2a |
| 5 | `src/codegen/src/target_desc.rs:1945` | Add `.not_allocatable()` to the RBP line: `RegDesc::gpr("RBP", 5).frame_pointer().callee_saved().not_allocatable()`. | 1 | 2a (recommended) or 2e |
| 6 | `src/codegen/src/regalloc.rs` (near `GenericSpillCode` enum, ~:3310-area) | Add a `phys_reg(&self) -> crate::backend::PhysicalReg` accessor on `GenericSpillCode` for use by `verify_callee_saved_x86_64`. | ~10 | 2a |
| 7 | `tests/` (NEW test file) | Add unit tests for `verify_callee_saved_x86_64` (positive + negative cases). | ~50 | 2a |
| 8 | `tests/` (NEW integration test) | Add a `try_recv`-equivalent x86_64 test that exercises the Syscall-position tracking on x86_64 (G6 regression guard). | ~30 | 2a |
| 9 | `docs/caveats.md` §2.1 | Document the new `VUMA_REAL_REGALLOC_X86_64` env var and the fork opt-out (clone=56/vfork=58). | ~20 | 2a |

**Total LOC for Phase 2a:** ~2200–2700 (dominated by the new `reg_isel.rs`).

---

## 10. Effort Estimate

**F2-a estimate:** 2–4 weeks (3–4 weeks in §6 Phase 2, p. 413).

**This audit's revised estimate:**

| Phase | Effort (developer-weeks) | Notes |
|-------|--------------------------|-------|
| 2a — integer-only skeleton + wire-up + fork opt-out + verifier | 2–3 | Bulk of the work: ~30 IR instruction arms in `reg_isel.rs`, each adapting the existing stack-slot arm to honour `vreg_to_preg`. The aarch64 wire-up at `backend.rs:3175-3300` is the template. |
| 2b — FP/SIMD arms | 1–2 | Adds ~10 arms, mostly SSE/SSE2 encoder calls (already exist). |
| 2c — IPC/capability builtin arms | 1 | Adds ~7 arms, mostly verbatim copies of stack-slot arms (the builtins consult structural-invariant slots that already exist). |
| 2d — default-on + verification | 0.5 | Run curated matrix, flip default, update docs. |
| 2e — cleanup (RBP fix, refactor, coalescing) | 1 | Optional; can ship without. |
| **Total (Phases 2a–2d, required for default-on)** | **4.5–6.5** | |
| **Total (Phases 2a–2e, with cleanup)** | **5.5–7.5** | |

**Achievable in this orchestration run? N.**

The orchestration run is operating under a 10-minute-per-task budget for sub-agents (per R2-a-audit's own constraint). The x86_64 register-based emitter is genuinely 4.5–6.5 developer-weeks of work — it is the largest single task in Wave 2 (larger than aarch64's Wave-1 wire-up, which was 1–2 weeks and consumed multiple sub-agent runs: R1-a through R1-f). The R2-b-impl sub-agent should be tasked with **Phase 2a only** (integer-only skeleton + wire-up + verifier + env-var gate, default off), which is itself 2–3 weeks and will require multiple sub-agent invocations to complete iteratively (mirror the Wave-1 R1-a→R1-b→R1-b2→R1-b3→R1-c→R1-f cadence). Default-on (Phase 2d) is a separate task after Phase 2a's curated-subset verification passes.

Per §0.7-6 of the orchestration protocol, this honest estimate should cause the orchestrator to defer the bulk of the work to a human developer OR sequence it across many orchestration waves. The R2-a-audit deliverable (this document) is itself the actionable artefact: R2-b-impl can proceed incrementally off it.

**Key risks that could inflate the estimate:**

- The per-function structural invariants (cap sig computation, formal-verify counter) are subtle and have many edge cases; replicating them in `reg_isel.rs` without refactoring `stack_slot_isel.rs` first will create maintenance hazard (§7.7).
- The two-operand ISA constraint (§7.6) will inflate code size in Phase 2a unless the linear-scan allocator is taught to coalesce; this is a known follow-up.
- The RBP allocatability bug (§6 gap 1) must be fixed in Phase 2a, not deferred — otherwise the linear-scan allocator will assign vregs to RBP and the emitter's frame-base convention will conflict.

---

## DoD Check

- [x] Design doc exists at `scripts/audit/regalloc_endianness_wave2_x86_64_design.md`.
- [x] All 10 required sections present: §1 Current Path, §2 Register File, §3 What emit_function_regalloc Needs, §4 Reusable Components, §5 New Components, §6 TargetDesc Readiness, §7 Risk Assessment, §8 Phased Rollout, §9 Concrete Code Changes, §10 Effort Estimate.
- [x] Concrete line numbers cited for every code path: §1 `mod.rs:4141`, `:4066`, `:3962`, `stack_slot_isel.rs:1277`, `:1594`, `:1385`; §2 `target_desc.rs:1932-2040`, `mod.rs:51-150`; §4 `emit.rs:1056-1354`, `regalloc.rs:4860`, `backend.rs:3226`; §5 `mod.rs:4141-4154`, `target_desc.rs:1945`, `regalloc.rs:3310`; §6 `target_desc.rs:1945`; §7 multiple.
- [x] Honest effort estimate: 4.5–6.5 developer-weeks total; Phase 2a alone is 2–3 weeks; **NOT achievable in a single 10-minute orchestration sub-agent run** — recommendation is to sequence R2-b-impl across multiple waves or defer to human developer per §0.7-6.
- [x] No source files edited (READ-ONLY audit — `git status --short` shows only the new markdown added).
- [x] No `git push`.
- [x] No sub-agents spawned.
