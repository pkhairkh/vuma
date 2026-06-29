# VUMA Releases

Release summaries with key changes and known limitations.

---

## v0.1.0-alpha.1 — 2026-06-28

**10 backends at 100% gold-standard pass rate**

### Highlights

- **10 backend architectures** at 100% pass rate on the 5,738-program gold-standard suite (57,380/57,380 runs): x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, x86_32, RISC-V 32, Wasm32
- **FFI**: 19 Linux syscalls across all 10 architectures, `extern "C"` blocks
- **Atomics**: `AtomicLoad`, `AtomicStore`, `AtomicCas` on all 10 backends
- **FP conversions**: `IntToFloat`, `UIntToFloat`, `FloatToInt`, `FloatToUInt`, `FloatToFloat`
- **Constant-time crypto**: `ct_select`, `ct_eq`, `ct_ne`, `ct_lt`, `ct_gte`
- **DWARF v4 debug info**: Per-backend address size and instruction length
- **66 diagnostic codes** with error chaining
- **LLM API**: `VumaForLLM` with compile/check/analyze/to_wasm/explain_error/suggest_fixes
- **LSP server**: Full protocol support
- **REPL**: `:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`
- **Module system**: `import` with circular import detection
- **Package manager**: `vuma pkg init/build/add`

### Key Bug Fixes

- mips64 `MAP_ANONYMOUS` flag: 0x22 (x86 value) → 0x802 (MIPS value where MAP_ANONYMOUS=0x800)
- wasm32 `__vuma_alloc`: Dynamic-size allocations now use bump allocator
- ppc64 enum_demo: Big-endian U8/U32 mismatch fixed via byte-level access
- x86_32: Stack-passed args, EBP clobber, EDX high word, store_vreg high-word zeroing
- riscv32/arm32: 64-bit return value handling
- lower_computation: Fixed prev_vreg remapping that treated let-bindings as reassignments

### Known Limitations

- **Self-hosting**: VUMA cannot compile itself; the compiler is written in Rust
- **Stdlib is host-side**: Math, fmt, string, crypto execute on host (Rust), not compiled to target
- **BD inference completeness**: Some complex scenarios deferred
- **Doubly-linked list verification**: Full verification not yet complete
- **Concurrent verification**: Limited to single-threaded programs
- **COR end-to-end**: Continuous Optimization Runtime not fully integrated

---

## v0.1.0-alpha.0 — 2026-06-16

**Initial alpha pre-release**

### Highlights

- SCG (Semantic Computation Graph) core
- IVE (Inference & Verification Engine) with five invariants
- BD (Behavioral Descriptors) with RepD/CapD/RelD
- MSG (Memory State Graph) construction
- Parser with lexer, AST, error recovery
- 8 initial backend architectures (x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, Wasm32)
- Proof system with counterexamples
- Standard library (host-side)
- LLM API and LSP server
- 15 formal specification documents
