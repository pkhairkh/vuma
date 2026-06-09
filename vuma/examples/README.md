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

### 4. [gpio_blink.vuma](gpio_blink.vuma) — Raspberry Pi 5 hardware access

Blinks an LED on a Raspberry Pi 5 by directly accessing GPIO registers. Uses `map_device()` to map hardware addresses into the program's address space. IVE verifies all register accesses stay within the mapped region.

**Features demonstrated:** `map_device()`, hardware register access, `const` addresses, volatile semantics, embedded/bare-metal programming

---

### 5. [lock_free_queue.vuma](lock_free_queue.vuma) — Lock-free concurrency

A single-producer single-consumer (SPSC) ring buffer with atomic head/tail indices. IVE extends its verification to concurrent code, proving absence of data races, correct memory ordering, and no torn reads.

**Features demonstrated:** `AtomicU64`, `Acquire`/`Release` ordering, generic structs (`Queue<T>`), concurrent access verification, `Option<T>` return type

---

## Running Examples

```bash
# Compile and verify a VUMA program
vuma compile examples/hello_memory.vuma

# Compile with verbose IVE output
vuma compile --ive-verbose examples/doubly_linked_list.vuma

# Run on bare metal (Raspberry Pi 5)
vuma flash --target rpi5 examples/gpio_blink.vuma
```

## Learning Path

1. Start with **hello_memory.vuma** to understand the four basic operations
2. Read **doubly_linked_list.vuma** to see how VUMA handles what Rust can't
3. Study **arena_allocator.vuma** for region-based memory management
4. Explore **gpio_blink.vuma** for hardware access patterns
5. Tackle **lock_free_queue.vuma** for concurrent data structures
