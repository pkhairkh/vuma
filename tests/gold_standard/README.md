# VUMA Gold-Standard Test Suite

A categorized collection of `.vuma` programs that serve as the gold standard for testing the VUMA compiler across all 8 backends (x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, Wasm32).

**Total programs:** 47  
**Categories:** 16  
**Source:** copies of `examples/*.vuma` (originals preserved)

## Categories

| # | Category | Programs | Title |
|---|----------|----------|-------|
| 1 | [`arithmetic/`](arithmetic/) | 3 | Basic Integer Arithmetic |
| 2 | [`bitwise/`](bitwise/) | 1 | Bitwise Operations |
| 3 | [`memory/`](memory/) | 5 | Memory Allocation, Load, and Store |
| 4 | [`control_flow/`](control_flow/) | 2 | Control Flow: if/else, while, for, break, continue |
| 5 | [`pointers/`](pointers/) | 3 | Pointer Arithmetic and Dereference |
| 6 | [`functions/`](functions/) | 4 | Function Calls, Recursion, and Parameters |
| 7 | [`structs/`](structs/) | 2 | Struct and Enum (Tagged Union) Types |
| 8 | [`atomics/`](atomics/) | 2 | Atomic Operations and Memory Ordering |
| 9 | [`u32_arith/`](u32_arith/) | 4 | 32-bit Arithmetic with Overflow Masking |
| 10 | [`edge_cases/`](edge_cases/) | 3 | Boundary Conditions and Unusual Features |
| 11 | [`multi_function/`](multi_function/) | 2 | Many Functions Calling Each Other |
| 12 | [`complex_stores/`](complex_stores/) | 2 | Multi-byte Stores and Computed Addresses |
| 13 | [`nested_loops/`](nested_loops/) | 1 | Two- and Three-Level Nested Loops |
| 14 | [`linked_structures/`](linked_structures/) | 3 | Linked Lists, Trees, and Ring Buffers |
| 15 | [`crypto_patterns/`](crypto_patterns/) | 3 | Cryptographic Hash and Checksum Patterns |
| 16 | [`concurrency/`](concurrency/) | 7 | Lock-Free Structures, Atomics, and Channels |

## Layout

```
tests/gold_standard/
+-- manifest.json          # machine-readable index of all categories
+-- README.md              # this file
+-- build_categories.py    # script that (re)generated this tree
+-- <category>/
    +-- README.md          # what kinds of tests belong here
    +-- *.vuma             # the test programs
```

## See also

- [`manifest.json`](manifest.json) - full machine-readable index.
- [`../../examples/README.md`](../../examples/README.md) - narrative descriptions of each example program.
- Project worklog (`worklog.md`) - Task 1-c created this structure.
