# VUMA Releases

This document summarizes each VUMA release with key changes and known limitations.

---

## v0.2.0 — 2026-03-05

**Phase 2: Multi-Architecture Codegen, LLM Integration, Wasm Sandbox**

This release represents the substantial completion of Phase 2 of the VUMA framework. The project now supports 8 backend architectures, provides comprehensive LLM integration, and includes a Wasm32 sandbox for safe LLM-generated code execution.

### Highlights

- **8 Backend Architectures**: x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, Wasm32 — all passing SHA256d or individual operation tests
- **LLM Integration**: VumaForLLM API, VumaCompiler API, LSP server, enhanced REPL (`:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`), parser hardened for LLM-generated code
- **Wasm32 Sandbox**: LLM agents compile to safe, sandboxed WebAssembly modules
- **Memory Safety**: 10 violation types (E041–E050), compile-time checks, runtime bounds checking (`--safe`)
- **Constant-Time Crypto**: Branchless `ct_select`, `ct_eq`, `ct_ne`, `ct_lt`, `ct_gte` across all 8 backends
- **Module System**: Multi-file compilation with `import "path"::{names};` and circular import detection
- **Package Manager**: Foundation with `vuma pkg init/build/add` and dependency resolution
- **FFI & Syscalls**: 19 Linux syscalls across 8 architectures, architecture-specific relocations
- **Register Allocator**: Loop-aware spill weights, GreedyRegCache, dead-vreg reuse
- **Diagnostics**: 65 diagnostic codes (E001–E050, W001–W010, I001–I005), error chaining, structured suggestions, 4 output formats (JSON, ANSI, plain text, LSP)
- **CI**: GitHub Actions with cross-compile matrix for all 8 ISA targets

### New Features (since v0.1.0)

#### Multi-Architecture Codegen
- All 6 native backends pass SHA256d: x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64
- LoongArch64 deep audit: fixed 7 encoding/ABI bugs, added maskeqz/masknez instructions
- PPC64 deep audit: fixed LR save offset, CMP l-field, RLDCL/RLDCR opcode, mb5/me5
- Wasm32 generates valid modules with 12-section validation
- Cross-backend consistency tests (9 tests × 8 backends)
- ELF validation for all 7 native backends (ELF32/64, endianness, machine types)
- ABI conformance testing (27 tests covering calling conventions)
- ELF emission: 3 LOAD segments (W^X), per-arch section alignment

#### LLM Integration
- `VumaForLLM`: `compile()`, `check()`, `analyze()`, `to_wasm()`, `explain_error()`, `suggest_fixes()`, `targets()`
- `VumaCompiler`: `compile()`, `parse()`, `analyze()`, `validate()`, `verify()`
- LSP server: diagnostics, hover, go-to-definition, completion, semantic tokens
- REPL: `:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`, tab completion, ANSI color
- Parser hardening: LLM type aliases (int→i32), macro detection (println!), C-style for loop, &T→*T

#### Verification & Safety
- `VumaCompiler.verify()` with per-invariant pass/fail and counterexamples
- Property-based testing (15 proptests across 6 categories)
- Memory safety analyzer: 10 violation types, compile-time and runtime checks
- 65 diagnostic codes with error chaining and structured suggestions
- Interprocedural analysis, escape analysis, verification cache

#### Standard Library
- `crypto.rs`: SHA-256 constants, host-side helpers, constant-time operations
- `string.rs`: strlen, strcmp, memcpy, memset
- `math.rs`: abs, min, max, clamp
- `alloc.rs`: heap_alloc, heap_free, heap_realloc
- `io.rs`: read_bytes, write_bytes, read_u32_le, write_u32_le

#### Infrastructure
- GitHub Actions CI with 8-target cross-compile matrix
- DWARF debug info with per-backend configuration
- `--debug-info`, `--safe`, `--bench`, `--sections` CLI flags
- Package manager: `vuma pkg init/build/add`
- Module system: `import "path"::{names};`

### Known Limitations

- BD inference completeness (M2.3) deferred to Phase 3
- Doubly-linked list verification (M2.4) deferred to Phase 3
- ARM64 atomics and concurrent verification are Phase 3 targets
- COR end-to-end integration not yet complete
- LoongArch64 full SHA256d too slow for QEMU (should work natively)

---

## v0.1.0 — 2026-03-05

**Phase 1: Foundation**

Initial release of the VUMA framework. This release establishes the complete architectural foundation: all 12 workspace crates, 15 formal specifications, 10 example programs, a comprehensive benchmark suite, and full documentation.

### Highlights

- **12 Workspace Crates**: scg, bd, vuma, ive, cor, projection, parser, codegen, proof, std, tests, package
- **5 VUMA Invariants**: Liveness, Exclusivity, Interpretation, Origin, Cleanup — all verifiable end-to-end
- **8 Backend Architectures**: x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, Wasm32
- **15 Formal Specifications**: SCG, RepD, CapD, RelD, invariants, MSG, codegen, benchmarks, proofs, decidability
- **Full Verification Pipeline**: SCG → MSG → IVE verification with counterexample generation
- **Proof System**: Formal proofs, checker, tactics, counterexample generation
- **Standard Library**: Ptr, RegionPtr, Slice, Vec, HashMap, VumaString, Mutex, RwLock, Channel
- **10 Example Programs**: hello_memory, doubly_linked_list, arena_allocator, gpio_blink, lock_free_queue, etc.

### Known Limitations

- Concurrent verification limited to single-threaded programs
- ARM64 codegen does not support atomic instructions
- COR not yet integrated end-to-end
- Parser has known type mismatches in AST→SCG lowering
- LoongArch64 and Wasm32 backends need further hardening
