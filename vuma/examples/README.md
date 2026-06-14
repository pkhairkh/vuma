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

## Running Examples

```bash
# Compile and verify a VUMA program
vuma compile examples/hello_memory.vuma

# Compile with verbose IVE output
vuma compile --ive-verbose examples/doubly_linked_list.vuma

# Run on bare metal
vuma flash --target bare examples/gpio_blink.vuma
```

## Learning Path

### Beginner
1. Start with **hello_memory.vuma** to understand the four basic operations
2. Read **doubly_linked_list.vuma** to see how VUMA handles what Rust can't

### Intermediate
3. Study **arena_allocator.vuma** for region-based memory management
4. Explore **sorted_map.vuma** for tree data structures with rotations
5. Learn **memory_arena.vuma** for advanced arena patterns with scopes

### Concurrency
6. Tackle **lock_free_queue.vuma** for atomic SPSC concurrency
7. Study **thread_pool.vuma** for mutex/condvar synchronization patterns
8. Master **channel_demo.vuma** for MPSC message-passing concurrency

### Embedded / Hardware
9. Start with **gpio_blink.vuma** for basic hardware access patterns

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

**No `unsafe` keyword exists in VUMA.** All verification is automatic.
