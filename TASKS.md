# VUMA Task Plan

> **Status key:** `[ ]` pending · `[~]` in progress · `[x]` done

---

## Wave Faithfulness Audit (static inspection, no build)

A full static audit of every wave claim was performed against the source
tree (`cargo`/`rustc` were unavailable in the audit environment, so all
verification was by grep/read/glob — no tests were executed). Verdicts:

- **Waves 1–13, 49 (backends + syscalls):** VERIFIED. 19 backends confirmed
  (`BackendKind` enum, `src/codegen/src/backend.rs`). `IRInstr::Syscall` at
  `src/codegen/src/ir.rs:1579`; per-arch syscall emission present on all 19.
  wasm32 `vuma.*` host imports present. BE wrappers delegate correctly.
- **Waves 17–19 (proof system):** VERIFIED. `prove_interpretation`, structural
  `Judgment` matching, `build_proof_bundle`, `ProofChecker` on all 5 proofs,
  `prove_exclusivity` vacuous-truth fix — all real.
- **Waves 14–16 (dead-code deletion + verification levels):** VERIFIED. No
  `invariant_*` orphans in `src/bd`; no parallel BD solver; 6 `VerificationLevel`s.
- **Wave 20 (memory safety):** VERIFIED. `CompileConfig.memory_safety` gates
  Stage 6b; UAF/DoubleFree/UninitRead hard errors; `--no-memory-safety` parsed.
- **Waves 21–24 (regalloc plumbing):** VERIFIED for `emit_binary(&[AllocationResult])`,
  `STACK_SLOT_VREG_THRESHOLD` removal, `TargetAgnosticRegAlloc`, coalescing,
  pressure-aware spill weight.
- **Wave 53 (real spill code):** PARTIAL / OVERSTATED — see Open Work §4.
- **Waves 25–35 (optimizers + lowerers):** VERIFIED. Inliner (cost model +
  Wave 54 param-clobber fix), LICM w/ preheader, scheduler w/ cast-aware TBAA,
  cross-fn const prop, identical-fn merge, loop+SLP vectorization, SSE/AVX/NEON,
  SCEV unroll + unroll-and-jam, e-graph (35 rules total, 19 algebraic per W31),
  escape analysis, interprocedural effects, 4 SCG passes, 5 lowerers.
  (Stale "stub — no-op, TODO" comment at `loop_unroll.rs:452` contradicts the
  real `try_unroll_and_jam` at line 896 — comment-only cleanup.)
- **Waves 36–38 (proof log + CoR):** VERIFIED. `ProofLog` in `saturate_with_proof`,
  `check_proof_log` advisory, `bv_verify` hard gate (caveat: unknown rules pass
  silently — documented best-effort at `bv_verify.rs:351`), CoR 4 real passes,
  CoR profiling-only (no splice-back).
- **Waves 39–50 (dep removal + bootstrap):** VERIFIED. Zero external deps in the
  10-crate workspace; hand-written `DiGraph`, `JsonValue`, `vuma_log!`, raw
  `extern "C"` syscalls in `cor`, `vuma-std` deleted, 5-file bootstrap,
  `compile_modules`, `vuma link`, `merge_module_asts`, `name_hash` u32 mask,
  Wave 52 runtime stubs. Caveat: `rand = "0.8"` lives in `src/parser/fuzz/Cargo.toml`
  (separate `[workspace]`, excluded from main workspace — see Open Work §8).
- **Naming nit:** TASKS.md previously called the codec trait `BinarySerializable`;
  it is actually `BinaryWrite`/`BinaryRead` (functionally equivalent, just renamed).

The single most consequential finding of the audit is the **womb vuma-native
gap** — see Open Work §5–§7. The `syscall` intrinsic already exists end-to-end
(lexer `"syscall"` → `TokenKind::Syscall` → `Expr::Syscall` → `IRInstr::Syscall`
→ all 19 backends), so the `extern "C" { fn write(...) }` syscall stubs in womb
are unnecessary wrappers and must be migrated to direct `syscall(SYS_*, ...)`
calls to satisfy the hard constraint that **nothing in vuma wraps C or Rust —
everything must be vuma-native**.

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
removed. `emit_function_regalloc` exists on 5 tier-1 backends (aarch64, x86_64,
riscv64, loongarch64, arm32) + a generic fallback in `backend.rs`; only the
aarch64 path consumes `AllocationResult` for real spill code (see Open Work §4).
`TargetAgnosticRegAlloc` wired. Coalescing and pressure-aware spill weight
implemented. Register allocation metadata flows end-to-end.
**Wave 53 (aarch64 only):** `emit_function_regalloc` consumes the
`AllocationResult` for real on **aarch64** only (`Emitter::emit_function_regalloc`,
`src/codegen/src/emit.rs:729`) — callee-saved prologue/epilogue, per-instruction
spill/reload, `vreg_to_preg` mapping, eliminated-copy skipping all active there.
The other tier-1 backends (x86_64, riscv64, loongarch64, arm32) and the generic
fallback (`backend.rs:1966`) consume `&RegAllocResult` and only annotate
metadata via `regalloc_emit::annotate_with_regalloc` — **no real spill code**.
See Open Work §4.

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

### Wave 45: libc removal
`cor` used raw `extern "C"` syscalls instead of `libc::`. `vuma-std`
(formerly at `src/std/`) was also cleaned up before being deleted entirely
in Wave 46.

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

### 1. O2 codegen bug — inliner fixed, scheduler remaining

Wave 51 made three fixes:
- Cast-aware alias analysis (`alias_analysis.rs` tracks `IRInstr::Cast`)
- Function-wide alias info shared across blocks (`schedule_function`)
- `IRInstr::Ret` stripping in inlined bodies + `threshold=0` guard

Wave 54 fixed the inliner's param-clobbering bug:
- Callee params were mapped directly to caller arg vregs
- Callee reassignments (e.g. `pos = pos + 1`) overwrote caller variables
- Fix: map each callee param to a fresh vreg + insert copy instruction

**Result:** O2 self-host now works with inliner + LICM + constant fold +
DCE + cross-function const prop. The bootstrap self-host test passes at
O2 (production default) for the first time.

**Remaining:** The instruction scheduler still has an alias analysis bug
when reordering inlined code. Disabled via `VUMA_NO_SCHED` env var until
the Cast-aware TBAA handles the larger functions created by inlining.

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

### 4. Real spill-code emission — aarch64 only (Wave 53 PARTIAL)

Real spill-code emission (`emit_function_regalloc` consuming `AllocationResult`:
callee-saved STP/LDP prologue/epilogue, per-instruction spill/reload from
`spill_code`, `vreg_to_preg` preassign, eliminated-copy skipping) is implemented
**only for aarch64** (`src/codegen/src/emit.rs:729`).

The other tier-1 backends (`x86_64`, `riscv64`, `loongarch64`, `arm32`) define
`emit_function_regalloc` but take `&RegAllocResult` and only call
`regalloc_emit::annotate_with_regalloc` — they emit the greedy path's bytes and
attach metadata, with **no real spill/reload insertion**. The remaining 14
backends use the generic fallback at `backend.rs:1966` (also metadata-only).

Test `test_wave53_real_spill_code_emission` (`emit.rs:7208`) is aarch64-scoped.

**Fix:** Port the aarch64 `Emitter` spill-code path to x86_64, riscv64,
loongarch64, arm32 (tier-1), then to the remaining 14 backends. The
`AllocationResult`/`spill_code`/`vreg_to_preg` plumbing already exists; each
backend needs its own prologue/epilogue + load/store encoders for spill slots.

### 5. womb vuma-native gap — syscall stub wrappers (P1, blocking self-host)

**Hard constraint violated:** "no wrappers within vuma from c or rust — all
must be vuma native." 24 womb files declare `extern "C" { fn write/read/open/
close/socket/connect/... }` (~85 declarations, 46 distinct syscalls). These are
unnecessary wrappers because the `syscall` intrinsic already exists end-to-end:
`TokenKind::Syscall` (`lexer.rs:671`) → `Expr::Syscall` (`ast.rs:1147`) →
`IRInstr::Syscall { nr, args, dst }` (`ir.rs:1579`) → real per-arch instructions
on all 19 backends.

**Fix:** Add a womb-side `syscalls.vuma` module with per-arch `__NR_*` constant
tables (x86_64 / aarch64 / riscv64 / arm32 / …) and replace every
`extern "C" { fn write(...) }` + `write(fd, buf, n)` call site with
`syscall(__NR_write, fd, buf, n)`. This removes ~85 declarations across 24 files
and makes the kernel ABI the only external surface — invoked via the vuma-native
`syscall` intrinsic, not a C calling-convention wrapper.

### 6. womb vuma-native gap — allocator stubs (P2)

18 womb files declare `extern "C" { fn __vuma_alloc(size) -> Address;
fn __vuma_free(addr, size); }` (36 declarations). These are runtime allocator
stubs. They are unnecessary wrappers: a vuma-native allocator can be built on
`syscall(SYS_mmap, 0, size, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0)`
and `syscall(SYS_munmap, addr, size)`. `womb/alloc/arena.vuma` already exists as
a vuma-native arena — it should become the single allocator, backed by mmap
syscalls, and all 18 files should import it instead of declaring extern stubs.

(Note: the bootstrap `womb/lang/codegen.vuma` already emits a vuma-native
`__vuma_alloc` per Wave 52 — the gap is the non-bootstrap / `vumac` path and
the womb library modules that still declare the extern.)

### 7. womb vuma-native gap — cross-module `import` mechanism (P3, root cause)

VUMA has **no first-class cross-module `import`**. As a workaround, `extern "C"`
is overloaded for three distinct purposes that should be three mechanisms:

| Current `extern "C"` use | Should be |
|---|---|
| kernel syscall ABI | `syscall(nr, …)` intrinsic (already exists) |
| runtime allocator | vuma-native arena over `syscall(SYS_mmap)` |
| sibling-module references (~50 decls across 6 files) | `import` mechanism |

The sibling-module `extern "C"` decls (e.g. `sha256_oneshot`, `hmac_sha256`,
`aes256_gcm_encrypt`, `tcp_connect`, `parse`, `codegen_emit`) are **not**
violations per se — every such name resolves to a `^fn` definition in a
pure-VUMA sibling file. They exist only because there is no `import`. They
disappear once a vuma-native `import`/`export` (or per-module symbol table in
`merge_module_asts`) lands.

**Fix:** Add a vuma-native module system: `import { write_str } from "stdio";`
syntax in the parser, a per-module exported-symbol table, and real symbol
resolution + relocation in `merge_module_asts` (this also resolves Open Work §3).

### 8. Minor cleanups (non-blocking)

- `src/codegen/src/=36` — 0-byte stray file from a botched shell redirect; delete.
- `src/lib.rs:20` — stale `vuma-std` row in the crate-overview doc table.
- `src/logging.rs:213-249` — `log_error!`/`log_warn!`/`log_info!`/`log_debug!`
  macros and `VumaLogger` are `#[macro_export]`-ed but never invoked; dead code.
- `src/pipeline.rs:5377-5401` — `fn_defs_equivalent` doc-comment still describes
  the removed serde_json approach (impl uses Debug-string normalization).
- `src/codegen/src/loop_unroll.rs:452` — stale "stub — no-op, TODO" comment
  contradicts the real `try_unroll_and_jam` implementation at line 896.
- `src/parser/fuzz/Cargo.toml` — `rand = "0.8"` external dep. Lives in a separate
  `[workspace]` (fuzzing-only, excluded from the main workspace's zero-dep claim),
  but for a fully self-contained repo it should be replaced by a hand-written
  PRNG (xorshift / chacha8) so the fuzz target is also dependency-free.
- `VUMA_NO_SCHED` (opt.rs:1666) vs `VUMA_NO_SCHED_REORDER` (scheduler.rs:262) —
  two env vars gate the same scheduler; consolidate or document the split.
- `womb/env/cli.vuma` — `argv` reader is stubbed (returns 0 args, reads
  `/proc/self/cmdline` but does not parse). Needed for real CLI tools.
- `womb/lib/compression_extra.vuma` — DEFLATE does stored-block-only (no
  Huffman). Needed for real HTTP `Content-Encoding: gzip`.
- `womb/crypto/falcon.vuma` — uses CBD sampling instead of Falcon's discrete
  Gaussian. Needed for spec-compliant Falcon signatures.
- `womb/collections/{vec,hashmap,btree_map}.vuma` — `// TODO: grow` markers;
  no capacity growth. Needed for non-toy collection sizes.
- Proof test count: TASKS.md claims 132; static count of `#[test]` in
  `src/proof/src/` is 128 (~3% gap; may be parameterized tests — reconcile when
  cargo is available).

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

The following were reported by the original authors and were **not re-run**
during this audit (`cargo`/`rustc` were unavailable in the audit environment).
Static inspection confirmed the test functions and harnesses exist:

- `cargo check --workspace`: 0 errors (reported)
- `cargo clippy --workspace`: 0 warnings (reported)
- `cargo test --workspace --no-run`: compiles cleanly (reported)
- `cargo test -p vuma-tests --lib wave48`: 9 passed (bootstrap self-host at **O2**)
- `cargo test -p vuma-tests --lib wave50`: 9 passed (final hardening)
- `cargo test -p vuma-codegen --lib emit`: 104 passed (regalloc + emit)
- `cargo test -p vuma-codegen --lib scheduler`: 6 passed
- `cargo test -p vuma-proof --lib`: 132 passed (static count of `#[test]` = 128;
  reconcile when cargo available)
- `cargo test -p vuma-package --lib`: 24 passed (including toml_lite)
- `cargo tree --depth 1 -e normal`: zero external dependencies (10 internal crates only)
- Workspace members: `vuma-scg`, `vuma-ive`, `vuma-bd`, `vuma-core`, `vuma-codegen`, `vuma-parser`, `vuma-cor`, `vuma-proof`, `vuma-package`, `vuma-tests`
- **Not covered by the above:** `src/parser/fuzz/` is a separate `[workspace]`
  with `rand = "0.8"` (see Open Work §8).
