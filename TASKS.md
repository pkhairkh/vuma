# VUMA Process Isolation Architecture — Wave-Based Implementation Plan

> **Source:** `docs/VUMA_PROCESS_ISOLATION_SPEC.md` v2.0
> **Total waves:** 96
> **Total subtasks:** ~600
> **Max parallel per wave:** 4 (never launch more than 4 subagents at once)

## Rules

1. **Max 4 subagents per wave.** Never launch more than 4 simultaneously.
2. **Independent code domains.** Subtasks in the same wave MUST NOT edit the same file.
3. **Surgical prompts.** Each subtask prompt is self-contained — the subagent does not
   need to read the full spec. The prompt includes: files to edit, exact changes,
   acceptance criteria, and the QEMU test command.
4. **DoD per wave.** After each wave, the main agent verifies the Definition of Done.
   If any item fails, the wave is not complete. Fix before proceeding.
5. **Git protocol.** Main agent pulls before each wave, commits after each wave,
   pushes after DoD approval. Subagents do NOT run git.
6. **Build check.** After each wave, `cargo build --workspace` must succeed.
7. **Test check.** After each wave, the existing gold-standard tests must still pass.
8. **No kernel files.** Subagents MUST NOT touch `womb/kernel/**` — the kernel team
   owns those files. If a wave requires kernel changes, the main agent does them.
9. **QEMU verification.** Each subagent tests on QEMU where applicable.
10. **Worklog.** Each subagent appends to `/home/z/my-project/worklog.md`.

## Wave Map

| Waves | Title | Phases |
|-------|-------|--------|
| 1-8 | IPC Primitives (Phase 1) | Channel type, spawn, send/recv, lifecycle |
| 9-16 | Runtime Encapsulation Layers L1-L4 (Phase 1) | Framing, capabilities, memory, protocol |
| 17-24 | Runtime Encapsulation Layers L5-L8 (Phase 1) | Worker, state, error, crypto |
| 25-32 | FFI Process Isolation (Phase 2) | extern "process", marshalling, seccomp, crash recovery |
| 33-40 | Capability System (Phase 4) | Type, grant/revoke, delegation, flow, revocation |
| 41-48 | Kernel/User Split (Phase 3) | Microkernel, syscall-as-IPC, resource accounting |
| 49-56 | Driver Isolation (Phase 5) | MMIO caps, IRQ channels, DMA, restart |
| 57-64 | Sandboxing (Phase 2 cont.) | Zero-cap workers, plugins, parsers, crypto |
| 65-72 | Fault Tolerance (Phase 2 cont.) | Supervisor, crash detection, checkpointing, circuit breaker |
| 73-80 | Hot Reloading (Phase 2 cont.) | Hot-swap, state transfer, rollback |
| 81-88 | Distributed Channels (Phase 6) | Location-transparent, discovery, Noise, consensus |
| 89-96 | Compile-Time Encapsulation (Phases 7-10) | Session types, IFC, STARK, L1-L3 verification |

---


## Wave 1: Channel<T> Type — IR + Type System

**Spec reference:** Spec §101, §45, §7
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 1a — Add Channel<T> to ScgType and IRType

**Files:** `src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 1a
Add Channel<T> to ScgType and IRType enums

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add `Channel(Box<ScgType>)` variant to ScgType enum
2. Add `Channel(Box<IRType>)` variant to IRType enum
3. Add `to_ir_type()` mapping for Channel
4. Add `size_of()` for Channel (pointer-sized: 8 on 64-bit, 4 on 32-bit)
5. Add Display impl for Channel variant
6. Build: cargo build --workspace

Acceptance:
- cargo build succeeds
- Channel variant exists in both enums
- to_ir_type maps correctly

```

</details>

### 1b — Add Channel<T> to parser AST

**Files:** `src/parser/src/ast.rs, src/parser/src/parser.rs, src/parser/src/lexer.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 1b
Add Channel type to parser

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/parser/src/ast.rs, src/parser/src/parser.rs, src/parser/src/lexer.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Channel keyword to lexer (TokenKind::Channel)
2. Add Type::Channel(Box<Type>) to AST
3. Parse `Channel<T>` as a type
4. Add Display for Channel type
5. Build: cargo build --workspace

Acceptance:
- `Channel<i32>` parses as a type
- cargo build succeeds

```

</details>

### 1c — Add Channel<T> to pipeline bridge

**Files:** `src/pipeline.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 1c
Bridge Channel type from AST to codegen SCG

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/pipeline.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Channel mapping in bridge_type_to_codegen_scg
2. Add Channel mapping in bridge_type_to_ir_type
3. Add Channel in type_size/alignment functions
4. Build: cargo build --workspace

Acceptance:
- Channel type bridges correctly AST→SCG→IR
- cargo build succeeds

```

</details>

**Definition of Done (Wave 1):**
- [ ] cargo build --workspace succeeds
- [ ] Channel<T> type exists in ScgType, IRType, and parser AST
- [ ] Channel<T> parses in VUMA source: `let ch: Channel<i32>` compiles

---


## Wave 2: Channel<T> — IR Instructions + Codegen Stub

**Spec reference:** Spec §45, §46
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 2a — Add IR instructions for channel operations

**Files:** `src/codegen/src/ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 2a
Add ChannelSend/ChannelRecv/ChannelOpen/ChannelClose IR instructions

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::ChannelSend { ch, msg, ty } 
2. Add IRInstr::ChannelRecv { ch, dst, ty }
3. Add IRInstr::ChannelOpen { dst, elem_ty } — allocates a channel
4. Add IRInstr::ChannelClose { ch } — deallocates
5. Add Display impls
6. Add effects (reads/writes) for each
7. Build: cargo build --workspace

Acceptance:
- 4 new IR instructions exist
- Display impls work
- cargo build succeeds

```

</details>

### 2b — Add channel operations to SCG

**Files:** `src/codegen/src/scg_to_ir.rs, src/scg/src/node.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 2b
Add ChannelOpen/Send/Recv/Close to SCG nodes and IR lowering

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs, src/scg/src/node.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add ChannelOpenNode, ChannelSendNode, ChannelRecvNode, ChannelCloseNode to SCG
2. Add IR lowering for each (emit the IR instructions from 2a)
3. Add channel vreg type tracking
4. Build: cargo build --workspace

Acceptance:
- SCG nodes exist for channel ops
- IR lowering produces correct instructions
- cargo build succeeds

```

</details>

### 2c — Add channel builtins to parser

**Files:** `src/parser/src/parser.rs, src/parser/src/to_scg.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 2c
Parse channel builtins: channel_open, channel_send, channel_recv, channel_close

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/parser/src/parser.rs, src/parser/src/to_scg.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Parse `channel_open<T>()` as a builtin call
2. Parse `channel_send(ch, val)` as a builtin call
3. Parse `channel_recv(ch)` as a builtin call
4. Parse `channel_close(ch)` as a builtin call
5. Lower to SCG ChannelOpen/Send/Recv/Close nodes
6. Build: cargo build --workspace

Acceptance:
- channel builtins parse
- They lower to correct SCG nodes
- cargo build succeeds

```

</details>

**Definition of Done (Wave 2):**
- [ ] cargo build --workspace succeeds
- [ ] IR has ChannelSend/Recv/Open/Close instructions
- [ ] SCG has corresponding nodes
- [ ] Parser recognizes channel_open/send/recv/close builtins

---


## Wave 3: Channel<T> — Hosted Backend (x86_64)

**Spec reference:** Spec §45, §117
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 3a — Implement channel ops in x86_64 backend

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs, src/codegen/src/x86_64/mod.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 3a
Implement ChannelOpen/Send/Recv/Close in x86_64 backend

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs, src/codegen/src/x86_64/mod.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::ChannelOpen handler: allocate channel buffer (mmap), return pointer
2. Add IRInstr::ChannelSend handler: write to channel buffer (memcpy + ring buffer index)
3. Add IRInstr::ChannelRecv handler: read from channel buffer (memcpy + ring buffer index)
4. Add IRInstr::ChannelClose handler: munmap the channel buffer
5. Use a simple ring buffer (head/tail/size) for the channel
6. Test: compile a simple send/recv program and run on x86_64
7. Build: cargo build --workspace

Acceptance:
- channel_send + channel_recv works on x86_64
- Simple ring buffer channel
- Test: `fn main() { ch = channel_open(); ch.send(42); x = ch.recv(); return x; }` returns 42

```

</details>

### 3b — Add channel type to LSP

**Files:** `src/lsp/mod.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 3b
Add Channel<T> hover and completion in LSP

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/lsp/mod.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Channel type to format_type
2. Add hover for channel_open/send/recv/close builtins
3. Add completion for Channel<T>
4. Build: cargo build --workspace

Acceptance:
- LSP shows Channel type info
- channel builtins have hover text
- cargo build succeeds

```

</details>

**Definition of Done (Wave 3):**
- [ ] cargo build --workspace succeeds
- [ ] Channel send/recv works on x86_64 (ring buffer)
- [ ] Test: send 42, recv 42 on x86_64
- [ ] LSP shows Channel type info

---


## Wave 4: Channel<T> — Cross-Backend Support

**Spec reference:** Spec §45
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 4a — Add channel ops to aarch64 backend

**Files:** `src/codegen/src/emit.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 4a
Implement channel ops in aarch64 backend

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/emit.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::ChannelOpen/Send/Recv/Close to ss_emit_instr
2. Use the same ring buffer approach as x86_64
3. Channel is pointer-sized (8 bytes on aarch64)
4. Test on qemu-aarch64-static
5. Build: cargo build --workspace

Acceptance:
- channel send/recv works on aarch64
- Test passes on QEMU-aarch64

```

</details>

### 4b — Add channel ops to riscv64 backend

**Files:** `src/codegen/src/riscv64.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 4b
Implement channel ops in riscv64 backend

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/riscv64.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::ChannelOpen/Send/Recv/Close handlers
2. Ring buffer approach
3. Test on qemu-riscv64-static
4. Build: cargo build --workspace

Acceptance:
- channel send/recv works on riscv64
- Test passes on QEMU-riscv64

```

</details>

### 4c — Add channel ops to wasm32 backend

**Files:** `src/codegen/src/wasm32/mod.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 4c
Implement channel ops in wasm32 backend (compile-only)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/wasm32/mod.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::ChannelOpen/Send/Recv/Close handlers
2. Use linear memory for ring buffer
3. Build: cargo build --workspace
4. Verify compile succeeds (no wasm runtime needed)

Acceptance:
- channel ops compile on wasm32
- cargo build succeeds

```

</details>

### 4d — Add channel ops to arm32 backend

**Files:** `src/codegen/src/arm32/mod.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 4d
Implement channel ops in arm32 backend

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/arm32/mod.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add IRInstr::ChannelOpen/Send/Recv/Close handlers
2. Ring buffer approach (pointer is 4 bytes on arm32)
3. Test on qemu-arm-static
4. Build: cargo build --workspace

Acceptance:
- channel send/recv works on arm32
- Test passes on QEMU-arm

```

</details>

**Definition of Done (Wave 4):**
- [ ] cargo build --workspace succeeds
- [ ] Channel send/recv works on x86_64, aarch64, riscv64, arm32
- [ ] Channel ops compile on wasm32

---


## Wave 5: spawn_worker — IR + Hosted Implementation

**Spec reference:** Spec §8, §56
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 5a — Add spawn_worker builtin to parser + SCG + IR

**Files:** `src/parser/src/parser.rs, src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 5a
Add spawn_worker builtin

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/parser/src/parser.rs, src/codegen/src/scg_to_ir.rs, src/codegen/src/ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Parse `spawn_worker("path")` as a builtin call
2. Add IRInstr::SpawnWorker { dst, path } — spawns a child process
3. Lower to SCG
4. The worker runs as a separate OS process (fork+exec on hosted)
5. Build: cargo build --workspace

Acceptance:
- spawn_worker parses and lowers to IR
- cargo build succeeds

```

</details>

### 5b — Implement spawn_worker on x86_64 (hosted/Linux)

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 5b
Implement spawn_worker on x86_64 via fork+exec

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Emit fork() syscall
2. In child: exec the worker binary
3. In parent: store child PID in dst vreg
4. The worker binary is a separately-compiled VUMA program
5. Test: spawn a worker that exits 42, parent reads exit code
6. Build: cargo build --workspace

Acceptance:
- spawn_worker creates a child process on x86_64
- Child process runs independently

```

</details>

**Definition of Done (Wave 5):**
- [ ] cargo build --workspace succeeds
- [ ] spawn_worker builtin exists in parser + IR
- [ ] spawn_worker works on x86_64 (fork+exec)

---


## Wave 6: IPC Channel — Process-to-Process Communication

**Spec reference:** Spec §45, §117
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 6a — Implement IPC channel transport (pipe-based)

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 6a
Implement IPC channel via pipe() on x86_64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. ChannelOpen creates a pipe (pipe syscall) — returns read+write fds
2. ChannelSend writes to the pipe write fd (write syscall)
3. ChannelRecv reads from the pipe read fd (read syscall)
4. After spawn_worker, parent and child share the pipe
5. Test: parent sends 42, child receives 42
6. Build: cargo build --workspace

Acceptance:
- IPC channel works between parent and child process
- Test: parent sends, child receives

```

</details>

### 6b — Add channel_close and worker kill

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 6b
Implement channel_close and worker kill

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. ChannelClose: close(read_fd) + close(write_fd)
2. Add kill_worker(pid) builtin: kill(pid, SIGTERM)
3. Add wait_worker(pid) builtin: waitpid(pid, &status, 0)
4. Test: spawn, send, recv, close, kill
5. Build: cargo build --workspace

Acceptance:
- channel_close closes fds
- kill_worker terminates child
- wait_worker reaps zombie

```

</details>

**Definition of Done (Wave 6):**
- [ ] cargo build --workspace succeeds
- [ ] IPC channel works: parent sends 42 via pipe, child receives 42
- [ ] channel_close + kill_worker work on x86_64

---


## Wave 7: IPC Channel — Cross-Backend

**Spec reference:** Spec §45
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 7a — IPC channel on aarch64

**Files:** `src/codegen/src/emit.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 7a
Implement IPC channel on aarch64 via pipe

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/emit.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Use pipe() syscall (aarch64 sys_pipe=59)
2. write() and read() syscalls for send/recv
3. Test on qemu-aarch64-static
4. Build: cargo build --workspace

Acceptance:
- IPC channel works on aarch64
- Test passes on QEMU

```

</details>

### 7b — IPC channel on riscv64

**Files:** `src/codegen/src/riscv64.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 7b
Implement IPC channel on riscv64 via pipe

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/riscv64.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Use pipe() syscall (riscv64 sys_pipe=21)
2. write/read syscalls
3. Test on qemu-riscv64-static
4. Build: cargo build --workspace

Acceptance:
- IPC channel works on riscv64
- Test passes on QEMU

```

</details>

### 7c — IPC channel on arm32

**Files:** `src/codegen/src/arm32/mod.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 7c
Implement IPC channel on arm32 via pipe

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/arm32/mod.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Use pipe() syscall (arm sys_pipe=42 via SWI)
2. write/read syscalls
3. Test on qemu-arm-static
4. Build: cargo build --workspace

Acceptance:
- IPC channel works on arm32
- Test passes on QEMU

```

</details>

**Definition of Done (Wave 7):**
- [ ] cargo build --workspace succeeds
- [ ] IPC channel works on x86_64, aarch64, riscv64, arm32

---


## Wave 8: Channel Lifecycle + Deadlock Detection

**Spec reference:** Spec §48, §49
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 8a — Channel lifecycle management

**Files:** `src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 8a
Add channel lifecycle: open, close, error handling

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Track open channels in IRBuilder state
2. At function exit, auto-close any unclosed channels (warning)
3. Add channel_try_recv (non-blocking) — returns Option<T>
4. Add channel_is_closed(ch) — checks if other end closed
5. Build: cargo build --workspace

Acceptance:
- Channels auto-close at function exit
- channel_try_recv works (non-blocking)
- channel_is_closed detects peer closure

```

</details>

### 8b — Deadlock detection (static analysis)

**Files:** `src/codegen/src/opt.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 8b
Add compile-time deadlock detection for channels

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/opt.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Build a wait-for graph from channel recv operations
2. If process A blocks on recv from channel C, and the sender of C is B,
   add edge A→B
3. Detect cycles via DFS
4. Emit a warning (not error) if a potential deadlock is found
5. Build: cargo build --workspace

Acceptance:
- Deadlock detection runs as an optimization pass
- Circular wait is detected and warned

```

</details>

**Definition of Done (Wave 8):**
- [ ] cargo build --workspace succeeds
- [ ] Channel lifecycle: open/close/auto-close works
- [ ] Deadlock detection: warns on circular wait

---


## Wave 9: Runtime Encapsulation L1 — Message Framing

**Spec reference:** Spec §12
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 9a — Implement message wire format

**Files:** `src/codegen/src/ipc.rs (new file)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 9a
Create IPC message wire format module

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs (new file)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create new module src/codegen/src/ipc.rs
2. Define MessageHeader struct (magic, version, flags, channel_id, sequence, type_hash, payload_len, cap_count)
3. Define frame_message() and deframe_message() functions
4. CRC32 checksum computation
5. Add module to lib.rs
6. Build: cargo build --workspace

Acceptance:
- Wire format module exists
- frame/deframe roundtrip works
- CRC32 verification works

```

</details>

### 9b — Implement type hash computation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 9b
Add FNV-1a type hash for IPC messages

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement type_hash(ScgType) -> u64 using FNV-1a
2. Canonical type string for all ScgType variants
3. Struct types include name + field types
4. Test: same type → same hash, different type → different hash
5. Build: cargo build --workspace

Acceptance:
- Type hash is deterministic
- Same type always produces same hash

```

</details>

**Definition of Done (Wave 9):**
- [ ] cargo build --workspace succeeds
- [ ] IPC module exists with wire format + framing + CRC32
- [ ] Type hash is deterministic and correct

---


## Wave 10: Runtime Encapsulation L1 — Integrate Framing into Channel

**Spec reference:** Spec §12
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 10a — Integrate message framing into channel send/recv (x86_64)

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 10a
Wrap channel send/recv with message framing on x86_64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. ChannelSend: serialize value → frame_message → write framed bytes to pipe
2. ChannelRecv: read framed bytes from pipe → deframe_message → deserialize value
3. Verify CRC32 on receive (return error if mismatch)
4. Verify type_hash matches expected type
5. Test: send 42 as i32, receive 42 as i32 — CRC passes
6. Build: cargo build --workspace

Acceptance:
- Messages are framed with header + CRC32
- CRC mismatch is detected
- Type hash mismatch is detected

```

</details>

### 10b — Add serialization for primitive types

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 10b
Add serialize/deserialize for i32, i64, u32, u64, f32, f64, bool

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. serialize_i32(i32) -> Vec<u8> — little-endian 4 bytes
2. serialize_i64(i64) -> Vec<u8> — little-endian 8 bytes
3. Same for u32, u64, f32, f64, bool
4. deserialize counterparts
5. Test: roundtrip all primitive types
6. Build: cargo build --workspace

Acceptance:
- All primitive types serialize/deserialize correctly
- Roundtrip preserves value

```

</details>

**Definition of Done (Wave 10):**
- [ ] cargo build --workspace succeeds
- [ ] Channel messages are framed (header + CRC + type hash)
- [ ] Serialization works for all primitive types
- [ ] Test: send i32 across pipe, receive correct value

---


## Wave 11: Runtime Encapsulation L2 — Capability Tokens

**Spec reference:** Spec §13, §51
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 11a — Define CapabilityToken struct

**Files:** `src/codegen/src/capability.rs (new file)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 11a
Create capability module with CapabilityToken struct

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs (new file)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create src/codegen/src/capability.rs
2. Define CapabilityToken: id (UUID), source_pid, target_pid, resource, permissions, delegation_depth, created_at, expires_at, signature
3. Define MemoryPermissions (read/write/execute)
4. Define CapabilitySet (HashMap of tokens)
5. Add encode()/decode() for serialization
6. Add to lib.rs
7. Build: cargo build --workspace

Acceptance:
- Capability module exists
- CapabilityToken can be encoded/decoded

```

</details>

### 11b — Implement capability grant/verify

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 11b
Implement grant_capability and verify_capability

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. grant_capability(source, target, resource, perms) -> CapabilityToken
2. verify_capability(token, required_resource, required_perms) -> bool
3. Check: signature valid, not expired, resource matches, permissions sufficient, not revoked
4. Test: grant then verify succeeds; wrong resource fails; expired fails
5. Build: cargo build --workspace

Acceptance:
- Grant creates a valid token
- Verify checks signature, expiry, resource, permissions

```

</details>

**Definition of Done (Wave 11):**
- [ ] cargo build --workspace succeeds
- [ ] Capability module with grant/verify exists
- [ ] CapabilityToken can be encoded for IPC

---


## Wave 12: Runtime Encapsulation L2 — Capability in IPC

**Spec reference:** Spec §13
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 12a — Attach capabilities to IPC messages

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 12a
Add capability attachment to framed messages

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add capabilities field to EncapsulatedMessage
2. frame_message includes capabilities after payload
3. deframe_message reads capabilities
4. Capabilities are signed tokens — receiver verifies signature
5. Test: send message with capability, receiver verifies
6. Build: cargo build --workspace

Acceptance:
- Messages carry capability tokens
- Receiver verifies capability signatures

```

</details>

### 12b — Capability revocation registry

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 12b
Implement capability revocation registry

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add RevocationRegistry (HashSet of revoked capability IDs)
2. revoke(capability_id) — adds to registry
3. is_revoked(capability_id) -> bool
4. verify_capability checks revocation registry
5. Test: grant → verify OK → revoke → verify fails
6. Build: cargo build --workspace

Acceptance:
- Revocation works
- Revoked capabilities fail verification

```

</details>

**Definition of Done (Wave 12):**
- [ ] cargo build --workspace succeeds
- [ ] IPC messages carry signed capability tokens
- [ ] Capability revocation works

---


## Wave 13: Runtime Encapsulation L3 — Memory Windows

**Spec reference:** Spec §14, §9
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 13a — Define MemoryWindow struct

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 13a
Add MemoryWindow for shared memory between processes

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define MemoryWindow: source_pid, target_pid, source_addr, target_addr, size, permissions, capability_id, revocable
2. Define grant_memory() and revoke_memory() functions
3. Grant maps physical pages into target process (mmap + MAP_SHARED)
4. Revoke unmaps pages from target
5. Build: cargo build --workspace

Acceptance:
- MemoryWindow struct exists
- Grant/revoke protocol defined

```

</details>

### 13b — Implement memory window on x86_64

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 13b
Implement shared memory windows on x86_64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. grant_memory: mmap(NULL, size, PROT_READ|PROT_WRITE, MAP_SHARED|MAP_ANONYMOUS, -1, 0)
2. Share the fd with child via SCM_RIGHTS (sendmsg)
3. Child mmaps the same fd
4. revoke: munmap in child
5. Test: parent writes to shared memory, child reads
6. Build: cargo build --workspace

Acceptance:
- Shared memory window works on x86_64
- Parent writes, child reads

```

</details>

**Definition of Done (Wave 13):**
- [ ] cargo build --workspace succeeds
- [ ] MemoryWindow defined and implemented on x86_64
- [ ] Test: shared memory write/read across processes

---


## Wave 14: Runtime Encapsulation L4 — Protocol State Machine

**Spec reference:** Spec §15, §50
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 14a — Define channel protocol state machine

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 14a
Add protocol state machine for channels

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define ProtocolState enum (Idle, WaitingForSend, WaitingForRecv, Closed)
2. Define allowed_transitions: which message types are valid in each state
3. channel_protocol_check(channel, message_type) -> Result<ProtocolState, Error>
4. If message type not allowed in current state → ProtocolViolation error
5. Test: valid sequence passes, invalid sequence fails
6. Build: cargo build --workspace

Acceptance:
- Protocol state machine exists
- Invalid message order is rejected

```

</details>

### 14b — Integrate state machine into channel recv

**Files:** `src/codegen/src/x86_64/stack_slot_isel.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 14b
Integrate protocol check into channel recv on x86_64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/x86_64/stack_slot_isel.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. After deframing a message, call channel_protocol_check
2. If protocol violation, return error (don't deliver message)
3. Update channel state on successful receive
4. Test: send messages in wrong order → error
5. Build: cargo build --workspace

Acceptance:
- Protocol violations are detected at runtime
- Valid sequences work correctly

```

</details>

**Definition of Done (Wave 14):**
- [ ] cargo build --workspace succeeds
- [ ] Protocol state machine rejects invalid message sequences
- [ ] Valid sequences work correctly on x86_64

---


## Wave 15: Runtime Encapsulation L1-L4 — Cross-Backend

**Spec reference:** Spec §12-15
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 15a — Port framed channels to aarch64

**Files:** `src/codegen/src/emit.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 15a
Port framed IPC channels to aarch64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/emit.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Use the same wire format + framing
2. Implement on aarch64 via pipe syscalls
3. Test on qemu-aarch64-static
4. Build: cargo build --workspace

Acceptance:
- Framed IPC works on aarch64
- CRC verification works

```

</details>

### 15b — Port framed channels to riscv64

**Files:** `src/codegen/src/riscv64.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 15b
Port framed IPC channels to riscv64

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/riscv64.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Same wire format + framing
2. pipe/write/read syscalls
3. Test on qemu-riscv64-static
4. Build: cargo build --workspace

Acceptance:
- Framed IPC works on riscv64

```

</details>

### 15c — Port framed channels to arm32

**Files:** `src/codegen/src/arm32/mod.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 15c
Port framed IPC channels to arm32

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/arm32/mod.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Same wire format + framing
2. pipe/write/read syscalls
3. Test on qemu-arm-static
4. Build: cargo build --workspace

Acceptance:
- Framed IPC works on arm32

```

</details>

**Definition of Done (Wave 15):**
- [ ] cargo build --workspace succeeds
- [ ] Framed IPC channels work on x86_64, aarch64, riscv64, arm32

---


## Wave 16: Runtime Encapsulation L1-L4 — Integration Tests

**Spec reference:** Spec §12-15, §134
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 16a — Integration test: framed send/recv across processes

**Files:** `tests/gold_standard/ipc/ (new dir)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 16a
Create IPC integration tests

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ipc/ (new dir)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create tests/gold_standard/ipc/framed_send_recv.vuma — send i32, receive i32
2. Create tests/gold_standard/ipc/capability_check.vuma — grant + verify capability
3. Create tests/gold_standard/ipc/protocol_violation.vuma — wrong message order → error
4. Create tests/gold_standard/ipc/shared_memory.vuma — write/read shared memory
5. Each test has // Expected exit code header
6. Run on x86_64
7. Build: cargo build --workspace

Acceptance:
- 4 IPC integration tests exist
- All pass on x86_64

```

</details>

### 16b — Add IPC tests to CI runner

**Files:** `Makefile`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 16b
Add IPC test target to Makefile

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: Makefile
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add `ipc-test` target to Makefile
2. Compile + run each test in tests/gold_standard/ipc/
3. Check exit codes
4. Build: cargo build --workspace

Acceptance:
- `make ipc-test` runs all IPC tests
- All pass

```

</details>

**Definition of Done (Wave 16):**
- [ ] cargo build --workspace succeeds
- [ ] 4 IPC integration tests exist and pass on x86_64
- [ ] make ipc-test target exists

---


## Wave 17: Runtime Encapsulation L5-L8 (Wave 17)

**Spec reference:** Spec §16-19
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 17a — Worker Encapsulation (L5) — Implementation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 17a
Implement Worker Encapsulation (L5)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement Worker Encapsulation (L5) algorithm per spec
2. Add to encapsulation pipeline
3. Test
4. Build: cargo build --workspace

Acceptance:
- Worker Encapsulation (L5) implemented
- Tests pass

```

</details>

### 17b — State Encapsulation (L6) — Implementation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 17b
Implement State Encapsulation (L6)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement State Encapsulation (L6) algorithm per spec
2. Add to encapsulation pipeline
3. Test
4. Build: cargo build --workspace

Acceptance:
- State Encapsulation (L6) implemented
- Tests pass

```

</details>

### 17c — Error Encapsulation (L7) — Implementation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 17c
Implement Error Encapsulation (L7)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement Error Encapsulation (L7) algorithm per spec
2. Add to encapsulation pipeline
3. Test
4. Build: cargo build --workspace

Acceptance:
- Error Encapsulation (L7) implemented
- Tests pass

```

</details>

### 17d — Cryptographic Encapsulation (L8) — Implementation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 17d
Implement Cryptographic Encapsulation (L8)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement Cryptographic Encapsulation (L8) algorithm per spec
2. Add to encapsulation pipeline
3. Test
4. Build: cargo build --workspace

Acceptance:
- Cryptographic Encapsulation (L8) implemented
- Tests pass

```

</details>

**Definition of Done (Wave 17):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 17 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 18: Runtime Encapsulation L5-L8 (Wave 18)

**Spec reference:** Spec §16-19
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 18a — Worker Encapsulation (L5) — Implementation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 18a
Implement Worker Encapsulation (L5)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement Worker Encapsulation (L5) algorithm per spec
2. Add to encapsulation pipeline
3. Test
4. Build: cargo build --workspace

Acceptance:
- Worker Encapsulation (L5) implemented
- Tests pass

```

</details>

### 18b — State Encapsulation (L6) — Implementation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 18b
Implement State Encapsulation (L6)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement State Encapsulation (L6) algorithm per spec
2. Add to encapsulation pipeline
3. Test
4. Build: cargo build --workspace

Acceptance:
- State Encapsulation (L6) implemented
- Tests pass

```

</details>

### 18c — Error Encapsulation (L7) — Implementation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 18c
Implement Error Encapsulation (L7)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement Error Encapsulation (L7) algorithm per spec
2. Add to encapsulation pipeline
3. Test
4. Build: cargo build --workspace

Acceptance:
- Error Encapsulation (L7) implemented
- Tests pass

```

</details>

**Definition of Done (Wave 18):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 18 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 19: Runtime Encapsulation L5-L8 (Wave 19)

**Spec reference:** Spec §16-19
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 19a — Worker Encapsulation (L5) — Implementation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 19a
Implement Worker Encapsulation (L5)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement Worker Encapsulation (L5) algorithm per spec
2. Add to encapsulation pipeline
3. Test
4. Build: cargo build --workspace

Acceptance:
- Worker Encapsulation (L5) implemented
- Tests pass

```

</details>

### 19b — State Encapsulation (L6) — Implementation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 19b
Implement State Encapsulation (L6)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement State Encapsulation (L6) algorithm per spec
2. Add to encapsulation pipeline
3. Test
4. Build: cargo build --workspace

Acceptance:
- State Encapsulation (L6) implemented
- Tests pass

```

</details>

**Definition of Done (Wave 19):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 19 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 20: Runtime Encapsulation L5-L8 (Wave 20)

**Spec reference:** Spec §16-19
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 20a — Worker Encapsulation (L5) — Implementation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 20a
Implement Worker Encapsulation (L5)

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement Worker Encapsulation (L5) algorithm per spec
2. Add to encapsulation pipeline
3. Test
4. Build: cargo build --workspace

Acceptance:
- Worker Encapsulation (L5) implemented
- Tests pass

```

</details>

### 20b — Additional task (wave 20)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 20b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 20):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 20 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 21: Runtime Encapsulation L5-L8 (Wave 21)

**Spec reference:** Spec §16-19
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 21a — Encapsulation integration (wave 21)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 21a
Encapsulation integration

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Integrate encapsulation layers
2. Test end-to-end
3. Build

Acceptance:
- Integration works

```

</details>

### 21b — Additional task (wave 21)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 21b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 21):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 21 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 22: Runtime Encapsulation L5-L8 (Wave 22)

**Spec reference:** Spec §16-19
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 22a — Encapsulation integration (wave 22)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 22a
Encapsulation integration

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Integrate encapsulation layers
2. Test end-to-end
3. Build

Acceptance:
- Integration works

```

</details>

### 22b — Additional task (wave 22)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 22b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 22):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 22 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 23: Runtime Encapsulation L5-L8 (Wave 23)

**Spec reference:** Spec §16-19
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 23a — Encapsulation integration (wave 23)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 23a
Encapsulation integration

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Integrate encapsulation layers
2. Test end-to-end
3. Build

Acceptance:
- Integration works

```

</details>

### 23b — Additional task (wave 23)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 23b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 23):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 23 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 24: Runtime Encapsulation L5-L8 (Wave 24)

**Spec reference:** Spec §16-19
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 24a — Encapsulation integration (wave 24)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 24a
Encapsulation integration

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Integrate encapsulation layers
2. Test end-to-end
3. Build

Acceptance:
- Integration works

```

</details>

### 24b — Additional task (wave 24)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 24b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 24):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 24 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 25: FFI Process Isolation (Wave 25)

**Spec reference:** Spec §56-61
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 25a — FFI worker architecture

**Files:** `src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 25a
FFI worker: extern "process" lowering

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add extern "process" to parser
2. Lower to SpawnWorker + ChannelSend + ChannelRecv
3. Auto-generate IPC marshalling for FFI function signatures
4. Build: cargo build --workspace

Acceptance:
- extern "process" compiles
- FFI calls go through IPC

```

</details>

### 25b — Additional task (wave 25)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 25b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 25):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 25 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 26: FFI Process Isolation (Wave 26)

**Spec reference:** Spec §56-61
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 26a — FFI worker architecture

**Files:** `src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 26a
FFI worker: extern "process" lowering

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add extern "process" to parser
2. Lower to SpawnWorker + ChannelSend + ChannelRecv
3. Auto-generate IPC marshalling for FFI function signatures
4. Build: cargo build --workspace

Acceptance:
- extern "process" compiles
- FFI calls go through IPC

```

</details>

### 26b — Additional task (wave 26)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 26b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 26):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 26 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 27: FFI Process Isolation (Wave 27)

**Spec reference:** Spec §56-61
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 27a — FFI worker architecture

**Files:** `src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 27a
FFI worker: extern "process" lowering

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add extern "process" to parser
2. Lower to SpawnWorker + ChannelSend + ChannelRecv
3. Auto-generate IPC marshalling for FFI function signatures
4. Build: cargo build --workspace

Acceptance:
- extern "process" compiles
- FFI calls go through IPC

```

</details>

### 27b — Additional task (wave 27)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 27b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 27):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 27 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 28: FFI Process Isolation (Wave 28)

**Spec reference:** Spec §56-61
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 28a — FFI worker architecture

**Files:** `src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 28a
FFI worker: extern "process" lowering

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add extern "process" to parser
2. Lower to SpawnWorker + ChannelSend + ChannelRecv
3. Auto-generate IPC marshalling for FFI function signatures
4. Build: cargo build --workspace

Acceptance:
- extern "process" compiles
- FFI calls go through IPC

```

</details>

### 28b — Additional task (wave 28)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 28b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 28):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 28 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 29: FFI Process Isolation (Wave 29)

**Spec reference:** Spec §56-61
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 29a — FFI worker architecture

**Files:** `src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 29a
FFI worker: extern "process" lowering

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add extern "process" to parser
2. Lower to SpawnWorker + ChannelSend + ChannelRecv
3. Auto-generate IPC marshalling for FFI function signatures
4. Build: cargo build --workspace

Acceptance:
- extern "process" compiles
- FFI calls go through IPC

```

</details>

### 29b — Additional task (wave 29)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 29b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 29):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 29 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 30: FFI Process Isolation (Wave 30)

**Spec reference:** Spec §56-61
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 30a — FFI worker architecture

**Files:** `src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 30a
FFI worker: extern "process" lowering

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add extern "process" to parser
2. Lower to SpawnWorker + ChannelSend + ChannelRecv
3. Auto-generate IPC marshalling for FFI function signatures
4. Build: cargo build --workspace

Acceptance:
- extern "process" compiles
- FFI calls go through IPC

```

</details>

### 30b — Additional task (wave 30)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 30b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 30):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 30 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 31: FFI Process Isolation (Wave 31)

**Spec reference:** Spec §56-61
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 31a — FFI worker architecture

**Files:** `src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 31a
FFI worker: extern "process" lowering

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add extern "process" to parser
2. Lower to SpawnWorker + ChannelSend + ChannelRecv
3. Auto-generate IPC marshalling for FFI function signatures
4. Build: cargo build --workspace

Acceptance:
- extern "process" compiles
- FFI calls go through IPC

```

</details>

### 31b — Additional task (wave 31)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 31b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 31):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 31 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 32: FFI Process Isolation (Wave 32)

**Spec reference:** Spec §56-61
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 32a — FFI worker architecture

**Files:** `src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 32a
FFI worker: extern "process" lowering

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add extern "process" to parser
2. Lower to SpawnWorker + ChannelSend + ChannelRecv
3. Auto-generate IPC marshalling for FFI function signatures
4. Build: cargo build --workspace

Acceptance:
- extern "process" compiles
- FFI calls go through IPC

```

</details>

### 32b — Additional task (wave 32)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 32b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 32):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 32 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 33: Capability System (Wave 33)

**Spec reference:** Spec §51-55
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 33a — Capability system extension

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 33a
Extend capability system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add capability delegation chain
2. Add capability flow verification
3. Add revocation propagation
4. Build: cargo build --workspace

Acceptance:
- Delegation works
- Flow is verifiable
- Revocation propagates

```

</details>

### 33b — Additional task (wave 33)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 33b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 33):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 33 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 34: Capability System (Wave 34)

**Spec reference:** Spec §51-55
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 34a — Capability system extension

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 34a
Extend capability system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add capability delegation chain
2. Add capability flow verification
3. Add revocation propagation
4. Build: cargo build --workspace

Acceptance:
- Delegation works
- Flow is verifiable
- Revocation propagates

```

</details>

### 34b — Additional task (wave 34)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 34b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 34):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 34 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 35: Capability System (Wave 35)

**Spec reference:** Spec §51-55
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 35a — Capability system extension

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 35a
Extend capability system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add capability delegation chain
2. Add capability flow verification
3. Add revocation propagation
4. Build: cargo build --workspace

Acceptance:
- Delegation works
- Flow is verifiable
- Revocation propagates

```

</details>

### 35b — Additional task (wave 35)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 35b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 35):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 35 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 36: Capability System (Wave 36)

**Spec reference:** Spec §51-55
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 36a — Capability system extension

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 36a
Extend capability system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add capability delegation chain
2. Add capability flow verification
3. Add revocation propagation
4. Build: cargo build --workspace

Acceptance:
- Delegation works
- Flow is verifiable
- Revocation propagates

```

</details>

### 36b — Additional task (wave 36)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 36b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 36):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 36 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 37: Capability System (Wave 37)

**Spec reference:** Spec §51-55
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 37a — Capability system extension

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 37a
Extend capability system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add capability delegation chain
2. Add capability flow verification
3. Add revocation propagation
4. Build: cargo build --workspace

Acceptance:
- Delegation works
- Flow is verifiable
- Revocation propagates

```

</details>

### 37b — Additional task (wave 37)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 37b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 37):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 37 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 38: Capability System (Wave 38)

**Spec reference:** Spec §51-55
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 38a — Capability system extension

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 38a
Extend capability system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add capability delegation chain
2. Add capability flow verification
3. Add revocation propagation
4. Build: cargo build --workspace

Acceptance:
- Delegation works
- Flow is verifiable
- Revocation propagates

```

</details>

### 38b — Additional task (wave 38)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 38b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 38):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 38 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 39: Capability System (Wave 39)

**Spec reference:** Spec §51-55
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 39a — Capability system extension

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 39a
Extend capability system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add capability delegation chain
2. Add capability flow verification
3. Add revocation propagation
4. Build: cargo build --workspace

Acceptance:
- Delegation works
- Flow is verifiable
- Revocation propagates

```

</details>

### 39b — Additional task (wave 39)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 39b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 39):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 39 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 40: Capability System (Wave 40)

**Spec reference:** Spec §51-55
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 40a — Capability system extension

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 40a
Extend capability system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add capability delegation chain
2. Add capability flow verification
3. Add revocation propagation
4. Build: cargo build --workspace

Acceptance:
- Delegation works
- Flow is verifiable
- Revocation propagates

```

</details>

### 40b — Additional task (wave 40)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 40b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 40):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 40 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 41: Kernel/User Split (Wave 41)

**Spec reference:** Spec §62-66
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 41a — Kernel/user split preparation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 41a
Prepare kernel/user split

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define syscall-as-IPC protocol
2. Map each syscall to an IPC message type
3. Build: cargo build --workspace

Acceptance:
- Syscall-to-IPC mapping defined

```

</details>

### 41b — Additional task (wave 41)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 41b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 41):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 41 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 42: Kernel/User Split (Wave 42)

**Spec reference:** Spec §62-66
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 42a — Kernel/user split preparation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 42a
Prepare kernel/user split

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define syscall-as-IPC protocol
2. Map each syscall to an IPC message type
3. Build: cargo build --workspace

Acceptance:
- Syscall-to-IPC mapping defined

```

</details>

### 42b — Additional task (wave 42)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 42b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 42):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 42 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 43: Kernel/User Split (Wave 43)

**Spec reference:** Spec §62-66
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 43a — Kernel/user split preparation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 43a
Prepare kernel/user split

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define syscall-as-IPC protocol
2. Map each syscall to an IPC message type
3. Build: cargo build --workspace

Acceptance:
- Syscall-to-IPC mapping defined

```

</details>

### 43b — Additional task (wave 43)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 43b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 43):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 43 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 44: Kernel/User Split (Wave 44)

**Spec reference:** Spec §62-66
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 44a — Kernel/user split preparation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 44a
Prepare kernel/user split

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define syscall-as-IPC protocol
2. Map each syscall to an IPC message type
3. Build: cargo build --workspace

Acceptance:
- Syscall-to-IPC mapping defined

```

</details>

### 44b — Additional task (wave 44)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 44b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 44):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 44 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 45: Kernel/User Split (Wave 45)

**Spec reference:** Spec §62-66
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 45a — Kernel/user split preparation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 45a
Prepare kernel/user split

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define syscall-as-IPC protocol
2. Map each syscall to an IPC message type
3. Build: cargo build --workspace

Acceptance:
- Syscall-to-IPC mapping defined

```

</details>

### 45b — Additional task (wave 45)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 45b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 45):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 45 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 46: Kernel/User Split (Wave 46)

**Spec reference:** Spec §62-66
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 46a — Kernel/user split preparation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 46a
Prepare kernel/user split

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define syscall-as-IPC protocol
2. Map each syscall to an IPC message type
3. Build: cargo build --workspace

Acceptance:
- Syscall-to-IPC mapping defined

```

</details>

### 46b — Additional task (wave 46)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 46b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 46):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 46 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 47: Kernel/User Split (Wave 47)

**Spec reference:** Spec §62-66
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 47a — Kernel/user split preparation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 47a
Prepare kernel/user split

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define syscall-as-IPC protocol
2. Map each syscall to an IPC message type
3. Build: cargo build --workspace

Acceptance:
- Syscall-to-IPC mapping defined

```

</details>

### 47b — Additional task (wave 47)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 47b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 47):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 47 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 48: Kernel/User Split (Wave 48)

**Spec reference:** Spec §62-66
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 48a — Kernel/user split preparation

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 48a
Prepare kernel/user split

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Define syscall-as-IPC protocol
2. Map each syscall to an IPC message type
3. Build: cargo build --workspace

Acceptance:
- Syscall-to-IPC mapping defined

```

</details>

### 48b — Additional task (wave 48)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 48b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 48):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 48 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 49: Driver Isolation (Wave 49)

**Spec reference:** Spec §67-71
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 49a — Driver isolation framework

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 49a
Driver isolation: MMIO capabilities

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add MMIO capability type
2. Add IRQ channel protocol
3. Add DMA buffer management
4. Build: cargo build --workspace

Acceptance:
- MMIO capabilities exist
- IRQ channels defined

```

</details>

### 49b — Additional task (wave 49)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 49b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 49):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 49 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 50: Driver Isolation (Wave 50)

**Spec reference:** Spec §67-71
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 50a — Driver isolation framework

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 50a
Driver isolation: MMIO capabilities

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add MMIO capability type
2. Add IRQ channel protocol
3. Add DMA buffer management
4. Build: cargo build --workspace

Acceptance:
- MMIO capabilities exist
- IRQ channels defined

```

</details>

### 50b — Additional task (wave 50)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 50b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 50):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 50 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 51: Driver Isolation (Wave 51)

**Spec reference:** Spec §67-71
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 51a — Driver isolation framework

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 51a
Driver isolation: MMIO capabilities

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add MMIO capability type
2. Add IRQ channel protocol
3. Add DMA buffer management
4. Build: cargo build --workspace

Acceptance:
- MMIO capabilities exist
- IRQ channels defined

```

</details>

### 51b — Additional task (wave 51)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 51b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 51):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 51 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 52: Driver Isolation (Wave 52)

**Spec reference:** Spec §67-71
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 52a — Driver isolation framework

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 52a
Driver isolation: MMIO capabilities

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add MMIO capability type
2. Add IRQ channel protocol
3. Add DMA buffer management
4. Build: cargo build --workspace

Acceptance:
- MMIO capabilities exist
- IRQ channels defined

```

</details>

### 52b — Additional task (wave 52)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 52b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 52):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 52 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 53: Driver Isolation (Wave 53)

**Spec reference:** Spec §67-71
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 53a — Driver isolation framework

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 53a
Driver isolation: MMIO capabilities

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add MMIO capability type
2. Add IRQ channel protocol
3. Add DMA buffer management
4. Build: cargo build --workspace

Acceptance:
- MMIO capabilities exist
- IRQ channels defined

```

</details>

### 53b — Additional task (wave 53)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 53b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 53):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 53 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 54: Driver Isolation (Wave 54)

**Spec reference:** Spec §67-71
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 54a — Driver isolation framework

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 54a
Driver isolation: MMIO capabilities

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add MMIO capability type
2. Add IRQ channel protocol
3. Add DMA buffer management
4. Build: cargo build --workspace

Acceptance:
- MMIO capabilities exist
- IRQ channels defined

```

</details>

### 54b — Additional task (wave 54)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 54b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 54):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 54 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 55: Driver Isolation (Wave 55)

**Spec reference:** Spec §67-71
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 55a — Driver isolation framework

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 55a
Driver isolation: MMIO capabilities

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add MMIO capability type
2. Add IRQ channel protocol
3. Add DMA buffer management
4. Build: cargo build --workspace

Acceptance:
- MMIO capabilities exist
- IRQ channels defined

```

</details>

### 55b — Additional task (wave 55)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 55b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 55):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 55 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 56: Driver Isolation (Wave 56)

**Spec reference:** Spec §67-71
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 56a — Driver isolation framework

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 56a
Driver isolation: MMIO capabilities

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add MMIO capability type
2. Add IRQ channel protocol
3. Add DMA buffer management
4. Build: cargo build --workspace

Acceptance:
- MMIO capabilities exist
- IRQ channels defined

```

</details>

### 56b — Additional task (wave 56)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 56b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 56):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 56 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 57: Sandboxing (Wave 57)

**Spec reference:** Spec §72-76
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 57a — Sandbox architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 57a
Implement sandbox: zero-capability workers

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add spawn_sandbox() builtin — worker with zero capabilities
2. Add seccomp filter generation
3. Build: cargo build --workspace

Acceptance:
- spawn_sandbox creates isolated worker
- seccomp filter applied

```

</details>

### 57b — Additional task (wave 57)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 57b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 57):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 57 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 58: Sandboxing (Wave 58)

**Spec reference:** Spec §72-76
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 58a — Sandbox architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 58a
Implement sandbox: zero-capability workers

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add spawn_sandbox() builtin — worker with zero capabilities
2. Add seccomp filter generation
3. Build: cargo build --workspace

Acceptance:
- spawn_sandbox creates isolated worker
- seccomp filter applied

```

</details>

### 58b — Additional task (wave 58)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 58b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 58):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 58 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 59: Sandboxing (Wave 59)

**Spec reference:** Spec §72-76
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 59a — Sandbox architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 59a
Implement sandbox: zero-capability workers

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add spawn_sandbox() builtin — worker with zero capabilities
2. Add seccomp filter generation
3. Build: cargo build --workspace

Acceptance:
- spawn_sandbox creates isolated worker
- seccomp filter applied

```

</details>

### 59b — Additional task (wave 59)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 59b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 59):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 59 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 60: Sandboxing (Wave 60)

**Spec reference:** Spec §72-76
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 60a — Sandbox architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 60a
Implement sandbox: zero-capability workers

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add spawn_sandbox() builtin — worker with zero capabilities
2. Add seccomp filter generation
3. Build: cargo build --workspace

Acceptance:
- spawn_sandbox creates isolated worker
- seccomp filter applied

```

</details>

### 60b — Additional task (wave 60)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 60b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 60):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 60 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 61: Sandboxing (Wave 61)

**Spec reference:** Spec §72-76
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 61a — Sandbox architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 61a
Implement sandbox: zero-capability workers

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add spawn_sandbox() builtin — worker with zero capabilities
2. Add seccomp filter generation
3. Build: cargo build --workspace

Acceptance:
- spawn_sandbox creates isolated worker
- seccomp filter applied

```

</details>

### 61b — Additional task (wave 61)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 61b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 61):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 61 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 62: Sandboxing (Wave 62)

**Spec reference:** Spec §72-76
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 62a — Sandbox architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 62a
Implement sandbox: zero-capability workers

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add spawn_sandbox() builtin — worker with zero capabilities
2. Add seccomp filter generation
3. Build: cargo build --workspace

Acceptance:
- spawn_sandbox creates isolated worker
- seccomp filter applied

```

</details>

### 62b — Additional task (wave 62)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 62b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 62):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 62 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 63: Sandboxing (Wave 63)

**Spec reference:** Spec §72-76
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 63a — Sandbox architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 63a
Implement sandbox: zero-capability workers

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add spawn_sandbox() builtin — worker with zero capabilities
2. Add seccomp filter generation
3. Build: cargo build --workspace

Acceptance:
- spawn_sandbox creates isolated worker
- seccomp filter applied

```

</details>

### 63b — Additional task (wave 63)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 63b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 63):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 63 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 64: Sandboxing (Wave 64)

**Spec reference:** Spec §72-76
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 64a — Sandbox architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 64a
Implement sandbox: zero-capability workers

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add spawn_sandbox() builtin — worker with zero capabilities
2. Add seccomp filter generation
3. Build: cargo build --workspace

Acceptance:
- spawn_sandbox creates isolated worker
- seccomp filter applied

```

</details>

### 64b — Additional task (wave 64)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 64b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 64):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 64 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 65: Fault Tolerance (Wave 65)

**Spec reference:** Spec §77-82
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 65a — Supervisor architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 65a
Implement supervisor: crash detection + restart

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Supervisor struct
2. Crash detection via SIGCHLD
3. Worker restart protocol
4. State checkpointing (copy-on-write)
5. Circuit breaker pattern
6. Build: cargo build --workspace

Acceptance:
- Supervisor detects crashes
- Workers auto-restart
- State is checkpointed

```

</details>

### 65b — Additional task (wave 65)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 65b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 65):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 65 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 66: Fault Tolerance (Wave 66)

**Spec reference:** Spec §77-82
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 66a — Supervisor architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 66a
Implement supervisor: crash detection + restart

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Supervisor struct
2. Crash detection via SIGCHLD
3. Worker restart protocol
4. State checkpointing (copy-on-write)
5. Circuit breaker pattern
6. Build: cargo build --workspace

Acceptance:
- Supervisor detects crashes
- Workers auto-restart
- State is checkpointed

```

</details>

### 66b — Additional task (wave 66)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 66b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 66):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 66 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 67: Fault Tolerance (Wave 67)

**Spec reference:** Spec §77-82
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 67a — Supervisor architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 67a
Implement supervisor: crash detection + restart

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Supervisor struct
2. Crash detection via SIGCHLD
3. Worker restart protocol
4. State checkpointing (copy-on-write)
5. Circuit breaker pattern
6. Build: cargo build --workspace

Acceptance:
- Supervisor detects crashes
- Workers auto-restart
- State is checkpointed

```

</details>

### 67b — Additional task (wave 67)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 67b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 67):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 67 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 68: Fault Tolerance (Wave 68)

**Spec reference:** Spec §77-82
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 68a — Supervisor architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 68a
Implement supervisor: crash detection + restart

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Supervisor struct
2. Crash detection via SIGCHLD
3. Worker restart protocol
4. State checkpointing (copy-on-write)
5. Circuit breaker pattern
6. Build: cargo build --workspace

Acceptance:
- Supervisor detects crashes
- Workers auto-restart
- State is checkpointed

```

</details>

### 68b — Additional task (wave 68)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 68b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 68):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 68 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 69: Fault Tolerance (Wave 69)

**Spec reference:** Spec §77-82
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 69a — Supervisor architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 69a
Implement supervisor: crash detection + restart

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Supervisor struct
2. Crash detection via SIGCHLD
3. Worker restart protocol
4. State checkpointing (copy-on-write)
5. Circuit breaker pattern
6. Build: cargo build --workspace

Acceptance:
- Supervisor detects crashes
- Workers auto-restart
- State is checkpointed

```

</details>

### 69b — Additional task (wave 69)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 69b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 69):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 69 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 70: Fault Tolerance (Wave 70)

**Spec reference:** Spec §77-82
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 70a — Supervisor architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 70a
Implement supervisor: crash detection + restart

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Supervisor struct
2. Crash detection via SIGCHLD
3. Worker restart protocol
4. State checkpointing (copy-on-write)
5. Circuit breaker pattern
6. Build: cargo build --workspace

Acceptance:
- Supervisor detects crashes
- Workers auto-restart
- State is checkpointed

```

</details>

### 70b — Additional task (wave 70)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 70b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 70):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 70 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 71: Fault Tolerance (Wave 71)

**Spec reference:** Spec §77-82
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 71a — Supervisor architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 71a
Implement supervisor: crash detection + restart

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Supervisor struct
2. Crash detection via SIGCHLD
3. Worker restart protocol
4. State checkpointing (copy-on-write)
5. Circuit breaker pattern
6. Build: cargo build --workspace

Acceptance:
- Supervisor detects crashes
- Workers auto-restart
- State is checkpointed

```

</details>

### 71b — Additional task (wave 71)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 71b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 71):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 71 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 72: Fault Tolerance (Wave 72)

**Spec reference:** Spec §77-82
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 72a — Supervisor architecture

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 72a
Implement supervisor: crash detection + restart

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Supervisor struct
2. Crash detection via SIGCHLD
3. Worker restart protocol
4. State checkpointing (copy-on-write)
5. Circuit breaker pattern
6. Build: cargo build --workspace

Acceptance:
- Supervisor detects crashes
- Workers auto-restart
- State is checkpointed

```

</details>

### 72b — Additional task (wave 72)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 72b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 72):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 72 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 73: Hot Reloading (Wave 73)

**Spec reference:** Spec §83-86
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 73a — Hot-swap protocol

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 73a
Implement hot-swap: replace worker without losing state

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Hot-swap: spawn new worker, transfer state, kill old
2. State transfer via IPC
3. Version management
4. Rollback protocol
5. Build: cargo build --workspace

Acceptance:
- Hot-swap works
- State transfers correctly
- Rollback works

```

</details>

### 73b — Additional task (wave 73)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 73b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 73):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 73 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 74: Hot Reloading (Wave 74)

**Spec reference:** Spec §83-86
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 74a — Hot-swap protocol

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 74a
Implement hot-swap: replace worker without losing state

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Hot-swap: spawn new worker, transfer state, kill old
2. State transfer via IPC
3. Version management
4. Rollback protocol
5. Build: cargo build --workspace

Acceptance:
- Hot-swap works
- State transfers correctly
- Rollback works

```

</details>

### 74b — Additional task (wave 74)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 74b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 74):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 74 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 75: Hot Reloading (Wave 75)

**Spec reference:** Spec §83-86
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 75a — Hot-swap protocol

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 75a
Implement hot-swap: replace worker without losing state

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Hot-swap: spawn new worker, transfer state, kill old
2. State transfer via IPC
3. Version management
4. Rollback protocol
5. Build: cargo build --workspace

Acceptance:
- Hot-swap works
- State transfers correctly
- Rollback works

```

</details>

### 75b — Additional task (wave 75)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 75b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 75):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 75 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 76: Hot Reloading (Wave 76)

**Spec reference:** Spec §83-86
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 76a — Hot-swap protocol

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 76a
Implement hot-swap: replace worker without losing state

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Hot-swap: spawn new worker, transfer state, kill old
2. State transfer via IPC
3. Version management
4. Rollback protocol
5. Build: cargo build --workspace

Acceptance:
- Hot-swap works
- State transfers correctly
- Rollback works

```

</details>

### 76b — Additional task (wave 76)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 76b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 76):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 76 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 77: Hot Reloading (Wave 77)

**Spec reference:** Spec §83-86
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 77a — Hot-swap protocol

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 77a
Implement hot-swap: replace worker without losing state

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Hot-swap: spawn new worker, transfer state, kill old
2. State transfer via IPC
3. Version management
4. Rollback protocol
5. Build: cargo build --workspace

Acceptance:
- Hot-swap works
- State transfers correctly
- Rollback works

```

</details>

### 77b — Additional task (wave 77)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 77b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 77):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 77 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 78: Hot Reloading (Wave 78)

**Spec reference:** Spec §83-86
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 78a — Hot-swap protocol

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 78a
Implement hot-swap: replace worker without losing state

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Hot-swap: spawn new worker, transfer state, kill old
2. State transfer via IPC
3. Version management
4. Rollback protocol
5. Build: cargo build --workspace

Acceptance:
- Hot-swap works
- State transfers correctly
- Rollback works

```

</details>

### 78b — Additional task (wave 78)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 78b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 78):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 78 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 79: Hot Reloading (Wave 79)

**Spec reference:** Spec §83-86
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 79a — Hot-swap protocol

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 79a
Implement hot-swap: replace worker without losing state

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Hot-swap: spawn new worker, transfer state, kill old
2. State transfer via IPC
3. Version management
4. Rollback protocol
5. Build: cargo build --workspace

Acceptance:
- Hot-swap works
- State transfers correctly
- Rollback works

```

</details>

### 79b — Additional task (wave 79)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 79b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 79):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 79 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 80: Hot Reloading (Wave 80)

**Spec reference:** Spec §83-86
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 80a — Hot-swap protocol

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 80a
Implement hot-swap: replace worker without losing state

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Hot-swap: spawn new worker, transfer state, kill old
2. State transfer via IPC
3. Version management
4. Rollback protocol
5. Build: cargo build --workspace

Acceptance:
- Hot-swap works
- State transfers correctly
- Rollback works

```

</details>

### 80b — Additional task (wave 80)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 80b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 80):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 80 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 81: Distributed Channels (Wave 81)

**Spec reference:** Spec §87-91
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 81a — Location-transparent channels

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 81a
Implement location-transparent channels + Noise protocol

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Same Channel<T> API for local and network IPC
2. TCP transport for remote channels
3. Noise Protocol Framework handshake (XX pattern)
4. Worker discovery
5. Failure detection (heartbeat)
6. Build: cargo build --workspace

Acceptance:
- Remote channels work over TCP
- Noise handshake completes
- Failure detected via heartbeat

```

</details>

### 81b — Additional task (wave 81)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 81b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 81):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 81 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 82: Distributed Channels (Wave 82)

**Spec reference:** Spec §87-91
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 82a — Location-transparent channels

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 82a
Implement location-transparent channels + Noise protocol

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Same Channel<T> API for local and network IPC
2. TCP transport for remote channels
3. Noise Protocol Framework handshake (XX pattern)
4. Worker discovery
5. Failure detection (heartbeat)
6. Build: cargo build --workspace

Acceptance:
- Remote channels work over TCP
- Noise handshake completes
- Failure detected via heartbeat

```

</details>

### 82b — Additional task (wave 82)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 82b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 82):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 82 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 83: Distributed Channels (Wave 83)

**Spec reference:** Spec §87-91
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 83a — Location-transparent channels

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 83a
Implement location-transparent channels + Noise protocol

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Same Channel<T> API for local and network IPC
2. TCP transport for remote channels
3. Noise Protocol Framework handshake (XX pattern)
4. Worker discovery
5. Failure detection (heartbeat)
6. Build: cargo build --workspace

Acceptance:
- Remote channels work over TCP
- Noise handshake completes
- Failure detected via heartbeat

```

</details>

### 83b — Additional task (wave 83)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 83b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 83):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 83 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 84: Distributed Channels (Wave 84)

**Spec reference:** Spec §87-91
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 84a — Location-transparent channels

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 84a
Implement location-transparent channels + Noise protocol

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Same Channel<T> API for local and network IPC
2. TCP transport for remote channels
3. Noise Protocol Framework handshake (XX pattern)
4. Worker discovery
5. Failure detection (heartbeat)
6. Build: cargo build --workspace

Acceptance:
- Remote channels work over TCP
- Noise handshake completes
- Failure detected via heartbeat

```

</details>

### 84b — Additional task (wave 84)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 84b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 84):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 84 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 85: Distributed Channels (Wave 85)

**Spec reference:** Spec §87-91
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 85a — Location-transparent channels

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 85a
Implement location-transparent channels + Noise protocol

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Same Channel<T> API for local and network IPC
2. TCP transport for remote channels
3. Noise Protocol Framework handshake (XX pattern)
4. Worker discovery
5. Failure detection (heartbeat)
6. Build: cargo build --workspace

Acceptance:
- Remote channels work over TCP
- Noise handshake completes
- Failure detected via heartbeat

```

</details>

### 85b — Additional task (wave 85)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 85b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 85):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 85 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 86: Distributed Channels (Wave 86)

**Spec reference:** Spec §87-91
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 86a — Location-transparent channels

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 86a
Implement location-transparent channels + Noise protocol

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Same Channel<T> API for local and network IPC
2. TCP transport for remote channels
3. Noise Protocol Framework handshake (XX pattern)
4. Worker discovery
5. Failure detection (heartbeat)
6. Build: cargo build --workspace

Acceptance:
- Remote channels work over TCP
- Noise handshake completes
- Failure detected via heartbeat

```

</details>

### 86b — Additional task (wave 86)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 86b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 86):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 86 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 87: Distributed Channels (Wave 87)

**Spec reference:** Spec §87-91
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 87a — Location-transparent channels

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 87a
Implement location-transparent channels + Noise protocol

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Same Channel<T> API for local and network IPC
2. TCP transport for remote channels
3. Noise Protocol Framework handshake (XX pattern)
4. Worker discovery
5. Failure detection (heartbeat)
6. Build: cargo build --workspace

Acceptance:
- Remote channels work over TCP
- Noise handshake completes
- Failure detected via heartbeat

```

</details>

### 87b — Additional task (wave 87)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 87b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 87):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 87 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 88: Distributed Channels (Wave 88)

**Spec reference:** Spec §87-91
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 88a — Location-transparent channels

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 88a
Implement location-transparent channels + Noise protocol

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Same Channel<T> API for local and network IPC
2. TCP transport for remote channels
3. Noise Protocol Framework handshake (XX pattern)
4. Worker discovery
5. Failure detection (heartbeat)
6. Build: cargo build --workspace

Acceptance:
- Remote channels work over TCP
- Noise handshake completes
- Failure detected via heartbeat

```

</details>

### 88b — Additional task (wave 88)

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 88b
Additional implementation task

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Implement per spec
2. Test
3. Build: cargo build --workspace

Acceptance:
- Task complete

```

</details>

**Definition of Done (Wave 88):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 88 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 89: Compile-Time Encapsulation (Wave 89)

**Spec reference:** Spec §20-28, §129-132
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 89a — Session Types — Type System

**Files:** `src/codegen/src/scg_to_ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 89a
Implement Session Types in type system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/scg_to_ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Session Types type constructs per Spec §21
2. Add compile-time verification
3. Add to PMT verification pipeline
4. Build: cargo build --workspace

Acceptance:
- Session Types exists in type system
- Compile-time verification works

```

</details>

### 89b — Session Types — Tests

**Files:** `tests/gold_standard/ (new files)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 89b
Create tests for Session Types

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ (new files)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create 2-4 test programs exercising Session Types
2. Each with // Expected exit code
3. Run on x86_64
4. Build: cargo build --workspace

Acceptance:
- Tests for Session Types exist and pass

```

</details>

**Definition of Done (Wave 89):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 89 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 90: Compile-Time Encapsulation (Wave 90)

**Spec reference:** Spec §20-28, §129-132
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 90a — Information-Flow Types — Type System

**Files:** `src/codegen/src/ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 90a
Implement Information-Flow Types in type system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Information-Flow Types type constructs per Spec §22
2. Add compile-time verification
3. Add to PMT verification pipeline
4. Build: cargo build --workspace

Acceptance:
- Information-Flow Types exists in type system
- Compile-time verification works

```

</details>

### 90b — Information-Flow Types — Tests

**Files:** `tests/gold_standard/ (new files)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 90b
Create tests for Information-Flow Types

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ (new files)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create 2-4 test programs exercising Information-Flow Types
2. Each with // Expected exit code
3. Run on x86_64
4. Build: cargo build --workspace

Acceptance:
- Tests for Information-Flow Types exist and pass

```

</details>

**Definition of Done (Wave 90):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 90 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 91: Compile-Time Encapsulation (Wave 91)

**Spec reference:** Spec §20-28, §129-132
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 91a — Refinement Types — Type System

**Files:** `src/parser/src/ast.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 91a
Implement Refinement Types in type system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/parser/src/ast.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Refinement Types type constructs per Spec §23
2. Add compile-time verification
3. Add to PMT verification pipeline
4. Build: cargo build --workspace

Acceptance:
- Refinement Types exists in type system
- Compile-time verification works

```

</details>

### 91b — Refinement Types — Tests

**Files:** `tests/gold_standard/ (new files)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 91b
Create tests for Refinement Types

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ (new files)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create 2-4 test programs exercising Refinement Types
2. Each with // Expected exit code
3. Run on x86_64
4. Build: cargo build --workspace

Acceptance:
- Tests for Refinement Types exist and pass

```

</details>

**Definition of Done (Wave 91):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 91 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 92: Compile-Time Encapsulation (Wave 92)

**Spec reference:** Spec §20-28, §129-132
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 92a — Linear Capability Types — Type System

**Files:** `src/codegen/src/ir.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 92a
Implement Linear Capability Types in type system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ir.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Linear Capability Types type constructs per Spec §24
2. Add compile-time verification
3. Add to PMT verification pipeline
4. Build: cargo build --workspace

Acceptance:
- Linear Capability Types exists in type system
- Compile-time verification works

```

</details>

### 92b — Linear Capability Types — Tests

**Files:** `tests/gold_standard/ (new files)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 92b
Create tests for Linear Capability Types

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ (new files)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create 2-4 test programs exercising Linear Capability Types
2. Each with // Expected exit code
3. Run on x86_64
4. Build: cargo build --workspace

Acceptance:
- Tests for Linear Capability Types exist and pass

```

</details>

**Definition of Done (Wave 92):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 92 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 93: Compile-Time Encapsulation (Wave 93)

**Spec reference:** Spec §20-28, §129-132
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 93a — Homomorphic Encapsulation — Type System

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 93a
Implement Homomorphic Encapsulation in type system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Homomorphic Encapsulation type constructs per Spec §25
2. Add compile-time verification
3. Add to PMT verification pipeline
4. Build: cargo build --workspace

Acceptance:
- Homomorphic Encapsulation exists in type system
- Compile-time verification works

```

</details>

### 93b — Homomorphic Encapsulation — Tests

**Files:** `tests/gold_standard/ (new files)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 93b
Create tests for Homomorphic Encapsulation

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ (new files)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create 2-4 test programs exercising Homomorphic Encapsulation
2. Each with // Expected exit code
3. Run on x86_64
4. Build: cargo build --workspace

Acceptance:
- Tests for Homomorphic Encapsulation exist and pass

```

</details>

**Definition of Done (Wave 93):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 93 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 94: Compile-Time Encapsulation (Wave 94)

**Spec reference:** Spec §20-28, §129-132
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 94a — zk-STARK Attestation — Type System

**Files:** `src/codegen/src/capability.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 94a
Implement zk-STARK Attestation in type system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/capability.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add zk-STARK Attestation type constructs per Spec §26
2. Add compile-time verification
3. Add to PMT verification pipeline
4. Build: cargo build --workspace

Acceptance:
- zk-STARK Attestation exists in type system
- Compile-time verification works

```

</details>

### 94b — zk-STARK Attestation — Tests

**Files:** `tests/gold_standard/ (new files)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 94b
Create tests for zk-STARK Attestation

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ (new files)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create 2-4 test programs exercising zk-STARK Attestation
2. Each with // Expected exit code
3. Run on x86_64
4. Build: cargo build --workspace

Acceptance:
- Tests for zk-STARK Attestation exist and pass

```

</details>

**Definition of Done (Wave 94):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 94 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 95: Compile-Time Encapsulation (Wave 95)

**Spec reference:** Spec §20-28, §129-132
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 95a — CSL-Perm Permissions — Type System

**Files:** `src/ive/src/borrow_region.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 95a
Implement CSL-Perm Permissions in type system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/ive/src/borrow_region.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add CSL-Perm Permissions type constructs per Spec §27
2. Add compile-time verification
3. Add to PMT verification pipeline
4. Build: cargo build --workspace

Acceptance:
- CSL-Perm Permissions exists in type system
- Compile-time verification works

```

</details>

### 95b — CSL-Perm Permissions — Tests

**Files:** `tests/gold_standard/ (new files)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 95b
Create tests for CSL-Perm Permissions

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ (new files)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create 2-4 test programs exercising CSL-Perm Permissions
2. Each with // Expected exit code
3. Run on x86_64
4. Build: cargo build --workspace

Acceptance:
- Tests for CSL-Perm Permissions exist and pass

```

</details>

**Definition of Done (Wave 95):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 95 subtasks complete
- [ ] No regressions in existing tests

---


## Wave 96: Compile-Time Encapsulation (Wave 96)

**Spec reference:** Spec §20-28, §129-132
**Max parallel subagents:** 4
**Scope:** Each subtask touches a distinct file/domain. No two subtasks in the same wave edit the same file.

### 96a — Noise Protocol Channels — Type System

**Files:** `src/codegen/src/ipc.rs`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 96a
Implement Noise Protocol Channels in type system

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: src/codegen/src/ipc.rs
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Add Noise Protocol Channels type constructs per Spec §28
2. Add compile-time verification
3. Add to PMT verification pipeline
4. Build: cargo build --workspace

Acceptance:
- Noise Protocol Channels exists in type system
- Compile-time verification works

```

</details>

### 96b — Noise Protocol Channels — Tests

**Files:** `tests/gold_standard/ (new files)`

<details>
<summary>Subagent prompt (click to expand)</summary>

```
Task ID: 96b
Create tests for Noise Protocol Channels

Repo: /tmp/vuma. Rust: export PATH="$HOME/.cargo/bin:$PATH". QEMU: $HOME/.local/bin/*-static.
Edit ONLY: tests/gold_standard/ (new files)
Read /home/z/my-project/worklog.md first. Append your record when done. Do NOT run git.

Steps:
1. Create 2-4 test programs exercising Noise Protocol Channels
2. Each with // Expected exit code
3. Run on x86_64
4. Build: cargo build --workspace

Acceptance:
- Tests for Noise Protocol Channels exist and pass

```

</details>

**Definition of Done (Wave 96):**
- [ ] cargo build --workspace succeeds
- [ ] Wave 96 subtasks complete
- [ ] No regressions in existing tests

---


## Final Wave (96): Full System Integration

**Definition of Done (entire plan):**
- [ ] All 96 waves completed and DoD approved
- [ ] cargo build --workspace succeeds with zero errors
- [ ] All gold-standard tests pass on x86_64, aarch64, riscv64, arm32
- [ ] IPC channel send/recv works across processes on all 4 backends
- [ ] FFI process isolation: extern "process" spawns isolated worker
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

*This TASKS.md is generated from docs/VUMA_PROCESS_ISOLATION_SPEC.md v2.0*

