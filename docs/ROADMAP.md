# VUMA Roadmap

**Version:** 0.2.0-alpha.2
**Status:** Alpha — research prototype with working verification.

---

## Current State (July 2026)

### What Works (empirically verified)

- **10 backend architectures emit code** — 5,745 test programs × 10 backends = 57,450 runs, **100.00% pass rate**. All backends produce correct exit codes under QEMU user-mode emulation and wasmtime.
- **IVE verification** — **100.00% pass rate** (57,449/57,449 IVE runs at `--verification normal`). All 5 invariants (Liveness, Exclusivity, Interpretation, Origin, Cleanup) pass on every test program. The test suite's `--verify` flag reports IVE pass rate.
- **IVE unit tests** — 237/237 pass.
- **Parser** — lexer (141 token kinds), AST (17 Item / 19 Stmt / 33 Expr / 8 Type variants), error recovery, AST-to-SCG lowering (325 unit tests pass). **But** `concept`/`gestalt`/`manifold`/`aura` are tokenized but never parsed, and `else { if … } else { … }` chains are rejected.
- **SCG** — Semantic Computation Graph core (26 NodeType, 7 EdgeKind; petgraph-backed; transform passes including InterproceduralAllocFlow) (191+ unit tests pass).
- **VUMA core** — MSG, invariants, region model, access analysis, security model, REPL (301+ unit tests pass).
- **BD** — Behavioral Descriptors (RepD 11, CapD 17, RelD 6) with inference (342+ unit tests pass). M2.3 generic inference deferred.
- **FFI** — 19+ Linux syscalls, `extern "C"` blocks emit relocations on all 10 backends. ppc64 big-endian syscall stubs (pipe, execve, waitpid, strcmp) correctly byte-swap for inter-process communication.

### What Doesn't Work (remaining limitations)

- **Self-hosting** — `src/bootstrap/vuma_compiler.vuma` (730 LOC, lexer-only) does not compile. `womb/lang/vuma_compiler.vuma` (506 LOC) is in the set that doesn't parse. Self-hosting is at <5%.
- **Womb data-model layer** — `concept`/`gestalt`/`manifold`/`aura` tokenized but never parsed. The entire Womb frontend is a gap.
- **Parser `else-if` chains** — `else { if … } else { … }` rejected; 6/16 `womb/lang/*.vuma` fail to parse.
- **Type checking** — Parser recognizes syntax but doesn't validate types.
- **Concurrent verification** — Single-threaded only.
- **COR runtime** — Partially integrated (`Option<CORuntime>`).
- **Standard library linking** — `vuma-std` Rust crate not linked to VUMA programs. Womb modules not auto-imported.
- **`map_device()` / `volatile`** — Not implemented (referenced in example comments only).

---

## Milestones

### M1: Multi-Architecture Codegen ✅ Complete
- 10 backends emit code, **100.00% gold-standard pass rate** (57,450/57,450)
- FFI (19+ syscalls), atomics, DWARF v4 debug info
- ppc64 big-endian syscall stubs with byte-swapping
- wasm32 self_exec properly skipped (architecturally impossible)

### M2: Verification Engine ✅ Complete
- M2.1 (Liveness, Origin, Cleanup): ✅ **100% pass** — Liveness CFG includes Derivation edges, skips FunctionReturn as leak endpoints; Origin accepts zero-size provenance ranges; Cleanup uses NodeId instead of region_id
- M2.2 (Exclusivity, Interpretation): ✅ **100% pass** — Exclusivity uses per-allocation conflict detection
- M2.3 (Generic BD inference): ❌ Deferred
- M2.4 (Interprocedural analysis): ✅ InterproceduralAllocFlow pass connects factory-function allocations
- **Flagship feature works end-to-end at 100%**

### M3: Language Features ⚠️ Partial
- Functions, structs, enums, match, if/while/for: ✅ (within the gold-standard suite)
- Imports, extern, type annotations: ✅
- Closures: ⚠️ Parsed but limited codegen
- Generics: ⚠️ Parsed but not monomorphized
- Type checking: ❌ Not implemented
- `concept`/`gestalt`/`manifold`/`aura`: ❌ Tokenized but never parsed
- `else { if … } else { … }` chains: ❌ Parser rejects

### M4: Self-Hosting ❌ Not Achievable Yet
- `src/bootstrap/vuma_compiler.vuma` (730 LOC lexer POC) does not compile
- `womb/lang/vuma_compiler.vuma` (506 LOC) does not parse (in the 6/16 broken set)
- End-to-end pipeline not testable until parser gaps and codegen bridges are fixed

---

## Next Steps (priority order)

1. **Fix the parser to accept `else { if … } else { … }` chains** (or rewrite the 6 broken `womb/lang` files to use `else if`).
2. **Implement `concept`/`gestalt`/`manifold`/`aura` parsing** (or remove them from the lexer/AST/SCG). The Womb layer is currently a ghost.
3. **Unify the two codegen bridges** — either make `bridge_ast_to_codegen_scg` route through verification, or make the canonical `bridge_scg_to_codegen` produce correct code.
4. **Implement type checking** — parser recognizes syntax but doesn't validate types.
5. **Rewrite the bootstrap lexer** to actually parse (not just lex), and fix the `src_len_global` bug.
6. **Expose RISC-V 32 and x86_32 in the CLI** (`IsaArg`).
7. **Standard library linking** — auto-import womb modules.
8. **Concurrent verification** — multi-threaded IVE.
