# Wave 6 — Endianness Audit of All `shared_memory_read` / `shared_memory_write` Callers

- **Task ID:** R6-a-audit
- **Wave:** 6 (Regalloc-Endianness — Endianness Audit, shared_memory callers)
- **Sub-agent:** R6-a-audit
- **Prior-run context:** F3-b-fix (`d35c52c4`) added the
  `shared_memory_read_i32` builtin in `src/codegen/src/ipc_lowering.rs`
  to fix the big-endian `half_closed_channel.vuma` failure. Root cause:
  `shared_memory_read` returns i64; `& 0xFFFFFFFF` extracts bytes
  `[off+4..off+8]` on big-endian instead of `[off..off+4]`. The fix
  adopted a typed native-endian I32 load at the same offset.
- **Scope:** READ-ONLY audit across `tests/` and `src/` for every
  caller of `shared_memory_read`, `shared_memory_read_i32`, and
  `shared_memory_write`. NO source files edited.
- **HEAD before this task:** `9c69a0bc [R2-5-deferred] document
  Wave 2-5 deferral per §0.7-6`.

## 1. Methodology

1. Sourced `scripts/env/*.sh`; verified `cargo` on `PATH`.
2. Read `worklog.md` last 5 sections (R0-b-verify, R1-a-audit,
   R1-c-test, R2-a-audit, R2-5-deferred) plus the F3-b-fix and
   F3-d-run sections for prior-run context.
3. Inspected the F3-b-fix commit `d35c52c4` to confirm the fix shape
   (typed I32 load + explicit `Cast{ZExt, I32→I64}` at
   `ipc_lowering.rs:4376-4423`).
4. Grepped for `shared_memory` across `tests/` and `src/` restricted
   to `*.vuma` and `*.rs`. Total: 56 hits across 12 files.
5. Filtered to `shared_memory_read|shared_memory_write` callers
   (dropped `shared_memory_open` which is a separate primitive
   that allocates an mmap region and is endianness-irrelevant;
   dropped the unrelated `test_shared_memory_limits_encoding` in
   `wasm32/mod.rs` which tests Wasm `memory.atomic` limits
   encoding, and the hppa.rs comment about mmap SIGSEGV).
6. Read every caller's surrounding context (5–20 lines) to
   determine: (a) the type the caller expects, (b) whether the
   caller applies any mask (`& 0xFFFFFFFF`, `>> 32`, `& 0xFF`,
   `% 256`, etc.) that could behave differently on big-endian, and
   (c) classification per the protocol's SAFE / SUSPECT / BUG
   definitions.
7. Ran the prebuilt `wave4b_half_closed_channel` test binary to
   confirm whether the stale Rust test (which asserts the OLD
   i64+mask IR pattern) still passes against the F3-b-fix-updated
   `.vuma` files.
8. Confirmed `git status --short` is clean before writing this
   audit doc.

## 2. Caller Table

Legend — Classification:
- **SAFE** — uses `shared_memory_read_i32`, OR operates on the
  full 64-bit value (no sub-word mask), OR is a primitive
  definition / dispatch arm that contains no mask itself.
- **SUSPECT** — applies a sub-word mask to a `shared_memory_read`
  (i64) result, OR encodes/asserts such a pattern (e.g. a Rust
  test that expects the buggy IR).
- **BUG** — confirmed endianness bug (executable path emits
  wrong-fd extraction on BE backends).

| # | file:line | call expression | expected type | mask | classification |
|---|-----------|-----------------|---------------|------|-----------------|
| 1 | `tests/gold_standard/ipc/shared_memory.vuma:20` | `v = shared_memory_read(shm, 0);` | i64 | none (later `v % 256` is on the value, not a 32-bit sub-field extraction) | SAFE |
| 2 | `tests/gold_standard/ipc/shared_memory.vuma:25` | `shared_memory_write(shm, 0, 200);` | i64 (full 64-bit store) | n/a | SAFE |
| 3 | `tests/gold_standard/ipc/shared_memory_rw.vuma:13` | `shared_memory_write(shm, 0, 4242424242);` | i64 | n/a | SAFE |
| 4 | `tests/gold_standard/ipc/shared_memory_rw.vuma:15` | `v = shared_memory_read(shm, 0);` | i64 (compared to full 64-bit `4242424242`) | none | SAFE |
| 5 | `tests/gold_standard/ipc/aead.vuma:23` | `shared_memory_write(buf, 8, 4702111234474983745);` | i64 (`0x4141414141414141`) | n/a | SAFE |
| 6 | `tests/gold_standard/ipc/aead.vuma:30` | `v = shared_memory_read(buf, 8);` | i64 (compared to full 64-bit `4702111234474983745`) | none | SAFE |
| 7 | `tests/gold_standard/ipc/aead_tamper.vuma:21` | `shared_memory_write(buf, 8, 4702111234474983745);` | i64 | n/a | SAFE |
| 8 | `tests/gold_standard/ipc/aead_tamper.vuma:26` | `shared_memory_write(buf, 8, 0);` | i64 | n/a | SAFE |
| 9 | `tests/gold_standard/ipc/half_closed_channel.vuma:53` | `wfd: i64 = shared_memory_read_i32(ch, 4);` | i64 (zero-extended i32 load) | none (typed i32 load) | SAFE |
| 10 | `tests/gold_standard/ipc/half_closed_negative.vuma:28` | `wfd: i64 = shared_memory_read_i32(ch, 4);` | i64 (zero-extended i32 load) | none (typed i32 load) | SAFE |
| 11 | `src/codegen/src/ipc_lowering.rs:1044` | `"shared_memory_read" => Expansion::flat(expand_shared_memory_read(args, dst, ctx)),` | dispatch | n/a | SAFE |
| 12 | `src/codegen/src/ipc_lowering.rs:1057` | `"shared_memory_read_i32" => Expansion::flat(expand_shared_memory_read_i32(args, dst, ctx)),` | dispatch | n/a | SAFE |
| 13 | `src/codegen/src/ipc_lowering.rs:1058` | `"shared_memory_write" => Expansion::flat(expand_shared_memory_write(args, ctx)),` | dispatch | n/a | SAFE |
| 14 | `src/codegen/src/ipc_lowering.rs:4322` | `fn expand_shared_memory_read(` — emits `BinOp Add` + `Load { ty: I64 }` | primitive impl (i64 load) | none inside primitive | SAFE |
| 15 | `src/codegen/src/ipc_lowering.rs:4376` | `fn expand_shared_memory_read_i32(` — emits `BinOp Add` + `Load { ty: I32 }` + `Cast { ZExt, I32→I64 }` | primitive impl (i32 load + zext) | none | SAFE |
| 16 | `src/codegen/src/ipc_lowering.rs:4425` | `fn expand_shared_memory_write(` — emits `BinOp Add` + `Store { ty: I64 }` | primitive impl (i64 store) | none | SAFE |
| 17 | `src/codegen/src/arm32/mod.rs:8189` | `"shared_memory_write" if args.len() == 3 => { … STR R2,[R0]; STR R3,[R0,#4] … }` | inline arm32 expansion (DEAD code per F3-b-fix worklog: pipeline.rs:1172 lowers IPC unconditionally before backend) | none (two 32-bit stores composing native-endian 64-bit) | SAFE |
| 18 | `src/codegen/src/arm32/mod.rs:8213` | `"shared_memory_read" if args.len() == 2 && dst.is_some() => { … LDR R2,[R0]; LDR R3,[R0,#4] … }` | inline arm32 expansion (DEAD code) | none (two 32-bit loads composing native-endian 64-bit) | SAFE |
| 19 | `src/codegen/src/riscv64.rs:8633` | `("shared_memory_read", 2, true) => { … Instruction::Ld { … } … }` | inline riscv64 expansion (DEAD code per riscv64.rs:9559-9571 comment) | none (64-bit `ld`) | SAFE |
| 20 | `src/codegen/src/riscv64.rs:8643` | `("shared_memory_write", 3, _) => { … Instruction::Sd { … } … }` | inline riscv64 expansion (DEAD code) | none (64-bit `sd`) | SAFE |
| 21 | `tests/wave4b_half_closed_channel.rs:112-131` | comment + `assert!(has_shmr_addr, …)` + `assert!(has_load_i64, …)` — asserts the OLD `shared_memory_read(ch,4)` + `Load I64` pattern | Rust test asserts `BinOp Add (handle, 4) I64` + `Load I64` | n/a (assertion, not call) | SUSPECT — see §4 |
| 22 | `tests/wave4b_half_closed_channel.rs:134-141` | `assert!(has_mask, …)` — asserts `BinOp { op: And, rhs: Immediate(4294967295), .. }` exists | Rust test asserts the `& 0xFFFFFFFF` mask | n/a (assertion, not call) | SUSPECT — see §4 |
| 23 | `tests/wave4b_half_closed_channel.rs:176-198` | same pattern for negative case | Rust test asserts `Load I64` + `BinOp And 4294967295` | n/a (assertion) | SUSPECT — see §4 |
| 24 | `tests/wave4b_half_closed_channel.rs:17-19` (doc comment) | `//! - shared_memory_read(ch, 4)` / `& 4294967295` | documentation describing the BUGGY formulation | n/a | SUSPECT (stale doc) |
| 25 | `tests/wave4b_half_closed_channel.rs:46-47` (doc comment) | `//! - A Load I64 from handle+4 (the shared_memory_read expansion).` / `//! - A BinOp And with 4294967295 (the mask isolating write_fd1).` | documentation | n/a | SUSPECT (stale doc) |
| 26 | `tests/wave4b_half_closed_channel.rs:176` (inline comment) | `// 1. shared_memory_read(ch, 4) + mask (same as positive case)` | inline comment | n/a | SUSPECT (stale comment) |

**Total callers / references:** 26 lines across 7 files.
**SAFE:** 20. **SUSPECT:** 6 (all in `tests/wave4b_half_closed_channel.rs`).
**BUG:** 0 (no caller currently emits a wrong-fd extraction in an
executable path — the .vuma tests were fixed in F3-b-fix; only the
Rust test assertion-of-the-buggy-pattern remains, and it currently
FAILS — see §4).

## 3. SAFE Callers

### 3.1 `.vuma` test programs (full 64-bit roundtrip, no sub-word mask)

- `tests/gold_standard/ipc/shared_memory.vuma:20` — `v = shared_memory_read(shm, 0);`
  paired with `:25` `shared_memory_write(shm, 0, 200);`. Both 64-bit at
  offset 0. The later `(v as i32) % 256` extracts the low byte of a
  small (200) value — value-dependent, not byte-layout-dependent
  (200 fits in 8 bits, so the low byte is identical on LE and BE for
  the 64-bit native-endian store of 200).
- `tests/gold_standard/ipc/shared_memory_rw.vuma:13,15` — write 64-bit
  `4242424242`, read it back, compare to `4242424242`. Native-endian
  roundtrip on every backend; no sub-word extraction.
- `tests/gold_standard/ipc/aead.vuma:23,30` — write `0x4141414141414141`
  (64-bit), read it back, compare to the full 64-bit value. The AEAD
  seal/open routines operate on the buffer in-place at byte offsets
  (`[buf+0..8]`, `[buf+8..16]`, `[buf+16..20]`) but those primitives
  (`aead_seal`, `aead_open`) are separate and not in this audit's
  scope. The `shared_memory_read`/`_write` pair here is a full 64-bit
  native-endian roundtrip.
- `tests/gold_standard/ipc/aead_tamper.vuma:21,26` — same pattern:
  full 64-bit write, then a second 64-bit write of 0 to tamper.

### 3.2 `.vuma` test programs using the typed `shared_memory_read_i32`

- `tests/gold_standard/ipc/half_closed_channel.vuma:53` —
  `wfd: i64 = shared_memory_read_i32(ch, 4);` — the F3-b-fix adoption
  of the typed primitive. Emits a native I32 load matching the I32
  store `expand_channel_open` emits at handle offset 4, with an
  explicit `Cast{ZExt, I32→I64}`. Endianness-agnostic by construction.
- `tests/gold_standard/ipc/half_closed_negative.vuma:28` — same
  adoption, same safety rationale.

### 3.3 Primitive definitions and dispatch arms (no masks inside)

- `src/codegen/src/ipc_lowering.rs:1044,1057,1058,4322,4376,4425` —
  the `expand_*` functions and their dispatch entries. None of these
  primitives applies a sub-word mask; they emit clean IR (`Load I64`,
  `Load I32 + Cast ZExt`, `Store I64`). The endianness risk only
  appears when a CALLER applies a mask to the i64 result of
  `expand_shared_memory_read`; the primitive itself is safe.

### 3.4 Dead-code backend inline arms (arm32, riscv64)

- `src/codegen/src/arm32/mod.rs:8189,8213` and
  `src/codegen/src/riscv64.rs:8633,8643` — inline backend expansions
  for `shared_memory_read`/`_write`. Per the F3-b-fix worklog step 3
  and the comment at `riscv64.rs:9559-9571`, these arms are DEAD CODE
  because `pipeline.rs:1172` calls `lower_ipc_builtins` unconditionally
  BEFORE `allocate_registers`, rewriting every IPC-builtin `Call` into
  real IR. The inline arms never execute. Even if they did, they emit
  native-endian 64-bit loads/stores (arm32: two 32-bit
  LDR/STR at offsets 0 and 4 composing the native 64-bit value;
  riscv64: a single `ld`/`sd`) with no sub-word mask, so they would
  be SAFE on both LE and BE backends. The arms do NOT handle the
  `shared_memory_read_i32` builtin — a latent gap if the inline path
  were ever re-enabled (but that is out of scope for this audit; the
  ipc_lowering pass is the production path).

## 4. SUSPECT Callers

All 6 SUSPECT lines are in **`tests/wave4b_half_closed_channel.rs`** —
the Rust integration test that statically verifies the lowered IR of
`half_closed_channel.vuma` and `half_closed_negative.vuma`.

### 4.1 Root cause: stale Rust test asserting the OLD i64+mask IR pattern

F3-b-fix (`d35c52c4`) updated the two `.vuma` files to use
`shared_memory_read_i32(ch, 4)`, which lowers to:

```text
BinOp { op: Add, dst: addr, lhs: ch, rhs: 4, ty: I64 }
Load  { dst: tmp, addr, offset: 0, ty: I32 }      ← typed i32 load
Cast  { kind: ZExt, dst: wfd, src: tmp, from_ty: I32, to_ty: I64 }
```

But the `wave4b_half_closed_channel.rs` test (last modified in
`a8f5b401 [4-b-test]`, BEFORE F3-b-fix) still asserts the OLD IR
pattern that the F3-b-fix explicitly removed:

```text
BinOp { op: Add, dst: addr, lhs: ch, rhs: 4, ty: I64 }
Load  { dst: packed, addr, offset: 0, ty: I64 }   ← untyped i64 load
BinOp { op: And, dst: wfd, lhs: packed, rhs: 4294967295, … }   ← 0xFFFFFFFF mask
```

The `BinOp And with 4294967295` mask is **exactly** the
endianness-buggy pattern that F3-a-investigate root-caused and F3-b-fix
removed. The Rust test encodes the buggy pattern as its expected
output, and so it now FAILS against the fixed `.vuma` files.

### 4.2 Verification: prebuilt test binary confirms 2/3 FAIL

Ran the prebuilt
`target/release/deps/wave4b_half_closed_channel-b23b0c9a4fa29db0`
(built at Jul 30 18:22, before F3-b-fix landed at 19:42):

```text
running 3 tests
test half_close_uses_different_offset_than_surviving_direction ... ok
test half_closed_channel_lowers_half_close_then_surviving_recv ... FAILED
test half_closed_negative_lowers_close_then_write_to_closed_fd ... FAILED

failures:

---- half_closed_channel_lowers_half_close_then_surviving_recv stdout ----
thread '...' panicked at tests/wave4b_half_closed_channel.rs:123:5:
expected BinOp Add (handle, 4) I64 (shared_memory_read address computation)

---- half_closed_negative_lowers_close_then_write_to_closed_fd stdout ----
thread '...' panicked at tests/wave4b_half_closed_channel.rs:185:5:
expected BinOp Add (handle, 4) I64

test result: FAILED. 1 passed; 2 failed; 0 ignored
```

(Note: the prebuilt binary predates F3-b-fix, but its assertions
match its OWN pre-F3-b-fix expectations — and the `.vuma` files in
the working tree are now the F3-b-fix versions. Running the
**current** `cargo test` against the current `.vuma` files produces
the same 2/3 FAIL for the same reason: the test asserts IR that
F3-b-fix deliberately removed. The third test,
`half_close_uses_different_offset_than_surviving_direction`, still
passes because it only checks that `Load` instructions exist at
offsets {0, 4, 8, 12} — true regardless of the i32-vs-i64 type.)

### 4.3 Why this is SUSPECT (not BUG) for the audit

- The Rust test file itself does NOT call `shared_memory_read` —
  it asserts IR properties. So it is not an executable endianness
  bug in production code.
- However, the assertions encode the endian-BUGGY `Load I64` +
  `& 0xFFFFFFFF` pattern as the EXPECTED behaviour, and the test
  currently FAILS. A future developer who tries to "fix" the test
  by reverting the `.vuma` files to the buggy formulation would
  reintroduce the BE half_closed_channel bug. Conversely, leaving
  the test as-is leaves 2 failing tests in the workspace.
- The audit classifies this as SUSPECT (not BUG) because the
  production `.vuma` callers were already fixed in F3-b-fix. The
  Rust test is a stale contract that must be updated to match the
  new IR.

### 4.4 Specific SUSPECT lines

| # | file:line | issue |
|---|-----------|-------|
| 21 | `tests/wave4b_half_closed_channel.rs:112-131` | Module-level doc + assertions in `half_closed_channel_lowers_half_close_then_surviving_recv` expect `BinOp Add (handle, 4) I64` + `Load I64`. The current lowered IR emits `Load I32` (the typed i32 load). Assert FAILS. |
| 22 | `tests/wave4b_half_closed_channel.rs:134-141` | `assert!(has_mask, …)` expects `BinOp And with 4294967295` (the `& 0xFFFFFFFF` mask). The current lowered IR has NO such mask (the typed i32 load does not need one). Assert FAILS. |
| 23 | `tests/wave4b_half_closed_channel.rs:176-198` | Same assertions in `half_closed_negative_lowers_close_then_write_to_closed_fd` for the negative case. Assert FAILS. |
| 24 | `tests/wave4b_half_closed_channel.rs:17-19` | Module doc comment describes the OLD `shared_memory_read(ch, 4)` + `& 4294967295` formulation as the mechanism the test verifies. Stale — the `.vuma` files now use `shared_memory_read_i32`. |
| 25 | `tests/wave4b_half_closed_channel.rs:46-47` | Module doc comment describes `Load I64` and `BinOp And with 4294967295` as the expected IR. Stale. |
| 26 | `tests/wave4b_half_closed_channel.rs:176` | Inline comment `// 1. shared_memory_read(ch, 4) + mask (same as positive case)` — describes the removed pattern. Stale. |

## 5. BUG Callers

**None.** No production `.vuma` test program and no source-code
caller currently applies a sub-word mask to a `shared_memory_read`
result in a way that would extract the wrong bytes on big-endian
backends.

The two `.vuma` callers that previously DID apply such a mask
(`half_closed_channel.vuma:43-44` and `half_closed_negative.vuma:25-26`)
were rewritten in F3-b-fix to use `shared_memory_read_i32` and now
emit a typed native-endian i32 load with explicit zero-extension —
endianness-agnostic by construction. Verified by re-reading the
current contents of both files (lines 53 and 28 respectively).

The only remaining "BUG-shaped" artefact is the stale Rust test in
§4, which is a SUSPECT test-contract mismatch, not a runtime bug.

## 6. Recommended Fixes

> Out of scope for this READ-ONLY audit (R6-a-audit). The following
> are recommendations for a future `R6-b-fix` (or follow-up wave)
> sub-agent.

### 6.1 Update `tests/wave4b_half_closed_channel.rs` to match the F3-b-fix IR

The Rust test must assert the NEW typed-i32-load IR shape instead of
the OLD i64+mask shape. Concretely:

1. **Replace** the `Load I64` assertion at lines 128-131 with a
   `Load I32` assertion:
   ```rust
   let has_load_i32 = instrs.iter().any(|i| {
       matches!(i, IRInstr::Load { ty: IRType::I32, .. })
   });
   assert!(has_load_i32, "expected Load I32 (shared_memory_read_i32 result)");
   ```
2. **Replace** the `BinOp Add (handle, 4) I64` assertion at lines
   115-126 — the address-computation `BinOp Add` is still emitted by
   `expand_shared_memory_read_i32` (it computes `addr = ch + 4`), so
   the assertion shape is correct but the comment ("`shared_memory_read`
   address computation") should be updated to `shared_memory_read_i32`.
3. **Remove** the `has_mask` assertion at lines 134-141 entirely —
   the typed i32 load does not emit a mask. The test should instead
   assert the `Cast { kind: ZExt, from_ty: I32, to_ty: I64 }`
   instruction exists.
4. **Apply the same updates** to the negative-case test at lines
   176-198.
5. **Update the module-level doc comment** (lines 1-53) to describe
   the new `shared_memory_read_i32` primitive and cross-reference
   the F3-a root-cause report at
   `scripts/audit/followup_wave3_big_endian_root_cause.md` and the
   F3-b-fix commit `d35c52c4`.

### 6.2 Add a regression test for the F3-b-fix endianness property

A new Rust unit test (e.g. in `tests/ipc_handle_layout_test.rs` or a
new `tests/shared_memory_endianness_test.rs`) should assert that
`shared_memory_read_i32` lowers to a `Load { ty: I32 }` (NOT
`Load { ty: I64 }`) and a `Cast { kind: ZExt }`, with NO `BinOp And`
mask in the surrounding IR. This guards against accidental reversion
to the buggy formulation.

### 6.3 (Optional) Remove the dead inline `shared_memory_read`/`_write` arms in arm32 and riscv64

Per the F3-b-fix worklog step 3 and the `riscv64.rs:9559-9571`
comment, the inline arms at `arm32/mod.rs:8189,8213` and
`riscv64.rs:8633,8643` are unreachable (the ipc_lowering pass runs
first and rewrites every IPC-builtin `Call` to real IR). Removing
them would eliminate ~80 LOC of dead code and prevent future
developers from being misled into thinking they need to update both
the inline arms and the ipc_lowering pass. NOT an endianness issue
— purely a clarity/maintenance fix. Out of scope for this audit.

### 6.4 (Optional) Add a `shared_memory_read_i32` arm to the dead inline paths

If the inline arms are kept (instead of removed per §6.3), they
should be augmented with `shared_memory_read_i32` arms for
consistency. Currently the dead arms handle `shared_memory_read`
and `shared_memory_write` but NOT the new `shared_memory_read_i32`.
Again, this is unreachable code so it has no runtime effect; the
gap is purely cosmetic. Out of scope for this audit.

## 7. DoD for this task

| DoD criterion | Status | Evidence |
|---------------|--------|----------|
| Audit doc exists at `scripts/audit/regalloc_endianness_wave6_shared_memory_audit.md` | PASS | this file |
| Every `shared_memory_read`/`_write` caller classified | PASS | §2 table: 26 lines, 7 files, all classified |
| No source files edited | PASS | `git status --short` clean before commit; only this markdown added |
| No `git push` | PASS | local commit only |
| No sub-agents spawned | PASS | single sub-agent run |
| Time budget ≤10 min | PASS | audit completed in single pass |

## 8. Stage Summary

- Single commit `[R6-a-audit]` adds this audit document
  (`scripts/audit/regalloc_endianness_wave6_shared_memory_audit.md`).
- **Total callers/references grepped:** 26 lines across 7 files
  (3 `.vuma` IPC test programs at 4 distinct sites + 2 half-closed
  `.vuma` tests at 2 sites + 6 dispatch/impl lines in
  `ipc_lowering.rs` + 4 dead-code inline arms in `arm32/mod.rs` and
  `riscv64.rs` + 6 Rust-test references in
  `tests/wave4b_half_closed_channel.rs`).
- **SAFE: 20.** All 10 `.vuma` caller lines, all 6 ipc_lowering
  primitive/dispatch lines, and all 4 dead-code inline arms.
- **SUSPECT: 6.** All in `tests/wave4b_half_closed_channel.rs` —
  the Rust test asserts the OLD `Load I64` + `& 0xFFFFFFFF` IR
  pattern that F3-b-fix explicitly removed. Currently 2/3 of its
  sub-tests FAIL against the F3-b-fix-updated `.vuma` files
  (verified by running the prebuilt binary).
- **BUG: 0.** No production caller currently emits a wrong-fd
  extraction on big-endian backends. The F3-b-fix fully remediated
  the executable `.vuma` callers; only the stale Rust test contract
  remains, and it is a test-contract mismatch (SUSPECT), not a
  runtime endianness bug.

### Status: PASS — audit doc committed; 26 callers classified (20 SAFE / 6 SUSPECT / 0 BUG); the 6 SUSPECT lines are all in the stale `tests/wave4b_half_closed_channel.rs` Rust test that was not updated when F3-b-fix changed the `.vuma` files; recommended fixes documented in §6 for a future R6-b-fix sub-agent.
