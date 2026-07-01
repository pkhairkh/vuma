# VUMA Examples

48 VUMA example programs demonstrating language features.

## What is VUMA?

VUMA is a systems programming language with compile-time memory safety verification. The compiler can verify five invariants (Liveness, Exclusivity, Interpretation, Origin, Cleanup) but currently has false positives on some valid programs. Most examples are compiled with `--verification none`.

**Note:** VUMA does have an `unsafe` keyword (for explicitly unverifiable code blocks). The `map_device()` and `volatile` features referenced in some example comments are aspirational — they are not implemented as language keywords.

## Example Index

| Example | Lines | What It Demonstrates |
|---------|-------|---------------------|
| `hello_memory.vuma` | 40 | Basic allocate/write/read/free |
| `doubly_linked_list.vuma` | 89 | Sentinel node pattern, pointer field access |
| `arena_allocator.vuma` | 78 | Arena allocation with region semantics |
| `gpio_blink.vuma` | 68 | Hardware register access (comment-only, not a language feature) |
| `lock_free_queue.vuma` | 91 | Lock-free SPSC queue with atomics |
| `channel_demo.vuma` | 237 | Channel-based concurrency |
| `fibonacci.vuma` | 74 | Recursive and iterative Fibonacci |
| `quicksort.vuma` | 111 | Quicksort with in-place swapping |
| `crc32.vuma` | 119 | CRC32 checksum |
| `base64_encode.vuma` | 177 | Base64 encoding |
| `enum_demo.vuma` | 114 | Enum (tagged union) types |
| `struct_demo.vuma` | 72 | Struct types with field access |
| `ffi_demo.vuma` | 32 | FFI and syscall usage |

All examples compile with `--verification none`. Some may fail `--verification normal` due to IVE false positives.

## Compiling Examples

```bash
# Compile to x86_64
vuma emit x86_64 examples/hello_memory.vuma -o hello.bin

# Compile to AArch64
vuma emit aarch64 examples/hello_memory.vuma -o hello.aarch64

# Compile to Wasm32
vuma emit wasm32 examples/hello_memory.vuma -o hello.wasm
```
