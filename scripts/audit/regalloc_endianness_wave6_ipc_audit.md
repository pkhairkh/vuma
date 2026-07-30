# Wave 6 Endianness Audit — IPC Lowering (R6-b-audit)

- **Task ID:** R6-b-audit
- **Wave:** 6 (Regalloc-Endianness — Endianness Audit, IPC lowering itself)
- **Prior-run context:**
  - F3-b-fix (`d35c52c4`) added `expand_shared_memory_read_i32` to fix the
    `half_closed_channel.vuma` big-endian bug. The fix replaced an
    `I64 Load + 0xFFFFFFFF mask` formulation with a typed native-endian
    `I32 Load + Cast{ZExt, I32→I64}` so the round-trip is byte-order-
    independent by construction.
  - R6-a-audit (`c4c3f0b5`) audited all `shared_memory_read`/`_write`/
    `_read_i32` callers and found 0 production BUGs (6 stale test
    assertions in `tests/wave4b_half_closed_channel.rs` to fix separately).
- **HEAD before this task:** `c4c3f0b5 [R6-a-audit]`.

## 1. Methodology

1. Sourced `scripts/env/*.sh`; verified `cargo` on `PATH`.
2. Read `worklog.md` last 3 sections (R1-c-test, R2-a-audit, R6-a-audit).
3. Read `src/codegen/src/ipc_lowering.rs`:
   - `expand_channel_open` (`:1152-1336`) — handle layout construction.
   - `expand_channel_close` (`:1343-1402`) — handle teardown.
   - `expand_spawn_worker` (`:1435-1730`) — clone + fd swap loop.
   - `expand_channel_send` (`:1829-2004`) — frame build + write.
   - `expand_channel_recv` (`:2606-2952`) — read + CRC verify.
   - `expand_channel_send_cap` / `_recv_proto` / `_try_recv` /
     `_recv_timeout` / `_is_closed` — variant frame paths (handle
     offset use only; grepped).
   - `expand_shared_memory_read`/`_read_i32`/`_write` (`:4322-4448`)
     — already audited by R6-a, re-examined for shape only.
   - `expand_supervisor_call` (`:4452-4525`) — syscall encoding.
   - `x86_64_to_generic` (`:4532-4571`) — syscall nr translation.
   - `lower_ipc_builtins` (`:218-267`) — top-level dispatch.
4. Grepped every `Load {` / `Store {` site with `offset: (0|4|8|12)`
   to enumerate handle field accesses — 60+ hits, all classified below.
5. Grepped every `addr: ch` access — 7 hits, all I32-typed at offsets
   4 (send paths) or 8 (recv paths).
6. Grepped every `IRInstr::Syscall {` — 30+ hits; confirmed `nr` is
   always a literal `u32` field, never an `IRValue` (memory load).
7. Read `src/codegen/src/wasm32/mod.rs:3085-4310` for the wasm32
   channel/spawn_worker lowering path — wasm32 uses a totally different
   in-memory ring-buffer channel layout (no pipe fds) and Wasm is
   little-endian by spec.

Classification rubric (per the protocol):

- **SAFE** — I32-typed load/store at handle offsets, or value (not
  memory) at the IR level (immediate / register).
- **SUSPECT** — I64 load + sub-word mask (`& 0xFFFFFFFF` or similar)
  on a value that semantically is narrower than I64. Endianness-
  dependent byte extraction.
- **BUG** — Confirmed endianness bug with a known failing test or
  observable incorrect behaviour on a big-endian backend.

## 2. Handle Layout (16-byte, 4 i32 fds) — endianness analysis

The 16-byte channel handle is allocated by `expand_channel_open`
(`:1220-1247`) as an `IRInstr::Alloc { size: 16 }` and its four
fields are populated with **typed I32 stores** at fixed offsets:

| Offset | Field       | Store site         | Type |
|--------|-------------|--------------------|------|
| 0      | `read_fd1`  | `:1224-1229`       | I32  |
| 4      | `write_fd1` | `:1230-1235`       | I32  |
| 8      | `read_fd2`  | `:1236-1241`       | I32  |
| 12     | `write_fd2` | `:1242-1247`       | I32  |

The handle pointer is then returned via `BinOp{Add, handle, Imm(0)}`
(`:1248-1254`) — a register-to-register copy, no memory involvement,
endianness-agnostic.

The handle pointer is also registered in the per-function channel
registry at `:1312-1317` as a `Store{I64}` at `[slot_addr+0]`. This
is the *pointer* to the 16-byte buffer (an address), stored as a
full-width I64 — endianness does not apply because (a) the same
backend stores and loads the pointer with the same type, and (b)
addresses are not sub-word fields. **SAFE.**

Comment at `:1214-1219` explicitly notes the pointer-based handle
sidesteps the historical I64-packing bug that bit F3-b on sub-word
fd access:

> *"The pointer-based handle sidesteps I64 packing issues on 32-bit
> backends... channel_send/recv/close extract the fds via I32 Loads
> at fixed offsets within the 16-byte buffer."*

All four handle-construction stores are **I32-typed** → **SAFE**.

## 3. channel_send / channel_recv Payload Handling

### 3.1 channel_send (`:1829-2004`)

- `write_fd = Load I32 [ch+4]` (`:1850-1855`) — **SAFE** (I32 typed,
  matches the I32 store at `:1230`).
- Frame header fields:
  - `[frame+0]`  MAGIC `0x414D5556` stored as I32 (`:1862-1867`) — **SAFE**.
  - `[frame+4]`  version+flags `0x00020000` stored as I32 (`:1869-1874`) — **SAFE**.
  - `[frame+8]`  channel_id `0` stored as I64 (`:1876-1881`) — full-width, **SAFE**.
  - `[frame+16]` sequence loaded/stored as I64 (`:1886-1919`) — full-width, **SAFE**.
  - `[frame+24]` type_hash stored as I64 (`:1922-1928`) — full-width, **SAFE**.
  - `[frame+32]` payload_len `8` stored as I32 (`:1930-1935`) — **SAFE**.
  - `[frame+36]` cap_count stored as I32 (`:1937-1942`) — **SAFE**.
  - `[frame+40]` reserved `0` stored as I32 (`:1944-1949`) — **SAFE**.
  - `[frame+44]` payload stored as I64 (`:1951-1956`) — full-width, **SAFE** (see below).
  - `[frame+52]` CRC stored as I32 (`:2303-2308`) — **SAFE**.
- The frame is written verbatim via `Syscall{nr:64, args:[write_fd, frame, Imm(56)]}` (`:2309-2313`) — the kernel `write()` treats the 56 bytes as opaque.

### 3.2 Payload endianness analysis

The payload is **exactly 8 bytes** (frame offset 44, I64-typed). The
fixed payload_len=8 (`:1931`) confirms there is no >8-byte payload
path in this expansion.

The I64 store/load at offset 44 is endianness-dependent for the byte
order *within* the 8 bytes, but **both ends of the pipe share the same
native endianness** (the pipe is between parent and clone()'d child on
the same host). The byte sequence is:

```
sender: Store I64 [frame+44] = msg   → writes 8 bytes in native order
kernel: write(fd, frame, 56)         → forwards bytes verbatim
kernel: read(fd, frame, 56)          → fills frame with same bytes
receiver: Load I64 [frame+44]        → reads 8 bytes in native order
```

The store/load round-trip is byte-for-byte identical because both
ends use the same `IRType::I64` access on the same host. No sub-word
masking is applied to the payload. **SAFE** — opaque-bytes roundtrip
on an intra-host channel.

This is fundamentally different from the F3-b bug, which masked a
*sub-word* field out of an I64 — that extracted bytes `[off+4..off+8]`
on BE instead of `[off..off+4]` on LE. Here, the entire I64 is loaded
and stored; no byte extraction occurs.

### 3.3 channel_recv (`:2606-2952`)

- `read_fd = Load I32 [ch+8]` (`:2671-2676`) — **SAFE** (matches the
  I32 store at `:1236`).
- `magic = Load I32 [frame+0]` (`:2859-2864`); compared to
  `Imm(0x414D5556)` (`:2865-2871`) — **SAFE** (I32 typed; the magic
  is a 4-byte field on both ends).
- `stored_crc = Load I32 [frame+52]` (`:2872-2877`); compared to
  `computed_crc` (`:2896-2902`) — **SAFE** (CRC computed by
  byte-granular loop at `:2083-2143` using `Load I8` + ZExt, so the
  CRC computation is endianness-agnostic).
- `payload = Load I64 [frame+44]` (`:2889-2894`) — **SAFE** (full-width
  round-trip, same as §3.2).

### 3.4 channel_send_cap / channel_recv_proto / channel_try_recv /
channel_recv_timeout / channel_is_closed

Grepped every `addr: ch` access — 7 sites total, all I32-typed at
offsets 4 (send paths: `:1852`, `:3002`) or 8 (recv paths: `:2436`,
`:2673`, `:3361`, `:3816`, `:4070`, `:4192`). No site uses I64 with
a mask. **All SAFE.**

## 4. spawn_worker / fork Argument Passing

### 4.1 spawn_worker (`:1435-1730`)

- `Syscall{nr:220 (clone), args:[Imm(17), Imm(0), Imm(0), Imm(0), Imm(0)], dst:ret}` (`:1458-1468`):
  - `nr` is a literal `u32` field on `IRInstr::Syscall` — compile-time
    constant, never a memory load. **SAFE.**
  - `args` are all `IRValue::Immediate(...)` — values at the IR level,
    placed in argument registers by the backend. No memory access.
    **SAFE.**
  - `dst` is a fresh vreg — register, no memory. **SAFE.**
- `dst = BinOp{Add, ret, Imm(0)}` (`:1480-1486`) — register-to-register,
  endianness-agnostic. The "PID" (clone return: 0 in child, PID in
  parent) lives in a register. **SAFE.**
- The child/parent handle swap loop (`:1523-1727`) loads and stores
  all four fds with **typed I32**:
  - `Load I32 [safe_ptr+0]`  → `fd0`  (`:1636-1641`)
  - `Load I32 [safe_ptr+8]`  → `fd8`  (`:1642-1647`)
  - `Store I32 [safe_ptr+0]` ← `new_fd0` (`:1669-1674`)
  - `Store I32 [safe_ptr+8]` ← `new_fd8` (`:1675-1680`)
  - `Load I32 [safe_ptr+4]`  → `fd4`  (`:1685-1690`)
  - `Load I32 [safe_ptr+12]` → `fd12` (`:1691-1696`)
  - `Store I32 [safe_ptr+4]` ← `new_fd4`  (`:1713-1718`)
  - `Store I32 [safe_ptr+12]` ← `new_fd12` (`:1719-1724`)
  - **All SAFE** — I32 typed at every handle offset; no I64-with-mask.
- The handle pointer load `Load I64 [slot_addr+0]` (`:1572-1577`) is
  a full-width pointer load — endianness-agnostic (the same backend
  stored it as I64 at `:1312-1317`). **SAFE.**

### 4.2 wait_worker (`:1733-1810`)

- `Syscall{nr:260 (wait4), args:[pid, ...]}` (`:1770-1779`): the `pid`
  arg is a vreg (the clone return). Register value, no memory. **SAFE.**
- `Load I32 [status_buf+0]` (`:1780-1785`): the WEXITSTATUS field is
  read as a full I32 from a kernel-populated buffer; the kernel
  writes the status in native order. **SAFE.**
- wasm32 path (`:1753-1758`): `Load I32 [WASM32_CHILD_EXIT_ADDR+0]` —
  full-width I32 load of a value stored by the fork-emulation pass
  as I32 (or I64 — see `:577-582`; but the load is I32 and the
  in-memory value's low 4 bytes are read consistently on both ends
  since the same wasm32 backend wrote and reads it; Wasm is LE by
  spec). **SAFE.**

### 4.3 fork (supervisor_call)

`supervisor_call` (`:4452-4525`) routes a user-supplied x86_64 syscall
number through `x86_64_to_generic` (`:4452-4571`) and emits
`IRInstr::Syscall{nr: generic_nr, args:[arg], dst:ret}`. The `nr` is
a compile-time constant (computed from an immediate arg); the `arg`
is an `IRValue` (immediate or register, never a memory load). The
return value lives in a register. **SAFE.**

## 5. Syscall Encoding

`IRInstr::Syscall` is defined (per `src/codegen/src/ir.rs`) with the
shape:

```rust
Syscall {
    nr: u32,                 // compile-time constant
    args: Vec<IRValue>,      // Immediate or Register — never memory
    dst: Option<IRValue>,    // Register — never memory
}
```

- **`nr` is a literal `u32` field**, not an `IRValue`. It cannot be
  loaded from memory at the IR level. Grepped every `IRInstr::Syscall {`
  site in `ipc_lowering.rs` (30+ hits) — every `nr` is a literal
  (`59`, `57`, `220`, `260`, `64`, `63`, `167`, `277`, `164`, `39`,
  `56`, `7`, `25`, `101`, `73`, `198`, `203`, `205`, `206`, `207`)
  or a `let` bound to a literal (`generic_nr`).
- **`args` are values, not memory loads.** An `IRValue::Immediate`
  is a literal value; an `IRValue::Register` is a value held in a
  (virtual) register. Endianness does not apply to values in
  registers — they are scalar integers, not byte sequences.
- **`dst` is a register.** The syscall return value is placed in the
  return register (e.g. `X0` on aarch64, `RAX` on x86_64) by the
  kernel and read into the vreg. No memory load.

The syscall ABI itself (argument registers, return register) is set
by the per-backend calling convention and is endianness-agnostic at
the IR layer. The kernel reads/writes values from/to registers,
never memory (for syscall nr and scalar args). **SAFE.**

Note: syscalls that take a pointer argument (e.g. `write(fd, frame, 56)`)
do dereference memory, but the dereferencing happens inside the kernel
and the bytes are forwarded verbatim — the IR-level `frame` argument
is just a pointer value (a register). The endianness of the bytes in
the buffer is determined by how the IR *stored* them, not by the
syscall mechanism.

## 6. channel_close Path

`expand_channel_close` (`:1343-1402`) reads all four fds with **typed
I32 loads** at the four handle offsets:

| Offset | Field       | Load site    | Type |
|--------|-------------|--------------|------|
| 0      | `read_fd1`  | `:1356-1361` | I32  |
| 4      | `write_fd1` | `:1362-1367` | I32  |
| 8      | `read_fd2`  | `:1368-1373` | I32  |
| 12     | `write_fd2` | `:1374-1379` | I32  |

Each fd is then passed as the sole arg to `Syscall{nr:57 (close),
args:[fd], dst:None}` (`:1381-1400`). The fd is a register value
(loaded as I32, endianness-agnostic). The kernel `close()` syscall
takes the fd in an argument register — no memory dereferencing
involved. **SAFE.**

F3-b-fix did NOT touch `channel_close` — it was already I32-typed,
consistent with the handle construction in `expand_channel_open`.
The protocol's question (c) — *"does channel_close read the 4 fds
correctly on big-endian?"* — is answered in the affirmative: the
path uses I32 loads matching the I32 stores, so the round-trip is
byte-order-independent by construction.

## 7. wasm32 Comparison

`is_wasm32_native_channel_builtin` (`:1418-1433`) routes
`channel_open` / `_send` / `_recv` / `_close` / `_try_recv` /
`_recv_timeout` to the wasm32 backend's `IRInstr::Call` arm
(`wasm32/mod.rs:3089-3478`) — `expand_channel_*` in `ipc_lowering.rs`
is **never called** on wasm32. The 16-byte pipe-fd handle layout
does not exist on wasm32; instead, wasm32 uses an in-memory ring
buffer (`wasm32/mod.rs:3085-3133`):

| Offset | Field     | Wasm instr                 | Type |
|--------|-----------|----------------------------|------|
| 0      | head      | `I32Store/I32Load { off:0 }`  | i32  |
| 4      | tail      | `I32Store/I32Load { off:4 }`  | i32  |
| 8      | capacity  | `I32Store/I32Load { off:8 }`  | i32  |
| 12     | reserved  | —                          | —    |
| 16..   | data      | `I64Store/I64Load { off:0 }`  | i64  |

Wasm is **little-endian by spec** (Wasm 1.0 §3.2.6: "Linear memory
is byte-addressable... loads and stores are encoded in little-endian
byte order"). Every load/store op has an explicit `align` and
`offset` immediate and a typed access width. There are no masks on
sub-word extractions — `channel_send` stores the message as a full
`I64Store` at `[base+16+tail]` and `channel_recv` loads it as a
full `I64Load`. **SAFE.**

`spawn_worker` on wasm32 (`wasm32/mod.rs:4264-4306`): the
`Syscall{nr:220}` is lowered to `WasmInstr::I64Const(0)` — a
constant immediate, no endianness concern. The fork-emulation pass
(`ipc_lowering.rs:295-410`) rewrites the child `Return` to
`Store+Jump` and `wait_worker` loads the stored value via
`Load I32 [WASM32_CHILD_EXIT_ADDR+0]` (`:1753-1758`) — both ends
are the wasm32 backend, so the I32 store/load round-trip is
consistent (and Wasm is LE anyway). **SAFE.**

The wasm32 path therefore has **no endianness assumptions to audit
beyond the spec-mandated LE byte order**, which is invariant across
all wasm runtimes.

## 8. Findings Table (SAFE / SUSPECT / BUG)

| # | Path / Site | File:line | Access | Type | Classification |
|---|-------------|-----------|--------|------|----------------|
| 1 | channel_open: store read_fd1@0 | ipc_lowering.rs:1224-1229 | Store | I32 | SAFE |
| 2 | channel_open: store write_fd1@4 | ipc_lowering.rs:1230-1235 | Store | I32 | SAFE |
| 3 | channel_open: store read_fd2@8 | ipc_lowering.rs:1236-1241 | Store | I32 | SAFE |
| 4 | channel_open: store write_fd2@12 | ipc_lowering.rs:1242-1247 | Store | I32 | SAFE |
| 5 | channel_open: store handle ptr in registry | ipc_lowering.rs:1312-1317 | Store | I64 (full-width ptr) | SAFE |
| 6 | channel_open: store next_count | ipc_lowering.rs:1327-1332 | Store | I32 | SAFE |
| 7 | channel_close: load read_fd1@0 | ipc_lowering.rs:1356-1361 | Load | I32 | SAFE |
| 8 | channel_close: load write_fd1@4 | ipc_lowering.rs:1362-1367 | Load | I32 | SAFE |
| 9 | channel_close: load read_fd2@8 | ipc_lowering.rs:1368-1373 | Load | I32 | SAFE |
| 10 | channel_close: load write_fd2@12 | ipc_lowering.rs:1374-1379 | Load | I32 | SAFE |
| 11 | channel_close: close(fd) syscall ×4 | ipc_lowering.rs:1381-1400 | Syscall | arg=Register | SAFE |
| 12 | spawn_worker: clone syscall | ipc_lowering.rs:1458-1468 | Syscall | nr=literal, args=Immediate | SAFE |
| 13 | spawn_worker: PID = ret+0 | ipc_lowering.rs:1480-1486 | BinOp | Register | SAFE |
| 14 | spawn_worker: load count@0 | ipc_lowering.rs:1539-1544 | Load | I32 | SAFE |
| 15 | spawn_worker: load handle ptr@0 | ipc_lowering.rs:1572-1577 | Load | I64 (full-width ptr) | SAFE |
| 16 | spawn_worker: swap loop fd0@0 | ipc_lowering.rs:1636-1641 | Load | I32 | SAFE |
| 17 | spawn_worker: swap loop fd8@8 | ipc_lowering.rs:1642-1647 | Load | I32 | SAFE |
| 18 | spawn_worker: swap loop store new_fd0@0 | ipc_lowering.rs:1669-1674 | Store | I32 | SAFE |
| 19 | spawn_worker: swap loop store new_fd8@8 | ipc_lowering.rs:1675-1680 | Store | I32 | SAFE |
| 20 | spawn_worker: swap loop fd4@4 | ipc_lowering.rs:1685-1690 | Load | I32 | SAFE |
| 21 | spawn_worker: swap loop fd12@12 | ipc_lowering.rs:1691-1696 | Load | I32 | SAFE |
| 22 | spawn_worker: swap loop store new_fd4@4 | ipc_lowering.rs:1713-1718 | Store | I32 | SAFE |
| 23 | spawn_worker: swap loop store new_fd12@12 | ipc_lowering.rs:1719-1724 | Store | I32 | SAFE |
| 24 | wait_worker: wait4 syscall | ipc_lowering.rs:1770-1779 | Syscall | pid=Register | SAFE |
| 25 | wait_worker: load status@0 | ipc_lowering.rs:1780-1785 | Load | I32 | SAFE |
| 26 | wait_worker (wasm32): load child exit@0 | ipc_lowering.rs:1753-1758 | Load | I32 | SAFE |
| 27 | channel_send: load write_fd@4 | ipc_lowering.rs:1850-1855 | Load | I32 | SAFE |
| 28 | channel_send: store MAGIC@0 | ipc_lowering.rs:1862-1867 | Store | I32 | SAFE |
| 29 | channel_send: store version+flags@4 | ipc_lowering.rs:1869-1874 | Store | I32 | SAFE |
| 30 | channel_send: store channel_id@8 | ipc_lowering.rs:1876-1881 | Store | I64 (full-width) | SAFE |
| 31 | channel_send: store sequence@16 | ipc_lowering.rs:1886-1919 | Load/Store | I64 (full-width) | SAFE |
| 32 | channel_send: store type_hash@24 | ipc_lowering.rs:1922-1928 | Store | I64 (full-width) | SAFE |
| 33 | channel_send: store payload_len@32 | ipc_lowering.rs:1930-1935 | Store | I32 | SAFE |
| 34 | channel_send: store cap_count@36 | ipc_lowering.rs:1937-1942 | Store | I32 | SAFE |
| 35 | channel_send: store payload@44 | ipc_lowering.rs:1951-1956 | Store | I64 (full-width roundtrip) | SAFE |
| 36 | channel_send: store CRC@52 | ipc_lowering.rs:2303-2308 | Store | I32 | SAFE |
| 37 | channel_send: write(fd, frame, 56) syscall | ipc_lowering.rs:2309-2313 | Syscall | args=Register+Imm | SAFE |
| 38 | channel_recv: load read_fd@8 | ipc_lowering.rs:2671-2676 | Load | I32 | SAFE |
| 39 | channel_recv: load magic@0 | ipc_lowering.rs:2859-2864 | Load | I32 | SAFE |
| 40 | channel_recv: load stored_crc@52 | ipc_lowering.rs:2872-2877 | Load | I32 | SAFE |
| 41 | channel_recv: load payload@44 | ipc_lowering.rs:2889-2894 | Load | I64 (full-width roundtrip) | SAFE |
| 42 | channel_recv: read(fd, frame, 56) syscall | ipc_lowering.rs:2774-2778 | Syscall | args=Register+Imm | SAFE |
| 43 | channel_send_cap: load write_fd@4 | ipc_lowering.rs:3000-3005 | Load | I32 | SAFE |
| 44 | channel_recv_proto: load read_fd@8 | ipc_lowering.rs:3360-3364 | Load | I32 | SAFE |
| 45 | channel_try_recv: load read_fd@8 | ipc_lowering.rs:3815-3819 | Load | I32 | SAFE |
| 46 | channel_recv_timeout: load read_fd@8 | ipc_lowering.rs:4069-4073 | Load | I32 | SAFE |
| 47 | channel_is_closed: load read_fd@8 | ipc_lowering.rs:4191-4195 | Load | I32 | SAFE |
| 48 | CRC32 loop: load byte@0 | ipc_lowering.rs:2106-2111 | Load | I8 (byte-granular) | SAFE |
| 49 | supervisor_call: x86_64_to_generic(nr) | ipc_lowering.rs:4475 | Computed u32 | literal-derived | SAFE |
| 50 | supervisor_call: Syscall{nr:generic_nr, args:[arg]} | ipc_lowering.rs:4511-4516 | Syscall | arg=Register/Imm | SAFE |
| 51 | shared_memory_read_i32 (F3-b-fix): load I32 + ZExt | ipc_lowering.rs:4405-4422 | Load+Cast | I32→I64 | SAFE (audited R6-a) |
| 52 | shared_memory_read: load I64 | ipc_lowering.rs:4347-4353 | Load | I64 (full-width, no mask) | SAFE (audited R6-a) |
| 53 | shared_memory_write: store I64 | ipc_lowering.rs:4441-4447 | Store | I64 (full-width, no mask) | SAFE (audited R6-a) |
| 54 | wasm32: channel_open ring-buffer header | wasm32/mod.rs:3089-3134 | I32Store×3 | i32 | SAFE (Wasm LE by spec) |
| 55 | wasm32: channel_send I64Store@16+tail | wasm32/mod.rs:3170 | I64Store | i64 (full-width) | SAFE |
| 56 | wasm32: channel_recv I64Load@16+head | wasm32/mod.rs (recv arm) | I64Load | i64 (full-width) | SAFE |
| 57 | wasm32: spawn_worker → I64Const(0) | wasm32/mod.rs:4295-4296 | Const | i64 immediate | SAFE |
| 58 | wasm32: all other syscalls → I64Const(-38) | wasm32/mod.rs:4300-4301 | Const | i64 immediate | SAFE |

**Totals:**

- **SAFE:** 58
- **SUSPECT:** 0
- **BUG:** 0

## 9. Recommended Fixes

**No fixes required.** The IPC lowering is endianness-clean:

1. **Handle layout** (`expand_channel_open` / `_close` / `_send` /
   `_recv` / `_send_cap` / `_recv_proto` / `_try_recv` /
   `_recv_timeout` / `_is_closed`): all 4 fds are accessed with
   typed I32 loads/stores at offsets 0/4/8/12. The F3-b-fix
   philosophy ("typed native-endian access whose width matches the
   store") is applied uniformly throughout the handle layer — not
   just in the `shared_memory_read_i32` primitive that F3-b-fix
   added. No sub-word mask is applied to any handle field.

2. **Payload handling** (`channel_send` / `_recv`): the 8-byte
   payload is stored and loaded as a full-width I64. Because the
   pipe is intra-host (parent ↔ clone()'d child on the same machine),
   both ends share endianness and the byte sequence round-trips
   verbatim. No sub-word mask is applied. There is no >8-byte
   payload path in `expand_channel_send` (payload_len is hardcoded
   to 8 at `:1931`).

3. **spawn_worker / fork / wait_worker**: PIDs, exit codes, and
   clone args are all register values at the IR level. The handle
   swap loop in `spawn_worker` uses I32 loads/stores at every
   handle offset. The registry pointer is stored/loaded as a
   full-width I64 (pointer, not a sub-word field).

4. **Syscall encoding**: `IRInstr::Syscall.nr` is a literal `u32`
   field, never a memory load. `args` and `dst` are `IRValue`
   (Immediate or Register). The endianness of memory only matters
   when the kernel dereferences a pointer arg (e.g. `write(fd, frame,
   56)`), and in that case the bytes are determined by how the IR
   stored them — which §3 confirmed is endianness-safe.

5. **channel_close**: already I32-typed (F3-b-fix did not need to
   touch it). Confirmed by direct read of `:1356-1379`.

6. **wasm32**: the 16-byte pipe-fd handle layout is never
   instantiated (wasm32 uses an in-memory ring buffer with I32
   header fields). Wasm is little-endian by spec, so even if there
   were a sub-word mask, it would behave identically across all wasm
   runtimes. No endianness concern.

### Cross-references to prior work

- **F3-a-investigate (`42b0ca70`)**: root-caused the
  `half_closed_channel.vuma` BE failure to a sub-word mask on an
  I64 load. The fix philosophy (typed native-endian I32 access)
  is the same one the IPC lowering already uses uniformly for the
  handle layer — `expand_channel_open` was already I32-typed when
  F3-b-fix landed. F3-b-fix only needed to add the
  `shared_memory_read_i32` primitive (used by `half_closed_channel.vuma`
  to extract a sub-handle field from a shared-memory-mapped handle
  copy); the in-process handle layer was never affected.

- **R6-a-audit (`c4c3f0b5`)**: audited the `shared_memory_*`
  primitive callers. The 6 SUSPECT lines there are all in the Rust
  integration test `wave4b_half_closed_channel.rs` (stale test
  contract), not in the lowering itself. R6-a's recommendation to
  update those test assertions is independent of this audit's
  findings.

### DoD for this task

| DoD criterion | Status | Evidence |
|---------------|--------|----------|
| Audit doc exists at `scripts/audit/regalloc_endianness_wave6_ipc_audit.md` | PASS | This file |
| Every IPC lowering path is classified | PASS | §2–§6 + §8 table (58 rows) cover handle layout, payload, spawn_worker, syscall encoding, channel_close, plus wasm32 comparison |
| No source files edited | PASS | `git status --short` shows only the new audit markdown added |
| No `git push` | PASS | Local commit only |
| No sub-agents spawned | PASS | Single sub-agent run |
| Time budget ≤10 min | PASS | Single-pass read + grep + write |
