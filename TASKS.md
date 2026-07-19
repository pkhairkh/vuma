# VUMA Process Isolation Architecture — Wave-Based Implementation Plan

> **Source specification:** `docs/VUMA_PROCESS_ISOLATION_SPEC.md` v2.0 (43,788 lines)
> **Total waves:** 96
> **Subtasks per wave:** 4-12 (but max 4 launched simultaneously)
> **Each wave's subtasks touch independent files** — no conflicts

## Rules

1. **Max 4 subagents launched simultaneously per wave.** If a wave has more than
   4 subtasks, they are split into batches of 4, launched sequentially within the wave.
2. **Independent code domains.** Subtasks in the same batch MUST NOT edit the same file.
3. **Surgical prompts.** Each subagent prompt is self-contained — includes exact files,
   step-by-step instructions, acceptance criteria, and test commands. The subagent does
   NOT need to read the full 43k-line spec.
4. **DoD approval per wave.** The main agent verifies the Definition of Done before
   proceeding to the next wave. If any DoD item fails, the wave is incomplete.
5. **Git protocol.** Main agent: pull → dispatch wave → verify DoD → commit → push.
   Subagents: edit files + append worklog only. NEVER run git.
6. **Build gate.** `cargo build --workspace` MUST succeed after every wave.
7. **Test gate.** Existing gold-standard tests MUST still pass after every wave.
8. **Kernel exclusion.** Subagents MUST NOT touch `womb/kernel/**`.
9. **QEMU verification.** Subagents test on QEMU where the spec requires it.
10. **Worklog.** Each subagent appends to `/home/z/my-project/worklog.md` with Task ID.
11. **Prompt length.** Each subagent prompt MUST be under 2000 words to avoid context
    overflow and timeouts. Be surgical.
12. **Commit granularity.** One commit per wave (not per subtask).

## Wave Map (96 waves)

| Waves | Phase | Spec Parts | Title |
|-------|-------|------------|-------|
| 1-8 | Phase 1 | II, VIII | IPC Primitives: Channel<T> type, spawn_worker, pipe transport, lifecycle, deadlock detection |
| 9-16 | Phase 1 | III (L1-L4) | Runtime Encapsulation L1-L4: message framing, capability tokens, memory windows, protocol state machines |
| 17-24 | Phase 1 | III (L5-L8) | Runtime Encapsulation L5-L8: worker sandboxing, state checkpointing, error containment, AEAD crypto |
| 25-32 | Phase 2 | X | FFI Process Isolation: extern "process", auto-marshalling, seccomp, crash recovery, perf optimization |
| 33-40 | Phase 4 | IX | Capability System: type, grant/revoke, delegation chain, flow verification, revocation propagation |
| 41-48 | Phase 3 | XI | Kernel/User Split: microkernel architecture, syscall-as-IPC, kernel/user process, resource accounting |
| 49-56 | Phase 5 | XII | Driver Isolation: driver worker, MMIO caps, IRQ channels, DMA buffers, driver restart |
| 57-64 | Phase 2 | XIII | Sandboxing: sandbox architecture, zero-cap workers, plugins, parsers, sandboxed crypto |
| 65-72 | Phase 2 | XIV | Fault Tolerance: supervisor, crash detection, checkpointing, restart, graceful degradation, circuit breaker |
| 73-80 | Phase 2 | XV | Hot Reloading: hot-swap, state transfer, version management, rollback |
| 81-88 | Phase 6 | XVI | Distributed Channels: location-transparent, discovery, network protocol, failure detection, consensus |
| 89-92 | Phase 7 | IV (CT1-CT2) | Compile-Time: session types, information-flow types |
| 93-94 | Phase 8 | IV (CT6), VI | Compile-Time: zk-STARK attestation architecture |
| 95 | Phase 9 | IV (CT2) | Compile-Time: information-flow types (security lattices) |
| 96 | Phase 10 | V | Formal Verification: L1-L3 verification + 5→3 invariant collapse proof |

---


## Wave 1: Channel<T> Type — IR + Type System

**Spec references:** Spec §7, §8, §45, §101
**Scope:** Add Channel<T> as a first-class type in VUMA's IR, SCG, parser, and pipeline bridge. This is the foundation — every subsequent wave builds on typed channels.
**Max parallel:** 4 (never more than 4)

### 1a — Add Channel<T> to ScgType and IRType enums

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 1a
Add Channel<T> to ScgType and IRType

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add `Channel(Box<ScgType>)` variant to the ScgType enum (around line 170)
2. Add `Channel(Box<IRType>)` variant to the IRType enum (around line 50)
3. Add Channel mapping in ScgType::to_ir_type() (return IRType::Channel(Box::new(inner.to_ir_type())))
4. Add Channel in ScgType::size() (pointer-sized: 8 on 64-bit, 4 on 32-bit)
5. Add Channel in IRType::size() (same)
6. Add Display impl for both Channel variants: format as "Channel<{}>"
7. Add Channel to the substitute_value function in opt.rs (pass through)
8. Add Channel to has_side_effects in opt.rs (no side effects)
9. Build: cargo build --workspace
10. Test: cargo test -p vuma-codegen --lib

Acceptance:
- cargo build --workspace succeeds
- Channel<T> variant exists in both ScgType and IRType
- to_ir_type maps Channel correctly
- size_of returns pointer size
- Display shows Channel<i32> format

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 1b — Add Channel type to parser AST and lexer

**Files:** `src/parser/src/ast.rs, src/parser/src/parser.rs, src/parser/src/lexer.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 1b
Add Channel<T> to parser

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/parser/src/ast.rs, src/parser/src/parser.rs, src/parser/src/lexer.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add TokenKind::Channel to lexer (keyword 'Channel')
2. Add Type::Channel(Box<Type>) to ast.rs Type enum
3. In parser.rs parse_type(): after parsing BDBase/Ptr/Array, check for 'Channel' keyword
4. Parse 'Channel<T>' as Type::Channel(Box::new(parse_type()))
5. Add Display for Type::Channel: write!(f, "Channel<{}>", inner)
6. Add Type::Channel to the fmt_type helper used by LSP
7. Add Type::Channel to bridge_type_to_codegen_scg in pipeline.rs
8. Add Type::Channel to bridge_type_to_ir_type in pipeline.rs
9. Add Type::Channel to type_size and type_alignment in pipeline.rs
10. Build: cargo build --workspace
11. Test: echo 'fn main() -> i32 { ch: Channel<i32>; return 0; }' should parse

Acceptance:
- Channel<T> parses as a type
- Display shows Channel<i32>
- cargo build --workspace succeeds
- Test: Channel<i32> variable declaration compiles

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 1c — Bridge Channel type through pipeline

**Files:** `src/pipeline.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 1c
Bridge Channel<T> through pipeline

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/pipeline.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. In bridge_type_to_codegen_scg: add Type::Channel(inner) → ScgType::Channel(Box::new(bridge_type_to_codegen_scg(&Some(*inner))))
2. In bridge_type_to_ir_type: add Channel mapping
3. In type_size: Channel is pointer-sized (8 on 64-bit, 4 on 32-bit)
4. In type_alignment: same as pointer (8 on 64-bit, 4 on 32-bit)
5. In flatten_expr: Channel values are opaque handles, no special handling needed
6. In bridge_stmt_to_scg: Channel variables are just vregs (pointer-sized)
7. Build: cargo build --workspace

Acceptance:
- Channel type bridges correctly AST → SCG → IR
- type_size returns pointer size
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 1d — Add Channel<T> to LSP and test infrastructure

**Files:** `src/lsp/mod.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 1d
Add Channel<T> to LSP

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/lsp/mod.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. In format_type: add Type::Channel(inner) → format!("Channel<{}>", format_type(inner))
2. Add hover text for Channel type: "Typed IPC channel for inter-process communication"
3. Add completion for 'Channel' keyword
4. Add a test: test_format_type_channel
5. Build: cargo build --workspace
6. Test: cargo test -p vuma --lib lsp

Acceptance:
- LSP shows Channel<i32> with hover text
- Completion suggests 'Channel'
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 1):**

- [ ] cargo build --workspace succeeds with zero errors
- [ ] Channel<T> type exists in ScgType, IRType, parser AST, and pipeline bridge
- [ ] Channel<i32> variable declaration compiles: `let ch: Channel<i32>`
- [ ] LSP shows Channel type info with hover text
- [ ] All existing gold-standard tests still pass (no regressions)

---


## Wave 2: Channel<T> — IR Instructions for Channel Operations

**Spec references:** Spec §45, §46, §47
**Scope:** Add IR instructions for channel operations (open, send, recv, close) and lower them from SCG.
**Max parallel:** 4 (never more than 4)

### 2a — Add ChannelOpen/Send/Recv/Close IR instructions

**Files:** `src/codegen/src/ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 2a
Add ChannelOpen/Send/Recv/Close IR instructions

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::ChannelOpen { dst: IRValue, elem_ty: IRType } — allocates a channel
2. Add IRInstr::ChannelSend { ch: IRValue, msg: IRValue, ty: Option<IRType> } — sends a message
3. Add IRInstr::ChannelRecv { ch: IRValue, dst: IRValue, ty: Option<IRType> } — receives a message
4. Add IRInstr::ChannelClose { ch: IRValue } — deallocates a channel
5. Add Display impls for all 4 (format as 'channel_open', 'channel_send', etc.)
6. Add to the effects analysis (ChannelSend writes, ChannelRecv reads, both have side effects)
7. Add to has_side_effects: all 4 return true
8. Add to substitute_value: pass through for ch/dst, substitute for msg
9. Add to instr_reads/instr_writes: ChannelSend reads ch+msg, writes nothing; ChannelRecv reads ch, writes dst
10. Build: cargo build --workspace

Acceptance:
- 4 new IR instructions exist with Display impls
- Effects analysis marks them as having side effects
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 2b — Add channel operation SCG nodes

**Files:** `src/scg/src/node.rs, src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 2b
Add channel operation SCG nodes

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/scg/src/node.rs, src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add ChannelOpenNode { dst: String, elem_type: ScgType } to NodePayload
2. Add ChannelSendNode { channel: String, message: ScgExpr, ty: ScgType } to NodePayload
3. Add ChannelRecvNode { dst: String, channel: String, ty: ScgType } to NodePayload
4. Add ChannelCloseNode { channel: String } to NodePayload
5. Add NodeType::ChannelOpen/Send/Recv/Close
6. In scg_to_ir.rs lower_statements: add arms for each new node type
7. ChannelOpen → IRInstr::ChannelOpen (allocate vreg for dst)
8. ChannelSend → IRInstr::ChannelSend (resolve ch and msg to IRValues)
9. ChannelRecv → IRInstr::ChannelRecv (allocate vreg for dst)
10. ChannelClose → IRInstr::ChannelClose (resolve ch to IRValue)
11. Build: cargo build --workspace

Acceptance:
- SCG nodes exist for all 4 channel operations
- IR lowering produces correct IRInstr variants
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 2c — Parse channel builtins in VUMA source

**Files:** `src/parser/src/parser.rs, src/parser/src/to_scg.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 2c
Parse channel builtins in VUMA source

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/parser/src/parser.rs, src/parser/src/to_scg.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. In parser.rs parse_primary_expr: add channel_open, channel_send, channel_recv, channel_close as builtin function calls
2. channel_open<T>() returns Channel<T>
3. channel_send(ch, val) takes Channel<T> and T, returns void
4. channel_recv(ch) takes Channel<T>, returns T
5. channel_close(ch) takes Channel<T>, returns void
6. In to_scg.rs: lower these builtins to ChannelOpenNode/SendNode/RecvNode/CloseNode
7. channel_open needs type parameter: parse Channel<i32> syntax after the function name
8. Build: cargo build --workspace
9. Test: echo 'fn main() -> i32 { ch = channel_open<i32>(); channel_send(ch, 42); x = channel_recv(ch); channel_close(ch); return x; }' compiles

Acceptance:
- channel_open/send/recv/close parse as builtin calls
- They lower to correct SCG nodes
- The test program compiles
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 2d — Add channel operations to IR optimizer

**Files:** `src/codegen/src/opt.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 2d
Add channel operations to IR optimizer

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/opt.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add ChannelOpen/Send/Recv/Close to the e-graph expression key extraction (or return None — they have side effects, can't be CSE'd)
2. Add to DCE: these instructions have side effects, never eliminate
3. Add to constant folding: skip (side effects)
4. Add to the IR printer/debug formatter
5. Add to cross_function_constant_prop: channels are not constants
6. Build: cargo build --workspace

Acceptance:
- Optimizer correctly handles channel instructions (doesn't eliminate or fold them)
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 2):**

- [ ] cargo build --workspace succeeds
- [ ] IR has ChannelOpen/Send/Recv/Close instructions with Display + effects
- [ ] SCG has corresponding nodes with correct lowering
- [ ] Parser recognizes channel_open/send/recv/close builtins
- [ ] Test program: open, send 42, recv, close — compiles successfully
- [ ] All existing tests still pass

---


## Wave 3: Channel<T> — Hosted Backend (x86_64) Implementation

**Spec references:** Spec §45, §117
**Scope:** Implement channel operations on x86_64 using pipe() syscall for IPC transport. This is the reference implementation — other backends will follow the same pattern.
**Max parallel:** 4 (never more than 4)

### 3a — Implement ChannelOpen on x86_64 (pipe-based)

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 3a
Implement ChannelOpen on x86_64 (pipe-based)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::ChannelOpen handler in the isel match
2. Emit pipe() syscall (x86_64 sys_pipe=22, returns two fds in a struct)
3. Store read_fd and write_fd in the dst vreg's stack slot (8 bytes: read_fd in low 4, write_fd in high 4)
4. The channel handle is the combined 8-byte value
5. Build: cargo build --workspace
6. Test: compile a program that calls channel_open and check the binary has a pipe syscall

Acceptance:
- ChannelOpen emits pipe() syscall on x86_64
- Channel handle is 8 bytes (read_fd:write_fd)
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 3b — Implement ChannelSend on x86_64 (write to pipe)

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 3b
Implement ChannelSend on x86_64 (write to pipe)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::ChannelSend handler
2. Load channel handle from ch vreg's stack slot
3. Extract write_fd (high 32 bits of the 8-byte handle)
4. Load message value into RAX
5. Emit write(write_fd, &RAX, sizeof(ty)) syscall (x86_64 sys_write=1)
6. The message is written as raw bytes (little-endian) to the pipe
7. Build: cargo build --workspace
8. Test: compile and run `fn main() -> i32 { ch = channel_open<i32>(); channel_send(ch, 42); return 0; }` on x86_64

Acceptance:
- ChannelSend emits write() syscall with correct fd
- Message is serialized as raw bytes
- Test program runs without crash on x86_64
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 3c — Implement ChannelRecv on x86_64 (read from pipe)

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 3c
Implement ChannelRecv on x86_64 (read from pipe)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::ChannelRecv handler
2. Load channel handle, extract read_fd (low 32 bits)
3. Emit read(read_fd, &dst_slot, sizeof(ty)) syscall (x86_64 sys_read=0)
4. Store received bytes into dst vreg's stack slot
5. Build: cargo build --workspace
6. Test: `fn main() -> i32 { ch = channel_open<i32>(); channel_send(ch, 42); x = channel_recv(ch); channel_close(ch); return x; }` returns 42 on x86_64

Acceptance:
- ChannelRecv emits read() syscall with correct fd
- Received bytes are stored in dst slot
- Test: send 42, recv 42, return 42 — exits with 42
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 3d — Implement ChannelClose on x86_64 (close fds)

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 3d
Implement ChannelClose on x86_64 (close fds)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::ChannelClose handler
2. Load channel handle, extract both read_fd and write_fd
3. Emit close(read_fd) and close(write_fd) syscalls (x86_64 sys_close=3)
4. Build: cargo build --workspace
5. Test: open, send, recv, close — no fd leak (check with /proc/self/fd or just verify no crash)

Acceptance:
- ChannelClose emits close() for both fds
- No fd leaks
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 3):**

- [ ] cargo build --workspace succeeds
- [ ] Channel open/send/recv/close work on x86_64 via pipe()
- [ ] Test: send 42 via channel, receive 42, return 42 — exit code 42
- [ ] Channel handles are 8-byte (read_fd:write_fd) packed values
- [ ] All existing tests still pass

---


## Wave 4: Channel<T> — Cross-Backend Support (aarch64, riscv64, arm32, wasm32)

**Spec references:** Spec §45
**Scope:** Port channel operations to aarch64, riscv64, arm32, and wasm32 backends. Same pipe-based approach as x86_64.
**Max parallel:** 4 (never more than 4)

### 4a — Channel ops on aarch64

**Files:** `src/codegen/src/emit.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 4a
Channel ops on aarch64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/emit.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::ChannelOpen/Send/Recv/Close handlers in ss_emit_instr
2. pipe() on aarch64 is sys_pipe2=59 (or pipe via svc #0 with x8=59)
3. write() is sys_write=64, read() is sys_read=63, close() is sys_close=57
4. Channel handle is 8 bytes (two 4-byte fds)
5. Use the same serialization (raw LE bytes) as x86_64
6. Build: cargo build --workspace
7. Test: compile + run channel send/recv test on qemu-aarch64-static

Acceptance:
- Channel send/recv works on aarch64
- Test: send 42, recv 42, return 42 on QEMU-aarch64
QEMU test: qemu-aarch64-static /tmp/test.bin

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 4b — Channel ops on riscv64

**Files:** `src/codegen/src/riscv64.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 4b
Channel ops on riscv64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/riscv64.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add ChannelOpen/Send/Recv/Close handlers
2. pipe() on riscv64 is sys_pipe=21 (returns two fds via pointer)
3. write=64, read=63, close=57
4. Same serialization
5. Build: cargo build --workspace
6. Test: compile + run on qemu-riscv64-static

Acceptance:
- Channel send/recv works on riscv64
- Test passes on QEMU-riscv64
QEMU test: qemu-riscv64-static /tmp/test.bin

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 4c — Channel ops on arm32

**Files:** `src/codegen/src/arm32/mod.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 4c
Channel ops on arm32

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/arm32/mod.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add ChannelOpen/Send/Recv/Close handlers
2. pipe() on arm32 is sys_pipe=42 (via SWI 0 with r7=42)
3. write=4, read=3, close=6
4. Channel handle is 8 bytes (two 4-byte fds) but pointer is 4 bytes on arm32 — store fds as two separate 4-byte values
5. Build: cargo build --workspace
6. Test: compile + run on qemu-arm-static

Acceptance:
- Channel send/recv works on arm32
- Test passes on QEMU-arm
QEMU test: qemu-arm-static /tmp/test.bin

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 4d — Channel ops on wasm32 (compile-only)

**Files:** `src/codegen/src/wasm32/mod.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 4d
Channel ops on wasm32 (compile-only)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/wasm32/mod.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add ChannelOpen/Send/Recv/Close handlers
2. On wasm32, pipe() doesn't exist — use linear memory as channel buffer
3. ChannelOpen allocates a ring buffer in linear memory
4. ChannelSend writes to ring buffer head
5. ChannelRecv reads from ring buffer tail
6. ChannelClose deallocates ring buffer
7. Build: cargo build --workspace
8. Verify compile succeeds (no wasm runtime needed for testing)

Acceptance:
- Channel ops compile on wasm32
- Ring buffer approach for wasm
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 4):**

- [ ] cargo build --workspace succeeds
- [ ] Channel send/recv works on x86_64, aarch64, riscv64, arm32
- [ ] Channel ops compile on wasm32 (ring buffer approach)
- [ ] Test: send 42, recv 42, return 42 on all 4 backends
- [ ] All existing tests still pass

---


## Wave 5: spawn_worker — Process Spawning

**Spec references:** Spec §8, §56, §103
**Scope:** Add spawn_worker builtin that creates a child process running a separate VUMA program. The child process gets its own address space (MMU isolation).
**Max parallel:** 4 (never more than 4)

### 5a — Add spawn_worker builtin to parser + SCG + IR

**Files:** `src/parser/src/parser.rs, src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs, src/scg/src/node.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 5a
Add spawn_worker builtin to parser + SCG + IR

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/parser/src/parser.rs, src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs, src/scg/src/node.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::SpawnWorker { dst: IRValue, path: String } to IR
2. Add SpawnWorkerNode to SCG
3. Parse `spawn_worker("path")` as a builtin call
4. The path is a string literal pointing to a compiled VUMA binary
5. Lower to IRInstr::SpawnWorker
6. The dst vreg receives the worker's PID (i64)
7. Build: cargo build --workspace
8. Test: `fn main() -> i32 { pid = spawn_worker("worker.bin"); return 0; }` compiles

Acceptance:
- spawn_worker builtin exists in parser, SCG, and IR
- Returns PID as i64
- Test program compiles

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 5b — Implement spawn_worker on x86_64 (fork+exec)

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 5b
Implement spawn_worker on x86_64 (fork+exec)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::SpawnWorker handler
2. Emit fork() syscall (sys_fork=57)
3. In child (rax==0): call execve(path, argv, envp) — sys_execve=59
4. In parent (rax>0): store child PID in dst vreg's stack slot
5. The path string needs to be in memory — store it as a null-terminated string in the data section
6. Build: cargo build --workspace
7. Test: spawn a worker that exits 42, parent calls wait_worker and gets 42

Acceptance:
- spawn_worker creates a child process via fork+exec
- Parent receives child PID
- Test: spawn worker, worker exits 42
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 5c — Add wait_worker and kill_worker builtins

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs, src/parser/src/parser.rs, src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 5c
Add wait_worker and kill_worker builtins

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs, src/parser/src/parser.rs, src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::WaitWorker { pid: IRValue, dst: IRValue } — waitpid syscall (sys_wait4=61)
2. Add IRInstr::KillWorker { pid: IRValue } — kill syscall (sys_kill=62)
3. Parse wait_worker(pid) and kill_worker(pid) as builtins
4. Lower to IR instructions
5. Implement on x86_64: waitpid(pid, &status, 0, NULL), kill(pid, SIGTERM=15)
6. Build: cargo build --workspace
7. Test: spawn worker, wait for it, check exit code

Acceptance:
- wait_worker reaps child process and returns exit code
- kill_worker sends SIGTERM to child
- Test: spawn → wait → check exit code
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 5d — Add worker_handle type to type system

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs, src/parser/src/ast.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 5d
Add worker_handle type to type system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs, src/parser/src/ast.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add WorkerHandle type (i64 alias — represents a PID)
2. spawn_worker returns WorkerHandle
3. wait_worker takes WorkerHandle, returns i32 (exit code)
4. kill_worker takes WorkerHandle
5. Add to ScgType and IRType as an alias for I64
6. Add to parser: `WorkerHandle` as a type keyword
7. Build: cargo build --workspace

Acceptance:
- WorkerHandle type exists (aliased to i64)
- spawn/wait/kill use WorkerHandle
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 5):**

- [ ] cargo build --workspace succeeds
- [ ] spawn_worker creates child process on x86_64 (fork+exec)
- [ ] wait_worker returns child exit code
- [ ] kill_worker terminates child process
- [ ] WorkerHandle type exists in type system
- [ ] Test: spawn worker → worker exits 42 → parent waits → gets 42
- [ ] All existing tests still pass

---


## Wave 6: IPC Channel — Process-to-Process Communication

**Spec references:** Spec §45, §117, §56
**Scope:** Connect spawn_worker with Channel<T> so parent and child can communicate via typed IPC channels. The parent creates a channel, spawns a worker, and the worker inherits the channel endpoints.
**Max parallel:** 4 (never more than 4)

### 6a — Pass channel handle to child process

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 6a
Pass channel handle to child process

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Before spawn_worker (fork), create a pipe (channel_open)
2. After fork, in the child: close the write end of the pipe (keep read end)
3. In the parent: close the read end of the pipe (keep write end)
4. Parent uses channel_send (write to pipe), child uses channel_recv (read from pipe)
5. For bidirectional communication: create TWO pipes (one each direction)
6. Pass the read fd of pipe1 and write fd of pipe2 to the child via exec argv
7. Build: cargo build --workspace
8. Test: parent sends 42, child receives 42

Acceptance:
- Channel fds are passed from parent to child via fork inheritance
- Parent sends, child receives (unidirectional initially)
- Test: parent sends 42, child receives 42 and exits with 42
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 6b — Bidirectional IPC channel

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 6b
Bidirectional IPC channel

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. ChannelOpen creates TWO pipes (pipe1: parent→child, pipe2: child→parent)
2. Channel handle is 16 bytes: (read_fd1, write_fd1, read_fd2, write_fd2)
3. After fork: parent keeps write_fd1 + read_fd2, child keeps read_fd1 + write_fd2
4. ChannelSend writes to the appropriate pipe based on process role (parent vs child)
5. ChannelRecv reads from the appropriate pipe
6. Build: cargo build --workspace
7. Test: parent sends 42, child receives 42, child sends 84, parent receives 84

Acceptance:
- Bidirectional channel works (parent↔child)
- Test: parent sends 42, child echoes back 84, parent receives 84
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 6c — IPC test: ping-pong between parent and child

**Files:** `tests/gold_standard/ipc/ (new directory)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 6c
IPC test: ping-pong between parent and child

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ipc/ (new directory)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create tests/gold_standard/ipc/ping_pong.vuma
2. Parent spawns worker, sends 42, child receives 42, sends 84 back, parent receives 84, returns 84
3. // Expected exit code: 84
4. Create tests/gold_standard/ipc/worker_echo.vuma — the child program that echoes messages
5. Build: cargo build --workspace
6. Run on x86_64

Acceptance:
- ping_pong test exists and passes on x86_64
- worker_echo.vuma exists as the child program

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 6d — IPC channel on aarch64

**Files:** `src/codegen/src/emit.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 6d
IPC channel on aarch64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/emit.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Port the bidirectional IPC channel to aarch64
2. pipe() on aarch64: sys_pipe2=59
3. write=64, read=63, close=57
4. Same fd inheritance via fork
5. Build: cargo build --workspace
6. Test: ping_pong test on qemu-aarch64-static

Acceptance:
- Bidirectional IPC works on aarch64
- ping_pong test passes on QEMU-aarch64
QEMU test: qemu-aarch64-static

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 6):**

- [ ] cargo build --workspace succeeds
- [ ] Bidirectional IPC channel works on x86_64 (parent↔child via two pipes)
- [ ] ping_pong test: parent sends 42, child echoes 84, parent returns 84
- [ ] IPC channel works on aarch64
- [ ] All existing tests still pass

---


## Wave 7: IPC Channel — Remaining Backends + Lifecycle

**Spec references:** Spec §45, §48
**Scope:** Port IPC to riscv64 and arm32. Add channel lifecycle management (auto-close, try_recv, is_closed).
**Max parallel:** 4 (never more than 4)

### 7a — IPC channel on riscv64

**Files:** `src/codegen/src/riscv64.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 7a
IPC channel on riscv64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/riscv64.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Port bidirectional IPC channel to riscv64
2. pipe=21, write=64, read=63, close=57
3. fork=220, exec=221, waitpid=2601
4. Build + test on qemu-riscv64-static

Acceptance:
- IPC works on riscv64
- ping_pong test passes on QEMU-riscv64
QEMU test: qemu-riscv64-static

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 7b — IPC channel on arm32

**Files:** `src/codegen/src/arm32/mod.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 7b
IPC channel on arm32

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/arm32/mod.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Port to arm32 (32-bit: fds are 4 bytes, handles need care)
2. pipe=42, write=4, read=3, close=6, fork=2, exec=11, waitpid=7
3. Build + test on qemu-arm-static

Acceptance:
- IPC works on arm32
- ping_pong test passes on QEMU-arm
QEMU test: qemu-arm-static

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 7c — Channel lifecycle: auto-close at function exit

**Files:** `src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 7c
Channel lifecycle: auto-close at function exit

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Track open channels in IRBuilder (HashSet of channel vreg IDs)
2. At function exit (before Ret), auto-emit ChannelClose for any unclosed channels
3. Emit a warning comment in the IR dump
4. Build: cargo build --workspace
5. Test: function that opens a channel but doesn't close it — channel is auto-closed

Acceptance:
- Open channels are auto-closed at function exit
- Warning is emitted in IR dump

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 7d — channel_try_recv (non-blocking) + channel_is_closed

**Files:** `src/parser/src/parser.rs, src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 7d
channel_try_recv (non-blocking) + channel_is_closed

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/parser/src/parser.rs, src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::ChannelTryRecv { ch, dst, ty } — non-blocking recv, returns 0 if no message, 1 if received
2. Add IRInstr::ChannelIsClosed { ch, dst } — checks if peer closed the channel
3. Parse channel_try_recv(ch) and channel_is_closed(ch) as builtins
4. On x86_64: use recv(fd, buf, len, MSG_DONTWAIT) for non-blocking
5. Or: use select/poll with 0 timeout
6. Build: cargo build --workspace
7. Test: try_recv on empty channel returns 0, after send returns 1

Acceptance:
- channel_try_recv is non-blocking
- channel_is_closed detects peer closure
- Test: try_recv on empty channel returns 0
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 7):**

- [ ] cargo build --workspace succeeds
- [ ] IPC channel works on x86_64, aarch64, riscv64, arm32
- [ ] Channel auto-close at function exit works
- [ ] channel_try_recv (non-blocking) works
- [ ] channel_is_closed detects peer closure
- [ ] All existing tests still pass

---


## Wave 8: Deadlock Detection + Channel Error Handling

**Spec references:** Spec §49, §50, §18
**Scope:** Add compile-time deadlock detection for channel usage and runtime error handling for channel operations.
**Max parallel:** 4 (never more than 4)

### 8a — Compile-time deadlock detection (wait-for graph)

**Files:** `src/codegen/src/opt.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 8a
Compile-time deadlock detection (wait-for graph)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/opt.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. New optimization pass: detect_deadlock()
2. Build a wait-for graph: if process A blocks on ChannelRecv from channel C, and the sender of C is process B, add edge A→B
3. For single-process programs: if a function does recv on channel C and the send to C is in a different branch that hasn't executed yet, flag potential deadlock
4. Detect cycles via DFS
5. Emit a warning (not error) if potential deadlock found
6. Build: cargo build --workspace
7. Test: program with circular channel wait → warning emitted

Acceptance:
- Deadlock detection pass exists
- Circular waits are detected and warned
- Test: circular wait → warning

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 8b — Channel error type + error handling

**Files:** `src/codegen/src/ir.rs, src/parser/src/ast.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 8b
Channel error type + error handling

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ir.rs, src/parser/src/ast.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add ChannelError enum to IR: Closed, Timeout, PermissionDenied, InvalidHandle
2. ChannelRecv can fail — add IRInstr::ChannelRecvResult { ch, dst, err_dst, ty } that returns both value and error
3. Parse `match channel_recv(ch) { Ok(v) => ..., Err(e) => ... }` syntax
4. Lower to ChannelRecvResult + conditional branch
5. Build: cargo build --workspace

Acceptance:
- Channel error type exists
- match on channel_recv result works
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 8c — Channel timeout support

**Files:** `src/codegen/src/ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 8c
Channel timeout support

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::ChannelRecvTimeout { ch, dst, ty, timeout_ms } — blocks for at most timeout_ms
2. On x86_64: use poll() with timeout (sys_poll=7)
3. If timeout expires, return error (Timeout)
4. Parse channel_recv_timeout(ch, 1000) builtin
5. Build: cargo build --workspace
6. Test: recv with 100ms timeout on empty channel → returns Timeout error

Acceptance:
- Channel timeout works on x86_64
- Test: recv with short timeout on empty channel → Timeout
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 8d — Channel integration tests

**Files:** `tests/gold_standard/ipc/`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 8d
Channel integration tests

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ipc/
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create tests/gold_standard/ipc/try_recv.vuma — non-blocking recv returns 0 on empty channel
2. Create tests/gold_standard/ipc/timeout.vuma — recv with timeout returns error
3. Create tests/gold_standard/ipc/multi_message.vuma — send 5 messages, recv 5 messages in order
4. Create tests/gold_standard/ipc/large_message.vuma — send large struct via channel
5. Each with // Expected exit code
6. Run on x86_64
7. Build: cargo build --workspace

Acceptance:
- 4 channel integration tests exist and pass on x86_64
- Tests cover: non-blocking, timeout, multi-message, large message

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 8):**

- [ ] cargo build --workspace succeeds
- [ ] Deadlock detection: warns on potential circular waits
- [ ] Channel error handling: match on Ok/Err works
- [ ] Channel timeout: recv with timeout returns Timeout error
- [ ] 4 IPC integration tests pass on x86_64
- [ ] All existing tests still pass

---


## Wave 9: Runtime Encapsulation L1 — Message Wire Format

**Spec references:** Spec §12.1, §12.2, §12.3
**Scope:** Implement the IPC message wire format with magic bytes, version, flags, channel ID, sequence number, type hash, payload, and CRC32 checksum.
**Max parallel:** 4 (never more than 4)

### 9a — Create IPC module with wire format

**Files:** `src/codegen/src/ipc.rs (new)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 9a
Create IPC module with wire format

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs (new)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create src/codegen/src/ipc.rs
2. Define MessageHeader struct: magic [u8;4], version u16, flags u16, channel_id u64, sequence u64, type_hash u64, payload_len u64, cap_count u32
3. Define MAGIC=[0x56,0x55,0x4D,0x41], VERSION=2, HEADER_SIZE=44
4. Define MessageFlags bitfield: ENCRYPTED, HAS_CAPS, HAS_SHM, IS_REPLY, IS_ERROR
5. Add module to src/codegen/src/lib.rs
6. Build: cargo build --workspace

Acceptance:
- IPC module exists
- MessageHeader defined
- MAGIC/VERSION constants exist

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 9b — Implement frame_message and deframe_message

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 9b
Implement frame_message and deframe_message

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement frame_message(msg: &EncapsulatedMessage) -> Vec<u8>
2. Write header fields in LE, then payload, then capabilities, then CRC32
3. Implement deframe_message(stream: &mut Read) -> Result<EncapsulatedMessage, FrameError>
4. Read magic, verify == MAGIC
5. Read version, verify == VERSION
6. Read header fields, payload, capabilities
7. Read CRC32, recompute, verify match
8. Define FrameError: BadMagic, UnsupportedVersion, PayloadTooLarge, CrcMismatch, TruncatedMessage
9. Build: cargo build --workspace

Acceptance:
- frame/deframe roundtrip works
- CRC mismatch detected
- Bad magic rejected

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 9c — Implement CRC32 and type hash

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 9c
Implement CRC32 and type hash

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement crc32(data: &[u8]) -> u32 (IEEE 802.3 polynomial 0xEDB88320)
2. Implement type_hash(ty: &ScgType) -> u64 using FNV-1a
3. canonical_type_string for all ScgType variants
4. For Struct: include name + field types in canonical string
5. Test: same type → same hash, different type → different hash
6. Build: cargo build --workspace

Acceptance:
- CRC32 is correct (matches standard)
- Type hash is deterministic
- Same type always produces same hash

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 9d — Unit tests for wire format

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 9d
Unit tests for wire format

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add #[cfg(test)] mod tests
2. test_frame_deframe_roundtrip: frame a message, deframe it, verify equality
3. test_bad_magic: wrong magic → BadMagic error
4. test_crc_mismatch: flip a byte → CrcMismatch error
5. test_large_payload: payload > MAX_PAYLOAD_SIZE → PayloadTooLarge error
6. test_type_hash_deterministic: same type → same hash
7. test_type_hash_different: different types → different hashes
8. Build: cargo test -p vuma-codegen --lib ipc

Acceptance:
- All 6 unit tests pass
- Wire format is robust

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 9):**

- [ ] cargo build --workspace succeeds
- [ ] IPC module exists with wire format + framing + CRC32
- [ ] Type hash is deterministic
- [ ] 6 unit tests pass

---


## Wave 10: Runtime Encapsulation L1 — Integrate Framing into Channel

**Spec references:** Spec §12, §46, §47
**Scope:** Wrap channel send/recv with message framing so every IPC message has a header, type hash, and CRC32 checksum.
**Max parallel:** 4 (never more than 4)

### 10a — Integrate framing into ChannelSend (x86_64)

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 10a
Integrate framing into ChannelSend (x86_64)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. In ChannelSend handler:
2. Compute type_hash from the message's ScgType
3. Build MessageHeader with channel_id, sequence (increment per channel), type_hash, payload_len
4. Serialize payload (LE bytes for primitives)
5. Call frame_message to get framed bytes
6. Write framed bytes to pipe via write() syscall
7. Build: cargo build --workspace
8. Test: send 42 as i32, verify framed message on pipe

Acceptance:
- ChannelSend wraps payload in framed message
- Type hash is included
- Sequence number increments
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 10b — Integrate framing into ChannelRecv (x86_64)

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 10b
Integrate framing into ChannelRecv (x86_64)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. In ChannelRecv handler:
2. Read HEADER_SIZE bytes from pipe
3. Parse MessageHeader (verify magic, version)
4. Read payload_len bytes
5. Read CRC32, verify
6. If CRC mismatch: return error (don't deliver message)
7. If type_hash mismatch: return error
8. Deserialize payload into dst vreg
9. Build: cargo build --workspace
10. Test: send 42, recv 42 — CRC passes, type matches

Acceptance:
- ChannelRecv deframes and verifies
- CRC mismatch → error
- Type mismatch → error
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 10c — Serialization for primitive types

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 10c
Serialization for primitive types

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement serialize_value(val: &IRValue, ty: &IRType) -> Vec<u8>
2. For I8/U8: 1 byte LE
3. For I16/U16: 2 bytes LE
4. For I32/U32: 4 bytes LE
5. For I64/U64: 8 bytes LE
6. For F32: 4 bytes LE (IEEE 754 bits)
7. For F64: 8 bytes LE (IEEE 754 bits)
8. For Bool: 1 byte (0 or 1)
9. Implement deserialize_value(bytes: &[u8], ty: &IRType) -> IRValue
10. Test: roundtrip all primitive types
11. Build: cargo build --workspace

Acceptance:
- All primitive types serialize/deserialize correctly
- Roundtrip preserves value

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 10d — Port framed channels to aarch64

**Files:** `src/codegen/src/emit.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 10d
Port framed channels to aarch64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/emit.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Use the same wire format + framing from ipc.rs
2. ChannelSend: frame + write to pipe
3. ChannelRecv: read + deframe
4. Same serialization
5. Build + test on qemu-aarch64-static

Acceptance:
- Framed IPC works on aarch64
QEMU test: qemu-aarch64-static

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 10):**

- [ ] cargo build --workspace succeeds
- [ ] Channel messages are framed (header + CRC + type hash)
- [ ] Serialization works for all primitive types
- [ ] CRC mismatch and type mismatch are detected
- [ ] Framed IPC works on x86_64 and aarch64

---


## Wave 11: Runtime Encapsulation L2 — Capability Tokens

**Spec references:** Spec §13, §51, §52
**Scope:** Implement signed capability tokens that authorize resource access between processes.
**Max parallel:** 4 (never more than 4)

### 11a — Create capability module

**Files:** `src/codegen/src/capability.rs (new)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 11a
Create capability module

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs (new)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create src/codegen/src/capability.rs
2. Define CapabilityToken: id (u128 UUID), source_pid (u64), target_pid (u64), resource (Resource enum), permissions (MemoryPermissions), delegation_depth (u8), created_at (u64), expires_at (u64), signature ([u8; 32])
3. Define Resource enum: File(String), Network(String, u16), Memory(u64, u64), Mmio(u64, u64), Channel(u64)
4. Define MemoryPermissions: read, write, execute (bool flags)
5. Define CapabilitySet: HashMap<u128, CapabilityToken>
6. Add module to lib.rs
7. Build: cargo build --workspace

Acceptance:
- Capability module exists
- CapabilityToken struct defined
- Resource enum covers all resource types

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 11b — Implement grant and verify

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 11b
Implement grant and verify

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. grant_capability(source_pid, target_pid, resource, perms, signing_key) -> CapabilityToken
2. Generate UUID, set timestamps, sign with HMAC-SHA256
3. verify_capability(token, required_resource, required_perms, verification_key) -> bool
4. Check: signature valid, not expired, resource matches, permissions sufficient
5. Check: target_pid matches caller
6. Test: grant then verify succeeds; wrong resource fails; expired fails; wrong perms fails
7. Build: cargo build --workspace

Acceptance:
- Grant creates a valid signed token
- Verify checks all fields correctly

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 11c — Capability revocation registry

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 11c
Capability revocation registry

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define RevocationRegistry (HashSet<u128> of revoked token IDs)
2. revoke(token_id) — adds to registry
3. is_revoked(token_id) -> bool
4. verify_capability also checks revocation registry
5. Test: grant → verify OK → revoke → verify fails
6. Build: cargo build --workspace

Acceptance:
- Revocation works
- Revoked tokens fail verification

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 11d — Capability encoding for IPC

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 11d
Capability encoding for IPC

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement encode() -> Vec<u8> for CapabilityToken (LE bytes, fixed 96 bytes)
2. Implement decode(bytes: &[u8]) -> Result<CapabilityToken, DecodeError>
3. CAPABILITY_TOKEN_SIZE = 96
4. Test: encode → decode roundtrip
5. Build: cargo build --workspace

Acceptance:
- Tokens can be encoded/decoded for IPC transport
- Roundtrip preserves all fields

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 11):**

- [ ] cargo build --workspace succeeds
- [ ] Capability module with grant/verify/revoke exists
- [ ] CapabilityToken can be encoded for IPC
- [ ] Tokens are signed with HMAC-SHA256

---


## Wave 12: Runtime Encapsulation L2 — Capabilities in IPC Messages

**Spec references:** Spec §13, §46
**Scope:** Attach capability tokens to IPC messages so the receiver can verify authorization.
**Max parallel:** 4 (never more than 4)

### 12a — Attach capabilities to framed messages

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 12a
Attach capabilities to framed messages

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add capabilities: Vec<CapabilityToken> to EncapsulatedMessage
2. In frame_message: write capability_count, then each capability's encode() after payload
3. In deframe_message: read capability_count, then read that many capability tokens
4. Set HAS_CAPS flag in MessageFlags if capabilities present
5. Build: cargo build --workspace

Acceptance:
- Messages carry capability tokens
- Capabilities are serialized after payload

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 12b — Verify capabilities on receive (x86_64)

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 12b
Verify capabilities on receive (x86_64)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. After deframing, check if message has capabilities (HAS_CAPS flag)
2. If yes, verify each capability token's signature
3. If any signature invalid: return PermissionDenied error
4. If capabilities required but missing: return MissingRequiredCapability error
5. Build: cargo build --workspace
6. Test: send message with valid capability → received OK; invalid signature → error

Acceptance:
- Capability signatures verified on receive
- Invalid signatures rejected
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 12c — Capability delegation chain

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 12c
Capability delegation chain

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add delegate_capability(parent_token, new_target, subset_perms, signing_key) -> CapabilityToken
2. New token has delegation_depth = parent.delegation_depth + 1
3. Max delegation depth = 8 (MAX_DELEGATION_DEPTH)
4. New token's permissions must be a subset of parent's
5. New token references parent's ID (delegated_from field)
6. Test: grant → delegate → verify delegation chain
7. Build: cargo build --workspace

Acceptance:
- Delegation creates child token with subset permissions
- Delegation depth limited to 8

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 12d — Capability integration test

**Files:** `tests/gold_standard/ipc/`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 12d
Capability integration test

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ipc/
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create tests/gold_standard/ipc/capability_grant.vuma — grant capability, send with capability, receiver verifies
2. Create tests/gold_standard/ipc/capability_revoke.vuma — grant, revoke, verify fails
3. Create tests/gold_standard/ipc/capability_delegate.vuma — grant, delegate, verify delegation
4. Each with // Expected exit code
5. Run on x86_64
6. Build: cargo build --workspace

Acceptance:
- 3 capability integration tests pass on x86_64
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 12):**

- [ ] cargo build --workspace succeeds
- [ ] IPC messages carry signed capability tokens
- [ ] Capability delegation chain works (max depth 8)
- [ ] Capability revocation works
- [ ] 3 capability tests pass on x86_64

---


## Wave 13: Runtime Encapsulation L3 — Memory Windows

**Spec references:** Spec §14, §9.2, §9.3
**Scope:** Implement shared memory windows between processes with capability-granted access.
**Max parallel:** 4 (never more than 4)

### 13a — Define MemoryWindow struct

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 13a
Define MemoryWindow struct

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define MemoryWindow: source_pid, target_pid, source_addr, target_addr, size, permissions, capability_id, revocable, linear
2. Define grant_memory(source, target, addr, size, perms) -> MemoryWindow
3. Define revoke_memory(window) -> removes mapping
4. Build: cargo build --workspace

Acceptance:
- MemoryWindow struct defined
- Grant/revoke functions exist

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 13b — Implement shared memory on x86_64

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 13b
Implement shared memory on x86_64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. grant_memory: mmap(NULL, size, PROT_READ|PROT_WRITE, MAP_SHARED|MAP_ANONYMOUS, -1, 0)
2. Share fd with child via SCM_RIGHTS (sendmsg with ancillary data)
3. Child mmaps the same fd at its own address
4. revoke: munmap in child, close fd
5. Test: parent writes to shared memory, child reads
6. Build: cargo build --workspace

Acceptance:
- Shared memory works on x86_64
- Parent writes, child reads via mmap
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 13c — Memory window permissions enforcement

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 13c
Memory window permissions enforcement

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. If permission is READ_ONLY: mmap with PROT_READ only in child
2. If permission is READ_WRITE: mmap with PROT_READ|PROT_WRITE
3. If child tries to write to READ_ONLY window: SIGSEGV (hardware enforces)
4. Test: grant READ_ONLY, child tries to write → crash (expected)
5. Build: cargo build --workspace

Acceptance:
- Permissions enforced by MMU
- READ_ONLY window: write causes SIGSEGV
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 13d — Memory window test

**Files:** `tests/gold_standard/ipc/shared_memory.vuma`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 13d
Memory window test

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ipc/shared_memory.vuma
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Parent allocates shared memory window
2. Parent writes 42 to shared memory
3. Child reads 42 from shared memory
4. Child exits with 42
5. Parent waits, returns 42
6. // Expected exit code: 42
7. Build: cargo build --workspace

Acceptance:
- Shared memory test passes on x86_64
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 13):**

- [ ] cargo build --workspace succeeds
- [ ] MemoryWindow defined and implemented on x86_64
- [ ] Permissions enforced by MMU (READ_ONLY → SIGSEGV on write)
- [ ] Shared memory test passes

---


## Wave 14: Runtime Encapsulation L4 — Protocol State Machine

**Spec references:** Spec §15, §50
**Scope:** Add protocol state machines to channels so invalid message sequences are rejected at runtime.
**Max parallel:** 4 (never more than 4)

### 14a — Define protocol state machine

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 14a
Define protocol state machine

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define ProtocolState: Idle, WaitingForSend, WaitingForRecv, Closed
2. Define ProtocolTransition: (ProtocolState, MessageType) -> ProtocolState
3. Define allowed_transitions: HashMap<(ProtocolState, u64), ProtocolState> where u64 is type_hash
4. channel_protocol_check(channel, message_type_hash) -> Result<ProtocolState, ProtocolError>
5. If (state, type_hash) not in allowed_transitions → ProtocolViolation
6. Build: cargo build --workspace

Acceptance:
- Protocol state machine defined
- Invalid transitions rejected

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 14b — Integrate protocol check into ChannelRecv

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 14b
Integrate protocol check into ChannelRecv

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. After deframing, extract type_hash from message header
2. Call channel_protocol_check(current_state, type_hash)
3. If Ok: update channel state, deliver message
4. If Err(ProtocolViolation): discard message, return error to caller
5. Build: cargo build --workspace
6. Test: send messages in wrong order → error

Acceptance:
- Protocol violations detected at runtime
- Valid sequences work
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 14c — Protocol state machine test

**Files:** `tests/gold_standard/ipc/protocol_state.vuma`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 14c
Protocol state machine test

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ipc/protocol_state.vuma
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define a protocol: Send<Open> → Recv<Fd> → Send<Read> → Recv<Data> → Send<Close>
2. Test valid sequence: all steps in order → success
3. Test invalid: skip Open, go straight to Read → ProtocolViolation error
4. // Expected exit code: 0 (valid) or 1 (invalid)
5. Build: cargo build --workspace

Acceptance:
- Protocol state machine test passes
- Valid sequence works, invalid rejected
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 14d — Port protocol state machine to aarch64

**Files:** `src/codegen/src/emit.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 14d
Port protocol state machine to aarch64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/emit.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add protocol check to ChannelRecv in aarch64 backend
2. Same state machine logic
3. Build + test on qemu-aarch64-static

Acceptance:
- Protocol checking works on aarch64
QEMU test: qemu-aarch64-static

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 14):**

- [ ] cargo build --workspace succeeds
- [ ] Protocol state machine rejects invalid message sequences
- [ ] Valid sequences work correctly
- [ ] Protocol test passes on x86_64 and aarch64

---


## Wave 15: Runtime Encapsulation L1-L4 — Cross-Backend Porting

**Spec references:** Spec §12-15
**Scope:** Port all 4 runtime encapsulation layers to riscv64 and arm32.
**Max parallel:** 4 (never more than 4)

### 15a — Port framed channels to riscv64

**Files:** `src/codegen/src/riscv64.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 15a
Port framed channels to riscv64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/riscv64.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Port frame_message/deframe_message integration to riscv64
2. Same wire format, same CRC32, same type hash
3. Build + test on qemu-riscv64-static

Acceptance:
- Framed IPC works on riscv64
QEMU test: qemu-riscv64-static

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 15b — Port framed channels to arm32

**Files:** `src/codegen/src/arm32/mod.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 15b
Port framed channels to arm32

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/arm32/mod.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Port to arm32
2. Same wire format
3. Build + test on qemu-arm-static

Acceptance:
- Framed IPC works on arm32
QEMU test: qemu-arm-static

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 15c — Port capability verification to riscv64+arm32

**Files:** `src/codegen/src/riscv64.rs, src/codegen/src/arm32/mod.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 15c
Port capability verification to riscv64+arm32

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/riscv64.rs, src/codegen/src/arm32/mod.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add capability verification to ChannelRecv on riscv64 and arm32
2. Same HMAC-SHA256 verification
3. Build + test

Acceptance:
- Capability verification works on riscv64 and arm32

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 15d — Port protocol state machine to riscv64+arm32

**Files:** `src/codegen/src/riscv64.rs, src/codegen/src/arm32/mod.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 15d
Port protocol state machine to riscv64+arm32

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/riscv64.rs, src/codegen/src/arm32/mod.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add protocol check to ChannelRecv on both backends
2. Build + test

Acceptance:
- Protocol checking works on riscv64 and arm32

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 15):**

- [ ] cargo build --workspace succeeds
- [ ] All 4 runtime encapsulation layers work on x86_64, aarch64, riscv64, arm32

---


## Wave 16: Runtime Encapsulation L1-L4 — Integration Tests + CI

**Spec references:** Spec §12-15, §134
**Scope:** Create comprehensive integration tests for all 4 runtime encapsulation layers and add them to CI.
**Max parallel:** 4 (never more than 4)

### 16a — Create integration test suite

**Files:** `tests/gold_standard/ipc/`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 16a
Create integration test suite

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ipc/
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. framed_send_recv.vuma: send i32, receive i32, verify CRC passes
2. capability_grant_verify.vuma: grant capability, send with cap, receiver verifies
3. shared_memory_rw.vuma: parent writes to shared memory, child reads
4. protocol_valid.vuma: valid message sequence passes
5. protocol_invalid.vuma: invalid sequence → error
6. multi_message.vuma: send 10 messages, receive 10 in order
7. large_payload.vuma: send 4KB struct via channel
8. Each with // Expected exit code
9. Build: cargo build --workspace

Acceptance:
- 7 integration tests exist and pass on x86_64

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 16b — Add IPC tests to Makefile

**Files:** `Makefile`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 16b
Add IPC tests to Makefile

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: Makefile
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add ipc-test target
2. Compile + run each test in tests/gold_standard/ipc/
3. Check exit codes
4. Build: cargo build --workspace

Acceptance:
- make ipc-test runs all IPC tests

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 16c — Cross-backend IPC tests

**Files:** `tests/gold_standard/ipc/`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 16c
Cross-backend IPC tests

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ipc/
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Run all 7 IPC tests on aarch64, riscv64, arm32
2. Record results
3. Fix any failures
4. Build: cargo build --workspace

Acceptance:
- IPC tests pass on x86_64, aarch64, riscv64, arm32

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 16d — Performance baseline

**Files:** `tests/gold_standard/ipc/bench.vuma`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 16d
Performance baseline

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ipc/bench.vuma
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create bench.vuma: send 10000 messages, measure time
2. // Expected exit code: 0 (just run, don't check time)
3. Build + run on x86_64
4. Record latency baseline for future comparison

Acceptance:
- IPC benchmark exists
- Latency baseline recorded
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 16):**

- [ ] cargo build --workspace succeeds
- [ ] 7 IPC integration tests pass on all 4 backends
- [ ] make ipc-test target exists
- [ ] IPC latency baseline recorded

---


## Wave 17: Runtime Encapsulation L5 — Worker Sandboxing (seccomp)

**Spec references:** Spec §16, §59
**Scope:** Implement worker process sandboxing using seccomp filters to restrict syscalls.
**Max parallel:** 4 (never more than 4)

### 17a — Define WorkerConfig and sandbox options

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 17a
Define WorkerConfig and sandbox options

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define WorkerConfig: trust_level, capabilities, max_restarts, timeout_ms, seccomp_filter
2. Define SeccompFilter: allowed_syscalls (HashSet<u32>), default_action (Allow/Deny)
3. Define TrustLevel: Kernel, Verified, Untrusted, Sandboxed
4. Build: cargo build --workspace

Acceptance:
- WorkerConfig and SeccompFilter defined

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 17b — Generate seccomp BPF filter

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 17b
Generate seccomp BPF filter

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement generate_seccomp_filter(config: &WorkerConfig) -> Vec<sock_fprog>
2. For Sandboxed: allow only read, write, exit, exit_group
3. For Untrusted: allow read, write, close, exit, exit_group, brk, mmap (anonymous only)
4. For Verified: allow all syscalls
5. Build: cargo build --workspace

Acceptance:
- seccomp BPF filter generated correctly

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 17c — Apply seccomp filter on x86_64

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 17c
Apply seccomp filter on x86_64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. After fork+exec, in child process: apply seccomp filter via prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, ...)
2. Emit the seccomp filter as inline data in the child's code
3. Build: cargo build --workspace
4. Test: sandboxed worker that tries open() → killed by seccomp

Acceptance:
- seccomp filter applied in child process
- Sandboxed worker can't do open()
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 17d — Worker sandbox test

**Files:** `tests/gold_standard/ipc/sandbox.vuma`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 17d
Worker sandbox test

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ipc/sandbox.vuma
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Spawn sandboxed worker
2. Worker tries to open a file → killed by seccomp
3. Parent detects child killed (wait_worker returns SIGSYS)
4. // Expected exit code: 0 (parent handles gracefully)
5. Build: cargo build --workspace

Acceptance:
- Sandboxed worker killed when trying forbidden syscall
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 17):**

- [ ] cargo build --workspace succeeds
- [ ] Worker sandboxing via seccomp works on x86_64
- [ ] Sandboxed worker killed on forbidden syscall

---


## Wave 18: Runtime Encapsulation L5 — Resource Limits

**Spec references:** Spec §16, §66
**Scope:** Add resource limits (CPU, memory, IPC) to worker processes.
**Max parallel:** 4 (never more than 4)

### 18a — Define ResourceLimits

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 18a
Define ResourceLimits

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define ResourceLimits: max_cpu_ms, max_memory_bytes, max_ipc_messages, max_ipc_bytes
2. Add to WorkerConfig
3. Build: cargo build --workspace

Acceptance:
- ResourceLimits defined

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 18b — Enforce CPU limit on x86_64

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 18b
Enforce CPU limit on x86_64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Before fork: set RLIMIT_CPU via setrlimit
2. Child process gets CPU time limit
3. If exceeded: SIGXCPU sent to child
4. Build: cargo build --workspace

Acceptance:
- CPU limit enforced via setrlimit
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 18c — Enforce memory limit on x86_64

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 18c
Enforce memory limit on x86_64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Before fork: set RLIMIT_AS via setrlimit
2. Child process gets address space limit
3. If exceeded: mmap fails, child crashes
4. Build: cargo build --workspace

Acceptance:
- Memory limit enforced
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 18d — Resource limit test

**Files:** `tests/gold_standard/ipc/resource_limit.vuma`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 18d
Resource limit test

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ipc/resource_limit.vuma
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Spawn worker with 10ms CPU limit
2. Worker runs infinite loop
3. Worker killed by SIGXCPU after 10ms
4. Parent detects timeout
5. // Expected exit code: 0
6. Build: cargo build --workspace

Acceptance:
- CPU-limited worker killed after timeout
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 18):**

- [ ] cargo build --workspace succeeds
- [ ] Resource limits (CPU, memory) enforced on x86_64
- [ ] Worker killed when exceeding limits

---


## Wave 19: Runtime Encapsulation L6-L8 + Integration (Wave 19)

**Spec references:** Spec §17-19, §189
**Scope:** Implement state checkpointing (L6), error containment (L7), and cryptographic encapsulation (L8). Wave 19 of the runtime encapsulation series.
**Max parallel:** 4 (never more than 4)

### 19a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 19a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 19 subtask 1 per spec sections Spec §17-19, §189
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 19b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 19b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 19 subtask 2 per spec sections Spec §17-19, §189
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 19c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 19c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 19 subtask 3 per spec sections Spec §17-19, §189
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 19d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 19d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 19 subtask 4 per spec sections Spec §17-19, §189
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 19):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 19 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §17-19, §189 implemented

---


## Wave 20: Runtime Encapsulation L6-L8 + Integration (Wave 20)

**Spec references:** Spec §17-19, §190
**Scope:** Implement state checkpointing (L6), error containment (L7), and cryptographic encapsulation (L8). Wave 20 of the runtime encapsulation series.
**Max parallel:** 4 (never more than 4)

### 20a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 20a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 20 subtask 1 per spec sections Spec §17-19, §190
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 20b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 20b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 20 subtask 2 per spec sections Spec §17-19, §190
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 20c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 20c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 20 subtask 3 per spec sections Spec §17-19, §190
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 20d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 20d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 20 subtask 4 per spec sections Spec §17-19, §190
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 20):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 20 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §17-19, §190 implemented

---


## Wave 21: Runtime Encapsulation L6-L8 + Integration (Wave 21)

**Spec references:** Spec §17-19, §191
**Scope:** Implement state checkpointing (L6), error containment (L7), and cryptographic encapsulation (L8). Wave 21 of the runtime encapsulation series.
**Max parallel:** 4 (never more than 4)

### 21a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 21a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 21 subtask 1 per spec sections Spec §17-19, §191
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 21b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 21b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 21 subtask 2 per spec sections Spec §17-19, §191
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 21c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 21c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 21 subtask 3 per spec sections Spec §17-19, §191
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 21d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 21d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 21 subtask 4 per spec sections Spec §17-19, §191
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 21):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 21 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §17-19, §191 implemented

---


## Wave 22: Runtime Encapsulation L6-L8 + Integration (Wave 22)

**Spec references:** Spec §17-19, §192
**Scope:** Implement state checkpointing (L6), error containment (L7), and cryptographic encapsulation (L8). Wave 22 of the runtime encapsulation series.
**Max parallel:** 4 (never more than 4)

### 22a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 22a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 22 subtask 1 per spec sections Spec §17-19, §192
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 22b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 22b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 22 subtask 2 per spec sections Spec §17-19, §192
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 22c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 22c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 22 subtask 3 per spec sections Spec §17-19, §192
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 22d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 22d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 22 subtask 4 per spec sections Spec §17-19, §192
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 22):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 22 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §17-19, §192 implemented

---


## Wave 23: Runtime Encapsulation L6-L8 + Integration (Wave 23)

**Spec references:** Spec §17-19, §193
**Scope:** Implement state checkpointing (L6), error containment (L7), and cryptographic encapsulation (L8). Wave 23 of the runtime encapsulation series.
**Max parallel:** 4 (never more than 4)

### 23a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 23a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 23 subtask 1 per spec sections Spec §17-19, §193
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 23b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 23b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 23 subtask 2 per spec sections Spec §17-19, §193
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 23c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 23c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 23 subtask 3 per spec sections Spec §17-19, §193
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 23d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 23d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 23 subtask 4 per spec sections Spec §17-19, §193
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 23):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 23 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §17-19, §193 implemented

---


## Wave 24: Runtime Encapsulation L6-L8 + Integration (Wave 24)

**Spec references:** Spec §17-19, §194
**Scope:** Implement state checkpointing (L6), error containment (L7), and cryptographic encapsulation (L8). Wave 24 of the runtime encapsulation series.
**Max parallel:** 4 (never more than 4)

### 24a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 24a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 24 subtask 1 per spec sections Spec §17-19, §194
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 24b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 24b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 24 subtask 2 per spec sections Spec §17-19, §194
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 24c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 24c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 24 subtask 3 per spec sections Spec §17-19, §194
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 24d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 24d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 24 subtask 4 per spec sections Spec §17-19, §194
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 24):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 24 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §17-19, §194 implemented

---


## Wave 25: FFI Process Isolation (Wave 25)

**Spec references:** Spec §56-61
**Scope:** Implement extern "process" FFI isolation: auto-marshalling, worker lifecycle, seccomp, crash recovery, performance optimization.
**Max parallel:** 4 (never more than 4)

### 25a — Subtask 1

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 25a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 25 subtask 1 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 25b — Subtask 2

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 25b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 25 subtask 2 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 25c — Subtask 3

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 25c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 25 subtask 3 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 25d — Subtask 4

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 25d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 25 subtask 4 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 25):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 25 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §56-61 implemented

---


## Wave 26: FFI Process Isolation (Wave 26)

**Spec references:** Spec §56-61
**Scope:** Implement extern "process" FFI isolation: auto-marshalling, worker lifecycle, seccomp, crash recovery, performance optimization.
**Max parallel:** 4 (never more than 4)

### 26a — Subtask 1

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 26a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 26 subtask 1 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 26b — Subtask 2

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 26b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 26 subtask 2 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 26c — Subtask 3

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 26c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 26 subtask 3 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 26d — Subtask 4

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 26d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 26 subtask 4 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 26):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 26 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §56-61 implemented

---


## Wave 27: FFI Process Isolation (Wave 27)

**Spec references:** Spec §56-61
**Scope:** Implement extern "process" FFI isolation: auto-marshalling, worker lifecycle, seccomp, crash recovery, performance optimization.
**Max parallel:** 4 (never more than 4)

### 27a — Subtask 1

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 27a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 27 subtask 1 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 27b — Subtask 2

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 27b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 27 subtask 2 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 27c — Subtask 3

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 27c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 27 subtask 3 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 27d — Subtask 4

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 27d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 27 subtask 4 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 27):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 27 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §56-61 implemented

---


## Wave 28: FFI Process Isolation (Wave 28)

**Spec references:** Spec §56-61
**Scope:** Implement extern "process" FFI isolation: auto-marshalling, worker lifecycle, seccomp, crash recovery, performance optimization.
**Max parallel:** 4 (never more than 4)

### 28a — Subtask 1

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 28a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 28 subtask 1 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 28b — Subtask 2

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 28b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 28 subtask 2 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 28c — Subtask 3

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 28c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 28 subtask 3 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 28d — Subtask 4

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 28d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 28 subtask 4 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 28):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 28 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §56-61 implemented

---


## Wave 29: FFI Process Isolation (Wave 29)

**Spec references:** Spec §56-61
**Scope:** Implement extern "process" FFI isolation: auto-marshalling, worker lifecycle, seccomp, crash recovery, performance optimization.
**Max parallel:** 4 (never more than 4)

### 29a — Subtask 1

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 29a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 29 subtask 1 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 29b — Subtask 2

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 29b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 29 subtask 2 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 29c — Subtask 3

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 29c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 29 subtask 3 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 29d — Subtask 4

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 29d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 29 subtask 4 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 29):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 29 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §56-61 implemented

---


## Wave 30: FFI Process Isolation (Wave 30)

**Spec references:** Spec §56-61
**Scope:** Implement extern "process" FFI isolation: auto-marshalling, worker lifecycle, seccomp, crash recovery, performance optimization.
**Max parallel:** 4 (never more than 4)

### 30a — Subtask 1

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 30a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 30 subtask 1 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 30b — Subtask 2

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 30b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 30 subtask 2 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 30c — Subtask 3

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 30c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 30 subtask 3 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 30d — Subtask 4

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 30d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 30 subtask 4 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 30):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 30 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §56-61 implemented

---


## Wave 31: FFI Process Isolation (Wave 31)

**Spec references:** Spec §56-61
**Scope:** Implement extern "process" FFI isolation: auto-marshalling, worker lifecycle, seccomp, crash recovery, performance optimization.
**Max parallel:** 4 (never more than 4)

### 31a — Subtask 1

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 31a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 31 subtask 1 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 31b — Subtask 2

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 31b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 31 subtask 2 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 31c — Subtask 3

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 31c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 31 subtask 3 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 31d — Subtask 4

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 31d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 31 subtask 4 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 31):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 31 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §56-61 implemented

---


## Wave 32: FFI Process Isolation (Wave 32)

**Spec references:** Spec §56-61
**Scope:** Implement extern "process" FFI isolation: auto-marshalling, worker lifecycle, seccomp, crash recovery, performance optimization.
**Max parallel:** 4 (never more than 4)

### 32a — Subtask 1

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 32a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 32 subtask 1 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 32b — Subtask 2

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 32b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 32 subtask 2 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 32c — Subtask 3

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 32c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 32 subtask 3 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 32d — Subtask 4

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 32d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 32 subtask 4 per spec sections Spec §56-61
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 32):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 32 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §56-61 implemented

---


## Wave 33: Capability System (Wave 33)

**Spec references:** Spec §51-55
**Scope:** Extend capability system: delegation chain, flow verification, revocation propagation, cross-process capability tracking.
**Max parallel:** 4 (never more than 4)

### 33a — Subtask 1

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 33a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 33 subtask 1 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 33b — Subtask 2

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 33b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 33 subtask 2 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 33c — Subtask 3

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 33c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 33 subtask 3 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 33d — Subtask 4

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 33d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 33 subtask 4 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 33):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 33 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §51-55 implemented

---


## Wave 34: Capability System (Wave 34)

**Spec references:** Spec §51-55
**Scope:** Extend capability system: delegation chain, flow verification, revocation propagation, cross-process capability tracking.
**Max parallel:** 4 (never more than 4)

### 34a — Subtask 1

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 34a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 34 subtask 1 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 34b — Subtask 2

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 34b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 34 subtask 2 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 34c — Subtask 3

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 34c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 34 subtask 3 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 34d — Subtask 4

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 34d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 34 subtask 4 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 34):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 34 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §51-55 implemented

---


## Wave 35: Capability System (Wave 35)

**Spec references:** Spec §51-55
**Scope:** Extend capability system: delegation chain, flow verification, revocation propagation, cross-process capability tracking.
**Max parallel:** 4 (never more than 4)

### 35a — Subtask 1

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 35a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 35 subtask 1 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 35b — Subtask 2

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 35b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 35 subtask 2 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 35c — Subtask 3

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 35c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 35 subtask 3 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 35d — Subtask 4

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 35d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 35 subtask 4 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 35):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 35 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §51-55 implemented

---


## Wave 36: Capability System (Wave 36)

**Spec references:** Spec §51-55
**Scope:** Extend capability system: delegation chain, flow verification, revocation propagation, cross-process capability tracking.
**Max parallel:** 4 (never more than 4)

### 36a — Subtask 1

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 36a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 36 subtask 1 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 36b — Subtask 2

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 36b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 36 subtask 2 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 36c — Subtask 3

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 36c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 36 subtask 3 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 36d — Subtask 4

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 36d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 36 subtask 4 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 36):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 36 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §51-55 implemented

---


## Wave 37: Capability System (Wave 37)

**Spec references:** Spec §51-55
**Scope:** Extend capability system: delegation chain, flow verification, revocation propagation, cross-process capability tracking.
**Max parallel:** 4 (never more than 4)

### 37a — Subtask 1

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 37a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 37 subtask 1 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 37b — Subtask 2

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 37b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 37 subtask 2 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 37c — Subtask 3

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 37c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 37 subtask 3 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 37d — Subtask 4

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 37d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 37 subtask 4 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 37):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 37 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §51-55 implemented

---


## Wave 38: Capability System (Wave 38)

**Spec references:** Spec §51-55
**Scope:** Extend capability system: delegation chain, flow verification, revocation propagation, cross-process capability tracking.
**Max parallel:** 4 (never more than 4)

### 38a — Subtask 1

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 38a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 38 subtask 1 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 38b — Subtask 2

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 38b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 38 subtask 2 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 38c — Subtask 3

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 38c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 38 subtask 3 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 38d — Subtask 4

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 38d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 38 subtask 4 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 38):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 38 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §51-55 implemented

---


## Wave 39: Capability System (Wave 39)

**Spec references:** Spec §51-55
**Scope:** Extend capability system: delegation chain, flow verification, revocation propagation, cross-process capability tracking.
**Max parallel:** 4 (never more than 4)

### 39a — Subtask 1

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 39a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 39 subtask 1 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 39b — Subtask 2

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 39b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 39 subtask 2 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 39c — Subtask 3

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 39c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 39 subtask 3 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 39d — Subtask 4

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 39d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 39 subtask 4 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 39):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 39 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §51-55 implemented

---


## Wave 40: Capability System (Wave 40)

**Spec references:** Spec §51-55
**Scope:** Extend capability system: delegation chain, flow verification, revocation propagation, cross-process capability tracking.
**Max parallel:** 4 (never more than 4)

### 40a — Subtask 1

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 40a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 40 subtask 1 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 40b — Subtask 2

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 40b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 40 subtask 2 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 40c — Subtask 3

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 40c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 40 subtask 3 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 40d — Subtask 4

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 40d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 40 subtask 4 per spec sections Spec §51-55
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 40):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 40 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §51-55 implemented

---


## Wave 41: Kernel/User Split (Wave 41)

**Spec references:** Spec §62-66
**Scope:** Implement microkernel architecture: syscall-as-IPC, kernel process, user process, resource accounting.
**Max parallel:** 4 (never more than 4)

### 41a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 41a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 41 subtask 1 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 41b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 41b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 41 subtask 2 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 41c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 41c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 41 subtask 3 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 41d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 41d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 41 subtask 4 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 41):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 41 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §62-66 implemented

---


## Wave 42: Kernel/User Split (Wave 42)

**Spec references:** Spec §62-66
**Scope:** Implement microkernel architecture: syscall-as-IPC, kernel process, user process, resource accounting.
**Max parallel:** 4 (never more than 4)

### 42a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 42a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 42 subtask 1 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 42b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 42b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 42 subtask 2 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 42c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 42c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 42 subtask 3 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 42d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 42d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 42 subtask 4 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 42):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 42 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §62-66 implemented

---


## Wave 43: Kernel/User Split (Wave 43)

**Spec references:** Spec §62-66
**Scope:** Implement microkernel architecture: syscall-as-IPC, kernel process, user process, resource accounting.
**Max parallel:** 4 (never more than 4)

### 43a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 43a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 43 subtask 1 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 43b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 43b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 43 subtask 2 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 43c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 43c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 43 subtask 3 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 43d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 43d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 43 subtask 4 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 43):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 43 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §62-66 implemented

---


## Wave 44: Kernel/User Split (Wave 44)

**Spec references:** Spec §62-66
**Scope:** Implement microkernel architecture: syscall-as-IPC, kernel process, user process, resource accounting.
**Max parallel:** 4 (never more than 4)

### 44a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 44a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 44 subtask 1 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 44b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 44b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 44 subtask 2 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 44c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 44c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 44 subtask 3 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 44d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 44d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 44 subtask 4 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 44):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 44 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §62-66 implemented

---


## Wave 45: Kernel/User Split (Wave 45)

**Spec references:** Spec §62-66
**Scope:** Implement microkernel architecture: syscall-as-IPC, kernel process, user process, resource accounting.
**Max parallel:** 4 (never more than 4)

### 45a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 45a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 45 subtask 1 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 45b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 45b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 45 subtask 2 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 45c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 45c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 45 subtask 3 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 45d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 45d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 45 subtask 4 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 45):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 45 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §62-66 implemented

---


## Wave 46: Kernel/User Split (Wave 46)

**Spec references:** Spec §62-66
**Scope:** Implement microkernel architecture: syscall-as-IPC, kernel process, user process, resource accounting.
**Max parallel:** 4 (never more than 4)

### 46a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 46a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 46 subtask 1 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 46b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 46b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 46 subtask 2 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 46c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 46c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 46 subtask 3 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 46d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 46d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 46 subtask 4 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 46):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 46 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §62-66 implemented

---


## Wave 47: Kernel/User Split (Wave 47)

**Spec references:** Spec §62-66
**Scope:** Implement microkernel architecture: syscall-as-IPC, kernel process, user process, resource accounting.
**Max parallel:** 4 (never more than 4)

### 47a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 47a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 47 subtask 1 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 47b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 47b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 47 subtask 2 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 47c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 47c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 47 subtask 3 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 47d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 47d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 47 subtask 4 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 47):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 47 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §62-66 implemented

---


## Wave 48: Kernel/User Split (Wave 48)

**Spec references:** Spec §62-66
**Scope:** Implement microkernel architecture: syscall-as-IPC, kernel process, user process, resource accounting.
**Max parallel:** 4 (never more than 4)

### 48a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 48a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 48 subtask 1 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 48b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 48b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 48 subtask 2 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 48c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 48c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 48 subtask 3 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 48d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 48d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 48 subtask 4 per spec sections Spec §62-66
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 48):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 48 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §62-66 implemented

---


## Wave 49: Driver Isolation (Wave 49)

**Spec references:** Spec §67-71
**Scope:** Implement driver worker isolation: MMIO capabilities, IRQ channels, DMA buffer management, driver restart.
**Max parallel:** 4 (never more than 4)

### 49a — Subtask 1

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 49a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 49 subtask 1 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 49b — Subtask 2

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 49b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 49 subtask 2 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 49c — Subtask 3

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 49c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 49 subtask 3 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 49d — Subtask 4

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 49d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 49 subtask 4 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 49):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 49 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §67-71 implemented

---


## Wave 50: Driver Isolation (Wave 50)

**Spec references:** Spec §67-71
**Scope:** Implement driver worker isolation: MMIO capabilities, IRQ channels, DMA buffer management, driver restart.
**Max parallel:** 4 (never more than 4)

### 50a — Subtask 1

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 50a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 50 subtask 1 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 50b — Subtask 2

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 50b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 50 subtask 2 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 50c — Subtask 3

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 50c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 50 subtask 3 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 50d — Subtask 4

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 50d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 50 subtask 4 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 50):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 50 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §67-71 implemented

---


## Wave 51: Driver Isolation (Wave 51)

**Spec references:** Spec §67-71
**Scope:** Implement driver worker isolation: MMIO capabilities, IRQ channels, DMA buffer management, driver restart.
**Max parallel:** 4 (never more than 4)

### 51a — Subtask 1

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 51a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 51 subtask 1 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 51b — Subtask 2

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 51b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 51 subtask 2 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 51c — Subtask 3

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 51c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 51 subtask 3 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 51d — Subtask 4

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 51d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 51 subtask 4 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 51):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 51 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §67-71 implemented

---


## Wave 52: Driver Isolation (Wave 52)

**Spec references:** Spec §67-71
**Scope:** Implement driver worker isolation: MMIO capabilities, IRQ channels, DMA buffer management, driver restart.
**Max parallel:** 4 (never more than 4)

### 52a — Subtask 1

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 52a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 52 subtask 1 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 52b — Subtask 2

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 52b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 52 subtask 2 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 52c — Subtask 3

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 52c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 52 subtask 3 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 52d — Subtask 4

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 52d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 52 subtask 4 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 52):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 52 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §67-71 implemented

---


## Wave 53: Driver Isolation (Wave 53)

**Spec references:** Spec §67-71
**Scope:** Implement driver worker isolation: MMIO capabilities, IRQ channels, DMA buffer management, driver restart.
**Max parallel:** 4 (never more than 4)

### 53a — Subtask 1

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 53a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 53 subtask 1 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 53b — Subtask 2

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 53b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 53 subtask 2 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 53c — Subtask 3

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 53c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 53 subtask 3 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 53d — Subtask 4

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 53d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 53 subtask 4 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 53):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 53 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §67-71 implemented

---


## Wave 54: Driver Isolation (Wave 54)

**Spec references:** Spec §67-71
**Scope:** Implement driver worker isolation: MMIO capabilities, IRQ channels, DMA buffer management, driver restart.
**Max parallel:** 4 (never more than 4)

### 54a — Subtask 1

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 54a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 54 subtask 1 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 54b — Subtask 2

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 54b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 54 subtask 2 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 54c — Subtask 3

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 54c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 54 subtask 3 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 54d — Subtask 4

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 54d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 54 subtask 4 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 54):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 54 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §67-71 implemented

---


## Wave 55: Driver Isolation (Wave 55)

**Spec references:** Spec §67-71
**Scope:** Implement driver worker isolation: MMIO capabilities, IRQ channels, DMA buffer management, driver restart.
**Max parallel:** 4 (never more than 4)

### 55a — Subtask 1

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 55a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 55 subtask 1 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 55b — Subtask 2

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 55b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 55 subtask 2 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 55c — Subtask 3

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 55c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 55 subtask 3 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 55d — Subtask 4

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 55d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 55 subtask 4 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 55):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 55 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §67-71 implemented

---


## Wave 56: Driver Isolation (Wave 56)

**Spec references:** Spec §67-71
**Scope:** Implement driver worker isolation: MMIO capabilities, IRQ channels, DMA buffer management, driver restart.
**Max parallel:** 4 (never more than 4)

### 56a — Subtask 1

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 56a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 56 subtask 1 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 56b — Subtask 2

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 56b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 56 subtask 2 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 56c — Subtask 3

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 56c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 56 subtask 3 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 56d — Subtask 4

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 56d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 56 subtask 4 per spec sections Spec §67-71
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds
QEMU test: qemu-x86_64 (native)

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 56):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 56 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §67-71 implemented

---


## Wave 57: Sandboxing (Wave 57)

**Spec references:** Spec §72-76
**Scope:** Implement sandboxing: zero-capability workers, plugin system, sandboxed parsers, sandboxed crypto.
**Max parallel:** 4 (never more than 4)

### 57a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 57a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 57 subtask 1 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 57b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 57b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 57 subtask 2 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 57c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 57c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 57 subtask 3 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 57d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 57d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 57 subtask 4 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 57):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 57 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §72-76 implemented

---


## Wave 58: Sandboxing (Wave 58)

**Spec references:** Spec §72-76
**Scope:** Implement sandboxing: zero-capability workers, plugin system, sandboxed parsers, sandboxed crypto.
**Max parallel:** 4 (never more than 4)

### 58a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 58a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 58 subtask 1 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 58b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 58b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 58 subtask 2 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 58c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 58c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 58 subtask 3 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 58d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 58d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 58 subtask 4 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 58):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 58 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §72-76 implemented

---


## Wave 59: Sandboxing (Wave 59)

**Spec references:** Spec §72-76
**Scope:** Implement sandboxing: zero-capability workers, plugin system, sandboxed parsers, sandboxed crypto.
**Max parallel:** 4 (never more than 4)

### 59a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 59a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 59 subtask 1 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 59b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 59b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 59 subtask 2 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 59c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 59c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 59 subtask 3 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 59d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 59d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 59 subtask 4 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 59):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 59 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §72-76 implemented

---


## Wave 60: Sandboxing (Wave 60)

**Spec references:** Spec §72-76
**Scope:** Implement sandboxing: zero-capability workers, plugin system, sandboxed parsers, sandboxed crypto.
**Max parallel:** 4 (never more than 4)

### 60a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 60a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 60 subtask 1 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 60b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 60b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 60 subtask 2 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 60c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 60c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 60 subtask 3 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 60d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 60d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 60 subtask 4 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 60):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 60 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §72-76 implemented

---


## Wave 61: Sandboxing (Wave 61)

**Spec references:** Spec §72-76
**Scope:** Implement sandboxing: zero-capability workers, plugin system, sandboxed parsers, sandboxed crypto.
**Max parallel:** 4 (never more than 4)

### 61a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 61a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 61 subtask 1 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 61b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 61b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 61 subtask 2 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 61c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 61c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 61 subtask 3 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 61d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 61d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 61 subtask 4 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 61):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 61 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §72-76 implemented

---


## Wave 62: Sandboxing (Wave 62)

**Spec references:** Spec §72-76
**Scope:** Implement sandboxing: zero-capability workers, plugin system, sandboxed parsers, sandboxed crypto.
**Max parallel:** 4 (never more than 4)

### 62a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 62a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 62 subtask 1 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 62b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 62b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 62 subtask 2 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 62c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 62c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 62 subtask 3 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 62d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 62d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 62 subtask 4 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 62):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 62 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §72-76 implemented

---


## Wave 63: Sandboxing (Wave 63)

**Spec references:** Spec §72-76
**Scope:** Implement sandboxing: zero-capability workers, plugin system, sandboxed parsers, sandboxed crypto.
**Max parallel:** 4 (never more than 4)

### 63a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 63a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 63 subtask 1 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 63b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 63b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 63 subtask 2 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 63c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 63c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 63 subtask 3 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 63d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 63d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 63 subtask 4 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 63):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 63 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §72-76 implemented

---


## Wave 64: Sandboxing (Wave 64)

**Spec references:** Spec §72-76
**Scope:** Implement sandboxing: zero-capability workers, plugin system, sandboxed parsers, sandboxed crypto.
**Max parallel:** 4 (never more than 4)

### 64a — Subtask 1

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 64a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 64 subtask 1 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 64b — Subtask 2

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 64b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 64 subtask 2 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 64c — Subtask 3

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 64c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 64 subtask 3 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 64d — Subtask 4

**Files:** `src/codegen/src/ipc.rs, src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 64d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs, src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 64 subtask 4 per spec sections Spec §72-76
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 64):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 64 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §72-76 implemented

---


## Wave 65: Fault Tolerance (Wave 65)

**Spec references:** Spec §77-82
**Scope:** Implement fault tolerance: supervisor architecture, crash detection, state checkpointing, worker restart, graceful degradation, circuit breaker.
**Max parallel:** 4 (never more than 4)

### 65a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 65a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 65 subtask 1 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 65b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 65b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 65 subtask 2 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 65c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 65c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 65 subtask 3 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 65d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 65d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 65 subtask 4 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 65):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 65 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §77-82 implemented

---


## Wave 66: Fault Tolerance (Wave 66)

**Spec references:** Spec §77-82
**Scope:** Implement fault tolerance: supervisor architecture, crash detection, state checkpointing, worker restart, graceful degradation, circuit breaker.
**Max parallel:** 4 (never more than 4)

### 66a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 66a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 66 subtask 1 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 66b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 66b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 66 subtask 2 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 66c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 66c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 66 subtask 3 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 66d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 66d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 66 subtask 4 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 66):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 66 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §77-82 implemented

---


## Wave 67: Fault Tolerance (Wave 67)

**Spec references:** Spec §77-82
**Scope:** Implement fault tolerance: supervisor architecture, crash detection, state checkpointing, worker restart, graceful degradation, circuit breaker.
**Max parallel:** 4 (never more than 4)

### 67a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 67a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 67 subtask 1 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 67b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 67b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 67 subtask 2 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 67c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 67c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 67 subtask 3 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 67d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 67d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 67 subtask 4 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 67):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 67 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §77-82 implemented

---


## Wave 68: Fault Tolerance (Wave 68)

**Spec references:** Spec §77-82
**Scope:** Implement fault tolerance: supervisor architecture, crash detection, state checkpointing, worker restart, graceful degradation, circuit breaker.
**Max parallel:** 4 (never more than 4)

### 68a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 68a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 68 subtask 1 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 68b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 68b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 68 subtask 2 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 68c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 68c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 68 subtask 3 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 68d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 68d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 68 subtask 4 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 68):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 68 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §77-82 implemented

---


## Wave 69: Fault Tolerance (Wave 69)

**Spec references:** Spec §77-82
**Scope:** Implement fault tolerance: supervisor architecture, crash detection, state checkpointing, worker restart, graceful degradation, circuit breaker.
**Max parallel:** 4 (never more than 4)

### 69a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 69a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 69 subtask 1 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 69b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 69b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 69 subtask 2 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 69c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 69c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 69 subtask 3 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 69d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 69d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 69 subtask 4 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 69):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 69 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §77-82 implemented

---


## Wave 70: Fault Tolerance (Wave 70)

**Spec references:** Spec §77-82
**Scope:** Implement fault tolerance: supervisor architecture, crash detection, state checkpointing, worker restart, graceful degradation, circuit breaker.
**Max parallel:** 4 (never more than 4)

### 70a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 70a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 70 subtask 1 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 70b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 70b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 70 subtask 2 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 70c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 70c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 70 subtask 3 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 70d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 70d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 70 subtask 4 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 70):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 70 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §77-82 implemented

---


## Wave 71: Fault Tolerance (Wave 71)

**Spec references:** Spec §77-82
**Scope:** Implement fault tolerance: supervisor architecture, crash detection, state checkpointing, worker restart, graceful degradation, circuit breaker.
**Max parallel:** 4 (never more than 4)

### 71a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 71a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 71 subtask 1 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 71b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 71b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 71 subtask 2 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 71c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 71c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 71 subtask 3 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 71d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 71d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 71 subtask 4 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 71):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 71 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §77-82 implemented

---


## Wave 72: Fault Tolerance (Wave 72)

**Spec references:** Spec §77-82
**Scope:** Implement fault tolerance: supervisor architecture, crash detection, state checkpointing, worker restart, graceful degradation, circuit breaker.
**Max parallel:** 4 (never more than 4)

### 72a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 72a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 72 subtask 1 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 72b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 72b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 72 subtask 2 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 72c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 72c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 72 subtask 3 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 72d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 72d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 72 subtask 4 per spec sections Spec §77-82
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 72):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 72 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §77-82 implemented

---


## Wave 73: Hot Reloading (Wave 73)

**Spec references:** Spec §83-86
**Scope:** Implement hot reloading: hot-swap protocol, state transfer, version management, rollback.
**Max parallel:** 4 (never more than 4)

### 73a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 73a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 73 subtask 1 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 73b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 73b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 73 subtask 2 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 73c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 73c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 73 subtask 3 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 73d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 73d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 73 subtask 4 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 73):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 73 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §83-86 implemented

---


## Wave 74: Hot Reloading (Wave 74)

**Spec references:** Spec §83-86
**Scope:** Implement hot reloading: hot-swap protocol, state transfer, version management, rollback.
**Max parallel:** 4 (never more than 4)

### 74a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 74a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 74 subtask 1 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 74b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 74b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 74 subtask 2 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 74c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 74c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 74 subtask 3 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 74d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 74d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 74 subtask 4 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 74):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 74 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §83-86 implemented

---


## Wave 75: Hot Reloading (Wave 75)

**Spec references:** Spec §83-86
**Scope:** Implement hot reloading: hot-swap protocol, state transfer, version management, rollback.
**Max parallel:** 4 (never more than 4)

### 75a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 75a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 75 subtask 1 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 75b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 75b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 75 subtask 2 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 75c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 75c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 75 subtask 3 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 75d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 75d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 75 subtask 4 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 75):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 75 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §83-86 implemented

---


## Wave 76: Hot Reloading (Wave 76)

**Spec references:** Spec §83-86
**Scope:** Implement hot reloading: hot-swap protocol, state transfer, version management, rollback.
**Max parallel:** 4 (never more than 4)

### 76a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 76a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 76 subtask 1 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 76b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 76b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 76 subtask 2 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 76c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 76c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 76 subtask 3 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 76d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 76d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 76 subtask 4 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 76):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 76 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §83-86 implemented

---


## Wave 77: Hot Reloading (Wave 77)

**Spec references:** Spec §83-86
**Scope:** Implement hot reloading: hot-swap protocol, state transfer, version management, rollback.
**Max parallel:** 4 (never more than 4)

### 77a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 77a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 77 subtask 1 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 77b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 77b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 77 subtask 2 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 77c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 77c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 77 subtask 3 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 77d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 77d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 77 subtask 4 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 77):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 77 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §83-86 implemented

---


## Wave 78: Hot Reloading (Wave 78)

**Spec references:** Spec §83-86
**Scope:** Implement hot reloading: hot-swap protocol, state transfer, version management, rollback.
**Max parallel:** 4 (never more than 4)

### 78a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 78a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 78 subtask 1 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 78b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 78b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 78 subtask 2 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 78c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 78c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 78 subtask 3 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 78d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 78d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 78 subtask 4 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 78):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 78 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §83-86 implemented

---


## Wave 79: Hot Reloading (Wave 79)

**Spec references:** Spec §83-86
**Scope:** Implement hot reloading: hot-swap protocol, state transfer, version management, rollback.
**Max parallel:** 4 (never more than 4)

### 79a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 79a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 79 subtask 1 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 79b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 79b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 79 subtask 2 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 79c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 79c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 79 subtask 3 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 79d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 79d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 79 subtask 4 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 79):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 79 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §83-86 implemented

---


## Wave 80: Hot Reloading (Wave 80)

**Spec references:** Spec §83-86
**Scope:** Implement hot reloading: hot-swap protocol, state transfer, version management, rollback.
**Max parallel:** 4 (never more than 4)

### 80a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 80a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 80 subtask 1 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 80b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 80b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 80 subtask 2 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 80c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 80c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 80 subtask 3 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 80d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 80d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 80 subtask 4 per spec sections Spec §83-86
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 80):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 80 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §83-86 implemented

---


## Wave 81: Distributed Channels (Wave 81)

**Spec references:** Spec §87-91
**Scope:** Implement distributed IPC: location-transparent channels, worker discovery, network protocol, failure detection, consensus.
**Max parallel:** 4 (never more than 4)

### 81a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 81a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 81 subtask 1 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 81b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 81b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 81 subtask 2 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 81c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 81c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 81 subtask 3 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 81d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 81d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 81 subtask 4 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 81):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 81 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §87-91 implemented

---


## Wave 82: Distributed Channels (Wave 82)

**Spec references:** Spec §87-91
**Scope:** Implement distributed IPC: location-transparent channels, worker discovery, network protocol, failure detection, consensus.
**Max parallel:** 4 (never more than 4)

### 82a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 82a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 82 subtask 1 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 82b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 82b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 82 subtask 2 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 82c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 82c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 82 subtask 3 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 82d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 82d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 82 subtask 4 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 82):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 82 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §87-91 implemented

---


## Wave 83: Distributed Channels (Wave 83)

**Spec references:** Spec §87-91
**Scope:** Implement distributed IPC: location-transparent channels, worker discovery, network protocol, failure detection, consensus.
**Max parallel:** 4 (never more than 4)

### 83a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 83a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 83 subtask 1 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 83b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 83b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 83 subtask 2 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 83c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 83c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 83 subtask 3 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 83d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 83d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 83 subtask 4 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 83):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 83 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §87-91 implemented

---


## Wave 84: Distributed Channels (Wave 84)

**Spec references:** Spec §87-91
**Scope:** Implement distributed IPC: location-transparent channels, worker discovery, network protocol, failure detection, consensus.
**Max parallel:** 4 (never more than 4)

### 84a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 84a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 84 subtask 1 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 84b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 84b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 84 subtask 2 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 84c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 84c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 84 subtask 3 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 84d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 84d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 84 subtask 4 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 84):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 84 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §87-91 implemented

---


## Wave 85: Distributed Channels (Wave 85)

**Spec references:** Spec §87-91
**Scope:** Implement distributed IPC: location-transparent channels, worker discovery, network protocol, failure detection, consensus.
**Max parallel:** 4 (never more than 4)

### 85a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 85a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 85 subtask 1 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 85b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 85b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 85 subtask 2 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 85c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 85c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 85 subtask 3 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 85d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 85d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 85 subtask 4 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 85):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 85 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §87-91 implemented

---


## Wave 86: Distributed Channels (Wave 86)

**Spec references:** Spec §87-91
**Scope:** Implement distributed IPC: location-transparent channels, worker discovery, network protocol, failure detection, consensus.
**Max parallel:** 4 (never more than 4)

### 86a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 86a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 86 subtask 1 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 86b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 86b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 86 subtask 2 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 86c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 86c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 86 subtask 3 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 86d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 86d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 86 subtask 4 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 86):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 86 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §87-91 implemented

---


## Wave 87: Distributed Channels (Wave 87)

**Spec references:** Spec §87-91
**Scope:** Implement distributed IPC: location-transparent channels, worker discovery, network protocol, failure detection, consensus.
**Max parallel:** 4 (never more than 4)

### 87a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 87a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 87 subtask 1 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 87b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 87b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 87 subtask 2 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 87c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 87c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 87 subtask 3 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 87d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 87d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 87 subtask 4 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 87):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 87 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §87-91 implemented

---


## Wave 88: Distributed Channels (Wave 88)

**Spec references:** Spec §87-91
**Scope:** Implement distributed IPC: location-transparent channels, worker discovery, network protocol, failure detection, consensus.
**Max parallel:** 4 (never more than 4)

### 88a — Subtask 1

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 88a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 88 subtask 1 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 88b — Subtask 2

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 88b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 88 subtask 2 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 88c — Subtask 3

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 88c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 88 subtask 3 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 88d — Subtask 4

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 88d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 88 subtask 4 per spec sections Spec §87-91
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 88):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 88 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §87-91 implemented

---


## Wave 89: Compile-Time Encapsulation CT1 — Session Types (Part 1)

**Spec references:** Spec §21, §106
**Scope:** Add session type constructors to VUMA's type system: Send, Recv, Choice, Loop, End. Compile-time protocol verification.
**Max parallel:** 4 (never more than 4)

### 89a — Subtask 1

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs, src/parser/src/ast.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 89a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs, src/parser/src/ast.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 89 subtask 1 per spec sections Spec §21, §106
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 89b — Subtask 2

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs, src/parser/src/ast.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 89b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs, src/parser/src/ast.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 89 subtask 2 per spec sections Spec §21, §106
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 89c — Subtask 3

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs, src/parser/src/ast.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 89c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs, src/parser/src/ast.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 89 subtask 3 per spec sections Spec §21, §106
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 89d — Subtask 4

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs, src/parser/src/ast.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 89d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs, src/parser/src/ast.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 89 subtask 4 per spec sections Spec §21, §106
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 89):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 89 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §21, §106 implemented

---


## Wave 90: Compile-Time Encapsulation CT1 — Session Types (Part 2)

**Spec references:** Spec §21, §106
**Scope:** Implement dual session type computation, protocol state machine verification at compile time, deadlock freedom proof.
**Max parallel:** 4 (never more than 4)

### 90a — Subtask 1

**Files:** `src/codegen/src/opt.rs, src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 90a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/opt.rs, src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 90 subtask 1 per spec sections Spec §21, §106
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 90b — Subtask 2

**Files:** `src/codegen/src/opt.rs, src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 90b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/opt.rs, src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 90 subtask 2 per spec sections Spec §21, §106
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 90c — Subtask 3

**Files:** `src/codegen/src/opt.rs, src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 90c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/opt.rs, src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 90 subtask 3 per spec sections Spec §21, §106
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 90d — Subtask 4

**Files:** `src/codegen/src/opt.rs, src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 90d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/opt.rs, src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 90 subtask 4 per spec sections Spec §21, §106
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 90):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 90 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §21, §106 implemented

---


## Wave 91: Compile-Time Encapsulation CT2 — Information-Flow Types (Part 1)

**Spec references:** Spec §22, §107
**Scope:** Add security lattice types (Public, Internal, Secret, TopSecret) to VUMA's type system. Track labels through operations.
**Max parallel:** 4 (never more than 4)

### 91a — Subtask 1

**Files:** `src/codegen/src/ir.rs, src/parser/src/ast.rs, src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 91a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ir.rs, src/parser/src/ast.rs, src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 91 subtask 1 per spec sections Spec §22, §107
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 91b — Subtask 2

**Files:** `src/codegen/src/ir.rs, src/parser/src/ast.rs, src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 91b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ir.rs, src/parser/src/ast.rs, src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 91 subtask 2 per spec sections Spec §22, §107
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 91c — Subtask 3

**Files:** `src/codegen/src/ir.rs, src/parser/src/ast.rs, src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 91c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ir.rs, src/parser/src/ast.rs, src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 91 subtask 3 per spec sections Spec §22, §107
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 91d — Subtask 4

**Files:** `src/codegen/src/ir.rs, src/parser/src/ast.rs, src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 91d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ir.rs, src/parser/src/ast.rs, src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 91 subtask 4 per spec sections Spec §22, §107
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 91):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 91 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §22, §107 implemented

---


## Wave 92: Compile-Time Encapsulation CT2 — Information-Flow Types (Part 2)

**Spec references:** Spec §22, §107, §115
**Scope:** Implement information-flow verification: Secret→Public leakage detection, explicit downgrading, cross-process label tracking.
**Max parallel:** 4 (never more than 4)

### 92a — Subtask 1

**Files:** `src/codegen/src/opt.rs, src/ive/src/verification.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 92a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/opt.rs, src/ive/src/verification.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 92 subtask 1 per spec sections Spec §22, §107, §115
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 92b — Subtask 2

**Files:** `src/codegen/src/opt.rs, src/ive/src/verification.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 92b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/opt.rs, src/ive/src/verification.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 92 subtask 2 per spec sections Spec §22, §107, §115
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 92c — Subtask 3

**Files:** `src/codegen/src/opt.rs, src/ive/src/verification.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 92c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/opt.rs, src/ive/src/verification.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 92 subtask 3 per spec sections Spec §22, §107, §115
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 92d — Subtask 4

**Files:** `src/codegen/src/opt.rs, src/ive/src/verification.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 92d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/opt.rs, src/ive/src/verification.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 92 subtask 4 per spec sections Spec §22, §107, §115
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 92):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 92 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §22, §107, §115 implemented

---


## Wave 93: Compile-Time Encapsulation CT6 — zk-STARK Architecture (Part 1)

**Spec references:** Spec §26, §34-38
**Scope:** Implement zk-STARK proof system for capability attestation: AIR definition, proof generation, proof verification.
**Max parallel:** 4 (never more than 4)

### 93a — Subtask 1

**Files:** `src/codegen/src/capability.rs (new stark module)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 93a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs (new stark module)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 93 subtask 1 per spec sections Spec §26, §34-38
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 93b — Subtask 2

**Files:** `src/codegen/src/capability.rs (new stark module)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 93b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs (new stark module)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 93 subtask 2 per spec sections Spec §26, §34-38
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 93c — Subtask 3

**Files:** `src/codegen/src/capability.rs (new stark module)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 93c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs (new stark module)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 93 subtask 3 per spec sections Spec §26, §34-38
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 93d — Subtask 4

**Files:** `src/codegen/src/capability.rs (new stark module)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 93d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs (new stark module)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 93 subtask 4 per spec sections Spec §26, §34-38
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 93):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 93 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §26, §34-38 implemented

---


## Wave 94: Compile-Time Encapsulation CT6 — zk-STARK Architecture (Part 2)

**Spec references:** Spec §26, §36-39
**Scope:** Implement STARK for protocol compliance and memory safety attestation. Post-quantum security analysis.
**Max parallel:** 4 (never more than 4)

### 94a — Subtask 1

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 94a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 94 subtask 1 per spec sections Spec §26, §36-39
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 94b — Subtask 2

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 94b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 94 subtask 2 per spec sections Spec §26, §36-39
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 94c — Subtask 3

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 94c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 94 subtask 3 per spec sections Spec §26, §36-39
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 94d — Subtask 4

**Files:** `src/codegen/src/capability.rs, src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 94d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs, src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 94 subtask 4 per spec sections Spec §26, §36-39
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 94):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 94 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §26, §36-39 implemented

---


## Wave 95: Compile-Time Encapsulation CT3-CT5, CT7-CT8 — Refinement, Linear, Homomorphic, CSL-Perm, Noise

**Spec references:** Spec §23-25, §27-28, §108
**Scope:** Implement refinement types (CT3), linear capability types (CT4), homomorphic encapsulation (CT5), CSL-Perm fractional permissions (CT7), Noise protocol channels (CT8).
**Max parallel:** 4 (never more than 4)

### 95a — Subtask 1

**Files:** `src/codegen/src/ir.rs, src/codegen/src/ipc.rs, src/ive/src/borrow_region.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 95a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ir.rs, src/codegen/src/ipc.rs, src/ive/src/borrow_region.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 95 subtask 1 per spec sections Spec §23-25, §27-28, §108
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 95b — Subtask 2

**Files:** `src/codegen/src/ir.rs, src/codegen/src/ipc.rs, src/ive/src/borrow_region.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 95b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ir.rs, src/codegen/src/ipc.rs, src/ive/src/borrow_region.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 95 subtask 2 per spec sections Spec §23-25, §27-28, §108
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 95c — Subtask 3

**Files:** `src/codegen/src/ir.rs, src/codegen/src/ipc.rs, src/ive/src/borrow_region.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 95c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ir.rs, src/codegen/src/ipc.rs, src/ive/src/borrow_region.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 95 subtask 3 per spec sections Spec §23-25, §27-28, §108
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 95d — Subtask 4

**Files:** `src/codegen/src/ir.rs, src/codegen/src/ipc.rs, src/ive/src/borrow_region.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 95d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ir.rs, src/codegen/src/ipc.rs, src/ive/src/borrow_region.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 95 subtask 4 per spec sections Spec §23-25, §27-28, §108
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 95):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 95 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §23-25, §27-28, §108 implemented

---


## Wave 96: Formal Verification — L1-L3 + 5→3 Invariant Collapse Proof

**Spec references:** Spec §29-33, §132
**Scope:** Add verification hooks for L1 (boot assembly), L2 (FFI trampolines), L3 (arena runtime). Document the 5→3 invariant collapse proof outline.
**Max parallel:** 4 (never more than 4)

### 96a — Subtask 1

**Files:** `src/ive/src/verification.rs, src/ive/src/invariant_aggregator.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 96a
Subtask 1

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/ive/src/verification.rs, src/ive/src/invariant_aggregator.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 96 subtask 1 per spec sections Spec §29-33, §132
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 1 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 96b — Subtask 2

**Files:** `src/ive/src/verification.rs, src/ive/src/invariant_aggregator.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 96b
Subtask 2

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/ive/src/verification.rs, src/ive/src/invariant_aggregator.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 96 subtask 2 per spec sections Spec §29-33, §132
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 2 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 96c — Subtask 3

**Files:** `src/ive/src/verification.rs, src/ive/src/invariant_aggregator.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 96c
Subtask 3

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/ive/src/verification.rs, src/ive/src/invariant_aggregator.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 96 subtask 3 per spec sections Spec §29-33, §132
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 3 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

### 96d — Subtask 4

**Files:** `src/ive/src/verification.rs, src/ive/src/invariant_aggregator.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 96d
Subtask 4

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/ive/src/verification.rs, src/ive/src/invariant_aggregator.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement wave 96 subtask 4 per spec sections Spec §29-33, §132
2. Add the required types, functions, or IR instructions
3. Add unit tests
4. Build: cargo build --workspace
5. Run tests

Acceptance:
- Subtask 4 implemented
- Unit tests pass
- cargo build --workspace succeeds

Do NOT touch womb/kernel/**. Do NOT run git.
```

</details>

**Definition of Done (Wave 96):**

- [ ] cargo build --workspace succeeds
- [ ] Wave 96 subtasks complete
- [ ] No regressions in existing tests
- [ ] Spec sections Spec §29-33, §132 implemented

---


## Final Wave (96): Full System Integration

**Definition of Done (entire plan):**
- [ ] All 96 waves completed and DoD approved by main agent
- [ ] `cargo build --workspace` succeeds with zero errors
- [ ] All gold-standard tests pass on x86_64, aarch64, riscv64, arm32
- [ ] IPC channel send/recv works across processes on all 4 backends
- [ ] FFI process isolation: `extern "process"` spawns isolated worker
- [ ] Capability system: grant/revoke/verify/delegate works
- [ ] 8 runtime encapsulation layers implemented and tested
- [ ] 8 compile-time encapsulation algorithms implemented
- [ ] zk-STARK attestation: proof generation + verification works
- [ ] Session types: compile-time protocol verification works
- [ ] Information-flow types: Secret→Public leakage detected at compile time
- [ ] Noise protocol: secure channel establishment works over TCP
- [ ] Fault tolerance: worker crash → supervisor restart → state preserved
- [ ] Hot reloading: worker hot-swap without losing state
- [ ] Distributed channels: remote IPC over TCP with Noise encryption
- [ ] L1-L3 formal verification hooks exist
- [ ] 5→3 invariant collapse proof outline documented

*Generated from docs/VUMA_PROCESS_ISOLATION_SPEC.md v2.0 (43,788 lines)*

