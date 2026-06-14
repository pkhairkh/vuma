# MIPS64 Backend Bug Analysis — SHA256d Failure

## Summary

The MIPS64 backend fails SHA256d (exit 181 instead of 79) and test_sha_manual
(exit 110 on MIPS64 vs 205 on x86_64/aarch64). The root cause is a **critical
offset error in the `Alloc` instruction**: the `alloc_offsets` map stores the
stack offset **before** incrementing by the allocation size, while every other
backend (RISC-V64, PPC64, ARM32, x86_64, LoongArch64) stores it **after**.
This causes `Alloc` to return a pointer that is `size` bytes too high on the
stack, pointing to the **end** of the allocated region instead of the
**beginning**. All subsequent memory reads/writes through that pointer access
wrong addresses, corrupting vreg values and producing incorrect SHA256 results.

Two secondary bugs were also found: (1) `ShrA` produces wrong results for
negative 32-bit values due to zero-extension before the 64-bit arithmetic shift,
and (2) function arguments 5+ are silently dropped instead of being passed on
the stack per the N64 ABI.

---

## Bug 1 (CRITICAL): Alloc Offset Off-By-Size

**File:** `src/codegen/src/mips64/mod.rs`
**Lines:** 2172–2176 (offset computation) and 2651–2664 (Alloc handler)

### Problem

The MIPS64 backend stores `alloc_offsets[id]` **before** incrementing
`current_offset` by the allocation size:

```rust
// WRONG — lines 2172-2176
for &id in &alloc_vreg_ids {
    let size = alloc_sizes[&id];
    alloc_offsets.insert(id, current_offset);  // ← stored BEFORE increment
    current_offset += size;
}
```

Every other backend stores it **after** the increment:

```rust
// CORRECT — RISC-V64, lines 3986-3990
for &id in &alloc_vreg_ids {
    let size = alloc_sizes[&id];
    current_offset += size;                    // ← increment FIRST
    alloc_offsets.insert(id, current_offset);  // ← stored AFTER increment
}
```

Same correct pattern in PPC64 (line 2766–2767), ARM32 (line 2978–2979),
x86_64 (line 150–151), and LoongArch64 (line 377).

### How the Bug Manifests

The `Alloc` instruction handler computes the pointer as `$fp - alloc_off`:

```rust
// Line 2656
code.extend_from_slice(&Instruction::Daddiu { rt: Gpr::T0, rs: Gpr::Fp, imm: -(alloc_off) }.encode());
```

With the buggy offset, the pointer is `$fp - current_offset_before`, which
points to the **top** (highest address) of the allocation region. The correct
pointer should be `$fp - (current_offset_before + size)`, pointing to the
**bottom** (lowest address / start) of the region.

### Concrete Example

For `state = allocate(32)` in test_sha_manual, with 50 vregs before the
alloc region:

| Item | current_offset | alloc_offsets[state] | Pointer computed |
|------|---------------|---------------------|-----------------|
| After vregs | 424 | — | — |
| Alloc state (size=32) | 424 → 456 | **424** (wrong) | $fp - 424 |
| Correct | 456 | 456 | $fp - 456 |

The pointer `$fp - 424` is **32 bytes too high** — it points into the vreg
slot area above the actual allocation. When `write_u32(state, 0, ...)` writes
to `state[0]`, it overwrites vreg data. When `read_u32(state, 0)` reads back,
it reads corrupted vreg values.

For `w = allocate(256)`, the pointer is **256 bytes too high**. For
`block = allocate(64)`, it's **64 bytes too high**. This causes massive data
corruption across all buffer operations.

### Why test_u32_arith Passes

The `test_u32_arith` test only uses function calls for u32 arithmetic — it
does not use `allocate()` or any `Alloc` instructions. Therefore this bug has
no effect on that test.

### Why test_exit and test_call Pass

These tests don't use `Alloc` either — they just return constants or call
simple functions.

### Proposed Fix

Change lines 2172–2176 from:

```rust
for &id in &alloc_vreg_ids {
    let size = alloc_sizes[&id];
    alloc_offsets.insert(id, current_offset);
    current_offset += size;
}
```

To:

```rust
for &id in &alloc_vreg_ids {
    let size = alloc_sizes[&id];
    current_offset += size;
    alloc_offsets.insert(id, current_offset);
}
```

This matches every other backend and ensures `alloc_offsets[id]` equals
`current_offset + size`, so the pointer `$fp - alloc_off` correctly points to
the start of the allocation region.

---

## Bug 2 (MODERATE): ShrA Zero-Extension Breaks Arithmetic Right Shift of Negatives

**File:** `src/codegen/src/mips64/mod.rs`
**Lines:** 2369–2372 (zero-extension) and 2438 (ShrA handler)

### Problem

The BinOp handler unconditionally zero-extends both operands with
`DSLL 32 + DSRL 32` before every operation (lines 2369–2372). For the
`ShrA` (arithmetic right shift) case, this clears the sign bit of
negative 32-bit values before the shift, making `DSRAV` fill with 0s instead
of 1s:

```rust
// Lines 2369-2372: zero-extends T0 and T1 (clears upper 32 bits)
code.extend_from_slice(&Instruction::Dsll { rd: Gpr::T0, rt: Gpr::T0, sa: 32 }.encode());
code.extend_from_slice(&Instruction::Dsrl { rd: Gpr::T0, rt: Gpr::T0, sa: 32 }.encode());
code.extend_from_slice(&Instruction::Dsll { rd: Gpr::T1, rt: Gpr::T1, sa: 32 }.encode());
code.extend_from_slice(&Instruction::Dsrl { rd: Gpr::T1, rt: Gpr::T1, sa: 32 }.encode());

// Line 2438: 64-bit arithmetic right shift on zero-extended value
BinOpKind::ShrA => {
    code.extend_from_slice(&Instruction::Dsrav { rd: Gpr::T0, rt: Gpr::T0, rs: Gpr::T1 }.encode());
}
```

For `x = 0x80000000` (i32 = -2147483648), `x >> 1` should give `0xC0000000`
(-1073741824), but:
1. Zero-extend: `0x00000000_80000000`
2. DSRAV by 1: `0x00000000_40000000` (fills with 0s, not 1s)
3. Result: `0x40000000` (wrong)

The correct result requires sign-extension (not zero-extension) before DSRAV.
For a 32-bit value stored as `0x00000000_80000000`, you need to sign-extend
it to `0xFFFFFFFF_80000000` first, then DSRAV gives `0xFFFFFFFF_C0000000`,
and masking to 32 bits gives `0x00000000_C0000000`.

### Impact on SHA256d

SHA256 uses only logical shifts (`ShrL`) and rotations (`Ror`/`Rol`), not
arithmetic shifts (`ShrA`). So this bug does **not** affect SHA256d. However,
it would break any program using signed right shifts of negative numbers.

### Proposed Fix

Before `DSRAV`, sign-extend the 32-bit value using `SLL` (32-bit shift left,
which sign-extends the result) instead of zero-extending:

```rust
BinOpKind::ShrA => {
    // Sign-extend T0 from 32 to 64 bits: SLL places bits[31:0] into
    // bits[31:0] and sign-extends, then DSRAV does the arithmetic shift.
    code.extend_from_slice(&Instruction::Sll { rd: Gpr::T0, rt: Gpr::T0, sa: 0 }.encode());
    code.extend_from_slice(&Instruction::Dsrav { rd: Gpr::T0, rt: Gpr::T0, rs: Gpr::T1 }.encode());
    code.extend_from_slice(&Instruction::Dsll { rd: Gpr::T0, rt: Gpr::T0, sa: 32 }.encode());
    code.extend_from_slice(&Instruction::Dsrl { rd: Gpr::T0, rt: Gpr::T0, sa: 32 }.encode());
}
```

Or alternatively, skip the zero-extension of T0 specifically for the ShrA case
and instead sign-extend.

---

## Bug 3 (LOW): Function Arguments 5+ Silently Dropped

**File:** `src/codegen/src/mips64/mod.rs`
**Lines:** 2757–2764 (Call handler) and 2336–2345 (prologue)

### Problem

The Call handler only loads arguments into `$a0–$a3` (first 4 args):

```rust
// Lines 2759-2764
let arg_regs = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3];
for (i, arg) in args.iter().enumerate() {
    if i < 4 {
        code.extend(ss_load_value(arg, &vreg_stack_slots, arg_regs[i]));
    }
    // args[4..] are silently ignored!
}
```

On MIPS64 N64 ABI, arguments 5+ must be passed on the stack at offsets
0, 8, 16, ... from the caller's stack pointer (before the call frame).

Similarly, the prologue only receives parameters from `$a0–$a3`:

```rust
// Lines 2337-2345
for (i, param) in func.params.iter().enumerate() {
    if let Some(id) = param.as_register() {
        if i < 4 {
            let offset = vreg_stack_slots.get(&id).copied().unwrap_or(0);
            code.extend(ss_sd(arg_regs[i], offset));
        }
        // params[4..] are never loaded from the stack!
    }
}
```

### Impact on SHA256d

None of the SHA256d functions have more than 4 parameters
(`rotr32(x, n)` = 2, `ch(x, y, z)` = 3, `write_u32(buf, off, val)` = 3,
etc.), so this bug does not affect SHA256d. However, any VUMA program calling
a function with 5+ arguments would get wrong results.

### Proposed Fix

In the Call handler, for arguments 5+, push them onto the stack before the
`JAL`:

```rust
for (i, arg) in args.iter().enumerate() {
    if i < 4 {
        code.extend(ss_load_value(arg, &vreg_stack_slots, arg_regs[i]));
    } else {
        // Pass arg on the stack at offset (i - 4) * 8 from $sp
        code.extend(ss_load_value(arg, &vreg_stack_slots, Gpr::T0));
        let stack_off = ((i - 4) * 8) as i32;
        code.extend_from_slice(&Instruction::Sd { rt: Gpr::T0, base: Gpr::Sp, offset: stack_off }.encode());
        code.extend_from_slice(&encode_nop());
    }
}
```

In the prologue, load parameters 5+ from the caller's stack frame
(at `$fp + 16 + (i-4)*8` for N64 ABI, where 16 accounts for the caller's
$ra and $fp save area, though exact offset depends on the call chain).

---

## Areas Verified as Correct

### ss_load_imm (lines 2186–2224)

The 64-bit immediate loading is correct for all ranges:
- Small signed (-32768..32767): `DADDIU $dst, $zero, imm` (sign-extends)
- Small unsigned (0..0xFFFF): `ORI $dst, $zero, imm` (zero-extends)
- 32-bit (0..0xFFFFFFFF): `LUI + ORI + DSLL32/DSRL32` if sign bit set
- Full 64-bit: `LUI + ORI + DSLL16 + ORI + DSLL16 + ORI` (correct)

Verified that `0xFFFFFFFF` (4294967295) loads correctly as
`0x00000000_FFFFFFFF` after the DSLL32+DSRL32 mask.

### ss_load_value for Address (line 2315)

`IRValue::Address(a) => ss_load_imm(scratch, *a as i64)` is correct.
The Address value is a u64 that gets loaded as a full 64-bit immediate.
Stack addresses fit within 64 bits, so no truncation occurs.

### BinOp Shl (lines 2426–2431)

`DSLLV` followed by `DSLL 32 + DSRL 32` correctly implements 32-bit left
shift. The zero-extended input ensures the lower 32 bits of the DSLLV result
are correct, and the mask discards any upper-32-bit spillover.

Verified: `0x80000000 << 1` → `0x00000001_00000000` → mask → `0x00000000`.
Correct for 32-bit overflow semantics.

### BinOp ShrL (lines 2432–2437)

`DSRLV` on a zero-extended value correctly implements 32-bit logical right
shift. The result is automatically zero-extended (no upper-32-bit garbage).

### BinOp Ror/Rol (lines 2439–2468)

Both rotation implementations are correct. For ROR: `(n >> r) | (n << (32-r))`
with DSRLV + DSLLV + OR + mask. The key insight is that for the DSLLV part,
the bits that wrap around in 32-bit end up in the lower 32 bits of the 64-bit
result (because the input was zero-extended), so the mask preserves them
correctly.

Verified: ROR of `0x12345678` by 4 = `0x81234567`. MIPS64 computation:
- DSRLV: `0x01234567`
- DSLLV by 28: `0x1234567_80000000` (lower 32 bits = `0x80000000`)
- OR: `0x1234567_81234567`
- Mask: `0x81234567` ✓

### Load/Store Handlers (lines 2601–2648)

Load uses `LBU` for U8 (zero-extends correctly), `LWU` for U32 (zero-extends
correctly), `LD` for 64-bit. Store uses `SB` for U8, `SW` for U32, `SD` for
64-bit. All correct.

### Offset Instruction (lines 2702–2711)

`DADDU` of base + offset without zero-extension preserves 64-bit pointers.
Correct for pointer arithmetic.

### Function Return Value (lines 2776–2783)

Return value in `$v0` is masked to 32 bits with `DSLL 32 + DSRL 32` before
storing. Correct for u32 return values.

### Call Parameter Passing for ≤4 Args (lines 2336–2345)

Parameters from `$a0–$a3` are stored with `SD` in the prologue, preserving
full 64-bit values. Correct.

### Branch Fixup (lines 2792–2801)

Branch offset patching uses `(target - source) / 4 - 1` (accounting for delay
slot). Correct for MIPS branch semantics.

### Prologue/Epilogue (lines 2323–2334, 2727–2731)

Frame setup and teardown are correct: `$fp = $sp + frame_size`, saved `$ra`
at `$fp - 8`, saved `$fp` at `$fp - 16`.

---

## Root Cause Summary

| Test | Expected | MIPS64 | Root Cause |
|------|----------|--------|------------|
| test_sha_manual | 205 | 110 | Bug 1: Alloc pointer off by `size` bytes → wrong memory addresses → data corruption |
| SHA256d | 79 | 181 | Bug 1: Same Alloc offset error corrupts all buffer I/O |
| test_u32_arith | 79 | 79 | Passes — no Alloc instructions used |

---

## Proposed Fix Priority

1. **Fix Bug 1** (Alloc offset) — swap lines 2174 and 2175 to store
   `alloc_offsets` after incrementing `current_offset`. This is the single
   line change that will fix SHA256d.
2. **Fix Bug 2** (ShrA) — sign-extend instead of zero-extend before `DSRAV`,
   then mask result to 32 bits. Does not affect SHA256d but needed for
   correctness with signed right shifts.
3. **Fix Bug 3** (args 5+) — implement N64 ABI stack argument passing.
   Does not affect SHA256d but needed for functions with 5+ parameters.
