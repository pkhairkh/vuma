# Wave 4 — Caveat §2.2 Audit: `try_recv` on wasm32 is non-blocking

- **Task ID:** 4-d-test
- **Agent:** 4-d-test (sub-agent, wave 4)
- **Wave:** 4 (depends on waves 0 / 1 / 2 / 3 / 4-a-audit / 4-c-test)
- **Caveat addressed:** §2.2 — on wasm32 under fork emulation, `try_recv` is
  non-blocking: the parent always runs first, so the child's `try_recv`
  either finds a buffered message immediately or returns "empty" (`-2`).
- **Files in scope (READ-ONLY audit + this summary):**
  - `src/codegen/src/wasm32/mod.rs` (read-only)
  - `src/codegen/src/ipc_lowering.rs` (read-only)
  - `tests/gold_standard/ipc/try_recv.vuma` (test program)
- **DoD:**
  1. `try_recv` on wasm32 is lowered to a non-blocking path
     (single load + compare + return, no spin/block).
  2. wasmtime execution (if attempted) exits 0 quickly.
  3. This summary markdown exists.

## 1. Test program used

`tests/gold_standard/ipc/try_recv.vuma` — opens a channel with **no sender**
and calls `channel_try_recv` on the empty channel:

```vuma
// Expected exit code: 77
transform main() -> i32 {
    ch = channel_open<i32>();
    result = channel_try_recv(ch);
    if result == 0 - 2 { return 77; }   // -2 == EAGAIN/empty sentinel
    return result;
}
```

The program asserts that `try_recv` on an empty channel returns `-2`
("empty") rather than blocking. Exit `77` ⟹ non-blocking empty-return
confirmed; a timeout / exit `124` ⟹ blocked (failure).

## 2. Lowering path observed — single load + compare + return (no spin/block)

### 2a. Source-level confirmation (two files cooperate)

**`src/codegen/src/ipc_lowering.rs:1404-1419`** — `is_wasm32_native_channel_builtin`
explicitly lists `channel_try_recv` as a wasm32-native builtin:

```rust
fn is_wasm32_native_channel_builtin(name: &str) -> bool {
    matches!(name,
        "channel_open" | "channel_send" | "channel_recv"
        | "channel_close" | "channel_try_recv" | "channel_recv_timeout"
    )
}
```

**`src/codegen/src/ipc_lowering.rs:846`** — `split_block_at_first_ipc`
SKIPS these on wasm32 so the `Call` reaches the backend intact:

```rust
!(ctx.backend == BackendKind::Wasm32 && is_wasm32_native_channel_builtin(fname))
```

The generic unix-pipe expander `expand_channel_try_recv`
(`ipc_lowering.rs:3769-…`) — which emits `nanosleep` → `poll` → `read` →
CRC32 framing — is therefore **NOT** invoked for `channel_try_recv` on
wasm32. The `Call` survives `lower_ipc_builtins` and reaches the wasm32
backend's instruction selector.

**`src/codegen/src/wasm32/mod.rs:3388-3470`** — the wasm32 backend's own
`IRInstr::Call` arm for `"channel_try_recv"` emits the non-blocking
ring-buffer path (comment: *"K11A-wasm32-fork-emulation: non-blocking
try_recv on the in-memory ring buffer. Returns the payload (i32) when
data is available, or -2 (EAGAIN) when the buffer is empty."*):

```text
base_local = ch
head_local = i32.load [base+0]
tail_local = i32.load [base+4]
if head_local == tail_local:        ; empty
    push i32.const -2               ; EAGAIN sentinel
else:                               ; data available
    payload = i64.load [base+16+head_local]
    i32.wrap_i64
    new_head = (head_local + 8) % i32.load[base+8]
    i32.store [base+0] = new_head
end
```

There is **no** `poll`, **no** `read` syscall, **no** `nanosleep`, **no**
`clock_time_get` busy-wait, and **no** `br`/`loop` back-edge that would
form a spin-loop. The path is a single memory-load of `head`, a single
memory-load of `tail`, an `i32.eq`, and a conditional push of `-2` or
the payload. This is the "single load + compare + return" non-blocking
form the DoD requires.

### 2b. Emitted bytecode confirmation (`dump_ir` allocated output)

`target/release/dump_ir tests/gold_standard/ipc/try_recv.vuma wasm32`
produces the allocated wasm bytecode for `main`. The `channel_try_recv`
lowering is (verbatim from the log):

```text
local.get 6                     ; base (channel handle ptr)
i32.load align=2 offset=0       ; head = [base+0]
local.set 7                     ; head_local
local.get 6                     ; base
i32.load align=2 offset=4       ; tail = [base+4]
local.set 8                     ; tail_local
local.get 7                     ; head
local.get 8                     ; tail
i32.eq                          ; cond = (head == tail)
if i32                          ; ↧ if cond:
  i32.const -2                  ;   push -2 (EAGAIN / empty sentinel)
else                            ; ↧ else (data available):
  local.get 6                   ;   base
  i32.const 16                  ;   16
  i32.add                       ;   base+16
  local.get 7                   ;   head
  i32.add                       ;   base+16+head
  i64.load align=3 offset=0     ;   payload = [base+16+head] (8 bytes)
  i32.wrap_i64                  ;   wrap to i32
  local.get 6                   ;   base
  local.get 7                   ;   head
  i32.const 8                   ;   8
  i32.add                       ;   head+8
  local.get 6                   ;   base
  i32.load align=2 offset=8     ;   capacity = [base+8]
  i32.rem_u                     ;   (head+8) % capacity
  i32.store align=2 offset=0    ;   [base+0] = new_head
end                             ; result on stack
```

This matches the source at `wasm32/mod.rs:3394-3469` instruction-for-
instruction. The only `br` in `main` is `br 2` (the dispatch-loop exit
after the call returns) — there is **no backward branch** that would
form a spin/retry loop.

### 2c. `.wasm` import table — no `poll` import

The emitted `/tmp/try_recv.wasm` (1203 bytes) imports exactly 9 WASI
functions:

```text
fd_write, proc_exit, fd_read, fd_close, fd_seek,
clock_time_get, random_get, args_sizes_get, args_get
```

**`poll` / `poll_oneoff` is NOT imported.** The unix-pipe path
(`expand_channel_try_recv` in `ipc_lowering.rs`) would require a `poll`
syscall to probe the pipe; its absence proves the wasm32 ring-buffer
path was used instead. (`fd_read` and `clock_time_get` are present for
`__vuma_print_int` / `__vuma_print_hex` and the dispatch-loop clock,
not for `try_recv` — the `try_recv` bytecode above uses neither.)

## 3. wasmtime execution result

```text
$ vuma emit wasm32 tests/gold_standard/ipc/try_recv.vuma -o /tmp/try_recv.wasm
Emitted tests/gold_standard/ipc/try_recv.vuma -> /tmp/try_recv.wasm (1203 bytes, ISA: wasm32, PMT-verified)

$ time wasmtime run --invoke _vuma_main /tmp/try_recv.wasm
EXIT=77

real    0m0.007s
user    0m0.001s
sys     0m0.006s
```

| Metric | Value | Interpretation |
|---|---|---|
| Exit code | `77` | Expected success: `try_recv` returned `-2` (empty) on the empty channel, `main` returned `77`. **Non-blocking empty-return confirmed.** |
| Wall time | `0.007s` | Effectively instant — no block, no spin, no timeout. |
| stdout / stderr | empty | Program returns via `_vuma_main` without printing. |

A blocking `try_recv` (e.g. the unix-pipe path mis-lowered to wasm32)
would either (a) trap on a missing `poll` import, (b) spin forever on
a backward branch (exit `124` under the test harness timeout), or (c)
return a wrong exit code. None of these occurred.

## 4. DoD assessment

| DoD criterion | Status | Evidence |
|---|---|---|
| `try_recv` on wasm32 lowered to non-blocking path (single load + compare + return, no spin/block) | **PASS** | Source `wasm32/mod.rs:3394-3469` + allocated bytecode in §2b; no `poll` import in §2c |
| wasmtime execution exits 0 quickly | **PASS** | Exit `77` (program's expected success code) in `0.007s` (§3) |
| Summary markdown at `vuma/scripts/audit/wave4_try_recv_nonblocking.md` | **PASS** | This file |

## 5. Constraint check

- No source files edited. `git status --short` shows only this new
  markdown (and the worklog append). `/tmp/try_recv.wasm` and
  `/tmp/two_fork_sites.vuma` are ephemeral, outside the repo.
- No push (local commit only).
- No further sub-agents spawned.
- Time budget: ~7 minutes (env setup + compile + wasmtime run + audit).

## 6. Note for orchestrator

The non-blocking `try_recv` semantics on wasm32 are confirmed both
statically (source + emitted bytecode) and dynamically (wasmtime
exits `77` in 7 ms). The mechanism is a two-file contract:

1. `ipc_lowering::is_wasm32_native_channel_builtin` declares
   `channel_try_recv` as wasm32-native → `split_block_at_first_ipc`
   skips it → `lower_ipc_builtins` leaves the `Call` intact.
2. `wasm32::lower_instruction`'s `IRInstr::Call` arm for
   `"channel_try_recv"` emits the ring-buffer load+compare+return path.

This contract is robust: the unix-pipe `expand_channel_try_recv`
(used by all non-wasm32 backends) is never invoked on wasm32, so the
`poll` / `read` / `nanosleep` syscalls that WOULD block (or trap, since
wasm32 has no pipe2/poll syscalls) cannot leak into the wasm32 output.
The caveat §2.2 claim — "the child's `try_recv` either finds a buffered
message immediately or returns 'empty'" — is faithfully implemented.

### Status: PASS
