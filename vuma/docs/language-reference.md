# VUMA Language Reference

> Version 0.2.0 — Last updated 2026-03-06

This document is the definitive reference for the VUMA programming language, a memory-oriented, verification-first systems language targeting 8 backend architectures (AArch64, x86_64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, Wasm32). VUMA eliminates the need for `unsafe` blocks by making the Invariant Verification Engine (IVE) an integral part of compilation. Every pointer dereference, allocation, and free is automatically checked against five core invariants: **Liveness**, **Exclusivity**, **Interpretation**, **Origin**, and **Cleanup** (collectively "LIVE").

---

## Table of Contents

1. [Lexical Structure](#1-lexical-structure)
2. [Types and BD Annotations](#2-types-and-bd-annotations)
3. [Memory Model](#3-memory-model)
4. [Pointer Operations](#4-pointer-operations)
5. [Control Flow](#5-control-flow)
6. [Functions](#6-functions)
7. [Concurrency](#7-concurrency)
8. [Memory Safety](#8-memory-safety)
9. [Standard Library Overview](#9-standard-library-overview)
10. [FFI and External Functions](#10-ffi-and-external-functions)
11. [Platform-Specific Features](#11-platform-specific-features)
12. [Appendix: Keyword and Operator Quick Reference](#12-appendix-keyword-and-operator-quick-reference)

---

## 1. Lexical Structure

The VUMA lexer transforms raw source text into a flat stream of tokens, each annotated with byte-offset spans and line/column positions for error reporting. The lexer supports full error recovery: it never stops on the first error, producing `Error` tokens and collecting diagnostic information that can be retrieved later. Whitespace and non-doc comments are silently skipped; doc comments (`///` and `//!`) are emitted as tokens for downstream tooling.

### Keywords

VUMA reserves the following keywords. They cannot be used as identifiers unless escaped. The keyword set is organized into six categories reflecting the language's design priorities:

**Core language keywords:** `fn`, `let`, `if`, `else`, `while`, `for`, `return`, `as`, `match`, `struct`, `enum`, `loop`

**Memory primitive keywords:** `ptr`, `region`, `alloc`, `allocate`, `free`, `derive`, `cast`, `read`, `write`

**Concurrency and synchronization keywords:** `sync`, `async`, `await`, `spawn`, `lock`, `unlock`, `channel`, `send`, `recv`

**Safety keywords:** `unsafe`, `safe`

**Behavioral domain directive keywords:** `bd`, `repd`, `capd`, `reld`

**Module system keywords:** `import`, `export`, `mod`, `use`, `self`, `super`

**Literal keywords:** `true`, `false`

**Type operator keywords:** `sizeof`, `alignof`

### Operators

VUMA provides a comprehensive set of arithmetic, comparison, logical, bitwise, and pointer-specific operators:

| Category | Operators |
|---|---|
| Arithmetic | `+` `-` `*` `/` `%` |
| Comparison | `==` `!=` `<` `<=` `>` `>=` |
| Logical | `&&` `\|\|` `!` |
| Bitwise | `&` `\|` `^` `~` `<<` `>>` |
| Assignment | `=` |
| Pointer | `*` (dereference) `@` (address-of) |
| Type | `as` (cast) `::` (path separator) |
| Arrow | `->` (return type) `=>` (match arm) |
| Other | `.` `..` `...` `:` `;` `,` `?` `#` `$` |

### Literals

VUMA supports the following literal forms, each with underscore separators for readability:

- **Integer literals:** `42`, `0`, `1_000_000`
- **Hex address literals:** `0xDEADBEEF`, `0xFF00_0000` — these produce the `Address` token kind, representing a `u64` memory address
- **Binary literals:** `0b1010_1100`
- **Octal literals:** `0o777`
- **Float literals:** `3.14`, `1e10`, `2.5e-3`
- **String literals:** `"hello"`, with escape sequences `\n`, `\t`, `\r`, `\\`, `\"`, `\0`, `\xHH`
- **Character literals:** `'a'`, `'\n'`
- **Byte string literals:** `b"hello"`
- **Raw string literals:** `r"..."`, `r#"..."#`
- **Boolean literals:** `true`, `false`

### Comments

VUMA supports three forms of comments:

- **Line comments:** `//` — extend to end of line, silently skipped by the lexer
- **Block comments:** `/* ... */` — can be nested, silently skipped
- **Doc comments:** `///` (outer) and `//!` (module-level) — emitted as `DocComment` and `ModuleDoc` tokens for documentation tooling

```vuma
// This is a line comment
/* This is a
   block comment */

/// This is an outer doc comment — documents the next item
//! This is a module-level doc comment
```

### Identifiers

Identifiers begin with an ASCII letter or underscore, followed by any combination of ASCII letters, digits, or underscores. Keywords are reserved and cannot be used as identifiers. Type names conventionally use `PascalCase`, while function and variable names use `snake_case`.

```vuma
my_variable    // valid
_buffer        // valid
NodeHeader     // valid (convention: type name)
3things        // invalid — starts with a digit
alloc          // invalid — reserved keyword
```

---

## 2. Types and BD Annotations

VUMA's type system is built around the concept of **Behavioral Descriptors (BDs)**, which provide a complete specification of a value along three orthogonal axes. Every value in VUMA carries a BD that the Invariant Verification Engine (IVE) uses to prove memory safety without requiring an `unsafe` escape hatch. The type system covers primitive types, pointer types, region-annotated types, struct types, generic type applications, function types, and explicit BD annotations.

### The BD Triple: RepD × CapD × RelD

A Behavioral Descriptor is the composition of three sub-descriptors:

```
BD = RepD × CapD × RelD
```

Two BDs are **compatible** when all three layers are pairwise compatible. One BD **refines** another when every layer is at least as specific:

```
bd1 ⊑ bd2  ⟺  bd1.repd.subsumes(bd2.repd)
               ∧ bd1.capd.is_subset(bd2.capd)
               ∧ bd1.reld.refines(bd2.reld)
```

### RepD — Representation Descriptor

RepD describes the memory shape, size, and alignment of a value. It forms a subsumption lattice where more-specific RepDs subsume less-specific ones. The `Byte` variant is the universal bottom element that is compatible with everything.

```vuma
// RepD variants (from the bd crate):
//   Byte(size, align)   — raw byte sequence, compatible with anything
//   Struct { fields }   — named field layout
//   Array { elem, len } — homogeneous fixed-size array
//   Enum { variants }   — tagged union
//   Ptr(pointee)        — pointer representation
//   Union { variants }  — untagged union
//   Func { params, ret }— function pointer representation

// In source code, RepD is inferred from type annotations:
let x: i32 = 42;           // RepD: Byte(4, 4)
let p: Address = allocate(8); // RepD: Ptr(Byte(8, 8))
```

### CapD — Capability Descriptor

CapD specifies what operations are permitted on a value. It forms a lattice where the **meet** (intersection of capabilities, union of conditions) and **join** (union of capabilities, intersection of conditions) operations are well-defined. Capabilities can be conditioned on holding a specific lock, enabling fine-grained concurrency reasoning.

```vuma
// CapD capabilities (from the bd crate):
//   Read     — the value may be read
//   Write    — the value may be written
//   Execute  — the value may be executed as code
//   Derive   — new pointers may be derived from this value

// CapD can be conditioned on lock acquisition:
//   CapD { read: true, write: true, write_requires_lock: 42 }
//   This means write is only permitted when lock #42 is held.

// CapD lattice operations:
//   meet(a, b) = intersection of caps, union of conditions (more restrictive)
//   join(a, b) = union of caps, intersection of conditions (less restrictive)

// Explicit BD annotation syntax:
#bd(ReadOnly)    // CapD with only Read capability
#bd(ReadWrite)   // CapD with Read and Write capabilities
#bd(LockedWrite) // CapD where Write requires a specific lock
```

### RelD — Relational Descriptor

RelD captures temporal, dependency, and security relationships that a value participates in. It ensures that constraints such as lifetime ordering, information flow policies, and dependency tracking are preserved across operations. RelD composition is the union of relations, and consistency checking ensures that contradictory constraints (e.g., `Outlives` + `Succeeds`) are rejected.

```vuma
// RelD relation types (from the bd crate):
//   Liveness      — value is guaranteed to be live
//   Outlives(a,b) — region a outlives region b
//   DependsOn(a,b)— computation depends on value b
//   Succeeds(a,b) — event a happens after event b
//   FlowPolicy(p) — information-flow policy label p

// RelD is usually inferred, but can be annotated:
// #bd(Exclusive) carries an implicit RelD with Liveness
```

### Type Syntax

The full type syntax in VUMA covers:

```vuma
// Primitive (BD base) types:
u8  u16  u32  u64    // unsigned integers
i8  i16  i32  i64    // signed integers
bool                  // boolean
void                  // unit/void type
Address               // opaque memory region handle (u64)

// Pointer types:
*i32                  // pointer to i32
*Address              // pointer to an address (double indirection)

// Region-annotated pointers:
*i32 @ arena          // pointer to i32 in region "arena"

// Fixed-size arrays:
[i32; 10]             // array of 10 i32 values

// Struct types:
NodeHeader            // named struct (defined with `struct`)

// Generic types:
Queue<T>              // generic type parameterized by T
Result<T, E>          // generic with two type parameters

// Function types:
(i32, i32) -> i32     // function taking two i32s, returning i32
() -> void            // nullary function returning void

// BD annotation types:
#bd(ReadOnly)         // behavioral descriptor annotation
```

### Type Ascription

Variables may optionally carry type annotations. When omitted, the type is inferred from context or the right-hand side expression:

```vuma
let x: i32 = 42;          // explicit type
let y = 42;               // inferred as integer literal
let p: Address = allocate(8); // explicit Address type
value: i32 = *region;     // type ascription without `let`
```

---

## 3. Memory Model

VUMA's memory model is built around four first-class operations: **allocate**, **write**, **read**, and **free**. Unlike languages where these are library calls, VUMA makes them primitive statements that the Invariant Verification Engine (IVE) automatically checks. Every pointer operation is verified against the five LIVE invariants, and there is no `unsafe` keyword to bypass verification. This section describes regions, allocations, derivations, and accesses — the core concepts that define how VUMA programs interact with memory.

### Regions

A **region** is a contiguous block of memory created by `allocate(size)` or `map_device(base, size)`. Regions are identified by their `Address` value, an opaque 64-bit handle. The IVE tracks the lifetime and bounds of every region, ensuring that all accesses fall within the region's allocated range.

```vuma
// Allocate a region for one 64-bit integer
region = allocate(8);

// Allocate a region large enough for a struct
node = allocate(24);  // sizeof(NodeHeader) = 24

// Map a hardware device region (never freed, volatile semantics)
gpio = map_device(0x7e200000, 4096);
```

Regions can be named using the `region` keyword for clarity:

```vuma
region pool = allocate(4096);  // named region declaration
```

### Allocations

The `allocate(size)` statement reserves `size` bytes of memory and returns an `Address`. The IVE verifies at compile time that:

1. **Liveness:** the returned address is live immediately after allocation
2. **Interpretation:** the allocation size is a positive, known value
3. **Cleanup:** every allocation is eventually freed (or mapped as a device that never needs freeing)

```vuma
// Simple allocation
p = allocate(8);        // 8 bytes for a u64
*p = 42;                // write to the region
value: u64 = *p;        // read from the region
free(p);                // release the region

// Arena-style allocation
arena = allocate(4096); // one large block
a = arena + 0;          // offset 0 within arena
b = arena + 64;         // offset 64 within arena
// IVE knows a and b are within the arena's 4096-byte range
free(arena);            // frees the entire arena; a and b are now dead
```

### Derivations

A **derivation** is the process of creating a new pointer from an existing one. VUMA tracks the full derivation chain so the IVE knows which original allocation a derived pointer comes from. This is critical for proving that offsets stay within bounds and that freeing a base allocation invalidates all derived pointers.

```vuma
// Pointer arithmetic derives a new address
base = allocate(1024);
offset_ptr = base + 128;     // derived: base + 128
// IVE knows offset_ptr is within base's 1024-byte allocation

// The derive expression is explicit:
derived = derive(ptr, region);  // formal derivation with region annotation

// Arena allocation uses derivation chains:
current = (*arena).base + (*arena).offset;  // derived from arena.base
aligned = current.align_to(16);             // further derivation with alignment
// IVE tracks: arena.base → current → aligned
```

When the base allocation is freed, the IVE marks **all** derived pointers as invalid. This prevents the most common bug in arena-based code: using a pointer after its arena has been destroyed.

```vuma
arena = arena_create(4096);
a = arena_alloc(arena, 64, 8);
b = arena_alloc(arena, 128, 16);
arena_destroy(arena);
// IVE: a and b are now dead pointers
// x = *a;  // ERROR: use of dead pointer `a`
```

### Accesses

A **memory access** is a read or write through a dereferenced pointer. Every access is checked by the IVE against the five invariants. The IVE tracks byte-level granularity, so it can prove that two accesses to different fields of the same struct do not conflict — this is how VUMA verifies doubly-linked list operations that Rust's borrow checker rejects.

```vuma
// Write access — stores a value into a region
*region = 42;
(*node).next = sentinel;
(*last).prev = node;

// Read access — loads a value from a region
value: i32 = *region;
next_node = (*current).next;

// The IVE verifies for each access:
//   Liveness:      the target address is live
//   Exclusivity:   no concurrent conflicting access (write-write or write-read)
//   Interpretation: the BD at the read matches the BD at the write
//   Origin:        the data came from a valid write
//   Cleanup:       the region will eventually be freed
```

---

## 4. Pointer Operations

VUMA provides a small set of first-class pointer operations that cover all the functionality traditionally achieved through `unsafe` pointer arithmetic in Rust or C. These operations are not library calls; they are primitive language constructs that the IVE checks automatically. The key operations are **derive** (create a derived pointer), **offset** (pointer arithmetic), and **cast** (type reinterpretation with proof obligations).

### Dereference (`*`)

The dereference operator `*` reads or writes through a pointer. When used on the left side of an assignment, it performs a write; when used in an expression, it performs a read. The IVE verifies every dereference against all five invariants.

```vuma
region = allocate(8);

// Write via dereference
*region = 42;

// Read via dereference
value: i32 = *region;

// Dereference with field access
node = allocate(24);
*node = NodeHeader { prev: 0, next: 0, data: 0 };
(*node).prev = node;   // write to .prev field
(*node).next = node;   // write to .next field
prev_val = (*node).prev; // read from .prev field
```

### Address-Of (`@`)

The `@` operator takes the address of a value, producing a pointer. This is the inverse of dereference:

```vuma
let x: i32 = 42;
ptr = @x;       // ptr has type *i32
*ptr = 100;     // modifies x through the pointer
```

### Offset (Pointer Arithmetic)

Pointer arithmetic is expressed using the `+` operator between an `Address` and an integer offset, or through the `Offset` expression. The IVE tracks the derivation chain and verifies that the resulting address remains within the bounds of the original allocation.

```vuma
base = allocate(1024);

// Offset by bytes
slot_0 = base + 0;     // first 8 bytes
slot_1 = base + 8;     // next 8 bytes
slot_2 = base + 16;    // and so on

// IVE verifies: each slot is within base's 1024-byte allocation
*slot_0 = 10;
*slot_1 = 20;
*slot_2 = 30;

// Hardware register access via offset
const GPIO_BASE: Address = 0x7e200000;
gpio = map_device(GPIO_BASE, 4096);
fsel = gpio + 0x00;    // GPFSEL0 offset
set = gpio + 0x1c;     // GPSET0 offset
clr = gpio + 0x28;     // GPCLR0 offset
```

### Derive

The `derive(ptr, region)` expression explicitly creates a derived pointer within a specific region. This is the formal mechanism for sub-allocation, where a pointer into a larger region is created and the IVE must track its provenance. Derive is implicit in pointer arithmetic but can be made explicit for annotation purposes.

```vuma
// Explicit derivation
inner = derive(ptr, arena_region);

// Implicit derivation via pointer arithmetic (equivalent)
inner = arena_base + offset;

// The derivation chain is tracked:
//   arena.base → current → aligned → ...
// When arena is freed, all derived pointers are invalidated.
```

### Cast

The `cast` operation (expressed with the `as` keyword or the `cast` statement) reinterprets a value as a different type. This is the only way to perform type punning in VUMA, and it carries proof obligations: the IVE checks that the RepD of the target type is compatible with the RepD of the source type. If the cast strengthens capabilities (CapD), a safety proof must be provided.

```vuma
// Safe cast: narrowing capability (always allowed)
let raw: u64 = ptr as u64;  // pointer to integer (weakening)

// Cast with proof obligation: widening capability
let ptr: *i32 = raw as *i32;  // integer to pointer (strengthening)
// IVE requires proof that `raw` originated from a valid pointer

// Explicit cast statement
cast expr as Type;

// sizeof and alignof for safe casting
let size = sizeof(NodeHeader);   // 24
let align = alignof(NodeHeader); // 8
```

---

## 5. Control Flow

VUMA provides the standard set of imperative control flow constructs, each with IVE integration to ensure that memory safety invariants are preserved across all execution paths. The IVE analyzes all branches of conditionals, all loop iterations, and all match arms to verify that every possible execution path satisfies the five LIVE invariants.

### If / Else

The `if` statement conditionally executes a block based on a boolean expression. An optional `else` block provides an alternative path. The IVE tracks the state of memory on both branches and merges them at the join point, ensuring that invariants hold regardless of which branch was taken.

```vuma
if next_head % (*q).capacity == tail % (*q).capacity {
    return false;  // Queue is full
} else {
    *slot = value;
    (*q).head.store(next_head, Release);
    return true;
}

// Nested conditionals are supported:
if x > 0 {
    if x > 100 {
        *ptr = 2;
    } else {
        *ptr = 1;
    }
} else {
    *ptr = 0;
}
```

### While

The `while` loop repeatedly executes a block as long as a condition is true. The IVE verifies that loop bodies preserve invariants on every iteration and that the loop terminates (when termination analysis is possible). For linked data structures, the IVE can prove that the loop will terminate because the list structure is acyclic aside from the sentinel.

```vuma
// Traverse a linked list
current = (*list).next;
while current != list {
    print((*current).data);
    current = (*current).next;
}

// Free all nodes in a list
current = (*list).next;
while current != list {
    next = (*current).next;
    free(current);
    current = next;
}
free(list);
```

### For

The `for` loop iterates over a range or iterable expression. The iterator variable is bound in the loop body's scope:

```vuma
// Iterate over a range
for i in 0..10 {
    slot = base + i * 8;
    *slot = i;
}

// Iterate over elements
for element in buffer {
    process(element);
}
```

### Loop

The infinite `loop` construct runs a block indefinitely. It is typically used in embedded systems for main event loops. The IVE accepts infinite loops in contexts where they are expected (e.g., bare-metal `main` functions) but flags them if they could leak resources.

```vuma
// Bare-metal blink loop
loop {
    gpio_set(led_pin);
    delay_ms(500);
    gpio_clear(led_pin);
    delay_ms(500);
}
```

### Match

The `match` statement performs pattern matching on a value. Patterns can be literals, identifiers, wildcards, or struct-like destructuring. Each arm maps a pattern to an expression. The IVE ensures that all match arms preserve invariants and that the match is exhaustive when required.

```vuma
match result {
    Some(value) => process(value),
    None => handle_empty(),
}

match opcode {
    0x01 => execute_load(),
    0x02 => execute_store(),
    _ => handle_unknown(),  // wildcard catches everything else
}

match node {
    Node { data, next } => traverse(next),
}
```

### Return

The `return` statement exits the current function, optionally with a value. The IVE verifies that returning does not leak any resources (Cleanup invariant) and that the returned value satisfies the function's declared return type.

```vuma
fn add(a: i32, b: i32) -> i32 {
    return a + b;
}

fn maybe_push(q: Address, value: u64) -> bool {
    if is_full(q) {
        return false;
    }
    push(q, value);
    return true;
}
```

---

## 6. Functions

Functions are the primary unit of code organization in VUMA. A function definition specifies a name, a list of typed parameters, an optional return type, and a body block. Functions are first-class items that appear at the top level of a program or within module declarations. The IVE analyzes each function independently and in the context of its callers to verify memory safety across function boundaries.

### Function Definition

The `fn` keyword introduces a function definition. Parameters may carry optional type annotations, and the return type is specified after `->`. If no return type is specified, the function returns `void`.

```vuma
fn main() -> i32 {
    region = allocate(8);
    *region = 42;
    value: i32 = *region;
    free(region);
    return value;
}

fn push_back(list: Address, value: u64) {
    sentinel = list;
    last = (*sentinel).prev;
    node = allocate(24);
    *node = NodeHeader { prev: last, next: sentinel, data: value };
    (*last).next = node;
    (*sentinel).prev = node;
}

fn arena_alloc(arena: Address, alloc_size: u64, align: u64) -> Address {
    current = (*arena).base + (*arena).offset;
    aligned = current.align_to(align);
    new_offset = (aligned - (*arena).base) + alloc_size;
    (*arena).offset = new_offset;
    return aligned;
}
```

### Function Calling

Functions are called by name with parenthesized arguments. The IVE verifies that the arguments are compatible with the parameter types and that any `Address` arguments point to live regions. Function calls can appear as expressions or statements.

```vuma
// Calling a function as a statement
push_back(list, 10);
free(region);

// Calling a function as an expression
value: u64 = queue_pop(q);
size = sizeof(NodeHeader);

// Method-style calls via namespace access
head = (*q).head.load(Acquire);
AtomicU64::new(0);
```

### Async / Spawn

VUMA supports lightweight concurrency through `async` blocks and `spawn` expressions. An `async` block creates a deferred computation that can be executed concurrently. A `spawn` expression launches an async block on a new task. The IVE extends its five invariants to concurrent code, verifying that no data races occur across tasks and that shared resources are properly synchronized.

```vuma
// Async block — creates a deferred computation
task = async {
    let result = compute_value();
    return result;
};

// Spawn — launches an async block as a concurrent task
handle = spawn async {
    process_data(buffer);
};

// Await — waits for a spawned task to complete
result = await handle;

// Spawning multiple concurrent tasks
h1 = spawn process_queue(q1);
h2 = spawn process_queue(q2);
await h1;
await h2;
```

The IVE verifies async/spawn code by:
- Checking that no two concurrent tasks write to overlapping memory without synchronization
- Ensuring that shared reads are safe (concurrent reads never conflict)
- Tracking the derivation chain across task boundaries
- Verifying that all resources are freed even in the presence of concurrent execution

---

## 7. Concurrency

VUMA provides language-level concurrency primitives that the IVE can reason about formally. Rather than relying on external libraries, VUMA bakes synchronization constructs into the language so that the IVE can prove the absence of data races, deadlocks, and use-after-free bugs in concurrent code. The key constructs are `sync` blocks, channels, locks, and atomic operations.

### Sync Blocks

The `sync` block establishes a happens-before ordering between accesses inside the block and accesses outside it. This is the fundamental mechanism for telling the IVE that certain operations are sequentially ordered. The IVE uses sync edges to build the ordered relation (transitive closure of happens-before), and skips conflict checks for ordered accesses.

```vuma
// Sync block ensures sequential ordering
sync {
    *shared_data = 42;
    result = *shared_data;
}
// IVE: the write happens before the read, no data race

// Without sync, concurrent accesses might conflict:
// Thread 1: *shared_data = 42;
// Thread 2: result = *shared_data;
// IVE would flag this as a write-read conflict without a sync edge
```

### Channels

Channels provide message-passing concurrency. The `channel` keyword creates a typed communication channel, `send` places a value into the channel, and `recv` extracts a value. Channels enforce ownership transfer: sending a value through a channel moves ownership to the receiver, preventing the sender from accessing it afterward.

```vuma
// Create a channel
ch = channel<u64>();

// Send a value (ownership transfers)
send(ch, computed_value);

// Receive a value
value = recv(ch);

// The IVE verifies:
// - After send(), the sender no longer has access to the value
// - recv() only succeeds after a matching send()
// - No two receivers can get the same value (Exclusivity)
```

### Locks

VUMA's `lock` and `unlock` statements provide mutual exclusion. The IVE integrates locks into the CapD lattice: when a CapD specifies `write_requires_lock: id`, the IVE knows that the write is safe as long as lock `id` is held. This enables the IVE to classify conflicting accesses as "probably safe" when they are both protected by the same mutex.

```vuma
// Lock-based synchronization
lock(mutex_id);
// IVE: mutex_id is now held, write-locked CapDs are active
(*shared).counter = (*shared).counter + 1;
unlock(mutex_id);
// IVE: mutex_id is no longer held

// The IVE tracks which locks are held and resolves CapD conditions:
// CapD { can_write: true, write_requires_lock: Some(42) }
// is_write_active(held_locks) = true when {42} ⊆ held_locks
```

### Atomics

VUMA provides atomic types such as `AtomicU64` with explicit memory ordering annotations. Atomic operations establish synchronization edges that the IVE uses to prove the absence of data races in lock-free code. The supported orderings are `Acquire`, `Release`, `AcqRel`, and `SeqCst`.

```vuma
struct Queue<T> {
    buffer: Address,
    capacity: u64,
    head: AtomicU64,    // producer write index
    tail: AtomicU64,    // consumer read index
}

fn queue_push(q: Address, value: u64) -> bool {
    head = (*q).head.load(Acquire);    // acquire: see all prior writes
    tail = (*q).tail.load(Acquire);

    if next_head % (*q).capacity == tail % (*q).capacity {
        return false;
    }

    slot = (*q).buffer + (head % (*q).capacity) * 8;
    *slot = value;

    (*q).head.store(next_head, Release);  // release: make value visible
    return true;
}

fn queue_pop(q: Address) -> Option<u64> {
    tail = (*q).tail.load(Acquire);
    head = (*q).head.load(Acquire);

    if tail % (*q).capacity == head % (*q).capacity {
        return None;
    }

    slot = (*q).buffer + (tail % (*q).capacity) * 8;
    value = *slot;

    (*q).tail.store(tail + 1, Release);
    return Some(value);
}
```

The IVE verifies lock-free code by:
- Checking that producer and consumer access non-overlapping slots (no data race)
- Verifying that Release ordering ensures the value write is visible before the index update
- Proving that Acquire ordering ensures the consumer sees the value before reading the slot
- Confirming that values read originated from valid writes (Origin invariant)

---

## 8. Memory Safety

Memory safety in VUMA is not optional — it is guaranteed by the Invariant Verification Engine (IVE), which checks every program against five core invariants. There is no `unsafe` keyword to bypass verification; instead, VUMA provides verification annotations and proof hints that help the IVE discharge proof obligations that would otherwise be too complex to verify automatically. This section describes the five invariants, the verification pipeline, and the annotation mechanisms available to programmers.

### The Five LIVE Invariants

The IVE checks the following invariants for every pointer operation in a VUMA program:

| Invariant | Meaning | Violation Example |
|---|---|---|
| **Liveness** | Every requested resource will eventually be provided | Use after free, null dereference |
| **Exclusivity** | At most one owner for exclusive resources | Data race (two concurrent writes) |
| **Interpretation** | Every read interprets data under the correct BD | Type confusion, reading a pointer as an integer |
| **Origin** | Every piece of data has a well-defined provenance | Reading uninitialized memory |
| **Cleanup** | Every acquired resource is eventually released | Memory leak, leaked lock |

### Verification Pipeline

The IVE operates through a multi-stage pipeline:

1. **Parsing:** Source code is parsed into an AST with full span information
2. **SCG Construction:** The AST is lowered into a Semantic Compute Graph (SCG) — a DAG where nodes represent operations and edges represent data flow, control flow, and security boundaries
3. **BD Inference:** The InferenceEngine walks the SCG and propagates BDs from leaf nodes, resolving composition at each node:
   - Sequential composition: forward-propagate the output BD
   - Parallel composition: intersect or unify BDs
   - Conditional composition: take the union of branch BDs
4. **Invariant Verification:** The VerificationEngine checks each of the five invariants against the annotated SCG
5. **Debt Tracking:** Verification obligations that cannot be discharged immediately are recorded as debt items with priority levels

```vuma
// After compilation, the IVE produces a verification summary:
// IVE verification: Liveness ✓, Exclusivity ✓, Interpretation ✓, Origin ✓, Cleanup ✓
```

### Verification Annotations

VUMA provides several annotation mechanisms to help the IVE:

**BD Annotations** (`#bd(Name)`): Explicitly specify a behavioral descriptor for a value or type. This overrides inferred BDs when the programmer has domain-specific knowledge:

```vuma
// Mark a buffer as read-only
buffer: #bd(ReadOnly) Address = allocate(1024);

// Mark a function as safe (manually verified)
#bd(Safe) fn trusted_operation() -> i32 { ... }
```

**Proof Hints**: Provide the IVE with additional information about why an operation is safe. Proof hints can be explicit casts, runtime checks, or formal proof steps:

```vuma
// Explicit cast provides proof of safe reinterpretation
let raw: u64 = ptr as u64;  // weakening: always safe

// Runtime check provides proof of safe strengthening
if tag == TYPE_PTR {
    let recovered: *i32 = raw as *i32;  // safe because tag was checked
}

// Formal proof (advanced)
// SafetyProof::FormalProof { steps: ["tag == TYPE_PTR implies raw is a valid pointer"] }
```

**Safe/Unsafe Blocks**: While VUMA has no `unsafe` escape hatch, the `safe` keyword can be used to assert that a block has been manually verified, creating a proof obligation that must be discharged:

```vuma
safe {
    // The programmer asserts this code is safe
    // IVE creates a proof obligation rather than checking automatically
    *raw_ptr = value;
}
```

### Verification Results

The IVE produces one of three verdicts for each invariant:

- **Proven:** The invariant holds on all execution paths (formal proof or exhaustive analysis)
- **ProbablySafe:** The invariant holds under stated assumptions (e.g., lock protection, runtime checks)
- **Violated:** A counterexample was found demonstrating the invariant can be broken

```vuma
// Example verification output:
// Liveness:     Proven (exhaustive analysis)
// Exclusivity:  ProbablySafe (2 conflicts protected by mutex locks)
// Interpretation: Proven (formal proof: 6 steps)
// Origin:       Proven (all reads trace to valid writes)
// Cleanup:      Proven (all allocations freed on all paths)
```

---

## 9. Standard Library Overview

VUMA's standard library provides essential data structures, memory management, I/O, formatting, mathematics, cryptography, string operations, concurrency, and filesystem primitives — all BD-annotated and verified by the IVE. The standard library is organized into modules that can be imported as needed.

### Memory Management

- **`allocate(size) -> Address`**: Reserve `size` bytes of memory. IVE verifies liveness and cleanup.
- **`free(ptr)`**: Release a memory region. IVE marks all derived pointers as dead.
- **`map_device(base, size) -> Address`**: Map a physical hardware address range into the program's address space. Implies volatile semantics; never needs to be freed.
- **`align_to(ptr, alignment) -> Address`**: Align a pointer to the given boundary. Part of the derivation chain.
- **Allocation strategies**: `GlobalAllocator`, `ArenaAllocator`, `BumpAllocator`, `PoolAllocator`, `FreeListAllocator` — each with BD-annotated allocation and deallocation.

```vuma
import "vuma:mem";

region pool = allocate(4096);
*pool = 0;
free(pool);
```

### Data Structures

- **`Queue<T>`**: Lock-free single-producer single-consumer ring buffer with `AtomicU64` indices
- **`Arena`**: Bump allocator with bulk invalidation on destroy
- **`NodeHeader`**: Doubly-linked list node with `prev`, `next`, and `data` fields
- **`Vec<T>`**: Growable array with BD-annotated push/pop/insert/remove
- **`HashMap<K, V>`**: Hash map using SipHash-1-3 with BD-tracked keys/values
- **`RingBuffer<T>`**: Fixed-capacity circular buffer

```vuma
import "vuma:ds";

q = queue_new(256);
queue_push(q, 42);
value = queue_pop(q);  // Some(42)
```

### Concurrency

- **`AtomicU64`**: 64-bit atomic integer with `load(ordering)`, `store(value, ordering)`, `compare_exchange(expected, desired, success_order, failure_order)`
- **`channel<T>`**: Typed message-passing channel with `send` and `recv`
- **`lock(id)` / `unlock(id)`**: Mutual exclusion primitives
- **`Mutex`**: BD-annotated mutual exclusion with `MutexGuard` RAII pattern
- **`RwLock`**: Reader-writer lock with separate read/write guards
- **`Barrier`**: Synchronization barrier for multi-threaded coordination

```vuma
import "vuma:sync";

counter = AtomicU64::new(0);
counter.store(1, Release);
val = counter.load(Acquire);
```

### I/O

- **`print(value)`**: Output a value to the standard output
- **`delay_ms(ms)`**: Busy-wait for the specified number of milliseconds (bare-metal)
- **`VumaReader` / `VumaWriter`**: Core I/O traits with UART and file backends
- **`VumaBufReader` / `VumaBufWriter`**: Buffered I/O wrappers
- **`read_bytes` / `write_bytes`**: Low-level syscall wrappers
- **`read_u32_le` / `write_u32_le`**: Little-endian u32 byte access

```vuma
import "vuma:io";

print((*current).data);
delay_ms(500);
```

### String Formatting (fmt)

The `fmt` module provides printf-style string formatting for VUMA programs:

- **`format_int(value, base, width)`**: Format a signed integer in the given base (2–36) with zero-padding
- **`format_uint(value, base, width)`**: Format an unsigned integer
- **`format_float(value, precision)`**: Format a floating-point number with configurable precision
- **`format_hex(value, width)`**: Format as hexadecimal
- **`format_binary(value, width)`**: Format as binary
- **`format_octal(value, width)`**: Format as octal
- **`format_pointer(addr)`**: Format an address as a hex pointer
- **`pad_left(s, width, fill)`** / **`pad_right(s, width, fill)`**: String padding
- **`join(strings, separator)`**: Join multiple strings with a separator
- **`write_str(buf, offset, s)`** / **`write_int(buf, offset, value, base)`** / **`write_float(buf, offset, value, precision)`**: Write formatted output into byte buffers

```vuma
import "vuma:fmt";

hex_str = format_hex(0xDEADBEEF, 8);  // "DEADBEEF"
padded = pad_left("42", 8, '0');         // "00000042"
```

### Mathematics (math)

The `math` module provides comprehensive mathematical utilities:

- **Integer arithmetic**: `abs`, `min`, `max`, `clamp`
- **Trigonometric (f64)**: `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `sinh`, `cosh`, `tanh`
- **Exponential/Logarithmic (f64)**: `sqrt`, `cbrt`, `exp`, `exp2`, `exp_m1`, `ln`, `log2`, `log10`, `ln_1p`, `pow`, `powi`
- **Rounding (f64)**: `floor`, `ceil`, `round`, `trunc`, `fract`
- **Comparison (f64)**: `min_of`, `max_of`
- **Classification (f64)**: `is_nan`, `is_infinite`, `is_finite`, `is_normal`, `signum`, `copysign`
- **Constants**: `PI`, `TAU`, `E`, `LN_2`, `LN_10`, `LOG2_E`, `LOG10_E`, `SQRT_2`, `FRAC_1_SQRT_2`
- **f32 variants**: All floating-point functions have `_f32` suffixed counterparts (e.g., `sin_f32`, `sqrt_f32`)
- **f32 constants**: `PI_F32`, `TAU_F32`, `E_F32`, etc.

```vuma
import "vuma:math";

val: f64 = sin(PI / 4.0);     // ≈ 0.7071
root: f64 = sqrt(2.0);        // ≈ 1.4142
clamped: i64 = clamp(x, 0, 100);
```

### Cryptography (crypto)

The `crypto` module provides SHA-256 constants and logical functions, plus constant-time operations:

- **SHA-256 helpers**: `sha256_ch`, `sha256_maj`, `sha256_big_sigma0`, `sha256_big_sigma1`, `sha256_small_sigma0`, `sha256_small_sigma1`
- **Byte access**: `sha256_read_u32_be`, `sha256_write_u32_be`
- **Constant-time operations**: `ct_select_u32`, `ct_eq_u32`, `ct_ne_u32`, `ct_lt_u32`, `ct_gte_u32` — branchless implementations across all 8 backends
- **SHA-256 constants**: `SHA256_K` (64 round constants), `SHA256_H` (8 initial hash values)

```vuma
import "vuma:crypto";

// Constant-time comparison (no data-dependent branches)
equal: u32 = ct_eq_u32(a, b);
selected: u32 = ct_select_u32(equal, a, b);
```

### String Operations

- **`strlen(ptr)`**: Compute the length of a null-terminated string
- **`strcmp(a, b)`**: Compare two null-terminated strings
- **`memcpy(dst, src, len)`**: Copy `len` bytes from `src` to `dst`
- **`memset(ptr, value, len)`**: Fill `len` bytes with `value`

### Type Operations

- **`sizeof(Type) -> u64`**: Return the size in bytes of a type
- **`alignof(Type) -> u64`**: Return the alignment requirement of a type

```vuma
node_size = sizeof(NodeHeader);   // 24
node_align = alignof(NodeHeader); // 8
```

### Option Type

The `Option<T>` type represents an optional value. It is used extensively for operations that may fail (e.g., `queue_pop` when the queue is empty):

```vuma
result = queue_pop(q);
match result {
    Some(value) => process(value),
    None => handle_empty(),
}
```

### Additional Modules

The standard library also includes:

- **`env`**: Environment variable access
- **`error`**: Error type definitions and BD-annotated error handling
- **`fs`**: Filesystem operations with capability-based access control
- **`path`**: Path manipulation and normalization
- **`process`**: Process management (exit, spawn, etc.)
- **`net`**: Network I/O (TCP, UDP sockets)
- **`thread`**: Thread creation and management
- **`time`**: Time measurement and duration types

---

## 10. FFI and External Functions

VUMA supports calling external C functions and Linux syscalls through `extern "C"` blocks. This enables interoperability with existing C libraries and direct system call access across all 8 backend architectures.

### Extern Block Syntax

The `extern "C" { ... }` block declares external functions that are resolved at link time:

```vuma
extern "C" {
    fn write(fd: i64, buf: Address, count: i64) -> i64;
    fn read(fd: i64, buf: Address, count: i64) -> i64;
    fn exit(code: i64);
}

fn main() -> i32 {
    write(1, message, 13);
    return 0;
}
```

Functions declared in `extern` blocks have the `is_extern` flag set in the SCG. During code generation, extern function calls emit relocations instead of local `BL` instructions, allowing the linker to resolve them against shared libraries.

### Supported Syscalls

VUMA provides FFI bindings for 19 Linux syscalls across all 8 architectures:

`read`, `write`, `open`, `close`, `exit`, `mmap`, `munmap`, `brk`, `ioctl`, `fcntl`, `getpid`, `clone`, `futex`, `pipe`, `dup`, `dup2`, `wait4`, `rt_sigaction`, `rt_sigprocmask`

Each syscall has architecture-specific relocations for all backends.

### DWARF Debug Information

When compiled with `--debug-info`, VUMA generates DWARF v4 debug information sections for all 8 backends. The emitted sections include:

| Section | Contents |
|---------|----------|
| `.debug_abbrev` | Abbreviation tables (tag + attribute encodings) |
| `.debug_info` | Compilation unit DIEs (subprograms, variables) |
| `.debug_line` | Line-number program (DWARF standard opcodes) |
| `.debug_frame` | Call frame information (CIE + FDE entries) |

The DWARF builder is parameterized by address size to support all 8 backends (8-byte addresses for 64-bit targets, 4-byte for ARM32 and Wasm32).

---

## 11. Platform-Specific Features

VUMA targets 8 backend architectures with AArch64 as the primary platform. This section describes platform-specific features, with detailed AArch64 bare-metal support.

### Device Memory Mapping

The `map_device(base, size)` intrinsic maps a physical address range into the program's virtual address space. On AArch64, this is used to access hardware peripherals such as GPIO, UART, and DMA controllers. The IVE treats mapped device regions specially:

- They never need to be freed (hardware is always present)
- They have volatile semantics (reads and writes have side effects)
- They have a known size for bounds checking

```vuma
const GPIO_BASE: Address = 0x7e200000;  // AArch64 GPIO base (BCM address)

fn gpio_set_output(pin: u32) {
    gpio = map_device(GPIO_BASE, 4096);
    fsel = gpio + 0x00;  // GPFSEL0 register
    *fsel = (*fsel & ~(7 << (pin * 3))) | (1 << (pin * 3));
}

fn gpio_set(pin: u32) {
    gpio = map_device(GPIO_BASE, 4096);
    *(gpio + 0x1c) = 1 << pin;  // GPSET0 register
}

fn gpio_clear(pin: u32) {
    gpio = map_device(GPIO_BASE, 4096);
    *(gpio + 0x28) = 1 << pin;  // GPCLR0 register
}
```

### GPIO Register Layout

The AArch64 GPIO peripheral is memory-mapped at BCM address `0x7e200000`. The key register offsets are:

| Register | Offset | Purpose |
|---|---|---|
| GPFSEL0 | 0x00 | Function select for pins 0-9 |
| GPFSEL1 | 0x04 | Function select for pins 10-19 |
| GPFSEL2 | 0x08 | Function select for pins 20-29 |
| GPSET0 | 0x1c | Set pin high |
| GPCLR0 | 0x28 | Set pin low |
| GPLEV0 | 0x34 | Pin level read |

Each pin uses 3 bits in the function select register: `000` = input, `001` = output.

### ARM64 Code Generation

VUMA compiles to native ARM64 machine code. The code generator targets the Cortex-A76 (AArch64's SoC) and produces optimized instruction sequences for:
- Pointer arithmetic using register-offset addressing
- Atomic operations using `LDXR`/`STXR` (exclusive access) and `LDA`/`STL` (acquire/release)
- Volatile device access using explicit load/store instructions (no optimization)
- Stack frame layout aligned to 16-byte boundaries per AAPCS64

### Bare-Metal Execution

VUMA programs can run bare-metal on the AArch64 without an operating system. The runtime provides:
- **Startup code:** Sets up the stack pointer, clears BSS, calls `main()`
- **Memory allocator:** Simple bump allocator for the `allocate()` primitive
- **Delay loops:** `delay_ms()` uses the AArch64's system timer at `0x7e003000`
- **UART output:** `print()` writes to the AArch64's AUX UART for console output

```vuma
// Complete bare-metal LED blink program for AArch64
const GPIO_BASE: Address = 0x7e200000;

fn main() -> i32 {
    led_pin: u32 = 25;
    gpio_set_output(led_pin);
    loop {
        gpio_set(led_pin);
        delay_ms(500);
        gpio_clear(led_pin);
        delay_ms(500);
    }
    return 0;
}
```

### Const Addresses

Hardware register addresses are declared as `const` values with the `Address` type. The IVE treats these as known constants and can verify at compile time that register accesses stay within mapped device regions:

```vuma
const GPIO_BASE: Address = 0x7e200000;
const GPFSEL0_OFFSET: Address = 0x00;
const GPSET0_OFFSET: Address = 0x1c;
const GPCLR0_OFFSET: Address = 0x28;
```

### Multi-Backend Support

VUMA supports 8 compilation targets with a unified `Backend` trait:

| Backend | Endianness | Pointer Width | Output Format |
|---------|-----------|---------------|---------------|
| AArch64 | Little | 64-bit | ELF64 |
| x86_64 | Little | 64-bit | ELF64 |
| RISC-V 64 | Little | 64-bit | ELF64 |
| ARM32 | Little | 32-bit | ELF32 |
| MIPS64 | Big | 64-bit | ELF64 |
| PPC64 | Big | 64-bit | ELF64 |
| LoongArch64 | Little | 64-bit | ELF64 |
| Wasm32 | Little | 32-bit | Wasm |

All 6 native backends (x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64) pass the full SHA256d execution test. LoongArch64 passes individual operation tests. Wasm32 generates valid modules.

---

## 12. Appendix: Keyword and Operator Quick Reference

### Complete Keyword List

| Keyword | Category | Purpose |
|---|---|---|
| `fn` | Core | Function definition |
| `let` | Core | Variable binding |
| `if` | Core | Conditional |
| `else` | Core | Conditional alternative |
| `while` | Core | Conditional loop |
| `for` | Core | Iterator loop |
| `loop` | Core | Infinite loop |
| `return` | Core | Function return |
| `as` | Core | Type cast |
| `match` | Core | Pattern matching |
| `struct` | Core | Struct definition |
| `enum` | Core | Enum definition |
| `ptr` | Memory | Pointer type annotation |
| `region` | Memory | Named region declaration |
| `alloc` | Memory | Allocation (short form) |
| `allocate` | Memory | Allocation (full form) |
| `free` | Memory | Deallocation |
| `derive` | Memory | Pointer derivation |
| `cast` | Memory | Explicit cast statement |
| `read` | Memory | Read access annotation |
| `write` | Memory | Write access annotation |
| `sync` | Concurrency | Synchronization block |
| `async` | Concurrency | Async block |
| `await` | Concurrency | Await async result |
| `spawn` | Concurrency | Spawn concurrent task |
| `lock` | Concurrency | Acquire mutex |
| `unlock` | Concurrency | Release mutex |
| `channel` | Concurrency | Channel creation |
| `send` | Concurrency | Channel send |
| `recv` | Concurrency | Channel receive |
| `unsafe` | Safety | Reserved (no escape hatch) |
| `safe` | Safety | Manual safety assertion |
| `bd` | BD | Behavioral descriptor directive |
| `repd` | BD | Representation descriptor |
| `capd` | BD | Capability descriptor |
| `reld` | BD | Relational descriptor |
| `import` | Module | Import declaration |
| `export` | Module | Export declaration |
| `mod` | Module | Module definition |
| `use` | Module | Use declaration |
| `self` | Module | Current module reference |
| `super` | Module | Parent module reference |
| `true` | Literal | Boolean true |
| `false` | Literal | Boolean false |
| `sizeof` | Type | Size-of operator |
| `alignof` | Type | Alignment-of operator |

### Operator Precedence (highest to lowest)

| Precedence | Operators | Associativity |
|---|---|---|
| 1 (highest) | `*` (dereference) `@` (address-of) `-` (negate) `!` `~` | Unary, right-to-left |
| 2 | `as` | Left-to-right |
| 3 | `*` `/` `%` | Left-to-right |
| 4 | `+` `-` | Left-to-right |
| 5 | `<<` `>>` | Left-to-right |
| 6 | `&` (bitwise AND) | Left-to-right |
| 7 | `^` | Left-to-right |
| 8 | `\|` (bitwise OR) | Left-to-right |
| 9 | `==` `!=` `<` `<=` `>` `>=` | Left-to-right |
| 10 | `&&` | Left-to-right |
| 11 | `\|\|` | Left-to-right |
| 12 (lowest) | `=` | Right-to-left |

---

*This language reference was generated from the VUMA parser AST, lexer token definitions, IVE verification engine, and behavioral descriptor subsystem source code. For implementation details, see the `vuma-parser`, `vuma-ive`, and `vuma-bd` crates.*
