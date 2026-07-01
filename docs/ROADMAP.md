# VUMA Roadmap

**Version:** 0.2.0-alpha.1
**Status:** Alpha — 10 backends at 100% gold-standard pass rate (with `--verification none`)

---

## Current State (July 2026)

### What Works

- **10 backend architectures** at 100% pass rate on the 5,738-program gold-standard suite (57,380/57,380 runs with `--verification none`): x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, x86_32, RISC-V 32, Wasm32
- **Parser** — lexer, AST, error recovery, AST-to-SCG lowering (286/286 tests pass)
- **SCG** — Semantic Computation Graph core (36/36 tests pass)
- **VUMA core** — MSG, invariants, region model (301/301 tests pass)
- **IVE** — five invariant verifiers with counterexample generation (has false positives on some valid programs)
- **BD** — Behavioral Descriptors (RepD, CapD, RelD) with inference (partially complete — M2.3 deferred)
- **Codegen** — 10 backends, register allocation, DWARF v4 debug info, FFI/syscalls
- **FFI** — 19+ Linux syscalls across all 10 architectures, `extern "C"` blocks
- **Atomics** — `AtomicLoad`, `AtomicStore`, `AtomicCas` on all 10 backends
- **Module system** — `import` with circular import detection
- **Package manager** — `vuma pkg init/build/add` (basic)
- **Womb stdlib** — 115 .vuma files, all compile on x86_64
- **Language features** — functions, structs, enums, match, if/while/for, imports, extern, type annotations
- **Heap allocation** — `__vuma_alloc` (mmap) and `__vuma_free` (munmap) work across function calls
- **Self-hosting infrastructure** — lexer, parser, IR builder, codegen, ELF writer modules exist in womb/ but are not tested end-to-end

### What Doesn't Work Yet

- **Self-hosting** — VUMA cannot compile itself. Individual pipeline modules exist but haven't been wired together end-to-end.
- **Verification** — `--verification normal` has false positives. Most programs use `--verification none`.
- **Type checking** — Parser recognizes syntax but doesn't perform semantic type validation.
- **BD inference (M2.3)** — Complex generic inference scenarios deferred.
- **Doubly-linked list verification (M2.4)** — Not fully verified.
- **Concurrent verification** — Single-threaded only.
- **COR runtime** — Not fully integrated end-to-end.
- **Standard library linking** — `vuma-std` Rust crate not linked. Womb modules exist but aren't auto-imported.
- **`map_device()`** — Referenced in examples but not a language feature.
- **`volatile`** — Not implemented.

---

## Milestones

### M1: Multi-Architecture Codegen ✅ Complete
- 10 backends, 100% gold-standard pass rate
- FFI, atomics, DWARF debug info

### M2: Verification Engine ⚠️ Partial
- M2.1 (Liveness, Origin, Cleanup): ✅ Working
- M2.2 (Exclusivity, Interpretation): ✅ Working
- M2.3 (Generic BD inference): ❌ Deferred
- M2.4 (Doubly-linked list verification): ❌ Partial
- False positives on valid programs using allocate/free

### M3: Language Features ⚠️ Partial
- Functions, structs, enums, match, if/while/for: ✅
- Imports, extern, type annotations: ✅
- Closures: ⚠️ Parsed but limited codegen
- Generics: ⚠️ Parsed but not monomorphized
- Type checking: ❌ Not implemented

### M4: Self-Hosting ❌ Not Started
- Womb modules (lexer, parser, IR, codegen, ELF) exist individually
- End-to-end pipeline not tested
- Bootstrap orchestrator not implemented

---

## Next Steps

1. **Fix verification false positives** — The IVE rejects valid programs using allocate/free with dereference
2. **End-to-end pipeline test** — Wire womb/lang modules together to compile "fn main() { return 42; }"
3. **Type checking** — Add semantic validation (type compatibility, scoping, function signatures)
4. **IVE modular integration** — Connect `src/ive/src/modular.rs` to the main pipeline
5. **Bootstrap** — Implement `vuma bootstrap` to compile VUMA with VUMA
