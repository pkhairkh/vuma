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
13. [Standard Library Modules](#13-standard-library-modules)
14. [Foreign Function Interface (FFI)](#14-foreign-function-interface-ffi)
15. [Atomic Operations](#15-atomic-operations)
16. [Debug Information](#16-debug-information)
17. [Common Patterns](#17-common-patterns)
18. [Pitfalls](#18-pitfalls)
19. [Error Codes](#19-error-codes)
20. [Target Platforms](#20-target-platforms)

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
| `f32` | 4 | IEEE 754 single-precision | N/A |
| `f64` | 8 | IEEE 754 double-precision | N/A |
| `bool` | 1 | `true` or `false` | N/A |
| `void` | 0 | N/A | N/A |

### Type Annotation Syntax

```vuma
x: i32 = 42;
y: u32 = 0xFFFFFFFF;
ptr: Address = allocate(64);
big: u64 = 18446744073709551615;
flag: bool = true;
pi: f64 = 3.141592653589793;
angle: f32 = 1.5707964;
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

## 13. Standard Library Modules

VUMA provides multiple standard library modules with VUMA-verified, BD-annotated functions: `fmt` (formatting), `math` (mathematical functions), `crypto` (cryptographic primitives), `string` (string/memory operations), and more.

### 13.1 The `fmt` Module (String Formatting)

The `fmt` module provides VUMA-verified string formatting primitives — the equivalent of C's `printf`/`sprintf` family. These functions convert numeric values to text representations in various bases, format floating-point numbers, pad strings, and write formatted output into byte buffers.

All `fmt` functions are pure (CapD: { Read, Compare }) except for buffer-writing functions which also declare { Write }.

#### Integer Formatting

```vuma
// Format a signed integer in the given base (2–36) with minimum width
s = fmt::format_int(value: i64, base: u32, width: u32);

// Format an unsigned integer in the given base with minimum width
s = fmt::format_uint(value: u64, base: u32, width: u32);

// Convenience wrappers for common bases:
s = fmt::format_hex(value: u64, width: u32);     // base 16, e.g. "0x000000ff"
s = fmt::format_binary(value: u64, width: u32);  // base 2, e.g. "11111111"
s = fmt::format_octal(value: u64, width: u32);   // base 8, e.g. "377"

// Format a pointer address as "0x" + 16-char hex
s = fmt::format_pointer(addr: u64);
```

**Key behaviors:**
- `base` must be 2–36; values outside that range are treated as base 10.
- `width` specifies the minimum number of digits; the output is left-padded with `'0'` to reach this width.
- For `format_int`, a negative value is prefixed with `'-'`; the sign occupies one character but is not counted as a digit for width purposes.
- `format_hex` uses lowercase hex digits (`a`–`f`).

**Examples:**

```vuma
// Decimal formatting
s = fmt::format_int(42, 10, 0);          // "42"
s = fmt::format_int(-7, 10, 4);          // "-0007"
s = fmt::format_uint(255, 10, 0);        // "255"

// Hex formatting
s = fmt::format_hex(255, 4);             // "00ff"
s = fmt::format_hex(0xDEADBEEF, 8);      // "deadbeef"

// Binary formatting
s = fmt::format_binary(5, 8);            // "00000101"

// Pointer formatting
s = fmt::format_pointer(0x7FFE4321);     // "0x000000007ffe4321"
```

#### Floating-Point Formatting

```vuma
// Format a float with exactly `precision` digits after the decimal point
s = fmt::format_float(value: f64, precision: u32);
```

**Special values:**
- NaN → `"nan"`
- Positive infinity → `"inf"`
- Negative infinity → `"-inf"`
- Zero with precision 0 → `"0"`, with precision > 0 → `"0.00..."`

**Examples:**

```vuma
s = fmt::format_float(3.14159, 2);       // "3.14"
s = fmt::format_float(3.14159, 6);       // "3.141590"
s = fmt::format_float(0.0, 0);           // "0"
s = fmt::format_float(0.0/0.0, 2);       // "nan"
```

#### String Padding and Joining

```vuma
// Left-pad a string to `width` characters with `fill`
s = fmt::pad_left(s: &str, width: u32, fill: char);

// Right-pad a string to `width` characters with `fill`
s = fmt::pad_right(s: &str, width: u32, fill: char);

// Join string slices with a separator
s = fmt::join(parts: &[&str], separator: &str);
```

**Examples:**

```vuma
s = fmt::pad_left("42", 6, '0');         // "000042"
s = fmt::pad_left("hello", 3, ' ');      // "hello" (already wider)
s = fmt::pad_right("abc", 6, '-');        // "abc---"
s = fmt::join(["a", "b", "c"], ", ");     // "a, b, c"
```

### 13.2 The `math` Module (Mathematical Functions)

The `math` module provides VUMA-verified mathematical helper functions. All functions are pure (CapD: { Read, Compare }).

#### Mathematical Constants (f64)

| Constant | Value (approx.) | Description |
|----------|-----------------|-------------|
| `math::PI` | 3.141592653589793 | Archimedes' constant (π) |
| `math::TAU` | 6.283185307179586 | Full circle constant (τ = 2π) |
| `math::E` | 2.718281828459045 | Euler's number |
| `math::LN_2` | 0.693147180559945 | Natural logarithm of 2 |
| `math::LN_10` | 2.302585092994046 | Natural logarithm of 10 |
| `math::LOG2_E` | 1.442695040888963 | Log base 2 of e |
| `math::LOG10_E` | 0.434294481903252 | Log base 10 of e |
| `math::SQRT_2` | 1.414213562373095 | Square root of 2 |
| `math::FRAC_1_SQRT_2` | 0.707106781186548 | 1/√2 |

**f32 constants** have `_F32` suffix: `math::PI_F32`, `math::E_F32`, etc.

#### Integer Arithmetic

```vuma
// Absolute value (i64). WARNING: abs(i64::MIN) wraps to i64::MIN
result: i64 = math::abs(x: i64);

// Minimum and maximum of two i64 values
result: i64 = math::min(a: i64, b: i64);
result: i64 = math::max(a: i64, b: i64);

// Clamp x to the range [lo, hi] (panics if lo > hi)
result: i64 = math::clamp(x: i64, lo: i64, hi: i64);
```

#### Trigonometric Functions (f64)

```vuma
y: f64 = math::sin(x: f64);       // sine
y: f64 = math::cos(x: f64);       // cosine
y: f64 = math::tan(x: f64);       // tangent
y: f64 = math::asin(x: f64);      // arcsine
y: f64 = math::acos(x: f64);      // arccosine
y: f64 = math::atan(x: f64);      // arctangent
y: f64 = math::atan2(y: f64, x: f64);  // two-argument arctangent
y: f64 = math::sinh(x: f64);      // hyperbolic sine
y: f64 = math::cosh(x: f64);      // hyperbolic cosine
y: f64 = math::tanh(x: f64);      // hyperbolic tangent
```

**f32 variants** have `_f32` suffix: `math::sin_f32`, `math::cos_f32`, etc.

#### Exponential and Logarithmic Functions (f64)

```vuma
y: f64 = math::sqrt(x: f64);      // square root
y: f64 = math::cbrt(x: f64);      // cube root
y: f64 = math::exp(x: f64);       // e^x
y: f64 = math::exp2(x: f64);      // 2^x
y: f64 = math::exp_m1(x: f64);    // e^x - 1 (accurate for small x)
y: f64 = math::ln(x: f64);        // natural logarithm
y: f64 = math::log2(x: f64);      // base-2 logarithm
y: f64 = math::log10(x: f64);     // base-10 logarithm
y: f64 = math::ln_1p(x: f64);     // ln(1+x) (accurate for small x)
y: f64 = math::pow(x: f64, y: f64);   // x^y
y: f64 = math::powi(x: f64, n: i32);  // x^n (integer exponent)
```

**f32 variants** have `_f32` suffix: `math::sqrt_f32`, `math::exp_f32`, etc.

#### Rounding Functions (f64)

```vuma
y: f64 = math::floor(x: f64);     // round toward −∞
y: f64 = math::ceil(x: f64);      // round toward +∞
y: f64 = math::round(x: f64);     // round to nearest, ties away from zero
y: f64 = math::trunc(x: f64);     // round toward zero (drop fractional part)
y: f64 = math::fract(x: f64);     // fractional part (x − trunc(x))
```

#### Comparison Functions (f64)

```vuma
y: f64 = math::min_of(a: f64, b: f64);  // minimum of two f64 values
y: f64 = math::max_of(a: f64, b: f64);  // maximum of two f64 values
```

#### Classification Functions (f64)

```vuma
b: bool = math::is_nan(x: f64);       // true if x is NaN
b: bool = math::is_infinite(x: f64);   // true if x is +∞ or −∞
b: bool = math::is_finite(x: f64);     // true if x is neither NaN nor infinite
b: bool = math::is_normal(x: f64);     // true if x is a normal (non-zero, non-subnormal) float
s: f64  = math::signum(x: f64);        // −1.0, 0.0, or 1.0
c: f64  = math::copysign(x: f64, y: f64); // magnitude of x with sign of y
```

#### Example: Using math and fmt Together

```vuma
fn main() -> i32 {
    // Compute the hypotenuse of a 3-4-5 triangle
    a: f64 = 3.0;
    b: f64 = 4.0;
    c_sq: f64 = a * a + b * b;          // 25.0
    c: f64 = math::sqrt(c_sq);           // 5.0

    // Format the result
    s = fmt::format_float(c, 2);         // "5.00"

    // Convert to integer exit code
    return c as i32;                      // returns 5
}
```

### 13.3 The `crypto` Module (Cryptographic Primitives)

The `crypto` module provides SHA-256 constants, logical functions, byte access helpers, and constant-time operations:

```vuma
// SHA-256 logical functions
result: u32 = crypto::sha256_ch(x: u32, y: u32, z: u32);
result: u32 = crypto::sha256_maj(a: u32, b: u32, c: u32);
result: u32 = crypto::sha256_big_sigma0(x: u32);
result: u32 = crypto::sha256_big_sigma1(x: u32);
result: u32 = crypto::sha256_small_sigma0(x: u32);
result: u32 = crypto::sha256_small_sigma1(x: u32);

// Big-endian byte access
val: u32 = crypto::sha256_read_u32_be(buf: Address, offset: u64);
crypto::sha256_write_u32_be(buf: Address, offset: u64, val: u32);

// Constant-time operations (no data-dependent branches — safe against timing attacks)
equal: u32 = crypto::ct_eq_u32(a: u32, b: u32);     // 1 if equal, 0 otherwise
neq: u32 = crypto::ct_ne_u32(a: u32, b: u32);        // 1 if not equal
selected: u32 = crypto::ct_select_u32(flag: u32, a: u32, b: u32);  // a if flag=1, b if flag=0
lt: u32 = crypto::ct_lt_u32(a: u32, b: u32);         // 1 if a < b
gte: u32 = crypto::ct_gte_u32(a: u32, b: u32);       // 1 if a >= b

// SHA-256 constants
// crypto::SHA256_K — 64 u32 round constants
// crypto::SHA256_H — 8 u32 initial hash values
```

**Critical:** Constant-time functions execute in the same number of cycles regardless of input values. Use them whenever comparing secret data to prevent timing side-channels. All 10 backends implement these with branchless codegen.

### 13.4 The `string` Module (String and Memory Operations)

```vuma
// String operations
len: u64 = string::strlen(ptr: Address);
cmp: i32 = string::strcmp(a: Address, b: Address);

// Memory operations
string::memcpy(dst: Address, src: Address, len: u64);
string::memset(ptr: Address, value: u32, len: u64);
```

### 13.5 Additional Standard Library Modules

VUMA also provides these modules (available via `import`):

- **`io`**: I/O traits (`VumaReader`, `VumaWriter`), buffered I/O, low-level syscall wrappers (`read_bytes`, `write_bytes`), little-endian byte access (`read_u32_le`, `write_u32_le`)
- **`sync`**: `Mutex`, `RwLock`, `Channel`, `Barrier` — all BD-annotated
- **`collections`**: `Vec`, `HashMap`, `RingBuffer`, `DoublyLinkedList` — VUMA-VERIFIED
- **`alloc`**: `GlobalAllocator`, `ArenaAllocator`, `BumpAllocator`, `PoolAllocator`, `FreeListAllocator`
- **`fs`**: Filesystem operations with capability-based access control
- **`net`**: TCP/UDP socket I/O
- **`env`**: Environment variable access
- **`path`**: Path manipulation and normalization
- **`process`**: Process management
- **`thread`**: Thread creation and management
- **`time`**: Time measurement and duration types

---

## 14. Foreign Function Interface (FFI)

VUMA supports calling external C functions through the `extern "C"` block syntax. This allows VUMA programs to interface with the system's C library, Linux syscalls, and other native code.

### 14.1 The `extern "C"` Block

Use an `extern "C" { ... }` block at the top level to declare external functions:

```vuma
extern "C" {
    fn write(fd: i64, buf: Address, count: i64) -> i64;
    fn read(fd: i64, buf: Address, count: i64) -> i64;
    fn exit(code: i64);
}
```

**Syntax rules:**
- The block must appear at the top level (not inside a function).
- The convention string is `"C"` (the only currently supported convention; `"system"` is reserved).
- Each function signature ends with `;` (no body — the implementation is external).
- Parameters use the same type syntax as regular VUMA functions.
- Functions without a return type are `void` (like `exit` above).

### 14.2 Calling External Functions

Once declared, extern functions are called like any other VUMA function:

```vuma
fn main() -> i64 {
    let msg_addr: Address = 0x400000;
    let msg_len: i64 = 21;
    let result: i64 = write(1, msg_addr, msg_len);
    exit(0);
    return result;   // unreachable — exit() does not return
}
```

### 14.3 How It Works (Codegen Details)

When the compiler encounters a call to an `extern` function:

1. **No local `BL` instruction** — Instead of emitting a direct branch, the codegen produces a **relocation** entry in the `.rela.text` section of the ELF object file.
2. **Symbol table** — The extern function is recorded as an `SHN_UNDEF` symbol in the ELF symbol table, indicating it must be resolved by the linker.
3. **Linking** — The system linker (`ld`) resolves the relocations against the C library or other object files.

**Compilation pipeline for FFI programs:**

```bash
# Compile VUMA source to a relocatable object (.o)
vuma compile --format obj --target aarch64 ffi_demo.vuma -o ffi_demo.o

# Link with the C library
ld -o ffi_demo ffi_demo.o -lc
```

### 14.4 Built-in Syscall Bindings

The VUMA compiler recognizes 19 Linux syscalls (enum `SyscallName` in `src/ffi.rs:478`). Each backend emits raw machine-code syscall stubs for these. Programs must still declare the ones they want to call via `extern "C"` blocks — the stubs make the calls linkable, they do not auto-import the symbols.

**Linux syscalls (19):**
`read`, `write`, `open`, `close`, `exit`, `exit_group`, `mmap`, `munmap`, `brk`, `ioctl`, `fcntl`, `getpid`, `kill`, `mprotect`, `clock_gettime`, `sched_yield`, `clone`, `futex`, `set_tid_address`

**Runtime helpers (not syscalls, emitted alongside stubs on some backends):**
- `__vuma_alloc(size)` — mmap wrapper (bump allocator on Wasm32); heap memory that persists across function calls
- `__vuma_free(addr, size)` — munmap wrapper (no-op on Wasm32)
- `memcpy(dst, src, n)` / `memset(dst, c, n)` / `strcmp(a, b)` — emitted by some backends as helper symbols
- `print_int(x)` / `print_hex(x)` — emitted by x86_64/x86_32 as debug helpers

> **Note:** VUMA does NOT provide `malloc`/`free` from a C library. Use `__vuma_alloc`/`__vuma_free` for heap memory, or the `allocate(size)` language builtin for stack-local (or ≤4096-byte heap in the canonical pipeline) memory.

### 14.5 Pitfalls for FFI

- **No type checking across the boundary**: The compiler cannot verify the types of external C functions. If you declare `fn write(fd: i32, ...)` but the C function expects `i64`, the behavior is undefined.
- **Calling convention must match**: The `"C"` convention uses the platform's standard C ABI (System V AMD64 on Linux/x86_64, AAPCS64 on AArch64, etc.).
- **Linking is required**: FFI programs cannot run as standalone ET_REL objects. They must be linked with `ld` or `gcc`.
- **E037 relocation errors**: If a relocation cannot be resolved (e.g., symbol not found, overflow), the linker emits error E037.

---

## 15. Atomic Operations

VUMA provides three atomic memory operations for concurrent programming. These compile to target-specific atomic instructions.

### 15.1 AtomicLoad

Atomically loads a value from memory with acquire semantics.

```vuma
value = AtomicLoad(addr);
```

**Target-specific lowering:**
- AArch64: `LDAXR`
- x86_64: `LOCK` prefix or plain `MOV` (x86 is already atomic for aligned loads)
- RISC-V: `LR.D`

### 15.2 AtomicStore

Atomically stores a value to memory with release semantics.

```vuma
AtomicStore(value, addr);
```

**Target-specific lowering:**
- AArch64: `STLXR`
- x86_64: `LOCK` prefix or plain `MOV`
- RISC-V: `SC.D`

### 15.3 AtomicCas (Compare-and-Swap)

Atomically compares the value at `addr` with `expected`. If equal, writes `desired` to `addr`. Returns the old value at `addr`.

```vuma
old_value = AtomicCas(addr, expected, desired);
```

**Target-specific lowering:**
- AArch64: `LDAXR` / `CMP` / `B.NE` / `STLXR` loop
- x86_64: `LOCK CMPXCHG`
- RISC-V: `LR.D` / `BNE` / `SC.D` loop

### 15.4 Example: Atomic Counter

```vuma
fn atomic_increment(counter: Address) -> u64 {
    old: u64 = AtomicLoad(counter);
    loop {
        new_val: u64 = old + 1;
        prev: u64 = AtomicCas(counter, old, new_val);
        if prev == old {
            return new_val;
        }
        old = prev;
    }
}
```

**Note:** Atomic operations are only available on 64-bit backends (x86_64, AArch64, RISC-V 64). The ARM32 and Wasm32 backends have limited atomic support.

---

## 16. Debug Information

VUMA can emit DWARF v4 debug information in the ELF output binary when the `--debug` flag is passed.

### 16.1 The `--debug` Flag

```bash
# Compile with debug info (alias: --debug-info)
vuma build program.vu --debug -o program

# Emit to specific target with debug info
vuma emit aarch64 program.vu --debug -o program.elf
```

When `--debug` is enabled, the compiler emits the following DWARF sections in the ELF binary:

| Section | Contents |
|---------|----------|
| `.debug_info` | Compilation unit metadata, subprogram definitions, variable types |
| `.debug_abbrev` | Abbreviation tables for `.debug_info` encoding |
| `.debug_line` | Line-number table mapping instruction offsets to source locations |
| `.debug_frame` | Call frame information (CIE + FDE entries) for stack unwinding |

### 16.2 Per-Architecture CIE Presets

The Common Information Entry (CIE) in `.debug_frame` is pre-configured for each target:

| Target | Stack Pointer | Return Address | Frame Pointer | Code Align | Data Align |
|--------|--------------|----------------|---------------|------------|------------|
| AArch64 | X31 (SP) | X30 (LR) | X29 (FP) | 4 | −8 |
| x86_64 | RSP (7) | — | RBP (6) | 1 | −8 |
| RISC-V 64 | SP (2) | RA (1) | FP (8) | 2 | −8 |
| ARM32 | SP (13) | LR (14) | FP (11) | 2 | −4 |
| MIPS64 | SP (29) | RA (31) | — | 4 | −8 |
| PPC64 | R1 | LR (65) | — | 4 | −8 |
| LoongArch64 | r3 (SP) | r1 (RA) | r22 (FP) | 4 | −8 |
| Wasm32 | — | — | — | 1 | — |

### 16.3 Using Debug Info

With `--debug`, the compiled binary can be debugged with standard tools:

```bash
# Debug with GDB
gdb ./program

# Disassemble with source lines
objdump -d -l ./program

# Inspect DWARF sections
readelf --debug-dump=info ./program
readelf --debug-dump=line ./program
readelf --debug-dump=frame ./program
```

Without `--debug`, the ELF binary does **not** contain any `.debug_*` sections.

---

## 17. Common Patterns

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

## 18. Pitfalls

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

### Pitfall 11: FFI Type Mismatches

**Problem**: The compiler cannot verify that the parameter types declared in an `extern "C"` block match the actual C function signature. A mismatch causes undefined behavior at runtime.

```vuma
// WRONG: C's write() expects size_t (u64) for count, not i32
extern "C" {
    fn write(fd: i64, buf: Address, count: i32) -> i64;  // BUG: count should be i64
}

// CORRECT: match the C ABI exactly
extern "C" {
    fn write(fd: i64, buf: Address, count: i64) -> i64;
}
```

**Rule**: Always consult the C header documentation when declaring `extern "C"` functions. Use `i64`/`u64` for pointer-sized C types (`size_t`, `ssize_t`, `intptr_t`).

### Pitfall 12: AtomicCas Retry Loop

**Problem**: `AtomicCas` can fail spuriously (the value at `addr` was changed by another thread between the load and the CAS). You must loop until the CAS succeeds:

```vuma
// WRONG: single CAS attempt may fail
fn bad_increment(counter: Address) -> u64 {
    old: u64 = AtomicLoad(counter);
    new_val: u64 = old + 1;
    prev: u64 = AtomicCas(counter, old, new_val);
    return new_val;  // may return wrong value if CAS failed!
}

// CORRECT: retry loop
fn atomic_increment(counter: Address) -> u64 {
    old: u64 = AtomicLoad(counter);
    loop {
        new_val: u64 = old + 1;
        prev: u64 = AtomicCas(counter, old, new_val);
        if prev == old {
            return new_val;
        }
        old = prev;
    }
}
```

**Rule**: Always wrap `AtomicCas` in a retry loop. Check that the returned old value matches the expected value.

---

## 19. Error Codes

VUMA produces structured diagnostics with error codes. These codes are organized into ranges:

| Range | Category | Description |
|-------|----------|-------------|
| E001–E030 | Compilation | Syntax, type, name resolution errors |
| E031–E040 | Codegen | Register allocation, encoding, relocation |
| E041–E050 | Verification | Invariant violations, proof failures |
| W001–W010 | Warnings | Unused vars, performance hints |
| I001–I005 | Informational | General compiler information |

### Compilation Errors (E001–E030)

| Code | Description |
|------|-------------|
| E001 | Syntax error |
| E002 | Undefined variable |
| E003 | Type mismatch |
| E004 | Duplicate definition |
| E005 | Missing return |
| E006 | Parameter count mismatch |
| E007 | Invalid assignment target |
| E008 | Missing main function |
| E009 | Recursive type |
| E010 | Undefined type |
| E011 | Invalid operation |
| E012 | Shadowing violation |
| E013 | Invalid region |
| E014 | Missing free |
| E015 | Dead pointer use |
| E016 | Double free |
| E017 | Invalid dereference |
| E018 | Address arithmetic overflow |
| E019 | Region bounds violation |
| E020 | Incompatible capabilities |
| E021 | Invalid extern block |
| E022 | Duplicate extern function |
| E023 | Invalid calling convention |
| E024 | Missing entry point |
| E025 | Unreachable code |
| E026 | Unused variable |
| E027 | Break/continue outside loop |
| E028 | Invalid cast |
| E029 | Missing function body |
| E030 | Invalid visibility modifier |

### Codegen Errors (E031–E040)

| Code | Description | Common Cause |
|------|-------------|--------------|
| E031 | Invalid instruction | Unsupported IR opcode for target |
| E032 | Register allocation failed | Too many live values, spilling overflow |
| E033 | Encoding error | Cannot encode instruction for target ISA |
| E034 | IR translation error | Failed to translate SCG to IR |
| E035 | ELF emission error | Failed to write ELF binary |
| E036 | Wasm section not found | Missing required Wasm section |
| **E037** | **Relocation error** | **Unresolved external symbol, relocation overflow — common when `extern "C"` function not found by linker** |
| E038 | Stack layout error | Misaligned stack, frame too large |
| E039 | Linker error | Undefined symbol, missing library |
| E040 | Target unsupported feature | Feature not available on this ISA |

**E037 (Relocation error)** is particularly important for FFI programs. It occurs when:
- An `extern "C"` function is declared but not found by the linker
- A relocation offset overflows the available bits for the target ISA
- The ELF object is compiled for a different architecture than the linker expects

### Verification Errors (E041–E050)

| Code | Description |
|------|-------------|
| E041 | Liveness invariant violation |
| E042 | Exclusivity invariant violation |
| E043 | Interpretation invariant violation |
| E044 | Origin invariant violation |
| E045 | Cleanup invariant violation |
| E046 | Proof verification failed |
| E047 | Counterexample found |
| E048 | BD inference failed |
| E049 | Capability conflict |
| E050 | Relational constraint violated |

---

## 20. Target Platforms

VUMA's codegen crate implements 10 backend architectures (enum `BackendKind`). The CLI `vuma emit`/`vuma compile` commands accept 8 ISA targets (enum `IsaArg`: aarch64, x86_64, riscv64, wasm32, loongarch64, arm32, mips64, ppc64). All 10 backends are exercised by the `compile_dump` test binary. Each target has its own backend that translates the IR to machine code.

### Supported Targets

| Target | Status | Pointer Size | Register Width | CLI-emittable | Notes |
|--------|--------|-------------|----------------|---------------|-------|
| x86_64 | Stable | 8 bytes | 64-bit | yes | System V AMD64 ABI, DWARF debug info, FFI, atomics |
| AArch64 | Stable | 8 bytes | 64-bit | yes | AAPCS64 calling convention, DWARF debug info, FFI, atomics |
| RISC-V 64 | Stable | 8 bytes | 64-bit | yes | RV64GC, LP64 ABI, DWARF debug info, FFI, atomics |
| ARM32 | Stable | 4 bytes | 32-bit | yes | AAPCS, DWARF debug info, FFI, atomics |
| MIPS64 | Stable | 8 bytes | 64-bit | yes | N64 ABI, big-endian, DWARF debug info, FFI |
| PPC64 | Stable | 8 bytes | 64-bit | yes | ELFv2 ABI, big-endian, DWARF debug info, FFI |
| LoongArch64 | Stable | 8 bytes | 64-bit | yes | LP64 ABI, DWARF debug info, FFI |
| x86_32 | Stable | 4 bytes | 32-bit | no (codegen only) | cdecl, DWARF debug info, FFI |
| RISC-V 32 | Stable | 4 bytes | 32-bit | no (codegen only) | ILP32 ABI, DWARF debug info, FFI |
| Wasm32 | Stable | 4 bytes | 32-bit | yes | WebAssembly MVP, bump allocator (no mmap), limited atomics |

Latest full-suite results (`test_results/summary.json`, 2026-07-01): 57,377/57,380 runs pass (99.99%). 3 failures: `crc32.vuma` on riscv64+ppc64, `s27_fn_two_args_mod.vuma` on ppc64.

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

**32-bit targets (ARM32, x86_32, RISC-V 32, Wasm32)**:
- Use 32-bit registers
- u32 masking is technically unnecessary but recommended for portability
- `Address` is 4 bytes
- Pointer arithmetic uses 32-bit addition

**Wasm32 specifics**:
- No native ROR/ROL instructions; rotates are synthesized from shift+or sequences
- Type inference can be tricky; explicit type annotations are important
- Stack machine architecture (no registers); the backend manages a virtual value stack

### Portability Rules

When writing VUMA code, follow these rules to ensure it works on all 10 targets:

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
i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 Address bool void

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

# Standard library — fmt module
fmt::format_int(value, base, width)
fmt::format_uint(value, base, width)
fmt::format_hex(value, width)
fmt::format_binary(value, width)
fmt::format_octal(value, width)
fmt::format_float(value, precision)
fmt::format_pointer(addr)
fmt::pad_left(s, width, fill)
fmt::pad_right(s, width, fill)
fmt::join(parts, separator)

# Standard library — math module
math::sin  math::cos  math::tan  math::sqrt  math::exp
math::ln   math::log2  math::log10  math::pow
math::abs  math::min  math::max  math::clamp
math::floor  math::ceil  math::round  math::trunc
math::PI  math::E  math::TAU
# f32 variants: math::sin_f32, math::sqrt_f32, etc.

# FFI — extern "C" blocks
extern "C" {
    fn name(param: type, ...) -> return_type;
}

# Atomic operations
AtomicLoad(addr)
AtomicStore(value, addr)
AtomicCas(addr, expected, desired)

# Debug info
vuma build program.vu --debug -o program
```

---

## End of Document

This reference covers all constructs needed to write correct VUMA programs, including the standard library (fmt, math), FFI, atomics, and debug information. The most critical rules are:

1. **Mask all u32 arithmetic results** with `& 4294967295`.
2. **Never use `~`** for u32 bitwise NOT; use `^ 4294967295`.
3. **Always pair `allocate()` with `free()`**.
4. **Compose rotates from shifts** and mask the result.
5. **Use big-endian byte-level access** for multi-byte values.
6. **Match C ABI types exactly** in `extern "C"` blocks.
7. **Always retry `AtomicCas`** in a loop until it succeeds.
8. **Use `--debug`** when you need DWARF debug info in the output binary.
