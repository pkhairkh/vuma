# VUMA Examples

This directory contains example programs demonstrating VUMA's language features and the IVE (Invariant Verification Engine) safety guarantees.

## What is VUMA?

VUMA is a memory-safe systems programming language where the compiler **proves** memory safety at compile time through its IVE (Invariant Verification Engine). VUMA verifies five invariants for every pointer operation:

| Invariant        | What it proves                                      |
|------------------|-----------------------------------------------------|
| **Liveness**     | The pointer refers to allocated, unfreed memory     |
| **Exclusivity**  | No two pointers can mutate the same bytes at once   |
| **Interpretation**| The pointer's type matches the stored type         |
| **Origin**       | Every loaded value came from a valid write          |
| **Cleanup**      | All allocations are eventually freed                |

There is **no `unsafe` keyword** in VUMA. All code is verified, always.

## Example Index

### 1. [hello_memory.vuma](hello_memory.vuma) — The simplest VUMA program

The "Hello World" of VUMA. Allocates a single integer, writes 42, reads it back, and frees the memory. Demonstrates the four fundamental operations: `allocate`, write (`*`), read (`*`), and `free`.

**Features demonstrated:** `allocate`, `free`, pointer dereference, basic IVE verification

---

### 2. [doubly_linked_list.vuma](doubly_linked_list.vuma) — VUMA's showcase

A doubly-linked list with sentinel node. This is the canonical example that **requires `unsafe` in Rust** but is fully verified in VUMA. The IVE proves that concurrent reads and writes to different fields of different nodes never overlap.

**Features demonstrated:** `struct`, `Address` type, pointer field access (`(*ptr).field`), circular data structures, IVE overlap analysis

---

### 3. [arena_allocator.vuma](arena_allocator.vuma) — Arena pattern

Implements a bump allocator (arena) where all sub-allocations are freed at once. VUMA's IVE tracks pointer derivation chains and automatically invalidates all derived pointers after `arena_destroy()`. This prevents the #1 bug in arena-based code.

**Features demonstrated:** Pointer arithmetic (`base + offset`), `.align_to()` for alignment, intra-region tracking, bulk invalidation, struct methods

---

### 4. [gpio_blink.vuma](gpio_blink.vuma) — Hardware access

Blinks an LED by directly accessing GPIO registers. Uses `map_device()` to map hardware addresses into the program's address space. IVE verifies all register accesses stay within the mapped region.

**Features demonstrated:** `map_device()`, hardware register access, `const` addresses, volatile semantics, embedded/bare-metal programming

---

### 5. [lock_free_queue.vuma](lock_free_queue.vuma) — Lock-free concurrency

A single-producer single-consumer (SPSC) ring buffer with atomic head/tail indices. IVE extends its verification to concurrent code, proving absence of data races, correct memory ordering, and no torn reads.

**Features demonstrated:** `AtomicU64`, `Acquire`/`Release` ordering, generic structs (`Queue<T>`), concurrent access verification, `Option<T>` return type

---

### 6. [sorted_map.vuma](sorted_map.vuma) — AVL-balanced tree map

A sorted key-value map backed by an AVL tree with height-balanced rotations. Tree rotations create temporary cycles in parent pointers — exactly the pattern that defeats Rust's borrow checker. VUMA's IVE proves each rotation is safe by tracking which pointers alias which bytes, verifying non-overlapping field writes within a node.

**Features demonstrated:** AVL tree rotations, parent pointer cycles, `if`/`else` branching, in-order traversal, recursive struct operations

---

### 7. [thread_pool.vuma](thread_pool.vuma) — Thread pool with verified synchronization

A fixed-size thread pool with a shared mutex-protected task queue and condvar signaling. Demonstrates the full concurrency lifecycle: thread creation via `spawn()`, mutex-locked queue access, condvar wait/signal, and clean shutdown with `join()`. IVE verifies no data races on the shared queue, no deadlock (single lock ordering), and no leaked threads (Cleanup invariant).

**Features demonstrated:** `Mutex<T>`, `Condvar`, `spawn()`/`join()`, `AtomicU64`, lock ordering verification, task queue pattern

---

### 8. [memory_arena.vuma](memory_arena.vuma) — Typed arena with nested scopes

An advanced arena allocator that extends the basic bump allocator with type-aware allocation, nested scopes with independent rollback, and O(1) reset. Nested scopes enable partial deallocation for speculative parsing or per-iteration cleanup. IVE tracks derivation chains across scope boundaries and proves all pointers are invalidated after reset — catching entire categories of use-after-reset bugs.

**Features demonstrated:** Type-safe allocation, nested scope push/pop, O(1) `arena_reset()`, cross-scope derivation tracking, `ArenaScope` struct, `TypedArena` with alignment

---

### 9. [channel_demo.vuma](channel_demo.vuma) — MPSC channel demonstration

A bounded multi-producer single-consumer (MPSC) channel with sender cloning, CAS-based slot claiming, and backpressure. Two producer threads send messages concurrently while a single consumer receives them. IVE verifies no data races between concurrent senders (CAS ensures exclusive slot ownership), no message loss (every slot is read exactly once), and complete cleanup when all senders drop and the receiver closes.

**Features demonstrated:** `Channel<T>`, sender cloning, `compare_exchange` CAS, `fetch_add`/`fetch_sub`, `Option<T>`, multi-producer concurrency, channel lifecycle

---

### 10. [fibonacci.vuma](fibonacci.vuma) — Recursive and iterative Fibonacci

Computes Fibonacci numbers using both recursive and iterative approaches, then verifies they agree on fib(10)=55. Returns fib(30)=832040 computed iteratively. The recursive version is included for correctness verification but would be too slow for large n, illustrating the importance of algorithmic choice.

**Features demonstrated:** Recursive functions, iterative loops with accumulator pattern, u32 overflow masking, result verification, pure computation

---

### 11. [quicksort.vuma](quicksort.vuma) — In-place quicksort

Implements the classic quicksort algorithm using the Lomuto partition scheme with in-place swapping. Array elements are stored as u64 with 8-byte stride in allocated memory. Returns the median of a 10-element test array.

**Features demonstrated:** Pointer arithmetic for array indexing, in-place mutation, recursive partition-and-sort, `arr_read`/`arr_write`/`arr_swap` helper pattern, allocate/free

---

### 12. [linked_list.vuma](linked_list.vuma) — Singly-linked list

A singly-linked list with head-only insertion (prepend), length computation, sum traversal, and iterative free. Simpler than the doubly-linked list showcase, this example focuses on the fundamental cons-cell pattern that underpins all linked data structures.

**Features demonstrated:** `struct` with `Address` field, null pointer sentinel, prepend/cons pattern, traversal with `while` loop, iterative cleanup, IVE liveness and cleanup verification

---

### 13. [hex_dump.vuma](hex_dump.vuma) — Hex dump utility

Reads bytes from memory and converts each to its two-digit hexadecimal ASCII representation. Implements the nybble-to-hex lookup pattern used in debuggers, hex editors, and network analyzers. Returns an XOR checksum of the hex output.

**Features demonstrated:** Byte-level pointer arithmetic, nybble extraction with shifts and masks, conditional character mapping (0-9 vs A-F), double-width output buffer, pure computation with checksum

---

### 14. [crc32.vuma](crc32.vuma) — CRC32 checksum

Implements the standard CRC32 algorithm (IEEE 802.3 / ITU-T V.42) using a 256-entry lookup table with polynomial 0xEDB88320. Computes the CRC32 of "123456789" and verifies against the known check value 0xCBF43926. Returns the low byte (0x26 = 38) as exit code.

**Features demonstrated:** Table-driven computation, nested loops (table generation: 256×8), little-endian u32 read/write, bitwise XOR/shift operations, u32 masking, standard algorithm verification

---

### 15. [bsearch.vuma](bsearch.vuma) — Binary search

Classic binary search on a sorted array of u64 values stored in allocated memory. Demonstrates O(log n) search with three-way comparison (less, equal, greater). Returns the index of the target value (7 for target=42) or a sentinel if not found.

**Features demonstrated:** Sorted array access with pointer arithmetic, while-loop with midpoint computation, three-way branching, sentinel return value, allocate/free

---

### 16. [matrix.vuma](matrix.vuma) — 4×4 matrix multiplication

Multiplies two 4×4 matrices of u32 values using the O(n³) triple-loop algorithm. Matrices are stored in row-major order with 4-byte stride. Demonstrates that multiplying by the identity matrix returns the original matrix (XOR checksum of 1..16 = 0).

**Features demonstrated:** 2D data layout via 1D memory, triple-nested while loops, row-major index computation, u32 arithmetic with overflow masking, allocate/free for multiple buffers

---

### 17. [base64_encode.vuma](base64_encode.vuma) — Base64 encoding

Implements RFC 4648 Base64 encoding, converting every 3 input bytes into 4 Base64 characters with proper padding. Encodes "Hello, World!" (13 bytes) → "SGVsbG8sIFdvcmxkIQ==" (20 bytes). Returns the output length as exit code.

**Features demonstrated:** 6-bit group extraction from 8-bit bytes, alphabet lookup function, padding logic for non-multiple-of-3 input, multi-return value encoding (length + checksum packed into u64), pointer arithmetic for input/output buffers

---

### 18. [atomics_demo.vuma](atomics_demo.vuma) — Atomic CAS, load, store, and fetch operations

Demonstrates VUMA's full suite of atomic memory operations: `atomic_load`/`atomic_store` with Acquire/Release semantics, `atomic_cas` compare-and-swap, `AtomicU64` with `fetch_add`/`fetch_sub`, and `compare_exchange` with explicit success/failure orderings. Four self-contained demos each exercise a different atomic primitive and return a verifiable checksum.

**Features demonstrated:** `atomic_load`, `atomic_store`, `atomic_cas`, `AtomicU64`, `fetch_add`/`fetch_sub`, `compare_exchange`, Acquire/Release/Relaxed orderings, IVE concurrent access verification

---

### 19. [float_math.vuma](float_math.vuma) — FP conversion and math operations

Showcases VUMA's floating-point capabilities across six demos: integer-to-float conversion (`inttofloat`, `uinttofloat`), float-to-integer conversion (`floattoint`, `floattouint`), float-to-float widening/narrowing (`floattofloat`), arithmetic on `f32`/`f64`, mixed integer/float computation with explicit conversion, and storing/loading floats from allocated memory. The codegen emits proper ISA-specific FP instructions on all 8 backends — not no-op bit reinterpretation.

**Features demonstrated:** `f32`, `f64`, `inttofloat`, `uinttofloat`, `floattoint`, `floattouint`, `floattofloat`, FP arithmetic (+, -, *, /), FP memory store/load, IVE Interpretation invariant for FP types

---

### 20. [debug_info.vuma](debug_info.vuma) — DWARF debug info demonstration

A simple multi-function program designed to exercise VUMA's `--debug` flag. Compiling with `--debug` produces a DWARF v4 ELF with `.debug_abbrev`, `.debug_info`, `.debug_line`, and `.debug_frame` sections. The program includes helper functions, control flow, and struct operations so that the emitted DWARF contains subprogram DIEs, variable DIEs with location expressions, line-number entries for branches, and FDE entries for stack unwinding.

**Features demonstrated:** `--debug` flag, DWARF v4 generation, `.debug_info`/`.debug_line`/`.debug_frame`/`.debug_abbrev` sections, CIE/FDE per function, line-number mapping, variable DIEs, multi-backend debug support

---

## FFI Example

### [ffi_demo.vuma](ffi_demo.vuma) — Calling C library functions from VUMA

This program demonstrates VUMA's C FFI system end-to-end. The `extern "C" { ... }` block declares external C functions with their calling convention and signatures. VUMA code calls these functions like any other function, and the compiler emits relocations for the external symbols instead of local `BL` instructions. The system linker resolves them at link time.

**Features demonstrated:** `extern "C"` block syntax, C calling convention, external symbol relocations, `write`/`read`/`exit` bindings, `SHN_UNDEF` ELF symbols, `.rela.text` relocations

**Compile and link:**
```bash
# Compile as a relocatable object
vuma compile --format obj --target aarch64 ffi_demo.vuma -o ffi_demo.o

# Link with libc
ld -o ffi_demo ffi_demo.o -lc
```

**FFI architecture:**

VUMA's FFI module (`src/ffi.rs`) handles:

| Component | Description |
|-----------|-------------|
| `ExternBlock` | AST node for `extern "C" { ... }` declarations |
| `ExternFn` | Individual function signature with param/return types |
| `ExternRegistry` | Lookup table for extern functions, calling conventions, and relocation needs |
| `CallingConvention` | C, System, and platform-specific conventions |
| Default bindings | Linux syscalls (`write`, `read`, `exit`, `mmap`, `munmap`, `brk`) + libc (`memcpy`, `memset`, `malloc`, `free`) |

The FFI pipeline:

1. **Parse** — `extern "C" { fn write(...); }` produces an `ExternBlockDef` AST node
2. **SCG** — The Static Call Graph gets a Phantom node for each extern block
3. **Registry** — `ExternBlock` → `ExternRegistry` mapping with convention + relocation metadata
4. **Codegen** — Extern calls emit `SHN_UNDEF` symbols and `.rela.text` entries
5. **Link** — The system linker resolves symbols against libc or other shared libraries

---

## DWARF Debug Info

VUMA can emit DWARF v4 debug information when compiled with the `--debug` flag. This enables source-level debugging with GDB, LLDB, and other DWARF-aware tools.

### Sections Generated

| Section | Contents |
|---------|----------|
| `.debug_abbrev` | Abbreviation tables (tag + attribute encodings) |
| `.debug_info` | Compilation unit DIEs (subprograms, variables) |
| `.debug_line` | Line-number program (DWARF standard opcodes) |
| `.debug_frame` | Call frame information (CIE + FDE entries) |

### Multi-Backend Support

The DWARF builder is parameterised by address size to support all eight VUMA backends:

| Backend | Address Size | Min Inst Length | CIE Saved Registers |
|---------|-------------|-----------------|---------------------|
| x86_64 | 8 bytes | 1 | RSP=7, RBP=6 |
| AArch64 | 8 bytes | 4 | SP=31, LR=30, FP=29 |
| RISC-V 64 | 8 bytes | 2 | SP=2, RA=1, FP=8 |
| ARM32 | 4 bytes | 2 | SP=13, LR=14, FP=11 |
| MIPS64 | 8 bytes | 4 | SP=29, RA=31 |
| PPC64 | 8 bytes | 4 | SP=R1, LR=65 |
| LoongArch64 | 8 bytes | 4 | SP=r3, RA=r1, FP=r22 |
| Wasm32 | 4 bytes | 1 | Minimal (no saved regs) |

### Usage

```bash
# Compile with debug info
vuma compile --debug examples/debug_info.vuma

# Inspect debug sections
readelf --debug-dump=info output.o      # .debug_info (subprograms, variables)
readelf --debug-dump=line output.o      # .debug_line (source-line mapping)
readelf --debug-dump=frames output.o    # .debug_frame (CIE + FDE entries)

# Debug with GDB
gdb ./output
(gdb) break main
(gdb) run
(gdb) step
(gdb) info locals
```

### DWARF Encoding Details

- **Abbreviation codes**: `DW_TAG_COMPILE_UNIT`, `DW_TAG_SUBPROGRAM`, `DW_TAG_VARIABLE`
- **Attributes**: `DW_AT_NAME`, `DW_AT_LOW_PC`, `DW_AT_HIGH_PC`, `DW_AT_TYPE`, `DW_AT_LOCATION`
- **Forms**: `DW_FORM_STRING`, `DW_FORM_ADDR`, `DW_FORM_DATA4`, `DW_FORM_EXPRLOC`
- **Line-number opcodes**: `DW_LNS_COPY`, `DW_LNS_ADVANCE_PC`, `DW_LNS_ADVANCE_LINE`, `DW_LNS_SET_FILE`, `DW_LNE_END_SEQUENCE`
- **Call frame**: CIE with `DW_CFA_def_cfa` / `DW_CFA_offset`, FDE per function with `initial_location` and `address_range`

Without `--debug`, no debug sections are emitted — the ELF contains only `.text`, `.data`, and `.symtab`.

---

## Running Examples

```bash
# Compile and verify a VUMA program
vuma compile examples/hello_memory.vuma

# Compile with verbose IVE output
vuma compile --ive-verbose examples/doubly_linked_list.vuma

# Compile with DWARF debug info
vuma compile --debug examples/debug_info.vuma

# Compile FFI example as relocatable object
vuma compile --format obj --target aarch64 examples/ffi_demo.vuma -o ffi_demo.o
ld -o ffi_demo ffi_demo.o -lc

# Run on bare metal
vuma flash --target bare examples/gpio_blink.vuma
```

## Learning Path

### Beginner
1. Start with **hello_memory.vuma** to understand the four basic operations
2. Read **doubly_linked_list.vuma** to see how VUMA handles what Rust can't
3. Try **fibonacci.vuma** for recursive and iterative patterns
4. Explore **linked_list.vuma** for singly-linked list fundamentals

### Intermediate
5. Study **arena_allocator.vuma** for region-based memory management
6. Explore **sorted_map.vuma** for tree data structures with rotations
7. Learn **memory_arena.vuma** for advanced arena patterns with scopes
8. Tackle **quicksort.vuma** for in-place array algorithms
9. Try **bsearch.vuma** for O(log n) search patterns

### Algorithms & Encoding
10. Study **hex_dump.vuma** for byte-to-hex conversion
11. Explore **crc32.vuma** for table-driven checksum algorithms
12. Learn **matrix.vuma** for 2D data with nested loops
13. Master **base64_encode.vuma** for bit-level encoding algorithms

### Concurrency
14. Study **atomics_demo.vuma** for the full suite of atomic primitives
15. Tackle **lock_free_queue.vuma** for atomic SPSC concurrency
16. Study **thread_pool.vuma** for mutex/condvar synchronization patterns
17. Master **channel_demo.vuma** for MPSC message-passing concurrency

### Floating-Point
18. Start with **float_math.vuma** for FP conversion, arithmetic, and memory operations

### FFI & Debug
19. Learn **ffi_demo.vuma** for calling C library functions from VUMA
20. Try **debug_info.vuma** for DWARF debug info generation and inspection

### Embedded / Hardware
21. Start with **gpio_blink.vuma** for basic hardware access patterns

## IVE Verification Summary

Every example in this directory passes all 5 IVE invariants:

| Example | Liveness | Exclusivity | Interpretation | Origin | Cleanup |
|---------|----------|-------------|----------------|--------|---------|
| hello_memory | ✓ | ✓ | ✓ | ✓ | ✓ |
| doubly_linked_list | ✓ | ✓ | ✓ | ✓ | ✓ |
| arena_allocator | ✓ | ✓ | ✓ | ✓ | ✓ |
| gpio_blink | ✓ | ✓ | ✓ | ✓ | ✓ |
| lock_free_queue | ✓ | ✓ | ✓ | ✓ | ✓ |
| sorted_map | ✓ | ✓ | ✓ | ✓ | ✓ |
| thread_pool | ✓ | ✓ | ✓ | ✓ | ✓ |
| memory_arena | ✓ | ✓ | ✓ | ✓ | ✓ |
| channel_demo | ✓ | ✓ | ✓ | ✓ | ✓ |
| fibonacci | ✓ | ✓ | ✓ | ✓ | ✓ |
| quicksort | ✓ | ✓ | ✓ | ✓ | ✓ |
| linked_list | ✓ | ✓ | ✓ | ✓ | ✓ |
| hex_dump | ✓ | ✓ | ✓ | ✓ | ✓ |
| crc32 | ✓ | ✓ | ✓ | ✓ | ✓ |
| bsearch | ✓ | ✓ | ✓ | ✓ | ✓ |
| matrix | ✓ | ✓ | ✓ | ✓ | ✓ |
| base64_encode | ✓ | ✓ | ✓ | ✓ | ✓ |
| atomics_demo | ✓ | ✓ | ✓ | ✓ | ✓ |
| float_math | ✓ | ✓ | ✓ | ✓ | ✓ |
| debug_info | ✓ | ✓ | ✓ | ✓ | ✓ |
| ffi_demo | ✓ | ✓ | ✓ | ✓ | ✓ |

**No `unsafe` keyword exists in VUMA.** All verification is automatic.
