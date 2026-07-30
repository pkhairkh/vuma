# Wave 4-a Audit — Two-pipe channel handle layout (caveat §2.3)

- **Task ID:** 4-a-audit
- **Agent:** 4-a-audit (sub-agent, wave 4)
- **Wave:** 4 (depends on waves 0 / 1 / 2 / 3)
- **Caveat addressed:** §2.3 — *"Two-pipe channel handles are 16 bytes / 4 fds"*
- **Files in scope (READ-ONLY audit + new test file):**
  - Read-only source audit: `src/codegen/src/ipc_lowering.rs` (`expand_channel_open` ~L1112-1322, `expand_channel_close` ~L1324-1388, `scan_needs` ~L629-675, `alloc_state_slots` ~L677-788)
  - New test file: `tests/ipc_handle_layout_test.rs`
- **Files OUT of scope:** any source file under `vuma/src/` (NOT edited — verified by `git status`).
- **DoD:**
  1. A test exists that asserts `size_of::<ChannelHandle>() == 16` (or equivalent).
  2. The test passes.
  3. This summary markdown exists.

## 1. Handle layout finding

The channel handle is **not a Rust `struct`** in `vuma-codegen`. It is a
runtime-allocated IR buffer created by `expand_channel_open` and consumed by
`channel_send` / `channel_recv` / `channel_close`. The layout is encoded
directly in the emitted IR:

| Offset | Field     | Type | Source pipe                  |
|--------|-----------|------|------------------------------|
| 0      | read_fd1  | I32  | pipe 1 (parent→child) read   |
| 4      | write_fd1 | I32  | pipe 1 (parent→child) write  |
| 8      | read_fd2  | I32  | pipe 2 (child→parent) read   |
| 12     | write_fd2 | I32  | pipe 2 (child→parent) write  |

**Total: 4 × 4 bytes = 16 bytes holding 4 file descriptors.** This matches
caveat §2.3 exactly.

The handle is created by:
1. Two `pipe2()` syscalls (asm-generic `nr: 59`), each producing a
   `{read_fd, write_fd}` pair stored into an 8-byte scratch buffer.
2. One `IRInstr::Alloc { size: 16 }` for the handle buffer.
3. Four `IRInstr::Store { addr: <handle_ptr>, offset: o, ty: I32 }` at
   offsets `o ∈ {0, 4, 8, 12}` — one per fd.

The handle pointer is also registered in the per-function channel registry
(`alloc_state_slots`, `needs.channel_registry = true`) so
`expand_spawn_worker` can swap `[0↔8]` and `[4↔12]` on every registered
handle in the child after `clone()`, giving the child the correct
parent→child read end and child→parent write end.

`channel_close` reads the same 4 fds back via 4 `IRInstr::Load` at offsets
`{0, 4, 8, 12}` and calls `close()` (`nr: 57`) on each — proving the
layout is consumed consistently with how it was created.

The previous single-pipe design (one pipe, parent reads its own writes →
deadlock) and its `nanosleep`-based send/recv race workaround are fully
removed. `expand_channel_open` documents the rationale: with two pipes, the
parent writes to pipe 1 (consumed by the child) and reads from pipe 2
(filled by the child), so send and recv touch DIFFERENT pipes and cannot
self-deadlock.

## 2. Test design (`tests/ipc_handle_layout_test.rs`)

Four tests, combining a compile-time mirror-struct assertion (the "or
equivalent" of `size_of::<ChannelHandle>() == 16`) with runtime IR-layout
verification proving the codegen actually emits the documented layout:

### 2.1 `handle_size_is_16_bytes_compile_time`
Defines a `#[repr(C)] struct ChannelHandle { read_fd1: i32, write_fd1: i32, read_fd2: i32, write_fd2: i32 }` mirroring the IR layout. Asserts:
- `size_of::<ChannelHandle>() == 16`
- `size_of::<i32>() == 4` (each field is exactly one fd)
- Field offsets within the `#[repr(C)]` struct are `{0, 4, 8, 12}` — matching the IR Store/Load offsets.

### 2.2 `channel_open_emits_16_byte_handle_with_4_i32_fds`
Builds a minimal `IRFunction` with a single `channel_open()` Call, runs
`lower_ipc_builtins(func, BackendKind::X86_64)`, flattens the post-lowering
IR, and asserts:
- Exactly one `Alloc { size: 16 }` (the handle buffer — the two `pipe2`
  scratch buffers are size 8, the channel registry is size 84).
- Exactly four `Store { addr: <handle_ptr>, offset: o, ty: I32 }` with
  `o` sorted equal to `vec![0, 4, 8, 12]` (one each, no dupes).
- Exactly two `Syscall { nr: 59, .. }` (`pipe2` — one per pipe).

### 2.3 `channel_close_reads_4_fds_at_same_offsets`
Builds `fn(channel_close(handle))`, lowers it, and asserts:
- Four `Load { addr: handle, offset: o, ty: I32 }` at `o ∈ {0, 4, 8, 12}`
  (close reads the same 4 fds open wrote).
- Four `Syscall { nr: 57, .. }` (`close` — one per fd).

### 2.4 `handle_layout_is_backend_independent_across_non_wasm32_backends`
Repeats the layout audit for `X86_64`, `AArch64`, `RiscV64`, `Arm32` —
confirming the 16-byte / 4-fd layout is an IR-level contract emitted
identically by every non-wasm32 backend. (Wasm32 lowers `channel_open`
natively to in-memory ring buffers and never reaches the `pipe2`-based
expansion — this short-circuit is documented in
`is_wasm32_native_channel_builtin` and `split_block_at_first_ipc`.)

## 3. Test execution

```
$ cargo test --release --test ipc_handle_layout_test 2>&1 | tee scripts/logs/wave4_handle_test.log
   Finished `release` profile [optimized] target(s) in 0.01s
    Running tests/ipc_handle_layout_test.rs (target/release/deps/ipc_handle_layout_test-3a4d5544b2988d8a)

running 4 tests
test channel_open_emits_16_byte_handle_with_4_i32_fds ... ok
test channel_close_reads_4_fds_at_same_offsets ... ok
test handle_layout_is_backend_independent_across_non_wasm32_backends ... ok
test handle_size_is_16_bytes_compile_time ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

EXIT_CODE=0
```

Log: `scripts/logs/wave4_handle_test.log`

## 4. DoD assessment

| DoD criterion | Status |
|---|---|
| A test exists that asserts `size_of::<ChannelHandle>() == 16` (or equivalent) | **PASS** — `handle_size_is_16_bytes_compile_time` asserts `size_of::<ChannelHandle>() == 16` on a `#[repr(C)]` mirror struct with 4 × `i32` fields; plus 3 IR-level tests verifying the codegen actually emits the 16-byte / 4-fd layout. |
| The test passes | **PASS** — `cargo test --release --test ipc_handle_layout_test` → 4 passed; 0 failed; exit 0. |
| Summary markdown at `vuma/scripts/audit/wave4_handle_layout.md` | **PASS** (this file). |

## 5. Constraint check

- No source files edited under `vuma/src/`. Verified: `git status` after
  the test run shows only the new `tests/ipc_handle_layout_test.rs` and
  this markdown as additions.
- New test file added under `vuma/tests/` — explicitly permitted.
- No push (local commit only).
- No further sub-agents spawned.
- Time budget: ~10 minutes (one 55s `cargo test --no-run` to warm the
  release cache, one 0.00s test run; the audit + test authoring was the
  bulk).

## 6. Note for the orchestrator

The channel handle is an IR-level buffer (`IRInstr::Alloc { size: 16 }`),
not a Rust struct, so the literal `size_of::<ChannelHandle>() == 16`
assertion cannot reference a type defined in `vuma-codegen`. The test
satisfies the DoD's "or equivalent" clause by defining a `#[repr(C)]`
mirror struct with the documented field layout AND by verifying at the IR
level that `lower_ipc_builtins` actually emits a 16-byte Alloc plus four
I32 Stores at offsets `{0, 4, 8, 12}` plus two `pipe2` syscalls. The
mirror struct's field offsets are themselves asserted to match the IR
offsets, binding the two checks together.

## Status: PASS
