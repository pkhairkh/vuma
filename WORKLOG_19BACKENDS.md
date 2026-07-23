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
