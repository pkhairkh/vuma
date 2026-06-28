# VUMA Changelog

All notable changes to the VUMA project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.1.0-alpha.1] — 2026-03-06

Pre-release alpha build incorporating Waves 1–5 of critical bug fixes, atomics/ABI support, FP conversion casts, infrastructure/stdlib expansion, and test hardening across all 10 backends.

---

### Bug Fixes

#### Wave 1 — Critical Bug Fixes

- **ARM64 ROR/ROL**: Rotation instructions now correctly emit `EXTR`/`RORV` instead of erroneously emitting `ASR` (arithmetic shift right). *(W1)*
- **Documentation cleanup**: Removed all Pi5 / Raspberry Pi references from documentation and code comments. *(W1)*
- **Development debris**: Deleted stale `.bak` files, `agent-ctx/` directory, and orphaned work logs from the repository. *(W1)*
- **`.gitignore`**: Added entries for development artifacts (`.bak`, `agent-ctx/`, work logs) to prevent future contamination. *(W1)*

#### Wave 2 — Atomics & ABI Fixes

- **LoongArch64 atomics**: All atomic operations now properly emit `LL.D`/`SC.D`/`AMSWAP.D` sequences with `DBAR` barriers for correct load-link/store-conditional semantics. *(W2)*
- **PPC64 atomics**: All atomic operations now properly emit `LDARX`/`STDCX` loops with `SYNC`/`LWSYNC` barriers (13 new instructions). *(W2)*
- **RISC-V 64 atomics**: Fixed `AtomicCAS` loop with missing label insertions that caused incorrect branch targets under certain code layouts. *(W2)*
- **Wasm32 atomics**: Proper `i32.atomic.rmw.cmpxchg` / `i64.atomic.rmw.cmpxchg` for compare-and-swap (24 new atomic instructions). *(W2)*
- **ARM32 atomics**: `AtomicCAS` now emits correct `LDREX`/`STREX`/`DMB` sequences (7 new instructions). *(W2)*
- **MIPS64 atomics**: `AtomicCAS` now emits `LLD`/`SCD`/`SYNC` sequences (5 new instructions). *(W2)*
- **ARM32 ABI**: Proper >4 argument passing via the stack, compliant with the AAPCS calling convention. *(W2)*
- **MIPS64 rotations**: Complete `ROR`/`ROL` rotation sequences using `ROTR` instruction. *(W2)*
- **LoongArch64 terminators**: Fixed `Switch`/`Invoke`/`TailCall`/`Resume` terminator lowering to correctly emit branch and call sequences. *(W2)*

#### Wave 3 — FP Conversion Casts

- **`IRInstr::Cast`**: Added `from_ty`/`to_ty` fields for type-aware floating-point conversion, replacing the previous type-agnostic single-operand cast. *(W3)*
- **x86_64**: Fixed FP conversions to emit proper `CVTSI2SS`/`CVTSI2SD`/`CVTSS2SI`/`CVTSD2SI` instructions, with u64→f64 halving for unsigned 64-bit to double conversion. *(W3)*
- **ARM32**: Fixed `VCVT` encoding, added unsigned and double-precision variants for correct f32↔i32, f64↔i32, f64↔i64 conversions. *(W3)*
- **RISC-V 64**: `FCVT` instructions now properly dispatch signed vs. unsigned rounding mode based on `from_ty`/`to_ty`. *(W3)*
- **MIPS64**: Correct `CVT` instructions with `MTC1`/`DMTC1` bridge for GPR→FPR moves before conversion. *(W3)*
- **PPC64**: Proper `FCFID`/`FCFIDU`/`FCTIDZ`/`FRSP` bridge instructions for signed/unsigned integer↔FP conversions. *(W3)*
- **LoongArch64**: `FFINT`/`FTINT` conversions with fixed `FfintDW` opcode encoding. *(W3)*
- **Wasm32**: Fixed 8 swapped opcodes (`i32.trunc_f32_s`↔`i32.trunc_f64_s`, etc.) and proper type inference for conversion instructions. *(W3)*

#### Wave 4 — Codegen Fixes

- **ARM64 stack slots**: Fixed spurious NOP emissions for `CtSelect`/`CtEq` and atomic operations that left unnecessary `NOP` instructions in the output. *(W4)*

---

### New Features

#### Atomics (Wave 2)

- **PPC64**: 13 new atomic instructions — `LDARX`, `STDCX`, `SYNC`, `LWSYNC`, `ISYNC`, `ADD_LDARX_STDCX`, `SUB_LDARX_STDCX`, `AND_LDARX_STDCX`, `OR_LDARX_STDCX`, `XOR_LDARX_STDCX`, `NAND_LDARX_STDCX`, `SWAP_LDARX_STDCX`, `CAS_LDARX_STDCX`. *(W2)*
- **Wasm32**: 24 new atomic instructions — `i32.atomic.rmw.cmpxchg`, `i64.atomic.rmw.cmpxchg`, plus full `i32`/`i64` atomic rmw set (add, sub, and, or, xor, xchg). *(W2)*
- **ARM32**: 7 new instructions — `LDREX`, `STREX`, `DMB`, `DSB`, `ISB`, `CLREX`, and `AtomicCAS` composite. *(W2)*
- **MIPS64**: 5 new instructions — `LLD`, `SCD`, `SYNC`, `SYNC_MB`, and `AtomicCAS` composite. *(W2)*
- **LoongArch64**: `LL.D`, `SC.D`, `AMSWAP.D`, `DBAR` for all atomic operations. *(W2)*

#### FP Conversion Casts (Wave 3)

- **`IRInstr::Cast`**: Extended with `from_ty`/`to_ty` enabling type-aware conversions across `i32`↔`f32`, `i32`↔`f64`, `i64`↔`f32`, `i64`↔`f64`, `f32`↔`f64` in both signed and unsigned variants. *(W3)*
- All 10 backends now emit architecturally correct FP conversion instructions instead of generic or wrong encodings. *(W3)*

#### Standard Library (Wave 4)

- **`math.rs`**: Expanded from 4 to 92 public items — comprehensive math primitives including trigonometric approximations, bit manipulation, integer overflow helpers, and saturating arithmetic. *(W4)*
- **`fmt.rs`**: New formatting module with 13 functions — integer formatting (decimal, hex, binary, octal), float formatting, buffer utilities. *(W4)*

#### Debug Info (Wave 4)

- **DWARF debug info generation**: Full `.debug_info`, `.debug_abbrev`, `.debug_line`, `.debug_frame` sections with per-backend address sizes, alignment factors, and CIE presets for all 10 architectures. *(W4)*

#### FFI (Wave 4)

- **C FFI end-to-end wiring**: Complete `extern "C"` block support from parser through SCG to ELF relocation emission, with `ffi_demo.vuma` example program. *(W4)*

---

### Infrastructure

- **AArch64**: Refactored `select_cast` for type-aware FP conversions, eliminating duplicate code paths and ensuring correct signed/unsigned dispatch. *(W4)*
- **E037 diagnostic**: All backends now emit a structured `E037` diagnostic when encountering unresolved relocations, replacing silent failures. *(W4)*
- **GitHub Actions CI/CD**: 5-job workflow (test, lint, cross-compile matrix, release build, publish) plus automated release workflow for tagged commits. *(W4)*
- **`.gitignore`**: Entries for `.bak` files, `agent-ctx/`, and development work logs to prevent artifact commits. *(W1)*
- **Repository cleanup**: Removed all `.bak` files, `agent-ctx/` directory, and stale development work logs. *(W1)*

---

### Tests

#### Wave 5 — Test Infrastructure

- **25 SHA256d backend validation tests** — End-to-end SHA256d execution correctness across all 10 backends. *(W5)*
- **13 dedicated regression tests** — Targeted tests for each fix from Waves 1–4 (ARM64 rotation, atomics per-arch, FP conversions per-arch, ABI compliance, stack-slot NOP). *(W5)*
- **74 diagnostics integration tests** — Full coverage of diagnostic emission, error chaining, suggestion applicability, and all 4 output formats. *(W5)*
- **29 DWARF/FFI integration tests** — CIE presets per-architecture, debug section validation, ELF relocation verification, extern block parsing, FFI pipeline tests. *(W5)*
- **21 expanded ABI conformance tests** — Calling convention compliance for argument passing, return values, and callee-saved register preservation across all 10 backends. *(W5)*
- **19 property-based tests** — Proptest-driven fuzzing for parser roundtrip, SCG invariants, codegen correctness, and cross-backend consistency. *(W5)*
- **55 math test functions** — Comprehensive coverage of the expanded `math.rs` module (92 public items). *(W5)*
- **30 fmt test functions** — Full coverage of the new `fmt.rs` module (13 functions). *(W5)*

**Total new/expanded tests**: ~266 tests across 8 categories.

---

## [0.1.0-alpha.0] — 2026-03-05

> **Note**: This is a pre-release (alpha) version. The API is not yet stabilized and may change before v0.1.0.

---

### Breaking Changes

- **W1**: Removed Pi5 bare-metal platform crate (`src/pi5/`) and all Pi5-specific code from workspace, pipeline, tests, COR, security, IO, and documentation. `CompileTarget::Pi5Bare`/`Pi5Linux` replaced with `CompileTarget::Linux`. `TestCategory::Pi5` removed from test framework. *(W1)*
- **W6**: ELF emission restructured to 3 LOAD segments (R / RX / RW) for W^X compliance. `.rodata` placed before `.text` in memory. *(W6-f, W21)*
- **W24**: `IRInstr::Call` now carries `is_extern: bool` field — all `IRInstr::Call` construction sites updated across all 10 backends and test code. *(W24)*
- **W24**: `CallNode` in `scg_to_ir.rs` now carries `is_extern: bool` field propagated through `lower_call`. *(W24)*

### Features

#### Multi-Architecture Codegen (W1, W2, W3, W4, W5, W6)

- **W1**: All 9 native backends pass SHA256d execution test: x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64. LoongArch64 passes individual operations; Wasm32 generates valid modules. *(W1)*
- **W1**: Fixed PPC64 RLDICL encoding, 32-bit masking, and ss_load_imm zero-extension. *(W1)*
- **W1**: Fixed LoongArch64 3R opcodes (24 of 26 were wrong), shift immediate formats, LU12I_W/LU32I_D, BEQZ/BNEZ, and FP opcodes. *(W1)*
- **W1**: Wasm32: Added ROR/ROL via shift+or sequence, fixed push_value type hints. *(W1)*
- **W2**: LoongArch64 deep audit: fixed prologue/epilogue register save ordering, Select branchless via maskeqz/masknez, stack space allocation for >8 args, added Maskeqz/Masknez instructions. *(W2)*
- **W2**: x86_64 ISel fixes: immediate operand handling, stack-slot store ordering. *(W2)*
- **W2**: RISC-V 64 fixes: immediate encoding, branch offset calculations. *(W2)*
- **W2**: ARM32 fixes: condition code encoding, LDR/STR offset calculations. *(W2)*
- **W2**: MIPS64 fixes: delay slot handling, big-endian ELF generation. *(W2)*
- **W3**: Cross-backend ABI consistency improvements across all 10 backends. *(W3)*
- **W3**: Backend trait architecture with `TargetInfo` and `Backend` traits for multi-backend support. *(W3)*
- **W4**: Per-backend instruction encoding verification and disassembler fixes. *(W4)*
- **W5**: Structured SCG output for LLMs: `SCG::to_json()` and `SCG::to_text()` methods with LLM-friendly JSON types (`LlmNode`, `LlmEdge`, `LlmFunction`, `LlmRegion`, `LlmSummary`). *(W5-d)*
- **W6**: Cross-backend consistency test suite (9 tests across 4 IR programs × 10 backends). *(W6-b)*
- **W6**: ELF validation tests for all 7 native backends (ELF32/64, endianness, machine types). *(W6-c)*
- **W6**: Wasm32 binary validation tests (12 tests: magic, sections, globals, exports, code bodies). *(W6-d)*
- **W6**: Parser roundtrip tests (10 tests: minimal program, memory ops, SHA256d parse, error recovery). *(W6-g)*
- **W6**: PPC64 deep audit: 7 encoding/ABI bugs fixed (LR save offset, CMP l-field, RLDCL/RLDCR opcode 30, mb5/me5, BH field, ROR/ROL, I-form LI mask), 11 new tests. *(W6-e)*

#### Register Allocator Improvements (W9)

- **W9**: LoopDetector with back-edge analysis and induction variable detection. *(W9)*
- **W9**: Loop-depth-aware spill weights (10^depth multiplier, induction variable 3x bonus). *(W9)*
- **W9**: GreedyRegCache: target-independent register cache with LRU eviction, caller-saved preference, dead-vreg release. *(W9)*
- **W9**: LivenessAnalysis: per-instruction dataflow with dead-at detection. *(W9)*
- **W9**: Enhanced `allocate_function_enhanced()` with priority sorting and dead-vreg reuse. *(W9)*

#### LLM Integration (W5, W7, W13, W21, W22)

- **W5**: LLM language reference document (`docs/llm-language-reference.md`, 5542 words) with 15 sections and code examples. *(W5-e)*
- **W7**: Parser hardened for LLM-generated code: LLM type aliases (int→i32, float→f32), macro detection (println!, vec!), C-style for loop detection, `&T`/`&mut T` auto-conversion to `*T`. *(W7)*
- **W13**: Enhanced REPL with LLM-friendly commands: `:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`, tab completion, ANSI color output. *(W13)*
- **W21**: `VumaForLLM` API layer: `compile()`, `check()`, `analyze()`, `to_wasm()`, `explain_error()`, `suggest_fixes()`, `targets()`. *(W21)*
- **W21**: `LLMCompileResult` with success, diagnostics, explanation, SCG JSON, Wasm binary, binary sizes. *(W21)*

#### Verification & Safety (W11, W12, W15, W17, W28)

- **W11**: `VumaCompiler.verify()` method integrated with IVE + proof pipeline; `VerificationReport` with per-invariant pass/fail and counterexamples. *(W11)*
- **W11**: Property-based testing with proptest: 15 tests across parser roundtrip, cross-backend, SCG invariants, verification. *(W11)*
- **W15**: Expanded diagnostic codes from 23 to 65 (E001-E050, W001-W010, I001-I005). *(W15)*
- **W15**: Error chaining with `VumaDiagnostic::chain()`, root cause analysis, causal chain serialization. *(W15)*
- **W15**: Structured `Suggestion` with edit ranges, replacements, and `SuggestionApplicability`. *(W15)*
- **W15**: Four output formats: JSON, ANSI rich text, plain text, LSP. `DiagnosticSummary` for statistical analysis. *(W15)*
- **W17**: Memory safety analyzer with 10 violation types (E041–E050): UseAfterFree, DoubleFree, MemoryLeak, BoundsCheckFailure, NullDereference, DanglingPointer, UninitializedRead, BufferOverflow, UseAfterScope, InvalidFree. *(W17)*
- **W17**: Runtime bounds checking behind `--safe` CLI flag. *(W17)*
- **W28**: Constant-time crypto operations: `ct_select`, `ct_eq`, `ct_ne`, `ct_lt`, `ct_gte` — branchless implementations across all 10 backends. *(W28)*
- **W28**: PPC64 carry-flag-based constant-time mask (addic+subfe). LoongArch64 maskeqz/masknez branchless select. *(W28)*

#### Module System & Package Manager (W10, W23)

- **W10**: Multi-file compilation with `import "path"::{name1, name2};` syntax. *(W10)*
- **W10**: `ModuleResolver` with circular import detection, name conflict detection, symbol validation. *(W10)*
- **W10**: `compile_with_path()` API for file-aware compilation. *(W10)*
- **W23**: Package manager foundation: `PackageManifest`, `parse_manifest()`, `resolve_dependencies()`, `PackageRegistry` with `publish()`, `fetch()`, `list()`. *(W23)*
- **W23**: CLI subcommands: `vuma pkg init <name>`, `vuma pkg build`, `vuma pkg add <dep> [version]`. *(W23)*

#### FFI & Syscalls (W24)

- **W24**: SyscallTable with 19 syscalls across 10 architectures (verified against Linux kernel headers). *(W24)*
- **W24**: Architecture-specific relocation kinds: Arm32Call, Mips26, Ppc64Rel24, LoongArchB26, etc. *(W24)*
- **W24**: `is_extern` flag on `IRInstr::Call` and `CallNode` for FFI vs local call distinction. *(W24)*

#### Standard Library (W8, W28)

- **W8**: New modules: `crypto.rs` (SHA-256 constants and host-side helpers), `string.rs` (strlen, strcmp, memcpy, memset), `math.rs` (abs, min, max, clamp). *(W8)*
- **W8**: Enhanced `alloc.rs` (heap_alloc, heap_free, heap_realloc), `io.rs` (read_bytes, write_bytes, read_u32_le, write_u32_le). *(W8)*
- **W28**: `std/crypto.rs`: 5 constant-time u32 functions with VUMA-VERIFIED annotations. *(W28)*

#### Tooling & Infrastructure (W16, W19, W20, W21)

- **W16**: GitHub Actions CI: test + release workflow, cross-compile matrix for all 10 ISA targets, Dependabot. *(W16)*
- **W19**: ABI conformance testing: 27 tests covering calling conventions for all 10 backends. *(W19)*
- **W20**: DWARF debug info: per-backend address size and min_inst_length, `DwarfBuilder::for_backend()`, `--debug-info` CLI alias. *(W20)*
- **W21**: ELF linker hardening: 3 LOAD segments (W^X), per-arch section alignment, `--sections` CLI flag. *(W21)*
- **W21**: Performance benchmarking suite: SHA256d per-backend, compilation speed, backend comparison, codegen quality. *(W21)*

#### Documentation (W5, W13)

- **W5**: LLM language reference (`docs/llm-language-reference.md`). *(W5)*
- **W13**: ROADMAP.md overhaul: Phase 2 milestones, LLM integration section, updated dependency graph. *(W13)*
- **W13**: architecture.md updated: LLM Integration Architecture section, multi-arch references, Pi5 removal. *(W13)*

### Bug Fixes

- **W1**: PPC64 RLDICL encoding used wrong instruction; replaced with Rlwinm for 32-bit masking. *(W1)*
- **W1**: LoongArch64 had 24 out of 26 3R-format opcodes completely wrong. *(W1)*
- **W2**: LoongArch64 prologue saved caller FP at wrong offset; fixed to use `$fp + (i-8)*8`. *(W2)*
- **W2**: LoongArch64 Select used hardcoded branch offset that broke for multi-instruction stores; replaced with maskeqz/masknez. *(W2)*
- **W2**: LoongArch64 Call didn't allocate stack space for >8 args, overwriting caller frame. *(W2)*
- **W6**: `TargetAgnosticRegAlloc::expire_old` misclassified callee-saved registers (checked free pool instead of original list). *(W6-f)*
- **W6**: DCE only tracked per-block liveness, could remove live cross-block definitions. *(W6-f)*
- **W6**: `emit_raw()` silently dropped data sections for bare-metal targets. *(W6-f)*
- **W6**: PPC64 LR save at wrong ELFv2 offset (fs+8 → fs+16). *(W6-e)*
- **W6**: PPC64 CMP/CMPL l-field at wrong bit position (shift 21 → 22). *(W6-e)*
- **W6**: PPC64 RLDCL/RLDCR used primary opcode 31 instead of 30. *(W6-e)*
- **W6**: PPC64 RLDCL/RLDCR missing mb5/me5 bit for mask values >= 32. *(W6-e)*
- **W19**: `BackendKind::PPC64` → `BackendKind::PowerPC64` in emit.rs. *(W19)*
- **W23**: Topological sort produced wrong build order; added `sorted.reverse()`. *(W23)*
- **W23**: `v == version_req` type mismatch; fixed to `**v == version_req`. *(W23)*
- **W28**: Non-exhaustive match errors in opt.rs, arm64.rs, emit.rs, loongarch64, ppc64. *(W28)*

### Documentation

- **W5**: LLM language reference with 15 sections covering types, functions, memory, pitfalls, and target platforms. *(W5)*
- **W13**: ROADMAP.md updated to reflect 10 backends, LLM API, LSP, REPL, Phase 2 progress. *(W13)*
- **W13**: architecture.md Section 9: LLM Integration Architecture with Wasm32 sandbox description. *(W13)*

---

## [0.1.0] — 2026-03-05

Initial release of the VUMA framework — Verified-Unsafe Memory Access AI-Native Programming Language.

---

### Wave 1: Foundation & Formal Specifications

*The first wave established the mathematical foundations, formal specifications, and initial implementations of all 12 workspace crates.*

#### Added — Formal Specifications

- **SCG Formal Specification** (`docs/specs/scg-formal-spec.md`, 475 lines) — Mathematical model for the Semantic Computation Graph.
- **RepD Formal Specification** (`docs/specs/repd-formal-spec.md`, 546 lines) — Representation descriptor lattice with 7 variants.
- **CapD Formal Specification** (`docs/specs/capd-formal-spec.md`, 492 lines) — Capability descriptor lattice.
- **RelD Formal Specification** (`docs/specs/reld-formal-spec.md`, 600 lines) — Relational descriptor kinds.
- **VUMA Invariants Specification** (`docs/specs/vuma-invariants-spec.md`, 742 lines) — Five global memory-safety invariants.
- **MSG Construction Specification** (`docs/specs/msg-construction-spec.md`, 850 lines) — Memory State Graph construction algorithm.
- **AArch64 Memory Model Specification** (`docs/specs/aarch64-memory-model-spec.md`, 809 lines).
- **Security Model Specification** (`docs/specs/security-model-spec.md`, 606 lines).
- **BD Inference Algorithm** (`docs/specs/bd-inference-algorithm.md`, 1027 lines).
- **VUMA Verification Algorithm** (`docs/specs/vuma-verification-algorithm.md`, 1098 lines).
- **ARM64 Codegen Algorithm** (`docs/specs/arm64-codegen-algorithm.md`, 1182 lines).
- **Benchmark Design** (`docs/specs/benchmark-design.md`, 695 lines).
- **Trivial Proofs** (`docs/specs/trivial-proofs.md`, 547 lines).
- **Doubly-Linked List Proof** (`docs/specs/dlist-proof.md`, 631 lines).
- **Decidability Analysis** (`docs/specs/decidability-analysis.md`, 416 lines).

#### Added — Source Crates (Initial Implementation)

- **`vuma-scg`** — Semantic Computation Graph (~10,268 lines)
- **`vuma-bd`** — Behavioral Descriptors (~10,073 lines)
- **`vuma-core`** — VUMA Memory Model (~16,204 lines)
- **`vuma-ive`** — Inference & Verification Engine (~12,500 lines)
- **`vuma-cor`** — Continuous Optimization Runtime (~6,244 lines)
- **`vuma-projection`** — Projection System (~8,090 lines)
- **`vuma-parser`** — Parser/Frontend (~9,461 lines)
- **`vuma-codegen`** — ARM64 Code Generation (~11,879 lines)
- **`vuma-proof`** — Formal Proof System (~9,124 lines)
- **`vuma-std`** — Standard Library (~10,303 lines)
- **`vuma-tests`** — Integration Tests & Benchmarks (~3,962 lines)

#### Added — Build & CI

- `Makefile`, `justfile`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`
- `.cargo/config.toml`, `.github/workflows/ci.yml`

#### Added — Documentation

- `docs/architecture.md` (994 lines), `docs/ROADMAP.md` (277 lines)
- `docs/CONTRIBUTING.md` (840 lines), `docs/CONVENTIONS.md` (796 lines)
- `docs/GLOSSARY.md` (893 lines)

---

### Wave 2: Core Verification & AArch64 Platform

- **SCG → MSG conversion** (1357 lines) — Topological walk producing well-formed Memory State Graphs.
- **Incremental MSG** (1907 lines) — MSGDelta computation and application.
- **Invariant aggregator** (1141 lines) — Unified verification pipeline.

---

### Wave 3: Standard Library & COR Enhancement

- **Standard Library Primitives**: RelD, BD triple, HasBD trait, Ptr\<T\>, RegionPtr\<T\>, Slice\<T\>, VumaResult, VumaOption, Range.
- **COR Enhancements**: PmuCounters, SpeculativeExecutor, DeploymentManager with hot-swap FSM.

---

### Wave 4: Parser, Collections, & Benchmarks

- **Parser Error Recovery**: 8 error kinds, 5 strategies, ParseResult\<T\>, "Did you mean?" suggestions.
- **Collections**: VumaString, SipHasher13, iterator types, BD tracking.
- **Benchmark Suite**: 8 categories with 40+ benchmarks.

---

### Wave 5: Documentation & Project Packaging

- **Architecture Document** (994 lines), **Language Reference** (1101 lines)
- **CONTRIBUTING.md**, **CONVENTIONS.md**, **GLOSSARY.md**, **ROADMAP.md**
- **MANIFEST.md**, **README.md**, **CHANGELOG.md**

---

## Release Notes

### [0.1.0-alpha.0] — 2026-03-05

This release represents Phase 2 (substantially complete) of the VUMA framework. Major additions include:

- **8-architecture codegen**: x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, Wasm32 — all passing SHA256d or individual operation tests
- **LLM integration**: VumaForLLM API, LSP server, enhanced REPL (`:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`), structured diagnostics
- **Wasm32 sandbox**: LLM agents can compile to safe, sandboxed WebAssembly modules
- **Verification hardening**: Interprocedural analysis, escape analysis, verification cache, property-based testing
- **Memory safety**: 10 violation types, compile-time checks, runtime bounds checking
- **Constant-time crypto**: Branchless ct_select/ct_eq across all 10 backends
- **Module system**: Multi-file compilation with import resolution
- **Package manager**: Foundation with manifest, resolver, registry
- **FFI & syscalls**: 19 syscalls across 10 architectures, is_extern flag, architecture-specific relocations
- **Register allocator**: Loop-aware spill weights, GreedyRegCache, dead-vreg reuse
- **Diagnostics**: 65 diagnostic codes, error chaining, structured suggestions, 4 output formats

**Known Limitations:**
- BD inference completeness (M2.3) and doubly-linked list verification (M2.4) remain pending
- ARM64 atomics and concurrent verification are Phase 3 targets
- COR end-to-end integration not yet complete

### [0.1.0] — 2026-03-05

This is the initial public release of the VUMA framework. It contains the complete architectural foundation: all 12 workspace crates, 15 formal specifications, 10 example programs, a comprehensive benchmark suite, and full documentation.

**Known Limitations:**
- Concurrent verification is limited to single-threaded programs
- ARM64 codegen does not yet support atomic instructions
- The COR is not yet integrated end-to-end
- The parser has known type mismatches in the AST→SCG lowering path

---

## Worklog

- **2026-03-05 — Wave 1-5:** Initial release (v0.1.0): All 12 workspace crates, 15 formal specifications, 10 example programs, benchmarks, documentation.
- **2026-03-05 — Wave 6:** Cross-backend tests, ELF/Wasm validation, parser roundtrip, PPC64 deep audit, shared codegen bug fixes.
- **2026-03-05 — Wave 7:** Parser hardened for LLM-generated code with type aliases, macro detection, error recovery.
- **2026-03-05 — Wave 8:** Standard library expanded with crypto, string, math, and I/O modules.
- **2026-03-05 — Wave 9:** Register allocator improved with loop detection, GreedyRegCache, dead-vreg reuse.
- **2026-03-05 — Wave 10:** Multi-file compilation with import resolution and ModuleResolver.
- **2026-03-05 — Wave 11-12:** Verification pipeline hardened with VumaCompiler.verify(), property-based testing.
- **2026-03-05 — Wave 13-14:** Documentation overhaul, REPL enhancements (`:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`).
- **2026-03-05 — Wave 15:** Comprehensive structured error reporting (65 diagnostic codes, error chaining, suggestions).
- **2026-03-05 — Wave 16:** CI build matrix for all 10 ISA targets.
- **2026-03-05 — Wave 17-18:** Memory safety analyzer, performance benchmarking suite.
- **2026-03-05 — Wave 19-20:** ABI conformance testing (27 tests), DWARF debug info enhancements.
- **2026-03-05 — Wave 21-22:** Linker hardening (3 LOAD segments, W^X), VumaForLLM API layer.
- **2026-03-05 — Wave 23:** Package manager foundation.
- **2026-03-05 — Wave 24:** FFI and syscalls (19 syscalls × 10 architectures, relocations).
- **2026-03-05 — Wave 25-27:** Security hardening, codegen quality improvements, test infrastructure.
- **2026-03-05 — Wave 28:** Constant-time crypto operations across all 10 backends.
- **2026-03-05 — Wave 29-31:** Final hardening, documentation updates, release preparation.
- **2026-03-05 — Wave 32:** Release preparation: Cargo.toml v0.1.0-alpha.1, CHANGELOG, README, ROADMAP, RELEASES.md.
