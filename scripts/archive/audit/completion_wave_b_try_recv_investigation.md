# try_recv Regalloc Fix — Investigation Status

## Summary

The try_recv regalloc edge case (exits 0 instead of expected 77) was
root-caused by CB-a-investigate (1ccefa6b) to a CSEL operand swap in
the regalloc path's Select/CtSelect lowering (emit.rs:2174-2182 and
2274-2280).

## Attempted Fix (CB-b-impl, reverted)

CB-b-impl (569659a9) swapped rn/rm in both Select and CtSelect arms:
- Before: `CSEL rd, rn=false_val, rm=true_val, NE`
- After:  `CSEL rd, rn=true_val, rm=false_val, NE`

This fixed try_recv (exit 0 → 77) but BROKE 17 other tests that were
previously passing (regalloc path dropped from 29/30 to 13/30).

## Root Cause of the Regression

The simple swap was too simplistic. The flag-setting instruction BEFORE
the CSEL differs between the regalloc path and the stack-slot path. The
regalloc path's Select lowering (emit.rs:2170-2182) has a comment saying
"CMP rc, #0" but the actual CMP emission is unclear — it may rely on
flags set by a prior instruction, or the cond flag (NE) may have different
semantics depending on what set the flags.

The stack-slot path's Select lowering (emit.rs:5499-5506) uses the same
`rn=true_val, rm=false_val, NE` pattern that CB-b-impl tried, but it
passes 30/30 because its flag-setting is correct.

## What's Needed

A proper fix requires:
1. Reading the FULL Select lowering in both paths (regalloc at emit.rs:2170,
   stack-slot at emit.rs:5499) to understand the flag-setting differences.
2. Ensuring the regalloc path emits an explicit CMP/SUBS to set flags
   correctly before the CSEL, matching the stack-slot path.
3. OR: fixing the cond flag (NE vs EQ) instead of swapping operands, if
   the flag-setting is already correct but the cond is inverted.

## Current State

- The CB-b-impl fix was reverted (1c0d343c).
- The regalloc path is back to 29/30 (try_recv is the 1 known edge case).
- VUMA_REAL_REGALLOC_AARCH64 env-var gate remains OFF by default.
- Production impact: ZERO (default path is stack-slot ISel).

## Recommendation

This fix requires deeper investigation of the flag-setting code before
CSEL in the regalloc path's Select lowering. It should be attempted by
a human developer with access to the full emit.rs context and a debugger.
The try_recv test is a narrow edge case (non-blocking recv on empty
channel returning EAGAIN); the 29/30 regalloc path is usable for all
other use cases via VUMA_REAL_REGALLOC_AARCH64=1.
