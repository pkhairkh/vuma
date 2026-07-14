# Atomic Operations and Memory Ordering

Tests for VUMA's atomic primitives: `atomic_load`, `atomic_store`,
`atomic_cas`, and the `Acquire` / `Release` / `Relaxed` / `SeqCst`
memory orderings. These lower to `LDXR`/`STXR` (AArch64),
`LOCK CMPXCHG` (x86_64), `LR.D`/`SC.D` (RISC-V), and equivalent atomic
sequences on the other backends — *when the SCG-to-codegen bridge
recognises them* (see Known Codegen Gap below).

## What belongs here

- `atomic_load(addr)` / `atomic_store(addr, val)` / `atomic_cas(addr, exp, des)`
- Implicit Release / Acquire ordering on the builtin store / load
- Atomic cells allocated via `allocate(8)` and used as `AtomicU64`
- Single-threaded correctness of the atomic operation codegen
- Cross-function atomic access (pass an atomic cell's address to a helper)

## Known Codegen Gap (as of wave 6-d)

The parser (`src/parser/src/parser.rs:2410-2453`) creates dedicated
`Expr::AtomicLoad` / `Expr::AtomicStore` / `Expr::AtomicCas` AST nodes
for the `atomic_load` / `atomic_store` / `atomic_cas` keywords. However,
`to_scg.rs` (`add_df_edges_recursive`, `src/parser/src/to_scg.rs:2425`)
has NO arm for these variants — they fall through to the catch-all `_`
branch, and `collect_uses` (`to_scg.rs:2569-2571`) returns nothing for
them. So the SCG represents every atomic operation as an opaque
`Computation { Other("atomic_store(...)") }` node with **NO DataFlow
edges** to its address or value arguments.

The SCG-to-codegen bridge (`bridge_scg_to_codegen` in
`src/pipeline.rs:2634`) does not pattern-match the `"atomic_store(...)"`
/ `"atomic_load(...)"` labels, so it lowers every atomic op to a
fallback `Add(0, 0)` no-op (visible in `dump_ir` output). Consequently:

- `atomic_load(addr)` always returns 0.
- `atomic_store(addr, val)` never writes.
- `atomic_cas(addr, exp, des)` always returns 0.

The codegen layer (`src/codegen/src/scg_to_ir.rs:2027-2113`) **already
knows how** to lower `CallNode { func: "AtomicLoad" | "AtomicStore" |
"AtomicCas" }` to proper `IRInstr::AtomicLoad` / `AtomicStore` /
`AtomicCas`, and every backend (x86_64, AArch64, RISC-V, ARM32, MIPS64,
PPC64, LoongArch64, Wasm32) implements those IR instructions natively.
The gap is purely in the bridge: it never produces those CallNodes from
the SCG. (The alternative `bridge_ast_to_codegen_scg` in `src/main.rs`
does handle atomic ops at lines 1241-1275, but `compile_dump` and the
test harness use `bridge_scg_to_codegen` from `pipeline.rs`, not the
AST bridge.)

**Minimal fix:** teach `to_scg.rs` to emit call-site `FunctionEntry` /
`FunctionReturn` nodes labelled `"call_AtomicLoad"` /
`"call_AtomicStore"` / `"call_AtomicCas"` for `Expr::AtomicLoad` /
`AtomicStore` / `AtomicCas` (mirroring how `Expr::Call` is handled in
`emit_call_nodes`), and ensure `add_df_edges_recursive` recurses into
the address/value sub-expressions. The bridge will then create
`CallNode`s with the right names, and `scg_to_ir.rs` will lower them to
proper atomic IR instructions.

Until that fix lands, the 30 tests below are gold-standard regression
tests: their expected exit codes document the *correct* behaviour, and
they will fail (returning 0 instead of the expected non-zero value)
until the bridge is fixed. Tests whose expected exit code is 0 pass
*accidentally* (because `atomic_load` happens to return 0).

## Files (32)

| # | File | Expected | Description | Status |
|---|------|---------:|-------------|--------|
| 0 | `atomics_demo.vuma` | — | reference example (uses `atomic_cas`, `fetch_add`) | existing |
| 0 | `spinlock.vuma` | — | reference example (spinlock via `atomic_cas`) | existing |
| 1 | `atom_store_load.vuma` | 42 | basic store then load | FAIL (returns 0) |
| 2 | `atom_store_zero.vuma` | 0 | store 0, load, return | PASS |
| 3 | `atom_store_max_u64.vuma` | 255 | store 255, load, return | FAIL (returns 0) |
| 4 | `atom_load_before_store.vuma` | 0 | load before any store | PASS |
| 5 | `atom_overwrite.vuma` | 99 | store 42 then 99 | FAIL (returns 0) |
| 6 | `atom_multiple_stores.vuma` | 3 | stores 1, 2, 3; last wins | FAIL (returns 0) |
| 7 | `atom_two_atomics.vuma` | 42 | two cells; store in first, load it | FAIL (returns 0) |
| 8 | `atom_copy.vuma` | 42 | copy atom1 to atom2 via load/store | FAIL (returns 0) |
| 9 | `atom_acquire_load.vuma` | 42 | Release-store / Acquire-load | FAIL (returns 0) |
| 10 | `atom_relaxed_load.vuma` | 42 | Relaxed store/load | FAIL (returns 0) |
| 11 | `atom_seqcst.vuma` | 42 | SeqCst store/load | FAIL (returns 0) |
| 12 | `atom_store_in_func.vuma` | 42 | helper stores; main loads | FAIL (returns 0) |
| 13 | `atom_load_in_func.vuma` | 42 | main stores; helper loads | FAIL (returns 0) |
| 14 | `atom_pass_atomic.vuma` | 42 | pass atomic addr to function | FAIL (returns 0) |
| 15 | `atom_compute.vuma` | 7 | store 3 and 4; return loaded sum | FAIL (returns 0) |
| 16 | `atom_xor.vuma` | 255 | store 0xFF, XOR with var, store, load | FAIL (returns 0) |
| 17 | `atom_add.vuma` | 10 | store 3, add 7 (var), store, load | FAIL (returns 0) |
| 18 | `atom_swap_pattern.vuma` | 42 | swap-by-copy between two atomics | FAIL (returns 0) |
| 19 | `atom_independent.vuma` | 42 | two independent atomics | FAIL (returns 0) |
| 20 | `atom_chain.vuma` | 3 | chain atom1 to atom2 to atom3 | FAIL (returns 0) |
| 21 | `atom_store_bool.vuma` | 1 | store 1 (true), load, return | FAIL (returns 0) |
| 22 | `atom_store_address.vuma` | 42 | store address-sized value, load | FAIL (returns 0) |
| 23 | `atom_multiple_fields.vuma` | 6 | three atomics holding 1, 2, 3; sum | FAIL (returns 0) |
| 24 | `atom_clear.vuma` | 0 | store 42 then 0 (clear), load | PASS |
| 25 | `atom_roundtrip.vuma` | 123 | store, load, store loaded, load | FAIL (returns 0) |
| 26 | `atom_large_buf.vuma` | 42 | allocate 256, use first 8 as atomic | FAIL (returns 0) |
| 27 | `atom_after_alloc.vuma` | 42 | allocate, immediately store, load | FAIL (returns 0) |
| 28 | `atom_before_free.vuma` | 42 | store, load, free, return loaded | FAIL (returns 0) |
| 29 | `atom_func_roundtrip.vuma` | 99 | main stores 99; func loads & returns | FAIL (returns 0) |
| 30 | `atom_double_store.vuma` | 55 | store 55 twice (idempotent), load | FAIL (returns 0) |

## Verification (x86_64)

```
PASS=3  FAIL=27  CRASH=0  (total 30 new tests)
```

All 30 tests compile cleanly and run without crashing. The 3 that pass
(`atom_store_zero`, `atom_load_before_store`, `atom_clear`) do so
because their expected exit code is 0, which is what `atomic_load`
happens to return under the bridge gap. The 27 failures all return 0
instead of the expected non-zero value. No source files outside
`tests/gold_standard/atomics/` were modified.
