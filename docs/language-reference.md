# VUMA 2.0 Language Reference

> **VUMA is PMT-only.** There are no pointers. Memory safety is a
> type-checking property, not a verification problem. Every program is a
> composition of typed state transforms over a single memory buffer.

This document is the canonical reference for the VUMA 2.0 *language*.
For the compiler that implements it see
[`architecture.md`](./architecture.md); for the kernel written in it
see [`kernel-architecture.md`](./kernel-architecture.md).

---

## Table of Contents

1. [Introduction](#1-introduction)
2. [Lexical Structure](#2-lexical-structure)
3. [Types](#3-types)
4. [Layouts](#4-layouts)
5. [States](#5-states)
6. [Field Access](#6-field-access)
7. [Transforms](#7-transforms)
8. [Arena States](#8-arena-states)
9. [Functions](#9-functions)
10. [Control Flow](#10-control-flow)
11. [Operators](#11-operators)
12. [Atomic Operations](#12-atomic-operations)
13. [FFI](#13-ffi)
14. [Pointers Are Not Supported](#14-pointers-are-not-supported)
15. [Verification](#15-verification)
16. [Complete Examples](#16-complete-examples)
17. [VUMA Parser Limitations](#17-vuma-parser-limitations)

---

## 1. Introduction

VUMA (Verified-Unsafe Memory Access) 2.0 is a systems programming
language built on **Programs as Memory Transformations (PMT)**. A
program is not "code that allocates and uses memory" — it is a
sequence of typed memory-state transformations. The compiler proves
each transformation is valid at compile time. Memory safety is free:
it is a structural property of the type system, not a runtime check or
a proof obligation.

Pointer syntax (`allocate`, `free`, `*ptr`, `&x`, `*T`) does not exist
in VUMA 2.0. Attempting to use it produces a **hard parse error** (the
lexer rejects the token before the type checker ever runs). All memory
access is through typed state fields with compile-time-verified
offsets. The runtime never allocates or frees per-object — the
program-wide buffer is sized once from the union of all states, and
each `state_new(Layout)` lowers to an `Offset` into that one buffer.

The PMT model has four constructs:

- `layout` — a typed record describing the byte shape of a buffer.
- `State<T>` — a typed view of the program-wide memory buffer at a
  specific offset.
- `state_new(Layout)` — the sole allocation primitive; produces a
  `State<T>` whose buffer slot is reserved at compile time.
- `transform` — a pure function `State<T> -> State<U>` that consumes
  its input and produces a new state.

Pointers are eliminated because they collapse memory safety into a
runtime problem. With pointers, the compiler must prove five
invariants over an unbounded heap graph: **liveness** (no
use-after-free), **exclusivity** (no aliasing during mutation),
**cleanup** (no double-free), **origin** (no uninitialized reads), and
**interpretation** (no type-punning). VUMA replaces that graph with a
fixed set of structural type checks on a single typed buffer — the
**5→3 invariant collapse**. See [`architecture.md` §1](./architecture.md#1-overview)
for the full table.

---

## 2. Lexical Structure

### 2.1 Keywords

VUMA's keywords (subset relevant to PMT; the full lexer in
[`vuma-parser/src/lexer.rs`](../src/parser/src/lexer.rs) recognises
more, but only the following have meaning in VUMA 2.0 PMT source):

```
layout  State  state_new  transform  fn  let  if  else  while  for
return  break  continue  as  true  false  extern  atomic_load
atomic_store  atomic_cas  arena_new  arena_alloc  arena_grow  arena_free
```

`Address` is a type name (not a keyword); `bool`, `u8`..`u64`, `i8`..`i64`,
and `void` are base type names (also not keywords — they are
identifiers recognised by the type parser).

### 2.2 Identifiers

Start with a letter or underscore (`_`), followed by letters, digits,
or underscores. Case-sensitive. By convention:

- **snake_case** for functions, fields, variables.
- **PascalCase** for layout names (`Point`, `Stack`, `InodeTable`).

The parser accepts both, but the kernel convention is snake_case for
values and PascalCase for type-level names (matches Rust).

### 2.3 Literals

#### Integer literals

```
42          // integer literal (i64 by default)
255u8       // u8 literal
1000000u32  // u32 literal
4294967295  // u64 literal (i64 representation, used as u64 by context)
```

A decimal integer literal with no suffix is `i64`. A suffix (`u8`,
`u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`) overrides the type.
Hex literals are supported (`0xFF`, `0xDEADBEEF`) — see §17.7 for the
width-extension subtlety that makes the kernel prefer decimal in
self-tests.

**Negative literals:** the lexer's signed-literal path occasionally
produces surprising values at the 64-bit boundary. The convention is
to write negative numbers as `0 - N` (e.g. `0 - 1` for `-1`,
`0 - 38` for `-ENOSYS`). See §17.6.

#### Boolean literals

```
true
false
```

A `bool` value is 1 byte; `true` is `1`, `false` is `0`.

#### Address literals

```
0xDEADBEEF        // hex address literal
0x7e200000         // GPIO_BASE on a Raspberry Pi
0                  // null address (must be cast: 0 as Address)
```

A hex literal `0x...` is lexed as an `Address` literal (a `u64`
tagged as `Lit::Address`). It can be passed directly to `extern "C"`
fns that take `Address` parameters.

### 2.4 Comments

```
// Single-line comment. Everything to end of line is ignored.
// No block comments in PMT — use multiple // lines.
```

### 2.5 Punctuation

```
{ } ( ) [ ] ; , : -> = == != < > <= >=
+ - * / % & | ^ << >> && || ! ~ @ .
```

`@` and `*` are reserved — they appear in the lexer for legacy
pointer syntax and produce hard parse errors if used in VUMA 2.0.

---

## 3. Types

### 3.1 Base types

| Type    | Size     | Range                                                        |
|---------|----------|--------------------------------------------------------------|
| `u8`    | 1 byte   | 0..255                                                       |
| `u16`   | 2 bytes  | 0..65535                                                     |
| `u32`   | 4 bytes  | 0..4294967295                                                |
| `u64`   | 8 bytes  | 0..18446744073709551615                                      |
| `i8`    | 1 byte   | -128..127                                                    |
| `i16`   | 2 bytes  | -32768..32767                                                |
| `i32`   | 4 bytes  | -2147483648..2147483647                                      |
| `i64`   | 8 bytes  | -9223372036854775808..9223372036854775807                    |
| `bool`  | 1 byte   | `true` (1) / `false` (0)                                     |
| `void`  | 0 bytes  | unit type — only valid as a function return type             |
| `Address` | 8 bytes | a raw C pointer; produced by `state as Address` or `0x..` literal |

The default integer type (when no suffix and no type annotation is
present) is `i64`. The `Address` type carries the same bit pattern as
`u64` but is type-distinct — it can only be produced by a cast
(`state as Address`, `0 as Address`, `n as Address`) and is the only
type accepted by `extern "C"` parameters declared `Address`.

### 3.2 Array types

```
[u8; 16]     // 16-byte array, 16 bytes total
[u32; 8]     // 8-element u32 array, 32 bytes total
[u64; 4]     // 4-element u64 array, 32 bytes total
```

Array types are always fixed-size. There is no slice type, no
`Vec<T>`, no dynamically-sized array. Arrays appear as layout fields;
see §4.4 for the byte-granular indexing limitation on non-`u8`
element types.

### 3.3 `State<T>` types

```
State<Point>       // a typed view of a buffer interpreted as a Point layout
State<Buffer>      // a typed view interpreted as a Buffer layout
State<DbHandle>    // a typed view of a #[foreign(raw)] layout
```

A `State<T>` is a typed view of the program's memory buffer at a
specific offset. The layout `T` determines how the buffer bytes are
interpreted at that program point. States are **linear**: they have
exactly one owner and are consumed when passed to a transform (or to
an FFI close-call, or cast to `Address` — see §7, §13).

`State<T>` may appear as:

- A function parameter type (`fn f(s: State<Point>)`).
- A function return type (`fn f() -> State<Point>`) — but see §17.3
  for the State-return limitation.
- A `transform`'s input and output types.
- The operand of an `as Address` cast at an FFI boundary.

A `State<T>` cannot be stored in a layout field, cannot appear inside
an array, and cannot be cast to a non-`Address` type.

### 3.4 Layout types

Layouts are defined at module scope (see §4). A layout name can be
used as a type parameter for `State<T>` and as a field type inside
another layout (nested layouts — see §4.3).

---

## 4. Layouts

A layout defines the byte-level structure of a memory state. Fields
are laid out sequentially with natural alignment padding. The compiler
resolves layout sizes via a multi-pass size resolver (see
[`architecture.md` §11](./architecture.md#11-nested-layout-resolution))
so nested and forward-referenced layouts both work.

### 4.1 Syntax

```
layout Name = { field1: Type, field2: Type, ... }
```

A layout may carry the `#[foreign(raw)]` attribute (see §13.4) when it
wraps an opaque C pointer:

```
#[foreign(raw)]
layout DbHandle = { raw: u64 }
```

### 4.2 Example

```
layout Point = { x: u32, y: u32 }
layout Line = { a: Point, b: Point }
layout Buffer = { count: u32, data: [u8; 256] }
layout Stack = { count: u32, data: [u32; 8] }
```

### 4.3 Field offset computation

Fields are placed sequentially. Each field is aligned to its natural
alignment (1 for `u8`/`bool`, 2 for `u16`/`i16`, 4 for `u32`/`i32`,
8 for `u64`/`i64`/`Address`, and the max field alignment for nested
layouts). Padding is inserted between fields as needed. The total size
is rounded up to the layout's alignment.

```
layout Example = { a: u8, b: u32 }
// a at offset 0 (1 byte), padding 3 bytes, b at offset 4 (4 bytes)
// total size: 8 bytes (rounded up from 5 to alignment 4)
```

### 4.4 Nested layouts

A layout field can be another layout type. The nested layout's fields
are inlined at the computed offset. The field-chain resolver descends
into nested layouts so `l.a.x` resolves to a cumulative byte offset.

```
layout Line = { a: Point, b: Point }
// a at offset 0 (8 bytes: a.x@0, a.y@4)
// b at offset 8 (8 bytes: b.x@8, b.y@12)
// total size: 16 bytes
```

`l.a.x` accesses offset `0 [a in Line] + 0 [x in Point] = 0`.
`l.b.y` accesses offset `8 [b in Line] + 4 [y in Point] = 12`.

### 4.5 Array fields

```
layout Stack = { count: u32, data: [u32; 8] }
// count at offset 0 (4 bytes)
// data at offset 4 (32 bytes: data[0]@4, data[1]@8, ..., data[7]@32)
// total size: 36 bytes
```

For `[u8; N]` arrays, indexed access `data[i]` lowers to a `Load` at
offset `field_offset + i`. For `[u16; N]`, `[u32; N]`, `[u64; N]`
arrays, indexed access has a known limitation — see §17.4. The
workaround is to store the array as a flat `[u8; N * width]` and
pack/unpack with shift-and-mask helpers.

### 4.6 Alignment

The layout's alignment is the maximum alignment of any of its fields.
The total size is rounded up to a multiple of the alignment so arrays
of the layout are correctly strided. There is no `#[repr(packed)]` or
`#[repr(align(N))]` — natural alignment is always used.

---

## 5. States

### 5.1 `State<T>` and `state_new`

```
let p = state_new(Point);
```

`state_new(Layout)` creates a new `State<Layout>`. The compiler
allocates buffer space in the single program-wide buffer
(`___pmt_buffer`) and assigns a fixed offset. **No runtime allocation
occurs** — the buffer is pre-allocated at program start, sized to the
sum of all state sizes used by the program. Each `state_new` lowers to
an `Offset` into `___pmt_buffer`.

### 5.2 State lifetime

States are **linear**: they have exactly one owner. A state is
consumed when:

- Passed to a `transform` (the transform takes ownership).
- Cast to `Address` at an FFI boundary with `#[foreign_consume]`
  (e.g. `sqlite3_close(db)`).
- Passed to `arena_free` (for arena states).

A state **cannot** be used after being consumed — any subsequent read
or write is a `linearity violation` rejected by the `StateWriteVerifier`.

A state goes out of scope at the end of its enclosing function. Its
slot in `___pmt_buffer` is reclaimed with the single buffer; there is
no `drop`, no `free`, and no per-state deallocation.

### 5.3 State parameters

```
fn get_x(p: State<Point>) -> i32 {
    return p.x;
}
```

`State<T>`-typed parameters are passed by reference (the buffer
pointer is passed in a register). The callee can read and write fields
through the parameter. The state is **not** consumed by the call (the
caller retains ownership) unless the function is a `transform`
(§7) or the parameter's layout is `#[foreign(raw)]` and the extern fn
is `#[foreign_consume]` (§13.4).

### 5.4 The init-style API pattern

The codegen does not propagate `State<T>`-typedness through function
return values (§17.3). The canonical workaround is the **init-style
API**: the caller allocates the state with `state_new(Layout)` (which
marks the binding as state-typed) and passes it by reference to a
function that populates it in place.

```
// Caller allocates:
let pmm = state_new(PmmState);
let pool = state_new(FlatPool);
// Caller passes by reference:
pmm_init(pool, pmm, mem_start, mem_size);
// Caller reads fields back:
let page = pmm_alloc(pmm, order);     // returns u64 page-frame address
```

This pattern is used by every stateful kernel subsystem. A future
code-change will fix the codegen to propagate `State` through returns;
until then, init-style is the canonical convention.

---

## 6. Field Access

### 6.1 Read

```
let v = p.x;              // read field x from state p
let first = buf.data[0];  // read first element of array field
let n = buf.count;        // read the count field
```

A `state.field` read lowers to an IR `Load { addr, offset, size }`
where `offset` and `size` are compile-time constants from the
`LayoutRegistry`. The IVE `StateReadVerifier` proves the field exists,
`offset + size ≤ layout.total_size`, and the read type matches the
field's declared type.

### 6.2 Write

```
p.x = 42;              // write 42 to field x of state p
buf.data[0] = 65;      // write 65 to first element of array field
buf.count = buf.count + 1;   // increment the count field
```

A `state.field = value` write lowers to an IR
`Store { addr, offset, value, size }`. The `StateWriteVerifier`
performs the same three checks as `StateReadVerifier` plus a
**linearity** check: the state must not be in `consumed_vars`.

### 6.3 Nested access

```
let line = state_new(Line);
line.a.x = 10;         // write to nested field
let v = line.b.y;      // read from nested field
```

The field-chain resolver descends into nested layouts, summing offsets.
`line.a.x` resolves to `0 [a in Line] + 0 [x in Point] = 0`;
`line.b.y` resolves to `8 [b in Line] + 4 [y in Point] = 12`. Both
reads and writes can chain arbitrarily deep, limited only by the
layout nesting.

### 6.4 Array element access

```
let stack = state_new(Stack);
stack.count = 0;
stack.data[stack.count] = 42;   // write at runtime index
stack.count = stack.count + 1;
let val = stack.data[0];        // read at constant index
```

The index expression is multiplied by the element size at bridge time
and added to the field offset. For runtime indices, the
`StateTransform` verifier proves `offset + (count * elem_size) ≤
layout_size` using linear arithmetic (Presburger-decidable — see
[`architecture.md` §5.5](./architecture.md#55-dependent-array-proof-obligation)).

### 6.5 The byte-granular indexing limitation

For `[u8; N]` arrays, indexed access works correctly. For `[u16; N]`,
`[u32; N]`, or `[u64; N]` arrays, the indexed-access path goes
through a code path that the IVE verifiers don't fully understand —
accesses compile but read back wrong values. The workaround is to
store the array as a flat `[u8; N * width]` and pack/unpack with
shift-and-mask helpers (see §17.4).

---

## 7. Transforms

A `transform` is a pure function from one state layout to another. It
**consumes** its input state (linear ownership transfer) and produces
a new state of the output layout.

### 7.1 Syntax

```
transform name(s: State<InputLayout>) -> State<OutputLayout> {
    // body: read from s, write to a fresh output state
}
```

The body is a regular block. Inside, `s` is a `State<InputLayout>`
that can be read and (if the input and output layouts alias) written.
A `return;` (with no value) is the typical body close — the transform
produces its `State<OutputLayout>` value implicitly from the consumed
input.

### 7.2 Layout compatibility

The `StateTransformVerifier` classifies every transform into one of
three kinds:

- **Identity** — `in_layout == out_layout`. The buffer is returned
  unchanged. The e-graph's `state_transform_elision` rule rewrites
  the transform away at O2.
- **Reinterpret** — `in.total_size == out.total_size` (but layouts
  differ). The buffer is reinterpreted in place at zero cost.
- **Copy** — different sizes. The compiler emits a fresh `Alloc` of
  `out.total_size` and a `Store`-by-`Store` copy of the source buffer.

```
layout Request  = { opcode: u32, arg: u32 }    // 8 bytes
layout Response = { status: u32, result: u32 } // 8 bytes

transform handle(req: State<Request>) -> State<Response> {
    // Request and Response are both 8 bytes → Reinterpret.
    // The buffer's bytes are reinterpreted in place.
    return;
}
```

### 7.3 Calling a transform

```
let req = state_new(Request);
req.opcode = 1;
req.arg = 42;
let resp = handle(req);     // req is consumed; resp is State<Response>
// req.opcode = 2;          // LINEARITY VIOLATION — req was consumed
```

After the call, the input variable is consumed (it is added to
`consumed_vars`). Any subsequent read or write to it is a linearity
violation rejected by `StateWriteVerifier`. The result is a fresh
`State<OutputLayout>` whose buffer slot is at the same offset (for
Identity/Reinterpret) or at a newly-allocated offset (for Copy).

### 7.4 Linearity after consume

Once a state is consumed by a transform (or by an FFI close-call, or
by `arena_free`), it cannot be read, written, or passed to another
transform. The only operation permitted on a consumed variable is to
let it go out of scope.

```
let p = state_new(Point);
p.x = 10;
let q = id(p);          // p consumed by transform `id`
// p.x = 20;            // linearity violation
// let r = id(p);       // linearity violation
return q.x;             // 10 — q is the new state
```

There is no `clone` or `copy` for states. If you need two copies, you
allocate two states and copy the fields explicitly.

### 7.5 The transform-1-param limitation

The `StateTransform` verifier requires that an `as Address` cast (the
FFI-boundary form of transform) appears in a context where the
`State<T>` is the function's parameter. Casting a state created inside
the function (`let s = state_new(...); let a = s as Address; ...`)
sometimes trips the verifier. The workaround is to **always pass states
from the caller**: even a helper that just needs a scratch buffer
takes it as a parameter (§17.5).

---

## 8. Arena States

Runtime-growable memory without pointers is provided by the arena
state model. The arena surface has four primitives that lower to
`mmap`/`mremap`/`munmap`:

| Primitive                | Effect                                                            |
|--------------------------|-------------------------------------------------------------------|
| `arena_new(capacity)`    | `mmap` a region of `capacity` bytes; return `State<Arena>`.       |
| `arena_alloc(a, L)`      | Bump `a.offset` by `sizeof(L)`; bounds-check; return `State<L>`.  |
| `arena_grow(a, min_cap)` | `mremap` the region to at least `min_cap` bytes.                  |
| `arena_free(a)`          | `munmap` the arena's region. Consumes the arena.                  |

### 8.1 The Arena layout

The runtime treats the arena's first 24 bytes as a header:

```
arena_ptr  ┌──────────────────────────────────────────────────┐
           │  [ptr+0]   base address (== arena_ptr itself)    │
           │  [ptr+8]   current bump offset                   │
           │  [ptr+16]  capacity  (the mmap'd region size)    │
           ├──────────────────────────────────────────────────┤
           │  … arena data starts here at offset 24 …         │
           ▼                                                  │
          bump offset ──────►  (grows toward capacity)        │
           ───────────────────────────────────────────────────│
           │  unmapped / overflow zone                        │
           └──────────────────────────────────────────────────┘
```

### 8.2 `arena_new`

```
let arena = arena_new(4096);     // State<Arena>, 4096-byte mmap'd region
```

`arena_new` calls `mmap(NULL, capacity, PROT_READ|PROT_WRITE,
MAP_PRIVATE|MAP_ANONYMOUS, -1, 0)` and returns the resulting pointer
as a `State<Arena>`. The arena's header is initialised with
`base = arena_ptr`, `offset = 24`, `capacity = capacity`.

### 8.3 `arena_alloc`

```
let widget = arena_alloc(arena, Widget);   // State<Widget>
```

`arena_alloc(a, L)` is **linear** (it consumes and re-emits the
arena). It bump-allocates `sizeof(L)` bytes inside the arena, performs
a runtime bounds check, and returns a fresh `State<L>` whose base
address is `arena_ptr + offset`. Field access on the result is then
identical to a `state_new`'d state — same `Load`/`Store` lowering,
same IVE checks.

### 8.4 `arena_grow`

```
let arena = arena_grow(arena, 16384);   // grow to at least 16384 bytes
```

`arena_grow(a, min_capacity)` calls `mremap(a.base, a.capacity,
min_capacity, MREMAP_MAYMOVE)` (Linux) to expand the region,
preserving contents. The arena's `capacity` field is updated; the
bumped offset is unchanged. Existing allocations remain valid.

### 8.5 `arena_free`

```
arena_free(arena);    // munmap the region; arena is consumed
```

`arena_free(a)` calls `munmap(a.base, a.capacity)` and marks the
arena consumed. There is **no per-object free**; the only
deallocation site in the entire language is `arena_free` on a whole
arena.

### 8.6 Bounds checking

Every `arena_alloc` site emits a runtime bounds check:

```
new_offset = offset + layout_size
if new_offset > capacity:        // unsigned compare
    call __arena_overflow(layout_id)   // exits(1) / halts CPU
store(new_offset, [arena_ptr + 8])
return arena_ptr + offset
```

`__arena_overflow` is defined on all 19 backends as a trap
instruction (`ud2` on x86_64, `brk #0` on aarch64, `unimp` on
riscv64, etc.). On hosted x86_64 it surfaces as a non-zero exit code;
on bare metal it halts the CPU. The IVE `arena_bounds` verifier
checks the static side — no arena variable is accessed after
`arena_free` consumes it.

### 8.7 The PMT-level Arena surface

The library-level PMT surface (`womb/alloc/arena.vuma`) mirrors the
runtime but uses a typed `[u8; N]` byte array as the arena's data
store. `arena_alloc` returns a **logical byte offset** (a `u32` handle
into `arena.data`), not a raw address — callers access bytes via
typed array indexing (`arena.data[off]`), never via pointer
arithmetic. This is the form user programs use when they want a
growable byte buffer without leaving the typed-state discipline.

---

## 9. Functions

### 9.1 Syntax

```
fn name(param1: Type, param2: Type) -> ReturnType {
    // body
}
```

A function with no return value uses `-> void` or omits the return
type entirely:

```
fn print_banner(c: State<Console>) {
    console_putc(c, 86);   // 'V'
    console_putc(c, 77);   // 'M'
    return;
}
```

### 9.2 State parameters

Functions can take `State<T>` parameters. The state is passed by
reference (the buffer pointer is passed in a register). The callee can
read and write fields through the parameter; the caller retains
ownership unless the parameter is consumed by a transform or an FFI
close-call inside the function.

```
fn set_x(p: State<Point>, val: i32) {
    p.x = val;
}
```

### 9.3 Return values

```
fn sum_fields(p: State<Pair>) -> i32 {
    return p.a + p.b;
}
```

A function with a `State<T>` return type works (the parser accepts
it), but the codegen does not propagate `State`-typedness through
returns — see §17.3. The caller's binding `let s = make_state()` will
not be registered as state-typed, and subsequent `s.field` accesses
silently return 0. The init-style API (§5.4) is the canonical
workaround.

### 9.4 The init-style API pattern

Because of the State-return limitation, every stateful kernel
subsystem uses the **init-style API**: the caller allocates the state
with `state_new(Layout)` and passes it by reference to an init
function that populates its fields.

```
// DON'T (return-style — s not registered as state-typed):
fn make_point(x: u32, y: u32) -> State<Point> {
    let p = state_new(Point);
    p.x = x;
    p.y = y;
    return p;
}
fn use_point() -> u32 {
    let s = make_point(10, 20);
    return s.x;       // WARNING, returns 0
}

// DO (init-style — caller-allocated, callee populates):
fn init_point(p: State<Point>, x: u32, y: u32) {
    p.x = x;
    p.y = y;
}
fn use_point() -> u32 {
    let s = state_new(Point);
    init_point(s, 10, 20);
    return s.x;       // 10
}
```

### 9.5 Entry point

Every program must have a `main` function:

```
fn main() -> i32 {
    // program body
    return 0;
}
```

`main` is called by the runtime `_start` stub. Its `i32` return value
becomes the process exit code (or the kernel's boot exit code).

### 9.6 Forward references

Functions may be called before they are declared in the file — the
parser does a two-pass scan (pass 1 collects all `fn` signatures into
the symbol table, pass 2 resolves call sites). Layouts, however, must
be declared before the first function that uses them (§17.9).

---

## 10. Control Flow

### 10.1 `if` / `else` (statement form)

```
if condition {
    // then branch
} else {
    // else branch
}
```

The `else` branch is optional. The condition is any expression of
type `bool` (or any integer — non-zero is true). The statement form
produces no value.

### 10.2 If-expressions

```
let x = if cond { a } else { b };
```

The expression form produces a value — the value of the taken
branch's trailing expression. The `else` branch is **mandatory** (a
missing else would make the value undefined when `cond` is false).
`else if` chains desugar to a single-statement block wrapping the
inner if-expression.

The then-block and else-block **must** each have a trailing
expression (the last statement is treated as the value if it is a
bare expression statement). If absent, the branch defaults to `0`
(the unit value).

```
fn classify(score: u32) -> u32 {
    let grade = if score >= 90 {
        4    // A
    } else {
        if score >= 80 {
            3    // B
        } else {
            2    // C or below
        }
    };
    return grade;
}
```

Both the statement form and the expression form lower to the same SCG
`ControlNode::If` node — see
[`architecture.md` §10](./architecture.md#10-if-expression-lowering)
for the lowering detail. The expression form threads the branch value
through a result temporary; the statement form does not.

### 10.3 `while`

```
while condition {
    // loop body
}
```

The condition is evaluated before each iteration. The body executes
while the condition is non-zero. `break` exits the loop; `continue`
jumps to the next condition evaluation.

### 10.4 `for`

```
for i in 0..10 {
    // i goes from 0 to 9 (range is exclusive on the right)
}
```

The `for` loop iterates over a range `start..end` (exclusive on the
right). The loop variable is `i32` by default. C-style `for` loops
(`for (i = 0; i < n; i++)`) are not supported — the parser rejects
them with `C-style for loop is not supported in VUMA — use 'for name
in start..end' instead`.

### 10.5 `break` and `continue`

```
while true {
    if done { break; }
    if skip { continue; }
    // ...
}
```

`break` exits the enclosing `while` or `for` loop. `continue` jumps
to the next iteration. `break` does not take a value (no labeled
break, no break-with-value).

### 10.6 `return`

```
return expr;
return;       // void return
```

`return` exits the enclosing function with the given value. A
`return;` with no expression is valid in `void` functions. `main`
must `return` an `i32` (the exit code).

---

## 11. Operators

### 11.1 Arithmetic

| Operator | Description       | Notes                                            |
|----------|-------------------|--------------------------------------------------|
| `+`      | Addition          |                                                  |
| `-`      | Subtraction       |                                                  |
| `*`      | Multiplication    |                                                  |
| `/`      | Division          | Signed for signed types, unsigned for unsigned.  |
| `%`      | Modulo            | Signed for signed types, unsigned for unsigned.  |

Division and modulo by zero produces a runtime trap (SIGFPE on
x86_64). There is no `try`/`catch` — wrap divisors in `if d != 0`
guards.

### 11.2 Bitwise

| Operator | Description     |
|----------|-----------------|
| `&`      | Bitwise AND     |
| `\|`     | Bitwise OR      |
| `^`      | Bitwise XOR     |
| `<<`     | Left shift      |
| `>>`     | Right shift     |
| `~`      | Bitwise NOT (unary) |

`<<` and `>>` shift by the right operand's value; shifting by ≥ the
type's width is undefined (wraps to 0 on most backends).

### 11.3 Comparison

| Operator | Description          |
|----------|----------------------|
| `==`     | Equal                |
| `!=`     | Not equal            |
| `<`      | Less than            |
| `>`      | Greater than         |
| `<=`     | Less than or equal   |
| `>=`     | Greater than or equal|

Comparisons produce a `bool` (1 for true, 0 for false). The
comparison is signed for signed types, unsigned for unsigned types.

### 11.4 Logical

| Operator | Description     |
|----------|-----------------|
| `&&`     | Logical AND     |
| `\|\|`   | Logical OR      |
| `!`      | Logical NOT (unary) |

`&&` and `||` short-circuit (the right operand is not evaluated if
the left determines the result).

### 11.5 Casts

```
let x: u64 = 7;
let y: u32 = x as u32;       // narrowing cast (truncates)
let z: i64 = y as i64;       // widening cast (zero-extends for unsigned)
let a: Address = x as Address;   // u64 → Address
let base: Address = p as Address; // State<Point> → Address (FFI boundary)
```

`as` performs:

- **Integer widening/contraction** between `u8`/`u16`/`u32`/`u64`/
  `i8`/`i16`/`i32`/`i64`. Narrowing truncates; widening zero-extends
  for unsigned and sign-extends for signed.
- **`u64` ↔ `Address`** — the bit pattern is preserved; only the type
  changes.
- **`State<T>` → `Address`** — the *only* cast on a state value. This
  produces the state's base address in `___pmt_buffer` and is the
  sanctioned way to pass a state to an FFI call. Casting a state to
  any non-`Address` type is a parse error.

The `as Address` cast on a `State<T>` is the **only sanctioned "lossy"
ownership transfer** in the language. It does not consume the state
(see §13.1 for when consumption happens — only `#[foreign_consume]`
closes the state).

---

## 12. Atomic Operations

VUMA provides three atomic intrinsics:

```
let v = atomic_load(addr);                  // load 8 bytes atomically
atomic_store(addr, 42);                     // store 8 bytes atomically
let ok = atomic_cas(addr, expected, new);   // compare-and-swap 8 bytes
```

### 12.1 The 8-byte (U64) granularity

`atomic_load`, `atomic_store`, and `atomic_cas` are all **hardcoded to
8 bytes (U64)**. They load/store/compare-and-swap a full 8-byte word
at the given address. The address is typically obtained by casting a
state to `Address`:

```
let addr = lock as Address;
let cur = atomic_load(addr);
```

On x86_64 these lower to `LOCK CMPXCHG` (CAS), plain `mov` (load), and
`xchg` (store); on aarch64 to `ldaxr`/`stlxr` (CAS) and `ldr`/`str`
(load/store); on riscv64 to `lr.d`/`sc.d`. Atomic operations are
scheduler barriers — they are never reordered relative to other memory
ops.

### 12.2 The `_pad0` workaround

Because `atomic_cas` is 8 bytes wide, any 8-byte-aligned region of a
state is the CAS window. If your layout has two adjacent 4-byte
fields that you want to CAS-protect independently, you must insert a
`_pad0: u32` field between them to push the second field out of the
CAS window:

```
layout Spinlock = {
    locked: u32,    // offset 0 — CAS target word's low 4 bytes
    _pad0:  u32,    // offset 4 — padding (pushes `holder` out of CAS window)
    holder: u32,    // offset 8 — outside the [0..7] CAS window
    depth:  u32,    // offset 12
}
```

Without `_pad0`, the CAS window `[0..7]` would include `holder` and
the round-trip would fail. The `_pad0` field is never read or written
directly — it exists only to control the CAS window. See
[`kernel-developer-guide.md` §8.5](./kernel-developer-guide.md#85-atomic-cas-loop)
for the full pattern.

### 12.3 Atomic CAS pattern

```
fn spinlock_acquire(lock: State<Spinlock>) {
    let addr = lock as Address;
    let i = 0;
    while i < 1000 {   // bounded spin
        if atomic_cas(addr, 0, 1) == 0 {
            // CAS succeeded (returned 0 = "old value matched")
            lock.holder = current_task_idx;
            lock.depth = 1;
            return;
        }
        i = i + 1;
    }
    // Fallback: yield (sched_yield on K11+)
}
```

`atomic_cas(addr, expected, desired)` returns `0` on success (the old
value matched `expected` and was replaced with `desired`) or the
old value on failure.

---

## 13. FFI

VUMA 2.0's FFI is built on a **4-mode matrix**: every foreign call is
classified into an argument mode per argument, a return mode, and
optionally a callback mode. This replaces the legacy binary
`#[pure]`/invalidate model. See
[`architecture.md` §9](./architecture.md#9-ffi--the-4-mode-matrix)
for the full design.

### 13.1 `extern "C"` blocks

Foreign functions are declared in `extern "C"` blocks:

```
extern "C" {
    #[borrow]
    fn write(fd: i64, buf: Address, count: i64) -> i64;

    #[foreign_consume(raw)]
    fn sqlite3_close(db: State<DbHandle>);
}
```

Each extern fn may carry fn-level attributes (`#[callback]`,
`#[foreign_consume]`, `#[unmarshal(L)]`, `#[foreign_return(raw)]`, or
the fn-level shorthand forms `#[borrow]`/`#[marshal]`/`#[may_retain]`
which apply to all State-typed params). Each State-typed parameter may
carry `#[borrow]`, `#[marshal]`, or `#[may_retain]`. See §13.4 below.

### 13.2 The 4 argument modes

| Mode            | Attribute                  | What C gets                                | After-call state                       |
|-----------------|----------------------------|--------------------------------------------|----------------------------------------|
| **Borrow**      | `#[borrow]`                | `___pmt_buffer_base + offset` (zero-copy)  | **preserved** — reads/writes still valid |
| **Invalidate**  | *(default, no attr)*       | `___pmt_buffer_base + offset`              | **invalidated** — must re-init before next read/write |
| **Marshal**     | `#[marshal]`               | scratchpad pointer (copy made)             | state untouched                        |
| **ForeignPass** | `#[foreign(raw)]` on layout| the `raw` field value (the C pointer)      | preserved (unless `#[foreign_consume]`)|

Precedence: `#[may_retain]` > `#[marshal]` > `#[borrow]` >
`#[foreign]` > default (Invalidate). The first matching attribute
wins.

A fifth mode, `MayRetain`, is a Marshal variant where the callee may
keep a reference past the call (selected by `#[may_retain]`). Same
runtime behavior as Marshal, but semantically distinct.

### 13.3 The 3 return modes

| Mode             | Attribute                   | What VUMA gets                                              |
|------------------|-----------------------------|-------------------------------------------------------------|
| **Scalar**       | *(default)*                 | a plain scalar (`i64`/`u64`/`Address`)                      |
| **Unmarshal**    | `#[unmarshal(Layout)]`      | a fresh `State<Layout>` (copied into `___pmt_buffer`)       |
| **ForeignWrap**  | `#[foreign_return(raw)]`    | a `State<ForeignLayout>` whose `raw` field holds the C ptr  |

Unmarshalling is **always a copy** back into `___pmt_buffer`, never a
borrow — the StateRead/StateWrite/StateTransform verifiers never see
scratchpad or C-owned memory.

### 13.4 The 8 FFI attributes

The parser recognises exactly 8 FFI attribute names. Their allowed
placement is enforced by `validate_extern_fn_ffi_attrs` and
`validate_layout_ffi_attrs`; misplacement is a fatal parse error.

```vuma
// On extern fn params (and as fn-level shorthand applying to all State args):
#[borrow]       // C reads, does not mutate, does not hold the pointer
#[marshal]      // force scratchpad routing (NUL-term, C-ownership)
#[may_retain]   // C may stash the pointer (epoll_ctl, sigaction)

// On the extern fn itself:
#[callback]              // C may call back into VUMA via vuma_context_t
#[foreign_consume(raw)]  // consumes the State arg linearly (e.g. sqlite3_close)
#[unmarshal(Response)]   // return mode: copy C return into State<Response>
#[foreign_return(raw)]   // return mode: wrap C pointer into State<ForeignLayout>

// On layout declarations:
#[foreign(raw)]  // the layout wraps an opaque C pointer (raw: u64 field)
```

The `#[foreign_consume]` attribute marks the State argument as
consumed (linear ownership transfer to C). The existing `state_write`
verifier treats this the same as a `StateTransform` consume — any
subsequent read/write to the consumed vreg is a linearity error.

### 13.5 Callback mode

| Mode           | Attribute     | Contract                                                                  |
|----------------|---------------|---------------------------------------------------------------------------|
| **Callback**   | `#[callback]` | C may invoke VUMA functions via a `vuma_context_t*` during the call       |

The callback runtime enforces the re-entrancy rule: callbacks run on
an isolated callback stack with their own scratchpad frame, forbidden
from touching caller-live State. A `LiveSet` tracks caller-live byte
ranges; `check_access` returns `false` (trap) for any access to a
caller-live region. Nested callbacks are supported via
`CALLBACK_DEPTH` tracking.

### 13.6 The marshal scratchpad

The scratchpad (`___ffi_scratch`) is a thread-local, `malloc`-backed
stack, separate from `___pmt_buffer`. It is used for:

- NUL-terminated C strings (`marshal_cstr` copies + appends `'\0'`)
- C-owned memory round-trips (`strdup`, `getline`)
- In-place mutation buffers for C APIs demanding specific layouts

**Sacred invariant:** scratchpad memory is **never** aliased by
`___pmt_buffer`. The `StateRead`/`StateWrite`/`StateTransform`
verifiers never see it. Unmarshalling is **always** a copy back into
`___pmt_buffer`, never a borrow. The scratchpad is stack-shaped:
`push_frame` on transform entry, `pop_frame` on transform exit.

### 13.7 The `as Address` cast at the FFI boundary

The `as Address` cast on a `State<T>` is the **only sanctioned "lossy"
ownership transfer** in the language. It produces the state's base
address in `___pmt_buffer` and is how state buffers are passed to
`extern "C"` functions that take `Address` parameters:

```
fn emit(b: State<IoBuf>) -> State<IoBuf> {
    let n = write(1, b as Address, b.len as i64);
    b.len = n as u64;
    return b;
}
```

The cast does not consume the state — the state is still owned by the
caller. Whether the state is preserved, invalidated, or consumed
after the call depends on the FFI attributes on the extern fn:

- `#[borrow]` → state preserved.
- Default (Invalidate) → state invalidated (must re-init before next
  read/write).
- `#[foreign_consume(raw)]` → state consumed (linearity error to use
  afterwards).

---

## 14. Pointers Are Not Supported

VUMA 2.0 is PMT-only. The following syntax produces a **hard parse
error** — the lexer/parser rejects it before the type checker runs:

| Syntax         | Error                                                                                       |
|----------------|---------------------------------------------------------------------------------------------|
| `allocate(N)`  | `pointer syntax 'allocate' is not supported in VUMA 2.0 (PMT-only); use state_new(Layout) and transforms` |
| `free(ptr)`    | `pointer syntax 'free' is not supported in VUMA 2.0 (PMT-only)`                             |
| `*ptr`         | `pointer syntax '*ptr (deref)' is not supported in VUMA 2.0 (PMT-only); use state.field`    |
| `&x`           | `pointer syntax '&x (address-of)' is not supported in VUMA 2.0 (PMT-only)`                  |
| `@x`           | `pointer syntax '@x (address-of)' is not supported in VUMA 2.0 (PMT-only)`                  |
| `*T` (type)    | `pointer syntax '*T (pointer type)' is not supported in VUMA 2.0 (PMT-only); use State<T>`   |

There is no `--pmt` flag, no `--pmt-only` flag, and no legacy pointer
mode. PMT is always on. The parser scans the accumulated error list
for messages starting with `pointer syntax '` and downgrades the
parse result to a fatal `ParseResult::err` so the failure propagates
to the CLI driver.

The PMT alternatives to each pointer construct:

| Pointer construct      | PMT alternative                                            |
|------------------------|------------------------------------------------------------|
| `allocate(N)`          | `state_new(Layout)` (static slot) or `arena_new(cap)` (runtime) |
| `free(ptr)`            | (none — states are reclaimed with the frame)               |
| `*ptr` (dereference)   | `state.field`                                              |
| `&x` (address-of)      | `state as Address` (FFI boundary only)                     |
| `*T` (pointer type)    | `State<T>`                                                 |

---

## 15. Verification

All programs are verified with `VerificationLevel::Pmt`, which runs
three state verifiers. There is no `--pmt` flag and no
`Normal`/`Exhaustive`/`Hardened` level selection — `Pmt` is the only
level the driver ever constructs.

### 15.1 `StateReadVerifier`

For every `state.field` read, proves:

1. The field exists in the state's layout.
2. `field_offset + field_size ≤ layout.total_size` (no out-of-bounds
   read).
3. The read type matches the field's declared type.

### 15.2 `StateWriteVerifier`

For every `state.field = value` write, proves:

1. Same bounds and type checks as reads.
2. **Linearity**: the state has not been consumed by a transform
   (or by an FFI close-call, or by `arena_free`). No
   write-after-consume.

### 15.3 `StateTransformVerifier`

For every `transform` call, proves:

1. Both input and output layouts exist (are registered).
2. Layout compatibility:
   - **Identity** — `in_layout == out_layout` → buffer returned
     unchanged.
   - **Reinterpret** — `in.total_size == out.total_size` → buffer
     reinterpreted in place at zero cost.
   - **Copy** — different sizes → fresh `Alloc` + `Store`-by-`Store`
     copy.

### 15.4 The collapsed-invariant table

The 5 pointer invariants from VUMA 1.x are replaced by structural
type checks:

| VUMA 1.x invariant                          | VUMA 2.0 PMT                                                                                       |
|---------------------------------------------|----------------------------------------------------------------------------------------------------|
| Liveness (no use-after-free)                | — (eliminated by syntax: no `free` in source)                                                      |
| Exclusivity (no aliasing during mutation)   | `StateWrite` linearity — states are linear, one owner, consumed on transform (structural)          |
| Cleanup (no double-free)                    | — (eliminated by syntax: no `free` in source; buffer freed once at program exit)                   |
| Origin (no uninitialized reads)             | `StateRead` field-exists + initialized — type-check against layout registry                        |
| Interpretation (no type-punning)            | `StateTransform` layout-type match — src/dst layout types must be equal where required             |

The two `—` rows are the structural win: with no `free` in the source
language, liveness and cleanup are not verifier obligations at all.
The remaining three pointer concerns map one-to-one onto three
structural checks that the compiler runs at SCG construction time,
before codegen. See
[`architecture.md` §5.1](./architecture.md#51-collapsed-invariant-table)
for the full table with discharge mechanisms.

### 15.5 Verification invocation

```
./target/release-fast/compile_dump counter.vuma counter.bin x86_64 --verify
```

With `--verify`, an IVE failure aborts compilation before emission;
without `--verify`, the IVE result is reported but compilation
proceeds. The output format is `IVE: Pass passed=N failed=N total=N`.

---

## 16. Complete Examples

### 16.1 Counter

A PMT counter — increment, read, exit with the value. Expected exit
code: 7.

```vuma
// counter.vuma — a PMT counter.

layout Counter = { value: u32 }

fn inc(c: State<Counter>, by: u32) {
    c.value = c.value + by;
}

fn main() -> i32 {
    let c = state_new(Counter);
    c.value = 0;
    inc(c, 3);
    inc(c, 4);
    return c.value;   // 7
}
```

### 16.2 Stack with push/pop

A PMT stack with `push`/`pop`. Expected exit code: 60.

```vuma
// stack.vuma — a PMT stack: push 10, 20, 30; pop three times; return sum.

layout Stack = { count: u32, data: [u32; 8] }

fn push(s: State<Stack>, v: u32) {
    s.data[s.count] = v;
    s.count = s.count + 1;
}

fn pop(s: State<Stack>) -> u32 {
    s.count = s.count - 1;
    return s.data[s.count];
}

fn main() -> i32 {
    let st = state_new(Stack);
    st.count = 0;
    push(st, 10);
    push(st, 20);
    push(st, 30);
    let a = pop(st);   // 30
    let b = pop(st);   // 20
    let c = pop(st);   // 10
    return a + b + c;  // 60
}
```

### 16.3 Arena allocator

An arena allocator with `arena_new`/`arena_alloc`/`arena_grow`/
`arena_free`. Expected exit code: 42.

```vuma
// arena.vuma — runtime-growable memory via the arena state model.

layout Widget = { tag: u32, payload: u32 }

fn main() -> i32 {
    let arena = arena_new(4096);              // mmap a 4096-byte region
    let w1 = arena_alloc(arena, Widget);      // bump-allocate a Widget
    w1.tag = 1;
    w1.payload = 42;
    let w2 = arena_alloc(arena, Widget);      // another Widget
    w2.tag = 2;
    w2.payload = w1.payload + 0;              // 42
    arena_free(arena);                        // munmap; arena consumed
    return w2.payload;                        // 42
}
```

### 16.4 FFI borrowed read

A foreign `write` call that borrows the state (zero-copy). Expected
exit code: the number of bytes written.

```vuma
// ffi_borrow.vuma — zero-copy I/O via #[borrow].

layout IoBuf = { len: u64, data: [u8; 16] }

extern "C" {
    #[borrow]
    fn write(fd: i64, buf: Address, count: i64) -> i64;
}

fn emit(b: State<IoBuf>) -> State<IoBuf> {
    let n = write(1, b as Address, b.len as i64);
    // b is #[borrow] — preserved after the call. b.len is still valid.
    b.len = n as u64;
    return b;
}

fn main() -> i32 {
    let buf = state_new(IoBuf);
    buf.data[0] = 104;   // 'h'
    buf.data[1] = 105;   // 'i'
    buf.data[2] = 10;    // '\n'
    buf.len = 3;
    let _b2 = emit(buf);
    return 0;
}
```

### 16.5 Foreign handle (open/close with linear safety)

A foreign handle that wraps an opaque C pointer and is consumed on
close. Expected exit code: 0.

```vuma
// ffi_handle.vuma — #[foreign(raw)] + #[foreign_consume(raw)] linear safety.

#[foreign(raw)]
layout DbHandle = { raw: u64 }

extern "C" {
    #[foreign_return(raw)]
    fn sqlite3_open(path: Address) -> State<DbHandle>;

    #[foreign_consume(raw)]
    fn sqlite3_close(db: State<DbHandle>);
}

fn main() -> i32 {
    let db = sqlite3_open(0 as Address);   // wraps C pointer into State<DbHandle>
    sqlite3_close(db);                     // consumes db
    // db.raw = 0;                          // LINEARITY VIOLATION — db was consumed
    return 0;
}
```

### 16.6 If-expression

A value-typed if-expression with an `else if` chain. Expected exit
code: 4.

```vuma
// if_expr.vuma — let x = if cond { a } else { b };

fn grade_for(score: u32) -> u32 {
    let grade = if score >= 90 {
        4    // A
    } else {
        if score >= 80 {
            3    // B
        } else {
            if score >= 70 {
                2    // C
            } else {
                1    // F
            }
        }
    };
    return grade;
}

fn main() -> i32 {
    return grade_for(95);   // 4
}
```

---

## 17. VUMA Parser Limitations

The VUMA 2.0 parser (`src/parser/src/parser.rs`) and the codegen
bridge (`src/pipeline.rs::flatten_expr`) have several known
limitations that shape kernel and library code style. Each is
documented in the file header of the kernel module that works around
it; this section consolidates them. See also
[`kernel-architecture.md` §10](./kernel-architecture.md#10-vuma-parser-limitations)
and [`architecture.md` §12](./architecture.md#12-vuma-parser-limitations).

### 17.1 No `import`

VUMA 2.0 has no `import` statement. Every module that wants to call
another module's functions or use another module's layouts must
**re-declare them locally**, byte-identically. The
`byte-identical-redeclaration invariant` is enforced by the
`LayoutRegistry` — if two files declare the same layout name with
different field offsets/types/order, the verifiers catch the drift at
compile time.

```
// womb/kernel/syscall/dispatch.vuma re-declares:
layout SyscallArgs = { nr: u64, a0: u64, a1: u64, a2: u64,
                       a3: u64, a4: u64, a5: u64 }
// byte-identical to womb/kernel/syscall/abi.vuma's SyscallArgs.
```

**Workaround:** copy-paste the declaration, byte-identically. A
future wave will add a real `import` mechanism.

### 17.2 No string-literal lowering

The lexer recognises string literals (`"hello"`) but the codegen
bridge does not lower them to data — there is no `.rodata` emission,
no string-interning table, and no `state_new(String)` lowering.

**Workaround:** write the bytes into a `State<Buffer>` field-by-field,
or use the `marshal_cstr` builtin (§13.6) to copy a buffer to the
scratchpad and append `'\0'` for FFI.

### 17.3 `State`-typedness doesn't propagate through function returns

The codegen does not propagate `State<T>`-typedness through function
return values. A binding `let s = make_state()` where `make_state`
returns `State<T>` is **not** registered as state-typed in the caller;
subsequent `s.field` accesses silently return 0 with a
`WARNING: unsupported FieldAccess (not state-typed)` from
`flatten_expr`.

**Workaround:** the **init-style API** (§5.4, §9.4). The caller
allocates the state with `state_new(...)` and passes it by reference
to a function that populates it in place.

### 17.4 Array index is byte-granular for non-`u8` element types

The codegen only lowers `state.array[idx]` correctly for `[u8; N]`
arrays. For `[u16; N]`, `[u32; N]`, or `[u64; N]` arrays, the
indexed-access path goes through a code path the IVE verifiers don't
fully understand — accesses compile but read back wrong values.

**Workaround:** store every "array of N u32/u64" as a parallel flat
`[u8; N * width]` array with pack/unpack helpers:

```vuma
layout InodeTable = {
    ino:  [u8; 1024],   // 128 inodes × 8 bytes, packed little-endian
    mode: [u8; 128],    // 128 inodes × 1 byte
    size: [u8; 1024],   // 128 inodes × 8 bytes, packed little-endian
}

fn inode_get_ino(tbl: State<InodeTable>, idx: u32) -> u64 {
    let off = idx * 8;
    let v: u64 = 0;
    let i = 0;
    while i < 8 {
        let sh = i * 8;
        let b = tbl.ino[off + i] as u64;
        v = v + (b << sh);
        i = i + 1;
    }
    return v;
}
```

The pack helper is an 8-iteration while loop (shift by `i*8`, mask
255 on writes, sum-of-shifted-bytes on reads). A future wave will fix
the codegen to support `[u64; N]` directly.

### 17.5 `transform` is limited to a single `State<T>` parameter

The `StateTransform` verifier requires that an `as Address` cast
appears in a context where the `State<T>` is the function's parameter.
Casting a state created inside the function
(`let s = state_new(...); let a = s as Address; ...`) sometimes trips
the verifier because the lifetime of `s` is local.

**Workaround:** always pass states from the caller. Even a helper that
just needs a scratch buffer takes it as a parameter:

```vuma
// DON'T (transform-on-local-state — sometimes trips verifier):
fn console_flush_local() {
    let c = state_new(Console);
    let base = c as Address;       // transform on local State — risky
    let _n = write(1, base, c.len as i64);
}

// DO (transform-on-parameter — always safe):
fn console_flush(c: State<Console>) {
    let base = c as Address;       // transform on parameter — OK
    let _n = write(1, base, c.len as i64);
    c.len = 0;
}
```

### 17.6 The `0 - N` negative-literal workaround

VUMA's integer-literal path goes through `parse_int_radix`, which on a
negative literal like `-1` interprets it as a signed `i64` and then
sign-extends to `u64` — producing `0xFFFFFFFFFFFFFFFF` correctly in
most cases but occasionally tripping a width-extension subtlety that
produces surprising values.

**Workaround:** always write negative numbers as `0 - N` (e.g.
`0 - 1` for -1, `0 - 11` for -EAGAIN, `0 - 38` for -ENOSYS) rather
than as the literal `-1` / `-11` / `-38`:

```vuma
// DON'T:
return -1;          // parser's signed-literal path — risky

// DO:
return 0 - 1;       // flatten_expr's BinOp::Sub arm — verified safe
```

The `0 - N` form lowers to the same machine code as `-N` (the
codegen's constant folder collapses it), so there is no perf cost.

### 17.7 The hex-literal width-extension subtlety

VUMA accepts `0x..` hex literals but the parser's hex path shares code
with the decimal path through `parse_int_radix`, which has subtle
width-extension behavior at the 64-bit boundary.

**Workaround:** use decimal literals in self-tests (`4096`, not
`0x1000`; `17592186028032`, not `0x000FFFFFFFFFF000`). The decimal
form lowers to identical machine code; the safety gain is avoiding
the hex path entirely. Hex literals are acceptable in comments and
prose for readability.

### 17.8 The `no_struct_literal` trap

VUMA has no struct-literal syntax (`Layout { field: value, ... }`).
State must be allocated with `state_new(Layout)` (zero-initialized)
and then populated field-by-field. There is no way to "construct" a
state inline.

**Workaround:** allocate-then-populate (which is also the init-style
API, §5.4):

```vuma
// DON'T (parse error — VUMA has no struct literal):
fn make_task(pid: u32) -> State<Task> {
    return Task { pid: pid, state: 1 };
}

// DO (allocate-then-populate):
fn make_task(tbl: State<ProcessTable>, pid: u32) {
    let idx = task_alloc(tbl);
    pt_set_pid(tbl, idx, pid);
    pt_set_state(tbl, idx, 1);
}
```

This is not really a workaround so much as a language-design choice
that aligns with the PMT discipline (no implicit allocation sites).

### 17.9 Forward references

Unlike C, VUMA allows forward references to functions: a function can
call another function declared later in the file. The parser does a
two-pass scan (pass 1 collects all `fn` signatures into the symbol
table, pass 2 resolves call sites). Layouts, however, **must** be
declared before the first function that uses them — the layout
registry is single-pass within a function.

**Workaround:** declare all layouts at the top of the file, before any
function. (This is also good style — it makes the data model visible
up front.)

---

## Cross-references

- [`architecture.md`](./architecture.md) — the VUMA 2.0 compiler
  architecture: PMT pipeline, state type system, arena state model,
  the 3 state verifiers, Behavioral Descriptors, e-graph layout
  optimization, 19 backends, FFI 4-mode marshal matrix, if-expression
  lowering, nested layout resolution, parser limitations.
- [`kernel-architecture.md`](./kernel-architecture.md) — the VWK
  kernel architecture: the four-layer cake, boot flow, PMT-in-the-kernel
  design, arena memory model, per-arch abstraction, FFI trampoline
  patterns.
- [`kernel-developer-guide.md`](./kernel-developer-guide.md) — how to
  extend the kernel with new syscalls, drivers, filesystems, and PMT
  kernel code.
- [`building.md`](./building.md) — complete build reference.
- [`contributing.md`](./contributing.md) — general contribution
  workflow.
