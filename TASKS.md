# VUMA Task Plan

> **Status key:** `[ ]` pending · `[~]` in progress · `[x]` done

---

## Completed Work

The following waves are fully delivered and verified against the source tree.
Detail has been removed; the code is the source of truth.

### Waves 1–9: Syscall stubs, POSIX coverage, backend fixes
All 14 ELF backends have real syscall stubs for the full POSIX surface
(file metadata, process/identity, system/advanced, mmap ABI normalization).
wasm32 has `vuma.*` host imports for filesystem, sockets, and process ops.

### Waves 10–13: IRInstr::Syscall + parser syntax
First-class `IRInstr::Syscall { nr, args, dst }` variant in IR, SCG, and parser.
All 19 backends emit real syscall instructions. Big-endian wrappers inherit
correctly via delegation.

### Waves 14–16: Dead IVE code deletion + hardened verification
Deleted 5,880 LOC of orphaned invariant_* verifiers and 1,521 LOC of the
parallel BD solver. Wired interprocedural, modular, and constant-time
analyses as new verification levels.

### Waves 17–19: Proof system
Implemented `prove_interpretation` tactic, fixed `is_well_founded`, replaced
string-matching with structural Judgment matching. Wired `build_proof_bundle`
to extract real ProofSCG/ProofMSG and call `prove_*` tactics. `ProofChecker`
runs on all 5 proofs. `prove_exclusivity` vacuous-truth bug fixed.

### Wave 20: Memory safety as blocking pass
`CompileConfig.memory_safety` gates Stage 6b. UAF, double-free, uninit-read
are hard errors. `--no-memory-safety` escape hatch documented.

### Waves 21–24: Register allocation plumbing
`emit_binary` accepts `&[AllocationResult]`. `STACK_SLOT_VREG_THRESHOLD`
removed. Per-backend `emit_function_regalloc` methods exist on all backends.
`TargetAgnosticRegAlloc` wired. Coalescing and pressure-aware spill weight
implemented. Register allocation metadata flows end-to-end.
**Note:** The regalloc emit path is metadata-only — `emit_function_regalloc`
delegates to `emit_function_greedy` for actual code generation. The
`AllocationResult` does not influence emitted bytes. Real spill-code
emission is deferred.

### Waves 25–28: Re-enabled optimizers
Inliner (with cost model + threshold config), LICM (with preheader emission),
instruction scheduler (with alias analysis + pressure heuristic), cross-function
constant propagation, and identical-function merge are all wired at O2+.

### Waves 29–31: Vectorizer, loop optimizer, e-graph
Loop vectorization with IV-step adjustment. SLP vectorization rewrites IR
via `IRInstr::VectorOp`. SSE/AVX/NEON encoders wired into x86_64 and
aarch64 ISel paths. Multi-block loop unrolling with SCEV. Conservative
unroll-and-jam for perfectly-nested loops. E-graph with rebuilding, bottom-up
DP extraction, and 19 algebraic rules.

### Waves 32–35: Analyses and lowering
Escape analysis (SROA + alloc elision), effects analysis (interprocedural),
4 SCG passes (LICM, StrengthReduction, TailCallOptDetection,
DeadRegionElimination), 5 lowerers (Monomorphizer, ClosureLowerer,
SwitchLowerer, TailCallLowerer, LoopOptimizer). Exception/Coroutine lowerers
deleted (no syntax for them).

### Waves 36–38: Proof log, CoR
`ProofLog` populated during `EGraph::saturate_with_proof`. `check_proof_log`
wired into production `EGraph::saturate` (advisory mode). `bv_verify` hard
gate before saturation. CoR: 4 real optimization passes (HotPathInlining,
ColdPathOutline, LoopOptimization, MemoryOptimization). Decision (b): CoR
is profiling-only, does not modify user binary.

### Waves 39–42: Self-hosting dependency removal
Hand-written `DiGraph` replacing petgraph. `indexmap`, `smallvec`,
`hashbrown` fully removed. `thiserror` fully removed (hand-written
Display/Error impls). `serde`/`serde_json` fully removed from all 10 core
crates — replaced by hand-written `BinarySerializable` codecs and a
root-crate `JsonValue` enum with recursive-descent parser.

### Waves 43–44: Serde + log removal
All 10 core crates are serde-free. `log` crate removed — `vuma_log!` macro
is the sole logging mechanism.

### Wave 45: libc removal from cor + std
`cor` and the former `vuma-std` used raw `extern "C"` syscalls instead of
`libc::`. (Note: `vuma-std` has since been deleted entirely — see below.)

### Wave 46: vuma-std deleted
`vuma-std` (24,819 LOC, 19 modules) has been **deleted entirely**. It was
depended on by zero other workspace crates. The runtime library for VUMA
programs lives in `womb/` (the VUMA-native standard library), not in a
Rust crate. The former "decision (b): runtime-only" is moot — there is no
Rust-side std crate to be "runtime-only".

### Wave 47: Bootstrap consolidation
5 canonical bootstrap files in `womb/lang/`: `full_lexer.vuma`,
`full_parser.vuma`, `ir_builder.vuma`, `codegen.vuma`, `elf.vuma`.
6 orphaned drafts deleted. File I/O via `extern "C" { fn open/read/close/write }`.
argv parsing via `__vuma_argc`/`__vuma_argv` runtime stubs.

### Wave 48: Bootstrap self-host
`compile_modules` API for multi-module compilation at AST level.
`vuma link` subcommand. Parser context-awareness for `repd`/`bd`/`capd`/`reld`
keywords. `merge_module_asts` dedup-or-conflict policy.
`scg_to_ir.rs` then-branch rollback fix. `name_hash` u32 mask fix.
**Bootstrap self-host works end-to-end at O0.** Test
`test_wave48_bootstrap_self_host` passes: compiles 5 bootstrap files →
`vumac` → runs on `hello.vuma` → `a.out` prints `42`.

### Wave 49: Wrapper-backend documentation
All 4 big-endian wrapper backends documented. Cross-backend syscall
conformance test and print-helper regression test pass on all 19 backends.

### Wave 50: Final hardening
Real SHA256d + mmap_sha256d regalloc tests. Real e2e proof test via
`VumaCompiler::build_proof_bundle`. Strengthened UAF rejection test.
Cross-backend execution harness (`execute_x86_64_elf`). Bootstrap milestone
test compiles and runs `hello.vuma`. CI test job is strict (blocking).
CI clippy job is advisory (0 warnings as of last check).

### External dependency removal (post-Wave-50)
All 9 external crate dependencies eliminated from the workspace:
`log`, `tempfile`, `chrono`, `rayon`, `libc`, `clap`, `toml`, `proptest`,
`serde`/`serde_json`. Each replaced by hand-written code using only
Rust's `std`. **Zero external dependencies** — `cargo tree --depth 1`
shows only the 10 internal workspace crates.

---

## Open Work

### 1. O2 codegen bug — partial fix, remaining inliner issue

Wave 51 made three fixes:
- Cast-aware alias analysis (`alias_analysis.rs` tracks `IRInstr::Cast`)
- Function-wide alias info shared across blocks (`schedule_function`)
- `IRInstr::Ret` stripping in inlined bodies + `threshold=0` guard

**Remaining:** O2 still crashes when the inliner is active. Bisecting
showed the inliner creates IR shapes that expose scheduler/LICM bugs.
With inliner + scheduler + LICM all disabled, O2 works. With any one
enabled, O2 crashes. Investigation continues.

### 2. Runtime stubs — partially emitted by bootstrap (Wave 52)

The bootstrap compiler (`womb/lang/codegen.vuma`) now emits its own
`_start`, `print_int`, `__vuma_alloc`, `__vuma_free`, `print_newline`,
`print_hex`, `__vuma_argc`, `__vuma_argv`, and syscall stubs (`write`,
`read`, `open`, `close`). 337 bytes of runtime stubs at offset 0 of the
code section, entry point at offset 0.

**Remaining:** `__vuma_argc`/`__vuma_argv` use a BSS placeholder that is
NOT patched (elf.vuma doesn't allocate BSS). The full `_start` stub
(argc/argv BSS save) requires BSS support in `write_elf64`.

### 3. `merge_module_asts` is not a real linker

The cross-module linking primitive parses each module independently,
concatenates ASTs, and deduplicates function definitions by structural
equality. No symbol visibility, no name mangling, no incremental
recompilation. The dedup policy actively encourages code duplication
(each of the 5 bootstrap files copy-pastes the same 4 helpers).

**Fix:** Replace with a proper symbol-resolution + relocation model, or
implement a VUMA-native `import` mechanism.

### 4. Real spill-code emission — implemented (Wave 53)

`emit_function_regalloc` now consumes the `AllocationResult`:
- Callee-saved prologue/epilogue (STP/LDP for used callee-saved GPRs)
- Per-instruction spill/reload from `spill_code` BTreeMap
- `vreg_to_preg` mapping fed to the greedy allocator via `preassign`
- Eliminated copies (coalesced moves) skipped

Test `test_wave53_real_spill_code_emission` verifies emitted bytes
differ from the greedy path when the allocator reports spills.

---

## Project Structure

```
vuma/
├── Cargo.toml              # workspace root — zero external deps
├── src/
│   ├── scg/                # Semantic Computation Graph
│   ├── ive/                # Inference & Verification Engine
│   ├── bd/                 # Bidirectional inference
│   ├── vuma/               # vuma-core (MSG, REPL, security)
│   ├── codegen/            # 19-architecture codegen backends
│   ├── parser/             # Lexer, parser, AST, to_scg
│   ├── cor/                # Continuous Optimization Runtime
│   ├── proof/              # Proof system (tactics, checker, artifacts)
│   ├── package/            # Package manager (manifest, registry)
│   └── tests/              # Integration test suite
├── womb/                   # VUMA-native standard library + bootstrap
│   ├── lang/               # 5-file self-hosting bootstrap compiler
│   ├── lib/                # VUMA standard library modules
│   ├── collections/        # VUMA-native collections (Vec, HashMap, etc.)
│   ├── crypto/             # VUMA-native crypto primitives
│   ├── encoding/           # hex, base64
│   ├── graph/              # DiGraph
│   └── ...
├── examples/               # Example .vuma programs
├── scripts/                # Test harnesses, KAT generators
└── tests/                  # Integration test files
```

### Workspace crates (10 — zero external dependencies)

| Crate | Purpose |
|-------|---------|
| `vuma-scg` | Semantic Computation Graph + hand-written DiGraph |
| `vuma-ive` | Inference & Verification Engine (6 verification levels) |
| `vuma-bd` | Bidirectional inference (RepD/CapD/RelD) |
| `vuma-core` | MSG, REPL, security analysis |
| `vuma-codegen` | 19-architecture codegen (x86_64, aarch64, riscv64/32, arm32, x86_32, loongarch64, mips64, ppc64, s390x, sparc64, alpha, hppa, m68k, wasm32, + 5 BE/LE wrappers) |
| `vuma-parser` | Lexer, parser, AST, SCG lowering |
| `vuma-cor` | Continuous Optimization Runtime (JIT, speculative optimization) |
| `vuma-proof` | Proof system (liveness, exclusivity, cleanup, origin, interpretation) |
| `vuma-package` | Package manager with hand-written TOML parser |
| `vuma-tests` | Integration test suite |

### Verification status

- `cargo check --workspace`: 0 errors
- `cargo clippy --workspace`: 0 warnings
- `cargo test --workspace --no-run`: compiles cleanly
- `cargo test -p vuma-tests --lib wave48`: 9 passed (bootstrap self-host)
- `cargo test -p vuma-tests --lib wave50`: 9 passed (final hardening)
- `cargo tree --depth 1`: zero external dependencies
