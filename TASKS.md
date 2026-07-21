# VUMA Process Isolation Architecture — Wave Implementation Plan

> **Source specification:** `docs/VUMA_PROCESS_ISOLATION_SPEC.md` v2.0
> **Total waves:** 96, organised into 10 phases.
> **This document is the single source of truth for what "done" means.**

---

## 0. Conventions (apply to EVERY subtask unless overridden)

The following rules are stated once here and are binding on every subtask in
every wave.  Individual subtask prompts do **not** repeat them.

### 0.1 Environment

| Item | Value |
|------|-------|
| Repo root | `/home/z/vuma-review` |
| Rust toolchain | `rust-toolchain.toml` pins `nightly-2026-03-01`; activate with `. "$HOME/.cargo/env"` |
| Build | `cargo build --workspace` from repo root — must succeed with zero errors after every subtask |
| Compiler binary | `./target/debug/compile_dump <input.vuma> <output.bin> <backend>` |
| QEMU static binaries | `$HOME/.local/bin/qemu-{aarch64,riscv64,arm,loongarch64}-static` |
| Worklog | `/home/z/my-project/worklog.md` — **READ FIRST**, then **APPEND** (never overwrite) using the template in §0.6 |

### 0.2 The five backends that matter

Every feature that emits machine code MUST be implemented on **all five** of
these backends unless the wave's "Target backends" line explicitly narrows it.

| Backend | Source dir | QEMU invocation | Syscall ABI |
|---------|-----------|-----------------|-------------|
| **x86_64** | `src/codegen/src/x86_64/` | native (no QEMU) | nr in RAX, args in RDI/RSI/RDX/R10/R8/R9, return in RAX |
| **aarch64** | `src/codegen/src/arm64.rs` + `src/codegen/src/emit.rs` | `qemu-aarch64-static` | nr in X8, args in X0–X5, return in X0 (asm-generic numbers) |
| **riscv64** | `src/codegen/src/riscv64.rs` | `qemu-riscv64-static` | nr in a7, args in a0–a5, return in a0 (asm-generic) |
| **arm32** | `src/codegen/src/arm32/mod.rs` | `qemu-arm-static` | nr in r7, args in r0–r3 (+stack), return in r0 (ARM EABI) |
| **loongarch64** | `src/codegen/src/loongarch64/` | `qemu-loongarch64-static` | nr in a7, args in a0–a5, return in a0 (asm-generic) |

Other backends in `src/codegen/src/` (alpha, hppa, m68k, mips64, ppc64,
ppc64le, riscv32, s390x, sparc64, wasm32, x86_32) are **stubs** — do NOT
touch them unless a wave explicitly says so.

### 0.3 The two IPC codegen paths (CRITICAL)

The compiler has **two** paths from IPC builtin calls to machine code:

1. **x86_64 inline path** — `src/codegen/src/x86_64/stack_slot_isel.rs`
   recognises `"channel_send"` / `"channel_recv"` / etc. by name in the
   Call-form instruction selector and emits inline x86_64 machine code
   directly.  This path has real CRC32, type_hash, capability, and
   protocol-state verification.

2. **Non-x86_64 IR-lowering path** — `src/pipeline.rs` calls
   `vuma_codegen::ipc_lowering::lower_ipc_builtins()` for every backend
   **except** x86_64.  This pass rewrites IPC `Call` instructions into
   generic `Syscall` / `Store` / `Load` / `BinOp` IR **before** the
   backend's instruction selector runs.  The backend then emits that
   generic IR.

**Anti-stub rule (NON-NEGOTIABLE):** The non-x86_64 path
(`src/codegen/src/ipc_lowering.rs`) MUST NOT contain stubs.  Specifically:
- `return 0` / `return 1` / `return 2` constant returns are forbidden for
  any builtin whose x86_64 counterpart does real work (circuit_breaker,
  formal_verify, stark_prove, etc.).
- `// TODO: real CRC32 loop`, `// simplified`, `// placeholder` are
  forbidden in shipped code paths.  If a feature is genuinely deferred,
  it MUST be marked `#[cfg(feature = "stub-ipc")]` and the stub-ipc
  feature MUST NOT be in the default build.
- The inline codegen that backends (aarch64/riscv64/arm32) have in their
  own instruction selectors is **dead code** if `ipc_lowering` rewrites
  the Call first.  Either (a) make `ipc_lowering` emit real IR for the
  feature, or (b) skip `ipc_lowering` for that builtin and let the
  backend's inline handler run.  Do NOT leave both paths half-implemented.

### 0.4 Process rules

- **Max 4 subagents launched simultaneously per wave.** If a wave has
  more than 4 subtasks, split into batches of 4.
- **Independent code domains.** Subtasks in the same batch MUST NOT edit
  the same file.
- **Git protocol.** Main agent: pull → dispatch wave → verify DoD →
  commit → push.  Subagents: edit files + append worklog only.  NEVER
  run git.
- **Kernel exclusion.** Subagents MUST NOT touch `womb/kernel/**`.
- **Build gate.** `cargo build --workspace` MUST succeed after every wave.
- **Test gate.** The gold-standard regression suite
  (`simple_send`=42, `ping_pong`=84, `multi_message`=63, `try_recv`=77,
  `recv_timeout`=88) MUST still pass after every wave.
- **One commit per wave.** Commit message: `Wave N: <summary> — <files>`.

### 0.5 Anti-fraud rules

- **No marker tests.** A `.vuma` test for feature X MUST call a builtin or
  syntax construct added FOR feature X.  If `grep -v '^//' test.vuma |
  grep -v '^$'` produces the same body as `simple_send.vuma`, the test is
  fake.  Verify with: `grep -v '^//' <test>.vuma | grep -v '^$' | md5sum`
  must NOT equal `ec6eb67ebb89132ebe877b0fa017dbb7`.
- **No library-only wiring.** If a function exists in `ipc.rs` /
  `capability.rs` / `verification.rs` / `borrow_region.rs` with passing
  unit tests but is never **called from `src/pipeline.rs` or
  `src/codegen/src/scg_to_ir.rs`**, the wave is NOT done.  Library code
  with no pipeline call-site is Failure Mode A.
- **No `use`-import-as-wiring.** A `use crate::capability::CapabilityToken;`
  import or a `// Wave 14b` comment does NOT count as "wired."  The
  acceptance criteria check for **emitted machine code** or **parser/IR
  constructs**, not symbol mentions.

### 0.6 Worklog template (append after every subtask)

```markdown
---
Task ID: Wave <N><subtask>
Agent: <agent name>
Task: <one-line summary>

Work Log:
- Pre-state: `rg '<symbol>' <file>` returned <N> matches
- Edited <file>: <what changed>
- Created <test>.vuma: <what feature it exercises>
- Anti-cheat: `grep -v '^//' <test>.vuma | grep -v '^$' | md5sum` = <md5>
- Build: PASS
- Regression: simple_send=42 ✓, ping_pong=84 ✓
- Wave test: <test>.vuma → exit=<expected> ✓ on backends: x86_64, aarch64, riscv64, arm32
- Commit: <sha> "Wave <N><subtask>: <summary>"

Stage Summary:
- <what's now wired end-to-end>
- <what remains>
```

---

## 1. Wave Map

| Waves | Phase | Spec Parts | Title | Target backends |
|-------|-------|------------|-------|-----------------|
| 1–4 | 1 | II, VIII | Channel<T> type + IR ops + x86_64 + cross-backend | all 5 |
| 5–8 | 1 | II, VIII | spawn_worker + IPC pipe + lifecycle + deadlock/error | all 5 |
| 9–16 | 1 | III (L1–L4) | Runtime encapsulation: framing, caps, shm, protocol FSM, cross-backend, tests | all 5 |
| 17–24 | 1 | III (L5–L8) | Sandbox, resource limits, checkpoint, error containment, AEAD | all 5 |
| 25–32 | 2 | X | FFI process isolation: `extern "process"`, marshal, seccomp, crash recovery | all 5 |
| 33–40 | 4 | IX | Capability system: delegation chain, flow verification, revocation | all 5 |
| 41–48 | 3 | XI | Kernel/user split: microkernel, syscall-as-IPC, resource accounting | all 5 |
| 49–64 | 5+2 | XII, XIII | Driver isolation + sandboxing (zero-cap workers, plugins, sandboxed crypto) | all 5 |
| 65–72 | 2 | XIV | Fault tolerance: supervisor, crash detection, checkpoint, circuit breaker | all 5 |
| 73–80 | 2 | XV | Hot reloading: hot-swap protocol, state transfer, version mgmt, rollback | all 5 |
| 81–88 | 6 | XVI | Distributed channels: location-transparent, discovery, network, consensus | all 5 |
| 89–92 | 7 | IV (CT1–CT2) | Compile-time: session types + information-flow types | n/a (compile-time) |
| 93–94 | 8 | IV (CT6), VI | Compile-time: zk-STARK attestation | all 5 (runtime proof) |
| 95 | 9 | IV (CT3–CT8) | Compile-time: refinement, linear, homomorphic, CSL-Perm, Noise | n/a (compile-time) |
| 96 | 10 | V | Formal verification: L1–L3 + 5→3 invariant collapse | n/a (compile-time) |

---

# Phase 1 — IPC Primitives & Runtime Encapsulation

## Wave 1: Channel<T> Type — IR + Type System

**Spec refs:** §7, §8, §45, §101 · **Target backends:** all 5 (type system is backend-independent)

### 1a — Add Channel<T> to ScgType and IRType
**Files:** `src/codegen/src/scg_to_ir.rs`, `src/codegen/src/ir.rs`

1. Add `Channel(Box<ScgType>)` variant to `ScgType` (~line 170)
2. Add `Channel(Box<IRType>)` variant to `IRType` (~line 50)
3. `ScgType::to_ir_type()`: map `Channel(inner)` → `IRType::Channel(Box::new(inner.to_ir_type()))`
4. `ScgType::size()` / `IRType::size()`: pointer-sized (8 on 64-bit, 4 on 32-bit)
5. `Display` impl: `"Channel<{}>"`
6. `opt.rs substitute_value`: pass through. `has_side_effects`: no.

### 1b — Add Channel type to parser AST + lexer
**Files:** `src/parser/src/ast.rs`, `src/parser/src/parser.rs`, `src/parser/src/lexer.rs`

1. `TokenKind::Channel` (keyword `Channel`)
2. `Type::Channel(Box<Type>)` in AST
3. `parse_type()`: after BDBase/Ptr/Array, check for `Channel` keyword → parse `Channel<T>`
4. `Display` for `Type::Channel`
5. Bridge in `pipeline.rs`: `bridge_type_to_codegen_scg`, `bridge_type_to_ir_type`, `type_size`, `type_alignment`

### 1c — Bridge Channel type through pipeline
**Files:** `src/pipeline.rs`

1. `bridge_type_to_codegen_scg`: `Type::Channel(inner)` → `ScgType::Channel(Box::new(bridge(inner)))`
2. `bridge_type_to_ir_type`: same mapping
3. `type_size` / `type_alignment`: pointer size
4. `flatten_expr` / `bridge_stmt_to_scg`: Channel variables are opaque vregs

### 1d — Add Channel<T> to LSP
**Files:** `src/lsp/mod.rs`

1. `format_type`: `Type::Channel(inner)` → `Channel<{}>`
2. Hover text: "Typed IPC channel for inter-process communication"
3. Completion for `Channel` keyword
4. Test: `test_format_type_channel`

**DoD (Wave 1):**
- [ ] `cargo build --workspace` succeeds
- [ ] `Channel<T>` exists in ScgType, IRType, parser AST, pipeline bridge
- [ ] `let ch: Channel<i32>` compiles
- [ ] LSP shows Channel type info
- [ ] Gold-standard tests still pass

---

## Wave 2: Channel<T> — IR Instructions for Channel Operations

**Spec refs:** §7, §8, §45 · **Target backends:** all 5 (IR is backend-independent)

### 2a — Add ChannelOpen/Send/Recv/Close IR instructions
**Files:** `src/codegen/src/scg_to_ir.rs`, `src/codegen/src/ir.rs`

1. `IRInstr::ChannelOpen { dst, elem_ty }`, `ChannelSend { ch, msg, ty }`, `ChannelRecv { ch, dst, ty }`, `ChannelClose { ch }`
2. `defined_regs` / `used_regs` / `Display` impls
3. Add `ChannelOpenStmt` / `ChannelSendStmt` / `ChannelRecvStmt` / `ChannelCloseStmt` to SCG
4. `ScgStatement` enum variants + dispatch in `lower_statements`

### 2b — Lower channel SCG statements to IR
**Files:** `src/codegen/src/scg_to_ir.rs`

1. `lower_channel_open`: allocate dst vreg, emit `ChannelOpen`
2. `lower_channel_send`: resolve ch + msg, emit `ChannelSend`
3. `lower_channel_recv`: allocate dst vreg, emit `ChannelRecv`
4. `lower_channel_close`: resolve ch, emit `ChannelClose`
5. Def/use analysis for each

### 2c — Parse channel builtins as Call expressions
**Files:** `src/parser/src/parser.rs`, `src/pipeline.rs`

1. `channel_open<T>(args)` parses as `Expr::Call` with type parameter (intercept `<` before postfix loop)
2. `channel_send(ch, msg)`, `channel_recv(ch)`, `channel_close(ch)` parse as plain `Expr::Call`
3. Pipeline: `CallNode { func: "channel_open", ... }` → SCG `ChannelOpen` statement

### 2d — IR + SCG unit tests
**Files:** `src/codegen/src/scg_to_ir.rs` (test module)

1. Round-trip: parse `channel_open<i32>()` → SCG → IR, verify `IRInstr::ChannelOpen` present
2. Verify `defined_regs` / `used_regs` correct for each channel instruction

**DoD (Wave 2):**
- [ ] `cargo build --workspace` succeeds
- [ ] 4 channel IR instructions exist with correct def/use
- [ ] `channel_open<i32>()` parses and lowers to `IRInstr::ChannelOpen`
- [ ] Gold-standard tests still pass

---

## Wave 3: Channel<T> — x86_64 Backend Implementation

**Spec refs:** §7, §8, §45 · **Target backends:** x86_64

### 3a — Implement ChannelOpen on x86_64 (pipe-based)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

1. `pipe2(fds, 0)` syscall (nr 293) — allocates read_fd + write_fd
2. Pack into 64-bit handle: `(read_fd as u64) | ((write_fd as u64) << 32)`
3. Store handle in dst's stack slot
4. Return the handle

### 3b — Implement ChannelSend/Recv/Close on x86_64
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

1. `ChannelSend`: load write_fd (high 32 bits), `write(fd, &msg, 8)` syscall (nr 1)
2. `ChannelRecv`: load read_fd (low 32 bits), `read(fd, &dst, 8)` syscall (nr 0)
3. `ChannelClose`: `close(read_fd)` + `close(write_fd)` (nr 3)
4. Handle both `IRInstr::ChannelSend` and Call-form `"channel_send"` in the isel

### 3c — Integration test: simple_send.vuma
**Files:** `tests/gold_standard/ipc/simple_send.vuma`

1. Parent opens channel, forks child, sends 42, child recvs and exits 42
2. `// Expected exit code: 42`

### 3d — Integration test: ping_pong.vuma
**Files:** `tests/gold_standard/ipc/ping_pong.vuma`

1. Bidirectional: parent sends 42, child recvs, adds 42, sends 84 back, parent recvs 84
2. `// Expected exit code: 84`

**DoD (Wave 3):**
- [ ] `simple_send.vuma` → exit 42 on x86_64
- [ ] `ping_pong.vuma` → exit 84 on x86_64
- [ ] `cargo build --workspace` succeeds

---

## Wave 4: Channel<T> — Cross-Backend Support

**Spec refs:** §7, §8, §45 · **Target backends:** aarch64, riscv64, arm32, loongarch64

### 4a — Channel ops on aarch64
**Files:** `src/codegen/src/arm64.rs` (or `src/codegen/src/emit.rs`)

1. `pipe2` (nr 59), `write` (nr 64), `read` (nr 63), `close` (nr 57) — asm-generic numbers
2. Pack/unpack 64-bit handle the same way as x86_64
3. Handle in `IRInstr::Channel{Open,Send,Recv,Close}` arms of the aarch64 isel

### 4b — Channel ops on riscv64
**Files:** `src/codegen/src/riscv64.rs`

1. Same syscalls, asm-generic numbers (`pipe2`=59, `write`=64, `read`=63, `close`=57)
2. nr in a7, args in a0–a5

### 4c — Channel ops on arm32
**Files:** `src/codegen/src/arm32/mod.rs`

1. ARM EABI: nr in r7, args in r0–r3 (+ stack)
2. `pipe2`=59, `write`=64, `read`=63, `close`=57 (asm-generic via `syscall_abi::translate`)

### 4d — Channel ops on loongarch64
**Files:** `src/codegen/src/loongarch64/stack_slot_isel.rs`

1. nr in a7, args in a0–a5 (asm-generic, identity translation)
2. Same syscall numbers as riscv64

**DoD (Wave 4):**
- [ ] `simple_send.vuma` → exit 42 on **all 4** non-x86_64 backends via QEMU
- [ ] `ping_pong.vuma` → exit 84 on all 4 non-x86_64 backends
- [ ] `cargo build --workspace` succeeds

---

## Wave 5: spawn_worker — Process Spawning

**Spec refs:** §8, §45 · **Target backends:** all 5

### 5a — Add spawn_worker builtin to parser + SCG + IR
**Files:** `src/parser/src/parser.rs`, `src/codegen/src/scg_to_ir.rs`, `src/codegen/src/ir.rs`

1. `spawn_worker()` parses as `Expr::Call`
2. Lower to `IRInstr::Call { func: "spawn_worker", ... }` (builtin recognised by name in isel)

### 5b — Implement spawn_worker on x86_64 (fork)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

1. `clone` syscall (nr 56) with SIGCHLD flag — returns child PID to parent, 0 to child
2. Store result in dst

### 5c — Implement wait_worker + kill_worker on x86_64
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

1. `wait_worker(pid)`: `wait4(pid, &status, 0, NULL)` (nr 61), extract `WEXITSTATUS(status)` = `(status >> 8) & 0xFF`
2. `kill_worker(pid)`: `kill(pid, SIGKILL=9)` (nr 62)

### 5d — Port spawn/wait/kill to aarch64 + riscv64 + arm32 + loongarch64
**Files:** all 4 backend isel files

1. `clone`=220 (asm-generic), `wait4`=260, `kill`=129
2. Same packing/extraction logic, different register conventions

**DoD (Wave 5):**
- [ ] `spawn_worker()` returns 0 in child, PID in parent on all 5 backends
- [ ] `wait_worker(pid)` returns child's exit code on all 5 backends
- [ ] `simple_send.vuma` (which uses spawn+wait) passes on all 5

---

## Wave 6: IPC Channel — Process-to-Process Communication

**Spec refs:** §8, §45 · **Target backends:** all 5

### 6a — Pass channel handle to child process
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

1. After `spawn_worker`, child inherits the pipe fds (fork semantics — fds are duplicated)
2. Child reads from read_fd, parent writes to write_fd (or vice versa)

### 6b — Multi-message test
**Files:** `tests/gold_standard/ipc/multi_message.vuma`

1. Send 3 messages (10, 20, 30), recv 3 in order, exit with sum (60+3=63)
2. `// Expected exit code: 63`

### 6c — Bidirectional channel test
**Files:** `tests/gold_standard/ipc/bidirectional.vuma`

1. Two channels: parent→child and child→parent
2. Parent sends 42, child recvs, adds 42, sends 84 back
3. `// Expected exit code: 84`

### 6d — Port to all 5 backends
**Files:** all backend isel files

1. Verify handle inheritance works on all 5 backends (fork/clone semantics)

**DoD (Wave 6):**
- [ ] `multi_message.vuma` → exit 63 on all 5 backends
- [ ] `bidirectional.vuma` → exit 84 on all 5 backends

---

## Wave 7: IPC Channel — Remaining Backends + Lifecycle

**Spec refs:** §8, §45 · **Target backends:** all 5

### 7a–7d — Channel lifecycle: try_recv, recv_timeout, close-channel detection
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ir.rs`, all backend isels

1. `channel_try_recv(ch)`: `recvfrom(fd, buf, len, MSG_DONTWAIT, NULL, NULL)` — non-blocking, returns 0 on empty
2. `channel_recv_timeout(ch, timeout_ms)`: `poll({fd, POLLIN}, 1, timeout_ms)` then `read` if ready — returns -2 on timeout
3. `channel_is_closed(ch)`: `poll({fd, POLLHUP}, 1, 0)` — returns 1 if peer closed
4. Port all three to aarch64, riscv64, arm32, loongarch64

**DoD (Wave 7):**
- [ ] `try_recv.vuma` → exit 77 on all 5 backends
- [ ] `recv_timeout.vuma` → exit 88 on all 5 backends
- [ ] `closed_channel.vuma` passes on all 5 backends

---

## Wave 8: Deadlock Detection + Channel Error Handling

**Spec refs:** §49, §50, §18 · **Target backends:** all 5

### 8a — Compile-time deadlock detection (wait-for graph)
**Files:** `src/codegen/src/opt.rs`

1. New pass `detect_deadlock()`: build wait-for graph (A blocks on ChannelRecv from C, sender of C is B → edge A→B)
2. Detect cycles via DFS
3. Emit warning (not error) on potential deadlock

### 8b — ChannelError enum + match Ok/Err syntax
**Files:** `src/codegen/src/ir.rs`, `src/parser/src/ast.rs`, `src/parser/src/parser.rs`, `src/pipeline.rs`, `src/codegen/src/scg_to_ir.rs`

1. `ChannelError` enum in `ir.rs`: `Closed=1, Timeout=2, PermissionDenied=3, InvalidHandle=4, CrcMismatch=5, ProtocolViolation=6`
2. `IRInstr::ChannelRecvResult { ch, dst, err_dst, ty }` — writes both payload and error discriminant
3. Parser: `validate_match_channel_recv_ok_err()` — validates `match channel_recv(ch) { Ok(v) => ..., Err(e) => ... }` form (exactly 2 arms, both bindings, no guards)
4. Pipeline: `try_match_channel_recv_result()` — detects the pattern, emits `ScgStatement::ChannelRecvResult` + `ControlNode::If` on `err_dst == 0`
5. `lower_channel_recv_result()` in scg_to_ir: allocates 2 vregs, emits `IRInstr::ChannelRecvResult`

### 8c — Channel timeout support
**Files:** `src/codegen/src/ir.rs`, `src/codegen/src/x86_64/stack_slot_isel.rs`

1. `IRInstr::ChannelRecvTimeout { ch, dst, ty, timeout_ms }`
2. x86_64: `poll()` with timeout, return -2 on timeout

### 8d — Channel integration tests
**Files:** `tests/gold_standard/ipc/`

1. `try_recv.vuma` (exit 77), `recv_timeout.vuma` (exit 88), `multi_message.vuma` (exit 63), `large_message.vuma` (exit 1), `match_recv.vuma` (exit 42)

**DoD (Wave 8):**
- [ ] `match channel_recv(ch) { Ok(v) => ..., Err(e) => ... }` compiles and runs on all 5 backends
- [ ] `match_recv.vuma` → exit 42 on all 5 backends
- [ ] Deadlock detection emits warning on circular waits
- [ ] `rg 'enum ChannelError' src/codegen/src/ir.rs` ≥1 match
- [ ] `rg 'ChannelRecvResult' src/codegen/src/ir.rs` ≥1 match

---

## Wave 9: Runtime Encapsulation L1 — Message Wire Format

**Spec refs:** §12.1–§12.3 · **Target backends:** n/a (library module, but consumed by all 5)

### 9a — Create IPC module with wire format
**Files:** `src/codegen/src/ipc.rs` (new), `src/codegen/src/lib.rs`

1. `MessageHeader`: `magic [u8;4]`, `version u16`, `flags u16`, `channel_id u64`, `sequence u64`, `type_hash u64`, `payload_len u64`, `cap_count u32`
2. `MAGIC=[0x56,0x55,0x4D,0x41]`, `VERSION=2`, `HEADER_SIZE=44`
3. `MessageFlags` bitfield: ENCRYPTED, HAS_CAPS, HAS_SHM, IS_REPLY, IS_ERROR
4. `EncapsulatedMessage` struct

### 9b — Implement frame_message and deframe_message
**Files:** `src/codegen/src/ipc.rs`

1. `frame_message(msg) -> Vec<u8>`: header LE, payload, capabilities, CRC32
2. `deframe_message(stream) -> Result<EncapsulatedMessage, FrameError>`: verify magic, version, CRC32
3. `FrameError`: BadMagic, UnsupportedVersion, PayloadTooLarge, CrcMismatch, TruncatedMessage

### 9c — Implement CRC32 and type hash
**Files:** `src/codegen/src/ipc.rs`

1. `crc32(data) -> u32` — IEEE 802.3 polynomial `0xEDB88320`
2. `type_hash(ty) -> u64` — FNV-1a 64-bit
3. `canonical_type_string` for all ScgType variants

### 9d — Unit tests for wire format
**Files:** `src/codegen/src/ipc.rs` (test module)

1. Round-trip: frame → deframe → verify all fields
2. CRC mismatch detection
3. Type hash determinism (same type → same hash)

**DoD (Wave 9):**
- [ ] `cargo build --workspace` succeeds
- [ ] `ipc.rs` module with wire format + framing + CRC32 exists
- [ ] `type_hash` is deterministic
- [ ] ≥6 unit tests pass
- [ ] `rg 'fn crc32|fn type_hash|fn frame_message|fn deframe_message' src/codegen/src/ipc.rs` ≥4 matches

---

## Wave 10: Runtime Encapsulation L1 — Integrate Framing into Channel

**Spec refs:** §12, §46, §47 · **Target backends:** all 5 (x86_64 inline + 4 via ipc_lowering)

### 10a — Integrate framing into ChannelSend (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. **x86_64**: build 56-byte frame (MAGIC + version + channel_id + sequence + type_hash + payload_len + cap_count + payload + CRC32), `write()` 56 bytes
2. **Non-x86_64** (`ipc_lowering.rs`): `expand_channel_send` must emit the SAME 56-byte frame via `Alloc` + `Store` + `Syscall`. MUST compute real CRC32 (not hardcoded 0). MUST use per-function sequence counter (not hardcoded 0).
3. CRC32 polynomial `0xEDB88320` — either inline loop (x86_64) or IR loop (non-x86_64)

### 10b — Integrate framing + CRC verification into ChannelRecv (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. **x86_64**: read 56 bytes, verify MAGIC, cap_count, CRC32 (inline loop), type_hash; extract payload. On CRC mismatch → -6 (CRC_MISMATCH).
2. **Non-x86_64**: `expand_channel_recv` MUST verify MAGIC, CRC32, type_hash via IR instructions (Cmp + CondBranch). MUST NOT be a stub that only checks `read() <= 0`.

### 10c — Serialization for primitive types
**Files:** `src/codegen/src/ipc.rs`

1. `serialize_value(val, ty) -> Vec<u8>`: I8/U8 (1 byte), I16/U16 (2), I32/U32 (4), I64/U64 (8), F32 (4), F64 (8), Bool (1) — all LE
2. `deserialize_value(bytes, ty) -> IRValue`
3. Round-trip tests for all primitives

### 10d — Port framed channels to aarch64 + riscv64 + arm32 + loongarch64
**Files:** `src/codegen/src/ipc_lowering.rs` (the shared non-x86_64 path)

1. The `ipc_lowering` pass MUST emit real framed IR for all 4 non-x86_64 backends
2. Verify via QEMU: `framed_send_recv.vuma` → exit 42 on all 4

**DoD (Wave 10):**
- [ ] `cargo build --workspace` succeeds
- [ ] Channel messages are framed (header + CRC + type hash) on **all 5 backends**
- [ ] CRC mismatch detected (returns -6) on all 5 backends
- [ ] `rg '0xEDB88320' src/codegen/src/x86_64/stack_slot_isel.rs` ≥1 match
- [ ] `rg '0xEDB88320|crc32' src/codegen/src/ipc_lowering.rs` ≥1 match (non-x86_64 path has real CRC)
- [ ] `framed_send_recv.vuma` → exit 42 on all 5 backends
- [ ] NO `// TODO: real CRC32 loop` or `// simplified` in `ipc_lowering.rs`

---

## Wave 11: Runtime Encapsulation L2 — Capability Tokens

**Spec refs:** §13, §51, §52 · **Target backends:** n/a (library, consumed by all 5)

### 11a — Create capability module
**Files:** `src/codegen/src/capability.rs` (new), `src/codegen/src/lib.rs`

1. `CapabilityToken`: id (u128), source_pid, target_pid, resource (Resource enum), permissions (MemoryPermissions), delegation_depth, created_at, expires_at, signature ([u8;32])
2. `Resource`: File(String), Network(String,u16), Memory(u64,u64), Mmio(u64,u64), Channel(u64)
3. `MemoryPermissions`: read, write, execute
4. `CapabilitySet`: HashMap<u128, CapabilityToken>

### 11b — Implement grant and verify
**Files:** `src/codegen/src/capability.rs`

1. `grant_capability(source_pid, target_pid, resource, perms, signing_key) -> CapabilityToken`
2. `verify_capability(token, signing_key, now, expected_resource, required_perms) -> Result<(), CapabilityError>`
3. Check: signature valid, not expired, resource matches, permissions sufficient

### 11c — Capability revocation registry
**Files:** `src/codegen/src/capability.rs`

1. `RevocationRegistry` (HashSet<u128>)
2. `revoke(token_id)`, `is_revoked(token_id)`, `revoke_with_propagation` (propagates to delegated children)

### 11d — Capability encoding for IPC
**Files:** `src/codegen/src/capability.rs`

1. `CapabilityToken::encode() -> [u8; CAPABILITY_TOKEN_SIZE]` (160 bytes)
2. `CapabilityToken::decode(bytes) -> Result<Self, String>`
3. `CAPABILITY_TOKEN_SIZE = 160`

**DoD (Wave 11):**
- [ ] `cargo build --workspace` succeeds
- [ ] Capability module with grant/verify/revoke exists
- [ ] `CapabilityToken::encode/decode` round-trips
- [ ] `rg 'fn grant_capability|fn verify_capability|fn delegate_capability' src/codegen/src/capability.rs` ≥3 matches

---

## Wave 12: Runtime Encapsulation L2 — Capabilities in IPC Messages

**Spec refs:** §13, §46 · **Target backends:** all 5

### 12a — Attach capabilities to framed messages
**Files:** `src/codegen/src/ipc.rs`

1. Add `capabilities: Vec<CapabilityToken>` to `EncapsulatedMessage`
2. `frame_message`: write `cap_count`, then each token's `encode()` after payload
3. `deframe_message`: read `cap_count`, then that many tokens
4. Set `HAS_CAPS` flag if capabilities present

### 12b — Verify capabilities on receive (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. **x86_64**: `capability_grant(resource_id, perms)` builtin — mints token at compile time via `ipc::capability::grant_capability`, returns cap_id
2. **x86_64**: `channel_send_cap(ch, msg, cap_id)` builtin — 64-byte frame (56 + 8-byte cap section), cap_count=1, cap_id at [rsp+56]
3. **x86_64**: `ChannelRecv` — when cap_count > 0, read 8-byte cap_id, verify non-zero (structural check). Zero → PermissionDenied (-4).
4. **Non-x86_64** (`ipc_lowering.rs`): `expand_channel_send_cap` MUST write the cap_id to the frame (not ignore it). `expand_channel_recv` MUST read + verify cap_id (not skip it).

### 12c — Capability delegation chain
**Files:** `src/codegen/src/capability.rs`, `src/codegen/src/x86_64/stack_slot_isel.rs`

1. `delegate_capability(parent_token, new_target, subset_perms, signing_key) -> CapabilityToken`
2. New token: `delegation_depth = parent.delegation_depth + 1`, max 8
3. New permissions must be subset of parent's
4. `capability_delegate(parent_id, resource_id, perms)` builtin on x86_64

### 12d — Capability integration tests
**Files:** `tests/gold_standard/ipc/`

1. `capability_grant_verify.vuma` — grant cap, send_cap, recv verifies, exit 42
2. `capability_send.vuma` — basic capability send, exit 42
3. `delegation.vuma` — grant → delegate → child uses, exit 1

**DoD (Wave 12):**
- [ ] IPC messages carry capability tokens on all 5 backends
- [ ] Capability signatures verified on receive (non-zero cap_id) on all 5 backends
- [ ] `capability_grant_verify.vuma` → exit 42 on all 5 backends
- [ ] `rg 'capability_grant|channel_send_cap' src/codegen/src/ipc_lowering.rs` ≥1 match (non-x86_64 path handles caps)

---

## Wave 13: Runtime Encapsulation L3 — Memory Windows

**Spec refs:** §14, §9.2, §9.3 · **Target backends:** all 5

### 13a — Define MemoryWindow struct
**Files:** `src/codegen/src/ipc.rs`

1. `MemoryWindow`: source_pid, target_pid, source_addr, target_addr, size, permissions, capability_id, revocable, linear
2. `grant_memory(source, target, addr, size, perms) -> MemoryWindow`
3. `revoke_memory(window) -> Result<()>`

### 13b — Implement shared memory on all backends
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. `shared_memory_open(size)`: `mmap(NULL, size, PROT_READ|PROT_WRITE, MAP_SHARED|MAP_ANONYMOUS, -1, 0)` (nr 9 on x86_64, 222 on asm-generic)
2. `shared_memory_read(ptr, offset)`: load 8 bytes from `ptr + offset`
3. `shared_memory_write(ptr, offset, value)`: store 8 bytes to `ptr + offset`
4. **Non-x86_64**: `expand_shared_memory_open/read/write` must emit real mmap/Load/Store IR

### 13c — Memory window permissions enforcement
**Files:** `src/codegen/src/ipc.rs`

1. READ_ONLY: `mmap` with `PROT_READ` only → SIGSEGV on write (hardware enforced)
2. READ_WRITE: `PROT_READ|PROT_WRITE`

### 13d — Memory window test
**Files:** `tests/gold_standard/ipc/shared_memory.vuma`, `tests/gold_standard/ipc/shared_memory_rw.vuma`

1. Parent writes 200 to shared memory, child reads, exits 200
2. Write+read roundtrip: exit 1

**DoD (Wave 13):**
- [ ] `shared_memory.vuma` → exit 200 on all 5 backends
- [ ] `shared_memory_rw.vuma` → exit 1 on all 5 backends
- [ ] `rg 'shared_memory_open|shared_memory_read|shared_memory_write' src/codegen/src/ipc_lowering.rs` ≥3 matches

---

## Wave 14: Runtime Encapsulation L4 — Protocol State Machine

**Spec refs:** §15, §50 · **Target backends:** all 5

### 14a — Define protocol state machine
**Files:** `src/codegen/src/ipc.rs`

1. `ProtocolState`: Idle, WaitingForSend, WaitingForRecv, Closed
2. `ProtocolTransition`: `(ProtocolState, type_hash) -> ProtocolState`
3. `allowed_transitions: HashMap<(ProtocolState, u64), ProtocolState>`
4. `channel_protocol_check(state, type_hash) -> Result<ProtocolState, ProtocolError>`

### 14b — Integrate protocol check into ChannelRecv (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. **x86_64**: `channel_recv_proto(ch, expected_state)` builtin — verifies per-function proto_state == expected_state before recv; on success advances proto_state (+= 1). Mismatch → -5 (ProtocolViolation).
2. **Non-x86_64** (`ipc_lowering.rs`): `expand_channel_recv_proto` MUST verify the state (Cmp + CondBranch), NOT just call `expand_channel_recv` ignoring `expected_state`.

### 14c — Protocol state machine tests
**Files:** `tests/gold_standard/ipc/protocol_valid.vuma`, `tests/gold_standard/ipc/protocol_invalid.vuma`

1. `protocol_valid.vuma`: recv_proto(0) then recv_proto(1) — both match advancing state. Exit 50.
2. `protocol_invalid.vuma`: recv_proto(0) twice — second detects mismatch, returns -5. Exit 99.

### 14d — Port protocol FSM to all backends
**Files:** `src/codegen/src/ipc_lowering.rs`

1. The non-x86_64 path MUST enforce the FSM (reject invalid transitions with -5)
2. Both `protocol_valid.vuma` and `protocol_invalid.vuma` must produce correct exit codes on all 5 backends

**DoD (Wave 14):**
- [ ] `protocol_valid.vuma` → exit 50 on all 5 backends
- [ ] `protocol_invalid.vuma` → exit 99 on all 5 backends (proves FSM rejects violations)
- [ ] `rg 'channel_recv_proto' src/codegen/src/ipc_lowering.rs` ≥1 match
- [ ] `expand_channel_recv_proto` in ipc_lowering.rs does NOT just call `expand_channel_recv` (must check state)

---

## Wave 15: Runtime Encapsulation L1-L4 — Cross-Backend Porting

**Spec refs:** §12–15 · **Target backends:** aarch64, riscv64, arm32, loongarch64

### 15a — Verify framed channels on riscv64
1. `framed_send_recv.vuma` → exit 42 on riscv64 via QEMU
2. CRC32 verified, type_hash checked

### 15b — Verify framed channels on arm32
1. Same test → exit 42 on arm32 via QEMU

### 15c — Verify capability verification on riscv64 + arm32
1. `capability_grant_verify.vuma` → exit 42 on both backends
2. Cap_id structurally verified

### 15d — Verify protocol FSM on riscv64 + arm32
1. `protocol_valid.vuma` → exit 50, `protocol_invalid.vuma` → exit 99 on both backends

**DoD (Wave 15):**
- [ ] All 4 runtime encapsulation layers (L1 framing, L2 caps, L3 shm, L4 protocol) work on all 5 backends
- [ ] No `// simplified` or `// TODO` stubs in `ipc_lowering.rs` for these features

---

## Wave 16: Runtime Encapsulation L1-L4 — Integration Tests + CI

**Spec refs:** §12–15 · **Target backends:** all 5

### 16a — Create integration test suite
**Files:** `tests/gold_standard/ipc/`

1. `capability_grant_verify.vuma` (exit 42), `shared_memory_rw.vuma` (exit 1), `protocol_valid.vuma` (exit 50), `protocol_invalid.vuma` (exit 99), `large_payload.vuma` (exit 45)
2. Each MUST call the feature's builtin (anti-cheat: md5 ≠ `ec6eb67ebb89132ebe877b0fa017dbb7`)

### 16b — Add `make test-ipc` target
**Files:** `Makefile`

1. `make test-ipc` runs all `tests/gold_standard/ipc/*.vuma` on x86_64, reports pass/fail
2. `make test-ipc-cross` runs the same on aarch64, riscv64, arm32 via QEMU

### 16c — Cross-backend IPC test runner
**Files:** `scripts/run_ipc_cross_backend.sh` (or `.py`)

1. For each IPC test × each backend: compile, run via QEMU, check exit code
2. Print matrix: rows=tests, cols=backends, cells=PASS/FAIL

### 16d — Performance baseline
**Files:** `tests/gold_standard/ipc/bench.vuma`

1. `bench.vuma` → exit 232 (1000 % 256 = 232)

**DoD (Wave 16):**
- [ ] 5 integration tests exist and pass on all 5 backends
- [ ] `make test-ipc` target exists and runs
- [ ] `rg 'ipc|channel_send|recv_timeout' Makefile` ≥1 match
- [ ] `bench.vuma` → exit 232

---

## Wave 17: Runtime Encapsulation L5 — Worker Sandboxing (seccomp)

**Spec refs:** §16 · **Target backends:** all 5

### 17a — Define WorkerConfig and sandbox options
**Files:** `src/codegen/src/ipc.rs`

1. `WorkerConfig`: allowed_syscalls (HashSet<u32>), resource_limits, sandbox_enabled
2. Default allowlist: read, write, close, exit, exit_group, brk, mmap, munmap, rt_sigreturn

### 17b — Generate seccomp BPF filter
**Files:** `src/codegen/src/ipc.rs`

1. `generate_seccomp_filter(allowlist) -> Vec<sock_filter>` — BPF program that checks `seccomp_data.nr` against allowlist
2. `SECCOMP_RET_KILL` for disallowed syscalls

### 17c — Apply seccomp filter (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. **x86_64**: `sandbox_apply()` builtin — `prctl(PR_SET_NO_NEW_PRIVS=38, 1)` + `seccomp(SECCOMP_SET_MODE_FILTER=1, 0, &prog)` (nr 317)
2. **Non-x86_64**: `expand_sandbox_apply` must emit both prctl + seccomp syscalls (not just prctl)
3. `sandbox_seccomp()` builtin — installs the BPF filter

### 17d — Worker sandbox test
**Files:** `tests/gold_standard/ipc/sandbox.vuma`

1. Apply sandbox, try a forbidden syscall → killed, exit 1
2. `// Expected exit code: 1`

**DoD (Wave 17):**
- [ ] `sandbox.vuma` → exit 1 on all 5 backends
- [ ] `rg 'seccomp|PR_SET_NO_NEW_PRIVS' src/codegen/src/ipc_lowering.rs` ≥1 match (non-x86_64 has real seccomp)

---

## Wave 18: Runtime Encapsulation L5 — Resource Limits

**Spec refs:** §17 · **Target backends:** all 5

### 18a — Define ResourceLimits
**Files:** `src/codegen/src/ipc.rs`

1. `ResourceLimits`: cpu_seconds, max_memory, max_fds, max_processes
2. Map to `setrlimit` resources: RLIMIT_CPU=0, RLIMIT_AS=9, RLIMIT_NOFILE=7, RLIMIT_NPROC=6

### 18b — Enforce CPU limit (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. `set_resource_limit(resource, limit)` builtin — `setrlimit(resource, {limit, limit})` (nr 160)
2. **Non-x86_64**: `expand_set_resource_limit` must emit real setrlimit syscall

### 18c — Enforce memory limit (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. `set_memory_limit(bytes)` builtin — `setrlimit(RLIMIT_AS=9, {bytes, bytes})` (distinct from generic `set_resource_limit`)
2. **Non-x86_64**: `expand_set_memory_limit` must emit real setrlimit

### 18d — Resource limit test
**Files:** `tests/gold_standard/ipc/resource_limit.vuma`, `tests/gold_standard/ipc/memory_limit.vuma`

1. `resource_limit.vuma` → exit 1, `memory_limit.vuma` → exit 1

**DoD (Wave 18):**
- [ ] `resource_limit.vuma` → exit 1 on all 5 backends
- [ ] `memory_limit.vuma` → exit 1 on all 5 backends
- [ ] `rg 'set_memory_limit|set_resource_limit' src/codegen/src/ipc_lowering.rs` ≥2 matches

---

## Wave 19: Runtime Encapsulation L6 — State Checkpointing

**Spec refs:** §18 · **Target backends:** all 5

### 19a — Define Checkpoint struct
**Files:** `src/codegen/src/ipc.rs`

1. `Checkpoint`: worker_pid, state_hash (u64), register_snapshot (Vec<u64>), memory_regions (Vec<MemoryRegion>), timestamp
2. `Checkpoint::encode() -> Vec<u8>`, `Checkpoint::decode(bytes) -> Result<Self>`

### 19b — Implement checkpoint_save (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. **x86_64**: `checkpoint_save(state)` builtin — serialize state to `/tmp/vuma_checkpoint.bin` via `open`+`write`
2. **Non-x86_64**: `expand_checkpoint_save` must emit real file I/O (Alloc + Syscall open/write/close)

### 19c — Implement checkpoint_restore (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. `checkpoint_restore()` builtin — read from `/tmp/vuma_checkpoint.bin`, return state

### 19d — Checkpoint test
**Files:** `tests/gold_standard/ipc/checkpoint.vuma`

1. Save state, restore, verify match. Exit 1.

**DoD (Wave 19):**
- [ ] `checkpoint.vuma` → exit 1 on all 5 backends
- [ ] `rg 'checkpoint_save|checkpoint_restore' src/codegen/src/ipc_lowering.rs` ≥2 matches
- [ ] Checkpoint uses the real `Checkpoint` struct (not a single u64)

---

## Wave 20: Runtime Encapsulation L6 — Error Containment

**Spec refs:** §19 · **Target backends:** all 5

### 20a — Define ErrorContainmentPolicy
**Files:** `src/codegen/src/ipc.rs`

1. `ErrorContainmentPolicy`: on_panic (Kill/Restart/Propagate), max_restarts, restart_delay_ms
2. `WorkerWatcher`: tracks worker health, restarts on crash

### 20b — Implement crash detection (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. `wait_worker` with `WUNTRACED|WCONTINUED` flags — detect SIGSEGV/SIGABRT
2. Extract `WTERMSIG(status)` to identify crash cause

### 20c — Implement worker restart (all backends)
1. On crash: `spawn_worker` again, `checkpoint_restore` to recover state

### 20d — Error containment test
**Files:** `tests/gold_standard/ipc/error_containment.vuma`

1. Child crashes (SIGSEGV), parent detects, restarts child, recovers. Exit 1.

**DoD (Wave 20):**
- [ ] `error_containment.vuma` → exit 1 on all 5 backends
- [ ] Crash detection extracts signal number

---

## Wave 21: Runtime Encapsulation L6 — Graceful Degradation

**Spec refs:** §19 · **Target backends:** all 5

### 21a — Define DegradationPolicy
**Files:** `src/codegen/src/ipc.rs`

1. `DegradationPolicy`: max_workers, fallback_handler, shed_load_threshold
2. Load shedding: when worker pool exhausted, reject new requests with `ServiceUnavailable`

### 21b–21d — Implement + test graceful degradation
**Files:** all backend isels + `ipc_lowering.rs`

1. `worker_pool_size()` builtin, `shed_load()` builtin
2. Test: pool exhausted → graceful rejection. Exit 1.

**DoD (Wave 21):**
- [ ] Graceful degradation test passes on all 5 backends

---

## Wave 22: Runtime Encapsulation L7 — Supervised Workers

**Spec refs:** §20 · **Target backends:** all 5

### 22a — Define Supervisor architecture
**Files:** `src/codegen/src/ipc.rs`

1. `Supervisor`: worker_pool, health_check_interval, restart_policy
2. `Supervisor::spawn_supervised(config) -> WorkerHandle`

### 22b–22d — Implement + test supervisor
**Files:** all backend isels + `ipc_lowering.rs`

1. `supervisor_spawn(config)` builtin, `supervisor_health_check()` builtin
2. Test: supervised worker crashes, supervisor restarts it. Exit 1.

**DoD (Wave 22):**
- [ ] Supervisor test passes on all 5 backends

---

## Wave 23: Runtime Encapsulation L8 — AEAD Crypto (Wire Format)

**Spec refs:** §20 · **Target backends:** all 5

### 23a — Define CryptoState / AeadXor wire format
**Files:** `src/codegen/src/ipc.rs`

1. `CryptoState`: nonce ([u8;8]), key ([u8;32]), counter (u64)
2. `AeadXor::seal(state, plaintext) -> ciphertext` — XOR stream cipher with nonce+counter keystream
3. `AeadXor::open(state, ciphertext) -> plaintext`
4. Wire format: `nonce(8) | ciphertext(len) | tag(4=CRC32 of nonce+ciphertext)`

### 23b — Implement aead_seal (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. **x86_64**: `aead_seal(ptr, len, key)` builtin — XOR each byte with key byte (stream cipher)
2. **Non-x86_64**: `expand_aead_seal` must emit real XOR loop via IR (not a stub)

### 23c — Implement aead_open (all backends)
1. Symmetric: `aead_open(ptr, len, key)` — same XOR operation

### 23d — AEAD test
**Files:** `tests/gold_standard/ipc/aead.vuma`

1. Seal, open, verify payload matches. Exit 1.

**DoD (Wave 23):**
- [ ] `aead.vuma` → exit 1 on all 5 backends
- [ ] AEAD uses the real `CryptoState` wire format (nonce + ciphertext + CRC32 tag)
- [ ] `rg 'aead_seal|aead_open' src/codegen/src/ipc_lowering.rs` ≥2 matches

---

## Wave 24: Runtime Encapsulation L8 — Integration

**Spec refs:** §20 · **Target backends:** all 5

### 24a–24d — End-to-end encrypted IPC + tamper detection
**Files:** all backend isels + `ipc_lowering.rs` + `tests/gold_standard/ipc/`

1. `aead_tamper.vuma` — seal a message, tamper with ciphertext, open → fails (CRC32 tag mismatch). Exit 1.
2. Encrypted channel: `channel_send` + `aead_seal` composed. Exit 42.

**DoD (Wave 24):**
- [ ] `aead_tamper.vuma` → exit 1 on all 5 backends (tamper detected)
- [ ] All L5–L8 features work on all 5 backends

---

# Phase 2 — FFI Process Isolation (Waves 25–32)

**Spec refs:** §56–61 · **Target backends:** all 5

## Wave 25: `extern "process"` ABI + Auto-Marshalling

### 25a — Add `extern "process"` ABI to parser + scg_to_ir
**Files:** `src/parser/src/parser.rs`, `src/codegen/src/scg_to_ir.rs`

1. Parse `extern "process" fn foo(arg: i32) -> i32;` declarations
2. Lower to `FfiEnvelope` descriptor in SCG — marks calls to `process_*` functions as cross-process
3. `rg 'extern.*process|FfiEnvelope|process_call' src/codegen/src/scg_to_ir.rs` ≥1 match

### 25b — Implement `process_call` builtin (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. `process_call(ch, arg)` — marshals arg via `channel_send(ch, arg)`, waits for reply via `channel_recv(ch)`, returns reply
2. **Non-x86_64**: `expand_process_call` must emit real channel_send + channel_recv (not a stub)

### 25c — FFI worker lifecycle
**Files:** `src/codegen/src/ipc.rs`

1. `FfiWorkerConfig`: entry_point, arg_types, return_type, sandbox_config
2. `spawn_ffi_worker(config) -> WorkerHandle`

### 25d — FFI basic test
**Files:** `tests/gold_standard/ipc/ffi_basic.vuma`, `tests/gold_standard/ipc/ffi_isolation.vuma`

1. `ffi_basic.vuma` — call process function that returns 42. Exit 42.
2. `ffi_isolation.vuma` — child doubles arg, parent calls process_call(ch, 21) → 42. Exit 42.

**DoD (Wave 25):**
- [ ] `ffi_basic.vuma` → exit 42 on all 5 backends
- [ ] `ffi_isolation.vuma` → exit 42 on all 5 backends
- [ ] `rg 'process_call|FfiEnvelope' src/codegen/src/ipc_lowering.rs` ≥1 match

---

## Wave 26: FFI Seccomp Isolation

### 26a–26d — Sandboxed FFI worker + test
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`, `tests/gold_standard/ipc/`

1. FFI workers get `sandbox_apply()` + `set_memory_limit()` before running foreign code
2. `ffi_sandbox.vuma` — FFI worker sandboxed, can't do forbidden syscall. Exit 1.

**DoD (Wave 26):** `ffi_sandbox.vuma` → exit 1 on all 5 backends.

---

## Wave 27: FFI Crash Recovery

### 27a–27d — Crash detection + recovery for FFI calls
**Files:** all backend isels + `ipc_lowering.rs` + `tests/`

1. `process_call` detects FFI worker crash (recv returns -1 = closed), returns error code
2. `ffi_crash_recovery.vuma` — FFI worker crashes, caller gets error. Exit 1.

**DoD (Wave 27):** `ffi_crash_recovery.vuma` → exit 1 on all 5 backends.

---

## Wave 28: FFI Performance Optimization

### 28a–28d — Batched marshalling + zero-copy shared memory FFI
**Files:** `src/codegen/src/ipc.rs`, all backend isels

1. Batch multiple args into one framed message
2. Zero-copy path: large args via shared memory instead of channel copy
3. `ffi_perf.vuma` — batched FFI call. Exit 1.

**DoD (Wave 28):** `ffi_perf.vuma` passes on all 5 backends.

---

## Waves 29–32: FFI Integration + Tests

### 29a–32d — FFI integration tests + CI
**Files:** `tests/gold_standard/ipc/`

1. `ffi_types.vuma` (all primitive types through FFI), `ffi_nested.vuma` (nested process calls), `ffi_concurrent.vuma` (concurrent FFI workers)
2. Each passes on all 5 backends

**DoD (Waves 29–32):** All FFI tests pass on all 5 backends.

---

# Phase 4 — Capability System (Waves 33–40)

**Spec refs:** §51–55 · **Target backends:** all 5

## Wave 33: Capability Delegation Chain

### 33a — Implement `delegate_capability` (real code, not re-export)
**Files:** `src/codegen/src/capability.rs`

1. `delegate_capability(parent_token, child_target, subset_perms, signing_key) -> CapabilityToken`
2. New token: `delegation_depth = parent.delegation_depth + 1`, max `MAX_DELEGATION_DEPTH=8`
3. Child permissions must be subset of parent's
4. `rg 'fn delegate_capability' src/codegen/src/capability.rs` ≥1 match of REAL code

### 33b — `capability_delegate` builtin (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. `capability_delegate(parent_id, resource_id, perms)` — calls `delegate_capability` at compile time, returns child cap_id

### 33c — Delegation chain verification
1. `verify_delegation_chain(token, signing_key) -> Result` — walks the chain to root

### 33d — Delegation test
**Files:** `tests/gold_standard/ipc/delegation.vuma`

1. Grant → delegate → child uses delegated cap. Exit 1.

**DoD (Wave 33):**
- [ ] `delegation.vuma` → exit 1 on all 5 backends
- [ ] `rg 'delegate_capability' src/codegen/src/capability.rs` ≥1 match (real function, not `pub use`)

---

## Wave 34: Capability Flow Verification

### 34a–34d — Verify capabilities flow through the system correctly
**Files:** `src/codegen/src/capability.rs`, all backend isels, `tests/`

1. `capability_flow_check(token, source, target)` — verify token's source_pid/target_pid match expected flow
2. `cap_flow.vuma` — token with wrong target rejected. Exit 1.

**DoD (Wave 34):** `cap_flow.vuma` passes on all 5 backends.

---

## Wave 35: Revocation Propagation

### 35a–35d — Revoking a parent token revokes all delegated children
**Files:** `src/codegen/src/capability.rs`, `tests/`

1. `revoke_with_propagation(token_id) -> Vec<u128>` (returns all revoked child IDs)
2. `cap_revoke.vuma` — grant → delegate → revoke parent → child cap fails. Exit 1.

**DoD (Wave 35):** `cap_revoke.vuma` passes on all 5 backends.

---

## Waves 36–40: Cross-Process Capability Tracking + Tests

### 36a–40d — Cross-process cap tracking + integration tests
**Files:** `src/codegen/src/capability.rs`, all backend isels, `tests/`

1. `CapabilityTracker`: per-process table of issued/revoked tokens
2. `cap_cross_process.vuma`, `cap_expiry.vuma`, `cap_depth_limit.vuma`

**DoD (Waves 36–40):** All capability tests pass on all 5 backends.

---

# Phase 3 — Kernel/User Split (Waves 41–48)

**Spec refs:** §62–66 · **Target backends:** all 5

## Wave 41: Microkernel Architecture — Supervisor Call

### 41a — Define kernel/user split types
**Files:** `src/codegen/src/ipc.rs`

1. `KernelProcess`, `UserProcess`, `SupervisorEntry` types
2. `KernelCapability`: gates which syscalls a user process may make

### 41b — Implement `supervisor_call` builtin (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. **x86_64**: `supervisor_call(nr, arg)` — emits a real syscall gate (`syscall` instruction with nr in RAX, arg in RDI). Capability-gated: checks a per-function allowlist before executing.
2. **Non-x86_64**: `expand_supervisor_call` must emit a real syscall (not `return 0`). MUST implement the capability gate (Verified allowlist) — not just a raw syscall.

### 41c — Kernel/user gate codegen
1. Before executing the syscall, check `nr` against a compile-time allowlist. If not allowed → return -4 (PermissionDenied).

### 41d — Supervisor test
**Files:** `tests/gold_standard/ipc/supervisor.vuma`

1. Make a supervisor call (e.g. `supervisor_call(39, 0)` = getpid), check return > 0. Exit 1.

**DoD (Wave 41):**
- [ ] `supervisor.vuma` → exit 1 on all 5 backends
- [ ] `rg 'supervisor_call' src/codegen/src/ipc_lowering.rs` ≥1 match
- [ ] Non-x86_64 `expand_supervisor_call` is NOT a `return 0` stub

---

## Waves 42–48: Syscall-as-IPC, Kernel Process, User Process, Resource Accounting

### 42a–48d — Kernel/user split integration
**Files:** `src/codegen/src/ipc.rs`, all backend isels, `tests/`

1. `syscall_as_ipc(nr, args)` — routes syscalls through the kernel process via IPC
2. `kernel_process_spawn()`, `user_process_spawn()` builtins
3. `resource_account(cpu, memory)` — track per-process resource usage
4. `kernel_split.vuma`, `resource_account.vuma` tests

**DoD (Waves 42–48):** All kernel/user split tests pass on all 5 backends.

---

# Phase 5 — Driver Isolation (Waves 49–56)

**Spec refs:** §67–71 · **Target backends:** all 5

## Wave 49: Driver Worker + MMIO Capabilities

### 49a — Define DriverWorkerConfig
**Files:** `src/codegen/src/ipc.rs`

1. `DriverWorkerConfig`: irq, mmio_base, mmio_size, dma_region, handler_ptr
2. `driver_register(irq, handler_ptr) -> driver_id` builtin

### 49b — Implement `driver_register` + `driver_call` (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. `driver_register(irq, handler)` — mints a driver ID (compile-time counter), registers handler
2. `driver_call(driver_id, cmd)` — dispatches to the driver via channel_send + channel_recv
3. **Non-x86_64**: `expand_driver_call` must emit real channel_send + channel_recv (not a stub)

### 49c — MMIO capability enforcement
1. `mmio_cap(mmio_base, mmio_size)` — grants access to a memory-mapped IO region

### 49d — Driver isolation test
**Files:** `tests/gold_standard/ipc/driver_isolation.vuma`

1. Register a driver, call it, get result. Exit 42.

**DoD (Wave 49):**
- [ ] `driver_isolation.vuma` → exit 42 on all 5 backends
- [ ] `rg 'driver_register|driver_call' src/codegen/src/ipc_lowering.rs` ≥2 matches

---

## Waves 50–56: IRQ Channels, DMA Buffers, Driver Restart

### 50a–56d — IRQ routing, DMA, driver restart
**Files:** `src/codegen/src/ipc.rs`, all backend isels, `tests/`

1. `irq_dispatch(irq)` — routes IRQ to registered driver handler
2. `dma_alloc(size)` — allocate DMA buffer shared with driver
3. `driver_restart(driver_id)` — restart crashed driver
4. `irq_routing.vuma`, `dma_buffer.vuma`, `driver_restart.vuma` tests

**DoD (Waves 50–56):** All driver tests pass on all 5 backends.

---

# Phase 2 — Sandboxing (Waves 57–64)

**Spec refs:** §72–76 · **Target backends:** all 5

## Waves 57–64: Zero-Cap Workers, Plugins, Sandboxed Parsers, Sandboxed Crypto

### 57a–64d — Sandbox architecture + zero-cap workers + plugins
**Files:** `src/codegen/src/ipc.rs`, `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`, `tests/`

1. `zero_cap_worker()` — spawn a worker with zero capabilities (only exit allowed)
2. `plugin_load(name)` — load a sandboxed plugin
3. `sandboxed_parser(input)` — parse untrusted input in a sandboxed worker
4. `sandboxed_crypto(op, data)` — run crypto in a sandboxed worker
5. `zero_cap.vuma`, `plugin.vuma`, `sandboxed_parser.vuma` tests

**DoD (Waves 57–64):**
- [ ] All sandboxing tests pass on all 5 backends
- [ ] Zero-cap workers cannot make forbidden syscalls (seccomp enforced)

---

# Phase 2 — Fault Tolerance (Waves 65–72)

**Spec refs:** §77–82 · **Target backends:** all 5

## Wave 65: Circuit Breaker

### 65a — Define CircuitBreaker FSM
**Files:** `src/codegen/src/ipc.rs`

1. `CircuitBreaker`: state (Closed/Open/HalfOpen), failure_count, threshold, reset_timeout
2. State transitions: Closed→Open (on `threshold` failures), Open→HalfOpen (after `reset_timeout`), HalfOpen→Closed (on success) or HalfOpen→Open (on failure)

### 65b — Implement `circuit_breaker_call` (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. **x86_64**: `circuit_breaker_call(fn_ptr, max_retries)` — real indirect `call r10` with retry loop. On failure, increment failure_count; on threshold reached, open the breaker.
2. **Non-x86_64**: `expand_circuit_breaker_call` MUST emit a real retry loop (not `return 0`). MUST track failure state per-function.

### 65c — Circuit breaker state builtins
1. `circuit_breaker_state() -> i64` (0=Closed, 1=Open, 2=HalfOpen)
2. `circuit_breaker_reset()`

### 65d — Fault tolerance test
**Files:** `tests/gold_standard/ipc/fault_tolerance.vuma`

1. Call a failing function through a circuit breaker, verify breaker trips. Exit 1.

**DoD (Wave 65):**
- [ ] `fault_tolerance.vuma` → exit 1 on all 5 backends
- [ ] `rg 'circuit_breaker' src/codegen/src/ipc_lowering.rs` ≥1 match
- [ ] Non-x86_64 `expand_circuit_breaker_call` is NOT a `return 0` stub

---

## Waves 66–72: Supervisor, Crash Detection, Checkpoint, Restart, Graceful Degradation

### 66a–72d — Fault tolerance integration
**Files:** `src/codegen/src/ipc.rs`, all backend isels, `tests/`

1. `supervisor_spawn_supervised(config)` — spawn with auto-restart
2. `crash_detect(pid) -> signal` — detect crash cause
3. `worker_restart(pid)` — restart from checkpoint
4. `graceful_degrade()` — shed load on overload
5. `supervised_worker.vuma`, `crash_recovery.vuma`, `graceful_degradation.vuma` tests

**DoD (Waves 66–72):** All fault tolerance tests pass on all 5 backends.

---

# Phase 2 — Hot Reloading (Waves 73–80)

**Spec refs:** §83–86 · **Target backends:** all 5

## Wave 73: Hot-Swap Protocol

### 73a — Define HotSwapRequest + version state machine
**Files:** `src/codegen/src/ipc.rs`

1. `HotSwapRequest`: module_id, old_version, new_version, state_transfer_fn
2. Version validation: `new_version > old_version` (monotonic)
3. State machine: `Idle → Swapping → Verified → Active` or `Swapping → RolledBack`

### 73b — Implement `hot_swap_trigger` (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. **x86_64**: `hot_swap_trigger(module_id, old_version, new_version)` — validates version monotonicity, writes swap request to state table, returns 1 on success or -5 on violation
2. **Non-x86_64**: `expand_hot_swap_trigger` must emit real version comparison (Cmp + Select), not `return 1`

### 73c — Hot-swap register + rollback
1. `hot_swap_register(module_id, initial_version)` — register a module for hot-swapping
2. `hot_swap_rollback(module_id)` — revert to previous version

### 73d — Hot swap test
**Files:** `tests/gold_standard/ipc/hot_swap.vuma`

1. Register a module, trigger a swap, verify. Exit 1.

**DoD (Wave 73):**
- [ ] `hot_swap.vuma` → exit 1 on all 5 backends
- [ ] `rg 'hot_swap_trigger|hot_swap_register' src/codegen/src/ipc_lowering.rs` ≥2 matches
- [ ] Non-x86_64 `expand_hot_swap_register` / `expand_hot_swap_rollback` are NOT `return 1` stubs

---

## Waves 74–80: State Transfer, Version Management, Rollback

### 74a–80d — Hot reloading integration
**Files:** `src/codegen/src/ipc.rs`, all backend isels, `tests/`

1. `hot_swap_state_transfer(module_id, old_state) -> new_state` — migrate state across versions
2. `hot_swap_version_check(module_id) -> current_version`
3. `hot_swap_rollback.vuma`, `state_transfer.vuma` tests

**DoD (Waves 74–80):** All hot reloading tests pass on all 5 backends.

---

# Phase 6 — Distributed Channels (Waves 81–88)

**Spec refs:** §87–91 · **Target backends:** all 5

## Wave 81: Location-Transparent Channels

### 81a — Define DistributedChannel
**Files:** `src/codegen/src/ipc.rs`

1. `DistributedChannel`: local_handle, remote_addr, remote_port, transport (Tcp/Udp)
2. `channel_open_remote(addr, port) -> u64` — opens a TCP socketpair (or loopback mock)

### 81b — Implement `channel_open_remote` (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. **x86_64**: `channel_open_remote(addr, port)` — `socket(AF_INET, SOCK_STREAM)`, `connect(addr, port)`, return handle
2. **Non-x86_64**: `expand_channel_open_remote` must emit real socket+connect syscalls (not a stub)

### 81c — Remote send/recv
1. `remote_send(handle, msg)` — `send(socket, &msg, 8, 0)`
2. `remote_recv(handle) -> msg` — `recv(socket, &buf, 8, 0)`

### 81d — Distributed channel test
**Files:** `tests/gold_standard/ipc/distributed.vuma`

1. Open a remote channel (loopback), send, recv. Exit 1.

**DoD (Wave 81):**
- [ ] `distributed.vuma` → exit 1 on all 5 backends
- [ ] `rg 'channel_open_remote|remote_send|remote_recv' src/codegen/src/ipc_lowering.rs` ≥3 matches

---

## Waves 82–88: Discovery, Network Protocol, Failure Detection, Consensus

### 82a–88d — Distributed IPC integration
**Files:** `src/codegen/src/ipc.rs`, all backend isels, `tests/`

1. `worker_discover(service_name) -> Vec<WorkerEndpoint>`
2. `failure_detect(handle) -> bool` — detect remote worker failure
3. `consensus_propose(value) -> consensus_id`, `consensus_vote(consensus_id, value)`
4. `distributed_discovery.vuma`, `failure_detection.vuma` tests

**DoD (Waves 82–88):** All distributed channel tests pass on all 5 backends.

---

# Phase 7 — Compile-Time Encapsulation (Waves 89–92)

**Spec refs:** §21–22, §106–107 · **Target backends:** n/a (compile-time checks, but must be WIRED into pipeline)

## Wave 89: Session Types (Part 1) — Type System

### 89a — Add SessionType to AST + IR
**Files:** `src/parser/src/ast.rs`, `src/codegen/src/ir.rs`

1. `SessionType` enum: `Send(Type, Box<SessionType>)`, `Recv(Type, Box<SessionType>)`, `Choice(Box<SessionType>, Box<SessionType>)`, `Loop(Box<SessionType>)`, `End`
2. Add `session_type: Option<SessionType>` to channel type AST node
3. `rg 'SessionType|SessionProtocol' src/parser/src/ast.rs` ≥1 match
4. `rg 'SessionType|SessionProtocol' src/codegen/src/ir.rs` ≥1 match

### 89b — Parse session type annotations
**Files:** `src/parser/src/parser.rs`

1. Parse `channel_open<Session<Send<i32, Recv<i32, End>>>>()` — session-typed channel
2. `rg 'SessionType' src/parser/src/parser.rs` ≥1 match (real parser code)

### 89c — Wire session type checking into the pipeline
**Files:** `src/pipeline.rs`, `src/codegen/src/scg_to_ir.rs`

1. After SCG construction, run `session_type_check(scg)` — verifies the program follows the declared session
2. On violation: compile error (not just a warning)
3. `rg 'session_type_check|SessionType' src/pipeline.rs` ≥1 match (MUST be called from pipeline, not just library code)

### 89d — Session type valid test
**Files:** `tests/gold_standard/ipc/session_valid.vuma`

1. Program follows the declared session → compiles. Exit 42.

**DoD (Wave 89):**
- [ ] `SessionType` exists in ast.rs + ir.rs
- [ ] `session_type_check` is CALLED from `src/pipeline.rs` (not just library code)
- [ ] `session_valid.vuma` compiles and exits 42

---

## Wave 90: Session Types (Part 2) — Rejection Path

### 90a — Session type violation detection
**Files:** `src/pipeline.rs`

1. `session_type_check` rejects programs that violate the declared session (e.g. send when session expects recv)
2. Compile error with diagnostic

### 90b — Session type invalid test
**Files:** `tests/gold_standard/ipc/session_invalid.vuma`

1. Program violates the session → compile ERROR (not runtime failure)
2. `compile_dump` exits non-zero

**DoD (Wave 90):**
- [ ] `session_invalid.vuma` produces a compile error (not a silent pass)
- [ ] The session checker is wired into the pipeline (not library-only)

---

## Wave 91: Information-Flow Types (Part 1) — Security Lattice

### 91a — Add SecurityLabel to AST + IR
**Files:** `src/parser/src/ast.rs`, `src/codegen/src/ir.rs`

1. `SecurityLabel` enum: `Low`, `High` (and optionally `Internal`, `Secret`, `TopSecret`)
2. `InformationFlow` annotation: variables carry labels
3. `rg 'SecurityLabel|InformationFlow' src/parser/src/ast.rs` ≥1 match
4. `rg 'SecurityLabel|InformationFlow' src/codegen/src/ir.rs` ≥1 match

### 91b — Parse security label annotations
**Files:** `src/parser/src/parser.rs`

1. Parse `let x: i32<Low> = ...` or `let x: High i32 = ...` — labelled variables

### 91c — Wire information-flow checking into the pipeline
**Files:** `src/pipeline.rs`

1. `information_flow_check(scg)` — verifies no High-to-Low flow (a High variable cannot be assigned to a Low variable)
2. `rg 'information_flow_check|SecurityLabel' src/pipeline.rs` ≥1 match (MUST be called from pipeline)

### 91d — Info-flow valid test
**Files:** `tests/gold_standard/ipc/infoflow_valid.vuma`

1. Program with valid label flow → compiles. Exit 1.

**DoD (Wave 91):**
- [ ] `SecurityLabel` exists in ast.rs + ir.rs
- [ ] `information_flow_check` is CALLED from `src/pipeline.rs`
- [ ] `infoflow_valid.vuma` compiles and exits 1

---

## Wave 92: Information-Flow Types (Part 2) — Rejection Path

### 92a — High-to-Low flow detection
**Files:** `src/pipeline.rs`

1. `information_flow_check` rejects programs that assign High variables to Low variables

### 92b — Info-flow invalid test
**Files:** `tests/gold_standard/ipc/infoflow_invalid.vuma`

1. Program with High-to-Low flow → compile ERROR

**DoD (Wave 92):**
- [ ] `infoflow_invalid.vuma` produces a compile error
- [ ] The info-flow checker is wired into the pipeline

---

# Phase 8 — zk-STARK Architecture (Waves 93–94)

**Spec refs:** §26, §34–38 · **Target backends:** all 5 (runtime proof generation + verification)

## Wave 93: STARK Proof Generation

### 93a — Define StarkProof struct
**Files:** `src/codegen/src/ipc.rs`

1. `StarkProof`: proof_data (Vec<u8>), verifier_key (u64), public_inputs (Vec<u64>), validity_window (u64)
2. `StarkProof::new_valid(proof_data, public_inputs, validity_window) -> StarkProof` — computes verifier_key via FNV-1a commitment
3. `StarkProof::commitment(&self) -> u64` — recomputes the commitment for verification

### 93b — Implement `stark_prove` builtin (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. **x86_64**: `stark_prove(input)` — embeds proof_data + verifier_key in a per-function proof table, returns 1-based proof handle
2. **Non-x86_64**: `expand_stark_prove` must emit real proof-table storage (Alloc + Store), NOT `return 1`

### 93c — Proof table management
1. Per-function proof table (max 4 entries), each 56 bytes (proof_data[32] + verifier_key[8] + public_input[8] + validity_window[8])

### 93d — STARK proof test
**Files:** `tests/gold_standard/ipc/stark_proof.vuma`

1. Generate a proof, verify it. Exit 1.

**DoD (Wave 93):**
- [ ] `stark_proof.vuma` → exit 1 on all 5 backends
- [ ] `rg 'StarkProof|zk_stark' src/codegen/src/ir.rs` ≥1 match
- [ ] Non-x86_64 `expand_stark_prove` is NOT a `return 1` stub

---

## Wave 94: STARK Proof Verification

### 94a — Implement `stark_verify` builtin (all backends)
**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`, `src/codegen/src/ipc_lowering.rs`

1. **x86_64**: `stark_verify(proof_handle)` — recomputes FNV-1a commitment over stored proof_data ++ public_input (40 bytes), compares with stored verifier_key. Returns 1 on match, 0 on mismatch.
2. **Non-x86_64**: `expand_stark_verify` must emit a real FNV-1a loop + Cmp (not `return 1`)

### 94b — STARK tamper test
**Files:** `tests/gold_standard/ipc/stark_tamper.vuma`

1. Generate a proof, tamper with proof_data, verify → fails (returns 0). Exit 1.

**DoD (Wave 94):**
- [ ] `stark_tamper.vuma` → exit 1 on all 5 backends (tamper detected)
- [ ] Non-x86_64 `expand_stark_verify` is NOT a `return 1` stub

---

# Phase 9 — Refinement, Linear, Homomorphic, CSL-Perm, Noise (Wave 95)

**Spec refs:** §23–25, §27–28, §108 · **Target backends:** n/a (compile-time, must be WIRED into pipeline)

## Wave 95: CT3–CT8 Compile-Time Encapsulation

### 95a — Linear type checking (CT4)
**Files:** `src/ive/src/borrow_region.rs`, `src/pipeline.rs`

1. `LinearType` enum: `Linear`, `Affine`, `Unrestricted`
2. `linear_check(uses, linset) -> Vec<LinearVerification>` — flags variables used twice (linear violation)
3. **MUST be called from `src/pipeline.rs`** (not just library code with unit tests)
4. `rg 'linear_check' src/pipeline.rs` ≥1 match (pipeline call-site)
5. `rg 'LinearType|linear_check' src/ive/src/borrow_region.rs` ≥1 match

### 95b — Refinement types (CT3)
**Files:** `src/parser/src/ast.rs`, `src/pipeline.rs`

1. `RefinementType { base: Type, predicate: Expr }` — e.g. `i32{ x: x > 0 }`
2. `refinement_check(scg)` — verifies predicates hold at compile time
3. Wired into pipeline

### 95c — Homomorphic encapsulation (CT5)
**Files:** `src/codegen/src/ir.rs`

1. `HomomorphicOp` — marks operations that preserve encapsulation across transformation
2. `rg 'Homomorphic' src/codegen/src/ir.rs` ≥1 match

### 95d — CSL-Perm fractional permissions (CT7) + Noise channels (CT8)
**Files:** `src/codegen/src/ir.rs`, `src/ive/src/borrow_region.rs`

1. `CslPerm` — fractional permission tracking (1/n of a capability)
2. `NoiseChannel` — authenticated encryption channel type
3. `rg 'CslPerm|NoiseChannel' src/codegen/src/ir.rs src/ive/src/borrow_region.rs` ≥1 match

### 95e — Linear type tests
**Files:** `tests/gold_standard/ipc/linear_valid.vuma`, `tests/gold_standard/ipc/linear_invalid.vuma`

1. `linear_valid.vuma` — each variable used once → compiles. Exit 1.
2. `linear_invalid.vuma` — variable used twice → compile ERROR (not silent pass)

**DoD (Wave 95):**
- [ ] `LinearType` exists in `borrow_region.rs`
- [ ] `linear_check` is CALLED from `src/pipeline.rs` (not library-only)
- [ ] `linear_valid.vuma` compiles and exits 1
- [ ] `linear_invalid.vuma` produces a compile error
- [ ] `rg 'Homomorphic|CslPerm|NoiseChannel' src/codegen/src/ir.rs src/ive/src/borrow_region.rs` ≥1 match each

---

# Phase 10 — Formal Verification (Wave 96)

**Spec refs:** §29–33, §132 · **Target backends:** n/a (compile-time, must be WIRED into pipeline)

## Wave 96: L1–L3 Verification + 5→3 Invariant Collapse

### 96a — Define L1L3Collapse proof
**Files:** `src/ive/src/verification.rs`, `src/ive/src/invariant_aggregator.rs`

1. `L1L3Collapse` struct: `collapsed: bool`, `folded_checks: usize`, `invariants: Vec<Invariant>`
2. `l1l3_collapse(scg) -> L1L3Collapse` — scans SCG for runtime-checked invariants (L1: channel framing, CRC, cap, protocol) and proves they collapse into compile-time-known invariants (L3: type-hash equality, structural cap-id checks)
3. `collapse_proof(scg)` — alias for `l1l3_collapse`
4. `rg 'L1L3|InvariantCollapse|collapse_proof' src/ive/src/verification.rs` ≥1 match

### 96b — 5→3 invariant reduction
**Files:** `src/ive/src/invariant_aggregator.rs`

1. `reduce_5to3(invariants: Vec<Invariant>) -> Vec<Invariant>` — reduces 5-level invariant hierarchy to 3 levels
2. `rg '5to3|reduce_5to3' src/ive/src/invariant_aggregator.rs` ≥1 match

### 96c — Wire the collapse proof into the pipeline
**Files:** `src/pipeline.rs`

1. After SCG construction, call `l1l3_collapse(scg)` and report the result
2. If `collapsed == false`, emit a warning (programs with non-collapsible invariants)
3. **MUST be called from `src/pipeline.rs`** (not just library code)
4. `rg 'l1l3_collapse|collapse_proof' src/pipeline.rs` ≥1 match (pipeline call-site)

### 96d — Formal verification test
**Files:** `tests/gold_standard/ipc/formal_verify.vuma`

1. Program where all L1 checks have compile-time-known arguments → collapse succeeds. Exit 1.

### 96e — Formal verification failure test
**Files:** `tests/gold_standard/ipc/formal_verify_fail.vuma`

1. Program with non-collapsible runtime invariants → collapse reports `collapsed: false`. The compiler emits a warning but still compiles. Exit 1.

**DoD (Wave 96):**
- [ ] `L1L3Collapse` / `l1l3_collapse` exists in `verification.rs`
- [ ] `l1l3_collapse` is CALLED from `src/pipeline.rs` (not library-only)
- [ ] `5to3` reduction exists in `invariant_aggregator.rs`
- [ ] `formal_verify.vuma` → exit 1
- [ ] `rg 'L1L3|InvariantCollapse|5to3|collapse_proof' src/ive/src/verification.rs src/ive/src/invariant_aggregator.rs` ≥1 match

---

# Appendix A — Machine-Checkable Acceptance Criteria Summary

Each criterion is checked by a command that inspects **emitted code** or
**parser/IR constructs**, NOT comments or imports.

## Phase 0 (Cleanup)
- [ ] No `.vuma` test has stripped-body md5 = `ec6eb67ebb89132ebe877b0fa017dbb7`

## Phase 1 (Waves 1–24)
- [ ] `rg 'enum ChannelError' src/codegen/src/ir.rs` ≥1
- [ ] `rg 'ChannelRecvResult' src/codegen/src/ir.rs` ≥1
- [ ] `rg 'Ok.*Err|match.*channel_recv' src/parser/src/parser.rs` ≥1 (real code)
- [ ] `rg '0xEDB88320' src/codegen/src/x86_64/stack_slot_isel.rs` ≥1
- [ ] `rg '0xEDB88320|crc32' src/codegen/src/ipc_lowering.rs` ≥1 (non-x86_64 has real CRC)
- [ ] `rg 'type_hash|0x414D5556' src/codegen/src/emit.rs` ≥1 (aarch64)
- [ ] `rg 'type_hash|0x414D5556' src/codegen/src/riscv64.rs` ≥1
- [ ] `rg 'type_hash|0x414D5556' src/codegen/src/arm32/mod.rs` ≥1
- [ ] `rg 'type_hash|0x414D5556' src/codegen/src/loongarch64/stack_slot_isel.rs` ≥1
- [ ] `rg 'ipc|channel_send|recv_timeout' Makefile` ≥1
- [ ] NO `// TODO: real CRC32 loop` or `// simplified` in `ipc_lowering.rs`
- [ ] `simple_send`=42, `ping_pong`=84, `multi_message`=63, `try_recv`=77, `recv_timeout`=88, `match_recv`=42 on **all 5 backends**

## Phase 2–6 (Waves 25–88)
- [ ] `rg 'process_call|FfiEnvelope|extern.*process' src/codegen/src/scg_to_ir.rs` ≥1 (Wave 25)
- [ ] `rg 'process_call' src/codegen/src/ipc_lowering.rs` ≥1 (non-x86_64 path)
- [ ] `rg 'delegate_capability' src/codegen/src/capability.rs` ≥1 (real function, not `pub use`)
- [ ] `rg 'supervisor_call' src/codegen/src/x86_64/stack_slot_isel.rs` ≥1
- [ ] `rg 'supervisor_call' src/codegen/src/ipc_lowering.rs` ≥1 (NOT a `return 0` stub)
- [ ] `rg 'driver_register|driver_call' src/codegen/src/x86_64/stack_slot_isel.rs` ≥1
- [ ] `rg 'driver_register|driver_call' src/codegen/src/ipc_lowering.rs` ≥1
- [ ] `rg 'circuit_breaker' src/codegen/src/x86_64/stack_slot_isel.rs` ≥1
- [ ] `rg 'circuit_breaker_call' src/codegen/src/ipc_lowering.rs` ≥1 (NOT a `return 0` stub)
- [ ] `rg 'hot_swap_trigger' src/codegen/src/x86_64/stack_slot_isel.rs` ≥1
- [ ] `rg 'hot_swap_trigger' src/codegen/src/ipc_lowering.rs` ≥1 (NOT a `return 1` stub)
- [ ] `rg 'channel_open_remote|remote_send|remote_recv' src/codegen/src/x86_64/stack_slot_isel.rs` ≥1
- [ ] `rg 'channel_open_remote|remote_send|remote_recv' src/codegen/src/ipc_lowering.rs` ≥1

## Phase 7–10 (Waves 89–96)
- [ ] `rg 'SessionType' src/parser/src/ast.rs` ≥1
- [ ] `rg 'SessionType' src/codegen/src/ir.rs` ≥1
- [ ] `rg 'session_type_check|SessionType' src/pipeline.rs` ≥1 (MUST be called from pipeline)
- [ ] `rg 'SecurityLabel|InformationFlow' src/parser/src/ast.rs` ≥1
- [ ] `rg 'SecurityLabel|InformationFlow' src/codegen/src/ir.rs` ≥1
- [ ] `rg 'information_flow_check|SecurityLabel' src/pipeline.rs` ≥1 (MUST be called from pipeline)
- [ ] `rg 'StarkProof|zk_stark' src/codegen/src/ir.rs` ≥1
- [ ] `rg 'stark_prove|stark_verify' src/codegen/src/ipc_lowering.rs` ≥1 (NOT stubs)
- [ ] `rg 'LinearType|linear_check' src/ive/src/borrow_region.rs` ≥1
- [ ] `rg 'linear_check' src/pipeline.rs` ≥1 (MUST be called from pipeline)
- [ ] `rg 'L1L3|InvariantCollapse|collapse_proof' src/ive/src/verification.rs` ≥1
- [ ] `rg 'l1l3_collapse|collapse_proof' src/pipeline.rs` ≥1 (MUST be called from pipeline)
- [ ] `rg '5to3|reduce_5to3' src/ive/src/invariant_aggregator.rs` ≥1

## Final
- [ ] `cargo build --workspace` green
- [ ] `cargo test -p vuma-codegen --lib` passes (existing + new tests)
- [ ] Gold-standard regression suite passes on **all 5 backends**
- [ ] `/home/z/my-project/worklog.md` has a section for every wave touched

---

# Appendix B — Backend Test Matrix

Every IPC test must pass on all 5 backends.  Run with:

```bash
for backend in x86_64 aarch64 riscv64 arm32 loongarch64; do
  for test in tests/gold_standard/ipc/*.vuma; do
    ./target/debug/compile_dump "$test" /tmp/test.bin "$backend"
    case $backend in
      x86_64)       /tmp/test.bin ;;
      aarch64)      qemu-aarch64-static /tmp/test.bin ;;
      riscv64)      qemu-riscv64-static /tmp/test.bin ;;
      arm32)        qemu-arm-static /tmp/test.bin ;;
      loongarch64)  qemu-loongarch64-static /tmp/test.bin ;;
    esac
    echo "$backend $(basename $test): exit=$?"
  done
done
```

Expected exit codes for the core regression suite (must match on ALL 5 backends):

| Test | Expected | Exercises |
|------|----------|-----------|
| `simple_send` | 42 | Channel open/send/recv + spawn/wait |
| `ping_pong` | 84 | Bidirectional IPC |
| `multi_message` | 63 | Multiple framed messages in order |
| `try_recv` | 77 | Non-blocking recv |
| `recv_timeout` | 88 | Poll-based timeout |
| `match_recv` | 42 | `match channel_recv { Ok/Err }` (Wave 8b) |
| `framed_send_recv` | 42 | L1 framing + CRC32 verification |
| `capability_grant_verify` | 42 | L2 capability grant + verify |
| `protocol_valid` | 50 | L4 protocol FSM valid transition |
| `protocol_invalid` | 99 | L4 protocol FSM rejection path |
| `shared_memory_rw` | 1 | L3 shared memory read+write |
| `memory_limit` | 1 | L5 set_memory_limit |
| `stark_proof` | 1 | L8 STARK proof generation |
| `ffi_basic` | 42 | FFI process_call |
| `supervisor` | 1 | Kernel/user supervisor_call |
| `driver_isolation` | 42 | Driver register + call |
| `fault_tolerance` | 1 | Circuit breaker |
| `hot_swap` | 1 | Hot-swap trigger |
| `distributed` | 1 | Remote channel |

---

*End of TASKS.md*
