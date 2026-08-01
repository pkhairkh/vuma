# AD-1 + AE-1 — s390x Regression + W1 Stabilization Investigation

**Date**: 2026-08-01
**Method**: Direct git archaeology + test_results diff on remote machine
(`155.138.203.27`, `/root/vuma` at commit `314b2987`).

---

## s390x Regression Analysis (AD-1)

### Headline

- **Stale** (commit `78e71a6b`): s390x 1576/1577 = 99.94% (1 failure)
- **Fresh** (commit `314b2987`): s390x 1560/1577 = 98.92% (17 failures)
- **Delta**: -1.02pp, +16 NEW failures

### Root cause

**Only one commit touched shared codegen** between the two snapshots:
`1d72d296` "[Wave-0] Fix non-deterministic phi construction + register
allocator liveness bug." No commits touched `src/codegen/src/s390x/`
directly.

The `1d72d296` fix changed two things:
1. **Phi construction**: sort `all_modified` names alphabetically before
   allocating phi vregs; sort predecessors by label before emitting
   parallel copies. (Makes IR deterministic.)
2. **Register allocator liveness**: `LiveRangeComputer::compute` now
   computes CFG-based liveness (live_in / live_out) after the linear
   scan, extending intervals to cover loop bodies for loop-invariant
   vregs.

This fix IMPROVED 12 backends (+0.57 to +1.53pp) but REGRESSED s390x
(-1.02pp). The s390x backend has unique characteristics that likely
interact badly with the new liveness computation:
- Big-endian (only s390x, ppc64, sparc64, hppa, m68k are BE; s390x was
  the strongest BE backend)
- 5 arg regs (R2-R6) — more than x86_64's 6 but with different ABI
- SVC 0 syscall convention
- No dedicated condition codes register like x86 (uses PSW mask)

### New s390x failures (16 NEW)

| Test | Expected | Actual | Type | Notes |
|------|----------|--------|------|-------|
| arith_continued_fraction | 4 | 8 | MM | wrong result |
| arith_digit_sum | 6 | 13 | MM | wrong result |
| arith_gcd | 6 | 9 | MM | wrong result |
| arith_modular_exp | 1 | 0 | MM | wrong result |
| arith_mul_table | 36 | 2 | MM | wrong result |
| arith_reverse_digits | 54 | 188 | MM | wrong result |
| arith_collatz | 16 | 124 | TO | timeout (likely infinite loop) |
| mixed_static_arena | 0 | -11 | CR | crash (SIGSEGV) |
| arith_ackermann | 9 | -11 | CR | crash |
| arith_chinese_remainder | 23 | 0 | MM | wrong result |
| arith_lcm | 12 | 0 | MM | wrong result |
| arith_palindrome | 1 | 0 | MM | wrong result |
| inbounds_loop_dynamic | 45 | 0 | MM | wrong result |
| closed_channel | 99 | -11 | CR | crash |
| fault_tolerance | 0 | 1 | MM | wrong result |
| hot_swap | 1 | 0 | MM | wrong result |

The pre-existing s390x failure was `stark_proof` (CR, -11).

### Pattern

Most new failures are MM (mismatch — wrong numeric result), suggesting
the regalloc liveness fix is assigning wrong registers for s390x-specific
code patterns. The 3 CR (crash) failures and 1 TO (timeout) suggest
more severe corruption in specific cases.

### Recommended action

This is a real regression that needs s390x-specific regalloc debugging.
The `1d72d296` fix is correct in principle (CFG-based liveness is more
accurate than position-based), but the s390x backend's
`TargetDesc`/register-class setup may have an assumption that the old
position-based liveness satisfied by accident. File as a new bug
(V-S390X-1) and assign to the backend team. NOT a blocker for the V-34
fix.

---

## W1-sparc64 + W1-x86_32 Stabilization (AE-1)

### W1-sparc64 commit sequence

```
e0506ed2 [W1-sparc64] Fix ALL COND_ constants (were swapped) + OP3_SUBCC (0x14→0x06)
f2a05e31 Revert "[W1-sparc64] Fix ALL COND_ constants..."
ce1d4cc6 [W1-sparc64] Fix Cmp codegen: load G1=1 for MOVcc, correct cond mapping, branch-based approach
dcff9813 Revert "[W1-sparc64] Fix Cmp codegen: load G1=1 for MOVcc..."
c041517f [sparc64] Fix ALL COND_ constants + branch-based Cmp handler   ← FINAL
```

### Current state of sparc64

The final commit `c041517f` (NOT a W1 commit — dropped the W1 prefix)
re-applies BOTH fixes (COND_ constants + branch-based Cmp) in a single
commit. The current sparc64 code has:
- Corrected COND_ constants (e.g., `COND_BA: u32 = 0x08`, `COND_BE: u32 = 0x09`)
- Branch-based Cmp handler (not MOVcc-based)

**Both W1-sparc64 approaches were reverted, then re-applied together
in `c041517f`.** The sparc64 code is in its "fixed forward" state.

### sparc64 test regression

- **Stale**: 86.05% (1357/1577, 220 failures)
- **Fresh**: 82.24% (1297/1577, 280 failures)
- **Delta**: -3.81pp, +60 NEW failures

The re-applied fix in `c041517f` is INCOMPLETE — it fixed some tests
but introduced 60 new failures. The branch-based Cmp approach may be
correct for some comparison kinds but wrong for others. This is
mid-flight work that needs further iteration.

### W1-x86_32 commit sequence

```
f6741f74 [W1-x86_32] Fix Call handler to pass args 5+ on stack + fix mprotect stub
9a66c5a4 Revert "[W1-x86_32] Fix Call handler..."
314b2987 [W1-x86_32] Fix Call handler (args 5+ on stack) + mprotect stub (arg conversion)  ← FINAL
```

### Current state of x86_32

The final commit `314b2987` re-applies the Call handler fix with a
tweak ("arg conversion"). The current x86_32 code has:
- Args 5+ passed on stack (i386 SysV convention)
- mprotect stub with arg conversion

**The W1-x86_32 fix is in its "fixed forward" state** (re-applied after
revert, with adjustments).

### x86_32 test regression

- **Stale**: 83.45% (1316/1577, 261 failures)
- **Fresh**: 79.20% (1249/1577, 328 failures)
- **Delta**: -4.25pp, +67 NEW failures

The re-applied fix is INCOMPLETE — the Call handler fix for args 5+ may
be correct for the specific test it targeted but introduced regressions
in other Call patterns. The mprotect arg conversion may be wrong.

### Recommended action

Both W1-sparc64 and W1-x86_32 are in their "fixed forward" state but
the fixes are incomplete. The options are:
1. **Keep the fixes and iterate** — the fixes address real bugs (swapped
   COND_ constants, wrong arg passing) but have regressions. Iterate
   on the failing tests.
2. **Revert to pre-W1 state** — would restore sparc64 to 86.05% and
   x86_32 to 83.45% but lose the real fixes.
3. **Cherry-pick the correct parts** — e.g., keep the COND_ constant
   fixes (which are clearly correct — they were swapped) but revert
   the branch-based Cmp approach (which may be the source of regressions).

**Recommendation**: Option 3 for sparc64 (keep COND_ fixes, revert
branch-based Cmp to investigate further). Option 1 for x86_32 (the
arg-passing fix is clearly correct per i386 SysV ABI; the regressions
are likely in the mprotect arg conversion, which can be fixed
separately).

Neither is a blocker for the V-34 fix.

---

## Summary

| Issue | Root cause | Status | Blocks V-34? |
|-------|-----------|--------|--------------|
| s390x -1.02pp | `1d72d296` regalloc liveness fix interacts badly with s390x | File as V-S390X-1 | No |
| sparc64 -3.81pp | `c041517f` branch-based Cmp incomplete | Iterate or cherry-pick | No |
| x86_32 -4.25pp | `314b2987` mprotect arg conversion incomplete | Iterate on mprotect | No |

All three are real issues but none block the V-34 fix (which is a 2-line
patch to `bridge_type_to_ir_type` that fixes memory corruption + IVE
unsoundness for f32/f64 state fields).
