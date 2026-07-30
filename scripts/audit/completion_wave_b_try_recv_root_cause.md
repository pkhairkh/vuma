# Wave B — `try_recv` Regalloc Root-Cause Report

- **Task ID:** CB-a-investigate
- **Wave:** B (try_recv Fix — investigation)
- **Prior-run context:** R1-b3-fix (`8194337b`) added `IRInstr::Syscall` to
  `call_positions` in `regalloc.rs:954`. `try_recv` no longer SIGSEGVs but
  exits 0 instead of the expected 77.
- **HEAD before this task:** `15a13de6` (`[CA-a-test]`).
- **Files in scope:** `tests/gold_standard/ipc/try_recv.vuma`,
  `src/codegen/src/ipc_lowering.rs`, `src/codegen/src/emit.rs`,
  `src/codegen/src/regalloc.rs`, `src/codegen/src/target_desc.rs`.
- **Files OUT of scope:** any source file (investigation only — no edits).

## 1. Reproduction

```bash
cd /home/z/my-project/vuma
# Regalloc path (bug):
VUMA_REAL_REGALLOC_AARCH64=1 target/release/compile_dump \
    tests/gold_standard/ipc/try_recv.vuma /tmp/cba_ra.bin aarch64
qemu-aarch64-static /tmp/cba_ra.bin
# regalloc exit=0  (expected 77, BUG: exits 0)

# Stack-slot path (correct):
target/release/compile_dump \
    tests/gold_standard/ipc/try_recv.vuma /tmp/cba_ss.bin aarch64
qemu-aarch64-static /tmp/cba_ss.bin
# stack-slot exit=77  (expected 77, CORRECT)
```

`qemu-aarch64-static -strace` confirms both paths see the **same** syscall
results (the program logic is identical up to register assignment):

```
pipe2(...) = 0
pipe2(...) = 0
fcntl(5, F_SETFL, O_RDONLY|O_NONBLOCK) = 0
nanosleep(...) = 0
fsetxattr(...) = -1 errno=14 (Bad address)   # poll nr=7 → fsetxattr on asm-generic
read(5, ..., 56) = -1 errno=11 (EAGAIN)       # empty pipe, O_NONBLOCK
exit(0)   # regalloc path  ← BUG (should be exit(77))
exit(77)  # stack-slot path  ← CORRECT
```

Both paths compute `poll_ret = -14` (EFAULT, because aarch64's asm-generic
nr 7 is `fsetxattr`, not `poll`) and `read_ret = -11` (EAGAIN). The
`expand_channel_try_recv` IR is designed to handle this: `poll_no_data =
(poll_ret <= 0)` is true, `read_failed = (read_ret != 56)` is true, so
`is_error = 1`, and `Select { cond: is_error, true_val: -2, false_val:
payload }` should yield `result = -2`, which the program maps to exit 77.

The bug is that the regalloc path's `Select` lowering returns the
**false_val** (payload = 0) instead of the **true_val** (-2) when
`is_error != 0`.

## 2. Program Logic

`tests/gold_standard/ipc/try_recv.vuma`:
```rust
transform main() -> i32 {
    ch = channel_open<i32>();
    result = channel_try_recv(ch);
    if result == 0 - 2 { return 77; }
    return result;
}
```

`expand_channel_try_recv` (`ipc_lowering.rs:3782-4028`) lowers to (key
tail):
```rust
IRInstr::Syscall { nr: 7,  args: [pollfd, 1, 0], dst: Some(poll_ret) },  // poll
IRInstr::Syscall { nr: 63, args: [read_fd, frame, 56], dst: Some(read_ret) }, // read
IRInstr::Load    { dst: payload, addr: frame, offset: 44, ty: I64 },
IRInstr::Cmp     { kind: SLe, dst: poll_no_data, lhs: poll_ret, rhs: Imm(0) },
IRInstr::Cmp     { kind: Ne,  dst: read_failed,  lhs: read_ret, rhs: Imm(56) },
IRInstr::BinOp   { op: And, dst: is_error, lhs: poll_no_data, rhs: read_failed },
IRInstr::Select  { dst: result, cond: is_error, true_val: Imm(-2), false_val: payload },
IRInstr::BinOp   { op: Add, dst: dst, lhs: result, rhs: Imm(0) },
```

For the empty-pipe case: `poll_ret=-14`, `read_ret=-11`, `payload=0`
(zero-initialised `Alloc`), so `is_error = 1`, `result` should be `-2`.

## 3. Root Cause: `IRInstr::Select` CSEL operand swap in `emit_ir_instr`

The `emit_ir_instr` Select arm (`src/codegen/src/emit.rs:2148-2183`,
used by `emit_function_regalloc`) emits the CSEL with **`rn` and `rm`
swapped** relative to the IR semantics and relative to the stack-slot
path's Select lowering (`emit.rs:5481-5508`, used by
`emit_function_greedy`).

### Buggy regalloc-path Select emit (`emit.rs:2174-2182`)

```rust
let rt = self.resolve_reg(true_val)?;    // rt = true_val (-2)
let rf = self.resolve_reg(false_val)?;   // rf = false_val (payload)
// Compare cond against zero and select.
self.emit_instruction_with_width(
    Instruction::SUB { rd: Register::XZR, rn: rc, rm: Operand::Imm12(0) },
    width,
)?;
// Set flags by using a separate CMP (SUB with XZR destination
// doesn't set flags; we need a flags-setting variant). [...]
self.emit_instruction_with_width(
    Instruction::CSEL {
        rd,
        rn: rf,   // ← BUG: should be `rt` (true_val)
        rm: rt,   // ← BUG: should be `rf` (false_val)
        cond: crate::arm64::Condition::NE,
    },
    width,
)?;
```

CSEL semantics: `if cond then Rd = Rn else Rd = Rm`. With `cond = NE`
(`is_error != 0`):
- Buggy:  `if NE then Rd = Rn = rf (false_val=payload); else Rd = Rm = rt (true_val=-2)`.
- Correct: `if NE then Rd = Rn = rt (true_val=-2);       else Rd = Rm = rf (false_val=payload)`.

### Correct stack-slot-path Select emit (`emit.rs:5497-5506`)

```rust
// Load true and false values (LDR does NOT affect condition flags)
self.ss_load_value_with_width(true_val,  Register::X10, slots, width)?;
self.ss_load_value_with_width(false_val, Register::X17, slots, width)?;
// CSEL: if NE (cond != 0), X9 = X10 (true_val); else X9 = X17 (false_val)
self.emit_instruction_with_width(
    Instruction::CSEL {
        rd: Register::X9,
        rn: Register::X10,   // true_val   ← CORRECT
        rm: Register::X17,   // false_val  ← CORRECT
        cond: Condition::NE,
    },
    width,
)?;
```

### Disassembly evidence (dumped via `dump_ir` post-allocation)

Regalloc path (`VUMA_REAL_REGALLOC_AARCH64=1 dump_ir … aarch64`),
Select emission at the `is_error` check:

```
line 227: cmp x12, #0              # SUBS XZR, x12, #0 — sets flags on is_error
line 228: CSEL x13, x14, x9, NE    # 0x9A8911CD — Rd=x13 Rn=x14 Rm=x9 cond=NE
                                  #  x14 = false_val (payload=0)
                                  #  x9  = true_val  (-2)
                                  # if NE: x13 = Rn = x14 = 0  ← WRONG (should be -2)
```

Stack-slot path (default `dump_ir`), same Select:

```
line 244: CSEL x9, x10, x17, NE    # 0x9A911149 — Rd=x9 Rn=x10 Rm=x17 cond=NE
                                  #  x10 = true_val  (-2)
                                  #  x17 = false_val (payload=0)
                                  # if NE: x9 = Rn = x10 = -2  ← CORRECT
```

The two CSEL encodings differ only in the Rn/Rm register assignments
(bits 20..16 and 9..5), confirming the operand-swap bug.

### Execution trace (regalloc path, empty-pipe case)

| Step | IR pos | Instruction | x12 (v24/v26/v30/v2) | x13 (v29/v31/v4) | x27 (v28) | x9 | x14 (v27) |
|------|--------|-------------|----------------------|------------------|-----------|----|-----------|
| poll_ret captured | 66 | `mov x12, x0` | -14 | | | | |
| poll_no_data | 68 | `cset x27, le` | -14 | | 1 | | |
| read_ret captured | 72 | `mov x12, x0` | -11 | | 1 | | |
| payload loaded | 74 | `ldr x14, [x9]` | -11 | | 1 | | 0 |
| read_failed | 76 | `cset x13, ne` | -11 | 1 | 1 | | 0 |
| is_error | 78 | `and x12, x27, x13` | **1** | 1 | 1 | | 0 |
| true_val loaded | 80 | `movz x9, #65534; movk…` | 1 | 1 | 1 | **-2** | 0 |
| cmp is_error, #0 | 80 | `cmp x12, #0` | 1 | 1 | 1 | -2 | 0 |
| **Select (buggy)** | 80 | `CSEL x13, x14, x9, NE` | 1 | **0** ← false_val | 1 | -2 | 0 |
| v2 = v31 + 0 | 82 | `add x12, x13, #0` | **0** | 0 | 1 | -2 | 0 |
| Cmp Eq v2, -2 | 84 | `cmp x12, x9` | 0 | 0 | 1 | -2 | 0 |
| v4 = (v2 == -2) | 84 | `cset x13, eq` | 0 | **0** ← false | 1 | -2 | 0 |
| CondBranch | 86 | `cbnz x13, #16` | 0 | 0 | 1 | -2 | 0 |
| Return v2 | — | `mov x0, x12(?)` | **0** → exit 0 | | | | |

With the fix (swap rn/rm), the Select CSEL becomes `CSEL x13, x9, x14, NE`:
`if NE then x13 = x9 = -2` → `v2 = -2` → `Cmp Eq` true → `exit 77`. ✓

## 4. Why 29/30 regalloc tests pass despite this bug

Only `try_recv` lowers to `IRInstr::Select` with a runtime-variable `cond`
whose true-branch value differs from the false-branch value AND the
cond evaluates to non-zero at runtime. Other tests either:
- don't use `Select` at all (most arithmetic/control-flow tests), or
- use `Select` with `cond = 0` at runtime (where the swap selects the
  `else` branch — which is the same branch both buggy and fixed code
  would take when cond is 0, since the swap only inverts which branch
  is taken; if cond is always 0, both produce the false_val), or
- have `true_val == false_val` (swap is a no-op).

The `CtSelect` arm (`emit.rs:2256-2282`) has the **same swapped
pattern** (`rn: rf, rm: rt`), so any future test exercising `CtSelect`
with a non-zero cond would hit the same bug. The stack-slot path's
`CtSelect` (`emit.rs:5513-5546`) is correct.

## 5. Proposed Minimal Fix (for CB-b to apply)

Swap `rn` and `rm` in **both** the `Select` and `CtSelect` arms of
`emit_ir_instr` in `src/codegen/src/emit.rs`, to match the stack-slot
path's correct operand order.

### Diff (NOT applied — investigation only)

```diff
--- a/src/codegen/src/emit.rs
+++ b/src/codegen/src/emit.rs
@@ -2172,8 +2172,8 @@ impl AArch64Backend {
                 // Set flags by using a separate CMP (SUB with XZR destination
                 // doesn't set flags; we need a flags-setting variant).
                 // We emulate this with: CMP rc, #0 which is SUBS XZR, rc, #0.
                 // Since we only have SUB, we use the existing CMP pattern.
                 self.emit_instruction_with_width(
                     Instruction::CSEL {
                         rd,
-                        rn: rf,   // false_val  ← BUG
-                        rm: rt,   // true_val   ← BUG
+                        rn: rt,   // true_val   ← FIX
+                        rm: rf,   // false_val  ← FIX
                         cond: crate::arm64::Condition::NE,
                     },
                     width,
@@ -2273,8 +2273,8 @@ impl AArch64Backend {
                 // Use the same CSEL pattern as Select, which is constant-time
                 // on AArch64 (no branch).
                 self.emit_instruction_with_width(
                     Instruction::CSEL {
                         rd,
-                        rn: rf,   // false_val  ← BUG
-                        rm: rt,   // true_val   ← BUG
+                        rn: rt,   // true_val   ← FIX
+                        rm: rf,   // false_val  ← FIX
                         cond: crate::arm64::Condition::NE,
                     },
                     width,
```

### Verification plan (for CB-b)

After applying the fix:
1. Rebuild: `cargo build --release --bin compile_dump`
2. Re-run regalloc: `VUMA_REAL_REGALLOC_AARCH64=1 compile_dump …
   try_recv.vuma /tmp/fixed.bin aarch64 && qemu-aarch64-static
   /tmp/fixed.bin; echo "exit=$?"` → expect `exit=77`.
3. Re-run the 30-test matrix (CA-a-test driver) → expect 30/30 regalloc
   + 30/30 stack-slot (no regression).
4. Spot-check the disassembly: the Select CSEL should now decode as
   `CSEL x13, x9, x14, NE` (Rn=x9=true_val, Rm=x14=false_val) matching
   the stack-slot path's operand order.

## 6. Constraint check

- No source files edited (investigation only; `git status` will show
  only the new audit markdown).
- No `git push`.
- No sub-agents spawned.
- Time budget: ~11 minutes (env setup + reproduction + IR/asm dump +
  decode + report).

## 7. Status

- Root cause identified: `IRInstr::Select` (and `CtSelect`) CSEL operand
  swap in `emit_ir_instr` (`emit.rs:2175-2181` and `:2274-2280`).
- Proposed fix: swap `rn`/`rm` to match the stack-slot path
  (`emit.rs:5499-5504`).
- Ready for CB-b to apply the 2-hunk fix and re-verify.
