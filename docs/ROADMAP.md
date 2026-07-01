# VUMA Roadmap

**Version:** 0.2.0-alpha.1
**Status:** Alpha — 10 backends at 99.99% gold-standard pass rate (57,377/57,380 runs with `--verification none`)

---

## Current State (July 2026)

### What Works

- **10 backend architectures** at 99.99% pass rate on the 5,738-program gold-standard suite (57,377/57,380 runs with `--verification none`): x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, x86_32, RISC-V 32, Wasm32. Three failures: `crc32.vuma` on riscv64+ppc64, `s27_fn_two_args_mod.vuma` on ppc64.
- **Parser** — lexer (141 token kinds), AST (17 Item / 19 Stmt / 33 Expr / 8 Type variants), error recovery, AST-to-SCG lowering (325 tests pass)
- **SCG** — Semantic Computation Graph core (26 NodeType, 7 EdgeKind variants; petgraph-backed, allows cycles; transform passes: ConstantFolding, DCE, CSE, InliningPass, LICM, StrengthReduction, DRE, TailCallOpt) (191 tests pass)
- **VUMA core** — MSG, invariants, region model, access analysis, security model, REPL (301 tests pass)
- **IVE** — five invariant verifiers with counterexample generation (has false positives on some valid programs; modular.rs exists but is not integrated)
- **BD** — Behavioral Descriptors (RepD 11 variants, CapD 17 capabilities, RelD 6 relation kinds) with inference (partially complete — M2.3 deferred)
- **Codegen** — 10 backends, register allocation (linear-scan), DWARF v4 debug info, FFI/syscalls (105K LOC, 1,061 tests)
- **FFI** — 19 Linux syscalls (enum `SyscallName`) across all 10 architectures, `extern "C"` blocks
- **Atomics** — `AtomicLoad`, `AtomicStore`, `AtomicCas` on all 10 backends
- **Module system** — `import` with circular import detection (`ResolveError::CircularImport`)
- **Package manager** — `vuma pkg init/build/add` (basic)
- **Womb stdlib** — 115 .vuma files (114 compilable on x86_64; `core.vuma` is a design spec)
- **Language features** — functions, structs, enums, match, if/while/for, imports, extern, type annotations, traits/impls, closures (parsed), sync blocks, BD directives
- **Heap allocation** — `__vuma_alloc` (mmap on 9/10 backends, bump allocator on Wasm32) and `__vuma_free` work across function calls
- **Self-hosting infrastructure** — `src/bootstrap/vuma_compiler.vuma` (730 LOC lexer POC) and `womb/lang/vuma_compiler.vuma` (506 LOC full pipeline) exist; not verified end-to-end by automated tests

### What Doesn't Work Yet

- **Self-hosting** — VUMA cannot compile itself end-to-end. Lexer POC and a full-pipeline attempt exist but are not tested by the automated harness.
- **Verification** — `--verification normal` has false positives. Most programs use `--verification none`.
- **Type checking** — Parser recognizes syntax but doesn't perform semantic type validation.
- **BD inference (M2.3)** — Complex generic inference scenarios deferred (`instantiate_generic` does shallow substitution only).
- **Doubly-linked list verification (M2.4)** — Hand-built tests exist (`src/tests/src/dlist.rs`); not fully verified.
- **Concurrent verification** — Single-threaded only.
- **COR runtime** — Partially integrated (`Option<CORuntime>` field in pipeline).
- **Standard library linking** — `vuma-std` Rust crate not linked to VUMA programs. Womb modules exist but aren't auto-imported.
- **`map_device()`** — Referenced in examples but not a language feature.
- **`volatile`** — Not implemented.
- **CLI ISA coverage** — `vuma emit`/`vuma compile` accept 8 ISAs (missing RISC-V 32, x86_32); all 10 are tested via `compile_dump`.
- **`womb/core.vuma`** — Design spec only, not compilable.

---

## Milestones

### M1: Multi-Architecture Codegen ✅ Complete
- 10 backends, 99.99% gold-standard pass rate (57,377/57,380)
- FFI (19 syscalls), atomics, DWARF v4 debug info

### M2: Verification Engine ⚠️ Partial
- M2.1 (Liveness, Origin, Cleanup): ✅ Working
- M2.2 (Exclusivity, Interpretation): ✅ Working
- M2.3 (Generic BD inference): ❌ Deferred
- M2.4 (Doubly-linked list verification): ❌ Partial
- False positives on valid programs using allocate/free
- modular.rs (per-function incremental verification) exists but is not wired in

### M3: Language Features ⚠️ Partial
- Functions, structs, enums, match, if/while/for: ✅
- Imports, extern, type annotations: ✅
- Closures: ⚠️ Parsed but limited codegen
- Generics: ⚠️ Parsed but not monomorphized
- Type checking: ❌ Not implemented

### M4: Self-Hosting ⚠️ Started
- `src/bootstrap/vuma_compiler.vuma` (730 LOC lexer POC) exists
- `womb/lang/vuma_compiler.vuma` (506 LOC full pipeline) exists
- End-to-end pipeline not verified by automated tests
- Bootstrap orchestrator not implemented

---

## Next Steps

1. **Fix verification false positives** — The IVE rejects valid programs using allocate/free with dereference
2. **Fix the 3 remaining test failures** — crc32 on riscv64/ppc64, s27_fn_two_args_mod on ppc64
3. **End-to-end pipeline test** — Wire womb/lang modules together to compile "fn main() { return 42; }"
4. **Type checking** — Add semantic validation (type compatibility, scoping, function signatures)
5. **IVE modular integration** — Connect `src/ive/src/modular.rs` to the main pipeline
6. **Expose RISC-V 32 and x86_32 in the CLI** — Add missing `IsaArg` variants
7. **Bootstrap** — Implement `vuma bootstrap` to compile VUMA with VUMA
