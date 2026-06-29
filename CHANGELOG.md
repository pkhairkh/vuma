# VUMA Changelog

All notable changes to the VUMA project.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.1.0-alpha.1] — 2026-06-28

Alpha release with 10 backend architectures at 100% gold-standard pass rate.

### Added

- **10 backend architectures**: x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, x86_32, RISC-V 32, Wasm32
- **5,738-program gold-standard test suite** at 100% pass rate (57,380/57,380 runs across all 10 backends)
- **FFI**: 19 Linux syscalls across all 10 architectures, `extern "C"` blocks, architecture-specific relocations
- **Atomics**: `AtomicLoad`, `AtomicStore`, `AtomicCas` on all 10 backends
- **FP conversions**: `IntToFloat`, `UIntToFloat`, `FloatToInt`, `FloatToUInt`, `FloatToFloat` on all 10 backends
- **Constant-time crypto**: `ct_select`, `ct_eq`, `ct_ne`, `ct_lt`, `ct_gte`
- **DWARF v4 debug info**: `.debug_abbrev`, `.debug_info`, `.debug_line`, `.debug_frame`
- **66 diagnostic codes**: E000–E050, W001–W010, I001–I005 with error chaining
- **LLM API**: `VumaForLLM` with `compile()`, `check()`, `analyze()`, `to_wasm()`, `explain_error()`, `suggest_fixes()`
- **LSP server**: diagnostics, hover, go-to-definition, completion, semantic tokens
- **REPL**: `:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`
- **Module system**: `import` with circular import detection
- **Package manager**: `vuma pkg init/build/add`
- **11 workspace crates**: scg, ive, vuma (core), bd, codegen, parser, cor, proof, std, tests, package

### Fixed

- **mips64 MAP_ANONYMOUS**: Fixed flag value from 0x22 (x86) to 0x802 (MIPS where MAP_ANONYMOUS=0x800)
- **wasm32 __vuma_alloc**: Dynamic-size allocations now use bump allocator instead of resolving to return-(-1) stub
- **ppc64 enum_demo**: Big-endian U8/U32 mismatch fixed via byte-level access
- **x86_32 stack-passed args**: Args 4+ now loaded from [EBP+8+(i-4)*4]
- **x86_32 EBP clobber**: mmap/futex stubs now save/restore EBP frame pointer
- **x86_32 EDX high word**: Removed incorrect EDX store after function calls
- **x86_32 store_vreg**: Zeroes high word after 32-bit stores
- **riscv32/arm32 64-bit return values**: Load both words for 64-bit returns, sign-extend extern returns
- **wasm32 stub**: Unknown extern functions resolve to stub returning -1
- **While-loop for-range guard**: Skip conversion when body reassigns variables
- **Continue block**: Always create continue block for all loops
- **mmap_sha256d race condition**: PID-based unique temp file path
- **lower_computation prev_vreg**: Removed incorrect lhs-based vreg remapping that treated let-bindings as reassignments

### Changed

- **FFI Arch enum**: Added `X86_32` and `RiscV32` variants with correct syscall tables
- **README**: Complete rewrite to remove AI-generated fluff, fix factual errors, update repo URL
- **Documentation**: All "8 backends" references updated to "10 backends" across docs/

---

## [0.1.0-alpha.0] — 2026-06-16

Initial alpha pre-release.

### Added

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
