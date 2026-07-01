# VUMA Glossary

Definitions of terms used across the VUMA project. All counts and listings here are verified against the source code.

---

## Core Terms

### SCG
**Semantic Computation Graph** — The primary program representation. A directed, attributed multigraph (backed by `petgraph::DiGraph`) where nodes are operations, edges are relationships, and regions delineate scopes. The SCG **allows cycles** (loops, recursion) and handles them via `topological_sort_with_cycles()` using Tarjan's SCC algorithm. Implemented in `vuma-scg` (`src/scg/`).

- **NodeType** (26 variants, `src/scg/src/node.rs:41`): 14 core (Computation, Allocation, Deallocation, Access, Cast, Effect, Control, Phantom, VTable, ClosureEnv, StructDef, EnumDef, Match, ConstantTime) + 12 WOMB data-model (ConceptDecl, ConceptField, ConceptAccess, GestaltDecl, GestaltInterpret, ContextAssert, ManifoldDecl, ManifoldQuery, ManifoldSlice, AuraAttach, AuraQuery, AuraUpdate)
- **NodePayload** (26 variants, `src/scg/src/node.rs:187`): 1:1 with NodeType
- **EdgeKind** (7 variants, `src/scg/src/edge.rs:41`): DataFlow, ControlFlow, Derivation, Annotation, Dispatch, Call{from_node, to_node, caller_region}, Return{from_node, to_node, return_values: Vec<NodeId>}

### IVE
**Inference & Verification Engine** — The reasoning core. Reads the SCG, infers Behavioral Descriptors, constructs the MSG, and verifies the five VUMA invariants. Implemented in `vuma-ive` (`src/ive/`). A modular, per-function verification infrastructure exists in `src/ive/src/modular.rs` but is not integrated into the main pipeline.

### BD
**Behavioral Descriptor** — Replaces nominal types with the triple (RepD, CapD, RelD). Inferred from SCG structure, not declared. Implemented in `vuma-bd` (`src/bd/`).

### RepD
**Representation Descriptor** — Memory layout. 11 variants (`src/bd/src/repd.rs:191`):
- `Byte(ByteRep)` — raw byte sequence (size, align)
- `Struct(StructRep)` — named fields with offsets
- `Array(ArrayRep)` — fixed-count homogeneous
- `Enum(EnumRep)` — tagged union
- `Ptr(PtrRep)` — pointer to another representation
- `Union(UnionRep)` — untagged overlapping alternatives
- `Func(FuncRep)` — function signature
- `ManifoldSpatial(ManifoldSpatialRep)` — multi-dimensional data with space-filling curve layout
- `GestaltSuperposition(GestaltSuperpositionRep)` — tagless, context-dependent superposition
- `ConceptRelational(ConceptRelationalRep)` — relational data with lazily-inferred layout
- `Generic { name, constraints }` — generic type parameter with BD constraints

### CapD
**Capability Descriptor** — Permitted operations. 17 capabilities (`src/bd/src/capd.rs:50`):
Read, Write, Execute, Iterate, Send, Persist, Serialize, Deserialize, Hash, Compare, DerivePtr, Cast, Fork, Drop, Share, Move, Pin

### RelD
**Relational Descriptor** — Relationships. 6 relation kinds (`src/bd/src/reld.rs:112`):
- `Temporal(TemporalKind)` — Outlives / Coincides / Precedes / Succeeds
- `Containment` — one value nested inside another
- `Dependency(DepKind)` — DataDep / ControlDep / AliasDep
- `Equivalence` — observational equivalence
- `Security(FlowPolicy)` — NoDowngrade / NoCrossBoundary / Sanitized
- `Liveness` — value is eventually usable

### MSG
**Memory State Graph** — Captures every allocation, pointer derivation, deallocation, concurrent access, and reinterpretation. The formal model for verification. Implemented in `vuma-core` (`src/vuma/src/msg.rs`, `msg_builder.rs`, `msg_incremental.rs`).

### VUMA
**Verified-Unsafe Memory Access** — The framework name. Unsafe memory operations are made verifiable instead of forbidden.

### COR
**Continuous Optimization Runtime** — Maintains an always-compiled invariant: every reachable SCG region is kept in compiled machine code. Performs incremental compilation, PGO (4 optimization passes), speculative optimization. Implemented in `vuma-cor` (`src/cor/`). Partially integrated as `Option<CORuntime>` in the pipeline.

---

## The Five VUMA Invariants

Each invariant has two implementations: a simple MSG-only checker in `src/vuma/src/invariant_*.rs` and a richer path-sensitive verifier in `src/ive/src/*.rs` (aggregated by `src/ive/src/invariant_aggregator.rs`).

### Liveness
Every access targets allocated memory. Detects use-after-free, dangling pointers, uninitialized reads.
- Simple checker: `src/vuma/src/invariant_liveness.rs`
- Path-sensitive verifier: `src/ive/src/liveness.rs` (2,234 LOC)

### Exclusivity
No conflicting concurrent accesses. Detects data races (single-threaded currently; concurrent verification is planned).
- Simple checker: `src/vuma/src/invariant_exclusivity.rs`
- Path-sensitive verifier: `src/ive/src/exclusivity.rs` (1,585 LOC)

### Interpretation
Every access uses a valid representation. Detects type confusion (reading integer as pointer, uninitialized memory).
- Simple checker: `src/vuma/src/invariant_interpretation.rs`
- Path-sensitive verifier: `src/ive/src/interpretation.rs` (2,173 LOC)

### Origin
Every address traces to a valid allocation. Detects invalid pointer derivation.
- Simple checker: `src/vuma/src/invariant_origin.rs`
- Path-sensitive verifier: `src/ive/src/origin.rs` (1,781 LOC)

### Cleanup
Every region is eventually freed or explicitly leaked. Detects memory leaks, double-free, resource exhaustion.
- Simple checker: `src/vuma/src/invariant_cleanup.rs`
- Path-sensitive verifier: `src/ive/src/cleanup.rs` (1,538 LOC)

---

## Verification

### VerificationLevel
Two enums exist:
- `pipeline::VerificationLevel` (`src/pipeline.rs:127`, 4 variants): `None`, `Quick`, `Normal` (default), `Exhaustive` — controls whether IVE runs at all. `None` short-circuits before IVE.
- `ive::VerificationLevel` (`src/ive/src/invariant_aggregator.rs:101`, 3 variants): `Quick`, `Normal` (default), `Exhaustive` — controls IVE depth when it runs.

### Verification Debt
Unverified properties tracked with priorities: `Critical` (safety violations), `High` (security), `Medium`/`Low` (quality).

### Verification Confidence
`Full` (formal proof), `Partial` (most cases), `BestEffort` (empirical evidence).

### Counterexample
An execution path demonstrating an invariant violation.

---

## Code Generation

### Backend
A target architecture implementation of the `Backend` trait. VUMA has 10 backends (enum `BackendKind`): x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, x86_32, RISC-V 32, Wasm32. All are tier `Complete`. The CLI `vuma emit`/`vuma compile` commands accept only 8 ISAs (enum `IsaArg`, missing RISC-V 32 and x86_32); all 10 are exercised by the `compile_dump` test binary.

### IR
**Intermediate Representation** — Target-independent instruction set lowered from the SCG. Defined in `vuma-codegen::ir` (`src/codegen/src/ir.rs`).
- **IRType** (16 variants): I8, I16, I32, I64, U8, U16, U32, U64, F32, F64, Ptr, Void, Func, Struct, Array, TaggedUnion
- **IRInstr** (25 variants): Load, Store, BinOp, UnaryOp, Add, Sub, Mul, Div, Cmp, Branch, CondBranch, Call, Ret, Alloc, Free, Cast, Offset, GetAddress, Phi, Select, CtSelect, CtEq, AtomicLoad, AtomicStore, AtomicCas
- **BinOpKind** (25 variants): Add, Sub, Mul, SDiv, UDiv, SRem, URem, And, Or, Xor, Shl, ShrL, ShrA, Ror, Rol, SLt, SLe, SGt, SGe, ULt, ULe, UGt, UGe, Eq, Ne

### Regalloc
**Register Allocation** — Assigns virtual registers to physical registers. Linear-scan algorithm (a legacy greedy allocator is also retained). Implemented in `vuma-codegen::regalloc` (`src/codegen/src/regalloc.rs`).

### DWARF
Debug info format (v4, `DWARF_VERSION = 4` in `src/codegen/src/dwarf.rs`). VUMA emits `.debug_abbrev`, `.debug_info`, `.debug_line`, `.debug_frame` sections.

### FFI
**Foreign Function Interface** — `extern "C"` blocks for calling C functions and Linux syscalls. 19 syscalls (enum `SyscallName` in `src/ffi.rs:478`) across all 10 architectures. Implemented in `src/ffi.rs` (root crate).

---

## Calling Conventions

### AAPCS64
**ARM Architecture Procedure Call Standard** (64-bit) — Args in X0-X7, return in X0. Used by AArch64.

### AAPCS
**ARM Architecture Procedure Call Standard** (32-bit) — Args in R0-R3, remaining on stack. Used by ARM32.

### System V AMD64
x86_64 calling convention — Args in RDI, RSI, RDX, RCX, R8, R9, return in RAX.

### N64
MIPS64 calling convention — Args in $a0-$a7 ($4-$11).

### ELFv2
PPC64 calling convention — Args in R3-R10, TOC register. VUMA emits ELFv2 (`e_flags = 0x2`).

### cdecl
x86_32 calling convention — Args on stack, return in EAX.

### LP64 / ILP32
RISC-V data models: LP64 (64-bit long/pointer), ILP32 (32-bit int/long/pointer).

---

## Memory Barriers

### DMB / DSB / ISB
AArch64 barrier instructions: Data Memory Barrier, Data Synchronization Barrier, Instruction Synchronization Barrier.

### SYNC / LWSYNC
PPC64 barrier instructions: heavyweight sync, lightweight sync.

### DBAR
LoongArch64 barrier instruction.

### Fence
RISC-V memory fence instruction.

---

## Atomics

### LL/SC
**Load-Linked / Store-Conditional** — Atomic primitive used by AArch64 (LDXR/STXR), ARM32 (LDREX/STREX), MIPS64 (LL/SC), LoongArch64 (LL.W/SC.W), RISC-V (LR/SC).

### CAS
**Compare-And-Swap** — Atomic primitive. x86 uses LOCK CMPXCHG, Wasm uses i32.atomic.rmw.cmpxchg. VUMA exposes `AtomicLoad`, `AtomicStore`, `AtomicCas` as IR instructions and language expressions on all 10 backends.

---

## LLM Integration

### VumaForLLM
Stateless API for LLM agents. 7 public methods (`src/llm_api.rs`): `compile()`, `check()`, `analyze()`, `to_wasm()`, `explain_error()`, `suggest_fixes()`, `targets()`.

### VumaCompiler
Full pipeline API. 9 public methods (`src/api.rs`): `new()`, `with_config()`, `compile()`, `compile_for_target()`, `parse()`, `analyze()`, `available_targets()`, `validate()`, `verify()`.

### LSP
**Language Server Protocol** — IDE integration. 6 capabilities (`src/lsp/mod.rs`, 2,055 LOC): `textDocumentSync`, `completion`, `hover`, `definition`, `documentSymbol`, `semanticTokens`.
