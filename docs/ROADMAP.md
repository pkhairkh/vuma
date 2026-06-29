# VUMA Roadmap

**Version:** 0.1.0-alpha.1
**Status:** Alpha — 10 backends at 100% gold-standard pass rate

---

## Current State (June 2026)

### What Works

- **10 backend architectures** at 100% pass rate on the 5,738-program gold-standard suite (57,380/57,380 runs): x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, x86_32, RISC-V 32, Wasm32
- **Parser** — lexer, AST, error recovery, AST-to-SCG lowering
- **SCG** — Semantic Computation Graph core (nodes, edges, regions, queries, dominance, liveness, transforms)
- **IVE** — five invariant verifiers (liveness, exclusivity, interpretation, origin, cleanup) with counterexample generation
- **BD** — Behavioral Descriptors (RepD, CapD, RelD) with inference
- **Codegen** — 10 backends, register allocation, DWARF v4 debug info, FFI/syscalls
- **LLM API** — `VumaForLLM` with `compile()`, `check()`, `analyze()`, `to_wasm()`, `explain_error()`, `suggest_fixes()`
- **LSP server** — diagnostics, hover, go-to-definition, completion
- **REPL** — `:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`
- **FFI** — 19 Linux syscalls across all 10 architectures, `extern "C"` blocks
- **Atomics** — `AtomicLoad`, `AtomicStore`, `AtomicCas` on all 10 backends
- **FP conversions** — `IntToFloat`, `UIntToFloat`, `FloatToInt`, `FloatToUInt`, `FloatToFloat`
- **Constant-time crypto** — `ct_select`, `ct_eq`, `ct_ne`, `ct_lt`, `ct_gte`
- **Module system** — `import` with circular import detection
- **Package manager** — `vuma pkg init/build/add`
- **Proof system** — formal proofs, checker, tactics, counterexamples

### What Doesn't Work Yet

- **Self-hosting** — VUMA cannot compile itself; the compiler is written in Rust
- **Womb data models** — `concept`/`gestalt`/`manifold`/`aura` keywords parse but produce no code; `womb/core.vuma` is unimplemented design pseudocode
- **Standard library** — `vuma-std` Rust crate exists (24K lines) but is not linked to VUMA programs; functions are not callable from VUMA source
- **BD inference completeness** — some complex scenarios deferred
- **Doubly-linked list verification** — full verification not yet complete
- **Concurrent verification** — limited to single-threaded programs
- **COR end-to-end** — Continuous Optimization Runtime not fully integrated

---

## Phases

### Phase 1: Foundation (Complete)

Core data structures, memory model, multi-architecture codegen, parser, verification pipeline.

| Milestone | Description | Status |
|-----------|-------------|--------|
| M1.1 | SCG core types, construction, serialization | ✅ Complete |
| M1.2 | MSG construction from SCG, derivation tracking | ✅ Complete |
| M1.3 | IVE liveness and origin verification | ✅ Complete |
| M1.4 | Multi-architecture codegen: 10 backends | ✅ Complete |
| M1.5 | Parser with lexer, AST, error recovery, AST-to-SCG | ✅ Complete |
| M1.6 | Proof system with formal proofs, counterexamples | ✅ Complete |

### Phase 2: Core Implementation (Substantially Complete)

Verification engine, BD inference, multi-architecture codegen for complex programs, LLM integration.

| Milestone | Description | Status |
|-----------|-------------|--------|
| M2.1 | Exclusivity and interpretation verification | ✅ Complete |
| M2.2 | Cleanup verification and full invariant pipeline | ✅ Complete |
| M2.3 | BD inference subsumes Rust type system | 📋 Deferred |
| M2.4 | Doubly-linked list verified by IVE | 📋 Deferred |
| M2.5 | Multi-architecture codegen handles complex programs | ✅ Complete |
| M2.6 | Profile-guided optimization | ✅ Complete |
| M2.7 | LLM API (`VumaCompiler`) | ✅ Complete |
| M2.8 | LSP server | ✅ Complete |
| M2.9 | REPL with LLM-friendly commands | ✅ Complete |
| M2.10 | Module resolution system | ✅ Complete |
| M2.11 | Wasm32 sandbox compilation | ✅ Complete |

### Phase 3: Hardening & Optimization (In Progress)

Concurrency support, atomics, barriers, COR integration, verification hardening.

| Milestone | Description | Status |
|-----------|-------------|--------|
| M3.1 | Atomic instructions on all backends | ✅ Complete |
| M3.2 | Concurrent exclusivity verification | 📋 Planned |
| M3.3 | COR integration: incremental compilation, PGO | 🔄 In Progress |
| M3.4 | Full peripheral support (GPIO, UART, I2C, SPI) | 📋 Planned |
| M3.5 | Lock-free data structure verified | 📋 Planned |
| M3.6 | Verification pipeline hardening | ✅ Complete |
| M3.7 | Cross-backend validation | ✅ Complete |
| M3.8 | Diagnostics system (66 codes) | ✅ Complete |

### Phase 4: Ecosystem (Planned)

- Outcome spaces
- Expanded stdlib compiled to target (not host-side)
- Full ecosystem tooling

### Phase 5: Self-Hosting (Planned)

- VUMA compiler verifies and compiles itself
- The compiler is currently written in Rust; self-hosting requires implementing the full compiler pipeline in VUMA

---

## LLM Integration

VUMA provides programmatic interfaces for AI agents:

| Interface | Description |
|-----------|-------------|
| `VumaForLLM` API | `compile()`, `check()`, `analyze()`, `to_wasm()`, `explain_error()`, `suggest_fixes()` |
| `VumaCompiler` API | `compile()`, `parse()`, `analyze()`, `validate()`, `verify()` |
| LSP Server | Diagnostics, hover, go-to-definition, completion |
| REPL `:wasm` | Compile to Wasm32, show binary size |
| REPL `:backends` | List 10 backends with status |
| REPL `:check` | Run IVE verification |
| REPL `:diagnostics` | Show all diagnostics as JSON |
| REPL `:exports` | List all function signatures |

The Wasm32 backend enables LLM agents to compile VUMA programs into sandboxed WebAssembly modules — the recommended execution path for LLM-generated code.

---

## Risk Mitigation

| Risk | Mitigation |
|------|-----------|
| IVE verification too slow for large programs | Verification cache and incremental verification implemented |
| Backend instruction encoding bugs | Cross-backend validation tests, QEMU testing, 5,738-program gold-standard suite |
| BD inference incompleteness | Fallback to explicit annotations; iterative refinement |
| Concurrent verification undecidability | Limit to finite-state abstraction; tiered verification confidence |
| Self-hosting complexity | Incremental approach: self-verify subsystem by subsystem |
