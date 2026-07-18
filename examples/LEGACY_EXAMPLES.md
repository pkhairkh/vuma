# Legacy Examples — PMT Porting Required

Of the 50 `.vuma` files in `examples/`, **14 compile under VUMA 2.0 (PMT-only)**
and **36 use legacy pointer syntax** (`allocate`, `free`, `*ptr`, `&x`) that is
now a hard parse error. These 36 cannot be added to the gold-standard suite
until they are ported to PMT syntax.

## PMT-compatible (14) — already in gold-standard or added

| Example | Gold-standard location |
|---------|----------------------|
| `epoll_echo.vuma` | `concurrency/epoll_echo.vuma` |
| `ffi_demo.vuma` | `edge_cases/ffi_demo.vuma` |
| `fibonacci.vuma` | `functions/fibonacci.vuma` |
| `fp_bench.vuma` | `float_advanced/fp_bench.vuma` (added 2026-07) |
| `hello_lang.vuma` | `edge_cases/hello_lang.vuma` (added 2026-07) |
| `minimal.vuma` | `arithmetic/minimal.vuma` |
| `test_call.vuma` | `arithmetic/test_call.vuma` |
| `test_exit.vuma` | `arithmetic/test_exit.vuma` |
| `test_loop.vuma` | `control_flow/test_loop.vuma` |
| `test_print.vuma` | `functions/test_print.vuma` |
| `test_print2.vuma` | `functions/test_print2.vuma` |
| `test_rotr.vuma` | `bitwise/test_rotr.vuma` |
| `test_sha_round.vuma` | `crypto_patterns/test_sha_round.vuma` |
| `test_u32_arith.vuma` | `u32_arith/test_u32_arith.vuma` |

## Legacy pointer syntax (36) — require PMT porting

These examples use `allocate`, `free`, `*ptr`, `&x` and other pointer syntax
that VUMA 2.0 rejects at the lexer level. To restore them as gold-standard
tests, each must be rewritten to use PMT constructs (`layout`, `State<T>`,
`state_new`, `transform`).

| Example | Lines | Feature | Porting effort |
|---------|------:|---------|---------------|
| `arena_allocator.vuma` | ~150 | Memory arena | Medium — layout + state_new |
| `atomics_demo.vuma` | ~214 | Atomic ops | Medium — layout + atomic fields |
| `base64_encode.vuma` | ~120 | Base64 | Medium — buffer as State<T> |
| `bsearch.vuma` | ~60 | Binary search | Easy — array field in layout |
| `channel_demo.vuma` | ~100 | Channels | Hard — ring buffer + sync |
| `crc32.vuma` | ~80 | CRC-32 | Easy — buffer as State<T> |
| `debug_info.vuma` | ~120 | DWARF debug | Medium — struct layout |
| `doubly_linked_list.vuma` | ~200 | Linked list | Hard — PMT has no pointers |
| `enum_demo.vuma` | ~80 | Enums | Easy — tagged union layout |
| `float_math.vuma` | ~60 | Float ops | Easy — already PMT-ish |
| `fp_vec_sum.vuma` | ~40 | Float vector | Easy — array field |
| `gpio_blink.vuma` | ~50 | GPIO | Easy — MMIO as State<T> |
| `hello2_lang.vuma` | ~20 | Self-host | Easy — copy of hello_lang |
| `hello_memory.vuma` | ~50 | Memory basics | Easy — layout + state_new |
| `hex_dump.vuma` | ~80 | Hex dump | Medium — buffer iteration |
| `linked_list.vuma` | ~150 | Singly linked | Hard — PMT has no pointers |
| `lock_free_queue.vuma` | ~200 | Lock-free queue | Hard — atomics + CAS |
| `matrix.vuma` | ~100 | Matrix mul | Easy — 2D array in layout |
| `memory_arena.vuma` | ~150 | Arena alloc | Medium — layout + state_new |
| `mmap_sha256d.vuma` | ~313 | mmap + SHA | Hard — syscall + crypto |
| `pipeline.vuma` | ~595 | fork/exec/pipe | Hard — syscalls + processes |
| `quicksort.vuma` | ~80 | Quicksort | Easy — array field |
| `self_exec.vuma` | ~615 | self-exec | Hard — fork/exec |
| `sha256d.vuma` | ~377 | SHA-256 | Medium — crypto on State<T> |
| `signal_hash.vuma` | ~364 | Signal + hash | Hard — signals + crypto |
| `sorted_map.vuma` | ~100 | Sorted map | Medium — BST without pointers |
| `spinlock.vuma` | ~60 | Spinlock | Medium — atomic CAS |
| `struct_demo.vuma` | ~80 | Structs | Easy — already layout-like |
| `syscall_32bit.vuma` | ~60 | Syscalls | Easy — extern decls |
| `test_alloc.vuma` | ~40 | Alloc test | Easy — layout + state_new |
| `test_endian.vuma` | ~40 | Endianness | Easy — byte manipulation |
| `test_hex.vuma` | ~30 | Hex output | Easy — byte formatting |
| `test_hex2.vuma` | ~30 | Hex output | Easy — byte formatting |
| `test_sha_manual.vuma` | ~80 | SHA manual | Medium — crypto |
| `test_store.vuma` | ~30 | Store test | Easy — layout + state_new |
| `test_u32_mem.vuma` | ~30 | u32 memory | Easy — layout + state_new |
| `test_w_sched.vuma` | ~40 | Scheduler test | Medium — task layout |
| `thread_pool.vuma` | ~200 | Thread pool | Hard — threads + sync |

## Porting guide

To port a legacy example to PMT:

1. Replace `allocate(T)` → `state_new(T)` (creates a `State<T>` offset)
2. Replace `*ptr` / `ptr.field` → `state.field` (typed field access)
3. Replace `free(ptr)` → nothing (PMT has no free; states are arena-managed)
4. Replace `&x` → nothing (PMT has no address-of; use the State<T> directly)
5. Replace pointer-based data structures (linked lists, trees) with array-based
   or layout-based equivalents (PMT has no pointer type)

The real implementations of crypto (SHA-256, AES, etc.) in `womb/crypto/`
and `womb/lib/` already use the legacy dialect and are KAT-verified. They
serve as reference implementations for porting.
