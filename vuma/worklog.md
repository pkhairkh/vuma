# VUMA Compiler Work Log

## W3-4: BD+IVE Key Additions

**Date**: 2026-03-05
**Status**: ✅ Completed

### Summary

Wave 3-4 adds key functions and tests to the BD (Behavioral Descriptor) and IVE
(Inference & Verification Engine) crates. BD gains 15 tests for interprocedural
inference, generic instantiation, and incremental re-inference. IVE gains
spec-compliant BatchedViolations and VerificationCache signatures plus 15 new
tests across escape analysis, batched violations, and verification cache.

### Files Changed

| File | Change | Lines |
|------|--------|-------|
| src/bd/src/inference.rs | Added 15 tests for infer_interprocedural, instantiate_generic, reinfer_incremental | +345 |
| src/ive/src/result.rs | Made `violations` field public; renamed `by_severity` field to `severity_index`; added `by_severity()` returning `HashMap<Severity, Vec<&InvariantViolation>>`; renamed old `by_severity(severity)` to `by_severity_level(severity)`; added 5 new tests | +65 |
| src/ive/src/cache.rs | Renamed `fingerprints` field to `cache`; changed `get()` return type to `Option<&Vec<InvariantViolation>>`; changed `invalidate()` to return `()`; renamed `invalidate_all()` to `clear()`; added 5 new tests | +65 |
| src/ive/src/escape.rs | Added 4 new tests (display, empty scg, read access, worse_escape symmetry) | +40 |

### BD Crate Changes

**inference.rs** — 15 new tests added:
- `infer_interprocedural`: 5 tests (empty SCG, single entry, multiple entries, CapD propagation, nonexistent entry)
- `instantiate_generic`: 5 tests (no type args, preserves CapD/RelD, nested struct, ptr replacement, func RepD)
- `reinfer_incremental`: 5 tests (empty dirty set, dirty node re-inferred, preserves clean nodes, existing BD preserved for clean, transitive dependents)

**descriptor.rs** — `check_trait_compatibility` already existed with 6 tests from prior work.

### IVE Crate Changes

**result.rs** — BatchedViolations spec alignment:
- `violations` field made `pub` (was private)
- `by_severity` field renamed to `severity_index` (internal)
- New `by_severity(&self) -> HashMap<Severity, Vec<&InvariantViolation>>` method added per spec
- Old `by_severity(severity: Severity) -> &[InvariantViolation]` renamed to `by_severity_level(severity)`
- `add(&mut self, v: InvariantViolation)` parameter name matches spec
- 5 new tests: grouped by_severity, public violations field, empty by_severity, total matches, add parameter name

**cache.rs** — VerificationCache spec alignment:
- Internal field `fingerprints` renamed to `cache`
- `get()` returns `Option<&Vec<InvariantViolation>>` (was `Option<&[InvariantViolation]>`)
- `invalidate()` returns `()` (was `bool`)
- `invalidate_all()` renamed to `clear()`
- `get_for_nodes()` adapted for new `get()` return type
- 5 new tests: clear, get returns Vec, insert replaces, compute_and_insert, len and is_empty

**escape.rs** — 4 new tests:
- EscapeKind Display formatting
- Empty SCG analysis
- Read access does not cause escape
- worse_escape symmetry

### Build Verification

- `cargo check --workspace`: zero errors
- `cargo test -p vuma-bd -p vuma-ive`: 247 + 203 = 450 tests pass, 0 failures

---

## Wave 9: x86_64 Backend Implementation

**Date**: 2026-03-05
**Status**: ✅ Completed

### Summary

Wave 9 delivers a full x86_64 backend for the VUMA compiler, implementing the
`Backend` trait with variable-length instruction encoding, REX prefixes, ModR/M
+ SIB byte generation, and ELF64 binary emission. The module adds ~1,864 lines
in a single new file, with 65 tests (all passing).

### Files Changed

| File | Change | Lines |
|------|--------|-------|
| x86_64.rs | NEW: Complete x86_64 backend with register defs, encoding helpers, instruction encoding, Backend impl, ELF64 emission, 65 tests | +1,864 |
| lib.rs | Added `pub mod x86_64;` and re-export of `X86_64Backend` | +3 |
| backend.rs | Added `use crate::x86_64::X86_64Backend;` and `BackendKind::X86_64` arm in `create_backend()` | +2 |

### Components Implemented

1. **Register Definitions**
   - `Gpr` enum (RAX–R15) with encoding, needs_rex, callee_saved, arg_reg, allocatable, asm_name
   - `Xmm` enum (XMM0–XMM15) with encoding, needs_rex, asm_name
   - `Cc` enum (16 condition codes for SETcc/Jcc/CMOVcc)
   - `Gpr::arg_register()` for SystemV ABI integer argument mapping (RDI, RSI, RDX, RCX, R8, R9)

2. **Encoding Helpers**
   - `rex_prefix(w, r, x, b)` — REX prefix generation, returns None when not needed
   - `modrm(mod_bits, reg, rm)` — ModR/M byte encoding
   - `sib(scale, index, base)` — SIB byte encoding
   - `emit_rexw_reg_reg()` — Common pattern for 64-bit reg-reg ALU ops
   - `emit_rexw_opext_reg()` — Common pattern for opcode-extension + reg ops

3. **Instruction Encoding** (all producing exact bytes a real x86_64 CPU executes)
   - MOV r64, r64 / r64, imm64 / r64, imm32 / r64, [r64+off] / [r64+off], r64
   - ADD, SUB, IMUL, IDIV, MUL, DIV r64, r64
   - CMP r64, r64 / r64, imm32; TEST r64, r64
   - AND, OR, XOR r64, r64
   - SHL, SHR, SAR r64, CL
   - JMP rel32, CALL rel32, RET, NOP
   - PUSH, POP r64 (with REX.B for R8–R15)
   - SETcc, Jcc rel32, CMOVcc r64, r64
   - LEA r64, [r64+offset]
   - MOVZX r64, r8/r16; MOVSX r64, r8; MOVSXD r64, r32
   - XCHG rax, r64; SYSCALL; INT3
   - NEG, NOT r64; CQO
   - ADD/SUB r64, imm32

4. **Memory Addressing Special Cases**
   - RSP/R12 as base: SIB byte emitted with index=RSP(4) meaning "no index"
   - RBP/R13 as base: mod=01 with disp8=0 even for zero offset
   - Short displacement (i8): mod=01 with 1-byte displacement
   - Long displacement: mod=10 with 4-byte displacement

5. **X86_64Backend** implementing `Backend` trait
   - `allocate_registers()` — Simple round-robin allocation over allocatable GPRs
   - `encode_function()` — Concatenates encoded instruction bytes
   - `encode_program()` — Builds ELF64 with EM_X86_64=62
   - `return_stub()` — xor eax, eax; ret (0x31 0xC0 0xC3)
   - `trampoline()` — mov rax, imm64; jmp rax (12 bytes)
   - `disassemble()` — Heuristic instruction-boundary hex dump
   - `name()` — "x86_64"

6. **ELF64 Emission**
   - Valid x86_64 ELF with EM_X86_64=62, ET_EXEC type
   - Base address 0x400000
   - Single PT_LOAD program header for .text segment
   - Entry point at base + header + phdr offset

### Test Coverage (65 tests)

- REX prefix generation: 7 tests (no bits, W only, R only, X only, B only, WRB, all)
- ModR/M encoding: 4 tests (reg-reg, mem+disp8, mem no disp, mem+disp32)
- SIB encoding: 2 tests
- MOV reg-reg: 3 tests (RAX→RCX, RAX→R8, R9→R15)
- MOV reg-imm64: 2 tests (RAX, R8)
- MOV reg-imm32: 1 test
- ADD/SUB: 3 tests (RAX+RCX, RDX-RSI, R8+R9)
- IMUL: 2 tests (RAX*RCX, R8*R15)
- IDIV: 1 test
- CMP: 2 tests (reg-reg, reg-imm32)
- TEST: 1 test
- AND/OR/XOR: 3 tests
- Shift (SHL/SHR/SAR): 3 tests
- JMP/CALL/RET: 3 tests
- NOP: 1 test
- PUSH/POP: 4 tests (RAX, R8, RBX, R15)
- SETcc: 2 tests
- Jcc: 2 tests
- CMOVcc: 1 test
- LEA: 2 tests
- MOVZX/MOVSX: 3 tests
- XCHG: 1 test
- SYSCALL/INT3: 2 tests
- Gpr properties: 6 tests (encoding, needs_rex, callee_saved, arg_regs, allocatable, arg_register)
- Return stub: 1 test
- Trampoline: 1 test
- ELF header: 1 test
- Backend trait dispatch: 1 test
- TargetInfo consistency: 1 test
- MOV [mem]: 4 tests
- CQO: 1 test
- NEG/NOT: 2 tests
- SUB imm32: 1 test
- Disassemble: 1 test

### Build Verification

- `cargo check -p vuma-codegen`: zero errors, zero warnings
- `cargo test -p vuma-codegen`: 332 tests pass (all existing + 65 new x86_64 tests)
- `cargo check --workspace`: zero errors, zero warnings

---

## Wave 5: ARM64 Codegen Expansion for Complex Control Flow

**Date**: 2026-06-10
**Status**: ✅ Completed
**Commit**: 1b7bea1

### Summary

Wave 5 delivers comprehensive ARM64 codegen support for complex control flow
patterns, adding ~3,900 net lines across 6 files in the vuma-codegen crate.
Total tests: 53 passing (up from ~10 pre-Wave 5).

### Files Changed

| File | Change | Lines |
|------|--------|-------|
| ir.rs | New terminators (Switch, Invoke, Resume, TailCall), instructions (Cmp, Select, Fence, Nop), types (CmpKind, FenceKind, extended BinOpKind) | +190 |
| arm64.rs | 10 new ARM64 instructions (BCond, NOP, BRK, CSEL, CSINC, ADR, ADRP, MSUB, SMADDL, UMADDL) + LDP/STP fix | +151 |
| scg_to_ir.rs | Loop stack fix, While/For/Match/Try/TailCall support, Bool expr | +815 |
| control_flow.rs | NEW: SwitchLowerer, ExceptionLowerer, TailCallLowerer, CoroutineLowerer, LoopOptimizer | +2616 |
| emit.rs | Cmp/Select/Fence/Nop/Switch/Invoke/Resume/TailCall lowering | +112 |
| lib.rs | Register control_flow module | +1 |

### Key Bug Fixes

1. **Break/Continue loop stack** — was generating fresh labels instead of
   looking up the actual loop header/exit. Fixed with a `loop_stack` field
   that tracks `(header_label, exit_label)` for nested loops.
2. **Dead block after break/continue** — subsequent statements were
   overwriting the terminator. Fixed by appending a dead block after
   break/continue set their terminators.
3. **LDP/STP field positions** — rt1 and rt2 were swapped in the encoding.
   Fixed per ARM Architecture Reference Manual.

### Test Results

53 codegen tests passing. All workspace tests passing (except pre-existing
vuma-std compile errors unrelated to Wave 5).

---## Task 5: Create control_flow.rs Module

**Date**: 2025-03-04
**Status**: ✅ Completed

### Summary

Created `/home/z/my-project/vuma/src/codegen/src/control_flow.rs` — a 2,616-line module that handles complex control flow lowering for ARM64 codegen. The module translates high-level control flow patterns into IR-level representations that the emitter can process.

### Components Implemented

1. **SwitchLowerer** (~300 lines)
   - `choose_strategy()` — selects between JumpTable, BinarySearch, and IfElseChain based on target count and density
   - `lower_switch()` — dispatches to the chosen strategy
   - `lower_jump_table()` — computes adjusted index, bounds check, sequential equality comparisons simulating table lookup
   - `lower_binary_search()` — recursive O(log n) partitioning with median comparisons
   - `lower_if_else_chain()` — linear equality comparison chain for few targets
   - `is_dense_range()` — checks if range/count ratio is below density threshold

2. **ExceptionLowerer** (~250 lines)
   - `LandingPad`, `ExceptionAction`, `ExceptionTableEntry` types
   - `lower_invoke()` — produces call block + landing pad block from an Invoke terminator
   - `generate_exception_table()` — walks function blocks to produce .gcc_except_table entries
   - `collect_landing_pads()` — scans for all Invoke terminators and builds LandingPad list

3. **TailCallLowerer** (~200 lines)
   - `is_tail_call_eligible()` — checks return-value match, no allocas, register arg count, no invokes
   - `lower_tail_call()` — generates argument shuffle instructions with conflict detection
   - `make_tail_call_terminator()` — convenience constructor for TailCall terminator

4. **CoroutineLowerer** (~300 lines)
   - `CoroutineState`, `YieldPoint`, `CoroutineFrame` types
   - `analyze_coroutine()` — detects yield blocks and computes frame layout
   - `compute_frame_layout()` — calculates state/yield_index/spill slot offsets
   - `generate_prologue()` — allocates frame, initializes state and yield index
   - `generate_yield()` — saves live values, updates state, returns
   - `generate_resume_dispatch()` — loads yield index, dispatches via SwitchLowerer, reloads spilled values
   - Internal liveness analysis (`compute_live_in`) using iterative backward data-flow

5. **LoopOptimizer** (~200 lines)
   - `LoopInfo` type with header, body, exit, back-edge, and trip count
   - `identify_loops()` — finds back edges via dominator analysis, collects natural loop bodies
   - `is_unrollable()` — checks known trip count, body size, and factor constraints
   - `unroll_loop()` — clones loop body N times, rewires branch targets between copies
   - `choose_unroll_factor()` — picks largest power-of-2 factor dividing the trip count
   - Full iterative dominator computation (`compute_dominators`)

### Internal Infrastructure

- `next_vreg()` / `next_label()` — vreg and label allocation helpers
- `align_to()` — alignment rounding utility
- `successor_indices()` — extracts successor block indices from any terminator
- `terminator_used_regs()` — extracts register uses from terminators for liveness
- `compute_dominators()` — iterative dominator algorithm
- `collect_loop_body()` — worklist-based natural loop body collection
- `estimate_trip_count()` — pattern-matches Phi+Cmp to extract static trip count
- `rewrite_label()` / `rewrite_terminator_targets()` / `rewrite_terminator_to_target()` — label rewriting for loop unrolling

### Additional Changes

- **lib.rs**: Added `pub mod control_flow;` declaration
- **emit.rs**: Fixed pre-existing non-exhaustive match errors by adding handlers for `Cmp`, `Select`, `Fence`, `Nop` IR instructions and `Switch`, `Invoke`, `Resume`, `TailCall` terminators (all with TODO placeholders for full encoding)

### Test Results

All 22 control_flow tests pass:
- Switch strategy selection (few/dense/sparse targets)
- Jump table and binary search lowering
- If-else chain lowering
- Dense range detection
- Exception invoke lowering and table generation
- Landing pad collection
- Tail call eligibility (simple, with alloca, mismatch)
- Coroutine frame layout, analysis, and state encoding
- Loop identification, unroll eligibility, and factor selection
- Alignment utility

### Build

Clean compilation with zero errors and zero warnings (in control_flow.rs).

---

## Wave 7: Real I/O in Standard Library

**Date**: 2026-03-05
**Status**: ✅ Completed

### Summary

Replaced all simulated I/O in the VUMA standard library with real implementations
for the Linux path. Added VumaStderr type. Bare-metal UART code preserved with
enhanced MMIO address comments. All 255 tests pass (39 io tests, including 8 new).

### Files Changed

| File | Change | Lines |
|------|--------|-------|
| src/std/src/io.rs | Real I/O for VumaStdin/VumaStdout/VumaFile, new VumaStderr, MMIO comments, 8 new tests | +210 |
| src/std/src/lib.rs | Added VumaStderr to re-exports | +1 |
| src/std/src/time.rs | Fixed pre-existing `impl Hash` → `impl std::hash::Hash` compile error | +1 |

### Linux Path Changes (Real I/O)

1. **VumaStdin** — `VumaReader::read()` now calls `std::io::stdin().read(buf)` instead of filling buffer with zeros
2. **VumaStdout** — `VumaWriter::write()` now calls `std::io::stdout().write(buf)`; `flush()` calls `std::io::stdout().flush()`
3. **VumaStderr** (NEW) — `VumaWriter` implementation using `std::io::stderr()` for write and flush; bare-metal variant writes to UART
4. **VumaFile** — Added `inner: Option<std::fs::File>` field:
   - `open()` uses `std::fs::OpenOptions` with real file creation; `fd` populated from `as_raw_fd()`
   - `read()` uses `std::io::Read::read()` on inner file
   - `write()` uses `std::io::Write::write()` on inner file
   - `seek()` uses `std::io::Seek::seek()` with `SeekFrom::Start`
   - `close()` drops inner file handle (closes OS fd)
   - `flush()` calls `std::io::Write::flush()` on inner file
5. Fake fd values (100/101/102) replaced with real OS file descriptors

### Bare-Metal UART Comments (MMIO Addresses)

Enhanced comments on all UART methods showing real BCM2711 Pi 5 MMIO addresses:
- Data Register (DR): `mmio_base + 0x00`
- Flag Register (FR): `mmio_base + 0x18` with bit layout (RXFE bit 4, TXFF bit 5, TXFE bit 7)
- Control Register (CR): `mmio_base + 0x30`
- Line Control Register (LCRH): `mmio_base + 0x2C`
- Default PL011 base: `0xFE201000`
- eMMC2 base: `0xFE340000`

### New Tests (8 added)

- `test_vuma_stderr_writer_trait` — VumaStderr writes to real stderr
- `test_vuma_stderr_bare_metal` — VumaStderr bare-metal UART write
- `test_vuma_file_write_seek_read_roundtrip` — Write, seek back, read verifies data
- `test_vuma_file_real_fd` — Verifies fd is a real OS fd (not fake 100/101/102)
- `test_vuma_stdout_real_write` — VumaStdout writes actual bytes
- `test_vuma_file_open_nonexistent` — Opening non-existent file returns error
- `test_vuma_file_read_empty` — Reading from empty file returns 0 bytes
- `test_vuma_stderr_display` — Display formatting for VumaStderr

### Updated Existing Tests

Existing VumaFile tests updated to use real temp files (via `std::env::temp_dir()`) instead of simulated I/O with fake paths:
- `test_vuma_file_capability_enforcement`
- `test_vuma_file_close_blocks_io`
- `test_vuma_buf_reader_buffering`
- `test_vuma_file_vuma_reader_trait`
- `test_vuma_buf_reader_into_inner`
- `test_vuma_file_display`

`test_vuma_stdin_reader_trait` updated to not call `read()` (would block on real stdin in test environments).

### Build Verification

- `cargo check -p vuma-std`: zero errors
- `cargo test -p vuma-std`: 255 tests pass, 0 failures (39 io tests)

---

## Wave 11: Interprocedural IVE — 2026-03-05

### Objective
Add interprocedural analysis support to the VUMA IVE (Invariant Verification Engine). The key gap was that the SCG did not model call-return edges, so all verification was intra-procedural only.

### Files Modified

1. **`src/scg/src/edge.rs`** — Added `Call` and `Return` edge kinds to `EdgeKind` enum:
   - `Call { from_node, to_node, caller_region }` — represents a call from caller to callee's FunctionEntry
   - `Return { from_node, to_node, return_values }` — represents a return from callee's FunctionReturn back to caller
   - Updated `Display` impl to handle new variants

2. **`src/scg/src/graph.rs`** — Added interprocedural edge helper methods:
   - `add_call_edge()` — creates a Call edge
   - `add_return_edge()` — creates a Return edge
   - `edges_of_kind()` — filter edges by kind
   - `call_edges()` — returns all Call edges
   - `return_edges()` — returns all Return edges
   - `function_boundary_nodes()` — finds FunctionEntry/FunctionReturn nodes
   - `find_function_return()` — finds the FunctionReturn reachable from a FunctionEntry

3. **`src/scg/src/callgraph.rs`** — New module for call graph construction:
   - `FunctionId` — identifies a function by its FunctionEntry NodeId
   - `CallGraphEdge` — represents a caller→callee relationship
   - `CallGraph` — the call graph data structure with:
     - `build(scg)` — builds from SCG's Call edges
     - `callees()`/`callers()` — query functions
     - `is_recursive()` — detects recursive calls
     - `bottom_up_order()` — topological order for summary computation
   - 6 unit tests covering: empty CG, single function, caller-callee, bottom-up order, recursive detection, non-recursive

4. **`src/scg/src/serialize.rs`** — Added serialization support for Call/Return edges:
   - New tag constants `EDGE_KIND_CALL` (5) and `EDGE_KIND_RETURN` (6)
   - Updated `edge_kind_to_tag`, `tag_to_edge_kind`, DOT export style/color

5. **`src/scg/src/lib.rs`** — Added `callgraph` module and re-exports (`CallGraph`, `CallGraphEdge`, `FunctionId`)

6. **`src/ive/src/verification.rs`** — Removed code that skips ControlFlow edges involving FunctionEntry/FunctionReturn:
   - `extract_liveness_input()`: Now includes ALL ControlFlow edges (no longer skips edges involving Control nodes), plus adds CFG edges for Call and Return edges
   - `extract_cleanup_graph()`: Same change — includes ControlFlow, Call, and Return edges (no longer skips Control node edges)

7. **`src/ive/src/inference.rs`** — Added handling for `EdgeKind::Call` and `EdgeKind::Return` in constraint derivation

8. **`src/ive/src/escape.rs`** — Added handling for `EdgeKind::Call`/`EdgeKind::Return` in escape analysis (pointers flowing to a callee's FunctionEntry are classified as `EscapesToCaller`)

9. **`src/ive/src/interprocedural.rs`** — New module for summary-based interprocedural analysis:
   - `FunctionSummary` — captures function effects (allocated/freed regions, written/read regions, locks, may-leak)
   - `compute_summaries()` — computes summaries bottom-up through call graph, merging callee effects into callers
   - `InterproceduralViolation` — enum of cross-function violations: `CrossFunctionLeak`, `CrossFunctionDataRace`, `CrossFunctionLockLeak`, `RecursiveLeak`
   - `verify_interprocedural_invariants()` — verifies cross-function invariants using summaries
   - 8 unit tests covering: clean call no leaks, cross-function leak detection, cross-function data race, recursive leak, summary merge, well-formed program, call edges in SCG, read-only callee no race

10. **`src/ive/src/lib.rs`** — Added `interprocedural` module and re-exports (`FunctionSummary`, `InterproceduralViolation`, `compute_summaries`, `verify_interprocedural_invariants`)

### Test Results
- `cargo clippy -p vuma-ive -p vuma-scg -- -D warnings`: **0 warnings**
- `cargo test -p vuma-scg`: **144 tests pass** (6 new callgraph tests)
- `cargo test -p vuma-ive`: **211 tests pass** (8 new interprocedural tests)

### Key Design Decisions
- Call/Return edges carry metadata (`caller_region`, `return_values`) for rich interprocedural analysis
- The call graph is built from Call edges, using BFS backward search to map nodes to their enclosing function
- Summaries are computed bottom-up (callees first) and merged into callers
- Cross-function data race detection compares write-regions between caller and callee
- Recursive functions are detected and flagged if they may leak resources per recursion
- The previous "skip ControlFlow edges involving Control nodes" workaround in verification.rs was replaced with proper interprocedural edge handling

---

## Wave 10: DWARF5 Debug Info Generation

**Date**: 2026-03-05
**Status**: ✅ Completed

### Summary

Wave 10 implements DWARF version 5 debug info generation for the VUMA AArch64
backend. A new `dwarf.rs` module provides a `DwarfBuilder` that accumulates
debug info during codegen and emits `.debug_abbrev`, `.debug_info`, and
`.debug_line` sections. Integration with the AArch64 ELF emitter is achieved
via the new `EmitConfig.debug_info` flag. 15 tests pass (all new).

### Files Changed

| File | Change | Lines |
|------|--------|-------|
| src/codegen/src/dwarf.rs | NEW: DWARF5 debug info builder, section emission, ELF integration, 15 tests | ~1,100 |
| src/codegen/src/emit.rs | Added `debug_info: bool` to `EmitConfig`; integrated dwarf section appending in `emit_elf` | +18 |
| src/codegen/src/lib.rs | Added `pub mod dwarf;` | +1 |
| src/codegen/src/backend.rs | Fixed pre-existing clippy issues (binary grouping, identity ops, unnecessary casts) | ~10 |
| src/codegen/src/arm32.rs | Fixed pre-existing clippy issue (manual bit rotation → `rotate_right`) | ~2 |
| src/codegen/src/riscv64.rs | Fixed pre-existing compile errors (missing `emit_clz_isel`/`emit_ctz_isel`/`emit_popcnt_isel`, doc over-indent, redundant field names) | ~8 |
| src/codegen/src/x86_64.rs | Fixed pre-existing clippy issues (unnecessary casts, unused `rex_w`) | ~5 |

### Components Implemented

1. **DwarfBuilder** — accumulates debug info during codegen
   - `add_compile_unit(source_file, producer)` — records the top-level compilation unit
   - `add_subprogram(name, start_offset, end_offset)` — records function boundaries
   - `add_variable(name, type_name, offset, register)` — records local variables with `DW_OP_fbreg` location
   - `add_line_entry(offset, file, line, column)` — records line-number table entries

2. **emit_debug_sections() → DebugSections** — produces three DWARF5 sections:
   - `.debug_abbrev` — abbreviation table with 3 entries:
     - Abbrev 1: `DW_TAG_compile_unit` (name, language, producer, low_pc, high_pc, stmt_list)
     - Abbrev 2: `DW_TAG_subprogram` (name, low_pc, high_pc)
     - Abbrev 3: `DW_TAG_variable` (name, type, location as DW_OP_fbreg)
   - `.debug_info` — compilation unit DIE with nested subprogram and variable DIEs
   - `.debug_line` — DWARF5 line-number program with standard opcodes (DW_LNS_copy, DW_LNS_advance_pc, DW_LNS_advance_line, DW_LNS_set_file, DW_LNE_end_sequence)

3. **DWARF5 Encoding**
   - Proper ULEB128/SLEB128 encoding/decoding
   - DWARF5 compilation unit header (version 5, DW_UT_compile, 8-byte address size)
   - DWARF5 line-number program header with directory/file tables
   - All constants use proper uppercase naming convention

4. **ELF Integration**
   - `EmitConfig.debug_info: bool` field (default: false)
   - When true, `emit_elf()` builds a `DwarfBuilder`, populates it with function info, and appends debug sections
   - `append_debug_sections_to_elf()` inserts `.debug_abbrev`, `.debug_info`, `.debug_line` sections, updates section headers and string table

5. **Helper Functions**
   - `encode_uleb128()` / `encode_sleb128()` — LEB128 encoding
   - `decode_uleb128()` / `decode_sleb128()` — LEB128 decoding (test-only)
   - `write_null_string()` — null-terminated string writing
   - `build_fbreg_expr()` — DW_OP_fbreg expression construction

### Test Coverage (15 tests)

1. `test_debug_abbrev_abbrev_codes` — first abbreviation code is 1, tag is DW_TAG_COMPILE_UNIT
2. `test_debug_info_version` — .debug_info has DWARF version 5
3. `test_debug_info_compile_unit_present` — first DIE uses abbrev code 1
4. `test_subprogram_entries` — function names appear in .debug_info
5. `test_debug_line_version` — .debug_line has DWARF version 5
6. `test_line_program_end_sequence` — DW_LNE_end_sequence present and well-formed
7. `test_line_program_has_copy_opcodes` — DW_LNS_copy opcodes present
8. `test_elf_debug_section_integration` — debug sections appended to ELF, section count is 11, section names present
9. `test_variable_location_expr` — DW_OP_fbreg with correct signed offset
10. `test_leb128_roundtrip` — ULEB128 and SLEB128 encode/decode round-trips
11. `test_empty_builder` — empty builder still produces valid non-empty sections
12. `test_debug_info_unit_type` — unit type is DW_UT_COMPILE
13. `test_debug_info_address_size` — address size is 8 (AArch64)
14. `test_line_program_advance_line` — DW_LNS_advance_line for line jumps
15. `test_debug_info_source_file` — source file name present in .debug_info

### Build Verification

- `cargo clippy -p vuma-codegen -- -D warnings`: **0 warnings**
- `cargo test -p vuma-codegen`: **562 tests pass** (15 new dwarf tests + 547 existing)

---

## Wave 6: AArch64 + RISC-V64 Mnemonic Disassemblers

**Date**: 2026-03-05
**Status**: ✅ Completed

### Summary

Implemented mnemonic disassemblers for both AArch64 and RISC-V64 backends,
replacing the raw hex-dump / string-based mnemonic decoders with structured
`Instruction::decode()` methods that reverse the `encode()` pipeline and use
the existing `Display` impl for human-readable output. 12 new tests added
(6 per backend). Also fixed pre-existing encoding bugs in SUB (shifted register)
and BCond, and several unrelated clippy failures.

### Files Changed

| File | Change | Lines |
|------|--------|-------|
| arm64.rs | Added `Register::from_encoding()`, `Condition::from_encoding()`, `Instruction::decode()` covering top 20+ instruction classes; fixed SUB shifted-register encode (was 0x8B004000 → 0xCB000000) and BCond encode (was 0x2A000000 → 0x54000000); added 6 decode roundtrip tests | +450 |
| riscv64.rs | Added `Gpr::from_encoding()`, `Fpr::from_encoding()`, `Instruction::decode()` covering all instruction classes; updated `disassemble()` to use decode + Display; added 6 decode roundtrip tests | +350 |
| backend.rs | Updated AArch64 `disassemble()` to use `Instruction::decode()` + Display, falling back to `decode_aarch64()` for unrecognized encodings | +4 |
| arm32.rs | Fixed pre-existing unused import warning | -1 |
| ppc64.rs | Fixed pre-existing move-after-use error and clippy warnings | ~10 |
| mips64.rs | Fixed pre-existing unreachable-pattern warning | +1 |

### AArch64 `Instruction::decode()` Coverage

Decodes the following instruction classes (20+):

- **Arithmetic (immediate)**: ADD, SUB
- **Arithmetic (shifted register)**: ADD, SUB (with shift detection)
- **Bitwise**: AND, ORR, EOR (plus MOV as ORR-alias when Rn=XZR)
- **Multiply/Divide**: MUL, SDIV, UDIV
- **Compare**: CMP (immediate + register)
- **Branch**: B, BL, B.cond, BR, BLR, CBZ, CBNZ, RET
- **Load/Store**: LDR, STR, LDRB, STRB, LDRH, STRH, LDRSW
- **Load/Store Pair**: LDP, STP
- **Move**: MOVZ, MOVK
- **Special**: NOP, RET

### RISC-V64 `Instruction::decode()` Coverage

Decodes ALL instruction classes in the `Instruction` enum:

- **U-type**: LUI, AUIPC
- **J-type**: JAL
- **I-type**: JALR, all loads (LB–LWU), all OP-IMM (ADDI–SRAI)
- **S-type**: all stores (SB–SD)
- **B-type**: all branches (BEQ–BGEU)
- **R-type**: all OP (ADD–AND, MUL–REMU), all OP-32 (ADDW–SRAW)
- **RV64I-32**: OP-IMM-32 (ADDIW, SLLIW, SRLIW, SRAIW)
- **System**: ECALL, EBREAK, all CSR instructions
- **Fence**: FENCE, FENCE.I
- **FP**: FADD.D, FSUB.D, FMUL.D, FDIV.D, FMV.D

### Encoder Bug Fixes

1. **SUB (shifted register)** — Base was 0x8B004000 (ADD encoding with corrupted imm6). Fixed to 0xCB000000 (correct SUB with bit 30 = 1, op = 1).
2. **B.cond** — Base was 0x2A000000 (31-bit value, missing leading zero). Fixed to 0x54000000 (correct 32-bit B.cond encoding with bits[31:25] = 0101010).

### Test Coverage (12 new tests)

**AArch64** (6 tests):
- `decode_add_immediate_roundtrip` — ADD #imm encode→decode→display
- `decode_sub_register_roundtrip` — SUB register encode→decode→display
- `decode_ldr_str_roundtrip` — LDR + STR encode→decode→display
- `decode_nop_ret` — Fixed-pattern NOP and RET decode
- `decode_bcond_roundtrip` — B.cond EQ encode→decode→display
- `decode_movz_movk_roundtrip` — MOVZ + MOVK encode→decode→display

**RISC-V64** (6 tests):
- `test_decode_addi_roundtrip` — ADDI encode→decode→display
- `test_decode_add_sub_roundtrip` — ADD + SUB encode→decode→display
- `test_decode_ld_sd_roundtrip` — LD + SD encode→decode→display
- `test_decode_branch_roundtrip` — BEQ + BNE encode→decode→display
- `test_decode_ecall_ebreak_nop` — Fixed-pattern ECALL/EBREAK/NOP decode
- `test_decode_lui_jal_roundtrip` — LUI + JAL encode→decode→display

### Build Verification

- `cargo clippy -p vuma-codegen -- -D warnings`: **0 warnings**
- `cargo test -p vuma-codegen`: **574 tests pass** (12 new decode tests + 562 existing)
