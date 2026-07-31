# F3-a — Big-Endian `half_closed_channel.vuma` Root-Cause Report

- **Task ID:** F3-a-investigate
- **Wave:** 3 (Big-Endian `half_closed_channel` Fix — investigation)
- **Prior-run context:** Pi5 cluster commit `d45d74a0` reported 6 failures
  (MM = mismatch) on `tests/gold_standard/ipc/half_closed_channel.vuma`
  across all 6 big-endian backends: `aarch64_be`, `mips64be`, `ppc64`,
  `s390x`, `m68k`, `hppa`. The prior Wave 4-b run only did static IR
  verification (CLI gap); the Pi5 cluster ran it at runtime and exposed
  an endianness bug.
- **HEAD before this task:** `b3529ef0 [followup-wave-2-dod-pass]`
- **Scope:** READ-ONLY investigation. No source files edited. The fix
  is deferred to F3-b.

---

## 1. Reproduction

Environment shims sourced from `scripts/env/*.sh` (sets `LD_LIBRARY_PATH`
to `$HOME/.local/lib` so `libz3.so` resolves). Tooling:

- `target/release/compile_dump` (release build present).
- `qemu-aarch64_be-static` at `$HOME/.local/bin/`.
- `qemu-x86_64-static` (system).

### 1.1 Compile + run on `aarch64_be` (failing backend)

```text
$ target/release/compile_dump tests/gold_standard/ipc/half_closed_channel.vuma /tmp/hc_be.bin aarch64_be
IVE: Pass passed=1 failed=0 unverified=0 total=1 discharge_rate=100%
Wrote 14476 bytes to /tmp/hc_be.bin
$ file /tmp/hc_be.bin
/tmp/hc_be.bin: ELF 64-bit MSB executable, ARM aarch64, version 1 (GNU/Linux), statically linked
$ qemu-aarch64_be-static /tmp/hc_be.bin; echo "exit=$?"
exit=1
```

**Result: exit code 1 — FAIL** (test expects exit 0). Reproduces the
Pi5 cluster's MM (mismatch) on `aarch64_be`.

### 1.2 Compile + run on `x86_64` (little-endian baseline, passing)

```text
$ target/release/compile_dump tests/gold_standard/ipc/half_closed_channel.vuma /tmp/hc_le.bin x86_64
IVE: Pass passed=1 failed=0 unverified=0 total=1 discharge_rate=100%
Wrote 16616 bytes to /tmp/hc_le.bin
$ qemu-x86_64-static /tmp/hc_le.bin; echo "exit=$?"
exit=0
```

**Result: exit code 0 — PASS.** Confirms the bug is endianness-specific
(LE passes, BE fails).

### 1.3 Negative companion sanity check

The companion test `half_closed_negative.vuma` (expects non-zero exit,
specifically `9 = EBADF`) was run on both backends to localise the bug:

```text
$ qemu-aarch64_be-static /tmp/hc_neg_be.bin; echo "neg_be_exit=$?"   # → 9
$ qemu-x86_64-static     /tmp/hc_neg_le.bin; echo "neg_le_exit=$?"   # → 9
```

The negative test PASSES on **both** endiannesses (exit 9 = EBADF on
both). This is the key localising evidence: the negative test only
checks that *some* closed-fd syscall returns `-EBADF`; it does not
check *which* fd was closed. So a wrong-fd extraction still yields
`-EBADF` on BE. The positive test, by contrast, checks that the
*surviving* direction still works — and on BE the wrong fd gets closed,
killing the surviving direction. (See §2 for the mechanism.)

---

## 2. Root Cause

### 2.1 The handle layout (correct, IR-level, backend-independent)

`expand_channel_open` (`src/codegen/src/ipc_lowering.rs:1138-1322`)
allocates a 16-byte handle and stores four `i32` fds at fixed offsets,
verified by `tests/ipc_handle_layout_test.rs` (passes on all backends
per Wave 4-a):

```text
handle offset →  field
       0       →  read_fd1   (read end,  pipe 1, parent→child)
       4       →  write_fd1  (write end, pipe 1, parent writes here)
       8       →  read_fd2   (read end,  pipe 2, child→parent; parent reads here)
      12       →  write_fd2  (write end, pipe 2, child writes here)
```

Relevant IR (`ipc_lowering.rs:1210-1244`):

```rust
IRInstr::Store { value: read_fd1,  addr: handle.clone(), offset: 0,  ty: IRType::I32 },
IRInstr::Store { value: write_fd1, addr: handle.clone(), offset: 4,  ty: IRType::I32 },
IRInstr::Store { value: read_fd2,  addr: handle.clone(), offset: 8,  ty: IRType::I32 },
IRInstr::Store { value: write_fd2, addr: handle,        offset: 12, ty: IRType::I32 },
```

Each `Store` is a **native-endian** `i32` store. The IR-level layout
(offsets, types, count) is correct on every backend — confirmed by
`channel_open_emits_16_byte_handle_with_4_i32_fds` and the
`handle_layout_is_backend_independent_across_non_wasm32_backends`
audit test. **The bug is NOT in the layout.**

### 2.2 The `shared_memory_read` primitive (correct semantic, wrong test usage)

`expand_shared_memory_read` (`ipc_lowering.rs:4308-4340`) is a generic
pointer-deref that emits a single native-endian `i64` load:

```rust
fn expand_shared_memory_read(args: &[IRValue], dst: Option<&IRValue>, ctx: &mut LowerContext) -> Vec<IRInstr> {
    ...
    vec![
        IRInstr::BinOp { op: BinOpKind::Add, dst: addr.clone(), lhs: ptr, rhs: offset, ty: Some(IRType::I64) },
        IRInstr::Load { dst, addr, offset: 0, ty: IRType::I64 },   // ← native-endian i64 load
    ]
}
```

This is a **native-endian** `i64` load — correct in the standard
compiler semantic (a `Load { ty: I64 }` reads 8 bytes in the CPU's
runtime byte order). The `aarch64_be` backend inherits
`arm64.rs`'s `select_load` unchanged (the wrapper only swaps ELF
headers; data instructions are emitted by the parent and the CPU's
`PSTATE.E` bit governs runtime endianness). Under
`qemu-aarch64_be-static`, `PSTATE.E = 1` and the same `LDR Xn, [Xm]`
instruction loads bytes in big-endian order.

### 2.3 The test's endianness-dependent fd extraction (the actual bug)

The test `tests/gold_standard/ipc/half_closed_channel.vuma:43-45` does:

```text
packed: i64 = shared_memory_read(ch, 4);   // i64 load from handle+4 → bytes [4..12]
wfd: i64     = packed & 4294967295;        // mask low 32 bits
syscall(57, wfd);                          // close(wfd)
```

The 8 bytes loaded from `handle+4` span `[write_fd1 (i32 @4)] [read_fd2 (i32 @8)]`.
The `& 0xFFFFFFFF` mask assumes the target fd (`write_fd1`) lives in
the **low 32 bits** of the packed `i64`. That assumption is true only
on little-endian:

| Backend byte order | i64 load interpretation of bytes `[4..12]` | `packed & 0xFFFFFFFF` extracts |
|---|---|---|
| Little-endian (x86_64, aarch64 LE, riscv64, …) | low 32 = bytes[4..8] = `write_fd1`; high 32 = bytes[8..12] = `read_fd2` | **`write_fd1`** ✓ (the fd the test intends to close) |
| Big-endian (aarch64_be, mips64be, ppc64, s390x, m68k, hppa) | high 32 = bytes[4..8] = `write_fd1`; low 32 = bytes[8..12] = `read_fd2` | **`read_fd2`** ✗ (the parent's recv end of pipe 2!) |

### 2.4 Why this produces exit code 1 on big-endian

On a big-endian backend the test therefore executes:

1. `channel_send(ch, 42)` → parent writes 42 to pipe-1 write end
   (`write_fd1`). Succeeds. Child reads 42. ✓
2. `wfd = packed & 0xFFFFFFFF` → **`read_fd2`** (parent's read end of
   pipe 2) — NOT the intended `write_fd1`.
3. `syscall(57, wfd)` = `close(read_fd2)` — closes the parent's
   *surviving-direction* read end. The intended `write_fd1` stays OPEN.
4. Child sends 99 to pipe-2 write end (`write_fd2`). Succeeds (the
   child's write end is unaffected).
5. Parent `channel_recv(ch)` tries to read from `read_fd2` — **but
   `read_fd2` was just closed in step 3.** The recv fails (returns 0
   / -EBADF / EOF). `y ≠ 99`.
6. `if y == 99 { return 0; } return 1;` → **returns 1.** Exit code 1.

This matches the observed QEMU exit code (§1.1) and the Pi5 cluster's
"MM" (mismatch) classification. The same mechanism applies identically
to all 6 failing big-endian backends: `mips64be`, `ppc64`, `s390x`,
`m68k`, `hppa` all use native-big-endian `i64` loads (none of the
backend wrappers perform a byte-swap on data loads — only ELF headers
are swapped, as documented for `aarch64_be` at `aarch64_be.rs:13` and
analogously for the other BE wrappers).

### 2.5 Why the negative companion passes on both endiannesses

`half_closed_negative.vuma` extracts `wfd` the same way, closes it,
then attempts `write(wfd, ch, 8)` and asserts the write returns a
negative errno (`-EBADF = -9`). On BE, `wfd = read_fd2`; closing
`read_fd2` and then `write(read_fd2, …)` returns `-EBADF` because
(a) the fd is closed AND (b) even if it were open, writing to a
read-end of a pipe is `EBADF`. Either way the symptom is `9`, so the
negative test passes on BE despite extracting the wrong fd. This is
why the Pi5 cluster flagged only the positive test.

### 2.6 What is NOT the bug

- **Not** the handle layout — `tests/ipc_handle_layout_test.rs`
  confirms offsets {0,4,8,12} × I32 on all backends.
- **Not** the syscall encoding — `syscall(57, wfd)` is correctly
  lowered (the negative test exercises `syscall(57, …)` and
  `syscall(64, …)` and returns the expected `-EBADF` on both
  endiannesses; §1.3).
- **Not** a regalloc / callee-saved issue — the failure is a clean
  exit-1 (logic mismatch), not a SIGSEGV or stack corruption.
- **Not** the `shared_memory_read` lowering per se — a native-endian
  `i64` load is the correct semantic for a generic pointer-deref
  primitive. The bug is the test's reliance on little-endian bit
  layout within that `i64`.

---

## 3. Proposed Minimal Fix (for F3-b — do NOT apply here)

The cleanest fix that is **minimal, portable across all backends, and
does not break any little-endian backend**: add a typed `i32` read
primitive so the test can load a single 4-byte fd directly, instead of
packing two fds into an `i64` and bit-masking.

### 3.1 Recommended fix — add `shared_memory_read_i32` builtin (source + test)

**Source change** (additive, ~12 lines in `src/codegen/src/ipc_lowering.rs`,
mirroring the existing `expand_shared_memory_read`):

```rust
fn expand_shared_memory_read_i32(
    args: &[IRValue],
    dst: Option<&IRValue>,
    ctx: &mut LowerContext,
) -> Vec<IRInstr> {
    if args.len() < 2 { return vec![]; }
    let ptr = args[0].clone();
    let offset = args[1].clone();
    let dst = match dst { Some(d) => d.clone(), None => return vec![] };
    let addr = ctx.new_vreg();
    vec![
        IRInstr::BinOp { op: BinOpKind::Add, dst: addr.clone(), lhs: ptr, rhs: offset, ty: Some(IRType::I64) },
        IRInstr::Load  { dst, addr, offset: 0, ty: IRType::I32 },   // ← native-endian i32 load
    ]
}
```

Wire it into the builtin dispatch table alongside the existing
`"shared_memory_read" => Expansion::flat(expand_shared_memory_read(...))`
entry at `ipc_lowering.rs:1043`, and into the `is_shared_memory_builtin`
predicate (or wherever `shared_memory_read` is registered for arg
validation).

Because the `i32` load is native-endian and matches the `i32` store
that `expand_channel_open` emitted at the same offset, the value read
back is the **same fd value** on every backend — no bit-mask, no
endianness branch, no packing. The existing `shared_memory_read` (i64)
is left untouched, so no little-endian caller is affected.

**Test change** (`tests/gold_standard/ipc/half_closed_channel.vuma:43-45`
and the analogous lines in `half_closed_negative.vuma:25-29`):

```text
wfd: i64 = shared_memory_read_i32(ch, 4);   // directly loads write_fd1 (i32 @4)
syscall(57, wfd);
```

The `i64` coercion is a widening (fd values are small non-negative
ints, no sign-extension concerns). The mask `& 4294967295` is dropped.

### 3.2 Why this is minimal and safe

- **No behavioural change on LE** — the existing `shared_memory_read`
  is unchanged; LE backends continue to produce identical binaries for
  any test that does not adopt the new builtin.
- **No new IR ops** — reuses `IRInstr::Load { ty: I32 }`, which every
  backend already lowers correctly (the same op `channel_close` uses
  to read the four fds at offsets {0,4,8,12}).
- **No endianness branches** — the fix is endianness-agnostic by
  construction: a native `i32` load of a native `i32` store always
  round-trips the value, regardless of byte order.
- **Fixes all 6 failing backends at once** — `aarch64_be`, `mips64be`,
  `ppc64`, `s390x`, `m68k`, `hppa` all share the same
  `expand_shared_memory_read` → native `i64` load lowering path.

### 3.3 Alternatives considered and rejected

- **Byte-swap inside `expand_shared_memory_read` on BE backends**
  (e.g. emit a `REV` / `bswap` after the `i64` load on BE). Rejected:
  this changes the documented semantic of `shared_memory_read` as a
  generic pointer-deref primitive and would silently mask any *other*
  caller's intent. It also adds per-backend emission complexity for no
  benefit over the typed-i32 approach.
- **Make the test endianness-aware** (e.g. `>> 32` on BE, `& mask` on
  LE). Rejected: VUMA test source has no portable compile-time
  endianness predicate, and branching in the test source would couple
  the test to backend internals. The test should express intent
  ("close `write_fd1`") not byte-order plumbing.
- **Change the test to read at offset 0 and shift.** Rejected: still
  endianness-dependent (the same packed-i64 bit-layout problem just
  moves to a different offset). Does not generalise.

### 3.4 Verification plan for F3-b

After applying the §3.1 fix, F3-b should re-run on all 6 failing
backends plus the LE baseline:

```bash
for be in aarch64_be mips64be ppc64 s390x m68k hppa x86_64 aarch64 riscv64; do
  target/release/compile_dump tests/gold_standard/ipc/half_closed_channel.vuma /tmp/hc_$be.bin $be
  qemu-${be}-static /tmp/hc_$be.bin; echo "$be exit=$?"
done
# Expected: every backend exits 0.
# Also re-run the negative companion (expects non-zero on all):
for be in aarch64_be mips64be ppc64 s390x m68k hppa x86_64; do
  target/release/compile_dump tests/gold_standard/ipc/half_closed_negative.vuma /tmp/hn_$be.bin $be
  qemu-${be}-static /tmp/hn_$be.bin; echo "$be neg_exit=$?"
done
# Expected: every backend exits non-zero (9 = EBADF).
```

And re-run `cargo test -p vuma-codegen --test ipc_handle_layout_test`
to confirm the IR-layout audit still passes (it must — the fix does
not touch `expand_channel_open` / `expand_channel_close`).

---

## 4. Files examined (READ-ONLY)

- `tests/gold_standard/ipc/half_closed_channel.vuma` — the failing test.
- `tests/gold_standard/ipc/half_closed_negative.vuma` — the negative
  companion (passes on BE; §1.3, §2.5).
- `src/codegen/src/ipc_lowering.rs` — `expand_channel_open` (1138-1322),
  `expand_channel_close` (1329+), `expand_shared_memory_read`
  (4308-4340), `expand_shared_memory_write` (4342-4365), `supervisor_call`
  (4369+) for the `syscall(nr, …)` path.
- `src/codegen/src/arm64.rs` — `IRInstr::Load` emission (4424-4486),
  `select_load` (3841) — confirms native-endian LDR, no byte-swap.
- `src/codegen/src/aarch64_be.rs` — confirms the BE wrapper only swaps
  ELF headers; data-load instructions are inherited unchanged from
  `arm64.rs` (lines 10-39).
- `tests/ipc_handle_layout_test.rs` — confirms the IR-level handle
  layout is {0,4,8,12} × I32 on every non-wasm32 backend; the bug is
  therefore NOT in the layout but in the test's packed-i64 masking.

No source files were edited. No `git push` invoked. No sub-agents
spawned.

---

## 5. Status

**PASS for F3-a (investigation).** Root cause identified, reproduced
locally on `aarch64_be` (exit 1) with LE baseline (exit 0) for
contrast, localised to the test's endianness-dependent
`packed & 0xFFFFFFFF` extraction in `half_closed_channel.vuma:43-45`,
and a minimal portable fix (add `shared_memory_read_i32` builtin +
adopt it in the test) is proposed for F3-b. DoD for F3-a: report
exists at this path with (a) reproduction steps + exit codes, (b) root
cause + code snippet, (c) proposed minimal fix — all three satisfied.
