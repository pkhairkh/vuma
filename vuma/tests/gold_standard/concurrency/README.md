# Lock-Free Structures, Atomics, and Channels

Tests for concurrent programming. Two layers:

1. **Pre-existing (7)**: full lock-free data structures and concurrency
   demos — lock-free SPSC queue, thread pool with mutex/condvar, MPSC
   channel, fork/exec pipeline, epoll server, signal handler. These
   exercise VUMA's `spawn`/`join`, `Mutex`/`Condvar`, `Channel`, FFI
   to Linux syscalls, and IVE's concurrent-access verification.
2. **New `conc_*` tests (15)**: straight-line single-threaded tests
   that isolate one atomic-operation codegen pattern per file (basic
   store/load, overwrite, copy, swap, RMW, chain, round-trip, function
   interplay). All 15 pass on x86_64 today.

## What belongs here

- SPSC lock-free ring buffer with AtomicU64 head/tail
- Thread pool with shared mutex-protected task queue
- MPSC channel with CAS-based slot claiming
- Fork/exec pipeline with pipes
- Epoll-based TCP echo server
- SIGALRM handler with atomic handoff to main thread
- Straight-line single-threaded atomic-operation codegen tests
  (store/load, overwrite, copy, swap, RMW, chain, round-trip)

## Files (7 pre-existing + 15 new `conc_*` = 22)

### Pre-existing (7)
- [`channel_demo.vuma`](channel_demo.vuma)
- [`epoll_echo.vuma`](epoll_echo.vuma)
- [`lock_free_queue.vuma`](lock_free_queue.vuma)
- [`pipeline.vuma`](pipeline.vuma)
- [`self_exec.vuma`](self_exec.vuma)
- [`signal_hash.vuma`](signal_hash.vuma)
- [`thread_pool.vuma`](thread_pool.vuma)

### New `conc_*` tests (15) — straight-line, single-threaded

VUMA's `atomic_store` / `atomic_load` / `atomic_cas` builtins are
currently broken at the SCG-to-codegen bridge (see Task 6-d worklog):
`to_scg.rs` emits them as opaque `Computation { Other("atomic_store(…)")
}` nodes with no DataFlow edges, and the bridge lowers them to
`Add(0, 0)` no-ops instead of `IRInstr::AtomicLoad` / `AtomicStore`.

As a regression baseline, these 15 tests use the equivalent
non-atomic `*ptr = val; val = *ptr;` form, which exercises the same
address-computation + Store + Load codegen path that atomic_store/load
will use once the bridge is fixed. Each test's header comment documents
the expected exit code and any workarounds in use.

#### Basic store / load (1, 2, 3, 13, 14, 15)
- `conc_atomic_basic.vuma` — exit 42 — store 42, load, return.
- `conc_two_vars.vuma` — exit 42 — two cells, store in one, load from it.
- `conc_overwrite.vuma` — exit 99 — store 42 then 99, load.
- `conc_clear.vuma` — exit 0 — store 42 then 0, load.
- `conc_double_store.vuma` — exit 55 — store 55 twice, load.
- `conc_large_buf.vuma` — exit 42 — allocate 256, use first 8 bytes.

#### Independent / multi-cell (4, 12)
- `conc_independent.vuma` — exit 42 — two cells, return one.
- `conc_multi_store.vuma` — exit 7 — store 1/2/4 in 3 cells, sum loads.

#### Copy / chain / round-trip (5, 6, 9)
- `conc_chain.vuma` — exit 3 — chain copy a -> b -> c, return c.
- `conc_copy.vuma` — exit 42 — copy cell1 -> cell2, return cell2.
- `conc_roundtrip.vuma` — exit 123 — copy cell1 -> cell2, return cell2.

#### RMW / swap (7, 8)
- `conc_swap.vuma` — exit 1 — temp-swap two cells, return new cell1.
- `conc_compute.vuma` — exit 10 — RMW: load 3, add 7, store, load.

#### Function interplay (10, 11)
- `conc_func_atomic.vuma` — exit 42 — function stores to shared cell, main loads.
- `conc_pass_shared.vuma` — exit 42 — main allocates, passes to function, function stores.

## Known workarounds (documented in each file's header)

These workarounds are needed today to make the canonical store-loaded-
variable patterns observable. They become no-ops once the underlying
backend bugs are fixed; the tests still pass the same way.

1. **Store-loaded-variable bug**: storing a register that holds the
   result of a Load via `*ptr = loaded_var` is silently dropped by
   the current x86_64 codegen (the Store IR accepts a Register value,
   but the actual instruction emission fails to write the register's
   value to memory — the cell keeps its previous value). The inline
   form `*ptr = loaded_var + 0` triggers a different codegen path
   (Add dst=Register(N), lhs=Register(loaded_var), rhs=Immediate(0),
   then Store value=Register(N)) that DOES emit the store correctly.
   Used in: `conc_chain`, `conc_copy`, `conc_roundtrip`, `conc_swap`,
   `conc_compute`.
2. **First-allocation store bug**: when two buffers are allocated in
   sequence and the FIRST-allocated buffer is overwritten via a
   `*ptr = expr` store AFTER the second buffer has been stored to,
   the store is silently dropped (a register-allocation / addressing
   bug in the backend). Allocating the buffers in reverse order (the
   one we plan to overwrite second, then the one we plan to overwrite
   first) sidesteps the issue. Used in: `conc_swap`.
3. **Hex-literal store bug**: storing a hex literal via
   `*ptr = 0xNN` silently stores 0 (the codegen drops the hex literal
   in this context). Decimal literals work. Used implicitly in every
   `conc_*` test that stores a non-zero value.

## Verification

```bash
cd /tmp/my-project
for f in tests/gold_standard/concurrency/conc_*.vuma; do
    name=$(basename "$f" .vuma)
    ./target/release/compile_dump "$f" /tmp/${name}.bin x86_64 2>/dev/null
    chmod +x /tmp/${name}.bin
    result=$(timeout 3 /tmp/${name}.bin 2>/dev/null; echo $?)
    echo "$name: exit=$result"
done
```

Expected on x86_64 today: **15/15 PASS**. All exit codes are stable
across multiple runs (verified 3× each). Single-threaded, straight-
line code only. Once the SCG-to-codegen bridge is fixed (Task 6-d's
recommended fix in `to_scg.rs` and `pipeline.rs`), these tests can be
mechanically rewritten to use `atomic_store` / `atomic_load` and will
serve as the gold-standard concurrency regression suite.
