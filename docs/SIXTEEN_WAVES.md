# VUMA 16-Wave Modernization Plan

Based on the verified technical audit, each wave addresses a specific
finding with concrete code changes, acceptance criteria, and measurable
outcomes.

## Wave 1: Backend Correctness — 100% Test Pass Rate
**Problem:** 360 failures across 8 backends (alpha, hppa, m68k, sparc64, BE variants)
**Root causes:** Stack layout overlap, 64-bit ops on 32-bit, timeouts, recursion ABI
**Files:** `alpha.rs`, `hppa.rs`, `m68k.rs`, `sparc64.rs`, `aarch64_be.rs`, `armeb.rs`
**Acceptance:** All 19 backends pass 5745/5745 tests (100%)

## Wave 2: Wire E-Graph into Optimizer
**Problem:** `egraph.rs` (240 LOC) implements equality saturation but is dead code
**Action:** Add `equality_saturation()` pass to `opt.rs` pipeline after CSE
**Files:** `opt.rs`, `egraph.rs`
**Acceptance:** E-graph pass runs, rewrite rules fire, tests pass

## Wave 3: Wire Alias Analysis into Optimizer
**Problem:** `alias_analysis.rs` (175 LOC) exists but `opt.rs` never calls it
**Action:** Add TBAA-driven dead store elimination and load/store reordering
**Files:** `opt.rs`, `alias_analysis.rs`
**Acceptance:** Dead stores eliminated, loads reordered safely

## Wave 4: Fix and Re-enable Optimizer Tests
**Problem:** `opt.rs:1140` has `#[cfg(any())]` — all optimizer tests disabled
**Action:** Fix broken tests, re-enable, add new tests for e-graph and alias passes
**Files:** `opt.rs`
**Acceptance:** `cargo test` runs optimizer tests, all pass

## Wave 5: Instruction Scheduler Framework
**Problem:** No instruction scheduler — instructions emitted in IR order
**Action:** Create `scheduler.rs` with list-scheduling algorithm and latency tables
**Files:** New `scheduler.rs`, `x86_64/mod.rs`, `arm64.rs`
**Acceptance:** Scheduler reduces load-use stalls on x86_64 and aarch64

## Wave 6: Register Allocator Improvements
**Problem:** Linear-scan only — no graph coloring, no spilling heuristics
**Action:** Add register pressure tracking, improve spilling, add coalescing
**Files:** `regalloc.rs`
**Acceptance:** Fewer spills on register-pressure benchmarks

## Wave 7: SMT-Based Backend Verification
**Problem:** Backend bugs found via runtime testing, not verified lowering
**Action:** Add Alive2-style equivalence checking for instruction lowering
**Files:** New `verify.rs` module
**Acceptance:** Peephole optimizations proven correct via SMT

## Wave 8: Close IVE → Codegen Loop
**Problem:** IVE proves non-aliasing but codegen doesn't exploit it
**Action:** Feed IVE proof results to optimizer for aggressive reordering
**Files:** `opt.rs`, `ive/` modules, `pipeline.rs`
**Acceptance:** IVE-proven non-aliasing enables load/store reordering

## Wave 9: Loop Optimization Suite
**Problem:** Only basic LICM; no strength reduction, unrolling, or vectorization
**Action:** Add loop strength reduction, unrolling with profitability, IV simplification
**Files:** `opt.rs`, new `loop_opts.rs`
**Acceptance:** Loop-heavy benchmarks improve measurably

## Wave 10: Interprocedural Optimization
**Problem:** Only simple inlining; no IP constant propagation or mod/ref analysis
**Action:** Add IP constant propagation, cross-function alias analysis
**Files:** `opt.rs`, new `ipo.rs`
**Acceptance:** Cross-function constants propagated

## Wave 11: Auto-Vectorization Framework
**Problem:** No SIMD code generation
**Action:** Add vectorization for x86_64 (SSE/AVX) and aarch64 (NEON)
**Files:** New `vectorize.rs`, backend SIMD encoders
**Acceptance:** Simple loops vectorized on x86_64 and aarch64

## Wave 12: Backend Peephole Optimization
**Problem:** No target-specific peephole passes
**Action:** Add x86_64 LEA, aarch64 fused multiply-add, RISC-V compressed instructions
**Files:** Backend `.rs` files
**Acceptance:** Peephole passes reduce instruction count

## Wave 13: Dead Code and Branch Optimization
**Problem:** No jump threading, no branch folding, no dead block elimination
**Action:** Add jump threading, branch-to-conditional-move, block merging
**Files:** `opt.rs`, new `cfg_opts.rs`
**Acceptance:** Branch-heavy code has fewer jumps

## Wave 14: Profile-Guided Optimization (PGO)
**Problem:** No profile data for inlining/layout decisions
**Action:** Add branch probability inference, code layout optimization
**Files:** New `pgo.rs`
**Acceptance:** Hot paths inlined, cold paths outlined

## Wave 15: Formal Verification of Codegen
**Problem:** No formal proof that instruction selection preserves semantics
**Action:** Add machine-checked lowering proofs for critical backends
**Files:** New `formal/` module
**Acceptance:** Key instruction lowerings formally verified

## Wave 16: Performance Benchmarking and Documentation
**Problem:** No benchmarks vs clang/rustc, no performance documentation
**Action:** Establish benchmark suite, compare against LLVM, document gaps
**Files:** New `bench/` directory, performance reports
**Acceptance:** Performance characterized vs LLVM -O0/-O1/-O2
