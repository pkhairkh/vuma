# VUMA Benchmark Suite Design: Comparative Performance vs C and Rust

**Document ID:** VUMA-SPEC-BENCH-001

**Author:** Parham Khairkhah

---

## Table of Contents

1. [Benchmark Categories](#1-benchmark-categories)
2. [Micro-Benchmarks](#2-micro-benchmarks)
3. [Data Structure Benchmarks](#3-data-structure-benchmarks)
4. [Concurrency Benchmarks](#4-concurrency-benchmarks)
5. [Real-World Benchmarks](#5-real-world-benchmarks)
6. [Verification Time Benchmarks](#6-verification-time-benchmarks)
7. [Measurement Methodology](#7-measurement-methodology)
8. [Expected Results](#8-expected-results)

---

## 1. Benchmark Categories

The VUMA benchmark suite is organized into five distinct categories, each designed to isolate and measure specific aspects of language runtime performance, memory behavior, and the unique cost of IVE (Invariant Verification Engine) proof construction. These categories span the full spectrum from single-instruction-level overhead to whole-application throughput, ensuring that no performance characteristic escapes quantitative scrutiny. The categorization also reflects the layered architecture of VUMA itself: micro-benchmarks probe the codegen quality of VUMA's custom multi-ISA backend (not an LLVM pipeline — VUMA emits machine code directly via `src/codegen/`), data structure benchmarks exercise the BD-based memory model (not an ownership/borrowing verifier — VUMA uses Behavioral Descriptors), concurrency benchmarks stress the IVE's concurrent-memory-model proofs, real-world benchmarks validate end-to-end developer experience, and verification time benchmarks quantify the "price of safety" that IVE imposes at compile time.

> **Implementation note (2026-07):** This spec is **aspirational**. The benchmark harness described here (`vuma-bench`, 4-way comparison with C/Rust) is not implemented. The actual benchmark suite lives in `src/tests/src/benchmarks.rs` and measures 8 categories (SCG construction, BD inference, MSG construction, IVE verification, ARM64 codegen, C-equivalent comparison, memory usage, end-to-end pipeline) producing `BenchmarkResult { mean_ns, median_ns, iterations }`. The IVE acronym throughout this document means "Invariant Verification Engine", not "Interactive Verification Engine".

Each category is designed to be independently runnable and reproducible. A benchmark harness script (`vuma-bench`) will orchestrate execution across all four language configurations (VUMA verified, C with `-O2`, Rust safe, Rust unsafe), collect raw cycle counts, compute statistical aggregates, and emit both machine-readable JSON and human-readable Markdown tables. The harness will also enforce environmental controls: CPU frequency scaling disabled, isolated CPU cores via `taskset`, transparent hugepages configured consistently, and no other user-space workloads running during measurement windows.

The rationale for five categories rather than the traditional two (micro vs macro) is that VUMA introduces a novel axis -- verification time -- that no existing benchmark suite accounts for. Traditional benchmarks assume compilation cost is a one-time developer inconvenience; VUMA's IVE makes verification a measurable, repeatable cost that directly affects iteration speed and CI pipeline duration. By elevating verification time to a first-class benchmark category, we ensure that IVE performance regressions are caught as aggressively as runtime regressions. Additionally, separating concurrency from data structures acknowledges that multi-core AArch64 processors introduce NUMA-like caching effects that would confound results if mixed with single-threaded collection benchmarks.

---

## 2. Micro-Benchmarks

Micro-benchmarks measure the raw cost of individual operations, stripped of algorithmic complexity. Each benchmark performs a single, well-defined operation millions of times so that per-operation cost can be extracted with sub-nanosecond precision using the ARM64 PMU cycle counter. The goal is to isolate codegen differences between VUMA, C (compiled with `gcc -O2`), Rust safe (compiled with `cargo build --release`), and Rust unsafe (same but with `unsafe` blocks bypassing bounds checks). All micro-benchmarks operate on a pre-allocated 8 MiB buffer (2x L2 cache size on typical AArch64 cores) to ensure predictable cache behavior unless the benchmark explicitly tests cache-miss scenarios.

### 2.1 alloc_free

**Purpose:** Measure the full round-trip cost of heap allocation and deallocation. This benchmark reveals allocator overhead, which is a dominant cost in real-world programs that churn short-lived objects.

**Parameters:**
- Allocation sizes: 16 B, 64 B, 256 B, 1 KiB, 4 KiB, 16 KiB
- Iterations: 1,000,000 per size
- VUMA: uses `vuma::alloc::Box::new()` which IVE verifies for size correctness
- C: uses `malloc()` / `free()`
- Rust safe: uses `Box::new()` / drop
- Rust unsafe: uses `alloc()` / `dealloc()` from `std::alloc`

**Measurement:** Total cycles for all iterations, divided by iteration count. Report per-size. Expect VUMA to match C within 2% since VUMA's verification is compile-time only and the generated `malloc`/`free` calls should be identical.

### 2.2 sequential_read

**Purpose:** Measure sequential memory read throughput, which is dominated by cache line fill bandwidth and hardware prefetcher effectiveness. This benchmark validates that VUMA's generated load instructions are scheduled as efficiently as C's.

**Parameters:**
- Buffer size: 8 MiB (2,097,152 x 4-byte integers)
- Access pattern: stride-1 sequential read
- Iterations: 100 full passes over buffer
- Prefetch: none (test hardware prefetcher only)
- Data type: `i32`

**Measurement:** Cycles per element read. Expect all four configurations within 1% of each other, since this is purely a memory subsystem benchmark and codegen differences should vanish for trivial load-add loops.

### 2.3 sequential_write

**Purpose:** Measure sequential write throughput, which exercises store buffers and write-combining. VUMA's store instructions should be identical to C after IVE verification completes at compile time.

**Parameters:**
- Buffer size: 8 MiB
- Access pattern: stride-1 sequential write
- Iterations: 100 full passes
- Data type: `i32`

**Measurement:** Cycles per element written. Expect near-identical results across all four configurations.

### 2.4 random_read

**Purpose:** Measure the cost of random memory accesses, which stress the TLB and L1/L2 cache miss pathways. This benchmark is critical for VUMA because it reveals whether IVE's bounds verification adds any indirect cost (e.g., by preventing aliasing optimizations that C enjoys).

**Parameters:**
- Buffer size: 64 MiB (16,777,216 x 4-byte integers), far larger than L2 cache
- Access pattern: pre-generated random permutation of indices, fixed seed for reproducibility
- Iterations: 1,000,000 random reads per measurement
- Index generation: Fisher-Yates shuffle, seed 42

**Measurement:** Cycles per random read. Expect VUMA and C within 3%, Rust safe slightly slower due to bounds checks on index operations, Rust unsafe matching C.

### 2.5 random_write

**Purpose:** Measure random write cost, which is higher than random read due to cache line allocation on write-miss. Same parameters as random_read but with store operations.

**Parameters:**
- Buffer size: 64 MiB
- Access pattern: same random permutation as random_read
- Iterations: 1,000,000 random writes per measurement
- Write value: index XOR'd with constant

**Measurement:** Cycles per random write. Same expectations as random_read.

### 2.6 pointer_chase

**Purpose:** Measure the cost of following a chain of pointers, which is the worst case for branch prediction and cache locality. This is the canonical "pointer dereference cost" benchmark and is highly relevant to linked data structures.

**Parameters:**
- Chain length: 1,048,576 nodes (1M)
- Node size: 64 bytes (one cache line on typical AArch64 cores)
- Layout: pre-shuffled to randomize cache behavior
- Each node contains a single `next` pointer plus padding
- Iterations: 10 full chases

**Measurement:** Cycles per node traversal. Expect VUMA and C within 2%. Rust safe and unsafe both within 2% of C, since pointer chasing involves no bounds checks.

### 2.7 arithmetic

**Purpose:** Measure raw compute throughput for integer and floating-point operations, ensuring VUMA's arithmetic codegen matches C and Rust. Tests both register-pressure-light and register-pressure-heavy scenarios.

**Parameters:**
- Integer operations: add, sub, mul, div (i64), bitwise AND/OR/XOR/shift
- Float operations: add, sub, mul, div (f64), sqrt, sin (libm)
- Loop count: 100,000,000 iterations per operation
- Accumulator pattern: prevent dead-code elimination

**Measurement:** Cycles per operation. Expect near-identical results for integer ops; float ops may differ slightly due to libm implementation choices.

### 2.8 cast_operations

**Purpose:** Measure the cost of type reinterpretation casts, which is the critical differentiator for VUMA. In C, casts are free (zero runtime cost) but unsafe. In Rust, safe casts (`as`, `try_into`) may involve runtime checks. In VUMA, IVE verifies cast correctness at compile time, potentially adding verification cost but zero runtime cost.

**Parameters:**
- Cast types: i32-to-f32 bit reinterpret, i64-to-i32 truncation, f64-to-i64 conversion, pointer-to-integer, struct-to-bytes
- Iterations: 10,000,000 per cast type
- VUMA: uses `vuma::cast::verified_transmute()` which IVE proves safe
- C: uses `*(target_type*)&value` (undefined behavior, but common)
- Rust safe: uses `try_from()` / `try_into()` with error handling
- Rust unsafe: uses `std::mem::transmute()`

**Measurement:** Cycles per cast. Expect VUMA and C to match (both zero runtime overhead), Rust safe to be 2-5x slower on checked casts, Rust unsafe to match C.

---

## 3. Data Structure Benchmarks

Data structure benchmarks exercise VUMA's core value proposition: safe, verified data structures with zero runtime overhead compared to C. Each benchmark measures the three fundamental operations (insert, traverse, remove) on a collection, and additionally reports peak memory usage. The doubly-linked list benchmark is the flagship -- it is the canonical example where Rust requires `unsafe` for interior mutability, C is trivially unsafe, and VUMA provides verified safety with C-level performance.

### 3.1 doubly_linked_list

**Purpose:** The headline benchmark. A doubly-linked list is trivial in C (but unsafe), painful in safe Rust (requires `Rc<RefCell<Node>>` with reference counting overhead), ergonomic in unsafe Rust (raw pointers), and the showcase data structure for VUMA's ownership model. VUMA's IVE verifies that all pointer manipulations preserve list invariants (no dangling pointers, no cycles, proper sentinel handling) at compile time.

**Parameters:**
- List size: 10,000 nodes
- Node payload: 64 bytes (cache-line sized)
- Operations:
  - `insert_head`: 10,000 insertions at head
  - `insert_tail`: 10,000 insertions at tail
  - `insert_sorted`: 10,000 insertions in sorted order
  - `traverse_forward`: full forward traversal, sum payload
  - `traverse_backward`: full backward traversal, sum payload
  - `remove_head`: 10,000 removals from head
  - `remove_tail`: 10,000 removals from tail
  - `remove_middle`: 10,000 removals of random middle nodes
- VUMA: `vuma::collections::DList<T>` with IVE-verified pointers
- C: hand-written `struct Node { prev; next; payload; }` with `malloc`/`free`
- Rust safe: `std::collections::LinkedList<T>` (which internally uses unsafe)
- Rust unsafe: hand-rolled raw-pointer implementation

**Measurement:** Cycles per operation. Memory: total bytes allocated including overhead. Expect VUMA to match C and unsafe Rust; safe Rust's `LinkedList` should also match since the standard library uses unsafe internally, but a naive safe Rust implementation with `Rc<RefCell<>>` would be 3-5x slower and use 2-3x more memory.

### 3.2 hash_map

**Purpose:** Measure hash table performance, which is dominated by hash computation quality and memory access patterns. VUMA's verified hash map should perform identically to C's because IVE only verifies memory safety, not algorithmic choices.

**Parameters:**
- Table size: 100,000 entries
- Key type: `i64` (hash via SipHash-1-3, same as Rust's default)
- Value type: `i64`
- Load factors tested: 0.5, 0.75, 0.9
- Operations:
  - `insert`: 100,000 unique insertions
  - `lookup_hit`: 100,000 lookups for existing keys
  - `lookup_miss`: 100,000 lookups for absent keys
  - `remove`: 50,000 removals
- VUMA: `vuma::collections::HashMap<K,V>` with IVE-verified bucket access
- C: hand-written open-addressing hash map with linear probing
- Rust safe: `std::collections::HashMap<K,V>`
- Rust unsafe: custom hash map with raw pointer bucket access

**Measurement:** Cycles per operation at each load factor. Expect VUMA and C within 3%, Rust safe may be slightly slower due to SipHash overhead vs C's simpler hash function.

### 3.3 vec

**Purpose:** Measure dynamic array performance, focusing on the cost of bounds checking. This benchmark directly tests VUMA's claim that verified access eliminates bounds checks at runtime while maintaining safety. Rust's safe `vec[i]` performs a bounds check on every index operation; VUMA's IVE proves that indices are in-bounds and elides the check.

**Parameters:**
- Initial capacity: 0 (test growth strategy)
- Maximum size: 1,000,000 elements
- Element type: `i64`
- Operations:
  - `push`: 1,000,000 push operations (measures amortized growth cost)
  - `pop`: 1,000,000 pop operations
  - `index_sequential`: sequential access of all 1,000,000 elements
  - `index_random`: random access of 1,000,000 elements (same permutation as micro-benchmark)
  - `iterate`: sum all elements via iterator
  - `resize`: 100 resize operations from 0 to 1,000,000 and back
- VUMA: `vuma::collections::Vec<T>` with IVE-verified index elision
- C: hand-written `realloc`-based dynamic array with no bounds checks
- Rust safe: `std::vec::Vec<T>` with bounds checks on `v[i]`
- Rust unsafe: `std::vec::Vec<T>` with `v.get_unchecked(i)`

**Measurement:** Cycles per operation. The critical comparison is `index_sequential` and `index_random`: VUMA should match C and unsafe Rust (no bounds checks), while safe Rust should show 10-30% overhead on random index access due to the branch-prediction cost of bounds checks.

### 3.4 arena_allocator

**Purpose:** Measure arena (region-based) allocation performance, which is important for systems programming patterns where many objects share a lifetime. VUMA's IVE can verify that no arena-allocated reference escapes the arena's lifetime scope.

**Parameters:**
- Arena size: 64 MiB
- Object sizes: 32 B, 64 B, 128 B, 256 B
- Objects allocated: 100,000 per size
- Operations:
  - `alloc_many`: allocate 100,000 objects of each size
  - `access_all`: touch every allocated object (read + write)
  - `reset`: deallocate all at once (arena reset)
- VUMA: `vuma::alloc::Arena` with IVE-verified lifetime scoping
- C: hand-written bump allocator
- Rust safe: `typed_arena` crate
- Rust unsafe: hand-written bump allocator with raw pointers

**Measurement:** Cycles per allocation, cycles per access, cycles per reset. Expect all configurations within 5% since arena allocation is inherently simple (bump a pointer).

### 3.5 ring_buffer

**Purpose:** Measure single-producer single-consumer (SPSC) ring buffer performance, which is the foundational primitive for concurrent communication. VUMA's IVE verifies that the producer and consumer never access the same slot simultaneously.

**Parameters:**
- Buffer capacity: 1024 elements (power of 2 for modulo optimization)
- Element type: `i64`
- Items produced/consumed: 10,000,000
- Operations:
  - `produce`: enqueue items from producer thread
  - `consume`: dequeue items from consumer thread
  - `round_trip`: time for 10M items from producer to consumer
- VUMA: `vuma::sync::RingBuffer<T>` with IVE-verified SPSC invariant
- C: hand-written lock-free SPSC ring with atomic indices
- Rust safe: `crossbeam::channel::bounded(1024)`
- Rust unsafe: hand-written lock-free SPSC ring with atomic raw pointers

**Measurement:** Nanoseconds per item end-to-end. Expect VUMA and C within 5% (both use identical atomic-based ring logic); crossbeam may be slightly slower due to internal bookkeeping.

---

## 4. Concurrency Benchmarks

Concurrency benchmarks exercise VUMA's IVE concurrent memory model verification on multi-core AArch64 processors. These benchmarks are designed to stress both the runtime performance of synchronization primitives and the IVE's ability to prove data-race freedom at compile time. Each benchmark pins threads to specific cores using `pthread_setaffinity_np` (or the VUMA equivalent) to eliminate scheduler noise. All benchmarks are run with the CPU governor set to `performance` mode to eliminate frequency scaling variance.

### 4.1 parallel_sum

**Purpose:** Measure the simplest form of parallelism: partition an array into N chunks, sum each chunk on a separate core, then combine results. This benchmark tests thread creation overhead, cache coherency cost for read-only shared data, and the cost of the final reduction.

**Parameters:**
- Array size: 100,000,000 `i64` elements (800 MiB)
- Thread counts: 1, 2, 4
- Partition strategy: equal-sized contiguous chunks
- Reduction: sequential sum of partial results
- Thread pinning: core 0, core 1, core 2, core 3
- VUMA: `vuma::thread::spawn` with IVE-verified thread safety
- C: `pthread_create` / `pthread_join`
- Rust safe: `std::thread::spawn` with `Arc<[i64]>`
- Rust unsafe: `std::thread::spawn` with raw shared pointers

**Measurement:** Total wall-clock time and speedup ratio vs single-threaded. Expect near-linear scaling up to 4 cores for all configurations. VUMA should match C within 3%.

### 4.2 producer_consumer

**Purpose:** Measure throughput of a single-producer single-consumer pipeline using a lock-free queue. This benchmark isolates the cost of inter-thread communication, which is dominated by cache-line bouncing between cores.

**Parameters:**
- Queue: lock-free MPMC queue (used in SPSC mode)
- Message size: 8 bytes (`i64`)
- Message count: 10,000,000
- Producer pinned to core 0, consumer pinned to core 1
- Backpressure: unbounded (producer never blocks)
- VUMA: `vuma::sync::MpmcQueue<T>` with IVE-verified linearizability
- C: hand-written lock-free queue using C11 atomics
- Rust safe: `crossbeam::channel::unbounded()`
- Rust unsafe: hand-written lock-free queue with raw atomics

**Measurement:** Messages per second and median latency per message. Expect VUMA and C within 5%; crossbeam within 10% due to internal Select machinery.

### 4.3 mutex_contention

**Purpose:** Measure the cost of mutex contention when multiple threads repeatedly acquire and release the same lock. This is the worst-case scenario for lock-based concurrency and reveals the quality of each language's mutex implementation and the overhead of any language-specific bookkeeping.

**Parameters:**
- Thread counts: 1, 2, 4
- Iterations per thread: 1,000,000
- Critical section: increment a shared counter
- Mutex type: normal (non-recursive, non-adaptive)
- Threads pinned to separate cores
- VUMA: `vuma::sync::Mutex<T>` with IVE-verified lock discipline
- C: `pthread_mutex_t`
- Rust safe: `std::sync::Mutex<T>`
- Rust unsafe: `pthread_mutex_t` via FFI

**Measurement:** Cycles per lock-unlock pair. Throughput (operations/second) per thread count. Expect VUMA and C within 3%; Rust's `std::sync::Mutex` may show 5-15% overhead due to poison checking.

### 4.4 rw_lock_read_heavy

**Purpose:** Measure read-write lock performance under a read-heavy workload (90% reads, 10% writes). This benchmark is representative of configuration data or cached metadata access patterns in embedded systems.

**Parameters:**
- Thread counts: 1, 2, 4
- Read/write ratio: 90/10
- Total operations per thread: 1,000,000
- Shared data: array of 1000 `i64` values
- Read operation: sum 10 random elements
- Write operation: update 1 random element
- VUMA: `vuma::sync::RwLock<T>` with IVE-verified reader/writer discipline
- C: `pthread_rwlock_t`
- Rust safe: `std::sync::RwLock<T>`
- Rust unsafe: `pthread_rwlock_t` via FFI

**Measurement:** Operations per second per thread count. Expect VUMA and C within 5%. Rust's `std::sync::RwLock` on Linux uses `pthread_rwlock_t` internally, so results should be similar.

### 4.5 map_reduce

**Purpose:** Measure a complete parallel map-reduce pipeline, which combines computation parallelism (map phase) with synchronization (reduce phase). This benchmark exercises thread pool management, work stealing, and barrier synchronization.

**Parameters:**
- Input size: 10,000,000 elements
- Map function: `x -> x * x + x` (compute-bound)
- Reduce function: sum
- Thread counts: 1, 2, 4
- Work partitioning: dynamic (work-stealing) and static (chunked)
- VUMA: `vuma::parallel::map_reduce` with IVE-verified data partitioning
- C: hand-written thread pool with work queue
- Rust safe: `rayon::par_iter().map().reduce()`
- Rust unsafe: custom thread pool with raw pointers

**Measurement:** Wall-clock time and speedup ratio. Rayon's work-stealing scheduler may outperform static partitioning on heterogeneous workloads, but for uniform compute, all should scale similarly.

---

## 5. Real-World Benchmarks

Real-world benchmarks measure end-to-end application performance, including I/O, parsing, system calls, and complex memory access patterns. These benchmarks are the most meaningful to developers evaluating whether VUMA is practical for production use. Each benchmark implements a realistic, self-contained application in all four language configurations, using idiomatic patterns for each language (no "writing C in Rust" or "writing Rust in VUMA").

### 5.1 json_parser

**Purpose:** Parse a JSON document into an in-memory tree structure, then serialize it back to a string. This benchmark exercises memory allocation (many small nodes), string processing, and recursive data structures -- all areas where VUMA's verified memory safety should shine.

**Parameters:**
- Input: synthetic JSON document, 10 MiB, with nested objects (depth 10), arrays (length 100), strings (average 32 bytes), numbers, and booleans
- Parse: full DOM-style parse into tree of `JsonValue` nodes
- Serialize: walk tree and emit JSON string
- Validate: round-trip check (re-parse serialized output and compare)
- Iterations: 100
- VUMA: `vuma::json::parse()` with IVE-verified string handling (no buffer overread)
- C: `cJSON` library (popular, lightweight, uses `malloc`/`free`)
- Rust safe: `serde_json::from_str()` + `serde_json::to_string()`
- Rust unsafe: custom parser with `unsafe` string slicing

**Measurement:** Parse time (cycles), serialize time (cycles), peak memory (bytes). Expect VUMA within 5% of cJSON; serde_json may be faster due to zero-copy deserialization optimizations.

### 5.2 http_server

**Purpose:** Implement a minimal HTTP/1.1 server that handles GET requests for static content. This benchmark measures network I/O performance, request parsing, and response generation under load. It is the most complex benchmark in the suite and exercises nearly every subsystem.

**Parameters:**
- Server: listen on port 8080, serve files from memory (no disk I/O)
- File set: 100 files, sizes from 100 B to 100 KiB, pre-loaded into memory
- Client: `wrk` HTTP benchmarking tool, 2 threads, 100 connections, 30-second duration
- Metrics: requests/second, latency P50/P95/P99, error rate
- VUMA: `vuma::net::TcpListener` with IVE-verified buffer handling
- C: `libevent`-based event loop
- Rust safe: `tokio` async runtime with `hyper`
- Rust unsafe: `mio`-based manual event loop with raw buffer handling

**Measurement:** Requests per second, latency distribution. Expect all configurations within 10% since this is I/O-bound; differences will appear in CPU utilization during peak load.

### 5.3 gpio_control

**Purpose:** Toggle AArch64 GPIO pins at maximum frequency using memory-mapped I/O. This benchmark is AArch64-specific and measures the raw latency of MMIO operations, which is critical for real-time embedded applications. VUMA's IVE can verify that GPIO register accesses follow the correct sequence (set function select before writing data).

**Parameters:**
- GPIO: GPIO pin 18 (GPIO_GEN1, physical pin 12)
- Access method: `/dev/gpiomem` memory-mapped register access
- Toggle count: 10,000,000 on/off cycles
- Measurement: toggle frequency (Hz) and jitter (standard deviation of half-period)
- VUMA: `vuma::hal::gpio::Pin` with IVE-verified register access protocol
- C: direct `volatile` pointer dereference on `/dev/gpiomem` mapping
- Rust safe: `rppal` crate (uses underlying MMIO)
- Rust unsafe: raw `volatile` pointer dereference via `write_volatile`

**Measurement:** Toggle frequency (kHz) and cycle-to-cycle jitter (ns). Expect VUMA and C within 2% (identical MMIO instructions); rppal within 5%.

### 5.4 image_transform

**Purpose:** Apply a 3x3 convolution filter (Gaussian blur) to an image buffer. This benchmark measures compute throughput on regular data with SIMD potential. The ARM NEON SIMD unit on AArch64 should be exercisable from all four language configurations.

**Parameters:**
- Image: 4096 x 4096 pixels, RGBA (16 MiB per channel, 64 MiB total)
- Filter: 3x3 Gaussian kernel `[1,2,1; 2,4,2; 1,2,1] / 16`
- Iterations: 10 full-image transformations
- SIMD: test both scalar and NEON-vectorized paths
- VUMA: `vuma::simd::neon` intrinsics with IVE-verified lane access
- C: hand-written NEON intrinsics via `<arm_neon.h>`
- Rust safe: `std::simd` or packed SIMD operations
- Rust unsafe: raw NEON intrinsics via `core::arch::aarch64`

**Measurement:** Megapixels per second. Expect VUMA and C within 3% for both scalar and NEON paths; Rust may show minor differences depending on autovectorization quality.

### 5.5 sort_benchmark

**Purpose:** Sort 1,000,000 integers using multiple sorting algorithms. This benchmark exercises comparison-based branching, memory access patterns (sequential for merge sort, random for quicksort), and in-place vs out-of-place memory behavior.

**Parameters:**
- Data: 1,000,000 `i64` values, pseudo-random (seed 42)
- Algorithms: quicksort (in-place), merge sort (out-of-place), radix sort (non-comparison)
- Pre-sorted input: random, nearly-sorted (10% out of order), reverse-sorted, all-equal
- Iterations: 50 per algorithm per input pattern
- VUMA: `vuma::sort::quicksort` / `merge_sort` / `radix_sort` with IVE-verified partition bounds
- C: `qsort()` from libc + hand-written merge/radix sort
- Rust safe: `vec.sort_unstable()` (pdqsort) + hand-written merge/radix
- Rust unsafe: same algorithms with `get_unchecked` for partition swaps

**Measurement:** Cycles per element sorted. Expect VUMA and C within 5% for all algorithms; Rust's `sort_unstable` (pdqsort) may outperform naive quicksort in C.

---

## 6. Verification Time Benchmarks

Verification time benchmarks measure how long IVE takes to prove program correctness at compile time. This is the novel cost axis that VUMA introduces: unlike C and Rust, where compilation is a fixed cost regardless of program correctness, VUMA's IVE must construct a formal proof that the program satisfies its safety invariants. These benchmarks quantify the "price of verification" and help the VUMA team identify scalability bottlenecks in IVE's proof engine.

All verification times are measured on AArch64 (not cross-compiled), using `time vuma build --release` with IVE enabled. The measurement includes parsing, type checking, IVE proof construction, and LLVM codegen. For comparison, we also measure `gcc -O2` and `cargo build --release` compilation times for equivalent C and Rust programs.

### 6.1 trivial_program

**Purpose:** Measure IVE overhead on the simplest possible program: allocate an integer, read it, free it. This establishes the baseline IVE startup cost.

**Parameters:**
- Program: `fn main() { let x = Box::new(42i64); let y = *x; assert_eq!(y, 42); }`
- Lines of code: ~5
- IVE proof obligations: 3 (allocation valid, dereference valid, free valid)
- Compilation iterations: 20 (to warm filesystem caches)
- VUMA: `vuma build --release`
- C: `gcc -O2 trivial.c`
- Rust: `cargo build --release`

**Measurement:** Wall-clock compilation time in milliseconds. Expect VUMA to be 2-5x slower than C and Rust due to IVE proof construction overhead.

### 6.2 dlist_10_nodes

**Purpose:** Measure IVE verification time for a 10-node doubly-linked list. This exercises IVE's pointer-aliasing analysis at a small scale.

**Parameters:**
- Program: create a `DList<i64>`, insert 10 nodes, traverse, remove all
- Lines of code: ~50
- IVE proof obligations: ~80 (10 inserts x 4 pointer updates + 10 removes + 1 traversal)
- Compilation iterations: 20

**Measurement:** Compilation time. Expect VUMA compilation to take 0.5-2 seconds; C and Rust should compile in <100ms.

### 6.3 dlist_100_nodes

**Purpose:** Scale the doubly-linked list to 100 nodes to measure IVE's scaling behavior. This is critical: if IVE scales quadratically with node count, large programs become impractical.

**Parameters:**
- Program: create a `DList<i64>`, insert 100 nodes, traverse, remove all
- Lines of code: ~50 (same structure, different loop count)
- IVE proof obligations: ~800
- Compilation iterations: 20

**Measurement:** Compilation time. Compare with dlist_10_nodes to determine scaling factor. Ideal: linear scaling (10x time for 10x nodes). Acceptable: O(n log n). Problematic: O(n^2) or worse.

### 6.4 arena_1000_allocs

**Purpose:** Measure IVE verification time for an arena allocator with 1000 allocations. This tests IVE's ability to handle bulk allocation patterns without per-allocation proof overhead.

**Parameters:**
- Program: create an `Arena`, allocate 1000 objects of varying sizes, access all, reset arena
- Lines of code: ~30
- IVE proof obligations: ~3000 (1000 allocs + 1000 accesses + 1 reset + lifetime checks)
- Compilation iterations: 20

**Measurement:** Compilation time. Arena patterns should amortize verification cost due to uniform lifetime scoping.

### 6.5 concurrent_4_threads

**Purpose:** Measure IVE verification time for a 4-thread concurrent program. This exercises IVE's concurrent memory model proofs, which are the most computationally expensive verification step.

**Parameters:**
- Program: 4 threads sharing a mutex-protected counter, each incrementing 1000 times
- Lines of code: ~40
- IVE proof obligations: ~4000 (thread safety + lock discipline + data-race freedom for each shared access)
- Compilation iterations: 20

**Measurement:** Compilation time. Expect this to be the most expensive verification benchmark; may take 5-30 seconds depending on IVE's concurrent proof engine efficiency.

### 6.6 graph_100_nodes

**Purpose:** Measure IVE verification time for a 100-node graph structure. Graphs are the hardest data structure for ownership-based verification because nodes have arbitrary connectivity.

**Parameters:**
- Program: create a directed graph with 100 nodes and ~200 edges, perform BFS and DFS, deallocate
- Lines of code: ~80
- IVE proof obligations: ~5000 (node lifetime + edge validity + traversal bounds + deallocation completeness)
- Compilation iterations: 20

**Measurement:** Compilation time. This benchmark may require IVE to use arena-based lifetime inference for graph nodes. Expect 10-60 seconds depending on IVE's graph analysis capabilities.

---

## 7. Measurement Methodology

Rigorous measurement methodology is essential for producing benchmark results that are reproducible, defensible, and free of confounding factors. The VUMA benchmark suite adopts the following methodology, which is designed for AArch64 hardware characteristics (Cortex-A76 class, 4+ cores, 512 KB L2 cache per core, shared L3 cache).

### 7.1 Hardware Configuration

- **Platform:** AArch64 (ARM Cortex-A76 class, quad-core @ 2.4 GHz or equivalent)
- **RAM:** 8 GB DDR4 or LPDDR4X (or equivalent)
- **OS:** AArch64 Linux (64-bit, Linux kernel 6.6+)
- **CPU governor:** Set to `performance` mode (`cpufreq-set -g performance`) to eliminate dynamic frequency scaling
- **Isolation:** Disable WiFi, Bluetooth, and HDMI output to minimize interrupt noise
- **CPU affinity:** Each benchmark thread pinned to a dedicated core via `taskset` or `pthread_setaffinity_np`
- **Huge pages:** Transparent hugepages disabled (`echo never > /sys/kernel/mm/transparent_hugepage/enabled`) for consistent page fault behavior

### 7.2 Timing Mechanism

- **Primary timer:** ARM64 PMU cycle counter (`cntvct_el0`), accessed via `__builtin_arm_rsr64("cntvct_el0")` in C/VUMA and `std::arch::aarch64::_rdcntvct_el0()` in Rust
- **Resolution:** 1 cycle at 2.4 GHz = ~0.42 ns per tick
- **Overhead:** Timer read overhead measured and subtracted (typically 15-20 cycles on Cortex-A76 class cores)
- **Secondary timer:** `clock_gettime(CLOCK_MONOTONIC_RAW)` for wall-clock measurements (verification time, I/O-bound benchmarks)
- **Conversion:** Cycles to nanoseconds via `cycles / 2.4` (fixed frequency in performance mode)

### 7.3 Iteration Protocol

- **Warmup:** 10 iterations (discard results) to warm instruction cache, branch predictor, and TLB
- **Measurement:** 100 iterations (record all results)
- **Reporting:**
  - Mean: arithmetic mean of 100 iterations
  - Stddev: standard deviation of 100 iterations
  - Median: 50th percentile
  - P95: 95th percentile (captures tail latency)
  - Min: minimum iteration (best achievable, useful for comparing codegen quality)
  - Coefficient of variation (CV): stddev/mean; any benchmark with CV > 5% is flagged as unreliable

### 7.4 Comparison Matrix

| Configuration | Compiler | Flags | Notes |
|---|---|---|---|
| VUMA verified | `vuma build` | `--release` | IVE enabled, all safety proofs constructed |
| C | `gcc` | `-O2 -march=armv8.2-a` | Standard optimization, no UB sanitizer |
| Rust safe | `cargo` | `--release` | All safe Rust idioms, bounds checks enabled |
| Rust unsafe | `cargo` | `--release` | `unsafe` blocks for pointer ops, `get_unchecked` for indexing |

### 7.5 Metrics Collected

- **Execution time:** Mean cycles per operation (micro), mean wall-clock time (real-world)
- **Memory usage:** Peak RSS via `/proc/self/status` VmHWM field, measured per benchmark
- **Verification time:** Wall-clock compilation time (`time vuma build --release`), including IVE
- **Binary size:** Stripped ELF size via `size` command and `ls -l` on the final binary
- **Code size:** `.text` section size, `.rodata` section size (for embedded relevance)

### 7.6 Statistical Significance

- All results reported with 95% confidence intervals
- Outliers (>3 sigma from mean) are recorded but excluded from primary statistics
- Benchmarks are run three times on different days to detect environmental drift
- Any benchmark where day-to-day variance exceeds 5% is re-investigated for hidden confounds

### 7.7 Reproducibility

- All benchmark source code, build scripts, and raw results are stored in `vuma/benchmarks/`
- A `Dockerfile` (ARM64) is provided to recreate the exact benchmark environment
- Random seeds are fixed for all randomized benchmarks
- GCC version: 13.2+, Rust version: 1.75+, VUMA version: git commit hash recorded

---

## 8. Expected Results

This section documents our hypotheses about benchmark outcomes. These are informed predictions based on VUMA's design principles (compile-time verification, zero-cost abstractions, C-equivalent codegen) and known performance characteristics of C and Rust on ARM64. Actual results may differ, and deviations from these expectations will guide VUMA's optimization roadmap.

### 8.1 Execution Time

**Hypothesis: VUMA execution time within 5% of C across all benchmarks, and equal to or better than Rust safe.**

Rationale: VUMA compiles through LLVM (same backend as Clang/Rust) after IVE verification completes. Since IVE operates entirely at compile time, the generated machine code should be identical to what C would produce for equivalent logic. The only potential overhead sources are: (a) VUMA-specific calling conventions for verified functions, which we expect LLVM to optimize away; (b) extra move instructions for ownership tracking, which register allocation should eliminate; (c) any IVE-inserted runtime checks that couldn't be statically proven, which we expect to be zero for well-typed programs.

Against Rust safe, VUMA should win on bounds-checked operations (vec index, array access) by 10-30%, since IVE elides bounds checks that Rust's safe API must retain. Against Rust unsafe, VUMA should be within 1-2%, since both produce equivalent unchecked machine code.

The 5% margin accounts for: (a) minor codegen differences due to VUMA's type layout rules, (b) linker differences (VUMA links against a minimal runtime vs glibc for C), and (c) instruction scheduling differences that may favor one compiler over another on specific AArch64 pipeline configurations.

### 8.2 Verification Time

**Hypothesis: Sub-second for micro-benchmarks, 1-10 seconds for data structures, 10-60 seconds for concurrent/graph programs.**

Rationale: IVE's proof engine uses an SMT-based approach (Z3 or custom solver) with incremental verification. Micro-benchmarks have trivial proof obligations (3-10 per benchmark) that should resolve in milliseconds. Data structure benchmarks have 100-1000 proof obligations with moderate complexity (pointer aliasing, lifetime scoping) that should resolve in 1-10 seconds. Concurrent benchmarks have 1000-5000 proof obligations with high complexity (happens-before relations, lock disciplines) that may require 10-60 seconds.

The critical threshold is the developer iteration loop: if IVE adds more than 5 seconds to compilation, it begins to impact productivity. For programs up to ~10,000 lines, we expect IVE to stay under this threshold. For larger programs, incremental verification (re-proving only changed functions) should maintain acceptable iteration times.

Comparison: C compilation is essentially free (gcc -O2 on a 100-line program: ~50ms). Rust compilation is slow but acceptable (cargo build --release on a 100-line program: ~2-5 seconds, mostly due to LLVM optimization and monomorphization). VUMA verification should fall between Rust and an order-of-magnitude slower than Rust for complex concurrent programs.

### 8.3 Memory Usage

**Hypothesis: VUMA memory usage identical to C, 10-30% less than Rust safe.**

Rationale: VUMA has no garbage collector, no reference counting, no hidden vtables, and no panic unwinding tables (VUMA uses abort-on-error semantics for verified programs). Rust safe programs pay for: (a) `Rc<T>` / `Arc<T>` reference counting overhead (2 `usize` per reference: strong count + weak count), (b) `RefCell<T>` dynamic borrowing tracking (1 `usize` for borrow state), (c) panic unwinding tables (~5-10% of binary size, though not runtime memory), and (d) `enum` discriminant overhead for `Result<T, E>` error propagation.

VUMA avoids all of these because: (a) IVE proves ownership statically, eliminating reference counting, (b) IVE proves borrowing statically, eliminating `RefCell`, (c) verified programs cannot panic (IVE proves all error paths handled), and (d) VUMA uses explicit error returns without `Result` discriminant overhead when IVE can prove the error path is unreachable.

C achieves the same memory efficiency through the programmer's discipline; VUMA achieves it through IVE's mechanical verification. The difference is that VUMA's efficiency is guaranteed, while C's is aspirational.

### 8.4 Binary Size

**Hypothesis: VUMA binary size similar to C, 20-40% smaller than Rust.**

Rationale: Rust binaries include: (a) panic unwinding code and `.eh_frame` sections (~50-100 KiB even for simple programs), (b) vtables for `dyn Trait` objects, (c) `std` library bloat (formatter, allocator, threading infrastructure), and (d) duplicate generic monomorphizations. VUMA eliminates: (a) panic unwinding (verified programs abort on impossible errors), (b) vtables (VUMA uses compile-time dispatch for verified trait objects), (c) minimal runtime (no `std`, only `vuma::rt` with ~5 KiB of startup code), and (d) IVE-aware monomorphization that eliminates dead code more aggressively than Rust's generic instantiation.

Expected stripped binary sizes for a "hello world" program:
- C: ~15 KiB (dynamically linked to glibc)
- VUMA: ~20 KiB (statically linked minimal runtime)
- Rust: ~300 KiB (dynamically linked to libc, but includes unwinding)
- Rust (no_std): ~5 KiB (bare metal, but impractical for most applications)

For the JSON parser benchmark:
- C (cJSON): ~50 KiB
- VUMA: ~60 KiB
- Rust (serde_json): ~500 KiB

---

## Appendix A: Benchmark Execution Order

Benchmarks should be executed in the following order to minimize thermal throttling and cache interference:

1. Micro-benchmarks (smallest memory footprint, shortest duration)
2. Data structure benchmarks (moderate memory, moderate duration)
3. Verification time benchmarks (CPU-intensive compilation, no runtime measurement)
4. Concurrency benchmarks (highest thermal load, run last)
5. Real-world benchmarks (mixed characteristics, run individually)

Between each benchmark category, a 30-second cool-down period is enforced to allow AArch64 thermal management to return to baseline.

## Appendix B: Results Template

```json
{
  "benchmark": "dlist_insert_head",
  "category": "data_structure",
  "parameters": {
    "node_count": 10000,
    "node_size_bytes": 64,
    "iterations": 100
  },
  "results": {
    "vuma_verified": {
      "mean_cycles": 0,
      "stddev_cycles": 0,
      "median_cycles": 0,
      "p95_cycles": 0,
      "min_cycles": 0,
      "peak_rss_bytes": 0,
      "binary_size_bytes": 0,
      "verification_time_ms": 0
    },
    "c_gcc_O2": { "mean_cycles": 0, "stddev_cycles": 0, "median_cycles": 0, "p95_cycles": 0, "min_cycles": 0, "peak_rss_bytes": 0, "binary_size_bytes": 0 },
    "rust_safe": { "mean_cycles": 0, "stddev_cycles": 0, "median_cycles": 0, "p95_cycles": 0, "min_cycles": 0, "peak_rss_bytes": 0, "binary_size_bytes": 0 },
    "rust_unsafe": { "mean_cycles": 0, "stddev_cycles": 0, "median_cycles": 0, "p95_cycles": 0, "min_cycles": 0, "peak_rss_bytes": 0, "binary_size_bytes": 0 }
  },
  "environment": {
    "platform": "aarch64",
    "cpu": "cortex-a76",
    "cores": 4,
    "ram_gb": 0,
    "kernel": "",
    "gcc_version": "",
    "rust_version": "",
    "vuma_version": ""
  }
}
```

## Appendix C: Glossary

- **IVE:** Interactive Verification Engine -- VUMA's compile-time proof system
- **PMU:** Performance Monitoring Unit -- ARM64 hardware performance counters
- **cntvct_el0:** Virtual counter register (ARM64 timer)
- **SPSC:** Single-Producer Single-Consumer
- **MPMC:** Multi-Producer Multi-Consumer
- **MMIO:** Memory-Mapped I/O
- **NEON:** ARM SIMD instruction set
- **RSS:** Resident Set Size (physical memory usage)
- **CV:** Coefficient of Variation (stddev/mean)
