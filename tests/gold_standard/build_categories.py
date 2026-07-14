#!/usr/bin/env python3
"""Build the gold-standard test category structure for VUMA."""
from __future__ import annotations

import json
import shutil
from collections import Counter
from pathlib import Path

ROOT = Path("/tmp/my-project")
EXAMPLES = ROOT / "examples"
GOLD = ROOT / "tests" / "gold_standard"

CATEGORIES = [
    (
        "arithmetic",
        "Basic Integer Arithmetic",
        "Tests covering the fundamental integer arithmetic operators "
        "(`+`, `-`, `*`, `/`, `%`) on `i32`, `i64`, `u64` values, including "
        "literal returns, single-operator expressions, and small function-call "
        "arithmetic. These are the simplest possible programs - they should "
        "compile, execute, and exit with a predictable code on every backend.",
        [
            "Addition / subtraction / multiplication of integer literals and locals",
            "Constant returns (smallest valid programs)",
            "Single-expression function bodies (`fn add1(x) -> x + 1`)",
            "Smoke tests for the register allocator and prologue/epilogue codegen",
        ],
    ),
    (
        "bitwise",
        "Bitwise Operations",
        "Tests for bitwise operators (`&`, `|`, `^`, `<<`, `>>`) and rotation "
        "patterns on integer types. Includes both single-operator tests and "
        "compound expressions that combine shifts and masks - the patterns "
        "that lower to `AND`/`ORR`/`EOR`/`LSL`/`LSR`/`ROR` (or equivalent) on "
        "every backend.",
        [
            "AND, OR, XOR on u32 / u64",
            "Left / right shifts (`<<`, `>>`)",
            "Bit-rotation patterns (`rotr(x, n) = (x >> n) | (x << (32-n))`)",
            "Nybble / byte extraction with shift-and-mask",
        ],
    ),
    (
        "memory",
        "Memory Allocation, Load, and Store",
        "Tests exercising VUMA's `allocate` / `free` intrinsics and the `*ptr` "
        "load/store syntax. Ranges from the canonical 4-operation program "
        "(allocate / write / read / free) through arena-style region allocators. "
        "These are the core programs that exercise IVE's Liveness, "
        "Exclusivity, Origin, and Cleanup invariants.",
        [
            "Single-cell allocate / store / load / free",
            "Multi-buffer allocations with independent lifetimes",
            "Arena (bump-allocator) pattern with bulk invalidation",
            "Typed arena with nested scopes and O(1) reset",
        ],
    ),
    (
        "control_flow",
        "Control Flow: if/else, while, for, break, continue",
        "Tests for branching and looping constructs. Covers `if`/`else` "
        "branches, `while` loops with mid-loop condition updates, and `for i "
        "in 0..N` ranges. Includes nested conditionals and loops with "
        "early-exit / sentinel patterns.",
        [
            "for-range loops with accumulator update",
            "while loops with three-way branch (binary search)",
            "if/else if/else chains",
            "Loop termination via sentinel comparison",
        ],
    ),
    (
        "pointers",
        "Pointer Arithmetic and Dereference",
        "Tests focused on `Address`-typed values, pointer arithmetic "
        "(`base + offset`), and `*ptr` dereference. These programs stress the "
        "backend's address-computation instruction selection (LEA / ADD with "
        "shift, etc.) and the IVE bounds-check on derived pointers.",
        [
            "Byte-stride pointer arithmetic (`*(buf + i)`)",
            "Multi-byte buffer population via computed offsets",
            "Round-trip write-then-read through a pointer",
            "Pointer + length idioms used by hex dumpers and serializers",
        ],
    ),
    (
        "functions",
        "Function Calls, Recursion, and Parameters",
        "Tests exercising VUMA's calling convention: parameter passing, return "
        "values, recursion (with base cases), and the call/ret prologue/"
        "epilogue. Includes both leaf functions and recursive callers that "
        "stress the stack.",
        [
            "Leaf function calls with arithmetic return",
            "Recursive Fibonacci with two recursive calls per frame",
            "Recursive quicksort with partition helper",
            "Built-in runtime calls (`print_int`)",
        ],
    ),
    (
        "structs",
        "Struct and Enum (Tagged Union) Types",
        "Tests for `struct` definitions, struct-literal initialization "
        "(`Foo { a: 1, b: 2 }`), field access via `(*ptr).field`, and `enum` "
        "(tagged-union) types with `match` expressions. These exercise the "
        "struct layout / field-offset computation in codegen and the "
        "discriminant + payload memory model for enums.",
        [
            "Plain struct definitions and field access",
            "Struct literals with shorthand field init",
            "Enum (tagged union) with `Some` / `None`-style variants",
            "`match` expressions for pattern matching on enum tag",
        ],
    ),
    (
        "atomics",
        "Atomic Operations and Memory Ordering",
        "Tests for VUMA's atomic primitives: `atomic_load`, `atomic_store`, "
        "`atomic_cas`, `AtomicU64` with `fetch_add` / `fetch_sub` / "
        "`compare_exchange`, and the `Acquire` / `Release` / `Relaxed` "
        "orderings. These lower to `LDXR`/`STXR` (AArch64), `LOCK CMPXCHG` "
        "(x86_64), and equivalent atomic sequences on the other backends.",
        [
            "atomic_load / atomic_store with Acquire/Release",
            "atomic_cas (compare-and-swap) returning old value",
            "AtomicU64::fetch_add / fetch_sub",
            "compare_exchange with success/failure orderings",
            "Spinlock built on atomic_cas",
        ],
    ),
    (
        "u32_arith",
        "32-bit Arithmetic with Overflow Masking",
        "Tests for `u32` arithmetic that explicitly masks results with "
        "`& 4294967295` to defeat 64-bit host widening. This is the dominant "
        "pattern in SHA-256 / SHA256d code and any code that must preserve "
        "exact 32-bit wrap-around semantics on a 64-bit ISA.",
        [
            "u32 add / xor / and / rotate with `& 4294967295`",
            "u32 store/load via 4 individual byte stores (big-endian)",
            "W-schedule style u32 word copy through byte buffers",
            "Endianness helpers (`read_u32_be`)",
        ],
    ),
    (
        "edge_cases",
        "Boundary Conditions and Unusual Features",
        "Tests that exercise unusual corners of the language: hardware "
        "register access via `map_device`, floating-point type conversions "
        "(`f32`/`f64`, `inttofloat`, `floattoint`), and the `extern \"C\"` "
        "FFI block syntax. These probe edge cases of the codegen, linker, "
        "and IVE that the core categories don't reach.",
        [
            "Hardware register access via `map_device()` (embedded)",
            "f32 / f64 conversion intrinsics on all 8 backends",
            "`extern \"C\" { fn write(...); }` FFI block + relocations",
            "FP arithmetic and FP store/load",
        ],
    ),
    (
        "multi_function",
        "Many Functions Calling Each Other",
        "Tests with large function counts and dense call graphs. These stress "
        "the Static Call Graph (SCG) builder, the function-offset resolution "
        "pass (`resolve_call_relocs`), and the `BL`/`CALL` relocation range "
        "checks. Also useful for verifying DWARF subprogram DIE generation.",
        [
            "SHA256d with ~10 helper functions (rotr, ch, maj, sigma, ...)",
            "Multi-function program designed to exercise DWARF debug info",
            "Cross-function pointer passing and return",
        ],
    ),
    (
        "complex_stores",
        "Multi-byte Stores and Computed Addresses",
        "Tests whose primary characteristic is writing multi-byte values "
        "(u32, u64) as sequences of byte stores, or writing to addresses "
        "computed from complex expressions. These exercise the codegen's "
        "store-lowering paths and the IVE bounds analysis on non-trivial "
        "address expressions.",
        [
            "u32 written as 4 big-endian bytes (`write_u32` helper)",
            "6-bit-group extraction and table-driven byte output (Base64)",
            "Padding / alignment logic for variable-length output",
            "Computed destination addresses (`*(buf + i*stride + off)`)",
        ],
    ),
    (
        "nested_loops",
        "Two- and Three-Level Nested Loops",
        "Tests with deeply nested loops - the classic matrix-multiply shape "
        "(`for i { for j { for k { ... } } }`). These stress the backend's "
        "loop-label register allocation, branch-target encoding, and "
        "instruction-cache footprint.",
        [
            "4x4 matrix multiply (triple-nested while loops)",
            "Row-major 2D indexing via 1D memory",
            "Accumulator patterns across inner loops",
        ],
    ),
    (
        "linked_structures",
        "Linked Lists, Trees, and Ring Buffers",
        "Tests for pointer-linked data structures: singly- and doubly-linked "
        "lists, AVL-balanced trees with rotations, and any structure where "
        "nodes reference each other via `Address` fields. These are the "
        "showcase programs for VUMA's IVE - they require `unsafe` in Rust "
        "but verify cleanly in VUMA.",
        [
            "Singly-linked list with head-only prepend",
            "Doubly-linked list with sentinel (cyclic pointer updates)",
            "AVL tree with rotations (parent-pointer cycles)",
            "Iterative free walking the link chain",
        ],
    ),
    (
        "crypto_patterns",
        "Cryptographic Hash and Checksum Patterns",
        "Tests implementing cryptographic primitives: SHA-256 (full and "
        "single-round), SHA256d (double SHA-256), CRC32, and memory-mapped "
        "file hashing. These combine u32 arithmetic, multi-byte stores, and "
        "table-driven lookups - they are the most algorithmically dense "
        "programs in the suite.",
        [
            "Full SHA-256 (NIST FIPS 180-4) compression",
            "SHA256d = SHA-256(SHA-256(message))",
            "Single SHA-256 round with known test vectors",
            "CRC32 (IEEE 802.3, polynomial 0xEDB88320) with lookup table",
            "mmap + SHA256d over a memory-mapped file",
        ],
    ),
    (
        "concurrency",
        "Lock-Free Structures, Atomics, and Channels",
        "Tests for concurrent programming: lock-free queues, thread pools "
        "with mutex/condvar, MPSC channels, fork/exec pipelines, epoll "
        "servers, and signal handlers. These exercise VUMA's `spawn`/`join`, "
        "`Mutex`/`Condvar`, `Channel`, FFI to Linux syscalls, and IVE's "
        "concurrent-access verification.",
        [
            "SPSC lock-free ring buffer with AtomicU64 head/tail",
            "Thread pool with shared mutex-protected task queue",
            "MPSC channel with CAS-based slot claiming",
            "Fork/exec pipeline with pipes",
            "Epoll-based TCP echo server",
            "SIGALRM handler with atomic handoff to main thread",
        ],
    ),
]

CATEGORIZATION: dict[str, list[str]] = {
    "arithmetic":         ["minimal", "test_exit", "test_call"],
    "bitwise":            ["test_rotr"],
    "memory":             ["hello_memory", "test_alloc", "test_store",
                           "arena_allocator", "memory_arena"],
    "control_flow":       ["test_loop", "bsearch"],
    "pointers":           ["test_hex", "test_hex2", "hex_dump"],
    "functions":          ["fibonacci", "test_print", "test_print2", "quicksort"],
    "structs":            ["struct_demo", "enum_demo"],
    "atomics":            ["atomics_demo", "spinlock"],
    "u32_arith":          ["test_u32_arith", "test_u32_mem",
                           "test_w_sched", "test_endian"],
    "edge_cases":         ["gpio_blink", "float_math", "ffi_demo"],
    "multi_function":     ["sha256d", "debug_info"],
    "complex_stores":     ["test_sha_manual", "base64_encode"],
    "nested_loops":       ["matrix"],
    "linked_structures":  ["linked_list", "doubly_linked_list", "sorted_map"],
    "crypto_patterns":    ["test_sha_round", "mmap_sha256d", "crc32"],
    "concurrency":        ["lock_free_queue", "thread_pool", "channel_demo",
                           "pipeline", "epoll_echo", "self_exec",
                           "signal_hash"],
}


def main() -> None:
    # sanity: every example is assigned exactly once
    assigned: list[str] = []
    for files in CATEGORIZATION.values():
        assigned.extend(files)
    counts = Counter(assigned)
    dupes = [name for name, n in counts.items() if n > 1]
    if dupes:
        raise SystemExit(f"Duplicate assignments: {dupes}")

    available = {p.stem for p in EXAMPLES.glob("*.vuma")}
    missing_from_assignment = sorted(available - set(assigned))
    missing_from_examples = sorted(set(assigned) - available)
    if missing_from_assignment:
        raise SystemExit(f"Examples not assigned to any category: "
                         f"{missing_from_assignment}")
    if missing_from_examples:
        raise SystemExit(f"Assigned files missing from examples/: "
                         f"{missing_from_examples}")
    if len(assigned) != 47:
        raise SystemExit(f"Expected 47 examples, got {len(assigned)}")

    GOLD.mkdir(parents=True, exist_ok=True)

    # copy each example into its category
    for cat, files in CATEGORIZATION.items():
        cat_dir = GOLD / cat
        cat_dir.mkdir(parents=True, exist_ok=True)
        for stem in files:
            src = EXAMPLES / f"{stem}.vuma"
            dst = cat_dir / f"{stem}.vuma"
            shutil.copy2(src, dst)

    # write per-category README.md
    for cat, title, desc, features in CATEGORIES:
        files = CATEGORIZATION[cat]
        readme = GOLD / cat / "README.md"
        lines = [
            f"# {title}",
            "",
            desc,
            "",
            "## What belongs here",
            "",
        ]
        for f in features:
            lines.append(f"- {f}")
        lines += [
            "",
            f"## Files ({len(files)})",
            "",
        ]
        for stem in sorted(files):
            lines.append(f"- [`{stem}.vuma`]({stem}.vuma)")
        lines.append("")
        readme.write_text("\n".join(lines), encoding="utf-8")

    # write top-level manifest.json
    manifest = {
        "schema_version": 1,
        "suite": "vuma-gold-standard",
        "description": (
            "Gold-standard test programs for the VUMA compiler, organized by "
            "the language feature or codegen path they primarily exercise. "
            "Each .vuma file is a copy of an example from "
            "/tmp/my-project/examples/; originals are preserved there. New "
            "test programs added to a category should follow the conventions "
            "described in that category's README.md."
        ),
        "source_dir": "examples/",
        "total_programs": len(assigned),
        "categories": [],
    }
    for cat, title, desc, features in CATEGORIES:
        files = sorted(CATEGORIZATION[cat])
        manifest["categories"].append({
            "name": cat,
            "title": title,
            "description": desc,
            "features": features,
            "program_count": len(files),
            "programs": [f"{f}.vuma" for f in files],
        })
    (GOLD / "manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )

    # write top-level README.md
    top_readme = [
        "# VUMA Gold-Standard Test Suite",
        "",
        "A categorized collection of `.vuma` programs that serve as the gold "
        "standard for testing the VUMA compiler across all 8 backends "
        "(x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, "
        "Wasm32).",
        "",
        f"**Total programs:** {len(assigned)}  ",
        f"**Categories:** {len(CATEGORIES)}  ",
        f"**Source:** copies of `examples/*.vuma` (originals preserved)",
        "",
        "## Categories",
        "",
        "| # | Category | Programs | Title |",
        "|---|----------|----------|-------|",
    ]
    for i, (cat, title, _desc, _f) in enumerate(CATEGORIES, 1):
        n = len(CATEGORIZATION[cat])
        top_readme.append(f"| {i} | [`{cat}/`]({cat}/) | {n} | {title} |")
    top_readme += [
        "",
        "## Layout",
        "",
        "```",
        "tests/gold_standard/",
        "+-- manifest.json          # machine-readable index of all categories",
        "+-- README.md              # this file",
        "+-- build_categories.py    # script that (re)generated this tree",
        "+-- <category>/",
        "    +-- README.md          # what kinds of tests belong here",
        "    +-- *.vuma             # the test programs",
        "```",
        "",
        "## See also",
        "",
        "- [`manifest.json`](manifest.json) - full machine-readable index.",
        "- [`../../examples/README.md`](../../examples/README.md) - narrative "
        "descriptions of each example program.",
        "- Project worklog (`worklog.md`) - Task 1-c created this structure.",
        "",
    ]
    (GOLD / "README.md").write_text("\n".join(top_readme), encoding="utf-8")

    # also drop the build script into the gold_standard dir for reproducibility
    shutil.copy2(__file__, GOLD / "build_categories.py")

    print(f"OK: wrote {len(assigned)} programs across {len(CATEGORIES)} categories")
    for cat, _t, _d, _f in CATEGORIES:
        n = len(CATEGORIZATION[cat])
        print(f"  {cat:<20} {n:>2} files")


if __name__ == "__main__":
    main()
