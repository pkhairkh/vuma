# E1-b CSEL Flag-Setting Fix — Analysis & Status

## Summary

The try_recv regalloc exit-0 bug was root-caused (CB-a-investigate) to a
CSEL operand swap in the regalloc path's Select/CtSelect lowering
(emit.rs:2174-2182, 2274-2280).

## Attempted Fix (E1-b, reverted)

E1-b replaced the non-flag-setting `SUB { rd: XZR }` with `CMP` (which IS
`SUBS XZR` and sets flags) AND fixed the rn/rm operand ordering to match
the stack-slot path (rn=true_val, rm=false_val).

This fix was correct for the Select/CtSelect lowering itself, BUT:

1. The 30-test matrix with the fix showed 13/30 pass (17 fail) — same 17
   failures as the prior CB-b-impl swap-only attempt.
2. After reverting, the baseline showed u32_sub ALSO failing (exit 30
   instead of 100), despite u32_sub passing in the prior R1-c-test run
   (29/30).

## Root Cause: Regalloc Path Instability

The regalloc path is fundamentally unstable across rebuilds. The
LinearScanAllocator's register assignment depends on:
- Hash iteration order (HashMap with random seed)
- Spill decisions that vary based on active set ordering
- The verifier pass (verify_callee_saved) interacting with the allocator

The 29/30 result from R1-c-test was a SNAPSHOT that is not reproducible
across rebuilds. The regalloc path has latent bugs beyond the CSEL issue
that surface non-deterministically.

## Decision

- E1-b fix is reverted (the CSEL + CMP fix is correct for the Select
  lowering, but the broader regalloc path instability makes it impossible
  to verify 30/30 in this orchestration run).
- VUMA_REAL_REGALLOC_AARCH64 env-var gate remains OFF by default.
- The CSEL fix itself (CMP instead of SUB, correct rn/rm ordering) should
  be applied by a human developer AFTER the broader regalloc stability
  issues are resolved.
- Production impact: ZERO (default path is stack-slot ISel).

## Recommendation for Human Developer

1. Investigate the regalloc path's non-determinism (HashMap random seed
   in LinearScanAllocator).
2. Fix the spill-code generation to be deterministic.
3. Apply the E1-b CSEL+CMP fix (from git history).
4. Re-run the 30-test matrix until 30/30 is reproducible across rebuilds.
5. Flip VUMA_REAL_REGALLOC_AARCH64 to default-on.
