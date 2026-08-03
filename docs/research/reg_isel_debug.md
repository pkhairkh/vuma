# reg_isel.rs Debugging Notes (x86_64)

## Test: arith_mul_table
- **Expected exit code:** 36
- **Actual exit code (reg_isel, BEFORE W1-A fix):** 4 (wrong result)
- **Actual exit code (reg_isel, AFTER W1-A proof-of-concept fix):** 36 (passes)
- **Actual exit code (stack_slot, production path):** 36 (passes)

## What arith_mul_table does

`tests/gold_standard/arithmetic/arith_mul_table.vuma` computes the sum of
products `i*j` for `i` in `1..=3` and `j` in `1..=3` using nested `while`
loops with the verified counter pattern:

```vuma
transform main() -> i32 {
    total: u32 = 0;
    i: u32 = 1;
    one: u32 = 1;
    four: u32 = 4;
    while i < four {
        j: u32 = 1;
        while j < four {
            total = total + i * j;
            j = j + 1;
        }
        i = i + 1;
    }
    return total;
}
```

Products: 1,2,3,2,4,6,3,6,9 — sum = **36**.

The IR emitted by `scg_to_ir` heavily uses `Add { dst, lhs, rhs: Immediate(0) }`
as a phi / copy primitive between loop iterations. Each loop back-edge is a
block of four `Add { dst: vreg_K, lhs: vreg_K', rhs: 0 }` instructions that
"join" the post-iteration values of `total`, `one`, `i`, `four` back into
their canonical vreg IDs. This pattern is what triggers the bug.

## Root cause(s) identified

### Finding 1: `resolve_register_reuse_conflicts` corrupts spilled vregs by removing them from `vreg_to_preg`

- **Symptom (objdump):** In the outer-loop back-edge / join block
  (`loop_continue_3`, `bb13`), the four `Add { dst, lhs, rhs: 0 }` reloads
  for `total` and `one` are followed by `mov %rax, %rdi` and `mov %rax, %r8`
  — using **RAX** (stale) instead of **R11** (the just-reloaded value).
  The reloads for `i` and `four` correctly use `mov %r11, %r9` and
  `mov %r11, %r10`. The net effect is that `rdi` (total) is overwritten
  with `i+1` on every outer iteration, so after 3 iterations the function
  returns `i+1 = 4` instead of `36`.

- **Location in regalloc.rs:** `resolve_register_reuse_conflicts`
  (line 2846) → `resolve_single_conflict` (line 2934). The buggy path is
  the "no free register available — spill the def vreg" block at
  **lines 2991-2998**, which does:
  ```rust
  result.spill_slots.insert(def_vreg, slot.clone());
  result.vreg_to_preg.remove(&def_vreg);   // <-- BUG: breaks invariant
  ```
  without generating any `GenericSpillCode` for the new slot.

- **Location in reg_isel.rs:** `resolve_value` (line 464-491) falls back to
  `ResolvedVal::Reg(Gpr::Rax)` whenever a vreg is not in `vreg_to_preg`
  (line 476-487). The comment "should not happen in correct alloc" is
  accurate — the bug is in `resolve_register_reuse_conflicts`, not the
  emitter. `emit_spill_code` (line 513-536) faithfully emits the
  `Reload { preg: R11, slot: <old slot> }` it was given, but the Add
  emitter in `emit_instruction` (line 541-565 for `IRInstr::Add`,
  line 838-859 for `BinOp::Add`) then calls `load_to_reg(lhs)` which
  resolves to RAX instead of R11, producing `mov %rax, %dst`.

- **Stack-slot path:** Does not call `resolve_register_reuse_conflicts`
  (the stack-slot ISel uses a different allocator / spill strategy that
  spills every vreg to its own slot at function entry). The conflict
  resolver is only invoked from `TargetAgnosticRegAlloc::allocate_function`
  (regalloc.rs line 3108) and `allocate_function_with_classes` (line 3128),
  which feed `emit_function_regalloc_full` in `reg_isel.rs`.

- **Hypothesis:** The conflict checker treats `use_preg == def_preg == R11`
  (the spill-scratch register) as a "register-reuse hazard". This is a
  false positive: when BOTH the use and def vregs are spilled with the
  same scratch, the spill mechanism already handles it correctly — the
  pre-instruction `Reload` brings `use_vreg` from its slot into R11, the
  instruction writes `def_vreg`'s new value into R11, and the
  post-instruction `Spill` (at `def_pos + 1`) stores R11 to `def_vreg`'s
  slot. By removing `def_vreg` from `vreg_to_preg`, the conflict resolver
  (a) creates a NEW spill slot that is never written, (b) leaves the OLD
  spill_code (referencing the OLD slot) in place, and (c) causes
  `resolve_value` to return RAX so the Add writes the new value to RAX,
  while the post-instruction Spill stores the OLD R11 value (the reloaded
  `use_vreg`) to the OLD slot. Subsequent reloads of `def_vreg` then read
  the OLD `use_vreg` value.

- **Evidence (debug output with `VUMA_DEBUG_REG_ISEL=1`):**
  ```
  CONFLICT: def_vreg=41 use_vreg=34 conflict_preg=Gpr:11 pos=92
  CONFLICT_SPILL: def_vreg=41 removed from vreg_to_preg, new slot idx=6 offset=-56
  CONFLICT: def_vreg=40 use_vreg=33 conflict_preg=Gpr:11 pos=94
  CONFLICT_SPILL: def_vreg=40 removed from vreg_to_preg, new slot idx=7 offset=-64
  RESOLVE_FALLBACK: vreg=41 root=41 (not in vreg_to_preg) -> RAX; spill_slot=Some({ index: 6, offset: -56 })
  RESOLVE_FALLBACK: vreg=40 root=40 (not in vreg_to_preg) -> RAX; spill_slot=Some({ index: 7, offset: -64 })
  ```
  Note the slot mismatch: `spill_slots[41] = slot 6 (offset -56)` but the
  spill_code (from `gen_spill_reload`) uses `slot 5 (offset -48)`. The
  reload at the use position reads slot 5 (correct old slot), but
  `resolve_value` returns RAX, so the new value goes to RAX, not R11.

- **Why `vreg 39` and `vreg 38` are NOT affected:** Their `lhs` operands
  (`vreg 36` and `vreg 30`) are NOT spilled (they live in real registers),
  so `use_preg != def_preg` and no conflict is triggered. The reload +
  `mov %r11, %r9` / `mov %r11, %r10` work correctly. The bug only fires
  when BOTH use and def vregs are spilled to the SAME scratch register.

### Finding 2 (secondary): post-instruction spill stores the WRONG register when `resolve_value` falls back to RAX

- **Symptom:** After the buggy Add writes `vreg 41`'s new value to RAX,
  the post-instruction spill at `pos+1` emits `mov %r11, -0x30(%rbp)`
  (spill R11 to slot 5). But R11 still holds the OLD `use_vreg` value
  (from the pre-instruction reload), NOT the new `def_vreg` value (which
  is in RAX). So slot 5 gets the OLD value, and the NEW value in RAX is
  discarded (clobbered by the next instruction).

- **Location in reg_isel.rs:** The post-instruction spill mechanism
  (lines 258-274) iterates `alloc.spill_code.get(&(global_pos + 1))` and
  calls `emit_spill_code`. `emit_spill_code` (line 522-526) emits
  `mov [rbp+offset], preg` using the `preg` from the `GenericSpillCode::Spill`
  entry — which is R11 (the scratch). It does NOT consult `resolve_value`
  to find where the instruction actually wrote the value. When the Add
  wrote to RAX (due to the fallback), the spill stores R11 (stale) instead.

- **Stack-slot path:** Not affected (doesn't use scratch + spill_code).

- **Hypothesis:** This is a downstream symptom of Finding 1, NOT an
  independent bug. Once Finding 1 is fixed (so `resolve_value` returns
  R11 for the spilled def_vreg), the Add will correctly write to R11, and
  the post-instruction spill will store the correct value. No separate
  fix needed for Finding 2 — but it explains why the bug is so destructive
  (the slot gets the OLD value, not garbage).

- **Evidence:** The debug output shows
  `SPILL_CODE: spill %v41 -> Gpr:11 [slot 5 offset -48]` immediately
  after `RESOLVE_FALLBACK: vreg=41 -> RAX`. The Add wrote to RAX; the
  spill stored R11 (which held the reloaded `vreg 34`).

## Recommended fixes (for W1-B/C/D)

### W1-D (reload-after-spill / `resolve_register_reuse_conflicts`) — PRIMARY FIX

**File:** `src/codegen/src/regalloc.rs`

**Fix (proof-of-concept, applied in this W1-A commit):** In
`resolve_register_reuse_conflicts`, after the `use_preg != def_preg`
check (line 2877-2879), add a skip for the case where EITHER `use_preg`
OR `def_preg` is the spill-scratch register (R11 = index 11 on x86_64
GPR). When a vreg is mapped to the scratch, it means the vreg is spilled
and only transiently lives in R11 — the spill_code mechanism already
handles the reload-before-use / spill-after-def correctly, so there is
no real "register-reuse hazard" to resolve.

```rust
// After: if use_preg != def_preg { continue; }

// If EITHER use_preg or def_preg is the spill-scratch register
// (R11 on x86_64 GPR, index 11), there is no real conflict. The
// spill mechanism reloads the use_vreg into scratch just for this
// instruction, the instruction writes the new def_vreg value to
// scratch, and the post-instruction spill stores scratch to the
// def_vreg's slot. Treating this as a "conflict" and removing the
// def_vreg from vreg_to_preg breaks the invariant that
// resolve_value() returns the scratch for spilled vregs.
let scratch_idx = 11; // R11 on x86_64 GPR — see spill_scratch()
if use_preg.class == crate::backend::RegClass::Gpr
    && (use_preg.index == scratch_idx || def_preg.index == scratch_idx)
{
    continue; // scratch register — no real conflict
}
```

**Verification:** With this fix, `arith_mul_table` exits 36 (was 4).
The proof-of-concept fix is already applied in this commit; W1-D should:

1. Generalize the scratch-register detection. The current POC hardcodes
   `index == 11` for x86_64. The clean solution is to call
   `self.spill_scratch(RegClass::Gpr)` (regalloc.rs line 3491) and
   compare. Since `resolve_register_reuse_conflicts` is a free function
   (not a method on `TargetAgnosticRegAlloc`), W1-D will need to either
   pass the scratch register in as a parameter, or refactor
   `spill_scratch` into a standalone function keyed on `isa_name`.

2. Investigate the other failing tests
   (`arith_modular_exp`=136 FPE, `arith_next_power_of_two`=124 timeout,
   `aead`/`shared_memory_rw`/`stark_proof`=0). They have DIFFERENT root
   causes (this fix does NOT resolve them) — likely W1-B (two-operand
   ALU for `div`/`idiv`, which clobbers RAX/RDX) and W1-C (caller-saved
   spills across `call`).

3. Consider hardening `resolve_single_conflict` itself (lines 2991-2998):
   even when a real conflict exists, the current "spill the def vreg"
   path is broken because it removes the vreg from `vreg_to_preg`
   without generating spill_code for the new slot. The correct behavior
   is to call `gen_spill_reload` (regalloc.rs line 3507) with the new
   slot and the scratch register, which inserts the vreg into
   `vreg_to_preg` as the scratch and generates proper
   spill-after-def / reload-before-use entries. This is the defensive
   fix that would prevent similar bugs if the skip in step 1 ever
   misses a case.

### W1-B (two-operand ALU encoding) — NOT the bug for arith_mul_table

The `Add`/`Sub`/`Mul` emitters (reg_isel.rs lines 541-565, 838-908)
correctly handle the 2-operand x86 encoding: they emit
`mov dst, lhs; op dst, rhs` and skip the `mov` when `lhs == dst`
(already aliased). The `imul` in `arith_mul_table`'s inner loop
(objdump line 661: `imul %rcx, %rbx` where `rbx` was just loaded from
`rdx`) is correctly 2-operand. **No fix needed in W1-B for this test.**

However, W1-B SHOULD investigate `Div`/`BinOp::SDiv`/`BinOp::UDiv`
(reg_isel.rs lines 622-744) for `arith_modular_exp` (exit 136 = FPE).
The div emitter saves RAX/RDX, but the `is_32bit` path (lines 639-644)
uses raw byte sequences (`0x89, 0xC0` for `mov eax, eax` etc.) that
may not handle R8-R15 correctly (no REX.B prefix for the `div ecx`
fallback at line 671). Also, the spill of RAX/RDX via `push`/`pop`
(lines 636-637, 681-682) does NOT spill RAX/RDX-based spilled vregs —
if a live vreg was reloaded into RAX before the div, the push/pop will
save/restore the WRONG value (the reloaded value, not the vreg's home).
W1-B should review whether the div path is clobbering caller-saved
registers that hold live spilled vregs.

### W1-C (calling convention spills) — NOT the bug for arith_mul_table

`arith_mul_table` has no `call` instructions (no syscalls, no function
calls). The bug is entirely within a single function's loop back-edge.
**No fix needed in W1-C for this test.**

However, W1-C SHOULD investigate `aead`, `shared_memory_rw`, `stark_proof`
(exit 0 = wrong result). These tests likely involve `call` instructions
(IPC / shared memory / crypto routines). The reg_isel.rs `Call` emitter
(around the `IRInstr::Call` arm) must spill all caller-saved registers
that hold live vregs BEFORE the call, and reload them AFTER. If the
spill_code mechanism doesn't cover call boundaries (the
`alloc.spill_code` is keyed on instruction `pos`, but call clobbering is
a range, not a point), live values in RAX/RCX/RDX/RSI/RDI/R8-R11 will be
destroyed by the callee. W1-C should audit the `Call` emitter and the
regalloc's `crosses_call` handling (regalloc.rs line 4917 — prefers
callee-saved for intervals that cross calls, but does NOT explicitly
spill caller-saved live values at the call site).

### W1-D (reload-after-spill) — see primary fix above

The `arith_next_power_of_two` test (exit 124 = timeout) likely has an
infinite loop caused by a DIFFERENT reload-after-spill bug: the loop
counter `n` is probably spilled and not reloaded correctly, so the loop
condition never becomes false. W1-D should:
1. Apply the primary fix above (resolves `arith_mul_table`).
2. Generate objdumps for `arith_next_power_of_two` (both stack-slot and
   reg_isel) and diff the loop-counter handling.
3. Check whether `next_power_of_two` uses `Shl`/`Shr` (reg_isel.rs lines
   803-837) — the shift emitter uses CL as the shift count and may
   clobber RCX if RCX holds a live spilled vreg (similar pattern to the
   div issue in W1-B).

## Objdump snippets

### Stack-slot (passing) — `_vuma_main` prologue + first loop header
```
  4107ae:       55                      push   %rbp
  4107af:       48 89 e5                mov    %rsp,%rbp
  4107b2:       48 81 ec 90 04 00 00    sub    $0x490,%rsp
  4107b9:       48 31 c0                xor    %rax,%rax
  4107bc:       48 89 85 50 fe ff ff    mov    %rax,-0x1b0(%rbp)   ; zero-init locals
  ...
  410808:       48 89 45 b8             mov    %rax,-0x48(%rbp)    ; total=0
  41081a:       48 89 45 c0             mov    %rax,-0x40(%rbp)    ; i=1
  41082c:       48 89 45 c8             mov    %rax,-0x38(%rbp)    ; one=1
  41083e:       48 89 45 d0             mov    %rax,-0x30(%rbp)    ; four=4
  41084c:       48 8b 45 c8             mov    -0x38(%rbp),%rax    ; reload i
  410850:       48 8b 4d d0             mov    -0x30(%rbp),%rcx    ; reload four
  410854:       48 39 c8                cmp    %rcx,%rax           ; i < four ?
```
Every variable lives in its own stack slot; reloads happen before every
use. No register reuse, no conflict resolution needed.

### reg_isel (FAILING, before W1-A fix) — outer-loop back-edge join block
```
  410a2c:   4c 8b 5d d0             mov    -0x30(%rbp),%r11   ; reload vreg41 (total_new) -> R11
  410a30:   48 89 c7                mov    %rax,%rdi          ; BUG: rdi = RAX (should be R11)
  410a33:   48 81 c7 00 00 00 00    add    $0x0,%rdi
  410a3a:   4c 8b 5d d8             mov    -0x28(%rbp),%r11   ; reload vreg40 (one_new) -> R11
  410a3e:   49 89 c0                mov    %rax,%r8           ; BUG: r8 = RAX (should be R11)
  410a41:   49 81 c0 00 00 00 00    add    $0x0,%r8
  410a48:   4c 8b 5d e0             mov    -0x20(%rbp),%r11   ; reload vreg39 (i_new) -> R11
  410a4c:   4d 89 d9                mov    %r11,%r9           ; r9 = R11 (correct)
  410a4f:   49 81 c1 00 00 00 00    add    $0x0,%r9
  410a56:   4c 8b 5d e8             mov    -0x18(%rbp),%r11   ; reload vreg38 (four_new) -> R11
  410a5a:   4d 89 da                mov    %r11,%r10          ; r10 = R11 (correct)
  410a5d:   49 81 c2 00 00 00 00    add    $0x0,%r10
  410a64:   e9 a7 fd ff ff          jmp    0x410810           ; back to outer loop header
```

### reg_isel (PASSING, after W1-A proof-of-concept fix) — same join block
```
  410a26:   4c 8b 5d d0             mov    -0x30(%rbp),%r11   ; reload total_new -> R11
  410a2a:   4c 89 df                mov    %r11,%rdi          ; rdi = R11 (CORRECT)
  410a2d:   48 81 c7 00 00 00 00    add    $0x0,%rdi
  410a34:   4c 8b 5d d8             mov    -0x28(%rbp),%r11   ; reload one_new -> R11
  410a38:   4d 89 d8                mov    %r11,%r8           ; r8 = R11 (CORRECT)
  410a3b:   49 81 c0 00 00 00 00    add    $0x0,%r8
  410a42:   4c 8b 5d e0             mov    -0x20(%rbp),%r11   ; reload i_new -> R11
  410a46:   4d 89 d9                mov    %r11,%r9           ; r9 = R11
  410a49:   49 81 c1 00 00 00 00    add    $0x0,%r9
  410a50:   4c 8b 5d e8             mov    -0x18(%rbp),%r11   ; reload four_new -> R11
  410a54:   4d 89 da                mov    %r11,%r10          ; r10 = R11
  410a57:   49 81 c2 00 00 00 00    add    $0x0,%r10
  410a5e:   e9 ad fd ff ff          jmp    0x410810           ; back to outer loop header
```

### First divergence (reg_isel failing vs stack-slot passing)

The divergence is NOT in the prologue or first loop iteration (those
work). It appears only in the OUTER-LOOP BACK-EDGE join block, after
the inner loop has run. Specifically:

- **Stack-slot path:** The back-edge simply re-reads all four variables
  from their stack slots (no register state to corrupt). The loop
  condition `i < four` is evaluated on fresh reloads every iteration.
- **reg_isel path (failing):** The back-edge reloads the post-iteration
  values into R11, then emits `mov %rax, %rdi` (BUG) instead of
  `mov %r11, %rdi`. This overwrites `total` (in RDI) with whatever
  happens to be in RAX (which is `i+1` from the preceding
  `Add { dst: 36, lhs: 35, rhs: 0 }` instruction). After 3 outer
  iterations, `total` (RDI) = `i+1` = 4, and the function returns 4.

The bug is **deterministic** and **position-dependent**: it only fires
for the FIRST TWO reloads in the join block (vregs 41 and 40), because
those are the only two whose `lhs` operands (vregs 34 and 33) are ALSO
spilled with the same scratch R11. The other two reloads (vregs 39 and
38) have `lhs` operands in real registers, so no conflict is triggered
and the reload works correctly.

## Debug methodology (for W1-B/C/D to reuse)

1. Build with `cargo build --profile release-fast --bin compile_dump`.
2. Run with `VUMA_DEBUG_REG_ISEL=1 VUMA_REAL_REGALLOC_X86_64=1` to dump:
   - IR blocks and terminators (`emit_function_regalloc_full` header).
   - `SPILL_CODE:` lines from `emit_spill_code` (reg_isel.rs line 519).
   - `RESOLVE_FALLBACK:` lines from `resolve_value` (reg_isel.rs line 476)
     — these are the SMOKING GUN: any vreg that falls back to RAX is a
     bug in the regalloc, NOT the emitter.
   - `CONFLICT:` / `CONFLICT_SPILL:` lines from `resolve_single_conflict`
     (regalloc.rs lines 2921, 2999) — these identify which vregs are
     being incorrectly removed from `vreg_to_preg`.
3. Disassemble with `objdump -d` and find the `_vuma_main` function
   (called from entry at `0x41001a`; function starts at `0x4107ae`).
4. Cross-reference `RESOLVE_FALLBACK` vreg IDs with the IR dump to find
   the exact instruction where the bug manifests.
