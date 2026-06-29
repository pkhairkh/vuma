# VUMA Glossary

Definitions of terms used across the VUMA project.

---

## Core Terms

### SCG
**Semantic Computation Graph** — The primary program representation. A directed, acyclic, attributed multigraph where nodes are operations, edges are relationships, and regions delineate scopes. Implemented in `vuma-scg`.

### IVE
**Inference & Verification Engine** — The reasoning core. Reads the SCG, infers Behavioral Descriptors, constructs the MSG, and verifies the five VUMA invariants. Implemented in `vuma-ive`.

### BD
**Behavioral Descriptor** — Replaces nominal types with the triple (RepD, CapD, RelD). Inferred from SCG structure, not declared. Implemented in `vuma-bd`.

### RepD
**Representation Descriptor** — Memory layout: size, alignment, field offsets, multiple simultaneous interpretations.

### CapD
**Capability Descriptor** — Permitted operations: read, write, execute, serialize, send, persist, derive-pointer. Context-dependent.

### RelD
**Relational Descriptor** — Relationships: temporal co-occurrence, structural containment, dependency ordering, semantic equivalence, security-level flow.

### MSG
**Memory State Graph** — Captures every allocation, pointer derivation, deallocation, concurrent access, and reinterpretation. The formal model for verification. Implemented in `vuma-core`.

### VUMA
**Verified-Unsafe Memory Access** — The framework name. Unsafe memory operations are made verifiable instead of forbidden.

### COR
**Continuous Optimization Runtime** — Maintains an always-compiled invariant: every reachable SCG region is kept in compiled machine code. Performs incremental compilation, PGO, speculative optimization. Implemented in `vuma-cor`.

---

## The Five VUMA Invariants

### Liveness
Every access targets allocated memory. Detects use-after-free, dangling pointers, uninitialized reads.

### Exclusivity
No conflicting concurrent accesses. Detects data races (single-threaded currently; concurrent verification is planned).

### Interpretation
Every access uses a valid representation. Detects type confusion (reading integer as pointer, uninitialized memory).

### Origin
Every address traces to a valid allocation. Detects invalid pointer derivation.

### Cleanup
Every region is eventually freed or explicitly leaked. Detects memory leaks, double-free, resource exhaustion.

---

## Verification

### Verification Debt
Unverified properties tracked with priorities: `Critical` (safety violations), `High` (security), `Medium`/`Low` (quality).

### Verification Confidence
`Full` (formal proof), `Partial` (most cases), `BestEffort` (empirical evidence).

### Counterexample
An execution path demonstrating an invariant violation.

---

## Code Generation

### Backend
A target architecture implementation of the `Backend` trait. VUMA has 10 backends: x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, x86_32, RISC-V 32, Wasm32.

### IR
**Intermediate Representation** — Target-independent instruction set lowered from the SCG. Defined in `vuma-codegen::ir`.

### Regalloc
**Register Allocation** — Assigns virtual registers to physical registers. Linear-scan algorithm. Implemented in `vuma-codegen::regalloc`.

### DWARF
Debug info format (v4). VUMA emits `.debug_abbrev`, `.debug_info`, `.debug_line`, `.debug_frame` sections.

### FFI
**Foreign Function Interface** — `extern "C"` blocks for calling C functions and Linux syscalls. 19 syscalls across 10 architectures. Implemented in `src/ffi.rs`.

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
PPC64 calling convention — Args in R3-R10, TOC register.

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

## Atomies

### LL/SC
**Load-Linked / Store-Conditional** — Atomic primitive used by AArch64 (LDXR/STXR), ARM32 (LDREX/STREX), MIPS64 (LL/SC), LoongArch64 (LL.W/SC.W), RISC-V (LR/SC).

### CAS
**Compare-And-Swap** — Atomic primitive. x86 uses LOCK CMPXCHG, Wasm uses i32.atomic.rmw.cmpxchg.

---

## LLM Integration

### VumaForLLM
Stateless API for LLM agents: `compile()`, `check()`, `analyze()`, `to_wasm()`, `explain_error()`, `suggest_fixes()`. Implemented in `src/llm_api.rs`.

### VumaCompiler
Full pipeline API: `compile()`, `parse()`, `analyze()`, `validate()`, `verify()`. Implemented in `src/api.rs`.

### LSP
**Language Server Protocol** — IDE integration for diagnostics, hover, go-to-definition, completion. Implemented in `src/lsp/`.
