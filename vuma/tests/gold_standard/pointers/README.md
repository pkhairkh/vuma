# Pointer Arithmetic and Dereference

Tests focused on `Address`-typed values, pointer arithmetic, and `*ptr`
dereference. These programs stress the backend's address-computation
instruction selection and the IVE bounds-check on derived pointers.

## What belongs here

- `*buf` dereference (offset 0) for all single-byte memory accesses
- Multi-buffer allocations with independent lifetimes
- Pointer parameter passing (Address-typed function args / return values)
- Cross-function buffer mutation
- Pointer chains and double / triple dereference (gold-standard SPEC
  tests for the not-yet-implemented AddressOf codegen path)

## Files (33 = 3 pre-existing + 30 new `ptr_*`)

### Pre-existing (3)
- `hex_dump.vuma`
- `test_hex.vuma`
- `test_hex2.vuma`

### New `ptr_*` tests (30) — straight-line, `*buf` (offset 0) only

Each `ptr_*` file documents its expected exit code in a header comment.
All are written as semantically correct VUMA programs using only
straight-line code (no `if`/`else`, no `for`/`while`) and `*buf`
(offset 0) for every memory access, per the test-harness constraints
that avoid the known VUMA codegen bugs around `*(buf + computed_offset)`,
control-flow body assignments, and immediate-RHS bitwise ops.

#### Basic store / load (1-4)
- `ptr_basic_store.vuma` — exit 42 — allocate / store 42 / load / free / return.
- `ptr_store_load_u8.vuma` — exit 255 — store/load the u8 maximum.
- `ptr_store_load_u32.vuma` — exit 255 — store/load the low byte of a u32.
- `ptr_multi_buf.vuma` — exit 42 — two buffers; store in one, load from it.

#### Buffer operations (5-6)
- `ptr_buf_swap.vuma` — exit 1 — swap two buffers' contents via a temp.
- `ptr_buf_copy.vuma` — exit 42 — copy a single value between buffers.

#### Function + pointer interaction (7-8, 19-20, 28)
- `ptr_pass_to_func.vuma` — exit 42 — pass buffer to helper; helper stores; main loads.
- `ptr_func_returns_ptr.vuma` — exit 42 — helper allocates, stores, returns Address; main loads.
- `ptr_store_return.vuma` — exit 55 — store value; helper loads and returns it.
- `ptr_func_modify.vuma` — exit 99 — helper stores 99 in caller's buffer; main loads.
- `ptr_store_in_func.vuma` — exit 42 — function-local allocate/store/load/free.

#### Multi-level indirection (9, 27, 29) — gold-standard SPEC tests
These exercise the AddressOf + double/triple deref codegen path, which
is **not yet implemented** on x86_64 today (no `AddressOf` lowering in
`src/codegen/src/x86_64/mod.rs`; the `Store` IR hardcodes U8 width,
truncating 64-bit addresses). They will fail / crash on x86_64 today
and turn green once the codegen lands.
- `ptr_double_deref.vuma` — exit 42 — `**buf1` reads buf2's stored value.
- `ptr_pointer_to_pointer.vuma` — exit 42 — `*buf1 = buf2; **buf1 = 42; return **buf1`.
- `ptr_multi_level.vuma` — exit 3 — 3-buffer chain; `***buf1` reads the tail's value.

#### Offset / multi-value patterns (10-12)
- `ptr_ptr_arith.vuma` — exit 10 — offset variable in scope alongside `*buf`.
- `ptr_store_multiple.vuma` — exit 3 — store 3 values (one per buffer), sum.
- `ptr_store_struct.vuma` — exit 6 — store 3 "struct fields" (one per buffer), sum.

#### Overwrite / clear / independence (13-15)
- `ptr_overwrite.vuma` — exit 99 — store 42 then 99; load.
- `ptr_clear.vuma` — exit 0 — store 42 then 0; load.
- `ptr_independent.vuma` — exit 42 — two buffers; verify independence.

#### Chained / swap idioms (16-18)
- `ptr_chain_store.vuma` — exit 7 — chain value through 3 buffers.
- `ptr_xor_exchange.vuma` — exit 1 — XOR-swap two buffer values (no temp).
- `ptr_temp_swap.vuma` — exit 1 — temp-variable swap two buffer values.

#### Allocation / lifecycle (21-23)
- `ptr_alloc_sizes.vuma` — exit 42 — allocate 8/16/32/64-byte buffers; load the 64-byte one.
- `ptr_reuse_after_free.vuma` — exit 42 — allocate, free, reallocate, use.
- `ptr_store_zero.vuma` — exit 0 — store and load zero.

#### Boundary / arithmetic (24-25, 30)
- `ptr_store_max_u8.vuma` — exit 255 — store/load 255 (high bit set).
- `ptr_store_add.vuma` — exit 10 — store 3 and 7 in two buffers; sum.
- `ptr_round_trip.vuma` — exit 123 — double round-trip through two buffers.

#### Conditional store without `if` (26)
- `ptr_conditional_store.vuma` — exit 42 — bool variable + arithmetic select.

## Verification

```bash
cd /tmp/my-project
for f in tests/gold_standard/pointers/ptr_*.vuma; do
    name=$(basename "$f" .vuma)
    ./target/release/compile_dump "$f" /tmp/${name}.bin x86_64 2>/dev/null
    chmod +x /tmp/${name}.bin
    result=$(timeout 3 /tmp/${name}.bin 2>/dev/null; echo $?)
    echo "$name: exit=$result"
done
```

Expected on x86_64 today: 27/30 PASS, 1 FAIL (`ptr_double_deref`,
returns 240 instead of 42), 2 CRASH (`ptr_multi_level`,
`ptr_pointer_to_pointer`, segfault 139). The 3 failures are all on the
not-yet-implemented AddressOf + multi-level deref codegen path; they
will turn green once `src/codegen/src/x86_64/mod.rs` gains an
`AddressOf` lowering and the `Store` IR supports wider-than-U8 stores.
