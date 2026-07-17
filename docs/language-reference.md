# VUMA 2.0 Language Reference

> **VUMA is PMT-only.** There are no pointers. Memory safety is a type-checking property, not a verification problem.

## 1. Introduction

VUMA (Verified Unsafe Memory Access) 2.0 is a systems programming language built on **Programs as Memory Transformations (PMT)**. A program is not "code that allocates and uses memory" — it is a sequence of typed memory state transformations. The compiler proves each transformation is valid at compile time. Memory safety is free: it is a structural property of the type system, not a runtime check or a proof obligation.

Pointer syntax (`allocate`, `free`, `*ptr`, `&x`, `*T`) does not exist in VUMA 2.0. Attempting to use it produces a hard parse error. All memory access is through typed state fields with compile-time-verified offsets.

## 2. Lexical Structure

### Keywords

```
layout State transform state_new fn let if else while for return break
true false as
```

### Identifiers

Start with a letter or underscore, followed by letters, digits, or underscores. Case-sensitive.

### Literals

```
42          // integer (i64 by default)
255u8       // u8 literal
1000000u32  // u32 literal
4294967295  // u64 literal
true        // boolean
```

### Comments

```
// Single-line comment
```

## 3. Types

### Base Types

| Type | Size | Range |
|------|------|-------|
| `u8` | 1 byte | 0..255 |
| `u16` | 2 bytes | 0..65535 |
| `u32` | 4 bytes | 0..4294967295 |
| `u64` | 8 bytes | 0..18446744073709551615 |
| `i8` | 1 byte | -128..127 |
| `i16` | 2 bytes | -32768..32767 |
| `i32` | 4 bytes | -2147483648..2147483647 |
| `i64` | 8 bytes | -9223372036854775808..9223372036854775807 |
| `bool` | 1 byte | true / false |
| `void` | 0 bytes | unit type |

### Array Types

```
[u8; 16]     // 16-byte array
[u32; 8]     // 8-element u32 array (32 bytes)
```

### State Types

```
State<Point>       // a typed view of the buffer interpreted as a Point layout
State<Buffer>      // a typed view interpreted as a Buffer layout
```

A `State<T>` is a typed view of the program's memory buffer. The layout `T` determines how the buffer bytes are interpreted at that program point. States are **linear**: they have one owner and are consumed when passed to a transform.

### Layout Types

Layouts are defined at module scope (see §4). A layout name can be used as a type parameter for `State<T>`.

## 4. Layouts

A layout defines the byte-level structure of a memory state. Fields are laid out sequentially with natural alignment padding.

### Syntax

```
layout Name = { field1: Type, field2: Type, ... }
```

### Example

```
layout Point = { x: u32, y: u32 }
layout Line = { a: Point, b: Point }
layout Buffer = { count: u32, data: [u8; 256] }
```

### Field Offset Computation

Fields are placed sequentially. Each field is aligned to its natural alignment (1 for u8, 2 for u16, 4 for u32, 8 for u64). Padding is inserted between fields as needed.

```
layout Example = { a: u8, b: u32 }
// a at offset 0 (1 byte), padding 3 bytes, b at offset 4 (4 bytes)
// total size: 8 bytes
```

### Nested Layouts

A layout field can be another layout type. The nested layout's fields are inlined at the computed offset.

```
layout Line = { a: Point, b: Point }
// a at offset 0 (8 bytes: a.x@0, a.y@4)
// b at offset 8 (8 bytes: b.x@8, b.y@12)
// total size: 16 bytes
```

### Array Fields

```
layout Stack = { count: u32, data: [u32; 8] }
// count at offset 0 (4 bytes)
// data at offset 4 (32 bytes: data[0]@4, data[1]@8, ..., data[7]@32)
// total size: 36 bytes
```

## 5. States

### State Initialization

```
let p = state_new(Point);
```

`state_new(Layout)` creates a new `State<Layout>`. The compiler allocates buffer space (in the single program-wide buffer) and assigns a fixed offset. No runtime allocation occurs — the buffer is pre-allocated at program start.

### State Lifetime

States are **linear**: they have exactly one owner. A state is consumed when passed to a transform (the transform takes ownership). A state cannot be used after being consumed.

### State Parameters

```
fn get_x(p: State<Point>) -> i32 {
    return p.x;
}
```

State-typed parameters are passed by reference (the buffer pointer). The callee can read and write fields through the parameter.

## 6. Field Access

### Read

```
let v = p.x;           // read field x from state p
let first = buf.data[0]; // read first element of array field
```

### Write

```
p.x = 42;              // write 42 to field x of state p
buf.data[0] = 65;      // write 65 to first element of array field
```

### Nested Access

```
let line = state_new(Line);
line.a.x = 10;         // write to nested field
let v = line.b.y;      // read from nested field
```

### Array Element Access

```
let stack = state_new(Stack);
stack.count = 0;
stack.data[stack.count] = 42;   // write at runtime index
stack.count = stack.count + 1;
let val = stack.data[0];        // read at constant index
```

The compiler verifies that array accesses are within bounds. For runtime indices, the State Transform verifier proves `offset + (count * elem_size) ≤ layout_size` using linear arithmetic (Presburger-decidable).

## 7. Transforms

A transform converts a state from one layout to another.

### Syntax

```
transform name(s: State<InputLayout>) -> State<OutputLayout> {
    // body: read from s, produce output state
}
```

### Layout Compatibility

- **Same size**: the buffer is reinterpreted in-place (zero-cost, no copy).
- **Different size**: the compiler generates a new allocation and copies the data.
- **Identity** (same layout): the transform is a no-op.

### Example

```
transform parse(raw: State<RawBuffer>) -> State<Request> {
    // Compiler proves RawBuffer and Request have compatible layout
    // (same total size, or generates a copy transformation)
}
```

### Calling a Transform

```
let req = parse(raw);
```

After the call, `raw` is consumed (linear ownership). The result `req` is a new `State<Request>`.

## 8. Functions

### Syntax

```
fn name(param1: Type, param2: Type) -> ReturnType {
    // body
}
```

### State Parameters

Functions can take `State<T>` parameters. The state is passed by reference (buffer pointer).

```
fn set_x(p: State<Point>, val: i32) {
    p.x = val;
}
```

### Return Values

```
fn sum_fields(p: State<Pair>) -> i32 {
    return p.a + p.b;
}
```

### Entry Point

Every program must have a `main` function:

```
fn main() -> i32 {
    // program body
    return 0;
}
```

## 9. Control Flow

### if / else

```
if condition {
    // then branch
} else {
    // else branch
}
```

### while

```
while condition {
    // loop body
}
```

### for

```
for i in 0..10 {
    // i goes from 0 to 9
}
```

### return

```
return expr;
return;  // void return
```

### break

```
while true {
    if done { break; }
}
```

## 10. Operators

### Arithmetic

| Operator | Description |
|----------|-------------|
| `+` | Addition |
| `-` | Subtraction |
| `*` | Multiplication |
| `/` | Division |
| `%` | Modulo |

### Bitwise

| Operator | Description |
|----------|-------------|
| `&` | Bitwise AND |
| `\|` | Bitwise OR |
| `^` | Bitwise XOR |
| `<<` | Left shift |
| `>>` | Right shift |

### Comparison

| Operator | Description |
|----------|-------------|
| `==` | Equal |
| `!=` | Not equal |
| `<` | Less than |
| `>` | Greater than |
| `<=` | Less than or equal |
| `>=` | Greater than or equal |

### Logical

| Operator | Description |
|----------|-------------|
| `&&` | Logical AND |
| `\|\|` | Logical OR |
| `!` | Logical NOT |

### Casts

```
let x: u64 = 7;
let y: u32 = x as u32;    // narrowing cast (truncates)
let z: i64 = y as i64;    // widening cast (zero-extends for unsigned)
```

## 11. Pointers Are Not Supported

VUMA 2.0 is PMT-only. The following syntax produces a **hard parse error**:

| Syntax | Error |
|--------|-------|
| `allocate(N)` | `pointer syntax 'allocate' is not supported in VUMA 2.0 (PMT-only); use state_new(Layout) and transforms` |
| `free(ptr)` | `pointer syntax 'free' is not supported in VUMA 2.0 (PMT-only)` |
| `*ptr` | `pointer syntax '*ptr' is not supported in VUMA 2.0 (PMT-only); use state.field` |
| `&x` | `pointer syntax '&x' is not supported in VUMA 2.0 (PMT-only)` |
| `*T` (type) | `pointer syntax '*T' is not supported in VUMA 2.0 (PMT-only); use State<T>` |

There is no `--pmt` flag, no `--pmt-only` flag, and no legacy pointer mode. PMT is always on.

## 12. Verification

All programs are verified with `VerificationLevel::Pmt`, which runs three state verifiers:

### StateReadVerifier

For every `state.field` read, proves:
1. The field exists in the state's layout.
2. `field_offset + field_size ≤ layout.total_size` (no out-of-bounds read).
3. The read type matches the field's declared type.

### StateWriteVerifier

For every `state.field = value` write, proves:
1. Same bounds and type checks as reads.
2. **Linearity**: the state has not been consumed by a transform (no write-after-consume).

### StateTransformVerifier

For every `transform` call, proves:
1. Both input and output layouts exist.
2. Layout compatibility: same size (reinterpret) or different size (copy generated).

### The Collapsed-Invariant Table

The 5 pointer invariants from VUMA 1.x are replaced by structural type checks:

| VUMA 1.x Invariant | VUMA 2.0 PMT |
|---------------------|--------------|
| Liveness (track pointer lifetime) | A state is live iff its type is in scope (O(1) type check) |
| Exclusivity (prove no aliasing) | States are linear — one owner, consumed on transform (structural) |
| Cleanup (track each free) | Buffer freed once at program exit (O(1)) |
| Origin (track derivation chain) | Every reference traces to a state transformation (structural) |
| Interpretation (prove type matches) | The state type IS the interpretation (structural) |

## 13. Complete Example

```
// A complete PMT program: stack with push/pop
// Expected exit code: 60

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
    let a = pop(st);  // 30
    let b = pop(st);  // 20
    let c = pop(st);  // 10
    return a + b + c;  // 60
}
```

## 14. FFI (Foreign Function Interface)

VUMA 2.0's FFI is built on a **4-mode matrix**: every foreign call is
classified into an argument mode per argument, a return mode, and
optionally a callback mode. This replaces the legacy binary `#[pure]`/
invalidate model.

### 14.1 Argument modes

| Mode | Attribute | What C gets | After-call state |
|---|---|---|---|
| **Borrow** | `#[borrow]` | `___pmt_buffer_base + offset` (zero-copy) | **preserved** — reads/writes still valid |
| **Invalidate** | *(default, no attr)* | `___pmt_buffer_base + offset` | **invalidated** — must re-init before next read/write |
| **Marshal** | `#[marshal]` | scratchpad pointer (copy made) | state untouched |
| **ForeignPass** | `#[foreign(raw)]` on the arg's layout | the `raw` field value (the C pointer) | preserved (unless `#[foreign_consume]`) |

### 14.2 Return modes

| Mode | Attribute | What VUMA gets |
|---|---|---|
| **Scalar** | *(default)* | a plain scalar (`i64`/`u64`/`Address`) |
| **Unmarshal** | `#[unmarshal(Layout)]` | a fresh `State<Layout>` (copied into `___pmt_buffer`) |
| **ForeignWrap** | `#[foreign_return(raw)]` | a `State<ForeignLayout>` whose `raw` field holds the C pointer |

### 14.3 Callback mode

| Mode | Attribute | Contract |
|---|---|---|
| **Callback** | `#[callback]` | C may invoke VUMA functions via a `vuma_context_t*` during the call |

### 14.4 The 8 FFI attributes

```vuma
// On extern fn params:
#[borrow]       // C reads, does not mutate, does not hold the pointer
#[marshal]      // force scratchpad routing (NUL-term, C-ownership)
#[may_retain]   // C may stash the pointer (epoll_ctl, sigaction)

// On the extern fn itself:
#[callback]             // C may call back into VUMA via vuma_context_t
#[foreign_consume(raw)] // consumes the State arg linearly (e.g. sqlite3_close)
#[unmarshal(Response)]  // return mode: copy C return into State<Response>
#[foreign_return(raw)]  // return mode: wrap C pointer into State<ForeignLayout>

// On layout declarations:
#[foreign(raw)]  // the layout wraps an opaque C pointer (raw: u64 field)
```

### 14.5 Worked examples

**Borrowed read (zero-copy I/O):**
```vuma
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
```

**Foreign handle (open/close with linear safety):**
```vuma
#[foreign(raw)]
layout DbHandle = { raw: u64 }

extern "C" {
    #[foreign_return(raw)]
    fn sqlite3_open(path: Address) -> State<DbHandle>;

    #[foreign_consume(raw)]
    fn sqlite3_close(db: State<DbHandle>);
}

fn main() -> i32 {
    let db = sqlite3_open(0 as Address);  // wraps C pointer into State<DbHandle>
    sqlite3_close(db);  // consumes db — post-close use is a linearity error
    return 0;
}
```

**Callback (sqlite3_exec):**
```vuma
extern "C" {
    #[callback]
    fn sqlite3_exec(db: State<DbHandle>, sql: Address,
                    cb: Address, arg: Address, err: Address) -> i32;
}

fn row_callback(ctx: Address, argc: i32, argv: Address, col: Address) -> i32 {
    // C invokes this for each row. The callback runs on an isolated stack
    // with its own scratchpad frame. It may NOT touch caller-live State
    // (enforced by the callback_live_set guard — trap on violation).
    return 0;
}
```

### 14.6 The marshal scratchpad

The scratchpad (`___ffi_scratch`) is a thread-local, `malloc`-backed stack,
separate from `___pmt_buffer`. It is used for:
- NUL-terminated C strings (`marshal_cstr` copies + appends `'\0'`)
- C-owned memory round-trips (`strdup`, `getline`)
- In-place mutation buffers for C APIs demanding specific layouts

**Sacred invariant:** scratchpad memory is NEVER aliased by `___pmt_buffer`.
The `StateRead`/`StateWrite`/`StateTransform` verifiers never see it.
Unmarshalling is ALWAYS a copy back into `___pmt_buffer`, never a borrow.

The scratchpad is stack-shaped: `push_frame` on transform entry, `pop_frame`
on transform exit. Nested transforms get nested frames.
