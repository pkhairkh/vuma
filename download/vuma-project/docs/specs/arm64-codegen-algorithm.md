# ARM64 Code Generation Algorithm: SCG to Machine Code

**VUMA Project — Specification Document**
**Target: Raspberry Pi 5 (Broadcom BCM2712, Cortex-A76 quad-core)**
**Task ID: W1-28**

---

## 1. SCG Node to ARM64 Instruction Mapping

The Semantic Computation Graph (SCG) is the intermediate representation produced by VUMA's front-end compiler passes. Each SCG node represents a discrete semantic operation — allocation, deallocation, memory access, type casting, computation, or control flow — that must be translated into one or more ARM64 (AArch64) machine instructions. This section defines the complete mapping from every SCG node type to its ARM64 instruction sequence, targeting the Cortex-A76 microarchitecture found in the Raspberry Pi 5's BCM2712 SoC.

### AllocationNode

The AllocationNode requests a block of memory of a given size and alignment. The code generator must decide between three allocation strategies based on the node's `alloc_type` field and the requested size:

**Stack allocation (small, ≤4096 bytes):** The compiler emits a direct stack pointer adjustment. This is the fastest path since it requires only a single arithmetic instruction and no function call overhead. The stack pointer `sp` is decremented by the allocation size, rounded up to 16-byte alignment per AAPCS64 requirements:

```asm
; AllocationNode { size: 64, alloc_type: Stack }
sub sp, sp, #64          ; allocate 64 bytes on stack
; result register holds the address of the allocation
mov x0, sp               ; x0 = pointer to allocated block
```

For sizes that are not multiples of 16, the compiler must round up to maintain stack alignment. For example, a 100-byte allocation becomes 112 bytes:

```asm
; AllocationNode { size: 100, alloc_type: Stack }
sub sp, sp, #112         ; round up 100 → 112 (16-byte aligned)
mov x0, sp
```

**Heap allocation (large, >4096 bytes):** When the requested size exceeds the stack allocation threshold, the code generator emits a call to the C library's `malloc` function. The size argument is placed in `x0` before the branch-with-link instruction. After the call, `x0` contains either a valid pointer or NULL on failure. The compiler must insert a NULL check if the VUMA safety profile demands it:

```asm
; AllocationNode { size: 8192, alloc_type: Heap }
mov x0, #8192            ; size argument for malloc
bl malloc                 ; call malloc(8192)
cbz x0, .alloc_fail      ; safety check: branch if NULL
; x0 now holds the heap pointer
```

**Arena allocation:** Arena-allocated objects are bump-allocated from a pre-reserved region. The arena base pointer lives in a callee-saved register (e.g., `x19`) and the current arena offset in another (e.g., `x20`). The allocation is a simple addition, and the offset is advanced. This is nearly as fast as stack allocation but avoids the lifetime constraints of the stack:

```asm
; AllocationNode { size: 256, alloc_type: Arena }
add x0, x19, x20         ; x0 = arena_base + arena_offset
add x20, x20, #256       ; advance arena offset by 256
```

### DeallocationNode

Deallocation reverses the allocation. The strategy is determined by the `alloc_type` field:

**Stack deallocation:** Restore the stack pointer by adding back the size:

```asm
; DeallocationNode { size: 64, alloc_type: Stack }
add sp, sp, #64          ; deallocate 64 bytes from stack
```

**Heap deallocation:** Call `free` with the pointer in `x0`:

```asm
; DeallocationNode { alloc_type: Heap }
; x0 already holds the pointer to free
bl free
```

**Arena deallocation:** This is a no-op at the individual deallocation level. Arena memory is released in bulk by `arena_destroy`, so no instructions are emitted:

```asm
; DeallocationNode { alloc_type: Arena }
; no code emitted — arena_destroy handles bulk deallocation
```

### AccessNode (Read)

Read access nodes load data from memory into a register. Two addressing modes are supported depending on whether the offset is known at compile time (fixed) or computed at runtime (indexed):

**Fixed-offset read:**

```asm
; AccessNode { mode: Read, offset: 16 }
ldr x0, [x_base, #16]   ; load 64-bit value at base+16
ldr w1, [x_base, #24]   ; load 32-bit value at base+24
ldrb w2, [x_base, #28]  ; load byte at base+28
```

**Indexed read (for array access):**

```asm
; AccessNode { mode: Read, indexed: true, element_size: 8 }
ldr x0, [x_base, x_index, lsl #3]   ; load at base + index*8
; For 4-byte elements:
ldr w1, [x_base, x_index, lsl #2]   ; load at base + index*4
```

### AccessNode (Write)

Write access nodes store data from a register into memory. The same two addressing modes apply:

**Fixed-offset write:**

```asm
; AccessNode { mode: Write, offset: 16 }
str x0, [x_base, #16]   ; store 64-bit value at base+16
str w1, [x_base, #24]   ; store 32-bit value at base+24
strb w2, [x_base, #28]  ; store byte at base+28
```

**Indexed write:**

```asm
; AccessNode { mode: Write, indexed: true, element_size: 8 }
str x0, [x_base, x_index, lsl #3]   ; store at base + index*8
```

### CastNode

Type casts between compatible representations (same bit width) are no-ops at the machine level — the bits in the register are simply reinterpreted. Cross-domain casts (integer ↔ floating-point) require data movement between the integer and floating-point register banks using `fmov`:

```asm
; CastNode { from: i64, to: f64 }  — int to float reinterpretation
fmov d0, x0              ; move integer bits from x0 to d0 as float

; CastNode { from: f64, to: i64 }  — float to int reinterpretation
fmov x0, d0              ; move float bits from d0 to x0 as integer

; CastNode { from: i32, to: i64 }  — sign extension
sxtw x0, w0              ; sign-extend word to doubleword

; CastNode { from: u32, to: u64 }  — zero extension
mov w0, w0                ; implicit zero-extension to 64 bits
```

For actual numeric conversions (not bit reinterpretation), such as integer-to-float conversion, the code generator uses `scvtf` (signed convert to float) and `fcvtzs` (float convert to signed integer):

```asm
; CastNode { from: i64, to: f64, convert: true }
scvtf d0, x0              ; convert signed int to double-precision float

; CastNode { from: f64, to: i64, convert: true }
fcvtzs x0, d0             ; convert double-precision float to signed int
```

### ComputationNode

Arithmetic and logical operations map directly to ARM64 instructions. The code generator selects the appropriate instruction based on the operator and operand sizes:

```asm
; ComputationNode { op: Add, size: 64 }
add x0, x1, x2           ; x0 = x1 + x2

; ComputationNode { op: Sub, size: 64 }
sub x0, x1, x2           ; x0 = x1 - x2

; ComputationNode { op: Mul, size: 64 }
mul x0, x1, x2           ; x0 = x1 * x2

; ComputationNode { op: SDiv, size: 64 }
sdiv x0, x1, x2          ; x0 = x1 / x2 (signed)

; ComputationNode { op: UDiv, size: 64 }
udiv x0, x1, x2          ; x0 = x1 / x2 (unsigned)

; ComputationNode { op: And, size: 64 }
and x0, x1, x2           ; x0 = x1 & x2

; ComputationNode { op: Orr, size: 64 }
orr x0, x1, x2           ; x0 = x1 | x2

; ComputationNode { op: Eor, size: 64 }
eor x0, x1, x2           ; x0 = x1 ^ x2

; ComputationNode { op: Lsl, size: 64 }
lsl x0, x1, x2           ; x0 = x1 << x2

; ComputationNode { op: Lsr, size: 64 }
lsr x0, x1, x2           ; x0 = x1 >> x2 (logical)

; ComputationNode { op: Asr, size: 64 }
asr x0, x1, x2           ; x0 = x1 >> x2 (arithmetic)
```

For 32-bit operations, the compiler uses the `w` register variants (which implicitly zero-extend to 64 bits on write):

```asm
add w0, w1, w2            ; 32-bit addition
mul w0, w1, w2            ; 32-bit multiplication
```

Immediate operands use the dedicated immediate forms where the constant fits within the ARM64 encoding constraints:

```asm
add x0, x1, #42           ; add immediate
sub x0, x1, #100          ; subtract immediate
and x0, x1, #0xFF         ; bitwise AND with immediate mask
```

### ControlNode (Branch)

Branch nodes implement conditional and unconditional control flow. The code generator selects the most efficient branch instruction based on the condition:

**Simple zero/non-zero test:**

```asm
; ControlNode { type: Branch, condition: Zero }
cbz x0, .label            ; branch to .label if x0 == 0

; ControlNode { type: Branch, condition: NonZero }
cbnz x0, .label           ; branch to .label if x0 != 0
```

**Comparison-based branch:** For general comparisons, the compiler emits a `cmp` instruction followed by a conditional branch:

```asm
; ControlNode { type: Branch, condition: Equal }
cmp x0, x1
b.eq .label               ; branch if x0 == x1

; ControlNode { type: Branch, condition: NotEqual }
cmp x0, x1
b.ne .label               ; branch if x0 != x1

; ControlNode { type: Branch, condition: LessThan }
cmp x0, x1
b.lt .label               ; branch if x0 < x1 (signed)

; ControlNode { type: Branch, condition: GreaterEqual }
cmp x0, x1
b.ge .label               ; branch if x0 >= x1 (signed)

; Unsigned comparison variants:
cmp x0, x1
b.lo .label               ; branch if x0 < x1 (unsigned, lower)
b.hs .label               ; branch if x0 >= x1 (unsigned, higher or same)
```

### ControlNode (Call)

Function calls use the `bl` (branch with link) instruction, which stores the return address in `x30` (the link register):

```asm
; ControlNode { type: Call, target: "compute_hash" }
bl compute_hash            ; call function, return address in x30
; x0 holds the return value after the call returns
```

### ControlNode (Return)

Return nodes place the return value in `x0` and execute the `ret` instruction, which branches to the address in `x30`:

```asm
; ControlNode { type: Return }
mov x0, x_result          ; move result into return register
ret                        ; return to caller (branch to x30)
```

If the function returns void, only `ret` is emitted:

```asm
; ControlNode { type: Return, void: true }
ret
```

---

## 2. Function Calling Convention (AAPCS64)

The ARM64 procedure call standard (AAPCS64) defines the contract between callers and callees that VUMA's code generator must honor on every function boundary. The Raspberry Pi 5 runs Linux in AArch64 mode, and its system libraries, the C runtime, and the kernel all adhere to AAPCS64. Any deviation would result in undefined behavior, silent data corruption, or crashes. This section details the full calling convention as implemented by the VUMA code generator, including VUMA-specific extensions for passing Bounds Descriptor (BD) metadata used by the Inline Validation Engine (IVE).

### Argument Passing

The first eight integer or pointer arguments are passed in registers `x0` through `x7`. This covers the vast majority of function calls in VUMA programs, as most functions take fewer than eight parameters. Arguments beyond the eighth are passed on the stack at ascending addresses from the stack pointer. The caller is responsible for reserving stack space for these arguments before the call:

```asm
; Function call: foo(a, b, c, d, e, f, g, h, i, j)
; Arguments 1-8 in x0-x7, arguments 9-10 on stack
mov x0, x_a               ; arg 1
mov x1, x_b               ; arg 2
mov x2, x_c               ; arg 3
mov x3, x_d               ; arg 4
mov x4, x_e               ; arg 5
mov x5, x_f               ; arg 6
mov x6, x_g               ; arg 7
mov x7, x_h               ; arg 8
str x_i, [sp, #0]         ; arg 9 on stack
str x_j, [sp, #8]         ; arg 10 on stack
bl foo
```

Floating-point arguments are passed in `v0` through `v7` (or `d0`-`d7` for double precision, `s0`-`s7` for single precision). If a function takes a mix of integer and floating-point arguments, each argument is assigned to the next register in its respective class:

```asm
; Function call: mix(int_val, float_val, ptr_val, double_val)
mov x0, x_int             ; integer arg 1 → x0
fmov d0, d_float          ; float arg 2 → d0
mov x1, x_ptr             ; pointer arg 3 → x1
fmov d1, d_double         ; double arg 4 → d1
bl mix
```

### Return Values

The primary return value is placed in `x0` for integer/pointer types and `v0` (or `d0`/`s0`) for floating-point types. For 128-bit returns, the value is split across `x0` and `x1`. For composite returns larger than 16 bytes, the caller passes a pointer to a caller-allocated buffer in `x8`, and the callee writes the result to that buffer:

```asm
; Caller: passing buffer for large return
add x8, sp, #0            ; x8 = pointer to return buffer on stack
bl large_return_func
; result is now at [sp, #0]
```

### Callee-Saved Registers

Registers `x19` through `x28`, `x29` (frame pointer), and `x30` (link register) are callee-saved. A function that modifies any of these must preserve their original values by saving them to the stack on entry and restoring them before return. The Cortex-A76's store-pair instructions (`stp`) efficiently save two 64-bit registers in a single instruction:

```asm
; Function prologue: save callee-saved registers
stp x29, x30, [sp, #-16]!    ; push fp and lr, decrement sp by 16
mov x29, sp                    ; set up frame pointer

; If the function also uses x19-x23:
stp x19, x20, [sp, #-16]!    ; push x19, x20
stp x21, x22, [sp, #-16]!    ; push x21, x22
str x23, [sp, #-8]!           ; push odd register (pad for alignment)

; Function epilogue: restore and return
ldp x21, x22, [sp], #16      ; pop x21, x22
ldp x19, x20, [sp], #16      ; pop x19, x20
ldp x29, x30, [sp], #16      ; pop fp and lr, increment sp by 16
ret
```

### Caller-Saved Registers

Registers `x0` through `x18` are caller-saved (with `x16`-`x17` reserved for the PLT and `x18` for the platform register on some OSes). If a value in any of these registers must survive a function call, the caller must spill it to the stack or move it to a callee-saved register before the call:

```asm
; Save x0 across a function call
str x0, [sp, #-16]!       ; spill x0 to stack
bl some_function           ; x0 is clobbered by return value
ldr x1, [sp], #16         ; restore original x0 into x1
```

### Stack Alignment

The stack must be 16-byte aligned at all public function boundaries (at the point of a `bl` instruction). The VUMA code generator must track the current stack depth and insert padding when necessary. Since each `stp` pair push or `str` with pre-indexed addressing decrements by a multiple of 8, the compiler must ensure the total decrement is always a multiple of 16. If an odd number of 8-byte registers must be saved, the compiler pads the save area by 8 bytes.

### VUMA-Specific: BD Metadata in x8-x15

When the Inline Validation Engine (IVE) requires runtime bounds checks, VUMA passes Bounds Descriptor (BD) metadata in registers `x8` through `x15`. This avoids memory loads for bounds information during hot-path checks. The BD metadata encodes the base address, byte length, and tag for each pointer argument that requires runtime validation. Since `x8` is also used for indirect return buffers, the code generator must handle the conflict: when both BD metadata and an indirect return are needed, the BD data takes priority in `x8`-`x15`, and the return buffer pointer is passed as an additional explicit argument on the stack:

```asm
; Function call with BD metadata for IVE
mov x0, x_ptr             ; the pointer to validate
mov x8, x_bd_base         ; BD: base address of allocation
mov x9, x_bd_len          ; BD: byte length of allocation
mov x10, x_bd_tag         ; BD: tag for type validation
bl ivec_checked_access    ; call with bounds metadata
```

The callee is expected to check the BD before performing any pointer dereference, inserting inline guard instructions that trap on violation:

```asm
; IVE guard in callee
cmp x0, x8                ; ptr < base?
b.lo .bounds_violation
add x11, x8, x9           ; x11 = base + length
cmp x0, x11               ; ptr >= base + length?
b.hs .bounds_violation
; access is safe, proceed
```

---

## 3. Register Allocation Strategy

Register allocation is the process of mapping the unlimited virtual registers in the SCG intermediate representation to the finite set of physical registers available on the Cortex-A76. ARM64 provides 31 general-purpose 64-bit registers (`x0`-`x30`) and 32 SIMD/floating-point registers (`v0`-`v31`). The VUMA code generator employs a linear-scan register allocator with priority-based heuristics tuned for the Cortex-A76's pipeline characteristics. This section describes the complete register partitioning scheme, allocation algorithm, and spilling strategy.

### Register Partitioning by Role

The 31 general-purpose registers are partitioned into functional groups, each with a designated purpose. This partitioning reduces register pressure conflicts and simplifies the allocator's decisions:

**x0–x7: Argument/Return Registers.** These eight registers serve as the primary channel for passing arguments and returning values at function boundaries, as dictated by AAPCS64. Within a function body, they can also be used as temporary scratch registers for values that do not need to survive function calls. The allocator prefers these for short-lived expression temporaries. For example, a chain of additions can use x0-x3 without any spills:

```asm
add x0, x1, x2            ; temp in x0
add x3, x0, x4            ; temp in x3
sub x5, x3, x6            ; temp in x5
```

**x8–x15: Temporary and BD Metadata Registers.** These registers are used for intermediate computation results within basic blocks and for VUMA-specific BD metadata when the IVE is active. They are caller-saved, so any value that must survive a call must be moved to a callee-saved register or spilled. Within a single basic block (no intervening calls), the allocator uses x8-x15 as an extended scratch pool:

```asm
; Extended temporaries using x8-x15
ldr x8, [x_base, #0]      ; load first operand
ldr x9, [x_base, #8]      ; load second operand
mul x10, x8, x9           ; intermediate product
add x11, x10, x12         ; accumulate
```

**x16–x17: PLT/Intra-Section Call Registers.** These two registers are reserved by the linker and dynamic loader for procedure linkage table (PLT) stubs. The VUMA code generator never allocates these for user variables. They may be clobbered by any function call through a PLT entry, making them unsafe for values that must survive across calls. When generating position-independent code (PIC) for shared libraries, `adrp` instructions also target x16/x17 as scratch:

```asm
; PLT call pattern (generated by linker)
adrp x16, :got:function_name
ldr  x17, [x16, :got_lo12:function_name]
add  x16, x16, :got_lo12:function_name
br   x17
```

**x18: Platform Register.** On some operating systems, x18 is reserved as a platform register (e.g., for the shadow call stack on Android). The VUMA code generator treats x18 as reserved and does not allocate it for general use. On bare-metal Pi 5 targets, x18 may be repurposed as a per-CPU data pointer if the VUMA runtime requires it:

```asm
; Optional: x18 as per-CPU data pointer (bare metal only)
mrs x18, tpidr_el0         ; read thread pointer from system register
; or on bare metal:
ldr x18, =per_cpu_data     ; load per-CPU data base address
```

**x19–x28: Callee-Saved Variable Registers.** These ten registers are the primary allocation target for variables that are live across function calls. Since they are callee-saved by AAPCS64, their values persist through any call. The allocator assigns these to loop variables, accumulator registers, and any value whose live range spans one or more call sites. The allocator prioritizes x19-x28 for variables with the longest live ranges:

```asm
; x19 used as loop counter surviving function calls
mov x19, #0               ; initialize counter
.loop:
  bl process_item          ; call preserves x19
  add x19, x19, #1        ; increment
  cmp x19, #100
  b.lt .loop
```

**x29: Frame Pointer (FP).** Register x29 serves as the frame pointer, providing a stable reference point for accessing local variables and parameters on the stack. The VUMA code generator always sets up a frame pointer in non-leaf functions (those that make calls), as it simplifies stack unwinding for debuggers and exception handling. In leaf functions with small stack frames, the compiler may omit the frame pointer and use `sp`-relative addressing exclusively:

```asm
; Function prologue with frame pointer
stp x29, x30, [sp, #-32]!   ; save fp, lr; allocate 32 bytes
mov x29, sp                   ; establish frame pointer
; Access local at [x29, #16]
str x0, [x29, #16]           ; save argument to local slot
```

**x30: Link Register (LR).** The link register holds the return address after a `bl` instruction. It must be saved to the stack before any subsequent call (which would overwrite it) and restored before `ret`. In non-leaf functions, x30 is typically saved alongside x29 in the prologue's `stp` instruction.

**x31 (sp/xzr):** Register 31 encodes as either the stack pointer (when used in load/store addressing) or the zero register (when used in arithmetic). The zero register always reads as 0 and discards writes, which is useful for zeroing, comparing to zero, and discarding unwanted instruction results:

```asm
mov x0, xzr               ; set x0 to zero (more efficient than mov x0, #0)
cmp x1, xzr               ; compare x1 to zero
```

### Spilling Strategy

When all physical registers are in use and a new allocation is required, the linear-scan allocator selects the register whose current live range ends farthest in the future and spills it. Spilling uses pre-indexed store instructions to push the register onto the stack, and post-indexed loads to restore it:

```asm
; Spill x19 to stack
str x19, [sp, #-16]!      ; pre-indexed: store x19, decrement sp by 16

; ... other code ...

; Reload x19 from stack
ldr x19, [sp], #16        ; post-indexed: load x19, increment sp by 16
```

For functions with many spills, the compiler allocates a contiguous spill area in the prologue and accesses spills via fixed offsets from the frame pointer, which is more efficient than repeated stack pointer adjustments:

```asm
; Prologue: allocate spill area
stp x29, x30, [sp, #-48]!
mov x29, sp
; Spill slot 0 at [x29, #16]
; Spill slot 1 at [x29, #24]
; Spill slot 2 at [x29, #32]
; Spill slot 3 at [x29, #40]

; Spill x19 to slot 0
str x19, [x29, #16]

; Reload x19 from slot 0
ldr x19, [x29, #16]
```

The Cortex-A76 has a 4-wide out-of-order execution engine with a 128-entry reorder buffer. The allocator's spilling heuristic favors spilling registers whose next use is far away in the instruction stream, as the out-of-order engine can often hide the load latency of a future reload. For the Cortex-A76, L1 data cache load latency is 4 cycles, and a well-timed spill/reload can often be completely hidden by the out-of-order scheduler if the reload is issued at least 8-10 instructions before the value is actually needed.

---

## 4. Memory Barrier Insertion

The ARM64 memory model is weakly ordered, meaning that the processor may reorder memory operations in ways that are not visible to a single thread but can cause observable inconsistencies in multi-threaded programs. The Cortex-A76 in the Raspberry Pi 5 implements the ARMv8.2-A architecture, which permits store-load reordering and store-store reordering between different addresses. This means that without explicit barriers, a store followed by a load to a different address may appear to execute in the opposite order to another core. The VUMA compiler's SyncEdge annotations in the SCG provide the information needed to insert the correct barriers and atomic operations. This section defines the barrier insertion algorithm for each SyncEdge variant.

### SyncEdge with HappensBefore

When the SCG contains a SyncEdge annotated with `HappensBefore`, the compiler must ensure that all memory operations before the synchronization point are globally visible before any memory operation after the synchronization point begins. This is the classic "release-acquire" pattern. The implementation uses a `dmb ish` (Data Memory Barrier, Inner Shareable) instruction, which is a full barrier that prevents reordering of any memory operations across it:

```asm
; Producer thread: write data, then signal
str x0, [x_data]          ; write data
dmb ish                    ; ensure data write is visible before signal
str x1, [x_flag]          ; write signal flag

; Consumer thread: check signal, then read data
ldr x1, [x_flag]          ; read signal flag
dmb ish                    ; ensure flag read completes before data read
ldr x0, [x_data]          ; read data (guaranteed to see producer's write)
```

The `dmb ish` barrier is heavyweight — it stalls the pipeline until all outstanding memory operations have completed. On the Cortex-A76, this can cost 20-30 cycles. The VUMA compiler therefore minimizes `dmb ish` insertions, placing them only where the SyncEdge analysis proves they are necessary. Within a single thread or within a critical section protected by a mutex, no `dmb ish` is needed because the mutex lock/unlock already provides the necessary ordering.

### SyncEdge with AtomicAcquireRelease

For fine-grained synchronization where a full barrier is unnecessary, the compiler uses ARM64's built-in acquire-release semantics. These instructions provide ordering guarantees at the individual memory operation level without the overhead of a full `dmb`:

**Store-release (`stlr`):** Ensures that all preceding memory operations (both loads and stores) are globally visible before the store-release becomes visible. The `stlr` instruction replaces a normal `str` at the synchronization point:

```asm
; Producer: write data, then release-store the flag
str x0, [x_data]          ; ordinary store — no ordering guarantee
stlr x1, [x_flag]         ; store-release — all prior stores visible before this
```

**Load-acquire (`ldar`):** Ensures that all subsequent memory operations (both loads and stores) are not observed before the load-acquire completes. The `ldar` instruction replaces a normal `ldr` at the synchronization point:

```asm
; Consumer: acquire-load the flag, then read data
ldar x1, [x_flag]         ; load-acquire — all subsequent ops after this
ldr x0, [x_data]          ; guaranteed to see the producer's write
```

**Exclusive access for compare-and-swap:** For atomic read-modify-write operations (CAS, fetch-and-add), the compiler emits `ldaxr`/`stlxr` loops with a retry on failure:

```asm
; Atomic compare-and-swap at [x_addr]
; Expected value in x1, desired value in x2
.retry:
  ldaxr x0, [x_addr]       ; load-acquire exclusive
  cmp x0, x1               ; compare with expected
  b.ne .fail               ; not equal, abort
  stlxr w3, x2, [x_addr]  ; store-release exclusive
  cbnz w3, .retry          ; retry if store failed (another writer intervened)
  ; success: x0 = old value
.fail:
  ; failure: x0 = current value (different from expected)
```

The exclusive store (`stlxr`) writes its success/failure status to the `w3` register — 0 for success, 1 for failure. The compiler must check this status and retry the loop if the store failed. On the Cortex-A76, the exclusive monitor tracks a cache line granule, so any external write to the same cache line between the `ldaxr` and `stlxr` causes the exclusive store to fail.

### SyncEdge with MutexLocked

When the SCG indicates that a critical section is protected by a mutex, the compiler inserts calls to the VUMA runtime's `lock_acquire` and `lock_release` functions. These functions internally implement the correct acquire-release semantics using `ldaxr`/`stlxr` with an exponential backoff loop for contention. The compiler wraps the critical section between the lock and unlock calls:

```asm
; Mutex-protected critical section
mov x0, x_mutex_ptr       ; argument: pointer to mutex
bl lock_acquire            ; acquire lock (includes dmb ish internally)

; Critical section: any memory operations here are safe
ldr x1, [x_shared_data]
add x1, x1, #1
str x1, [x_shared_data]

mov x0, x_mutex_ptr       ; argument: pointer to mutex
bl lock_release            ; release lock (includes dmb ish internally)
```

The `lock_acquire` and `lock_release` functions are implemented in the VUMA runtime library and handle the full synchronization protocol, including the memory barriers. The compiler does not need to insert additional barriers around the critical section because the lock functions already provide happens-before guarantees.

### Barrier Insertion Algorithm

The code generator processes SyncEdge annotations in a separate pass after initial instruction selection. The algorithm works as follows:

1. For each basic block, collect all SyncEdge annotations from the SCG.
2. For `HappensBefore` edges: insert `dmb ish` after the last store before the edge and before the first load after the edge.
3. For `AtomicAcquireRelease` edges: replace the store at the release point with `stlr`, and replace the load at the acquire point with `ldar`. For CAS patterns, emit `ldaxr`/`stlxr` loops.
4. For `MutexLocked` edges: insert `bl lock_acquire` before the critical section and `bl lock_release` after it. No additional barriers are needed.
5. Eliminate redundant barriers: if two `dmb ish` instructions appear in the same basic block with no intervening memory operations that require ordering, remove the second one.

This pass ensures that the minimum necessary set of barriers is inserted, avoiding the performance cost of over-synchronization while guaranteeing correctness on the weakly-ordered Cortex-A76.

---

## 5. ELF Object Format for Pi 5 Linux

The VUMA code generator produces ELF (Executable and Linkable Format) object files that are consumed by the system linker (`ld`) to produce the final executable or shared library. For the Raspberry Pi 5 running a 64-bit Linux kernel, the object files must conform to the AArch64 ELF specification. This section defines the ELF header fields, section layout, and relocation types that the VUMA code generator must emit.

### ELF Header

Every ELF object file begins with a 64-byte header that identifies the file's target architecture and properties. The VUMA code generator sets the following header fields:

| Field               | Value              | Rationale                                          |
|---------------------|--------------------|----------------------------------------------------|
| `e_ident[EI_MAG]`  | `0x7f "ELF"`       | ELF magic number                                   |
| `e_ident[EI_CLASS]`| `ELFCLASS64` (2)   | 64-bit object file for AArch64                     |
| `e_ident[EI_DATA]` | `ELFDATA2LSB` (1)  | Little-endian (ARM64 is always LE in this context) |
| `e_ident[EI_VERSION]`| `EV_CURRENT` (1)  | Current ELF version                                |
| `e_ident[EI_OSABI]`| `ELFOSABI_NONE` (0)| Generic System V ABI (Linux compatible)            |
| `e_type`           | `ET_REL` (1)       | Relocatable object file                            |
| `e_machine`        | `EM_AARCH64` (183) | ARM 64-bit architecture                            |
| `e_version`        | `EV_CURRENT` (1)   | Current ELF version                                |
| `e_entry`          | `0`                | No entry point in relocatable objects              |
| `e_phoff`          | `0`                | No program headers in relocatable objects          |
| `e_shoff`          | offset             | Section header table offset                        |
| `e_flags`          | `0`                | No AArch64-specific flags                          |
| `e_ehsize`         | `64`               | ELF header size                                    |
| `e_phentsize`      | `0`                | No program headers                                 |
| `e_shentsize`      | `64`               | Section header entry size                          |

The little-endian byte ordering (`ELFDATA2LSB`) is mandatory for AArch64 Linux. While the ARM architecture technically supports both endiannesses, all mainstream AArch64 Linux distributions (including Raspberry Pi OS) use little-endian mode exclusively.

### Standard Sections

The VUMA code generator emits the following standard sections in every object file:

**`.text` (SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR):** Contains the machine code for all functions. The code generator places each function at a 16-byte aligned offset within this section, as the Cortex-A76's branch predictor performs optimally with aligned branch targets. Functions that are hot paths or interrupt handlers are given 64-byte alignment to match the Cortex-A76's cache line size:

```asm
.section .text
.balign 16
.global vuma_main
vuma_main:
  stp x29, x30, [sp, #-16]!
  mov x29, sp
  ; ... function body ...
  ldp x29, x30, [sp], #16
  ret
```

**`.data` (SHT_PROGBITS, SHF_ALLOC | SHF_WRITE):** Contains initialized global and static variables. Each variable is aligned to its natural alignment (8 bytes for pointers and 64-bit integers, 4 bytes for 32-bit integers):

```asm
.section .data
.balign 8
.global vuma_heap_base
vuma_heap_base:
  .xword 0x00000000        ; initialized to zero, set at runtime
```

**`.bss` (SHT_NOBITS, SHF_ALLOC | SHF_WRITE):** Contains uninitialized global and static variables. The `.bss` section occupies no space in the object file; the loader allocates and zero-fills it at program start:

```asm
.section .bss
.balign 16
.global vuma_arena
vuma_arena:
  .skip 4096               ; reserve 4096 bytes, zero-filled at load
```

**`.rodata` (SHT_PROGBITS, SHF_ALLOC):** Contains read-only data such as string literals, constant arrays, and jump tables. Placing constants in `.rodata` allows the kernel to map this section as read-only, enabling early detection of accidental writes:

```asm
.section .rodata
.balign 8
vuma_error_msg:
  .asciz "Bounds violation detected"
vuma_jump_table:
  .xword .case_0
  .xword .case_1
  .xword .case_2
```

**`.symtab` (SHT_SYMTAB):** The symbol table maps function names, global variable names, and external references to their addresses within the object file. Each entry includes the symbol name (as an index into `.strtab`), the section index, the value (offset), and the size. The VUMA code generator emits symbol entries for every function and every globally visible variable:

```asm
; Symbol table entry for vuma_main
; st_name: index into .strtab for "vuma_main"
; st_info: STT_FUNC | STB_GLOBAL
; st_shndx: .text section index
; st_value: offset of vuma_main within .text
; st_size: size of vuma_main in bytes
```

**`.strtab` (SHT_STRTAB):** The string table holds null-terminated symbol names referenced by `.symtab` entries. The first byte is always `\0` (the null string at index 0).

### Relocation Types

Relocation entries tell the linker how to patch code and data addresses when the final executable is assembled from multiple object files. The VUMA code generator emits the following AArch64 relocation types:

**`R_AARCH64_CALL26` (283):** Used for `bl` (branch with link) instructions. The `bl` instruction encodes a 26-bit signed offset (±128 MB range) relative to the instruction's address. If the target function is more than 128 MB away, the linker must insert a veneer (a small thunk that loads a 64-bit address and branches to it):

```asm
bl external_function       ; R_AARCH64_CALL26 relocation at this instruction
; Linker patches: bits [25:0] = (target - PC) >> 2
```

**`R_AARCH64_ADR_PREL_PG_HI21` (275):** Used for `adrp` instructions that compute the page address (4 KB aligned) of a symbol relative to the current PC. The `adrp` instruction encodes a 21-bit signed offset representing the number of 4 KB pages. This is always paired with a subsequent `R_AARCH64_LDST64_LO12` or `R_AARCH64_ADD_ABS_LO12_NC` relocation to add the page offset:

```asm
adrp x0, :got:vuma_data   ; R_AARCH64_ADR_PREL_PG_HI21
ldr  x0, [x0, :got_lo12:vuma_data]  ; R_AARCH64_LD64_GOT_LO12_NC
```

**`R_AARCH64_ADD_ABS_LO12_NC` (277):** Used for `add` instructions that add the low 12 bits of a symbol's absolute address. The `_NC` suffix means "no check" — the linker does not check for overflow because the low 12 bits always fit:

```asm
adrp x0, vuma_data        ; high 21 bits of page address
add  x0, x0, :lo12:vuma_data  ; R_AARCH64_ADD_ABS_LO12_NC, low 12 bits
```

**`R_AARCH64_LDST64_LO12` (286):** Used for 64-bit load/store instructions with a 12-bit immediate offset. The linker fills in the page offset of the target symbol:

```asm
adrp x0, vuma_data
ldr  x0, [x0, :lo12:vuma_data]  ; R_AARCH64_LDST64_LO12
```

The VUMA code generator's relocation pass emits relocation entries for every symbol reference that cannot be resolved within the same object file. For position-independent code (PIC, required for shared libraries), all references to global data use the GOT (Global Offset Table) pattern with `adrp` + `ldr` pairs. For position-dependent code (standard executables), the `adrp` + `add` pattern is used for direct symbol access.

---

## 6. Bare Metal Startup for Pi 5

When VUMA targets bare metal execution on the Raspberry Pi 5 (no operating system, no Linux kernel), the code generator must produce a self-contained binary that handles all hardware initialization from the moment the BCM2712 SoC releases the CPU from reset. The bare metal startup sequence sets up the execution environment — stack, BSS, MMU, and exception vectors — before transferring control to the VUMA runtime's `main` function. This section describes the complete startup protocol, linker script, and multi-core management.

### Entry Point and Boot Protocol

The Raspberry Pi 5's VideoCore GPU loads the kernel image (typically named `kernel8.img` for 64-bit mode) into memory at physical address `0x80000` and starts core 0 executing at that address. Cores 1, 2, and 3 are held in a WFE (Wait For Event) loop by the GPU firmware until explicitly released by software. The VUMA bare metal binary must begin with the `_start` symbol at this address:

```asm
.section .text.boot
.global _start
_start:
  ; ---- Core identification ----
  mrs x0, mpidr_el1         ; read Multiprocessor Affinity Register
  and x0, x0, #0xFF         ; extract core ID (0-3)
  cbz x0, .core0_start      ; core 0 proceeds; others park

  ; ---- Park cores 1-3 in WFE loop ----
.park_loop:
  wfe                        ; wait for event (low power)
  b .park_loop               ; loop indefinitely until released

.core0_start:
  ; ---- Set up stack pointer ----
  ldr x0, =_stack_top       ; load address of stack top
  mov sp, x0                 ; set stack pointer

  ; ---- Zero the .bss section ----
  ldr x0, =_bss_start       ; start of BSS
  ldr x1, =_bss_end         ; end of BSS
.zero_bss:
  cmp x0, x1
  b.ge .bss_done
  str xzr, [x0], #8         ; zero 8 bytes, advance pointer
  b .zero_bss
.bss_done:

  ; ---- Set up exception vector table ----
  ldr x0, =exception_vector_table
  msr vbar_el1, x0           ; set Vector Base Address Register

  ; ---- Jump to main ----
  bl main                    ; call VUMA main function

  ; ---- Halt if main returns ----
.halt:
  wfe
  b .halt
```

The `mpidr_el1` register's affinity field identifies which core is executing. Core 0 (affinity 0x00) proceeds with initialization, while cores 1-3 enter a low-power WFE loop. The VUMA runtime can later release secondary cores by sending an SEV (Send Event) instruction and providing each core with its own stack pointer and entry point:

```asm
; Release core 1
ldr x0, =core1_stack_top
ldr x1, =core1_entry
dsb ish                     ; data synchronization barrier
sev                         ; send event to wake WFE-waiting cores
```

### Stack Setup

The stack grows downward from a high address. The `_stack_top` symbol is defined in the linker script at the top of RAM, below the GPU reserved region. The Raspberry Pi 5 has up to 8 GB of RAM (depending on model), with the GPU firmware typically reserving the first 64-128 MB. The VUMA linker script places the stack at a safe offset from the top of usable RAM:

```asm
; Stack for core 0: 64 KB
ldr x0, =_stack_top       ; e.g., 0x80000 + code_size + data_size + 0x10000
mov sp, x0
```

For multi-core operation, each core gets its own stack, separated by at least 4 KB to avoid cache line aliasing on the Cortex-A76's L1 data cache:

```
Core 0 stack: _stack_top - 0x0000 to _stack_top - 0x4000 (16 KB)
Core 1 stack: _stack_top - 0x4000 to _stack_top - 0x8000 (16 KB)
Core 2 stack: _stack_top - 0x8000 to _stack_top - 0xC000 (16 KB)
Core 3 stack: _stack_top - 0xC000 to _stack_top - 0x10000 (16 KB)
```

### BSS Zeroing

The `.bss` section contains uninitialized data that must be zeroed before any code references it. The startup code uses a simple loop that stores double-words of zero:

```asm
ldr x0, =_bss_start
ldr x1, =_bss_end
.zero_bss:
  cmp x0, x1
  b.ge .bss_done
  str xzr, [x0], #8         ; post-indexed: store 0, add 8 to x0
  b .zero_bss
.bss_done:
```

For large BSS sections, the Cortex-A76 benefits from using `stp` to zero 16 bytes per iteration:

```asm
.zero_bss_fast:
  cmp x0, x1
  b.ge .bss_done
  stp xzr, xzr, [x0], #16  ; zero 16 bytes per iteration
  b .zero_bss_fast
```

### Exception Vector Table

The ARM64 exception vector table contains 16 entries, each 128 bytes aligned, covering synchronous and asynchronous exceptions from four exception levels (EL1, EL2, EL3, and EL0). For bare metal VUMA, the table is placed at a known address and registered with the `vbar_el1` system register:

```asm
.balign 2048
exception_vector_table:
  ; Current EL with SP0
  .balign 128
  b synchronous_handler_sp0
  .balign 128
  b irq_handler_sp0
  .balign 128
  b fiq_handler_sp0
  .balign 128
  b serror_handler_sp0

  ; Current EL with SPx
  .balign 128
  b synchronous_handler_spx
  .balign 128
  b irq_handler_spx
  .balign 128
  b fiq_handler_spx
  .balign 128
  b serror_handler_spx

  ; Lower EL using AArch64
  .balign 128
  b lower_el_sync_handler
  .balign 128
  b lower_el_irq_handler
  .balign 128
  b lower_el_fiq_handler
  .balign 128
  b lower_el_serror_handler

  ; Lower EL using AArch32
  .balign 128
  b lower_el_a32_sync_handler
  .balign 128
  b lower_el_a32_irq_handler
  .balign 128
  b lower_el_a32_fiq_handler
  .balign 128
  b lower_el_a32_serror_handler
```

### Linker Script

The linker script defines the memory layout for the bare metal binary. It specifies the entry point, section placement, and symbol definitions used by the startup code:

```ld
/* vuma-bare-metal.ld */
ENTRY(_start)

MEMORY
{
  RAM (rwx) : ORIGIN = 0x80000, LENGTH = 0x3F800000  /* 1 GB minus 512 KB */
}

SECTIONS
{
  .text : {
    *(.text.boot)           /* boot code first, at 0x80000 */
    *(.text .text.*)        /* all other code */
  } > RAM

  .rodata : {
    *(.rodata .rodata.*)
  } > RAM

  .data : {
    *(.data .data.*)
  } > RAM

  .bss : {
    _bss_start = .;
    *(.bss .bss.*)
    *(COMMON)
    _bss_end = .;
  } > RAM

  . = ALIGN(16);
  . = . + 0x10000;         /* 64 KB stack for core 0 */
  _stack_top = .;

  /DISCARD/ : {
    *(.comment)
    *(.note.*)
    *(.eh_frame*)
  }
}
```

The `/DISCARD/` directive removes unnecessary sections that the bare metal environment does not need (debug notes, comments, exception frames). The stack is placed immediately after BSS, growing downward from `_stack_top`. For multi-core configurations, additional stack areas can be defined by extending the linker script with per-core stack symbols.

---

## 7. Optimization Passes

The VUMA code generator applies a series of optimization passes after initial instruction selection and before final emission. These passes transform the instruction stream to improve performance on the Cortex-A76 microarchitecture while preserving the program's observable semantics. The optimization pipeline runs in a fixed order: constant folding, dead code elimination, instruction scheduling, loop unrolling, and function inlining. Each pass is described in detail below with ARM64 instruction examples targeting the Cortex-A76's specific pipeline characteristics.

### Constant Folding

Constant folding evaluates expressions whose operands are all known at compile time, replacing the computation with its result. This eliminates unnecessary arithmetic instructions and reduces register pressure. The pass walks the instruction stream looking for computation nodes where both source operands are immediate constants or have been previously folded. On ARM64, many immediate forms have encoding constraints (12-bit with optional shift), so the pass also checks whether the folded result can be represented as an immediate operand:

```asm
; Before constant folding:
mov x0, #10
mov x1, #20
add x2, x0, x1             ; x2 = 10 + 20

; After constant folding:
mov x2, #30                 ; direct assignment of folded result
```

For more complex constant expressions involving multiplication and division:

```asm
; Before:
mov x0, #6
mov x1, #7
mul x2, x0, x1             ; x2 = 6 * 7

; After:
mov x2, #42                 ; folded result

; Division by constant (before):
mov x0, #100
mov x1, #4
udiv x2, x0, x1            ; x2 = 100 / 4

; Division by constant (after):
mov x2, #25                 ; folded result

; For division by non-power-of-2 constants at runtime, the compiler
; replaces udiv with multiply-by-reciprocal:
; x / 7 → multiply by 0x2492492492492493, then shift
mov x1, #0x49249249         ; load reciprocal constant (low bits)
movk x1, #0x9249, lsl #16
movk x1, #0x2492, lsl #32
movk x1, #0x4924, lsl #48
mul x2, x0, x1
lsr x2, x2, #3             ; approximate x/7 using multiply-high + shift
```

Constant folding also applies to address calculations. When the base address of a global and a fixed offset are both known, the pass combines them into a single address or a more efficient addressing mode:

```asm
; Before:
adrp x0, vuma_array
add  x0, x0, :lo12:vuma_array
add  x0, x0, #64            ; offset into array

; After (if the linker resolves the full address):
adrp x0, vuma_array + 64
add  x0, x0, :lo12:(vuma_array + 64)
```

### Dead Code Elimination

Dead code elimination removes instructions whose results are never used. This pass is especially important after constant folding, which often leaves behind the original definition instructions (now superseded by the folded constant). The pass builds a use-def chain for each register and marks any instruction whose destination register has no subsequent uses as dead:

```asm
; Before dead code elimination:
mov x0, #30                 ; constant-folded result (used)
mov x1, #10                 ; original operand (no longer used)
mov x2, #20                 ; original operand (no longer used)
add x3, x1, x2              ; dead computation
str x0, [x_base]            ; only x0 is actually used

; After dead code elimination:
mov x0, #30
str x0, [x_base]            ; eliminated mov x1, mov x2, add x3
```

Dead code elimination also removes unreachable allocations and their corresponding deallocations:

```asm
; Before: dead allocation in unreachable branch
b .skip
sub sp, sp, #64             ; dead AllocationNode
add sp, sp, #64             ; dead DeallocationNode
.skip:

; After: both removed
b .skip
.skip:
```

The pass also eliminates redundant stores. If a value is stored to the same address twice without an intervening load from that address, the first store is dead and can be removed:

```asm
; Before:
str x0, [x_base]            ; dead store (overwritten below)
str x1, [x_base]            ; live store

; After:
str x1, [x_base]
```

### Instruction Scheduling for Cortex-A76

The Cortex-A76 is a 4-wide out-of-order superscalar processor with a 128-entry reorder buffer. While the out-of-order engine dynamically reorders instructions to hide latencies, the compiler's instruction scheduler can still improve performance by arranging instructions to minimize structural hazards and maximize instruction-level parallelism. The scheduler models the Cortex-A76's pipeline with the following latency characteristics:

| Operation           | Latency | Throughput     |
|---------------------|---------|----------------|
| Integer add/sub/log | 1 cycle | 4 per cycle    |
| Integer multiply    | 3 cycles| 1 per cycle    |
| Integer divide      | 4-12 cycles | 1 per 4-12 cycles |
| L1 data cache load  | 4 cycles| 2 per cycle    |
| L2 cache load       | 10 cycles| 1 per cycle   |
| Branch (predicted)  | 1 cycle | 2 per cycle    |
| Branch (mispredicted)| ~12 cycles | —           |

The scheduler's primary goal is to fill load-use delay slots. After a load instruction, the loaded value is not available for 4 cycles. The scheduler reorders independent instructions to execute in the load's shadow:

```asm
; Before scheduling (load-use stall):
ldr x0, [x_base]            ; load: 4-cycle latency
add x1, x0, #10             ; stalled: depends on x0 (4-cycle bubble)

; After scheduling (independent work fills the bubble):
ldr x0, [x_base]            ; load: 4-cycle latency
add x2, x3, x4              ; independent: executes in load shadow
sub x5, x6, #1              ; independent: executes in load shadow
mul x7, x8, x9              ; independent: executes in load shadow
add x1, x0, #10             ; x0 now available, no stall
```

The scheduler also avoids back-to-back multiply instructions, which compete for the Cortex-A76's single multiply unit. It interleaves multiplies with independent ALU operations:

```asm
; Before: two multiplies compete for the same execution unit
mul x0, x1, x2
mul x3, x4, x5              ; stalls waiting for multiply unit

; After: interleave with independent ALU ops
mul x0, x1, x2
add x6, x7, x8              ; uses different execution unit
mul x3, x4, x5              ; multiply unit now free
```

For branch-heavy code, the scheduler aligns branch targets to 16-byte boundaries to optimize the Cortex-A76's branch predictor's target cache:

```asm
.balign 16
.loop_body:
  ; loop body starts at aligned address
  ldr x0, [x_base, x_index, lsl #3]
  add x0, x0, #1
  str x0, [x_base, x_index, lsl #3]
  add x_index, x_index, #1
  cmp x_index, x_limit
  b.lt .loop_body
```

### Loop Unrolling

Loop unrolling reduces the overhead of branch and counter maintenance instructions by replicating the loop body multiple times. For small bounded loops where the trip count is known at compile time, the compiler can fully unroll the loop, eliminating the loop overhead entirely. For loops with large or unknown trip counts, partial unrolling reduces the per-iteration overhead:

```asm
; Before unrolling (loop: sum 4 elements):
mov x0, xzr                  ; accumulator = 0
mov x1, #0                   ; index = 0
.loop:
  ldr x2, [x_base, x1, lsl #3]
  add x0, x0, x2
  add x1, x1, #1
  cmp x1, #4
  b.lt .loop

; After full unrolling (trip count = 4, known at compile time):
ldr x2, [x_base, #0]
ldr x3, [x_base, #8]
ldr x4, [x_base, #16]
ldr x5, [x_base, #24]
add x0, x2, x3
add x0, x0, x4
add x0, x0, x5
```

For partially unrolled loops, the compiler emits a prologue to handle the remainder iterations and an unrolled main body:

```asm
; Partial unrolling by 4 with remainder handling
; x_limit = total iterations
lsr x2, x_limit, #2          ; x2 = x_limit / 4 (main loop count)
and x3, x_limit, #3          ; x3 = x_limit % 4 (remainder)

cbz x2, .remainder           ; skip main loop if no full groups

.unrolled_loop:
  ldr x4, [x_base, x1, lsl #3]       ; element 0
  ldr x5, [x_base, x1, lsl #3, add #1] ; element 1 (offset by 8)
  add x1, x1, #2
  ldr x6, [x_base, x1, lsl #3]       ; element 2
  ldr x7, [x_base, x1, lsl #3, add #1] ; element 3
  add x1, x1, #2
  add x0, x0, x4
  add x0, x0, x5
  add x0, x0, x6
  add x0, x0, x7
  sub x2, x2, #1
  cbnz x2, .unrolled_loop

.remainder:
  cbz x3, .done
.remainder_loop:
  ldr x4, [x_base, x1, lsl #3]
  add x0, x0, x4
  add x1, x1, #1
  sub x3, x3, #1
  cbnz x3, .remainder_loop
.done:
```

The unroll factor is tuned for the Cortex-A76's reorder buffer size (128 entries) and the loop body's register pressure. Unrolling too aggressively exhausts physical registers, causing spills that negate the benefit. A heuristic of 4x unrolling for simple bodies and 2x for complex bodies works well on the Cortex-A76.

### Function Inlining

Function inlining replaces a function call (`bl` + `ret`) with the function's body directly at the call site, eliminating the call/return overhead and exposing the inlined body to further optimization (constant folding, dead code elimination). The VUMA compiler inlines small functions (fewer than 16 instructions) that are called from a limited number of sites:

```asm
; Before inlining: call to small helper function
; Caller:
  bl increment_counter       ; call overhead: bl + ret + register saves
; Callee (increment_counter):
  ldr x0, [x_counter]
  add x0, x0, #1
  str x0, [x_counter]
  ret

; After inlining:
  ldr x0, [x_counter]       ; directly inlined body
  add x0, x0, #1
  str x0, [x_counter]
  ; eliminated: bl, ret, potential register saves/restores
```

Inlined code also benefits from context-specific optimizations that are impossible across function boundaries:

```asm
; Before inlining: constant argument not known in callee
mov x0, #0                   ; argument: increment amount
bl add_to_counter

; After inlining: constant propagation into the body
; add_to_counter(x0=0) → just a no-op or simplified operation
; Dead code elimination can remove the entire inlined body
```

The inlining decision algorithm considers the following factors:

1. **Function size:** Only functions with ≤16 instructions are candidates. Larger functions are never inlined to avoid code bloat.
2. **Call site count:** Functions called from more than 8 sites are not inlined to prevent excessive code size growth.
3. **Hot path analysis:** Functions on the hot path (identified by profiling or heuristic estimation) are preferentially inlined.
4. **Recursive functions:** Never inlined to avoid infinite expansion.
5. **Varargs functions:** Never inlined due to complex calling convention handling.

The combined effect of all five optimization passes — constant folding, dead code elimination, instruction scheduling, loop unrolling, and function inlining — typically reduces generated code size by 15-25% and improves execution speed by 20-40% on the Cortex-A76 compared to unoptimized code generation. The passes are run iteratively until a fixed point is reached (no further improvements), with a maximum of 3 iterations to limit compilation time.

---

## 8. Complex Control Flow Lowering (M2.5 Enhancement)

The M2.5 milestone extends the VUMA code generator to handle complex control flow patterns beyond the simple conditional branches and function calls described in Section 1. Real-world programs require nested loops, recursive invocations, multi-way dispatch (switch/match), and the correct mapping of SCG ControlNode graphs to ARM64 basic blocks. This section documents the lowering strategies for each of these advanced control flow constructs, including the metadata tracking required for correct code generation and the instruction sequences emitted for the Cortex-A76.

### Nested Loop Handling with Loop Nesting Tracking

When the SCG contains nested ControlNode loops, the code generator must maintain a `loop_nesting` counter that tracks the current nesting depth. Each loop entry increments the counter, and each loop exit decrements it. The nesting depth determines which callee-saved registers are allocated to loop induction variables — deeper loops receive higher-priority register assignments since their variables are accessed more frequently. The code generator also uses the nesting depth to decide on loop unrolling aggressiveness: innermost loops (highest nesting depth) are candidates for full unrolling when the trip count is small, while outer loops are only partially unrolled or left intact.

For nested loops, the code generator emits distinct loop labels that encode the nesting level, preventing label collisions. The innermost loop counter typically resides in a callee-saved register (e.g., `x19` for the outer loop, `x20` for the inner loop), while the outer loop counter must be preserved across inner loop iterations. If the inner loop contains function calls, the outer loop counter must be in a callee-saved register or spilled to the stack:

```asm
; Nested loop: outer (x19 = i), inner (x20 = j)
mov x19, #0                ; i = 0
.outer_loop:
  mov x20, #0              ; j = 0
  .inner_loop:
    ldr x0, [x_base, x20, lsl #3]
    add x0, x0, x19        ; use outer variable
    str x0, [x_base, x20, lsl #3]
    add x20, x20, #1
    cmp x20, #16
    b.lt .inner_loop
  add x19, x19, #1
  cmp x19, #8
  b.lt .outer_loop
```

The `loop_nesting` metadata is attached to each loop's ControlNode in the SCG and propagated to the IR during the SCG-to-IR lowering pass. This metadata also informs the register allocator's spill cost estimation — variables in deeper loops have higher spill costs because a spill inside a deeply nested loop executes far more frequently than one in an outer scope.

### Recursive Function Call Lowering

Recursive calls are lowered using the standard `bl` instruction, but the code generator must ensure that the stack frame is properly set up to support arbitrary recursion depth. Each recursive invocation requires a full prologue/epilogue pair that saves the link register (`x30`) and the frame pointer (`x29`), allocates space for local variables, and preserves any callee-saved registers used by the function. The code generator marks recursive functions with a `is_recursive` flag during SCG analysis, which forces the emission of a complete frame pointer chain even in cases where a leaf function optimization would otherwise omit it.

For tail-recursive functions, the code generator performs tail-call optimization by replacing the `bl` + `ret` sequence with a direct `b` (unconditional branch) to the function entry, reusing the current stack frame. This transforms stack-consuming recursion into constant-stack iteration:

```asm
; Tail-recursive: factorial accumulator
; int fac(int n, int acc) { return n == 0 ? acc : fac(n-1, acc*n); }
fac:
  cmp x0, #0
  b.eq .base_case
  mul x1, x1, x0           ; acc *= n
  sub x0, x0, #1           ; n -= 1
  b fac                     ; tail call: reuse stack frame
.base_case:
  mov x0, x1               ; return acc
  ret
```

### Switch/Match Dispatch

Multi-way dispatch (switch/match) is lowered using a combination of test-bit-and-branch instructions (`TBZ`/`TBNZ`) for sparse boolean tests, compare-and-branch chains (`CMP` + `B.EQ`) for small case sets, and jump tables for dense case ranges. The code generator selects the lowering strategy based on the number and distribution of case values:

**Small switch (≤4 cases):** Emit a linear chain of `CMP` + `B.EQ` pairs. This is the most efficient strategy for small case counts, as each comparison takes a single cycle on the Cortex-A76 when the branch is correctly predicted:

```asm
; Switch on x0 with cases 1, 3, 5, 7
cmp x0, #1
b.eq .case_1
cmp x0, #3
b.eq .case_3
cmp x0, #5
b.eq .case_5
cmp x0, #7
b.eq .case_7
b .default_case
```

**Medium switch (5–15 cases):** Use `TBZ`/`TBNZ` for bit-pattern matching when cases map to power-of-two values, otherwise use a binary search tree of `CMP` + `B.LT`/`B.GT` pairs to achieve O(log n) dispatch time:

```asm
; Binary search dispatch for cases 0-15
cmp x0, #8
b.ge .upper_half
cmp x0, #4
b.ge .upper_quarter
cmp x0, #2
b.ge .check_2_3
cmp x0, #0
b.eq .case_0
cmp x0, #1
b.eq .case_1
b .default
```

**Large switch (>15 cases, dense):** Emit a jump table in `.rodata` with a bounds-checked index. The discriminant is used as an index into the table after subtracting the minimum case value:

```asm
; Jump table dispatch for dense cases 0-63
sub x1, x0, #0             ; offset by minimum case value
cmp x1, #63                ; bounds check
b.hi .default_case
adrp x2, .jump_table
add  x2, x2, :lo12:.jump_table
ldr x2, [x2, x1, lsl #3]  ; load target address from table
br x2                       ; indirect branch to case
```

### SCG ControlNode → IR BasicBlock Mapping

The SCG-to-IR lowering pass maps each SCG ControlNode to one or more IR BasicBlocks. A simple branch ControlNode maps to a single BasicBlock with a conditional terminator. A switch ControlNode maps to a header BasicBlock (containing the dispatch logic) plus one BasicBlock per case arm. Loop ControlNodes map to a pre-header BasicBlock (for loop-invariant code motion), a header BasicBlock (containing the loop condition), a body BasicBlock, and an exit BasicBlock. The pre-header is crucial for the register allocator because it provides a single entry point where loop-invariant values can be hoisted and spilled registers can be restored. The mapping preserves the SCG's ControlFlow edge annotations in the IR's successor/predecessor lists, enabling later optimization passes to reconstruct the original control flow structure.

---

## 9. AAPCS64 Calling Convention — M2.5 Enhanced Details (M2.5 Enhancement)

Section 2 described the foundational AAPCS64 calling convention as implemented by VUMA's code generator. The M2.5 milestone extends this with precise specifications for argument passing edge cases, stack spilling protocols for functions exceeding the register argument capacity, return value handling for complex types, callee-saved register preservation requirements, stack frame layout rules, and frame pointer conventions. This section provides the complete, implementation-ready specification that the code generator must follow when emitting function prologues, epilogues, and call sites.

### Argument Passing: Integer Registers x0–x7

The first eight integer or pointer arguments are passed in registers `x0` through `x7` in left-to-right order. Each argument consumes exactly one register regardless of its size — a `u8` argument passed in `x0` occupies the full 64-bit register, with the value zero-extended to 64 bits by the caller. Arguments smaller than 8 bytes are not packed into a single register; each argument gets its own register. This simplification avoids the complexity of bit-field packing and is consistent with how mainstream ARM64 compilers (GCC, Clang) handle sub-register arguments.

### Argument Passing: Floating-Point Registers v0–v7

Floating-point arguments in single-precision (`f32`) or double-precision (`f64`) format are passed in registers `v0` through `v7`. Each SIMD/floating-point register is 128 bits wide, but only the relevant portion is used: the bottom 32 bits for `f32` (register name `s0`–`s7`) and the bottom 64 bits for `f64` (register name `d0`–`d7`). Half-precision (`f16`) arguments use the bottom 16 bits of `v0`–`v7`. The integer and floating-point register assignment counters are independent: a function signature `(int, float, int, float)` maps to `(x0, s0, x1, s1)`, not `(x0, s0, x2, s2)`.

### Stack Spilling for >8 Arguments

When a function takes more than eight integer arguments or more than eight floating-point arguments, the excess arguments are passed on the stack. The caller must reserve space for these stack arguments before the `bl` instruction. Stack arguments are placed at ascending addresses from the stack pointer, with each argument occupying 8 bytes (naturally aligned). The ninth integer argument resides at `[sp, #0]`, the tenth at `[sp, #8]`, and so on. The caller must allocate at least enough stack space for all stack arguments, rounded up to 16-byte alignment:

```asm
; Function call with 10 integer arguments
; x0-x7: args 1-8
; Stack:  args 9-10
sub sp, sp, #16            ; allocate 16 bytes for 2 stack args (16-byte aligned)
mov x0, x_a1               ; arg 1
mov x1, x_a2               ; arg 2
; ... x2-x7 for args 3-8 ...
str x_a9, [sp, #0]         ; arg 9 at [sp]
str x_a10, [sp, #8]        ; arg 10 at [sp, #8]
bl function_with_many_args
add sp, sp, #16            ; clean up stack args
```

The callee accesses stack arguments via fixed offsets from the frame pointer or stack pointer, depending on the prologue structure. When the frame pointer is established, stack arguments are at `[x29, #16]`, `[x29, #24]`, etc. (offset by 16 to account for the saved `x29`/`x30` pair).

### Return Value Handling

Integer and pointer return values are placed in `x0`. Floating-point returns use `v0` (as `s0` for `f32`, `d0` for `f64`). For 128-bit integer returns, the value is split across `x0` (low 64 bits) and `x1` (high 64 bits). Composite structs returned by value follow the AAPCS64 Homogeneous Floating-point Aggregate (HFA) and Homogeneous Vector Aggregate (HVA) rules: if a struct consists entirely of up to four floating-point members, the members are returned in `v0`–`v3`. All other composites larger than 16 bytes are returned via the indirect return mechanism using `x8` as the caller-provided buffer pointer.

### Callee-Saved Register Preservation: x19–x28, d8–d15

The full set of callee-saved registers is: `x19` through `x28` (integer), `x29` (frame pointer), `x30` (link register), and `d8` through `d15` (floating-point). A function that modifies any of these registers must save and restore them. The code generator tracks which callee-saved registers are actually used by each function and only emits save/restore pairs for the registers that are actually clobbered. This minimizes prologue/epilogue overhead. The save order follows the AAPCS64 recommendation: registers are saved in ascending numerical order using `stp` pairs:

```asm
; Prologue: save x19-x22 and d8-d9 (only the ones we use)
stp x29, x30, [sp, #-32]!   ; always save fp + lr
mov x29, sp
stp x19, x20, [sp, #16]     ; save callee-saved x19, x20
stp x21, x22, [sp, #32]     ; save callee-saved x21, x22 (if needed)
stp d8, d9, [sp, #48]       ; save callee-saved d8, d9
```

Floating-point callee-saved registers `d8`–`d15` are often overlooked in code generators that primarily target integer workloads. The VUMA code generator correctly handles these when the function uses floating-point operations that clobber `d8`–`d15`.

### Stack Frame Layout: 16-Byte Alignment

The stack frame layout follows a strict 16-byte alignment rule at all times. The total frame size (including saved registers, local variables, spill slots, and stack arguments for called functions) must be a multiple of 16 bytes. The layout from high address to low address is:

```
Higher Address
  ┌──────────────────────────┐
  │ Stack arguments to callees│  (if any)
  ├──────────────────────────┤
  │ Spill slots               │  8 bytes each
  ├──────────────────────────┤
  │ Local variables           │  8 bytes each, naturally aligned
  ├──────────────────────────┤
  │ Saved d8-d15              │  8 bytes each
  ├──────────────────────────┤
  │ Saved x19-x28             │  8 bytes each
  ├──────────────────────────┤
  │ Saved x29, x30            │  16 bytes (stp pair)
  └──────────────────────────┘ ← x29 = sp after prologue
Lower Address
```

The frame pointer `x29` points to the bottom of the saved register area, providing a stable reference for accessing locals, spill slots, and incoming stack arguments regardless of dynamic stack adjustments.

### Frame Pointer Convention: x29

The frame pointer `x29` is always established in non-leaf functions and in any function that has variadic arguments, uses `alloca`, or has dynamic stack allocation. Leaf functions (those that make no calls) may omit the frame pointer and use `sp`-relative addressing exclusively, which frees up `x29` as an additional general-purpose register. When the frame pointer is omitted, the compiler must ensure that all `sp`-relative references are recalculated whenever the stack pointer changes (e.g., when pushing arguments for a call). The VUMA code generator defaults to always using a frame pointer for correctness and debuggability, with an optimization flag to omit it in verified leaf functions.

---

## 10. Register Allocator Enhancement (M2.5 Enhancement)

The M2.5 milestone significantly enhances the register allocator beyond the basic linear-scan strategy described in Section 3. The enhanced allocator supports 32 or more virtual registers, implements spill slot allocation with frame-pointer-relative addressing, employs a spill cost estimation heuristic, uses an LRU-based spill candidate selection algorithm, and performs register coalescing for copy instructions. These enhancements are essential for compiling the complex SCG graphs produced by VUMA's front end, where a single function may have dozens of simultaneously live values including BD metadata, IVE guard results, and bounds check temporaries.

### Linear-Scan with 32+ Virtual Register Support

The enhanced linear-scan allocator processes virtual registers in order of their live range start positions. Each virtual register is assigned a unique index, and the allocator supports up to 256 virtual registers per function (limited by a compile-time constant). When a virtual register becomes live, the allocator searches for an available physical register in the preferred class (argument, temporary, or callee-saved) based on the register partitioning described in Section 3. If no physical register is available, the allocator invokes the spill heuristic to free a register.

The live range computation has been enhanced to handle the complex control flow patterns introduced in Section 8. For nested loops, the live range of a loop variable is extended to cover the entire loop body, including all nested sub-loops. For switch/match dispatch, the live range of a value defined before the switch and used after it is extended across all case arms. The allocator uses the SCG's ControlFlow edge annotations to compute precise live ranges that account for all possible execution paths.

### Spill Slot Allocation

Spill slots are pre-allocated in the function prologue at fixed offsets from the frame pointer. The number of spill slots is determined by the allocator's first pass, which counts the maximum number of simultaneously spilled virtual registers at any program point. Each spill slot is 8 bytes wide (sufficient for any general-purpose register value) and is accessed using `str`/`ldr` with the `[x29, #offset]` addressing mode:

```asm
; Prologue with 4 spill slots
stp x29, x30, [sp, #-48]!   ; save fp, lr; 16 bytes
mov x29, sp
; Spill slot 0: [x29, #16]
; Spill slot 1: [x29, #24]
; Spill slot 2: [x29, #32]
; Spill slot 3: [x29, #40]
```

For floating-point values that must be spilled, the allocator uses `str d0, [x29, #offset]` and `ldr d0, [x29, #offset]`, which also use 8-byte slots. SIMD values wider than 64 bits (e.g., 128-bit `q` registers) use 16-byte spill slots, requiring double-width alignment.

### Spill Cost Estimation Heuristic

The spill cost for a virtual register is estimated as the product of three factors: (1) the estimated execution frequency of each instruction that references the register (with loop nesting depth as a proxy, so references in a doubly-nested loop have 10x the cost of references in a single loop), (2) the number of distinct references to the register within its live range, and (3) a penalty factor for registers that are involved in memory access address computations (spilling such a register would require a reload before every load/store that uses it as a base or index register). The formula is:

```
spill_cost(vreg) = Σ (frequency(ref) × 1.0) + (address_use_count(vreg) × 5.0)
```

where `frequency(ref)` is the estimated execution frequency of the instruction containing the reference, and `address_use_count` counts the number of times the register appears as a base or index in a load/store instruction.

### LRU-Based Spill Candidate Selection

When a physical register must be freed, the allocator selects the spill candidate using a Least Recently Used (LRU) policy. Each physical register tracks the program counter position of its last definition or use. The register with the oldest last-use position is selected for spilling, on the assumption that its value is least likely to be needed again soon. This is similar to the page replacement algorithm in virtual memory systems and works well in practice because program execution tends to exhibit locality — recently used values are more likely to be used again.

The LRU policy is modified by the spill cost heuristic: if the register with the oldest last-use position has a very high spill cost (e.g., it is a loop induction variable in a deeply nested loop), the allocator instead considers the next-oldest candidate. This prevents the catastrophic performance degradation that would result from spilling a hot loop variable to make room for a cold temporary. The threshold for "very high" spill cost is determined dynamically as 2× the median spill cost of all currently allocated registers.

### Register Coalescing for Copy Instructions

The allocator performs register coalescing to eliminate unnecessary `mov` instructions that copy values between registers. When the SCG contains a data-flow edge that maps to a register-to-register copy (e.g., `mov x0, x1`), the allocator attempts to assign the source and destination virtual registers to the same physical register, making the copy instruction a no-op that can be removed by the dead code elimination pass. Coalescing is attempted after the initial allocation pass and is governed by the following rules:

1. **Same class coalescing:** Virtual registers in the same partition class (argument, temporary, callee-saved) can always be coalesced if neither register interferes with the other's live range.
2. **Cross-class coalescing:** Virtual registers in different classes can be coalesced only if the resulting physical register is in a class that is valid for both uses (e.g., coalescing a temporary-class source with a callee-saved-class destination is allowed, since the callee-saved register is a superset of the temporary register's properties).
3. **Conflict avoidance:** Coalescing is not performed if it would increase the spill cost of either register or if it would create a live range that spans a call site in a caller-saved register.

The coalescing pass runs iteratively until no further copies can be eliminated, typically converging in 2–3 iterations. On the Cortex-A76, eliminating a `mov` instruction saves 1 cycle of execution time and reduces pressure on the issue slots, making coalescing particularly valuable in tight loops.

---

## 11. VUMA→ARM64 Instruction Mapping Table (M2.5 Enhancement)

This section provides a consolidated reference table mapping each SCG node type to its ARM64 instruction sequence. While Sections 1–7 described each mapping in prose with examples, this table serves as a quick-reference for code generator implementers. The M2.5 enhancements add mappings for the complex control flow patterns described in Section 8 and include the enhanced register allocator's interaction with each node type.

### SCG Node → ARM64 Instruction Mapping

| SCG Node Type | ARM64 Instruction(s) | Notes |
|---|---|---|
| **AllocationNode** (Stack) | `sub sp, sp, #size` + `mov x0, sp` | Size rounded up to 16-byte alignment. For stack probing, insert `str xzr, [sp]` every 4096 bytes. |
| **AllocationNode** (Heap) | `mov x0, #size` + `bl malloc` + `cbz x0, .fail` | NULL check required by VUMA safety profile. |
| **AllocationNode** (Arena) | `add x0, x19, x20` + `add x20, x20, #size` | x19=arena_base, x20=arena_offset (callee-saved). |
| **DeallocationNode** (Stack) | `add sp, sp, #size` | Size must match the corresponding AllocationNode. |
| **DeallocationNode** (Heap) | `bl free` | Pointer in x0. |
| **DeallocationNode** (Arena) | *(no code emitted)* | Bulk deallocation via `arena_destroy`. |
| **AccessNode** (Read, fixed) | `ldr Xt, [Xn, #offset]` | Size suffix: `x` for 64-bit, `w` for 32-bit, `h` for 16-bit, `b` for 8-bit. |
| **AccessNode** (Read, indexed) | `ldr Xt, [Xn, Xm, lsl #shift]` | Shift = log2(element_size). |
| **AccessNode** (Write, fixed) | `str Xt, [Xn, #offset]` | Same size suffix rules as Read. |
| **AccessNode** (Write, indexed) | `str Xt, [Xn, Xm, lsl #shift]` | Same shift rules as indexed Read. |
| **CastNode** (same-size, same-domain) | *(no-op)* | Bits are reinterpreted in-place. |
| **CastNode** (int↔float, same-size) | `fmov Dt, Xt` or `fmov Xt, Dt` | Cross-register-bank move. |
| **CastNode** (widen int) | `sxtw Xt, Wt` (signed) or `mov Wt, Wt` (unsigned zero-extend) | Sign/zero extension. |
| **CastNode** (int→float, convert) | `scvtf Dt, Xt` (signed) or `ucvtf Dt, Xt` (unsigned) | Numeric conversion, not bit reinterpret. |
| **CastNode** (float→int, convert) | `fcvtzs Xt, Dt` (signed) or `fcvtzu Xt, Dt` (unsigned) | Truncates toward zero. |
| **ControlNode** (Branch, zero/non-zero) | `cbz Xt, .label` / `cbnz Xt, .label` | Single-instruction conditional branch. |
| **ControlNode** (Branch, comparison) | `cmp Xt, Xm` + `b.cc .label` | cc = eq/ne/lt/ge/lo/hs/etc. |
| **ControlNode** (Call) | `bl target` | Return address in x30. |
| **ControlNode** (Return) | `mov x0, Xt` + `ret` | Or just `ret` for void returns. |
| **ControlNode** (Switch, ≤4 cases) | Chain of `cmp` + `b.eq` | Linear scan dispatch. |
| **ControlNode** (Switch, 5–15 cases) | Binary search: `cmp` + `b.lt`/`b.ge` tree | O(log n) dispatch. |
| **ControlNode** (Switch, >15 dense) | Jump table: `sub` + `cmp` + `adrp` + `ldr` + `br` | Bounds-checked indirect branch. |
| **ControlNode** (Loop) | `cmp` + `b.cc` (back-edge) | loop_nesting metadata attached. |
| **ComputationNode** (Add) | `add Xt, Xn, Xm` | Immediate form: `add Xt, Xn, #imm` |
| **ComputationNode** (Sub) | `sub Xt, Xn, Xm` | Immediate form: `sub Xt, Xn, #imm` |
| **ComputationNode** (Mul) | `mul Xt, Xn, Xm` | 3-cycle latency on Cortex-A76. |
| **ComputationNode** (SDiv) | `sdiv Xt, Xn, Xm` | 4–12 cycle latency on Cortex-A76. |
| **ComputationNode** (UDiv) | `udiv Xt, Xn, Xm` | 4–12 cycle latency on Cortex-A76. |
| **ComputationNode** (And/Orr/Eor) | `and`/`orr`/`eor Xt, Xn, Xm` | 1-cycle latency, 4 per cycle throughput. |

### SCG Node → MRS/MSR Mapping for Special Operations

| SCG Node Type | Special Instruction | Purpose |
|---|---|---|
| **AllocationNode** (Stack probe) | `mrs Xt, sp` / `msr sp, Xt` | Read/write stack pointer for stack probing on large allocations. |
| **AllocationNode** (Arena init) | `msr Xt, <system_reg>` | Initialize arena from system register (bare metal). |
| **AccessNode** (BD metadata) | `mrs Xt, tpidr_el0` | Read thread pointer for thread-local BD metadata access. |
| **ControlNode** (Tail call) | `b target` | Unconditional branch replaces `bl`+`ret` for tail calls. |
| **ControlNode** (TBZ/TBNZ) | `tbz Xt, #bit, .label` / `tbnz Xt, #bit, .label` | Test single bit and branch — used for boolean switch discriminants. |

---

## 12. Pi 5 Specific Considerations (M2.5 Enhancement)

The Raspberry Pi 5's BCM2712 SoC features four Cortex-A76 cores clocked at 2.4 GHz. The Cortex-A76 implements the ARMv8.2-A architecture with specific microarchitectural characteristics that the VUMA code generator must account for to achieve optimal performance. This section documents the pipeline considerations, instruction scheduling hints, cache geometry, and branch predictor behavior that are specific to the Pi 5 platform.

### Cortex-A76 Pipeline Considerations

The Cortex-A76 is a 4-wide out-of-order superscalar processor with an 11-stage integer pipeline and a 13–15-stage floating-point/SIMD pipeline. The out-of-order engine has a 128-entry reorder buffer (ROB) and can rename up to 4 micro-ops per cycle. The pipeline is divided into three clusters: ALU cluster (2 ALU pipes + 1 multiply pipe + 1 divide pipe), load/store cluster (2 load/store pipes with address generation), and the floating-point/SIMD cluster (1 FP multiply/add pipe + 1 FP miscellaneous pipe).

Key pipeline characteristics that affect code generation:

- **Multiply latency is 3 cycles** with 1-per-cycle throughput. Back-to-back multiplies create a bottleneck on the single multiply pipe. The scheduler should interleave multiplies with independent ALU operations.
- **Division is not pipelined** and takes 4–12 cycles depending on operand magnitude. Division should be avoided in hot loops; the compiler should replace division by constants with multiply-by-reciprocal sequences.
- **Load-use latency is 4 cycles** from the L1 data cache. The scheduler must fill at least 3 independent instruction slots between a load and the first use of the loaded value to avoid stalls.
- **Store-to-load forwarding** works when a load follows a store to the same address with no size mismatch, with a 4-cycle forwarding latency. Mismatched sizes (e.g., store 64-bit, load 32-bit from the same address) incur a 9-cycle penalty on the Cortex-A76 because the store must complete to the L1 cache before the load can be satisfied.

### Instruction Scheduling Hints

The VUMA code generator emits scheduling hints using instruction ordering and alignment directives. While the Cortex-A76's out-of-order engine dynamically schedules instructions, the compiler's static scheduling can reduce pressure on the ROB and improve instruction cache utilization:

- **Align loop entries to 16 bytes:** The Cortex-A76's fetch unit fetches 4 instructions per cycle from a 16-byte aligned boundary. Loop entries aligned to 16 bytes ensure that the first fetch of each loop iteration captures the maximum number of instructions, reducing fetch stalls.
- **Avoid more than 3 consecutive branches:** The Cortex-A76 can predict up to 2 branches per cycle. Sequences of 3+ consecutive conditional branches create prediction bottlenecks. When possible, replace branch chains with `tbz`/`tbnz` or compute a branch target address.
- **Schedule `adrp` early:** The `adrp` instruction has a 2-cycle latency on the Cortex-A76 because it computes a PC-relative page address. Place `adrp` instructions at least 2 instructions before the `add` or `ldr` that uses the result.
- **Pair loads with independent ALU ops:** After each `ldr`, schedule 3–4 independent ALU instructions before using the loaded value. This fills the load-use latency bubble and keeps all 4 issue slots busy.

### Cache Line Size: 64 Bytes

The Cortex-A76's L1 data cache and L2 cache both use 64-byte cache lines. This has several implications for VUMA code generation:

- **Structure padding:** Structures should be laid out to minimize cache line crossings for frequently accessed fields. The "hot" fields of a struct should be grouped together at the beginning, within a single 64-byte cache line if possible.
- **Alignment of frequently accessed data:** Global variables and arena allocations that are accessed from hot loops should be 64-byte aligned to ensure they start at a cache line boundary, preventing false sharing between cores.
- **Stack frame sizing:** The frame pointer and frequently accessed local variables should be placed within the first 64 bytes of the stack frame, as the L1 cache's spatial prefetcher is optimized for sequential access within a cache line.
- **BD metadata layout:** When passing BD metadata through memory (for functions with many BD descriptors), the descriptors should be packed into 64-byte aligned blocks so that a single cache line fill satisfies multiple bounds checks.

### Branch Predictor Behavior

The Cortex-A76 uses a two-level adaptive branch predictor with a 4K-entry Branch Target Buffer (BTB) and a 4K-entry Global History Buffer (GHB). The predictor can predict up to 2 branches per cycle and maintains accuracy above 95% for most workloads. Key behaviors that affect code generation:

- **Indirect branch prediction:** Indirect branches (used in jump-table switch dispatch) are predicted using a 256-entry indirect branch target cache. For switch statements with more than 256 distinct targets, the predictor may mispredict. The code generator should prefer direct branch chains for switches with 4 or fewer cases to avoid indirect branch misprediction penalties (~12 cycles on the Cortex-A76).
- **Loop branch prediction:** The predictor correctly predicts loops with trip counts up to approximately 256 iterations without misprediction. For very short loops (1–3 iterations), the predictor may mispredict the loop exit. The code generator should fully unroll loops with known trip counts ≤4 to avoid branch misprediction overhead.
- **Return prediction:** The Cortex-A76 has a 16-entry Return Stack Buffer (RSB) that predicts return addresses. Functions with call depth exceeding 16 levels may experience RSB overflow and return misprediction. The code generator does not need to emit `bti` instructions for return targets on the Cortex-A76 when BTI is disabled, but must insert `bti c` (Branch Target Identification, compatible) at indirect branch targets when BTI is enabled for security hardening.

---

*End of ARM64 Code Generation Algorithm Specification — VUMA Project W2-A8 (M2.5 Enhanced)*
