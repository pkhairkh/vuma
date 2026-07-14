# Memory Allocation, Load, and Store

Tests exercising VUMA's `allocate` / `free` intrinsics and the `*ptr` /
`*(ptr + offset)` load/store syntax. Ranges from the canonical 4-operation
program (allocate / write / read / free) through arena-style region
allocators, ring buffers, stacks/queues, hash tables, and pool/arena
allocators. These are the core programs that exercise IVE's Liveness,
Exclusivity, Origin, and Cleanup invariants.

## What belongs here

- Single-cell allocate / store / load / free
- Multi-buffer allocations with independent lifetimes
- Byte-level access via `*(ptr + offset)` for u8/u32/u64 store/load
- Little-endian and big-endian byte decomposition / assembly
- Buffer operations: copy, fill, zero, reverse, swap, shift, compare
- Checksums: sum, XOR, Fletcher-32, Adler-32
- Ring buffers, stacks, queues (in-memory simulation)
- Bitmap operations
- Open-addressing hash table
- Null-terminated string operations (length, compare, copy)
- Pool allocator (free-list bitmap) and arena allocator (bump pointer)
- Double buffering, scatter / gather
- Memmove (overlapping copy)
- Nested allocation (allocate-in-callee pattern)
- Various allocation sizes (8, 16, 32, 64, 128, 256 bytes)

## Files (55)

### Original examples (5)

- [`arena_allocator.vuma`](arena_allocator.vuma) — region-style arena
- [`hello_memory.vuma`](hello_memory.vuma) — minimal alloc/store/load/free
- [`memory_arena.vuma`](memory_arena.vuma) — typed arena with nested scopes
- [`test_alloc.vuma`](test_alloc.vuma) — bare allocation
- [`test_store.vuma`](test_store.vuma) — bare byte store

### Wave 3-c: 50 gold-standard memory programs

Each file has a header comment documenting the expected exit code.
Programs use byte-level `*(ptr + offset)` access and always pair
`allocate` with `free`.

1. [`mem_alloc_free.vuma`](mem_alloc_free.vuma) — allocate and free
2. [`mem_store_load_u8.vuma`](mem_store_load_u8.vuma) — store/load u8
3. [`mem_store_load_u32.vuma`](mem_store_load_u32.vuma) — store/load u32 (LE)
4. [`mem_store_load_u64.vuma`](mem_store_load_u64.vuma) — store/load u64 (LE)
5. [`mem_store_array.vuma`](mem_store_array.vuma) — store 8-byte array
6. [`mem_load_array.vuma`](mem_load_array.vuma) — load 10-byte array, sum
7. [`mem_store_offset.vuma`](mem_store_offset.vuma) — store at computed offset
8. [`mem_load_offset.vuma`](mem_load_offset.vuma) — load from computed offset
9. [`mem_multi_alloc.vuma`](mem_multi_alloc.vuma) — 3 buffers, sum values
10. [`mem_alloc_loop.vuma`](mem_alloc_loop.vuma) — allocate in a loop
11. [`mem_fill_buffer.vuma`](mem_fill_buffer.vuma) — fill with 0xAB pattern
12. [`mem_copy_buffer.vuma`](mem_copy_buffer.vuma) — src→dst copy
13. [`mem_zero_buffer.vuma`](mem_zero_buffer.vuma) — zero-fill a buffer
14. [`mem_compare_buffers.vuma`](mem_compare_buffers.vuma) — compare two
15. [`mem_reverse_buffer.vuma`](mem_reverse_buffer.vuma) — reverse in place
16. [`mem_swap_buffers.vuma`](mem_swap_buffers.vuma) — swap via temp
17. [`mem_shift_buffer.vuma`](mem_shift_buffer.vuma) — shift left by 1
18. [`mem_checksum.vuma`](mem_checksum.vuma) — additive checksum
19. [`mem_xor_checksum.vuma`](mem_xor_checksum.vuma) — XOR checksum
20. [`mem_sum_checksum.vuma`](mem_sum_checksum.vuma) — sum modulo 256
21. [`mem_fletcher32.vuma`](mem_fletcher32.vuma) — Fletcher-32 (low byte)
22. [`mem_adler32.vuma`](mem_adler32.vuma) — Adler-32 (low byte of a)
23. [`mem_store_struct.vuma`](mem_store_struct.vuma) — store 3-field "struct"
24. [`mem_load_struct.vuma`](mem_load_struct.vuma) — load 2 fields, sum
25. [`mem_nested_alloc.vuma`](mem_nested_alloc.vuma) — alloc in main, pass to helper
26. [`mem_alloc_size_varies.vuma`](mem_alloc_size_varies.vuma) — 8/16/32/64/128/256
27. [`mem_store_overflow_check.vuma`](mem_store_overflow_check.vuma) — last valid byte
28. [`mem_byte_swap.vuma`](mem_byte_swap.vuma) — byte-swap u32
29. [`mem_le_store.vuma`](mem_le_store.vuma) — little-endian store
30. [`mem_le_load.vuma`](mem_le_load.vuma) — little-endian load
31. [`mem_be_store.vuma`](mem_be_store.vuma) — big-endian store
32. [`mem_be_load.vuma`](mem_be_load.vuma) — big-endian load
33. [`mem_memset_pattern.vuma`](mem_memset_pattern.vuma) — repeating [0xAA,0x55]
34. [`mem_memmove.vuma`](mem_memmove.vuma) — overlapping copy (high-to-low)
35. [`mem_buffer_to_u32.vuma`](mem_buffer_to_u32.vuma) — 4 bytes → u32 (BE)
36. [`mem_u32_to_buffer.vuma`](mem_u32_to_buffer.vuma) — u32 → 4 bytes (BE)
37. [`mem_ring_buffer_write.vuma`](mem_ring_buffer_write.vuma) — ring write
38. [`mem_ring_buffer_read.vuma`](mem_ring_buffer_read.vuma) — ring read
39. [`mem_stack_like.vuma`](mem_stack_like.vuma) — push/pop LIFO
40. [`mem_queue_like.vuma`](mem_queue_like.vuma) — enqueue/dequeue FIFO
41. [`mem_bitmap.vuma`](mem_bitmap.vuma) — set/clear bits in 1-byte bitmap
42. [`mem_hash_table.vuma`](mem_hash_table.vuma) — open addressing, linear probe
43. [`mem_string_length.vuma`](mem_string_length.vuma) — strlen("Hello")
44. [`mem_string_compare.vuma`](mem_string_compare.vuma) — strcmp equal
45. [`mem_string_copy.vuma`](mem_string_copy.vuma) — strcpy
46. [`mem_pool_alloc.vuma`](mem_pool_alloc.vuma) — free-list pool allocator
47. [`mem_arena_alloc.vuma`](mem_arena_alloc.vuma) — bump-pointer arena
48. [`mem_double_buffered.vuma`](mem_double_buffered.vuma) — front/back buffers
49. [`mem_scatter.vuma`](mem_scatter.vuma) — scatter to non-contiguous
50. [`mem_gather.vuma`](mem_gather.vuma) — gather from non-contiguous

## Verification

```bash
cd /tmp/my-project
for f in tests/gold_standard/memory/mem_*.vuma; do
    name=$(basename "$f" .vuma)
    ./target/release/compile_dump "$f" /tmp/${name}.bin x86_64 2>/dev/null
    chmod +x /tmp/${name}.bin
    result=$(timeout 3 /tmp/${name}.bin 2>/dev/null; echo $?)
    echo "$name: exit=$result"
done
```

Each `mem_*.vuma` file's header comment documents the expected exit code
(theoretically correct value, given the program logic). Discrepancies
between expected and actual exit codes on x86_64 (or other backends)
indicate VUMA codegen bugs — exactly what these gold-standard tests
are designed to surface.
