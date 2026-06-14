# VUMA Language Reference for LLMs

> **Audience**: This document is written for large language models that need to generate correct VUMA source code. It is unambiguous, example-heavy, and explicitly documents every pitfall and edge case. If you are an LLM reading this, follow these rules precisely.

---

## Table of Contents

1. [Quick Start](#1-quick-start)
2. [Types](#2-types)
3. [Functions](#3-functions)
4. [Variables](#4-variables)
5. [Control Flow](#5-control-flow)
6. [Memory](#6-memory)
7. [Pointer Operations](#7-pointer-operations)
8. [Bitwise Operations](#8-bitwise-operations)
9. [Arithmetic](#9-arithmetic)
10. [Comparison](#10-comparison)
11. [Constants](#11-constants)
12. [Calling Convention](#12-calling-convention)
13. [Common Patterns](#13-common-patterns)
14. [Pitfalls](#14-pitfalls)
15. [Target Platforms](#15-target-platforms)

---

## 1. Quick Start

A VUMA program consists of one or more function definitions. The entry point is always `fn main()`. The compiler produces a native binary (or Wasm module) that can be executed directly or via QEMU for cross-architecture testing.

### Minimal Program

```vuma
fn main() -> i32 {
    return 0;
}
```

This program exits with code 0.

### Compile and Run

VUMA compiles `.vuma` source files to native binaries. The build system uses Cargo under the hood. To build and test:

```bash
# Build the VUMA compiler
cargo build --workspace

# Run all tests
cargo test --workspace

# Cross-architecture testing uses QEMU
# x86_64:
qemu-x86_64 ./output_binary
# AArch64:
qemu-aarch64 ./output_binary
# RISC-V 64:
qemu-riscv64-static ./output_binary
# ARM32:
qemu-arm ./output_binary
# MIPS64:
qemu-mips64-static ./output_binary
# PPC64:
qemu-ppc64 ./output_binary
# LoongArch64:
qemu-loongarch64 ./output_binary
```

### Program with Memory

```vuma
fn main() -> i32 {
    msg = allocate(3);
    *(msg + 0) = 97;   // 'a'
    *(msg + 1) = 98;   // 'b'
    *(msg + 2) = 99;   // 'c'
    free(msg);
    return 0;
}
```

**Key facts**:
- `main()` returns `i32` (the process exit code) or `u64`.
- Every `allocate()` must be paired with a `free()`.
- Pointer dereference uses `*(ptr + offset)` syntax.
- Bytes are stored as integer values (e.g., `97` for ASCII `'a'`).

---

## 2. Types

VUMA supports the following primitive types:

| Type | Size (bytes) | Range | Signed |
|------|-------------|-------|--------|
| `i8`  | 1 | -128 to 127 | Yes |
| `i16` | 2 | -32768 to 32767 | Yes |
| `i32` | 4 | -2147483648 to 2147483647 | Yes |
| `i64` | 8 | -9223372036854775808 to 9223372036854775807 | Yes |
| `u8`  | 1 | 0 to 255 | No |
| `u16` | 2 | 0 to 65535 | No |
| `u32` | 4 | 0 to 4294967295 | No |
| `u64` | 8 | 0 to 18446744073709551615 | No |
| `Address` | 8 (4 on 32-bit targets) | N/A (opaque pointer) | No |
| `bool` | 1 | `true` or `false` | N/A |
| `void` | 0 | N/A | N/A |

### Type Annotation Syntax

```vuma
x: i32 = 42;
y: u32 = 0xFFFFFFFF;
ptr: Address = allocate(64);
big: u64 = 18446744073709551615;
flag: bool = true;
```

### Critical Constraint: u32 on 64-bit Registers

**This is the single most important thing to know about VUMA types.** All 64-bit target platforms (x86_64, AArch64, RISC-V 64, MIPS64, PPC64, LoongArch64) use 64-bit general-purpose registers. A `u32` value stored in a 64-bit register can have garbage bits in the upper 32 bits after arithmetic operations. You **must** manually mask u32 results with `& 4294967295` (which is `0xFFFFFFFF` in decimal) after any operation that could set bits above bit 31:

```vuma
// WRONG: bits above bit 31 may be set
sum: u32 = a + b;

// CORRECT: mask to 32 bits after addition
sum: u32 = (a + b) & 4294967295;
```

See [Section 14: Pitfalls](#14-pitfalls) for the complete list of operations that require masking.

### Address Type

`Address` is the type for pointers and memory regions. It is 8 bytes on 64-bit targets and 4 bytes on 32-bit targets (ARM32, Wasm32). The `allocate()` built-in returns an `Address`.

```vuma
buf: Address = allocate(256);
```

---

## 3. Functions

### Function Declaration

```vuma
fn function_name(param1: type1, param2: type2) -> return_type {
    // body
    return value;
}
```

- The `fn` keyword introduces a function.
- Parameters have mandatory type annotations when declared in the signature.
- The return type follows `->`. If omitted, the function returns `void`.
- The body is a block enclosed in `{ }`.
- Use `return expr;` to return a value. The last expression in a block without `return` is also the return value in some contexts, but `return` is preferred for clarity.

### Examples

```vuma
// Function returning a value
fn add(a: i32, b: i32) -> i32 {
    return a + b;
}

// Function with no return value (void)
fn store_byte(buf: Address, offset: u64, val: u32) {
    *(buf + offset) = val & 255;
}

// Function with Address parameter
fn read_u32_be(buf: Address, offset: u64) -> u32 {
    b0: u32 = *(buf + offset);
    b1: u32 = *(buf + offset + 1);
    b2: u32 = *(buf + offset + 2);
    b3: u32 = *(buf + offset + 3);
    return ((b0 << 24) | (b1 << 16) | (b2 << 8) | b3) & 4294967295;
}

// Main entry point — must return i32 or u64
fn main() -> i32 {
    return 0;
}
```

### Function Call

```vuma
result: i32 = add(10, 20);
store_byte(buffer, 0, 65);
```

Functions are called by name with parenthesized arguments. Function calls can appear as expressions (when the function returns a value) or as statements (when the function returns void or the return value is discarded).

---

## 4. Variables

### Declaration with Type Annotation

```vuma
name: type = expression;
```

Variables are declared with a name, a type annotation, and an initial value. The type annotation is required in most cases.

```vuma
x: i32 = 42;
y: u32 = 0xFFFFFFFF;
ptr: Address = allocate(64);
sum: u32 = (a + b) & 4294967295;
```

### Declaration Without Type Annotation (Inferred)

VUMA can infer the type from the right-hand side in some cases, but explicit annotation is strongly recommended to avoid ambiguity:

```vuma
result = add1(41);       // type inferred from function return type
msg = allocate(3);        // type inferred as Address
```

**When writing VUMA code as an LLM, always use explicit type annotations** except for variables assigned from `allocate()` (which always returns `Address`).

### Assignment

Once declared, variables can be reassigned without repeating the type:

```vuma
x: u32 = 10;
x = 20;              // reassignment
x = (x + 1) & 4294967295;  // reassignment with expression
```

### Compound Assignment

VUMA supports compound assignment operators:

```vuma
x += 1;     // x = x + 1
x -= 1;     // x = x - 1
x *= 2;     // x = x * 2
x &= 255;   // x = x & 255
x |= mask;  // x = x | mask
x ^= key;   // x = x ^ key
x <<= 4;    // x = x << 4
x >>= 4;    // x = x >> 4
```

**Note**: Compound assignments on `u32` values still need the `& 4294967295` mask. The compound form does **not** automatically mask. Prefer explicit assignment with masking:

```vuma
// WRONG for u32 on 64-bit registers:
x += 1;

// CORRECT:
x = (x + 1) & 4294967295;
```

---

## 5. Control Flow

### For Loop (Range)

The `for` loop iterates over an exclusive range `start..end`. The loop variable takes each integer value from `start` (inclusive) to `end` (exclusive).

```vuma
for i in 0..10 {
    // i = 0, 1, 2, ..., 9
}

for i in 0..64 {
    // i = 0, 1, 2, ..., 63
}

for i in 16..64 {
    // i = 16, 17, ..., 63
}
```

**The range is always exclusive on the upper bound**: `0..10` means `0 <= i < 10`.

### For Loop with Memory Access

```vuma
// Copy 32 bytes from src to dst
fn copy32(dst: Address, src: Address) {
    i: u64 = 0;
    for i in 0..32 {
        *(dst + i) = *(src + i);
    }
}
```

### While Loop

```vuma
current = (*list).next;
while current != list {
    process(current);
    current = (*current).next;
}
```

### If / Else

```vuma
if x > 100 {
    y = 2;
} else {
    y = 1;
}
```

```vuma
if value & 1 == 1 {
    // odd
}
```

Nested conditionals:

```vuma
if x > 0 {
    if x > 100 {
        result = 2;
    } else {
        result = 1;
    }
} else {
    result = 0;
}
```

### Infinite Loop

```vuma
loop {
    // runs forever (useful in embedded/bare-metal contexts)
}
```

### Break and Continue

```vuma
for i in 0..100 {
    if i == 50 {
        break;
    }
    if i % 2 == 0 {
        continue;
    }
    process(i);
}
```

---

## 6. Memory

VUMA provides two first-class memory operations: `allocate` and `free`. There is no garbage collector; memory management is manual.

### allocate(n)

Reserves `n` bytes of memory and returns an `Address` (pointer to the start of the block). The allocated memory is uninitialized.

```vuma
buf = allocate(64);     // 64-byte buffer
state = allocate(32);   // 32-byte state buffer
k_table = allocate(256); // 256-byte constant table
```

**The returned `Address` does not need a type annotation** — it is always inferred as `Address`:

```vuma
state = allocate(32);   // state has type Address
```

### free(ptr)

Releases the memory block pointed to by `ptr`. After `free(ptr)`, the pointer is invalid and must not be used.

```vuma
free(state);
free(k_table);
free(buf);
```

**Every `allocate` must be matched with a `free`**. Failing to free memory is a resource leak. The IVE (Invariant Verification Engine) checks this at compile time.

### Memory Layout Example

```vuma
fn sha256d(msg: Address, msg_len: u64, out: Address) {
    // Allocate working memory
    state = allocate(32);
    k = allocate(256);
    w = allocate(256);
    block = allocate(64);
    inner = allocate(32);

    // ... use the memory ...

    // Clean up all allocations
    free(state);
    free(k);
    free(w);
    free(block);
    free(inner);
}
```

### Region Declarations

Named regions can be declared with the `region` keyword:

```vuma
region pool = allocate(4096);
```

---

## 7. Pointer Operations

### Dereference: Load

Read a byte from memory at `ptr + offset`:

```vuma
b0: u32 = *(buf + offset);
```

The result is a single byte zero-extended to the type of the variable (typically `u32`).

### Dereference: Store

Write a byte to memory at `ptr + offset`:

```vuma
*(buf + offset) = value & 255;
```

Only the low 8 bits of `value` are stored.

### Pointer Arithmetic

Pointer + integer produces a new `Address` offset by that many bytes:

```vuma
ptr = base + 128;      // 128 bytes forward
byte3 = *(buf + 3);    // read byte at offset 3
*(buf + 4) = 99;       // write byte at offset 4
```

The offset can be an expression:

```vuma
*(buf + i * 4) = val;  // write at byte offset i*4
byte_val = *(buf + i); // read at byte offset i
```

### Read/Write Multi-byte Values (u32)

VUMA does not have native multi-byte load/store. You must compose byte-level accesses. For big-endian u32:

```vuma
fn write_u32_be(buf: Address, offset: u64, val: u32) {
    *(buf + offset) = (val >> 24) & 255;
    *(buf + offset + 1) = (val >> 16) & 255;
    *(buf + offset + 2) = (val >> 8) & 255;
    *(buf + offset + 3) = val & 255;
}

fn read_u32_be(buf: Address, offset: u64) -> u32 {
    b0: u32 = *(buf + offset);
    b1: u32 = *(buf + offset + 1);
    b2: u32 = *(buf + offset + 2);
    b3: u32 = *(buf + offset + 3);
    return ((b0 << 24) | (b1 << 16) | (b2 << 8) | b3) & 4294967295;
}
```

**Why the `& 4294967295` mask?** After left-shifting `b0` by 24 bits in a 64-bit register, bits above bit 31 may be set. The mask clears them. This is the most common source of bugs in VUMA code. See [Section 14: Pitfalls](#14-pitfalls).

### Address-Of Operator

The `@` operator takes the address of a variable:

```vuma
let x: i32 = 42;
ptr = @x;       // ptr has type Address, points to x
*ptr = 100;     // modifies x through the pointer
```

### Derive

The `derive(ptr, region)` expression explicitly creates a derived pointer within a specific region. This is the formal mechanism for sub-allocation:

```vuma
inner = derive(ptr, arena_region);
```

In practice, pointer arithmetic (`base + offset`) implicitly derives a new pointer.

---

## 8. Bitwise Operations

VUMA supports the standard bitwise operations:

| Operator | Name | Example |
|----------|------|---------|
| `&` | Bitwise AND | `a & 0xFF` |
| `\|` | Bitwise OR | `a \| b` |
| `^` | Bitwise XOR | `a ^ 0xFFFFFFFF` |
| `<<` | Left shift | `a << 8` |
| `>>` | Right shift | `a >> 4` |

### No Built-in NOT Operator

**VUMA does not have a reliable bitwise NOT (`~`) operator for u32 values.** The `~x` operator inverts all 64 bits of the register, which corrupts the upper 32 bits. Instead, use XOR with `0xFFFFFFFF`:

```vuma
// WRONG: ~x inverts bits 32-63 which corrupts u32 values
not_x = ~x;

// CORRECT: XOR with 0xFFFFFFFF inverts only the lower 32 bits
not_x: u32 = x ^ 4294967295;
```

**Always use `x ^ 4294967295` instead of `~x` for u32 bitwise NOT.**

### Bitwise AND for Masking

```vuma
low_byte: u32 = value & 255;          // mask to 8 bits
low_16: u32 = value & 65535;          // mask to 16 bits
low_32: u32 = value & 4294967295;     // mask to 32 bits
```

### Bitwise OR for Combining

```vuma
combined: u32 = (high << 8) | low;
```

### Left Shift

```vuma
shifted: u32 = (value << 24) & 4294967295;  // MUST mask after left shift
```

**Left shift on u32 values always requires masking** because the result can have bits set above bit 31.

### Right Shift

```vuma
upper: u32 = value >> 24;   // no mask needed for right shift
```

Right shift does not require masking because it only clears bits (never sets new bits above the existing ones).

### Rotate Right (Compose from Shifts)

VUMA does not have a built-in rotate operator. Compose it from shifts:

```vuma
fn rotr32(x: u32, n: u32) -> u32 {
    return ((x >> n) | (x << (32 - n))) & 4294967295;
}
```

The mask is required because `x << (32 - n)` sets bits above bit 31.

---

## 9. Arithmetic

### Basic Operators

| Operator | Name | Example |
|----------|------|---------|
| `+` | Addition | `a + b` |
| `-` | Subtraction | `a - b` |
| `*` | Multiplication | `a * b` |
| `/` | Division | `a / b` |
| `%` | Modulo | `a % b` |

### u32 Masking Rule

**All u32 arithmetic results must be masked with `& 4294967295`.** VUMA uses 64-bit registers on 64-bit targets. Arithmetic on u32 values can produce carries into the upper 32 bits:

```vuma
// WRONG: overflow bits pollute upper 32 bits
sum: u32 = a + b;

// CORRECT: mask to 32 bits
sum: u32 = (a + b) & 4294967295;
```

This applies to **all** arithmetic operations on u32 values:

```vuma
sum: u32 = (a + b) & 4294967295;
diff: u32 = (a - b) & 4294967295;
prod: u32 = (a * b) & 4294967295;
quot: u32 = (a / b) & 4294967295;
rem: u32 = (a % b) & 4294967295;
```

Division and modulo technically cannot overflow for u32, but masking is still recommended for consistency and safety.

### u64 Arithmetic

u64 arithmetic does **not** require masking:

```vuma
big_sum: u64 = a + b;    // OK, no mask needed
```

### i32 Arithmetic

Signed 32-bit arithmetic should also be masked for correctness:

```vuma
signed_result: i32 = (a + b) & 4294967295;
```

### Chained Arithmetic

When multiple u32 operations are chained, mask the **final** result:

```vuma
// SHA-256 T1 computation
t1: u32 = (h + big_sigma1(e) + ch(e, f, g) + ki + wi) & 4294967295;
```

Intermediate results in sub-expressions (like `big_sigma1(e)`) should also be masked in their function definitions. Every function that returns u32 must mask its return value.

---

## 10. Comparison

### Comparison Operators

| Operator | Name | Example |
|----------|------|---------|
| `==` | Equal | `a == b` |
| `!=` | Not equal | `a != b` |
| `<` | Less than | `a < b` |
| `<=` | Less than or equal | `a <= b` |
| `>` | Greater than | `a > b` |
| `>=` | Greater than or equal | `a >= b` |

### Usage

```vuma
if x == 0 {
    // handle zero case
}

if offset < 256 {
    // within bounds
}

if a != b {
    // not equal
}
```

### Comparison in Loops

```vuma
for i in 0..64 {
    if i < 16 {
        // first 16 iterations
    } else {
        // remaining iterations
    }
}
```

### Boolean Results

Comparison operators produce boolean values. However, in VUMA's compiled form, booleans are represented as integers (1 for true, 0 for false). This means they can be used in arithmetic:

```vuma
is_zero: u32 = (x == 0);  // 1 if x is 0, 0 otherwise
```

---

## 11. Constants

### Decimal Literals

```vuma
x: i32 = 42;
y: u32 = 4294967295;
z: u64 = 1000000;
```

### Hexadecimal Literals

Use the `0x` prefix:

```vuma
mask: u32 = 0xFF;           // 255
full: u32 = 0xFFFFFFFF;     // 4294967295
k_val: u32 = 0x428a2f98;    // SHA-256 round constant
byte: u32 = 0x80;           // 128 — padding byte
```

### Underscore Separators

Literals support underscore separators for readability:

```vuma
big: u64 = 1_000_000;
hex: u32 = 0xFF00_0000;
```

### Important: Decimal vs Hex for Masking

The u32 mask value `4294967295` is `0xFFFFFFFF`. Both notations are equivalent:

```vuma
x = value & 4294967295;   // decimal
x = value & 0xFFFFFFFF;   // hex (equivalent)
```

**In existing VUMA code, the decimal form `4294967295` is conventional for u32 masking.** When generating VUMA code, prefer the decimal form for consistency with the existing codebase.

### Common Constant Values

| Decimal | Hex | Meaning |
|---------|-----|---------|
| `255` | `0xFF` | 8-bit mask |
| `65535` | `0xFFFF` | 16-bit mask |
| `4294967295` | `0xFFFFFFFF` | 32-bit mask |
| `128` | `0x80` | SHA padding byte |

---

## 12. Calling Convention

### Argument Passing

On 64-bit targets, the first 8 integer/pointer arguments are passed in registers (X0-X7 on AArch64, RDI/RSI/RDX/RCX/R8/R9 on x86_64, a0-a7 on RISC-V). Additional arguments are passed on the stack.

On 32-bit targets (ARM32, Wasm32), the first 4 arguments use registers (r0-r3 on ARM32, the value stack on Wasm32).

### Return Values

The return value is passed in the first return register (X0 on AArch64, RAX on x86_64, a0 on RISC-V). For void functions, no return register is used.

### Address Parameters

`Address` parameters are passed like integer values (pointer-sized). On 64-bit targets, they occupy 8 bytes; on 32-bit targets, 4 bytes.

### Function Call Example

```vuma
fn add1(x: i32) -> i32 {
    return x + 1;
}

fn main() -> i32 {
    return add1(41);  // returns 42
}
```

### Cross-Function u32 Values

When a function returns `u32`, the caller receives the value in a 64-bit register. The upper 32 bits may contain garbage. The **returning function** must mask the value before returning, and the **calling function** should not assume the upper bits are clear unless the callee guarantees it.

```vuma
fn safe_add(a: u32, b: u32) -> u32 {
    return (a + b) & 4294967295;  // callee masks the result
}

fn main() -> i32 {
    result: u32 = safe_add(0xFFFFFFFF, 1);  // result = 0
    return 0;
}
```

### Stack Layout (AArch64)

```
Higher addresses
  +----------------------+
  | Incoming stack args   |  FP+16, FP+24, ...
  +----------------------+
  | Saved FP (X29)       |  FP+0
  | Saved LR (X30)       |  FP+8
  +----------------------+
  | Callee-saved regs    |  FP-8, FP-16, ...
  | (X19..X28)           |
  +----------------------+
  | Local variables      |
  | (aligned)            |
  +----------------------+
  | Outgoing stack args  |
  | (for nested calls)   |
  +----------------------+  SP
Lower addresses
```

---

## 13. Common Patterns

### Pattern 1: SHA256d (Double SHA-256)

This is the canonical VUMA program. It exercises all major language features: functions, memory, pointer arithmetic, bitwise operations, u32 masking, and loops.

```vuma
fn rotr32(x: u32, n: u32) -> u32 {
    return ((x >> n) | (x << (32 - n))) & 4294967295;
}

fn ch(x: u32, y: u32, z: u32) -> u32 {
    return (x & y) ^ ((x ^ 4294967295) & z);
}

fn maj(a: u32, b: u32, c: u32) -> u32 {
    return (a & b) ^ (a & c) ^ (b & c);
}

fn big_sigma0(x: u32) -> u32 {
    return rotr32(x, 2) ^ rotr32(x, 13) ^ rotr32(x, 22);
}

fn big_sigma1(x: u32) -> u32 {
    return rotr32(x, 6) ^ rotr32(x, 11) ^ rotr32(x, 25);
}

fn small_sigma0(x: u32) -> u32 {
    return (rotr32(x, 7) ^ rotr32(x, 18) ^ (x >> 3)) & 4294967295;
}

fn small_sigma1(x: u32) -> u32 {
    return (rotr32(x, 17) ^ rotr32(x, 19) ^ (x >> 10)) & 4294967295;
}

fn write_u32_be(buf: Address, offset: u64, val: u32) {
    *(buf + offset) = (val >> 24) & 255;
    *(buf + offset + 1) = (val >> 16) & 255;
    *(buf + offset + 2) = (val >> 8) & 255;
    *(buf + offset + 3) = val & 255;
}

fn read_u32_be(buf: Address, offset: u64) -> u32 {
    b0: u32 = *(buf + offset);
    b1: u32 = *(buf + offset + 1);
    b2: u32 = *(buf + offset + 2);
    b3: u32 = *(buf + offset + 3);
    return ((b0 << 24) | (b1 << 16) | (b2 << 8) | b3) & 4294967295;
}

fn main() -> i32 {
    msg = allocate(3);
    *(msg + 0) = 97;
    *(msg + 1) = 98;
    *(msg + 2) = 99;
    // ... SHA-256 compression ...
    free(msg);
    return 0;
}
```

### Pattern 2: Memory Buffer (Read/Write Bytes)

```vuma
fn main() -> i32 {
    buf = allocate(16);

    // Write bytes
    *(buf + 0) = 0x4f;
    *(buf + 1) = 0x8b;

    // Read bytes back
    b0: u32 = *(buf + 0);
    b1: u32 = *(buf + 1);

    free(buf);
    return 0;
}
```

### Pattern 3: Byte Manipulation (Extract/Insert)

```vuma
// Extract byte n from a u32 (big-endian)
fn get_byte(val: u32, n: u32) -> u32 {
    return (val >> (n * 8)) & 255;
}

// Set byte n in a u32 (big-endian)
fn set_byte(val: u32, n: u32, byte: u32) -> u32 {
    shift: u32 = n * 8;
    mask: u32 = 255 << shift;
    return (val & (mask ^ 4294967295)) | ((byte & 255) << shift) & 4294967295;
}
```

### Pattern 4: Memory Copy

```vuma
fn copy_bytes(dst: Address, src: Address, len: u64) {
    i: u64 = 0;
    for i in 0..len {
        *(dst + i) = *(src + i);
    }
}
```

### Pattern 5: Memory Set (Fill with Value)

```vuma
fn mem_set(buf: Address, len: u64, val: u32) {
    i: u64 = 0;
    for i in 0..len {
        *(buf + i) = val & 255;
    }
}
```

### Pattern 6: Array-style Access via Stride

When storing an array of u32 values in a byte buffer, each element occupies 4 bytes (stride = 4):

```vuma
fn w_store(w_base: Address, idx: u64, val: u32) {
    write_u32_be(w_base, idx * 4, val);
}

fn w_load(w_base: Address, idx: u64) -> u32 {
    return read_u32_be(w_base, idx * 4);
}
```

### Pattern 7: SHA-256 Compression Round

```vuma
for i in 0..64 {
    ki: u32 = read_u32_be(k, i * 4);
    wi: u32 = w_load(w, i);
    t1: u32 = (h + big_sigma1(e) + ch(e, f, g) + ki + wi) & 4294967295;
    t2: u32 = (big_sigma0(a) + maj(a, b, c)) & 4294967295;
    h = g;
    g = f;
    f = e;
    e = (d + t1) & 4294967295;
    d = c;
    c = b;
    b = a;
    a = (t1 + t2) & 4294967295;
}
```

---

## 14. Pitfalls

This section documents the most common mistakes when writing VUMA code. **Read this section carefully.**

### Pitfall 1: u32 Arithmetic Without Masking

**Problem**: VUMA uses 64-bit registers. When two u32 values are added, the 64-bit result can have bits set above bit 31 (the carry). Subsequent operations on this value (especially right shifts) will produce wrong results.

```vuma
// WRONG
sum: u32 = a + b;        // upper 32 bits may have carry
rotated: u32 = sum >> 4;  // wrong! shift includes garbage bits

// CORRECT
sum: u32 = (a + b) & 4294967295;  // clear upper 32 bits
rotated: u32 = sum >> 4;          // now correct
```

**Rule**: After every arithmetic operation (`+`, `-`, `*`) on u32 values, apply `& 4294967295`.

### Pitfall 2: Using `~` for Bitwise NOT

**Problem**: The `~` operator inverts all 64 bits. For a u32 value in a 64-bit register, this means bits 32-63 are also inverted, producing `0xFFFFFFFF????????` instead of `0x00000000????????`.

```vuma
// WRONG
not_x = ~x;  // produces 0xFFFFFFFF???????? for u32 x

// CORRECT
not_x: u32 = x ^ 4294967295;  // flips only lower 32 bits
```

**Rule**: Never use `~` for u32 bitwise NOT. Always use `x ^ 4294967295`.

### Pitfall 3: Left Shift Without Masking

**Problem**: Left-shifting a u32 value by N bits in a 64-bit register moves bits into the upper 32 bits.

```vuma
// WRONG
shifted: u32 = value << 24;  // bits above bit 31 are set

// CORRECT
shifted: u32 = (value << 24) & 4294967295;  // clear upper 32 bits
```

**Rule**: After any left shift of a u32 value, apply `& 4294967295`.

### Pitfall 4: Rotate Without Masking

**Problem**: A rotate-right composed from shifts produces bits above bit 31 from the left-shift component.

```vuma
// WRONG
rotated: u32 = (x >> n) | (x << (32 - n));  // upper bits polluted

// CORRECT
rotated: u32 = ((x >> n) | (x << (32 - n))) & 4294967295;
```

**Rule**: Always mask the result of a rotate operation.

### Pitfall 5: Forgetting to Free Memory

**Problem**: Every `allocate()` must be matched with a `free()`. The IVE checks this, but it is easy to forget in complex control flow.

```vuma
// WRONG — memory leak
fn bad() -> i32 {
    buf = allocate(64);
    return 0;  // buf is never freed!
}

// CORRECT
fn good() -> i32 {
    buf = allocate(64);
    // ... use buf ...
    free(buf);
    return 0;
}
```

**Rule**: Always pair `allocate()` with `free()`. Free in reverse order of allocation to match the typical stack discipline.

### Pitfall 6: Using `!x` for Bitwise NOT

**Problem**: The `!` operator is logical NOT (converts non-zero to 0 and 0 to 1), not bitwise NOT.

```vuma
// WRONG — logical NOT, not bitwise NOT
not_x = !x;  // produces 0 or 1, not bitwise complement

// CORRECT — bitwise NOT via XOR
not_x: u32 = x ^ 4294967295;
```

**Rule**: Use `x ^ 4294967295` for bitwise NOT, never `!x` or `~x`.

### Pitfall 7: Hex Literals as Byte Values in Stores

**Problem**: When storing a value to memory via `*(ptr + offset) = value`, only the low 8 bits are stored. Make sure you are not accidentally storing a truncated value.

```vuma
// This stores only the low 8 bits (0xC2)
*(buf + 0) = 0x4FC2;  // stores 0xC2, NOT 0x4FC2

// To store a specific byte, mask explicitly:
*(buf + 0) = (0x4FC2 >> 8) & 255;  // stores 0x4F
*(buf + 1) = 0x4FC2 & 255;        // stores 0xC2
```

### Pitfall 8: Exit Code Truncation

**Problem**: The process exit code is typically 8 bits (0-255) on Linux. When `main()` returns a u32 or i32, only the low byte is used as the exit code.

```vuma
fn main() -> i32 {
    // To exit with code 79 (0x4F):
    return 79;  // exit code = 79
}
```

If you need to verify a specific byte from a u32 computation, mask it and return:

```vuma
fn main() -> i32 {
    result: u32 = compute_something();
    return result & 255;  // return low byte as exit code
}
```

### Pitfall 9: Reading Bytes as u32 Without Masking

When you read a byte from memory into a `u32` variable, the value is zero-extended by the load. However, if you then shift and combine bytes, the result must be masked:

```vuma
// Each byte read is zero-extended to u32, but after shifting
// and combining, upper bits may be set
b0: u32 = *(buf + 0);  // e.g., 0x61
combined: u32 = (b0 << 24);  // 0x61000000 in 64-bit register, but
                               // might have upper bits
combined: u32 = (b0 << 24) & 4294967295;  // CORRECT
```

### Pitfall 10: Range Bounds in For Loops

The range `a..b` is exclusive on the upper bound. `0..64` means 0 through 63, not 0 through 64:

```vuma
for i in 0..64 {
    // i goes from 0 to 63 inclusive (64 iterations)
}

for i in 16..64 {
    // i goes from 16 to 63 inclusive (48 iterations)
}
```

---

## 15. Target Platforms

VUMA compiles to native machine code for 8 architectures plus Wasm. Each target has its own backend that translates the IR to machine code.

### Supported Targets

| Target | Status | Pointer Size | Register Width | Notes |
|--------|--------|-------------|----------------|-------|
| x86_64 | Stable | 8 bytes | 64-bit | System V AMD64 ABI |
| AArch64 | Stable | 8 bytes | 64-bit | AAPCS64 calling convention |
| RISC-V 64 | Stable | 8 bytes | 64-bit | RV64GC, LP64 ABI |
| ARM32 | Stable | 4 bytes | 32-bit | AAPCS, no u32 masking needed* |
| MIPS64 | Stable | 8 bytes | 64-bit | N64 ABI |
| PPC64 | Stable | 8 bytes | 64-bit | ELFv2 ABI |
| LoongArch64 | Experimental | 8 bytes | 64-bit | LP64 ABI, QEMU slow |
| Wasm32 | Experimental | 4 bytes | 32-bit | WebAssembly MVP |

*ARM32 uses 32-bit registers, so u32 masking is not needed on this target. However, **write code that works on all targets** by always masking u32 results.

### Cross-Compilation and Testing

The VUMA build system uses QEMU user-mode emulation to test cross-compiled binaries:

```bash
# Test on x86_64 (native on x86_64 hosts)
./sha256d_x86_64; echo $?

# Test on AArch64
qemu-aarch64 ./sha256d_aarch64; echo $?

# Test on RISC-V 64
qemu-riscv64-static ./sha256d_riscv64; echo $?

# Test on ARM32
qemu-arm ./sha256d_arm32; echo $?

# Test on MIPS64
qemu-mips64-static ./sha256d_mips64; echo $?

# Test on PPC64
qemu-ppc64 ./sha256d_ppc64; echo $?

# Test on LoongArch64
qemu-loongarch64 ./sha256d_loongarch64; echo $?
```

### Target-Specific Considerations

**64-bit targets (x86_64, AArch64, RISC-V 64, MIPS64, PPC64, LoongArch64)**:
- All use 64-bit general-purpose registers
- u32 values stored in 64-bit registers **must** be masked with `& 4294967295` after arithmetic and left shifts
- `Address` is 8 bytes
- Pointer arithmetic uses 64-bit addition

**32-bit targets (ARM32, Wasm32)**:
- Use 32-bit registers
- u32 masking is technically unnecessary but recommended for portability
- `Address` is 4 bytes
- Pointer arithmetic uses 32-bit addition

**Wasm32 specifics**:
- No native ROR/ROL instructions; rotates are synthesized from shift+or sequences
- Type inference can be tricky; explicit type annotations are important
- Stack machine architecture (no registers); the backend manages a virtual value stack

### Portability Rules

When writing VUMA code, follow these rules to ensure it works on all 8 targets:

1. **Always mask u32 results** with `& 4294967295` after addition, subtraction, multiplication, and left shift.
2. **Use `x ^ 4294967295`** for bitwise NOT, never `~x`.
3. **Compose rotates from shifts** with a final mask: `((x >> n) | (x << (32 - n))) & 4294967295`.
4. **Use explicit type annotations** for all variable declarations.
5. **Use `u64` for offsets and lengths**, not `u32`, to avoid truncation on large buffers.
6. **Always pair `allocate()` with `free()`**.

---

## Quick Reference Card

```
# Types
i8 i16 i32 i64 u8 u16 u32 u64 Address bool void

# Variable declaration
name: type = value;

# Assignment
name = value;
name = (expression) & 4294967295;  # for u32

# Function
fn name(param: type, ...) -> return_type { ... }

# Control flow
for i in 0..n { ... }
while condition { ... }
if condition { ... } else { ... }
loop { ... }

# Memory
ptr = allocate(n);
free(ptr);

# Pointer access
*(ptr + offset)              # load byte
*(ptr + offset) = value;     # store byte

# Bitwise
a & b    a | b    a ^ b    a << n    a >> n
NOT: x ^ 4294967295          # NOT ~x
ROR: ((x >> n) | (x << (32-n))) & 4294967295

# Arithmetic
(a + b) & 4294967295    # u32 add
(a - b) & 4294967295    # u32 sub
(a * b) & 4294967295    # u32 mul

# Comparison
==  !=  <  <=  >  >=

# Return
return value;
```

---

## End of Document

This reference covers all constructs needed to write correct VUMA programs. The most critical rules are:

1. **Mask all u32 arithmetic results** with `& 4294967295`.
2. **Never use `~`** for u32 bitwise NOT; use `^ 4294967295`.
3. **Always pair `allocate()` with `free()`**.
4. **Compose rotates from shifts** and mask the result.
5. **Use big-endian byte-level access** for multi-byte values.
