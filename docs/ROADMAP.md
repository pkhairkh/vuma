# VUMA Roadmap

**Version:** 0.2.0-alpha.1
**Status:** Alpha — research prototype. The "99.99% gold-standard pass rate" measures only that 5,738 tiny test programs exit with the expected code under `--verification none`; it does **not** mean the verifier works, that emitted binaries are correct in general, or that the language is usable for real programs. See "Critical Known Issues" in `README.md` for the full blocking list.

---

## Current State (July 2026)

### What Actually Works (empirically verified)

- **10 backend architectures emit code** — 5,738 tiny test programs exit with the expected code under `--verification none` (57,377/57,380 = 99.99%). 3 failures: `crc32.vuma` (riscv64, ppc64), `s27_fn_two_args_mod.vuma` (ppc64). This is the only thing that genuinely works end-to-end.
- **Parser** — lexer (141 token kinds), AST (17 Item / 19 Stmt / 33 Expr / 8 Type variants), error recovery, AST-to-SCG lowering (325 unit tests pass). **But** `concept`/`gestalt`/`manifold`/`aura` are tokenized but never parsed, and `else { if … } else { … }` chains are rejected.
- **SCG** — Semantic Computation Graph core (26 NodeType, 7 EdgeKind; petgraph-backed; transform passes) (191 unit tests pass).
- **VUMA core** — MSG, invariants, region model, access analysis, security model, REPL (301 unit tests pass). **But** the MSG-to-codegen link is broken (see below).
- **BD** — Behavioral Descriptors (RepD 11, CapD 17, RelD 6) with inference (342 unit tests pass). M2.3 generic inference deferred.
- **FFI** — 19 Linux syscalls, `extern "C"` blocks emit relocations on all 10 backends.

### What Doesn't Work (blocking)

- **IVE verification** — `--verification normal` rejects every `examples/*.vuma` file. Top-level `region` declarations are flagged as leaks (`pipeline.rs:5783-5797`); spec §5.4 static-lifetime inference unimplemented. **Flagship feature is unusable.** Test suite bypasses with `--verification none`.
- **Self-hosting** — `src/bootstrap/vuma_compiler.vuma` (730 LOC, lexer-only) fails SCG→MSG construction and has a live `src_len_global` bug. `womb/lang/vuma_compiler.vuma` (506 LOC) is in the 6/16 set that doesn't parse. Self-hosting is at <5%.
- **Codegen correctness** — Two divergent SCG→IR bridges: canonical path verifies but emits broken code; `bridge_ast_to_codegen_scg` emits but skips verification. MSG and codegen IR are not connected. Standalone `Allocate`/`Free`/`Match`/`Sync`/`Access` statements are silently dropped (`main.rs:1656-1657, 1781-1784`). Top-level `region buf = allocate(1024)` → SIGSEGV; `womb/lang/minicompiler.vuma` → infinite loop.
- **`vuma run` on non-aarch64 hosts** — Native exec fails (ENOEXEC), qemu-aarch64 fallback not installed by default. No host-arch detection, no `--target` flag.
- **Womb data-model layer** — `concept`/`gestalt`/`manifold`/`aura` tokenized but never parsed. The entire Womb frontend is a gap.
- **Parser `else-if` chains** — `else { if … } else { … }` rejected; 6/16 `womb/lang/*.vuma` fail to parse.
- **Type checking** — Parser recognizes syntax but doesn't validate types.
- **Concurrent verification** — Single-threaded only.
- **COR runtime** — Partially integrated (`Option<CORuntime>`).
- **Standard library linking** — `vuma-std` Rust crate not linked to VUMA programs. Womb modules not auto-imported.
- **`map_device()` / `volatile`** — Not implemented (referenced in example comments only).

---

## Milestones

### M1: Multi-Architecture Codegen ⚠️ Partial
- 10 backends emit code, 99.99% gold-standard pass rate (under `--verification none`)
- FFI (19 syscalls), atomics, DWARF v4 debug info
- **But** emitted binaries crash/infinite-loop on non-trivial programs; standalone memory ops are silently dropped

### M2: Verification Engine ❌ Broken
- M2.1 (Liveness, Origin, Cleanup): ❌ False positives on every program using top-level `region`
- M2.2 (Exclusivity, Interpretation): ⚠️ Unit tests pass, but untested on real programs
- M2.3 (Generic BD inference): ❌ Deferred
- M2.4 (Doubly-linked list verification): ❌ Partial
- **Flagship feature is unusable end-to-end**

### M3: Language Features ⚠️ Partial
- Functions, structs, enums, match, if/while/for: ✅ (within the gold-standard suite)
- Imports, extern, type annotations: ✅
- Closures: ⚠️ Parsed but limited codegen
- Generics: ⚠️ Parsed but not monomorphized
- Type checking: ❌ Not implemented
- `concept`/`gestalt`/`manifold`/`aura`: ❌ Tokenized but never parsed
- `else { if … } else { … }` chains: ❌ Parser rejects

### M4: Self-Hosting ❌ Not Achievable Yet
- `src/bootstrap/vuma_compiler.vuma` (730 LOC lexer POC) does not compile; has live `src_len_global` bug
- `womb/lang/vuma_compiler.vuma` (506 LOC) does not parse (in the 6/16 broken set)
- End-to-end pipeline not testable until IVE (#1), codegen bridges (#2/#5/#6), and parser gaps (#3/#4) are fixed

---

## Next Steps (priority order)

1. **Fix IVE's false positive on top-level `region`** — implement spec §5.4 "Global scope / Static lifetime" inference in `src/ive/src/verification.rs::extract_cleanup_graph`. Until this is fixed, the language's value proposition is fictional.
2. **Unify the two codegen bridges** — either make `bridge_ast_to_codegen_scg` route through verification, or make the canonical `bridge_scg_to_codegen` produce correct code. Right now the verified path produces broken code and the working path skips verification.
3. **Stop silently dropping `Allocate`/`Free`/`Match`/`Sync`/`Access` statements** in `bridge_stmt_to_scg` (`src/main.rs:1656-1657, 1781-1784`). Generate real IR or emit an explicit error.
4. **Fix the parser to accept `else { if … } else { … }` chains** (or rewrite the 6 broken `womb/lang` files to use `else if`).
5. **Implement `concept`/`gestalt`/`manifold`/`aura` parsing** (or remove them from the lexer/AST/SCG). The Womb layer is currently a ghost.
6. **Fix runtime bugs** — top-level `region` segfault, `minicompiler` infinite loop.
7. **Make `vuma run` host-arch-aware** (or default `vuma build` to the host arch).
8. **Then** rewrite the bootstrap lexer to actually parse (not just lex), and fix the `src_len_global` bug.
9. **Fix the 3 remaining gold-standard failures** — `crc32` on riscv64/ppc64, `s27_fn_two_args_mod` on ppc64.
10. **Expose RISC-V 32 and x86_32 in the CLI** (`IsaArg`).
11. **Delete `src/codegen/src/lib.rs.tmp`** and clean up `pipeline.rs` dead code.

