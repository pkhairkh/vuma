# VUMA Multi-Wave Worklog — All 19 Backends

This worklog tracks wave-by-wave progress on TASKS.md across ALL 19 backends:
x86_64, aarch64, riscv64, arm32, loongarch64, mips64, mips64be, ppc64, ppc64le,
riscv32, x86_32, sparc64, s390x, m68k, alpha, hppa, armeb, aarch64_be, wasm32.

Test matrix script: /home/z/my-project/scripts/vuma_test_matrix.sh

---
Task ID: Wave-baseline-audit
Agent: main (orchestrator)
Task: Audit current state of all 19 backends against the TASKS.md gold-standard suite.

Work Log:
- Cloned vuma repo at commit fae3bef3 (Wave 7: try_recv — nanosleep before read)
- Installed Rust nightly-2026-03-01 toolchain
- Installed QEMU user-static for all 19 architectures (Debian qemu-user 10.0.11)
- Built workspace with cargo build --workspace → success
- Ran gold-standard IPC test matrix across all 19 backends

Test Results (Tests passing on each backend out of those tested):
  Test                  | Failing backends
  ---------------------+-----------------------------------------
  simple_send           | (all 18 runnable pass)
  ping_pong             | (all 18 pass)
  multi_message         | riscv32 (hang/timeout)
  try_recv              | (all 18 pass)
  recv_timeout          | (all 18 pass)
  match_recv            | (all 18 pass)
  framed_send_recv     | (all 18 pass)
  capability_grant_verify | (all 18 pass)
  protocol_valid        | (all 18 pass)
  protocol_invalid      | (all 18 pass)
  shared_memory_rw      | (all 18 pass)
  memory_limit          | (all 18 pass)
  aead                  | (all 18 pass)
  checkpoint            | (all 18 pass)
  stark_proof           | m68k, hppa (F:0 expected 1)
  ffi_basic             | arm32, riscv32, x86_32 (hang - 32-bit timespec bug)
  ffi_isolation         | (all 18 pass)
  ffi_crash_recovery    | (all 18 pass)
  supervisor            | (all 18 pass)
  driver_isolation      | riscv32 (hang), hppa (segfault)
  fault_tolerance       | ppc64, ppc64le, sparc64, hppa (segfault), riscv32, m68k (F:1)
  hot_swap              | (all 18 pass)
  distributed           | arm32, hppa (F:0)
  sandbox               | (all 18 pass)
  resource_limit        | (all 18 pass)
  error_containment     | (all 18 pass)
  cap_flow              | (all 18 pass)
  cap_revoke            | (all 18 pass)
  delegation            | (all 18 pass)
  linear_valid          | (all 18 pass)
  infoflow_valid        | (all 18 pass)
  session_valid         | (all 18 pass)
  formal_verify         | (all 18 pass)

Stage Summary:
- 18/19 runnable backends pass the vast majority of the gold-standard suite
- wasm32 backend is not QEMU-testable (not a Linux binary)
- Identified 6 distinct bug clusters to fix in subsequent waves:
  * Wave A: 32-bit timespec bug (affects ffi_basic on 3 32-bit backends)
  * Wave B: stark_proof on m68k + hppa (FNV-1a hash mismatch)
  * Wave C: distributed on arm32 + hppa (socket error path)
  * Wave D: driver_isolation on riscv32 (hang) + hppa (segfault)
  * Wave E: fault_tolerance on ppc64/ppc64le/sparc64/hppa (segfault) + riscv32/m68k (F:1)
  * Wave F: multi_message on riscv32 (hang)

---
Task ID: Wave-A-32bit-timespec
Agent: main (orchestrator)
Task: Fix 32-bit timespec bug — ffi_basic hangs on arm32, riscv32, x86_32.

Work Log:
- Pre-state: `rg 'nanosleep|tv_nsec' src/codegen/src/ipc_lowering.rs` returned 10 matches across 2 call sites
- Identified root cause: `expand_channel_try_recv` and `expand_driver_call` (which backs process_call) both emitted nanosleep timespec as 2x I64 stores (offsets 0/8). On 32-bit Linux, struct timespec is { i32 tv_sec, i32 tv_nsec } (8 bytes, nsec at offset 4). The I64 stores corrupted the struct, causing nanosleep to return -EINVAL immediately and the subsequent channel_recv to race with the child's send → deadlock.
- Edited src/codegen/src/ipc_lowering.rs:
  * Added `is_32bit_backend(backend)` helper — true for Arm32, ArmEb, RiscV32, X86_32
  * Added `emit_nanosleep(ctx, nsec)` helper — emits 8-byte timespec (i32/i32) on 32-bit, 16-byte (i64/i64) on 64-bit
  * Replaced channel_try_recv nanosleep block with `emit_nanosleep(ctx, 10_000_000)`
  * Replaced driver_call/process_call nanosleep block with `emit_nanosleep(ctx, 1_000_000)`
- Build: PASS (cargo build --workspace, 0 errors)
- Regression: simple_send=42, ping_pong=84, multi_message=63, try_recv=77, recv_timeout=88 — all pass on all 18 runnable backends
- Wave test: ffi_basic.vuma → exit=42 on arm32, riscv32, x86_32 ✓ (was: timeout/hang)
- No regressions: ffi_isolation=42, ffi_crash_recovery=1 still pass on all 18 backends

Stage Summary:
- ffi_basic now passes on 18/18 runnable backends (was 15/18)
- The 32-bit timespec fix also unlocks driver_isolation (which uses expand_driver_call) on riscv32 — needs verification
- Next: Wave B (stark_proof on m68k/hppa), Wave C (distributed on arm32/hppa), Wave D (driver_isolation on hppa), Wave E (fault_tolerance segfaults)

---
Task ID: Wave-B-m68k-stark-proof
Agent: main (orchestrator)
Task: Fix stark_proof on m68k — FNV-1a hash mismatch.

Work Log:
- Pre-state: stark_proof.vuma → exit 0 on m68k (expected 1). FNV-1a hash mismatch.
- Root cause: The m68k I64 Mul used encoding 0x4C01 which is MULU.W (16×16→32),
  NOT MULU.L (32×32→64). Bit 8 of the MULU opcode distinguishes MULU.W (0)
  from MULU.L (1). The schoolbook 64×64→64 multiply was silently doing 16×16
  multiplies, producing garbage FNV-1a hashes.
- Attempted fix 1: Changed encoding to 0x4D01 (MULU.L, bit 8 = 1). This caused
  an Illegal Instruction signal because QEMU-m68k defaults to the m68000 CPU
  model, which does NOT support MULU.L (a 68020+ instruction).
- Final fix: Rewrote the m68k I64 Mul to use only MULU.W (68000-compatible):
  * Added emit_mulu32_to_64() helper — computes 32×32→64 using four MULU.W
    (16×16→32) multiplies with schoolbook 16-bit limbs.
  * The helper uses LINK A0 / UNLK A0 to allocate 24 bytes of scratch stack
    space (A0-relative), avoiding conflicts with FP-relative vreg stack slots.
  * The I64 Mul caller uses LINK A1 / UNLK A1 for its own scratch, calling
    emit_mulu32_to_64 three times for the three 32×32→64 partial products
    (a_lo*b_lo, a_lo*b_hi, a_hi*b_lo) and combining them into the 64-bit result.
  * Also fixed two MOVE.L register-copy encodings (0x2003/0x2002 were reversed;
    correct is 0x2600 for D0→D3, 0x2400 for D0→D2).
- Build: PASS (cargo build --workspace, 0 errors)
- Wave test: stark_proof.vuma → exit 1 on m68k ✓ (was: exit 0)
- No regressions: simple_send=42, ping_pong=84, multi_message=63, try_recv=77,
  recv_timeout=88, match_recv=42, framed_send_recv=42 all pass on m68k.

Stage Summary:
- stark_proof now passes on 17/18 runnable backends (m68k fixed; hppa remains).
- hppa uses a repeated-addition loop for Mul (O(n) where n = multiplier value),
  which is infeasible for FNV-1a's 0x100000001b3 prime (~10^12 iterations).
  hppa is BackendTier::Scaffolded — needs a real I64 Mul implementation.
- Next: Wave C (distributed on arm32/hppa), Wave D (driver_isolation on riscv32/hppa),
  Wave E (fault_tolerance segfaults on ppc64/ppc64le/sparc64/hppa).
