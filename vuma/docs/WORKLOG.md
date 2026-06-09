# VUMA Worklog

## 2026-03-05 — Task 4-15: Comprehensive Architecture Documentation Rewrite

### Files Modified

- **`docs/architecture.md`** — Complete rewrite with 8 major sections (each 200+ words):
  - **Section 1: System Overview** — 6-layer architecture (SCG, IVE, Projections, COR, BD, VUMA) with detailed descriptions of each layer, layer interaction diagram (ASCII), and 5 architectural principles (SCG primacy, verification over restriction, inference over annotation, continuous optimization, bare-metal first)
  - **Section 2: Data Flow Diagram** — Full text-based data flow from source to execution with three feedback loops (verification feedback, profile feedback, deployment feedback), 6-stage pipeline description (lexing/parsing, AST-to-SCG lowering, BD inference, VUMA verification, code generation, execution/feedback)
  - **Section 3: Crate Dependency Graph** — Complete workspace layout with all 12 crates and their file-level contents, ASCII dependency graph, 7 key dependency rules (SCG as foundation, BD/proof orthogonality, IVE as orchestrator, VUMA extension, codegen/COR execution layer, Pi 5 platform, projection/parser human interface)
  - **Section 4: Key Data Structures and Their Relationships** — 5 subsections: SCG (core types, node payloads, edge kinds, operations), BD (RepD/CapD/RelD with design decisions, composition/compatibility), MSG (region/derivation/access/sync, SCG-to-MSG conversion, incremental MSG), Proof (derivation tree, tiered confidence, counterexample), Data Structure Relationships (flow diagram showing SCG → BD → Annotated SCG → MSG → Verification → Proof → Verified SCG → Codegen → ARM64 → COR → Pi 5)
  - **Section 5: Verification Pipeline** — Pipeline architecture diagram, the five VUMA invariants (liveness, exclusivity, interpretation, origin, cleanup) with detailed algorithm descriptions, verification result aggregation (VerificationSummary, AggregatedResult, DiagnosticsReport, InvariantDelta, VerificationDebt), incremental verification (MSGDelta, compute_delta, apply_delta)
  - **Section 6: Code Generation Pipeline for Pi 5** — Three-phase pipeline (SCG → IR → regalloc → emission), IR types (IrFunction, IrBasicBlock, IrInstruction, IrTerminator, IrValue), node-to-IR mapping, register allocation (linear-scan, AAPCS64), ARM64 machine code emission (instruction encoding, ELF generation), Pi 5 bare-metal boot sequence (_start, boot_main, linker script, FDT parsing)
  - **Section 7: Runtime Optimization Pipeline** — COR architecture diagram, profile-guided optimization (ProfileCollector, Pi5PmuCounters, HotPath, collect_profile, CacheOptimize/BranchLayout suggestions), speculative optimization (SpeculativeExecutor 3-phase lifecycle: identify/apply/validate-and-rollback, BranchPredictionTable, SpeculativeInlining, SpeculativeCodeMotion, Snapshot-based rollback), SCG transformation passes (PassManager, DCE, constant folding, inlining, CSE, VerificationPass), deployment and hot-swap (DeploymentManager, 6-phase state machine, delta deployment, version tracking)
  - **Section 8: Security Model Overview** — 5 security layers (memory safety via VUMA invariants, capability security via CapD, information flow via RelD, region security via SCG regions, platform security via Pi 5), threat model (6 categories: memory corruption, information disclosure, privilege escalation, resource exhaustion, concurrent access violations, supply chain attacks), verification confidence and debt (VerificationLevel tier, VerificationDebt priority tracking)

- **`docs/ROADMAP.md`** — Complete rewrite with 5 phases:
  - **Phase 1: Foundation (COMPLETED)** — 4 milestones (SCG core, MSG construction, IVE core, ARM64 codegen/Pi 5 boot), comprehensive deliverables listing all 12 crates with their implemented functionality, Phase 1 achievement summary
  - **Phase 2: Core Implementation (CURRENT)** — 6 milestones (exclusivity/interpretation verification, cleanup/full pipeline, BD inference completeness, verified data structures, ARM64 codegen expansion, PGO), 6 deliverables with detailed descriptions, Phase 2 success criteria (7 items)
  - **Phase 3: Hardening & Optimization (NEXT)** — 5 milestones (ARM64 atomics, concurrent verification, COR integration, Pi 5 peripherals, lock-free Pi 5 demo), 5 deliverables, Phase 3 success criteria (11 items)
  - **Phase 4: Language Server & Tooling (PLANNED)** — 5 milestones (projections, outcome spaces, parser/LSP, standard library, ecosystem), 4 deliverables, Phase 4 success criteria (11 items)
  - **Phase 5: Self-Hosting Compiler (PLANNED)** — 5 milestones (compiler core in VUMA, self-verification, self-compilation, self-hosting, performance parity), 5 deliverables, Phase 5 success criteria (5 items)
  - Plus: dependency graph, risk mitigation table (7 risks), success criteria summary table

### Source Files Read for Context
- `Cargo.toml` — workspace members, dependencies
- `MANIFEST.md` — project manifest, documentation and source crate inventory
- `src/scg/src/lib.rs` — SCG crate overview, module layout, re-exports
- `src/vuma/src/lib.rs` — VUMA core module overview, MSG, Region, Address, re-exports
- `src/ive/src/lib.rs` — IVE module overview, re-exports, inference/verification
- `src/codegen/src/lib.rs` — Codegen pipeline, CodegenError
- `src/bd/src/lib.rs` — BD crate overview, RepD/CapD/RelD
- `src/cor/src/lib.rs` — COR architecture, CORuntime, profile, speculative, deployment
- `docs/WORKLOG.md` — prior worklog entries for context on completed tasks

## 2026-03-05 — Task 4-18: Comprehensive Rewrite of CONTRIBUTING.md, CONVENTIONS.md, GLOSSARY.md

### Files Modified

- **`docs/CONTRIBUTING.md`** — Complete rewrite with 7 major sections and table of contents:
  - Section 1: How to Build the Project — prerequisites table (stable toolchain per rust-toolchain.toml, not nightly), workspace crate table (12 crates with paths and purposes), Make/just commands, Pi 5 cross-compilation (userspace + bare-metal with link.ld/build.rs/kernel8.img pipeline), QEMU setup, formatting/linting (rustfmt.toml max_width=100, clippy.toml cognitive-complexity-threshold=50)
  - Section 2: How to Run Tests — unit tests (per-crate, specific test), integration tests (vuma-tests crate), verification tests (IVE with --test-threads=1), codegen tests, proof tests, benchmarks, full CI check command
  - Section 3: How to Add New SCG Node Types — 8-step process: define payload struct → register NodeType variant → add NodePayload variant → re-export → update graph construction → add IVE verification → add codegen → add tests at all levels; example with BarrierNode
  - Section 4: How to Add New Verification Passes — 9-step process: define invariant in InvariantKind → write formal spec in docs/specs/ → implement verification module → register module → integrate into aggregator → add proof support → update VUMA core → add tests/examples → update documentation
  - Section 5: How to Add New ARM64 Instructions — 7-step process: define Instruction variant → implement encode() → implement Display → add IR instruction → add SCG-to-IR mapping → add encoding tests verified against ARM spec → update documentation
  - Section 6: Code Review Process — 7 review criteria (correctness, verification compliance, documentation, testing, conventions, performance, architecture), review timeline (2 days initial, 1 day follow-up, 1-2 approving reviews), special rules for SCG nodes/verification passes/ARM64 instructions/unsafe annotation changes
  - Section 7: PR Template — summary, related issues, changes, verification impact checklist (6 items), test plan (4 items), checklist (9 items including fmt/clippy/test/public docs/no bare unsafe/glossary/conventions/spec)

- **`docs/CONVENTIONS.md`** — Complete rewrite with 6 major sections and table of contents:
  - Section 1: Rust Coding Style (Beyond rustfmt) — max_width=100 code/80 docs, import grouping (4 tiers), trailing commas, match arm blocks, implicit returns, struct literal formatting, method chaining, clippy rules (cognitive-complexity-threshold=50), newtype pattern for domain IDs, derive macro requirements
  - Section 2: Error Handling Patterns — thiserror for libraries (with #[error], #[from], structured fields), anyhow for applications (with .context() chaining), no-panic rules (4 acceptable cases), anti-patterns (silent unwrap, panicking index), cross-crate error wrapping
  - Section 3: Testing Conventions — 3 categories (unit/integration/verification) with table, unit test rules (Arrange-Act-Assert, independence), integration test rules (cross-crate pipelines, positive+negative), verification test rules (--test-threads=1, per-invariant), property-based testing with proptest, test naming convention ({unit}_{scenario}_{outcome})
  - Section 4: Naming Conventions for VUMA Types — general Rust naming table, SCG node types ({Kind}Node), BD components ({DescriptorKind}Descriptor in APIs, abbreviations OK in docs/locals), verification results ({Property}Result/Report), violations ({Property}Violation), errors ({Domain}Error), verifiers ({Property}Verifier), ARM64 instructions (CamelCase mnemonic), ID newtypes ({Entity}Id), function naming patterns table (verify_, infer_, build_, compute_, find_, emit_, lower_, encode_, as_, to_, is_, has_)
  - Section 5: Documentation Conventions — mandatory doc comments (6 required sections: summary, description, examples, panics, errors, safety), module-level documentation (5 required elements), internal documentation rule, 80-char doc line limit, code example requirements (valid Rust, assertions)
  - Section 6: Git Commit Message Format — conventional commits specification, type table (10 types: feat/fix/docs/test/refactor/perf/chore/style/build/ci), scope table (14 scopes matching crate names), examples with body and footer, branch naming (feat/ and fix/ prefixes), PR titles

- **`docs/GLOSSARY.md`** — Complete rewrite with expanded entries and new terms:
  - Project Core Terms: SCG, IVE, COR, BD, RepD, CapD, RelD, VUMA, MSG, Projection, Outcome Space, Verification Debt, Verification Confidence (13 entries, all 50+ words with implementation details)
  - Verification Invariant Terms: Liveness, Exclusivity, Interpretation, Origin, Cleanup (5 entries with verifier module paths, algorithms, and violation types)
  - ARM64 Terms: AAPCS64, DMB, DSB, ISB, LDXR, STXR, CAS, Cortex-A76 (8 entries with codegen Instruction enum references)
  - Raspberry Pi 5 Terms: BCM2712, GPIO, UART (3 entries with driver module paths)
  - Type Theory Terms: Nominal Types, Structural Types, Behavioral Types, Capability Calculi (4 entries)
  - Additional Project Terms: Derivation Chain, Region (SCG), VUMA-VERIFIED, IVE-TODO, Invariant Aggregator, Proof Object (6 entries, including 2 new entries not in original)
  - All entries 50+ words with pronunciation, detailed definitions referencing actual code paths, and cross-references
  - New entries added: CAS, Invariant Aggregator, Proof Object

### Source Files Read for Context
- `Cargo.toml` — workspace members, dependencies, profile settings
- `rust-toolchain.toml` — stable channel, components, targets
- `rustfmt.toml` — max_width=100, tab_spaces=4, edition=2021
- `clippy.toml` — cognitive-complexity-threshold=50
- `Makefile` — build/test/pi5/bench targets
- `justfile` — build/test/clean shortcuts
- `src/scg/src/lib.rs` — SCG crate overview, node types, re-exports
- `src/scg/src/node.rs` — NodeType enum, NodePayload, all node structs
- `src/ive/src/lib.rs` — IVE module layout, re-exports
- `src/ive/src/verification.rs` — VerificationEngine, 5 invariants
- `src/vuma/src/lib.rs` — VUMA core module layout, MSG, Region, Address
- `src/codegen/src/lib.rs` — CodegenError, pipeline description
- `src/codegen/src/arm64.rs` — Register, Condition, Instruction enum, encode()
- `src/bd/src/lib.rs` — BD crate overview, RepD/CapD/RelD
- `src/proof/src/lib.rs` — Proof module overview, checker, counterexample

## 2026-03-04 — Task 4-16: VUMA Language Reference Document

### Files Created
- **`docs/language-reference.md`** — Complete VUMA language reference document with 11 sections
  - Section 1: Lexical Structure — keywords (6 categories: core, memory, concurrency, safety, BD, module), operators (arithmetic, comparison, logical, bitwise, pointer, type), literals (integer, hex address, binary, octal, float, string, char, byte string, raw string, boolean), comments (line, block, doc), identifiers
  - Section 2: Types and BD Annotations — BD triple (RepD × CapD × RelD), RepD (representation descriptor with subsumption lattice), CapD (capability descriptor with meet/join lattice and lock conditions), RelD (relational descriptor with temporal/dependency/security relations), full type syntax (primitives, pointers, region-annotated pointers, arrays, structs, generics, function types, BD annotations), type ascription
  - Section 3: Memory Model — regions (allocate, map_device), allocations (IVE Liveness/Interpretation/Cleanup checks), derivations (pointer derivation chains, bulk invalidation on free), accesses (read/write with LIVE invariant checks at byte-level granularity)
  - Section 4: Pointer Operations — dereference (*), address-of (@), offset/pointer arithmetic (+), derive(ptr, region), cast (as keyword with proof obligations, sizeof, alignof)
  - Section 5: Control Flow — if/else, while, for, loop, match (literals, identifiers, wildcards, struct patterns), return
  - Section 6: Functions — function definition (fn), calling conventions, async blocks, spawn expressions, await
  - Section 7: Concurrency — sync blocks (happens-before ordering), channels (send/recv with ownership transfer), locks (lock/unlock with CapD integration), atomics (AtomicU64 with Acquire/Release/AcqRel/SeqCst ordering)
  - Section 8: Memory Safety — the five LIVE invariants (Liveness, Exclusivity, Interpretation, Origin, Cleanup), verification pipeline (parsing → SCG → BD inference → invariant verification → debt tracking), verification annotations (#bd, proof hints, safe blocks), verification results (Proven, ProbablySafe, Violated)
  - Section 9: Standard Library Overview — memory management, data structures, concurrency primitives, I/O, type operations, Option type
  - Section 10: Pi 5 Platform-Specific Features — device memory mapping, GPIO register layout, ARM64 code generation, bare-metal execution, const addresses
  - Section 11: Appendix — complete keyword reference table (40+ keywords), operator precedence table (12 levels)
  - All sections include 200+ words with code examples drawn from actual VUMA example programs and source code

### Source Files Read
- `examples/hello_memory.vuma` — Basic allocate/write/read/free pattern
- `examples/doubly_linked_list.vuma` — Doubly-linked list with sentinel node pattern
- `examples/gpio_blink.vuma` — Pi 5 GPIO hardware access
- `examples/arena_allocator.vuma` — Arena allocator with derivation chains
- `examples/lock_free_queue.vuma` — Lock-free SPSC queue with atomics
- `src/parser/src/ast.rs` — Full AST type definitions (Program, Item, Stmt, Expr, Type, etc.)
- `src/parser/src/lexer.rs` — Token definitions, keyword table, lexing rules
- `src/ive/src/lib.rs` — IVE module layout and public API
- `src/ive/src/verification.rs` — Five invariant verification engine
- `src/ive/src/inference.rs` — BD inference engine
- `src/ive/src/exclusivity.rs` — Exclusivity verifier with interference graph
- `src/ive/src/interpretation.rs` — Interpretation verifier with RepD/CapD/RelD checks
- `src/bd/src/lib.rs` — BD crate overview
- `src/bd/src/descriptor.rs` — BD struct definition and compatibility/refinement

## 2026-03-05 â Task 3-19: COR Deployment System Enhancement

### Files Modified
- **`src/cor/src/deployment.rs`** â Major enhancement of the deployment subsystem
  - **`DeploymentTarget`** â Expanded from 3 to 4 variants: `Local`, `Pi5Bare { board_id, core_id }`, `Pi5Linux { host, core_affinity }`, `Remote { endpoint }`; predicates `is_pi5()`, `supports_hot_swap()`, `kind_label()`
  - **`DeploymentPackage`** â Compiled binary + metadata + debug info; constructors `new()`, `with_debug_info()`, `validate_checksum()`
  - **`PackageVersion(u64)`** â Monotonic version wrapper with Ord, Display
  - **`PackageMetadata`** â Region ID, version, CRC32 checksum, optimization label, code size, timestamp
  - **`DebugInfo`** â Source-to-offset mapping, symbol table, compiler notes
  - **`DeploymentResult`** â Outcome: region_id, version, target, duration, bytes_transferred, hot_swapped, was_delta
  - **`DeploymentManager`** â Central orchestrator: `deploy()`, `hot_swap()`, `rollback()`, `deploy_delta()`
  - **`HotSwapPhase`** â 6-phase state machine: Idle, PreparingShadow, AwaitingSafePoint, Swapping, Completed, Failed
  - **`HotSwapState`** â Per-region hot-swap tracking
  - **`VersionLog`** â Per-region version history with rollback support
  - **`VersionRecord`** â Single deployment record with full code retained for rollback
  - **`DeploymentDelta`** â Block-level binary diff: `compute()`, `apply()`, `estimated_size()`, `is_empty()`
  - **`DeploymentError`** â Extended with ChecksumMismatch, HotSwapNotSupported, NoRollbackTarget, NoPreviousVersion, DeltaApplyFailed
  - CRC32 implementation (IEEE 802.3) for checksum validation
  - Preserved DeploymentPlanner/DeploymentPlan backwards compatibility
  - 18 tests, all passing


## 2026-03-05 — Task 3-29: Std Primitives Module Enhancement

### Files Modified
- **`src/std/src/primitives.rs`** — Major enhancement of the VUMA standard library primitives module
  - **`RelD` (Relational Descriptor)** — New type capturing the relational properties a value participates in:
    - `RelKind` enum: Containment, Liveness, Aliasing, DataFlow, RegionBound, Ownership
    - `RelD` struct with `new()`, `empty()`, `has()`, `compose()`, `refines()`, `intersect()`
    - Factory functions: `ptr_reld()`, `region_ptr_reld()`, `slice_reld()`, `result_reld()`, `option_reld()`, `numeric_reld()`
  - **`BD` (Behavioral Descriptor)** — New type combining RepD × CapD × RelD:
    - `BD` struct with `new()`, `compatible()`, `refines()`
    - `Display` implementation
  - **`HasBD` trait** — Unified interface for types that produce a BD: `as_bd() -> BD`
  - **`Ptr<T>`** — VUMA pointer with embedded BD annotation:
    - Fields: `addr: u64`, `pointee_bd: BD`, `PhantomData<T>`
    - Methods: `new()`, `null()`, `is_null()`, `offset()`, `repd()`, `capd()`, `reld()`, `as_bd()`
    - BD: RepD = `ptr<T>` (8 bytes), CapD = pointee + Read/Write, RelD = Containment + Liveness
  - **`RegionPtr<T>`** — Pointer bound to a specific memory region:
    - Fields: `addr`, `region_base`, `region_size`, `pointee_bd`
    - Methods: `new()` (panics on out-of-bounds), `in_bounds()`, `checked_offset()`, `repd()`, `capd()`, `reld()`, `as_bd()`
    - BD: RepD = `region_ptr<T>` (24 bytes), CapD = pointee + Read/Write/Shared, RelD = Containment + Liveness + RegionBound
  - **`Slice<T>`** — Pointer + length with BD annotation:
    - Fields: `addr`, `len`, `elem_bd`
    - Methods: `new()`, `empty()`, `is_empty()`, `byte_size()`, `subslice()`, `repd()`, `capd()`, `reld()`, `as_bd()`
    - BD: RepD = `slice<T>` (16 bytes), CapD = elem + Read/Write/Iterate, RelD = Containment + Liveness + DataFlow
  - **`VumaResult<T, E>`** — VUMA result type with BD tracking:
    - Enum: `Ok(T)`, `Err(E)`
    - Methods: `is_ok()`, `is_err()`, `unwrap()`, `unwrap_err()`, `map()`, `map_err()`, `repd()`, `capd()`, `reld()`, `as_bd()`
    - BD: RepD = tagged union, CapD = intersection of Ok/Err, RelD = DataFlow + Ownership
  - **`VumaOption<T>`** — VUMA option type with BD tracking:
    - Enum: `Some(T)`, `None`
    - Methods: `is_some()`, `is_none()`, `unwrap()`, `map()`, `unwrap_or()`, `repd()`, `capd()`, `reld()`, `as_bd()`
    - BD: RepD = `Option<T>` (val_size + 1), CapD = inner CapD, RelD = DataFlow + Liveness
  - **`Range`** — Integer range type (start..end):
    - Fields: `start: u64`, `end: u64`
    - Methods: `new()`, `is_empty()`, `len()`, `contains()`, `repd()`, `capd()`, `reld()`, `as_bd()`
    - BD: RepD = `Range` (16 bytes), CapD = Read/Write/Compare/Iterate/Serialize, RelD = empty
  - Preserved all existing types (CapFlag, CapD, RepD, SyncEdge, SyncEdgeKind, primitive RepD constructors) and their 8 legacy tests
  - **26 tests total** (8 legacy + 18 new):
    1. `test_numeric_capd_has_expected_flags` (legacy)
    2. `test_string_capd_has_expected_flags` (legacy)
    3. `test_uint8_repd_properties` (legacy)
    4. `test_float64_repd_properties` (legacy)
    5. `test_ptr_repd_inherits_pointee_caps` (legacy)
    6. `test_capd_union` (legacy)
    7. `test_capd_intersect` (legacy)
    8. `test_capd_subcap` (legacy)
    9. `test_reld_compose_unions_relations` — RelD compose produces union
    10. `test_reld_refines_superset` — RelD refinement ordering
    11. `test_bd_compatible_same_type` — BD compatibility check
    12. `test_bd_refines` — BD refinement ordering
    13. `test_ptr_creation_and_bd` — Ptr construction + as_bd() BD
    14. `test_ptr_null_and_offset` — Ptr null + offset operations
    15. `test_region_ptr_creation_and_bd` — RegionPtr construction + as_bd() with RegionBound
    16. `test_region_ptr_checked_offset` — RegionPtr bounded offset
    17. `test_region_ptr_out_of_bounds_panics` — RegionPtr panic on OOB
    18. `test_slice_creation_and_bd` — Slice construction + as_bd() with Iterate/DataFlow
    19. `test_slice_subslice` — Slice subslice bounds checking
    20. `test_vuma_result_ok_bd` — VumaResult Ok variant BD
    21. `test_vuma_result_err_bd` — VumaResult Err variant BD
    22. `test_vuma_result_map` — VumaResult map transformation
    23. `test_vuma_option_some_bd` — VumaOption Some variant BD
    24. `test_vuma_option_none_bd` — VumaOption None variant BD
    25. `test_vuma_option_map_and_unwrap_or` — VumaOption map + default
    26. `test_range_creation_and_bd` — Range construction + as_bd() with Iterate/Compare

- **`src/std/src/lib.rs`** — Updated re-exports:
  - Added: `BD`, `RelD`, `RelKind`, `HasBD`
  - Added: `Ptr`, `RegionPtr`, `Slice`, `VumaResult`, `VumaOption`, `Range`
  - Added: `ptr_reld`, `region_ptr_reld`, `slice_reld`, `result_reld`, `option_reld`, `numeric_reld`

### Notes
- No compilation errors from primitives.rs (verified via `cargo check`).
- Pre-existing errors in collections.rs (BdResult/Ve c type mismatches) are unrelated and unchanged.
- All 6 VUMA primitive types implement the `HasBD` trait with `as_bd() -> BD`.
- Each type's BD is fully derivable from its fields and the pointee/element BD.

## 2026-03-05 — Task 3-13: Pi 5 Bare Metal Boot Code

### Files Created
- **`src/pi5/src/boot.rs`** — Bare-metal boot code for Raspberry Pi 5 (BCM2712)
  - **ARM64 exception vector table** (`exception_vector_table`) — Full 16-entry vector table in naked assembly:
    - Current EL with SP0: sync, irq, fiq, serror
    - Current EL with SPx: sync, irq, fiq, serror
    - Lower EL AArch64: sync, irq, fiq, serror
    - Lower EL AArch32: sync, irq, fiq, serror
    - Each handler currently spin-loops; stubs for future context-save/dispatch
  - **`_start` entry point** — Naked assembly in `.text.boot` section:
    1. Saves DTB pointer (x0 → x6) before any clobber
    2. Reads core ID from `MPIDR_EL1.Aff0`
    3. Parks secondary cores (1–3) in `WFE` loop
    4. Sets up 16 KiB stack for core 0 above `__bss_end`, 16-byte aligned
    5. Zeros BSS section (8-byte stores from `__bss_start` to `__bss_end`)
    6. Installs exception vector table via `VBAR_EL1`
    7. Restores DTB pointer and jumps to `boot_main`
  - **`kernel_entry`** — `KERNEL_ENTRY = 0x80000`, the Pi 5 bootloader load address
  - **FDT parsing** — `FdtHeader` struct with `from_raw()`, `from_bytes()`, `is_valid()`:
    - Parses all 10 u32 fields from the 40-byte DTB header
    - Correct mapping: words[0]=magic, [1]=totalsize, [2]=off_dt_struct, [3]=off_dt_strings, [5]=version, [8]=size_dt_strings, [9]=size_dt_struct
    - `is_valid()` checks: magic match, positive totalsize, offsets within bounds, struct/strings don't overflow
  - **`BootInfo`** — Parsed boot context: dtb_addr, FdtHeader, boot_core
  - **`boot_main()`** — High-level entry (core 0 only):
    - Saves DTB address to global
    - Initialises UART at 115200 baud
    - Parses FDT header, logs result
    - Constructs `BootInfo` and calls user `main()`
    - Parks core 0 if main() returns
  - **`zero_bss()`** — Standalone BSS zeroing with SeqCst fence
  - **`park_core()`** — Infinite WFE loop for parking cores
  - **`install_exception_vector_table()`** — Writes VBAR_EL1 + ISB
  - **Linker symbol externs** — `__bss_start`, `__bss_end` with `bss_boundaries()` helper
  - **7 tests** (all passing):
    1. `fdt_header_from_bytes_valid_magic` — Full header round-trip with all fields verified
    2. `fdt_header_from_bytes_invalid_magic_returns_none` — Bad magic rejected
    3. `fdt_header_from_bytes_short_slice_returns_none` — Under-sized buffer rejected
    4. `fdt_header_is_valid_rejects_inconsistent_offsets` — off_dt_struct > totalsize
    5. `fdt_header_is_valid_accepts_consistent_header` — Valid header accepted
    6. `boot_constants_are_correct` — KERNEL_ENTRY, STACK_SIZE_PER_CORE, STACK_ALIGN, FDT_MAGIC, BOOT_BAUD_RATE
    7. `zero_bss_clears_memory` — 64-byte buffer zeroed in place

### Files Modified
- **`src/pi5/src/lib.rs`** — Added `pub mod boot;` and module overview table entry for `boot`

## 2026-03-05 — Task 3-18: COR Speculative Executor Enhancement

### Files Modified
- **`src/cor/src/speculative.rs`** — Major enhancement of the speculative optimization framework
  - **`SpeculationCandidate`** — A code region suitable for speculation with `CandidateKind` (LikelyBranch, HotPath, MonomorphicCall, UncontendedRegion), confidence score `[0,1]`, and affected region ID; `meets_threshold()` predicate
  - **`SpeculationResult`** — Outcome of a speculation: `Success { candidate_id }` or `Failure { candidate_id, reason }`; accessors `is_success()`, `is_failure()`, `candidate_id()`
  - **`BranchPrediction`** — Single edge prediction with probability and sample count; `is_confident(threshold, min_samples)` predicate
  - **`BranchPredictionTable`** — Table of per-edge predictions derived from `ProfileData`; `from_profile()`, `get()`, `insert()`, `sorted_by_probability()`, `generate_candidates()`; frequency-based prediction model
  - **`SpeculativeInlining`** — Speculative inlining engine: `InlineDecision` (call_site, callee, confidence, region), `analyze(profile, threshold)`, `apply_inline(decision, optimized, fallback) → SpeculativeOpt`
  - **`SpeculativeCodeMotion`** — Speculative code-motion engine: `CodeMotionKind` (HoistInvariant, SinkColdCode), `CodeMotionDecision` (kind, confidence), `analyze(profile, hot_threshold, cold_threshold)`, `apply_motion(decision, optimized, fallback) → SpeculativeOpt`
  - **`SpeculativeExecutor`** — Top-level orchestrator for the full speculative-optimization lifecycle:
    - Phase 1: `identify_candidates()`, `identify_inline_candidates()`, `identify_code_motion_candidates()` from profile data
    - Phase 2: `apply_speculation()`, `apply_inline()`, `apply_code_motion()` — each saves a snapshot before transforming
    - Phase 3: `validate_and_rollback()` — checks all active assumptions against runtime observations; invalidates, rolls back, records `Failure` result, and removes from active set
    - Rollback mechanism: `Snapshot` struct captures compiled regions + SCG dimensions before each speculation; `restore()` returns pre-speculation state
    - Queries: `active_count()`, `total_applied()`, `total_rollbacks()`, `results()`, `has_snapshot()`, `is_active(config)`
  - Preserved all existing types (`Assumption`, `SpeculativeOpt`, `SpeculativeOptimizer`) and their 5 legacy tests
  - 19 tests total (5 legacy + 14 new), all passing:
    1. `try_speculative_returns_optimized_when_valid` (legacy)
    2. `deoptimize_switches_to_fallback` (legacy)
    3. `check_assumption_invalidates_on_wrong_branch` (legacy)
    4. `no_contention_check` (legacy)
    5. `optimizer_validate_all` (legacy)
    6. `speculation_candidate_meets_threshold`
    7. `speculation_result_accessors`
    8. `branch_prediction_table_from_profile`
    9. `branch_prediction_generate_candidates`
    10. `speculative_inlining_analyze`
    11. `speculative_inlining_apply`
    12. `speculative_code_motion_analyze`
    13. `speculative_executor_apply_and_rollback`
    14. `speculative_executor_preserves_valid_on_correct_branch`
    15. `speculative_executor_identify_candidates_from_profile`
    16. `speculative_executor_full_lifecycle`
    17. `rollback_restores_snapshot_data`
    18. `candidate_kind_labels`
    19. `executor_is_active_respects_config`

## 2026-03-05 — Task 3-16: COR Profile Collector Enhancement

### Files Modified
- **`src/cor/src/profile.rs`** — Major enhancement of the profile-guided optimization subsystem
  - **`Pi5PmuCounters`** — Pi 5 hardware performance counter snapshot (cycle count, instruction count, cache misses, branch misses) with computed `ipc()`, `cache_miss_rate()`, `branch_miss_rate()`
  - **`ProfileSample`** — Single profiling sample: `timestamp_ns`, `node_id`, `execution_time_ns`, optional `Pi5PmuCounters`; constructors `new()` and `with_pmu()`
  - **`HotPath`** — Sequence of nodes accounting for >80% of execution time, with `total_time_ns`, `cumulative_fraction`, `is_dominant()` predicate
  - **`ProfileData`** — Enhanced with `edge_frequencies: HashMap<EdgeId, u64>`, `node_time_ns: HashMap<NodeId, u64>`, `node_pmu: HashMap<NodeId, Pi5PmuCounters>`; new methods: `record_access_timed()`, `record_edge()`, `record_pmu()`, `ingest_samples()`, `compute_hot_paths()`, `cold_spots()`, `total_execution_time_ns()`; updated `suggest_optimizations()` to emit `CacheOptimize` and `BranchLayout` suggestions from PMU data
  - **`ProfileCollector`** — Thread-safe (`Mutex<ProfileData>` + `AtomicU64` sample counter) runtime collector with `record_access()`, `record_access_timed()`, `record_edge()`, `record_sample()`, `record_pmu()`, `make_sample()`, `make_sample_with_pmu()`, `snapshot()`, `reset()`
  - **`ProfileReport`** — Full analysis output with `total_samples`, `total_execution_time_ns`, `hot_spots: Vec<NodeHotSpot>`, `cold_spots: Vec<NodeId>`, `hot_paths: Vec<HotPath>`, `aggregate_pmu`, `node_pmu`, `recommendations`
  - **`NodeHotSpot`** — Per-node execution stats: `node_id`, `call_count`, `total_time_ns`, `time_fraction`
  - **`collect_profile(scg: &SCG, samples: &[ProfileSample]) -> ProfileReport`** — Main analysis entry point: ingests samples, computes hot/cold spots, hot paths, PMU aggregates, and recommendations
  - **`SuggestionKind`** — Extended with `CacheOptimize` and `BranchLayout` variants
  - 11 tests (all passing):
    1. `record_access_increments_count`
    2. `get_hot_paths_returns_top_k`
    3. `alloc_stats_peak_tracking`
    4. `suggest_optimizations_inline`
    5. `profile_sample_creation_and_pmu`
    6. `hot_path_dominance_threshold`
    7. `profile_collector_thread_safe`
    8. `collect_profile_produces_report`
    9. `pi5_pmu_counters_ipc_and_rates`
    10. `edge_frequencies_recorded`
    11. `ingest_samples_accumulates_pmu`

- **`src/cor/src/deployment.rs`** (minor fix) — Fixed `&usize` dereference bug in `CodeDelta::apply()` where `offset` and `end` were borrowed references used as slice indices; changed `offset` → `*offset` and `offset + len` → `*offset + len`

## 2026-03-05 — Task 3-31: SCG → MSG Conversion (retry of 2-28)

### Files Created
- **`src/vuma/src/scg_to_msg.rs`** — Full SCG → MSG conversion pipeline
  - `scg_to_msg(scg: &SCG) -> Result<MSG, ConversionError>` — primary entry point
  - Topological walk of SCG nodes ensuring all predecessors are converted first
  - **AllocationNode → Region** with monotonic address allocator (base `0x1_0000`)
  - **AccessNode → Derivation + Access** with proper kind (Read/Write) and size
  - **DeallocationNode → Region status Freed** with free_point set
  - **CastNode → DerivationKind::Cast** derivation from parent chain
  - **ComputationNode → passthrough Derivation** (Direct, forwarding provenance)
  - Pointer operations → Derivation with DerivationKind::Offset and provenance range
  - Memory operations → Access with AccessKind and byte size
  - ControlFlow edges between Access nodes → SyncEdge with HappensBefore
  - Post-conversion verification of all derivation chains
  - `ConversionError` enum: CycleDetected, UnknownRegion, UnknownAllocation,
    MissingDerivation, AccessRegionNotFound, CastWithoutParent,
    BrokenDerivationChain, InvalidProvenanceRange
  - 14 tests covering all conversion paths

### Files Modified
- **`src/vuma/src/lib.rs`** — `pub mod scg_to_msg;` already present (from prior attempt)

### Test Coverage (14 tests, all passing)
1. `test_allocation_creates_region_with_monotonic_address` — AllocationNode → Region
2. `test_allocation_creates_direct_derivation` — Direct Derivation from region
3. `test_access_creates_derivation_and_access_event` — AccessNode → Derivation + Access
4. `test_deallocation_marks_region_freed` — DeallocationNode → RegionStatus::Freed
5. `test_cast_creates_cast_derivation` — CastNode → DerivationKind::Cast
6. `test_control_flow_creates_sync_edge` — ControlFlow edge → SyncEdge
7. `test_monotonic_address_assignment` — Non-overlapping monotonic addresses
8. `test_access_with_offset_creates_offset_derivation` — Offset Derivation with range
9. `test_empty_scg_produces_empty_msg` — Edge case: empty graph
10. `test_all_derivation_chains_are_well_formed` — Verification pass
11. `test_concurrent_accesses_detected` — No sync edge → concurrent
12. `test_stack_deployment_produces_stack_region` — DeploymentTarget::Stack → RegionStatus::Stack
13. `test_computation_node_passthrough_derivation` — Computation passthrough
14. `test_gpu_deployment_produces_device_region` — DeploymentTarget::Gpu → RegionStatus::Device

## 2026-03-05 — Task 3-15: Pi 5 Link Script and Build System

### Files Created
- **`src/pi5/link.ld`** — ARM64 linker script for Raspberry Pi 5 bare-metal
  - `ENTRY(_start)` matching the `_start` naked function in `boot.rs`
  - RAM region: `0x80000`, 8 MiB (kernel load address per VideoCore convention)
  - MMIO region: `0x100000`, 1 MiB (memory-mapped I/O window)
  - Section order: `.text.boot` → `.text` → `.rodata` → `.data` → `.bss`
  - All sections aligned to 4 KiB page boundaries
  - Per-core stacks: 64 KiB × 4 cores = 256 KiB after BSS
  - Exports: `__bss_start`, `__bss_end`, `__stack_core0..3`, `__stack_start/end`, `__kernel_end`
  - Discards `.comment`, `.note.*`, `.eh_frame*`, `.ARM.exidx*`

- **`src/pi5/build.rs`** — Cargo build script for bare-metal `aarch64-unknown-none`
  - Only activates when `TARGET = aarch64-unknown-none` (no-op for hosted builds)
  - Sets `cargo:rerun-if-changed=link.ld`
  - Passes `-Tlink.ld`, `-nostartfiles`, `-static`, `-no-pie`
  - Generates `kernel8.map` linker map file

### Files Modified
- **`Makefile`** — Added Pi 5 bare-metal build infrastructure
  - `make pi5` — cross-compile `vuma-pi5` for `aarch64-unknown-none`
  - `make pi5-image` — ELF → raw binary (`kernel8.img`) via `aarch64-none-elf-objcopy`
  - `make pi5-flash` — copy `kernel8.img` to SD card (`SD=/mnt/sd-boot` overridable)
  - `make pi5-debug` — launch QEMU with `-s -S` for GDB debug on `:1234`
  - Variables: `PI5_TARGET`, `PI5_CROSS`, `PI5_OBJCOPY`, `PI5_ELF`, `PI5_IMG`, `SD`, `QEMU`

- **`src/pi5/Cargo.toml`** — Bare-metal compatibility
  - Added `build = "build.rs"`
  - `serde`: `default-features = false` for `no_std` compatibility
  - `log`: `default-features = false` for `no_std` compatibility

### Notes
- The linker script's `__bss_start`/`__bss_end` symbols are consumed by `boot.rs::_start`
  to zero the BSS section during early boot.
- The linker script defines 64 KiB per-core stacks; `boot.rs` currently uses 16 KiB
  (`STACK_SIZE_PER_CORE`). A follow-up task should update `boot.rs` to use the
  `__stack_coreN` symbols from the linker script for consistency.
- QEMU debug target uses `-M raspi3b` (closest available model); when QEMU adds
  native BCM2712 support, the machine flag should be updated.
