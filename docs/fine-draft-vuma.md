# VUMA Layer — Fine Draft (Final Engineering Plan)

**Status**: Final draft. Supersedes `docs/vuma-side-research-draft-v2.md`.
**Scope**: Layer 1 (VUMA compiler + codegen + runtime + verification).
**Date**: 2026-08-01.

---

## 1. Executive summary

VUMA (Verified-Unsafe Memory Access) is a verification-first systems
language whose entire architecture is shaped by a single decision: every
`transform` carries `requires`/`ensures` contracts discharged by a hard-wired
Z3 SMT solver, every arena-allocated access is bounded by a runtime
`__oob_trap` check (exit 134), and the formal PMT (Programs as Memory
Transformations) memory model is specified sorry-free in Lean. The
compiler is a 10-stage pipeline (parse → AST → SCG → IVE → IR →
channel-lowering → opt → regalloc → backend → ELF/Wasm) implemented as a
Cargo workspace of seven crates (`vuma-parser`, `vuma-scg`, `vuma-ive`,
`vuma-codegen`, `vuma-bd`, `vuma-core`, `vuma-package`) plus an in-tree
LSP module. Z3 is a hard dependency — there is no `--no-z3` fallback —
and the Lean FFI bridge that used to link formal proofs into the binary
has been removed; the Lean layer (`proof/PMT/`, 82 files, 280 theorems,
0 `sorry` tokens) now stands as a standalone formal specification, while
the compiler's runtime verification is performed by hand-written Rust
verifiers driving Z3 in-process.

This plan covers Layer 1 only — the VUMA compiler, codegen, runtime,
and verification. WOMB (Layer 2, UI engine) and VEEE (Layer 3, UX
language) are out of scope and are covered by separate fine drafts.
Cross-layer concerns (the WOMB-provided HMAC-SHA256 bootstrap consumed
by VUMA's capability model, the `womb/net/*.vuma` import fix surfaced
as a CI-side VUMA check, etc.) are noted here only where they touch a
VUMA-side file.

The headline decisions, all empirically verified against the source at
`main` HEAD `3f2f3a23` (which includes the V-34 fix at `a58dee80`):

- **V-34 is FIXED on `main`.** Commit `a58dee80` adds the two missing
  arms `"f32" => IRType::F32, "f64" => IRType::F64` to
  `bridge_type_to_ir_type` at `src/pipeline.rs:6515-6516`. Empirical
  validation on `scripts/v34_test.vuma` (per
  `docs/test-report-waves-s-z.md` §2) confirmed the pre-fix bug
  produced memory corruption (8-byte stores overwriting adjacent 4-byte
  f32 fields) AND IVE unsoundness (`discharge_rate=100%` on a wrong
  program). ADR-0011 reverted V-34's severity from P1 back to P0 based
  on this evidence; the fix has now landed.
- **The one true P0 remaining is the security cluster (V-16 + V-A3-2 +
  V-A3-6).** ADR-0007 (promoted from Proposed to Accepted by ADR-0024)
  captures the decision: hand-write HMAC-SHA256 in pure Rust, replace
  the hardcoded `b"vuma_dev_signing_key"` at `capability.rs:117`, and
  wire the IVE `l1l3_collapse` capability verifier to actually check
  signatures instead of `let known = true`. 7 weeks.
- **The bridge-fix epic of v2 has collapsed.** Of 8 P0 bugs the v1
  draft identified, only V-34 was truly P0 (now fixed). V-A3-2 is the
  other true P0 (in the security cluster, not the bridge cluster). The
  remaining bridge bugs are P1/P2 cleanup, not blockers — V-03/V-NEW-2
  is a P1 IVE-soundness lockstep migration, V-35/V-42/V-44 is P2
  cleanup of a dormant stub path, V-36/V-A2-1 is P2 work on a
  test-only SCG path, V-46 and V-NEW-1 are P1 deferred items needing
  their own ADRs.
- **V-A2-9 is DROPPED.** F-3 refuted it: `resolve_register_reuse_conflicts`
  at `regalloc.rs:2836-2897` explicitly models syscall arg/dst
  interference; `contains_fork` is a documented correctness requirement
  for `clone(2)`/`vfork(2)`, not a workaround. V-37 is also REFUTED —
  the trailing padding at `pipeline.rs:6741-6744` is computed correctly.
- **The dependency policy is locked.** ADR-0010 caps external crates
  at 5; ADR-0005 deletes `cc`, `find-msvc-tools`, and `shlex` once
  ADR-0004 lands. Post-cleanup state: 5 external crates (bitflags, z3,
  z3-sys, log, pkg-config). VUMA hand-writes its lexer NFA, TOML
  parser, JSON encoder, HMAC-SHA256, all 19 backends, and the
  e-graph — no `serde`, no `toml`, no `regex`, no LLVM, no MLIR, no
  Cranelift.
- **The three-layer architecture is locked.** ADR-0013 formalizes
  VUMA/WOMB/VEEE; ADR-0014 mandates VEEE compiles to VUMA AST (not IR)
  so VEEE programs inherit the full PMT verification pipeline; ADR-0021
  deletes the dead `Effect` enum (V-A3-7); ADR-0022 mandates
  hand-written SPIR-V (not MLIR) for the GPU path; ADR-0023 mandates
  VUMA's 19-backend codegen with `--dev` flags (not Cranelift) for
  VEEE dev builds.

---

## 2. Current state (empirically verified)

All numbers in this section were verified by direct source/tree
inspection on `main` HEAD `3f2f3a23` (audit branch). Citations point at
either the source file or the test report that recorded the empirical
evidence.

### 2.1 Test suite (per `docs/test-report-waves-s-z.md`)

| Metric | Stale (`78e71a6b`, 2026-07-31 23:46 UTC) | Fresh (`314b2987`/`c041517f`, 2026-08-01 12:51 UTC) | Delta |
|---|---|---|---|
| Total runs | 29963 | 29963 | 0 |
| Matches | 27992 | **28067** | +75 |
| Pass rate | 93.42% | **93.67%** | +0.25pp |
| Failures | 1971 | 1896 | -75 |
| Failing tests | 364 | 437 | +73 |

The fresh snapshot is post-`1d72d296` (phi+regalloc liveness fix) and
post-`c041517f`/`314b2987` (W1-sparc64/W1-x86_32 in-progress fixes).
The `1d72d296` fix improved 12 backends (+0.57 to +1.53pp) but
regressed s390x (-1.02pp); sparc64 (-3.81pp) and x86_32 (-4.25pp)
regressed from the W1 in-flight work.

An even fresher snapshot exists in `test_results/summary.json`
(timestamped `2026-08-01 14:36:54 UTC`, run on `turbogp.benchmarks.0x01`)
showing 28036/29963 = **93.57%** — a 0.10pp drift from the 12:51 UTC
snapshot, attributable to additional `c041517f`/`314b2987` test
iterations landing between the two runs. The 93.67% figure from the
test report is the cited headline; the 93.57% figure is the most recent
snapshot. Both are post-`1d72d296` baselines.

### 2.2 Per-backend pass rate (fresh snapshot, ranked)

| Rank | Backend | Pass/Total | Pass% | Delta vs stale | Notes |
|------|---------|------------|-------|----------------|-------|
| 1 | wasm32 | 1577/1577 | 100.00% | 0 | stack machine, no regalloc |
| 2 | s390x | 1560/1577 | 98.92% | **-1.02** | regressed (V-S390X-1) |
| 3 | loongarch64 | 1558/1577 | 98.80% | +1.53 | improved |
| 4 | mips64 | 1552/1577 | 98.41% | +1.14 | improved |
| 4 | mips64be | 1552/1577 | 98.41% | +1.14 | wrapper |
| 6 | x86_64 | 1551/1577 | 98.35% | +0.57 | improved |
| 7 | hppa | 1550/1577 | 98.29% | +0.70 | improved (F-3 confirmed F64 is real) |
| 8 | aarch64 | 1549/1577 | 98.22% | +1.01 | improved |
| 8 | aarch64_be | 1549/1577 | 98.22% | +1.01 | wrapper |
| 10 | riscv64 | 1544/1577 | 97.91% | +0.57 | improved |
| 10 | riscv32 | 1544/1577 | 97.91% | +1.14 | improved |
| 12 | alpha | 1540/1577 | 97.65% | +0.38 | improved |
| 13 | arm32 | 1535/1577 | 97.34% | +0.57 | improved |
| 13 | armeb | 1535/1577 | 97.34% | +0.57 | wrapper |
| 15 | ppc64 | 1282/1577 | 81.29% | 0 | unchanged (loop-lowering cluster) |
| 15 | ppc64le | 1282/1577 | 81.29% | 0 | wrapper, inherits ppc64 bug |
| 17 | sparc64 | 1297/1577 | 82.24% | **-3.81** | regressed (W1-sparc64 in-flight) |
| 18 | m68k | 1262/1577 | 80.03% | -0.44 | slightly worse (V-A2-8 F32 stub) |
| 19 | x86_32 | 1249/1577 | 79.20% | **-4.25** | regressed (W1-x86_32 in-flight) |

### 2.3 Lean proof layer

| Metric | Value | Verification |
|---|---|---|
| Lean files | 82 | `find proof/ -name '*.lean' -type f \| wc -l` |
| Theorems + lemmas | 280 | `grep -rE '^(theorem\|lemma) ' proof/ --include='*.lean' \| wc -l` |
| Actual `sorry` tactic uses | 0 | `grep -rnE '(:=\s*sorry\|exact\s+sorry\|^\s+sorry\s*$)' proof/ --include='*.lean'` |
| Iris separation-logic layer | 9 files in `proof/PMT/Iris/` | `CapBndInvariant`, `LiveMirrorInvariant`, `GuardInvariant`, `Composition`, `HeapModel`, `FractionalPerm`, `SepGenuine`, `WeakestPrecond`, `ArenaRes` |
| Faithful Lean↔Rust simulation | 22 files in `proof/PMT/Faithful/` | `Model`, `Simulation`, `SimSound`, `SimTransform`, `SimWrite`, `RustConformance`, `UafProof`, `OverflowProof`, … |
| BitVec overflow model | `proof/PMT/BitVecArena.lean` | fixed-width usize, more faithful than `Nat` |
| Allocator-failure model | `proof/PMT/MmapArena.lean` | `MAP_FAILED` paths |
| Pipeline simulation refinement | `proof/PMT/PipelineSim.lean` | `SimRel` |
| Strengthened well-typedness | `proof/PMT/WellTypedStrong.lean` | `DataflowOk` + per-step `FieldBounds` |

The `language-reference.md` §10 claim of "2 sorries" is STALE — the
actual count is 0.

### 2.4 Toolchain (per `docs/test-report-waves-s-z.md` §Environment)

| Component | Version | ADR requirement | Status |
|---|---|---|---|
| Rust | nightly-2026-03-01 (rustc 1.96.0-nightly) | nightly-2026-03-01 | ✅ matches `rust-toolchain.toml` |
| Z3 | 4.13.4 | ≥ 4.12 | ✅ installed from GitHub prebuilt |
| QEMU | 10.0.11 (all 18 user-mode binaries) | ≥ 10.0 (ADR-0009) | ✅ |
| wasmtime | 47.0.2 | ≥ 47.0 | ✅ |
| Lean | not installed (lake build deferred) | — | not tested |

### 2.5 Backends

19 backends (verified via `src/codegen/src/backend.rs:784` `BackendKind`
enum — 19 CPU ISA variants). All 19 have a `reg_isel.rs` module; 15 are
substantive, 4 are byte-swap wrappers (`aarch64_be`, `armeb`,
`mips64be`, `ppc64le`). `wasm32` uses structured stack-machine emission
(the correct architecture for WebAssembly, not a fallback). The legacy
`loongarch64/reg_alloc_isel.rs` file is dead code (module declaration
commented out at `loongarch64/mod.rs`).

### 2.6 Dependency manifest

8 external crates in `Cargo.lock` (verified):

| Crate | Declared at | Transitive of | Role | Post-ADR-0005 status |
|---|---|---|---|---|
| `bitflags` | `Cargo.toml:65` | — | bitset macros for IR/capability flags | KEEP |
| `cc` | `Cargo.toml:75` (build-dep) | — | UNUSED (Lean FFI bridge removed) | **DELETE** |
| `find-msvc-tools` | (Cargo.lock:21-25) | `cc` | MSVC toolchain discovery | **DELETE** (transitive) |
| `shlex` | (Cargo.lock:39-43) | `cc` | shell-style tokenizer | **DELETE** (transitive) |
| `z3` | `src/ive/Cargo.toml:22` | — | SMT solver; the "V" in VUMA | KEEP |
| `z3-sys` | (Cargo.lock:133-140) | `z3` | FFI bindings to libz3 | KEEP (transitive) |
| `log` | (Cargo.lock:27-31) | `z3` | logging facade | KEEP (transitive) |
| `pkg-config` | (Cargo.lock:33-37) | `z3-sys` | libz3 system-library discovery | KEEP (transitive) |

Post-ADR-0005 state: **5 external crates** (bitflags, z3, z3-sys, log,
pkg-config). This satisfies ADR-0010's 5-crate cap exactly.

---

## 3. The bridge-fix epic (V-34 done, remaining items)

The v1 draft's "bridge-fix epic" was built on 8 P0 bugs. Wave F
re-audit downgraded 7 of them; Wave S-Z empirical testing reverted
V-34 back to P0 (and it has now been fixed). The remaining items are
sequenced below with ADR references, status, what's done, and what's
left.

### 3.1 V-34 (ADR-0001) — FIXED on main

- **Status**: Fixed.
- **ADR**: [ADR-0001](adr/ADR-0001.md) (Accepted). Severity history: P0
  → P1 by ADR-0011 → reverted to P0 by Wave S-Z empirical test.
- **Layer**: VUMA.
- **Effort**: 3 days (1-line fix + regression tests + gold-standard
  f32-state-field test).
- **What's done**: Commit `a58dee80` adds `"f32" => IRType::F32, "f64"
  => IRType::F64` arms to the `Type::BDBase(name)` inner match in
  `bridge_type_to_ir_type` at `src/pipeline.rs:6515-6516`. Verified by
  direct source read at `main` HEAD `3f2f3a23`. The fix also adds
  `tests/verification/v34_test.vuma` (27 LOC regression test) and
  `docs/research/AD-AE-s390x-w1-investigation.md` (179 LOC).
- **What's left**: nothing. Closed.
- **Empirical validation**: per `docs/test-report-waves-s-z.md` §2, the
  pre-fix IR dump for `scripts/v34_test.vuma` showed `Load { ty: U64 }`
  and `Store { ty: U64 }` for `f32` fields, causing 8-byte stores that
  overwrite adjacent 4-byte fields. Post-fix, the test program exits 4
  (1.5 + 2.5 = 4.0 → cast to i32 = 4) instead of the buggy 0.

### 3.2 V-35 + V-42 + V-44 (ADR-0002) — P2, dormant stub path

- **Status**: Open (low urgency).
- **ADR**: [ADR-0002](adr/ADR-0002.md) (Accepted; severity revised
  P0→P2 by ADR-0011).
- **Layer**: VUMA (parser-side, semantic SCG).
- **Effort**: 1 week + 2 days = ~9 days.
- **What's done**: ADR written; fix design locked.
- **What's left**:
  1. Add a `&self.layouts` lookup to the catch-all arms of
     `type_size_from_name` (`src/parser/src/to_scg.rs:4063`) and
     `type_alignment` (`src/parser/src/to_scg.rs:3846`).
  2. Add a new `layout_total_alignment` helper (~10 lines).
  3. Add code-level regression tests (zero exist today).
- **Dependencies**: none.
- **Rationale for P2**: IVE has zero references to `StructDefNode` /
  `StructFieldInfo`. The sole codegen consumer of `type_size_from_name`
  is the dormant stub `state_merge_compatible_layouts` at
  `bv_verify.rs:421` (documented as dormant in
  `language-reference.md` §9 caveat 8). The codegen production path
  uses `build_layout_registry` (`pipeline.rs:6625-6699`), a separate
  correct multi-pass algorithm.
- **Next action**: implement when the dormant stub is needed, or as
  cleanup after the V-03 lockstep lands.

### 3.3 V-36 + V-A2-1 (ADR-0003) — P2, test-only SCG path

- **Status**: Open (low urgency).
- **ADR**: [ADR-0003](adr/ADR-0003.md) (Accepted; severity revised
  P0→P2 by ADR-0011).
- **Layer**: VUMA (codegen SCG → IR lowering).
- **Effort**: 1 week + 1 week = ~2 weeks.
- **What's done**: ADR written; fix design locked. Per ADR-0003
  §"Scope note": this ADR does **not** fix V-A2-2 (`inttofloat`/
  `floattoint` hardcoded to I64↔F64); V-A2-2 lives on the SCG cast
  node, not the `PmtOpStmt` path.
- **What's left**:
  1. Thread IRType + offset through `PmtOpStmt::StateRead`/`StateWrite`
     lowering at `src/codegen/src/scg_to_ir.rs:6002-6028`.
  2. Fix `Alloc { size: 0 }` in `StateInit` / `ArenaNew` / `ArenaAlloc`
     at `src/codegen/src/scg_to_ir.rs:6044-6067` (consult the layout
     table for the real element size).
  3. Add regression tests for `float_mem/*` and `mem_copy_buffer.vuma`.
- **Dependencies**: ADR-0001 (V-34) ✅, ADR-0004 (V-03+V-NEW-2) for the
  layout-table plumbing.
- **Rationale for P2**: F-1 verified that `PmtOpStmt` fires only for
  IVE-test-constructed or deserialized SCGs (per
  `language-reference.md` §7). Production path uses `AccessNode::Load`
  (affected by V-34, now fixed). However, F-3 confirmed V-A2-1 is still
  live — it causes `float_mem/*` and `mem_copy_buffer.vuma` failures.
- **Next action**: implement alongside V-03 lockstep, or when the
  `float_mem/*` failures become blocking.

### 3.4 V-03 + V-NEW-2 (ADR-0004) — P1, IVE-soundness lockstep

- **Status**: Open (P1).
- **ADR**: [ADR-0004](adr/ADR-0004.md) (Accepted; framing revised
  P0→P1 by ADR-0011, IVE-soundness not codegen-correctness).
- **Layer**: VUMA (codegen bridge + IVE-side parity).
- **Effort**: 1 week + 3 days = ~10 days, must land as one PR.
- **What's done**: ADR written; three-change lockstep design locked.
- **What's left**:
  1. **Change 1**: Migrate `build_pmt_layout_specs` at
     `src/pipeline.rs:6715-6756` from legacy `bridge_type_size`
     (`:6532`) to `bridge_type_size_with_layouts` (`:6557`).
  2. **Change 2**: Migrate IVE `rederive_layout` at
     `src/ive/src/verification.rs:268-291` in lockstep — the parity
     with codegen is INTENTIONAL per the docstring at
     `verification.rs:264-267`. If only Change 1 lands, IVE's
     `verify_layout_consistency` will FAIL for any program with a
     nested layout.
  3. **Change 3**: Delete legacy `bridge_type_size` (V-40, ADR-0005).
- **Dependencies**: ADR-0001 (V-34) ✅. ADR-0002 (V-35) is NOT a
  dependency — V-35 lives on the parser-side semantic SCG, V-03 lives
  on the codegen-side IVE-public table; they are independent paths.
- **Next action**: land the three-change lockstep as one PR. This
  unblocks sound IVE discharge on programs with nested layouts.

### 3.5 V-40 (ADR-0005) — P2, delete legacy function after ADR-0004

- **Status**: Open (P2, bundled into ADR-0004 as Change 3).
- **ADR**: [ADR-0005](adr/ADR-0005.md) (Accepted).
- **Layer**: VUMA.
- **Effort**: 1 day.
- **What's done**: ADR written; deletion point locked
  (`src/pipeline.rs:6532`).
- **What's left**: delete legacy `bridge_type_size` after ADR-0004
  Change 1+2 land. Also delete the `cc` build-dep, `find-msvc-tools`,
  and `shlex` transitive deps per ADR-0005 (drops the manifest from 8
  to 5 crates).
- **Dependencies**: ADR-0004 must land first (legacy function has zero
  callers after Change 1+2).
- **Next action**: land alongside ADR-0004 as Change 3.

### 3.6 V-46 — P1, deferred (write ADR)

- **Status**: Open (P1, deferred — no ADR yet).
- **ADR**: none — deferred. ADR-0001 §"Alternatives" notes that the
  unified `VumaType` refactor would eliminate V-46 in one stroke, but
  that refactor is a 2-3 week effort tracked as a separate RFC.
- **Layer**: VUMA (codegen, `resolve_state_array_access`).
- **Effort**: 1 week.
- **What's done**: cataloged; site located at
  `src/pipeline.rs:7403` (`_ => (1, None)` catch-all). F-1 did not
  fully re-verify this; preserved at P1 pending follow-up.
- **What's left**:
  1. Write an ADR specifying the fix: consult the layout table for
     unknown element types in `resolve_state_array_access`, returning
     `(real_elem_size, Some(real_elem_type))` instead of `(1, None)`.
  2. Implement the fix.
  3. Add regression tests for `[StructType; N]` indexing (currently
     accesses byte `i` not `i * sizeof(T)`).
- **Dependencies**: ADR-0002 (V-35) for the layout-table helper.
- **Next action**: write the ADR. Without it, `[StructType; N]` state
  arrays produce wrong offsets on indexing.

### 3.7 V-NEW-1 — P1, deferred (write ADR)

- **Status**: Open (P1, deferred — no ADR yet).
- **ADR**: none — deferred.
- **Layer**: VUMA (codegen, `allocate` builtin).
- **Effort**: 1 week.
- **What's done**: cataloged; sites located at
  `src/pipeline.rs:9228`, `:9297`, `:9598` (silent 8-byte truncation
  when `allocate(<non-literal>)` is called with a runtime-computed
  size). F-1 did not re-verify; preserved at P1 pending follow-up.
- **What's left**:
  1. Write an ADR specifying the fix: thread the runtime size through
     to the `Alloc` IR instruction instead of truncating to 8 bytes.
  2. Implement the fix.
  3. Add regression tests for `allocate(<expr>)`.
- **Dependencies**: ADR-0002 (V-35) for the layout-table helper; ADR-0003
  (V-A2-1) for the `Alloc` IR plumbing (per ADR-0003 §"Scope note",
  V-NEW-1 overlaps with V-A2-1's `allocate(<non-literal>)` pattern).
- **Next action**: write the ADR. Without it, dynamic allocation with
  a non-literal size silently over-allocates or under-allocates.

---

## 4. Security cluster (P0)

The security cluster is the one true P0 remaining on VUMA after V-34
was fixed. It comprises three coupled bugs that together make the
capability model security theater: the IVE verifier is a stub, the
signing key is hardcoded, and the signature scheme is not a MAC.

### 4.1 V-16 + V-A3-2 + V-A3-6 (ADR-0007, promoted by ADR-0024)

- **Status**: Open (P0). ADR-0007 was Proposed (medium-confidence on
  the per-backend call-ABI wiring step); ADR-0024 promoted it to
  Accepted after F-2 verified that capability verification happens at
  IVE compile time (in `l1l3_collapse`), not in emitted binaries. The
  compile-time-only design is correct by design.
- **ADR**: [ADR-0007](adr/ADR-0007.md) (Accepted, promoted by
  [ADR-0024](adr/ADR-0024.md)). Severity revised P1→P0 by ADR-0011.
- **Layer**: VUMA (capability model + IVE verifier + Lean model).
- **Effort**: 7 weeks (5 weeks code + 2 weeks Lean model updates).
- **What's done**:
  - ADR-0007 written with the full fix design.
  - ADR-0024 verified the IVE `l1l3_collapse` wiring target
    (`src/ive/src/verification.rs:2379`).
  - The hardcoded signing key site is located at
    `src/codegen/src/capability.rs:117` (`b"vuma_dev_signing_key"`).
  - The FNV-1a × 4 signature function is located at
    `src/codegen/src/ipc.rs:996-1007`.
  - The `let known = true` stub branch is located at
    `src/ive/src/verification.rs:2379`.
- **What's left** (per ADR-0007 §Decision, as revised by ADR-0024):
  1. **Hand-written HMAC-SHA-256 in pure Rust** — ~300 LOC, no deps.
     This honors the hand-write philosophy (no `sha2`/`hmac` crates)
     and the 5-crate policy. The `womb/crypto/mac_kdf/hmac.vuma`
     module (193 LOC, RFC 2104) exists as a reference implementation
     in VUMA source form but cannot be consumed at build time without
     a VUMA self-compilation bootstrap (see Open Questions §11.2); the
     Rust re-implementation is the right path for v1.
  2. **Replace FNV-1a × 4** at `ipc.rs:996-1007` with HMAC-SHA256 over
     the canonical serialization of the capability token.
  3. **Replace hardcoded `b"vuma_dev_signing_key"`** at
     `capability.rs:117` with a per-process random 32-byte secret
     (read from `/dev/urandom` at compile time, never written to
     disk, never embedded in the binary).
  4. **Wire the IVE `l1l3_collapse` capability verifier** to actually
     check HMAC-SHA256 signatures instead of `let known = true`. This
     is the revised step (per ADR-0024): the verifier is at IVE
     compile time, NOT in emitted binaries. The capability model
     being compile-time-only is correct by design — only the `cap_id`
     (low 64 bits) survives into the emitted binary as
     `BinOp(Add, cap_id_immediate, 0)`.
  5. **Update the Lean model** to reflect the new signature scheme.
- **Dependencies**: should land after ADR-0005 (deps cleanup) to avoid
  merge conflicts in `Cargo.toml`. No deps on the bridge-fix epic.
- **Next action**: begin Phase 1 (hand-written HMAC-SHA-256 in
  `src/codegen/src/hmac_sha256.rs`, ~300 LOC, parity-tested against
  RFC 4231 test vectors).

---

## 5. Verification cluster

The verification cluster covers IVE / Lean proof / information-flow
work that is not in the security cluster.

### 5.1 V-14 (ADR-0006) — P1, defer f32 PMT Lean proof to v2

- **Status**: Open (deferred to v2). ADR-0006 mandates runtime
  `__float_overflow_trap` (exit 142) checks only for v1; no formal
  verification of f32 arithmetic in v1.
- **ADR**: [ADR-0006](adr/ADR-0006.md) (Accepted; effort revised
  3-6mo → 2-4wk bit-pattern / 2-3mo IEEE-754 by ADR-0011).
- **Layer**: VUMA (Lean proof layer).
- **Effort**: 2-4 weeks (bit-pattern model on top of `BitVec 32`) or
  2-3 months (full IEEE-754 with NaN/±inf/ULP/rounding). NOT 3-6
  months greenfield — the existing `BitVecArena` + Iris layer provides
  a foundation.
- **What's done**:
  - ADR-0006 written with the defer-to-v2 decision.
  - Runtime `__float_overflow_trap` exists on all 19 backends (exit
    142).
  - F-2 verified the foundation: 82 Lean files, 280 theorems, 0
    `sorry` tokens, real Iris separation-logic layer, real
    Lean↔Rust simulation refinement, real `BitVecArena` overflow
    model.
- **What's left** (for v2):
  1. Define `FloatArena` on top of `BitVec 32` (bit-pattern model).
  2. Define `verified_float_add` / `verified_float_sub` / etc. with
     NaN/inf propagation.
  3. Add `float_alloc_preserves_finite` lemma.
  4. (Optional, full IEEE-754): ULP error, rounding modes,
     distributivity, associativity.
- **Dependencies**: none. The v1 ship doesn't depend on this.
- **Next action**: defer to v2 planning. For v1, document the
  soundness gap in `docs/pmt-formal-spec.md`.

### 5.2 V-A3-3 (ADR-0008) — P1, fix `discharge_rate` denominator

- **Status**: Open (P1, low effort).
- **ADR**: [ADR-0008](adr/ADR-0008.md) (Accepted; confirmed by
  ADR-0011).
- **Layer**: VUMA (IVE summary output).
- **Effort**: 3 days (3-line fix + regression test).
- **What's done**:
  - ADR-0008 written.
  - The buggy formula located at `src/bin/compile_dump.rs:233-235`:
    `(100 * passed).checked_div(passed + unverified).unwrap_or(100)`.
    Excludes `failed` from denominator; `unwrap_or(100)` returns 100%
    on all-failed.
  - F-2 found that `VerificationSummary::pass_rate()` uses the correct
    denominator (`total_checked`); `compile_dump.rs` just doesn't call
    it.
- **What's left**:
  1. Replace the formula at `compile_dump.rs:233-235` with a call to
     `result.summary.pass_rate()`.
  2. Add a regression test that constructs a `VerificationSummary`
     with `failed > 0` and asserts the printed `discharge_rate` is
     less than 100%.
- **Dependencies**: none.
- **Next action**: land as a standalone PR. The metric is published in
  every compile summary, so the impact is visible.

### 5.3 V-A3-5 — P2, Lean `SessionType` behind Rust IVE by 4 variants

- **Status**: Open (P2).
- **ADR**: none — deferred.
- **Layer**: VUMA (Lean proof layer).
- **Effort**: 2 weeks.
- **What's done**: cataloged. The 4 missing variants are `Choice`,
  `Offer`, `Select`, `Branch` (per V-11's IVE-side work, which is
  already done as dead code at `src/ive/src/session_type.rs:38-56`).
- **What's left**:
  1. Extend the Lean `SessionType` inductive at `proof/PMT/IVE/Soundness/SessionType.lean`
     with the 4 missing variants.
  2. Re-prove session-type soundness for branching protocols.
- **Dependencies**: V-11 (AST/IR plumbing for Choice/Offer) should land
  first so the Lean model matches the Rust IVE.
- **Next action**: defer until V-11 lands. Track alongside V-11's ADR.

### 5.4 V-A3-8 — P1, `verify_information_flow_from_ir` misses indirect flows

- **Status**: Open (P1, preserved, not re-verified).
- **ADR**: none — deferred.
- **Layer**: VUMA (IVE information-flow verifier).
- **Effort**: 2 weeks.
- **What's done**: cataloged. The verifier at
  `src/ive/src/information_flow.rs` checks the Denning lattice
  `Public ⊑ Internal ⊑ Secret ⊑ TopSecret` over real vregs (not
  source names — the legacy "hardcode `Public` for every vreg"
  behavior was removed). However, it misses indirect flows: a `Store`
  of a `Secret`-labeled vreg to a memory cell followed by a `Load`
  from that cell into a `Public`-labeled vreg is not flagged as a
  leak.
- **What's left**:
  1. Write an ADR specifying the fix: add an alias-analysis pass
     before the information-flow check, track `Store`→`Load` flows
     through memory cells, and flag indirect leaks.
  2. Implement the fix.
  3. Add regression tests with indirect leak patterns.
- **Dependencies**: none.
- **Next action**: write the ADR. The current verifier is sound for
  direct flows but unsound for indirect flows — this matters for any
  program that buffers `#[secret]` data through memory.

---

## 6. Backend cluster

The backend cluster covers codegen bugs, softfloat stubs, dead-code
arms, SIMD, and per-backend regressions.

### 6.1 V-A2-7 (ADR-0025) — P2, HPPA F64 softfloat

- **Status**: Open (P2). F-3 REFUTED the v1 P1 framing for HPPA F64
  sub/mul/div: they ARE real IEEE 754 implementations at
  `hppa/mod.rs:2304-3031`. The P1 framing trusted a stale doc comment.
  The CORRECT remaining bugs are: F32 entirely stubbed, and F64 `lt`/
  `le` return 0 for negative operands.
- **ADR**: [ADR-0025](adr/ADR-0025.md) (Accepted, SIMD context).
- **Layer**: VUMA (hppa backend).
- **Effort**: 2 weeks (F32 softfloat + F64 lt/le fix).
- **What's done**: cataloged; F-3 verified the real F64 implementations.
- **What's left**:
  1. Implement F32 softfloat for HPPA (the F32 stub is at
     `hppa/mod.rs:3695-3699`).
  2. Fix F64 `lt`/`le` to handle negative operands correctly.
- **Dependencies**: none.
- **Next action**: defer until HPPA is a target for a real consumer
  (currently HPPA passes 98.29% of tests — not blocking).

### 6.2 V-A2-8 — P2, m68k F32 softfloat

- **Status**: Open (P2). m68k has full F64 via 68881 FPU; only F32
  register-operand arithmetic is missing.
- **ADR**: none — deferred.
- **Layer**: VUMA (m68k backend).
- **Effort**: 1 week.
- **What's done**: cataloged; F-3 confirmed the F32 stubs return 0.0
  for Register operands at `m68k/mod.rs:3904-3921`.
- **What's left**:
  1. Implement F32 register-operand arithmetic for m68k (or fall
     back to memory-operand arithmetic, which IS implemented).
  2. Add regression tests.
- **Dependencies**: none.
- **Next action**: defer. m68k currently passes 80.03% of tests —
  most failures are TO (timeout) from QEMU 7.2 TCG slowness, not
  F32 softfloat bugs.

### 6.3 V-A2-4 — P3, dead-code backend arms

- **Status**: Open (P3 cleanup). F-3 MOSTLY REFUTED the v1 P1 framing:
  the `=> {}` arms for `IRInstr::ChannelSend` / `StarkProof` / `BulkCopy`
  / `BulkFill` / `Transform` exist but are DEAD CODE in production. The
  active path is Call-form builtin → `ipc_lowering::lower_ipc_builtins`
  → `Syscall` / `Store` / `Load` / `BinOp`; the backend never sees the
  `IRInstr::ChannelSend` etc. variants.
- **ADR**: none — deferred (cleanup).
- **Layer**: VUMA (14+ backends).
- **Effort**: 3 weeks (cleanup across 14+ backends).
- **What's done**: cataloged; F-3 confirmed the dead-code status.
- **What's left**:
  1. Delete the unreachable `=> {}` arms across all backends.
  2. OR: convert them to `unreachable!()` with a debug-mode assertion
     that they are never reached.
- **Dependencies**: none.
- **Next action**: defer. The `mem_copy_buffer.vuma` failure is V-A2-1,
  not V-A2-4; the `stark_proof.vuma` failure is an `expand_stark_prove`
  lowering bug, not V-A2-4.

### 6.4 V-A2-2 — P1, `inttofloat`/`floattoint` hardcoded to I64↔F64

- **Status**: Open (P1, deferred — write ADR).
- **ADR**: none — deferred. ADR-0003 §"Scope note" explicitly excludes
  V-A2-2 from ADR-0003's scope: V-A2-2 lives on the SCG cast node, not
  the `PmtOpStmt` path.
- **Layer**: VUMA (codegen, SCG→IR cast lowering).
- **Effort**: 1 week.
- **What's done**: cataloged; the bug blocks f32 casts (e.g.,
  `1.5 as i32` and `42 as f32` are mis-lowered through the I64↔F64
  path). A-2 confirmed the path is hardcoded.
- **What's left**:
  1. Write an ADR specifying the fix: thread the source and target
     `IRType` through the SCG cast node, and emit the correct
     `inttofloat`/`floattoint` variant per type pair.
  2. Implement the fix.
  3. Add regression tests for `i32 as f32`, `f32 as i32`, `u32 as f32`,
     etc.
- **Dependencies**: ADR-0001 (V-34) ✅ — needed so f32 state fields
  have the correct IRType to cast from/to.
- **Next action**: write the ADR. Without this, `newton_sqrt`-style
  f32 algorithms that mix integer and float casts fail.

### 6.5 V-A2-3 (ADR-0025) — P1, SIMD vectorizer hardcodes Xmm0/1/2

- **Status**: Open (P1, Phase 1 of ADR-0025).
- **ADR**: [ADR-0025](adr/ADR-0025.md) (Accepted). ADR-0025 §Phase 1
  fixes V-A2-3 first, before any new SIMD ops are added.
- **Layer**: VUMA (x86_64 + aarch64 backends, vectorizer).
- **Effort**: 2 weeks.
- **What's done**: ADR-0025 written with the Phase 1 / Phase 2 / Phase
  3 design.
- **What's left**:
  1. **Phase 1**: Fix the vectorizer to use allocator-assigned physical
     registers from `RegAllocResult` instead of hardcoded `Xmm0/1/2`
     (x86_64) and `V0/1/2` (aarch64). This unblocks the existing
     `{Add, Sub, Mul} × {i32}` SIMD ops on both backends — no new ops
     needed.
  2. **Phase 2**: Add ops incrementally as the WOMB text shaper
     demands them (`pmaxsd`/`pminsd`, `pcmpeqd`/`cmeq.4s`,
     `pshufd`/`tbl.16b`, AVX2 if i64 SIMD is needed, AVX-512 skip
     for v1).
  3. **Phase 3**: AArch64 `2D` form (2×i64) only if a real consumer
     needs i64 SIMD on AArch64.
- **Dependencies**: none for Phase 1. Phase 2 depends on the WOMB text
  shaper existing (not a VUMA-layer concern).
- **Next action**: implement Phase 1 (the vectorizer fix). This must
  land BEFORE any new SIMD ops are added — otherwise the new ops will
  also be unusable.

### 6.6 V-13 (ADR-0025) — RESOLVED, incremental SIMD coverage

- **Status**: Resolved by ADR-0025. The original "deferred — needs
  benchmarking data" status is superseded; the existing `fp_simd`
  latency tables in `src/codegen/src/target_desc.rs` ARE benchmark
  data (at the instruction level), and `tests/gold_standard/float_advanced/fp_bench.vuma`
  provides workload-level data.
- **ADR**: [ADR-0025](adr/ADR-0025.md) (Accepted).
- **Layer**: VUMA (SIMD coverage).
- **Effort**: Phase 1: 2 weeks (V-A2-3 fix). Phase 2: incremental,
  ~2-4 weeks spread across months 6-12 of WOMB text-shaper development.
- **What's done**: ADR-0025 written; benchmark data located.
- **What's left**: see V-A2-3 above (Phase 1) + Phase 2 incremental.
- **Dependencies**: V-A2-3 must land first.
- **Next action**: implement V-A2-3 Phase 1.

### 6.7 V-S390X-1 (new) — s390x -1.02pp regression from `1d72d296`

- **Status**: Open (newly filed, P1, deferred — write ADR).
- **ADR**: none — deferred. Surfaced by Wave AD/AE investigation
  (`docs/research/AD-AE-s390x-w1-investigation.md`).
- **Layer**: VUMA (s390x backend + regalloc liveness).
- **Effort**: 1-2 weeks of s390x-specific regalloc debugging (estimate;
  no ADR yet).
- **What's done**: cataloged; root cause identified. The `1d72d296`
  phi+regalloc liveness fix IMPROVED 12 backends (+0.57 to +1.53pp)
  but REGRESSED s390x (-1.02pp, 16 new failures). The s390x backend
  has unique characteristics that interact badly with the new
  CFG-based liveness computation: big-endian, 5 arg regs (R2-R6),
  SVC 0 syscall convention, no dedicated condition codes register
  (uses PSW mask). Most new failures are MM (mismatch — wrong numeric
  result), suggesting the regalloc liveness fix is assigning wrong
  registers for s390x-specific code patterns. The 3 CR (crash) and
  1 TO (timeout) suggest more severe corruption in specific cases.
- **What's left**:
  1. Write an ADR specifying the fix: investigate the s390x
     `TargetDesc` / register-class setup for an assumption that the
     old position-based liveness satisfied by accident.
  2. Implement the fix.
  3. Add regression tests for the 16 new failures.
- **Dependencies**: none. NOT a blocker for V-34 (which is fixed).
- **Next action**: file as a tracked regression and assign to the
  backend team. The 16 new failures are listed in
  `docs/research/AD-AE-s390x-w1-investigation.md` §s390x.

### 6.8 W1-sparc64 — `c041517f` branch-based Cmp incomplete

- **Status**: Open (in-flight, not a catalog entry — surfaced by Wave
  AE). The sparc64 backend regressed -3.81pp from the W1-sparc64 work
  (COND_ constant fixes + branch-based Cmp handler). The branch-based
  Cmp approach is incomplete — it fixed some comparison kinds but
  introduced 60 new failures for others.
- **ADR**: none — deferred (in-flight work).
- **Layer**: VUMA (sparc64 backend).
- **Effort**: 1-2 weeks of iteration.
- **What's done**: commit `c041517f` re-applies both fixes (COND_
  constants + branch-based Cmp) in a single commit, after two
  reverts. The COND_ constant fixes are clearly correct (they were
  swapped); the branch-based Cmp approach is the source of
  regressions.
- **What's left**:
  1. **Recommended** (per `docs/research/AD-AE-s390x-w1-investigation.md`):
     cherry-pick the COND_ constant fixes (keep), revert the
     branch-based Cmp approach (investigate further).
  2. OR: keep both fixes and iterate on the failing tests.
- **Dependencies**: none.
- **Next action**: cherry-pick per the recommendation. sparc64 is
  currently at 82.24% (or 80.22% per the latest 14:36 UTC snapshot);
  the W1 work made it worse, not better.

### 6.9 W1-x86_32 — `314b2987` mprotect arg conversion incomplete

- **Status**: Open (in-flight, not a catalog entry — surfaced by Wave
  AE). The x86_32 backend regressed -4.25pp from the W1-x86_32 work
  (Call handler args 5+ on stack + mprotect stub arg conversion).
- **ADR**: none — deferred (in-flight work).
- **Layer**: VUMA (x86_32 backend).
- **Effort**: 1-2 weeks of iteration.
- **What's done**: commit `314b2987` re-applies the Call handler fix
  with a tweak ("arg conversion"). The Call handler fix for args 5+
  on stack is clearly correct per i386 SysV ABI; the regressions are
  likely in the mprotect arg conversion.
- **What's left**:
  1. **Recommended** (per `docs/research/AD-AE-s390x-w1-investigation.md`):
     keep the Call handler fix, iterate on the mprotect arg
     conversion.
- **Dependencies**: none.
- **Next action**: iterate on the mprotect arg conversion. x86_32 is
  currently at 79.20%, the weakest backend.

---

## 7. Parser / SCG cluster

### 7.1 V-11 — P1, session types `Choice`/`Offer` (AST/IR + parser + Lean)

- **Status**: Open (P1, deferred — write ADR). IVE-side done as dead
  code; AST/IR + parser + Lean remain.
- **ADR**: none — deferred. ADR-0001 §"Alternatives" mentions session
  types in passing but does not cover them.
- **Layer**: VUMA (parser AST + codegen IR + IVE Lean proof).
- **Effort**: 2 weeks (down from 2-4 weeks because IVE-side work is
  done).
- **What's done**:
  - IVE-side `SessionType` already has `Choice`/`Offer` as dead code
    at `src/ive/src/session_type.rs:38-56`.
- **What's left**:
  1. Add `Choice(Vec<SessionType>)` and `Offer(Vec<SessionType>)` to
     the AST `SessionType` enum at `src/parser/src/ast.rs:1632-1647`.
  2. Add the same variants to the IR `SessionType` enum at
     `src/codegen/src/ir.rs:167-176`.
  3. Add parser syntax for branching protocols (e.g.,
     `channel_open::<Choice<...>>`).
  4. Extend the IVE linear-type checker to handle branching (each
     branch must independently satisfy linearity).
  5. Re-prove session-type soundness in
     `proof/PMT/IVE/Soundness/SessionType.lean` (this is V-A3-5).
- **Dependencies**: V-A3-5 (Lean model update) is the last step.
- **Next action**: write the ADR with the syntax design. Blocks IME
  channels and any protocol with branching.

### 7.2 V-26 — P1, const byte arrays (needed for SPIR-V embedding)

- **Status**: Open (P1, deferred — write ADR). Needed for ADR-0022's
  hand-written SPIR-V backend.
- **ADR**: none — deferred. ADR-0022 §"What VUMA provides" calls out
  V-26 as the ONLY VUMA-side patch needed for the GPU path.
- **Layer**: VUMA (parser AST + codegen `.rodata` emission).
- **Effort**: 2 weeks.
- **What's done**:
  - The `Lit` enum (`src/parser/src/ast.rs:1511-1525`) currently has
    `Int(i64)`, `Float(f64)`, `String(String)`, `Bool(bool)`,
    `Address(u64)` — no `Bytes(Vec<u8>)`, no `Array(Vec<Lit>)`.
  - The `Expr` enum has no `ArrayLit`/`BytesLit` variant.
  - `parse_primary` has no `TokenKind::LBracket` arm.
  - The codegen has no `.rodata` lowering for const byte arrays (const
    items are lowered as immediate scalars).
- **What's left**:
  1. Write an ADR specifying the syntax design: `b"..."` literal?
     `[u8; 4]: 0x01, 0x02, 0x03, 0x04`? `Expr::ArrayLit` vs
     `Lit::Bytes`? (ADR-0022 hints at `Lit::Bytes(Vec<u8>)` and
     `Expr::ArrayLit`.)
  2. Add `TokenKind::LBracket` arm to `parse_primary` → new
     `Expr::ArrayLit(Vec<Expr>)` variant.
  3. Add `Lit::Bytes(Vec<u8>)` or handle `ArrayLit` of `Int` literals
     as bytes.
  4. Add codegen path: emit `.rodata` section with bytes, lower the
     expression to a `Load` of the base address.
- **Dependencies**: none.
- **Next action**: write the ADR with the syntax design. Blocks SPIR-V
  embedding (ADR-0022), font subsetting, any module that needs a
  `.rodata` byte blob.

### 7.3 V-43 — P2, `infer_expr_type` returns variable NAMES not types

- **Status**: Open (P2, deferred).
- **ADR**: none — deferred.
- **Layer**: VUMA (parser, type inference).
- **Effort**: 1 week.
- **What's done**: cataloged.
- **What's left**:
  1. Write an ADR specifying the fix: return `Type` enum values, not
     `String` names, from `infer_expr_type`.
  2. Update all callers.
- **Dependencies**: none.
- **Next action**: defer. Low priority — the name-as-type pattern
  works for most cases but is a footgun.

### 7.4 V-47 — P2, `extract_state_write_target` only handles `DerefField`

- **Status**: Open (P2, deferred).
- **ADR**: none — deferred.
- **Layer**: VUMA (parser/SCG, state-write lowering).
- **Effort**: 1 week.
- **What's done**: cataloged. The function handles only the
  `DerefField` case; other write target patterns (`Index`, `Deref`,
  etc.) are silently dropped.
- **What's left**:
  1. Write an ADR specifying the fix: handle all `Expr` variants that
     can be write targets.
  2. Implement the fix.
  3. Add regression tests.
- **Dependencies**: none.
- **Next action**: defer.

### 7.5 V-48 — P2, `ConstantFolding` folds 3 ops, parses as f64, effectively dead

- **Status**: Open (P2, deferred).
- **ADR**: none — deferred.
- **Layer**: VUMA (codegen optimizer, `opt.rs`).
- **Effort**: 1 week.
- **What's done**: cataloged. The `ConstantFolding` pass folds only
  `Add`, `Sub`, `Mul` over `Int` literals parsed as `f64`, which is
  both too narrow (no `Div`, no `Mod`, no `Shl`/`Shr`, no `And`/`Or`)
  and wrong-typed (integers parsed as floats). Effectively dead code.
- **What's left**:
  1. Write an ADR specifying the fix: either delete the pass (it's
     dead) or extend it to cover all `BinOp` variants over the
     correct `IRType`.
  2. Implement the fix.
- **Dependencies**: none.
- **Next action**: defer. Cleanup, not urgency.

### 7.6 V-49 — P2, `NodeVisitor::dispatch` handles 10/28 `NodePayload` variants

- **Status**: Open (P2, deferred).
- **ADR**: none — deferred.
- **Layer**: VUMA (parser/SCG, visitor pattern).
- **Effort**: 1 week.
- **What's done**: cataloged. The visitor dispatch handles only 10 of
  28 `NodePayload` variants; the remaining 18 fall through to a
  no-op default.
- **What's left**:
  1. Write an ADR specifying whether to extend the dispatch or
     explicitly mark the unhandled variants as `unreachable!()` (if
     they are genuinely unreachable in the visitor's use cases).
  2. Implement the fix.
- **Dependencies**: none.
- **Next action**: defer.

### 7.7 V-45 — P3, stale `Lit::Float` doc comment

- **Status**: Open (P3, deferred).
- **ADR**: none — deferred.
- **Layer**: VUMA (parser AST docs).
- **Effort**: 1 day.
- **What's done**: cataloged. The doc comment at
  `src/parser/src/ast.rs` (on `Lit::Float`) claims the lexer doesn't
  produce floats; it does.
- **What's left**: delete or rewrite the stale doc comment.
- **Dependencies**: none.
- **Next action**: defer. Trivial cleanup.

---

## 8. CI / build cluster

### 8.1 V-A3-4 — P2, `lean-rust-parity.yml` tests non-existent FFI bridge

- **Status**: Open (P2, deferred).
- **ADR**: none — deferred.
- **Layer**: VUMA (CI configuration).
- **Effort**: 1 day.
- **What's done**: cataloged. The `lean-rust-parity.yml` CI workflow
  tests a Lean↔Rust FFI bridge that was removed (the Lean FFI bridge
  is gone per `build.rs` file-level doc and the `verification.rs`
  "Lean FFI bridge removed" comment).
- **What's left**:
  1. Delete the workflow, OR repurpose it to test the standalone Lean
     proofs (via `lake build`) against the Rust hand-translations in
     `src/codegen/src/pmt_check.rs`.
- **Dependencies**: none.
- **Next action**: defer. CI is running, just wastefully.

### 8.2 V-NEW-6 — P2, `ci_run_tests.sh:61` pass criterion is "didn't crash"

- **Status**: Open (P2, deferred).
- **ADR**: none — deferred.
- **Layer**: VUMA (CI test harness).
- **Effort**: 3 days.
- **What's done**: cataloged. The pass criterion at
  `scripts/ci_run_tests.sh:61` checks that the test binary exited
  normally (didn't crash), not that it produced the correct exit
  code. Tests that exit 0 with the wrong answer pass.
- **What's left**:
  1. Write an ADR specifying the fix: compare the test's exit code
     against the expected exit code (already recorded in the test
     manifest).
  2. Implement the fix.
  3. Audit existing CI runs for false-pass tests.
- **Dependencies**: none.
- **Next action**: defer. The false-passes are a real concern but the
  93.67% baseline is from the real `pi5_test_suite.sh` (which DOES
  check exit codes), not from `ci_run_tests.sh`.

### 8.3 V-NEW-7 — P2, duplicate `lean-proofs` job

- **Status**: Open (P2, deferred).
- **ADR**: none — deferred.
- **Layer**: VUMA (CI configuration).
- **Effort**: 1 day.
- **What's done**: cataloged. The `lean-proofs` job is duplicated
  between `ci.yml` and `proof-verify.yml`.
- **What's left**:
  1. Delete one of the duplicates (keep the one in
     `proof-verify.yml`, remove from `ci.yml`).
- **Dependencies**: none.
- **Next action**: defer.

### 8.4 V-NEW-8 — P2, full 19-backend × 1577-test matrix NOT in CI

- **Status**: Open (P2, deferred).
- **ADR**: none — deferred.
- **Layer**: VUMA (CI configuration).
- **Effort**: 1 week.
- **What's done**: cataloged. CI runs only 7 backends × 47 examples;
  the full 19-backend × 1577-test matrix is run manually via
  `scripts/pi5_test_suite.sh` and snapshots are committed to
  `test_results/`. The 12 backends not in CI include the weakest
  (m68k, ppc64, ppc64le, sparc64, x86_32) — they can be silently
  broken between snapshot runs.
- **What's left**:
  1. Add a "second-tier" CI job that runs the full matrix nightly
     (not on every PR).
  2. Wire the job to fail on regressions from the last green
     snapshot.
- **Dependencies**: ADR-0009 (re-run on main HEAD) is DONE — the
  fresh baseline is `93.67%` (or `93.57%` per the latest snapshot).
- **Next action**: defer. The nightly job is a "should-have" but not
  a blocker.

### 8.5 V-WOMB-1 (ADR-0020) — P1, broken `womb/net/*.vuma` imports

- **Status**: Open (P1). Resolved by ADR-0020 but the fix is WOMB-side
  (path string updates in 8 `womb/net/*.vuma` files); the CI check
  that would catch this kind of bug is VUMA-side (V-NEW-8).
- **ADR**: [ADR-0020](adr/ADR-0020.md) (Accepted).
- **Layer**: WOMB (the broken files are WOMB-layer), but the CI check
  that surfaces it is VUMA-side (cross-layer concern).
- **Effort**: 1 day.
- **What's done**: ADR-0020 written with the 8 affected files and
  their broken imports. The VUMA module resolver at
  `src/parser/src/resolver.rs:500-510` is just `base_dir.join(path)`
  with no fallback — broken imports fail at compile time IF
  triggered. They are not triggered because
  `scripts/test_womb_compile.sh` compiles each `.vuma` file as a
  standalone unit (does not follow imports).
- **What's left**:
  1. Update the 8 broken imports in `womb/net/{ssh,quic,tls12,tls13,
     http2,http3_mqtt_coap,websocket}.vuma` and
     `womb/lib/sys/email.vuma` to point to the correct
     `crypto/mac_kdf/hmac.vuma` paths.
  2. Add a CI check that compiles `womb/net/*.vuma` with import
     following enabled (V-NEW-8).
- **Dependencies**: none.
- **Next action**: land the path-string updates. The files are
  effectively dead code today.

### 8.6 V-41 — P3, stale doc references

- **Status**: Open (P3, deferred).
- **ADR**: none — deferred.
- **Layer**: VUMA (docs).
- **Effort**: 2 days.
- **What's done**: cataloged. Stale references that still need fixing:
  - Anywhere that still cites `regalloc.rs:2307` or `regalloc.rs:2899`
    (correct lines are `:1284` and `:2966` per Wave-2).
  - Anywhere that still says "15 of 19 backends" or "18 of 19: 14
    native + 4 wrappers" (correct is "all 19 backends: 14 native + 4
    wrappers + 1 stack machine" per Wave-6 commit `f714a7a5`).
  - Anywhere that references `arm64.rs` (correct is
    `aarch64/{mod,reg_isel}.rs` since W7-impl).
- **What's left**:
  1. Grep across `docs/` for the stale patterns.
  2. Update each.
- **Dependencies**: none.
- **Next action**: defer. Documentation cleanup.

### 8.7 ADR-0009 — Re-run test suite on main HEAD — DONE

- **Status**: Done. The fresh results are in `test_results/summary.json`
  (timestamped `2026-08-01 14:36:54 UTC` on host
  `turbogp.benchmarks.0x01`).
- **ADR**: [ADR-0009](adr/ADR-0009.md) (Accepted; QEMU 10.0+ mandate
  confirmed by ADR-0011).
- **What's done**: full 19-backend × 1577-test matrix re-run on `main`
  HEAD with QEMU 10.0.11. Baseline is `93.67%` (per
  `docs/test-report-waves-s-z.md`), or `93.57%` per the latest
  snapshot.
- **What's left**: ongoing triage. The 9 backends fixed by `1d72d296`
  (x86_64, aarch64, riscv64, arm32, mips64, loongarch64, s390x,
  alpha, hppa) need only confirm-pass. The 4 still-broken backends
  (m68k, ppc64, ppc64le, x86_32) plus the regressed s390x and the
  in-flight sparc64/x86_32 are the actual triage targets.

---

## 9. Dependency policy

VUMA's dependency policy is locked by two ADRs:

- [ADR-0010](adr/ADR-0010.md): hard cap of **5 external crates** for
  the VUMA compiler crates, with an ADR gate for any addition.
- [ADR-0005](adr/ADR-0005.md): delete the `cc` build-dependency, the
  legacy `bridge_type_size` function, and (transitively)
  `find-msvc-tools` and `shlex`.

### 9.1 Current state (8 external crates)

| Crate | Declared at | Transitive of | Role | Post-ADR-0005 |
|---|---|---|---|---|
| `bitflags` | `Cargo.toml:65` | — | bitset macros | KEEP |
| `cc` | `Cargo.toml:75` | — | UNUSED (Lean FFI bridge removed) | **DELETE** |
| `find-msvc-tools` | (Cargo.lock) | `cc` | MSVC toolchain discovery | **DELETE** |
| `shlex` | (Cargo.lock) | `cc` | shell-style tokenizer | **DELETE** |
| `z3` | `src/ive/Cargo.toml:22` | — | SMT solver; the "V" in VUMA | KEEP |
| `z3-sys` | (Cargo.lock) | `z3` | FFI bindings to libz3 | KEEP |
| `log` | (Cargo.lock) | `z3` | logging facade | KEEP |
| `pkg-config` | (Cargo.lock) | `z3-sys` | libz3 system-library discovery | KEEP |

### 9.2 Post-ADR-0005 state (5 external crates)

| Crate | Role | Why kept |
|---|---|---|
| `bitflags` | bitset macros for IR/capability flags | Replaces hand-written bitset boilerplate; macro-only, no runtime |
| `z3` | SMT solver; the "V" in VUMA | The entire IVE depends on it |
| `z3-sys` | FFI bindings to libz3 | Transitive of `z3` |
| `log` | logging facade | Transitive of `z3` |
| `pkg-config` | libz3 system-library discovery | Transitive of `z3-sys` |

This satisfies ADR-0010's 5-crate cap exactly.

### 9.3 Hand-write philosophy

VUMA hand-writes:

- **Lexer NFA** — no `logos`, no `nom`.
- **TOML parser** (for `vuma.toml`) — no `toml` crate.
- **JSON encoder** (for telemetry) — no `serde_json`.
- **HMAC-SHA-256** (per ADR-0007) — no `sha2`/`hmac` crates. ~300 LOC.
- **All 19 backends** — no LLVM, no Cranelift, no MLIR.
- **E-graph rewriter** (`src/codegen/src/egraph.rs`, 3235 lines) — no
  `egg` crate. Includes the `state_store_load_forward` rewrite rule
  that VEEE's incremental computation will reuse.
- **Proof-carrying rewrites** (`src/codegen/src/bv_verify.rs`,
  `src/codegen/src/proof_artifacts.rs`).
- **SPIR-V bytecode emission** (per ADR-0022) — no MLIR, no
  glslangValidator Rust bindings. The build script invokes
  glslangValidator as a build-time tool (like Z3 and QEMU), not as a
  Rust crate.

The hand-write philosophy is the reason ADR-0015 (Cranelift dev track)
and ADR-0018 (MLIR→SPIR-V GPU track) were SUPERSEDED by ADR-0023 (VUMA
codegen with `--dev` flags) and ADR-0022 (hand-written SPIR-V). Both
superseding ADRs honor the philosophy and the 5-crate cap.

---

## 10. Execution plan (revised with V-34 done)

The v2 research draft's "Phase 1: True P0 (V-A3-2)" + "Phase 2:
Security cluster" is now the headline work. The bridge-fix epic has
collapsed to one fixed item (V-34) plus a P1 lockstep (V-03+V-NEW-2)
and two P1 deferred items needing ADRs (V-46, V-NEW-1).

### Phase 1: V-34 — DONE ✅

- **Effort**: 3 days (1-line fix + regression tests).
- **Status**: Fixed on `main` at commit `a58dee80`.
- **Validation**: `scripts/v34_test.vuma` exits 4 (correct) on x86_64
  per `docs/test-report-waves-s-z.md` §2.

### Phase 2: Security cluster (V-A3-2 + V-16 + V-A3-6) — 7 weeks

- **Effort**: 5 weeks code + 2 weeks Lean model updates.
- **Dependencies**: should land after ADR-0005 (deps cleanup) to avoid
  merge conflicts in `Cargo.toml`.
- **Sub-phases**:
  1. **Phase 2a** (1 week): Hand-written HMAC-SHA-256 in
     `src/codegen/src/hmac_sha256.rs`, ~300 LOC, parity-tested
     against RFC 4231 test vectors. Replaces FNV-1a × 4 in
     `ipc.rs:996-1007`.
  2. **Phase 2b** (3 days): Replace hardcoded `b"vuma_dev_signing_key"`
     at `capability.rs:117` with per-process random 32-byte secret.
  3. **Phase 2c** (3 weeks): Wire the IVE `l1l3_collapse` capability
     verifier at `verification.rs:2379` to actually check HMAC-SHA256
     signatures instead of `let known = true`.
  4. **Phase 2d** (2 weeks): Update the Lean model in `proof/PMT/`
     to reflect the new signature scheme. Re-prove capability
     soundness lemmas.
- **Milestone**: capability model no longer security theater.

### Phase 3: IVE soundness (V-03 + V-NEW-2 + V-40) — 2 weeks

- **Effort**: 1 week + 3 days + 1 day = ~10 days, must land as one PR.
- **Dependencies**: ADR-0001 (V-34) ✅.
- **Sub-phases**:
  1. **Phase 3a** (1 week): Migrate `build_pmt_layout_specs` to
     `bridge_type_size_with_layouts` (Change 1 of ADR-0004).
  2. **Phase 3b** (3 days): Migrate IVE `rederive_layout` in lockstep
     (Change 2 of ADR-0004).
  3. **Phase 3c** (1 day): Delete legacy `bridge_type_size` + delete
     `cc` build-dep + transitive `find-msvc-tools`/`shlex` (Change 3
     of ADR-0004 / ADR-0005).
- **Milestone**: sound IVE discharge on programs with nested layouts;
  dependency manifest at 5 crates.

### Phase 4: Backend stabilization — 4-6 weeks (parallel with Phase 2/3)

- **Sub-phases**:
  1. **Phase 4a** (1-2 weeks): V-S390X-1 — investigate s390x
     regalloc liveness regression from `1d72d296`. Write the ADR,
     implement the fix, add regression tests for the 16 new failures.
  2. **Phase 4b** (1-2 weeks): W1-sparc64 — cherry-pick COND_ constant
     fixes, revert branch-based Cmp per the AD-AE-s390x-w1
     investigation recommendation.
  3. **Phase 4c** (1-2 weeks): W1-x86_32 — keep Call handler fix,
     iterate on mprotect arg conversion.
  4. **Phase 4d** (1 week): V-A2-8 — m68k F32 softfloat. Defer until
     m68k is a target for a real consumer.
  5. **Phase 4e** (2 weeks): V-A2-7 — HPPA F32 softfloat + F64 lt/le
     fix. Defer until HPPA is a target for a real consumer.
  6. **Phase 4f** (1 week): V-A2-2 — `inttofloat`/`floattoint` I64↔F64
     hardcoding. Write the ADR, implement the fix, add regression
     tests.
  7. **Phase 4g** (2 weeks): V-A2-3 — SIMD vectorizer fix (Phase 1 of
     ADR-0025). Land BEFORE any new SIMD ops.
- **Milestone**: 4 weakest backends (m68k, ppc64, ppc64le, x86_32)
  above 90%; s390x, sparc64 regressions resolved.

### Phase 5: Parser gaps — 4-5 weeks (parallel)

- **Sub-phases**:
  1. **Phase 5a** (2 weeks): V-26 — const byte arrays. Write the ADR
     with syntax design, implement parser AST + codegen `.rodata`
     emission. Blocks SPIR-V embedding (ADR-0022).
  2. **Phase 5b** (2 weeks): V-11 — session types `Choice`/`Offer`.
     Write the ADR with syntax design, implement AST + IR + parser +
     IVE checker + Lean proof (V-A3-5).
  3. **Phase 5c** (deferred): V-43, V-47, V-48, V-49 — parser/SCG
     cleanups.
- **Milestone**: VUMA parser supports const byte arrays and branching
  session types.

### Phase 6: CI hardening — 2-3 weeks (parallel)

- **Sub-phases**:
  1. **Phase 6a** (1 day): V-A3-4 — delete or repurpose
     `lean-rust-parity.yml`.
  2. **Phase 6b** (1 day): V-NEW-7 — delete duplicate `lean-proofs`
     job.
  3. **Phase 6c** (3 days): V-NEW-6 — fix `ci_run_tests.sh:61` pass
     criterion to check exit codes.
  4. **Phase 6d** (1 week): V-NEW-8 — add nightly full-matrix CI job.
  5. **Phase 6e** (1 day): V-WOMB-1 — fix broken `womb/net/*.vuma`
     imports per ADR-0020.
  6. **Phase 6f** (2 days): V-41 — stale doc references.
- **Milestone**: CI catches regressions on all 19 backends; no
  false-pass tests; no duplicate jobs.

### Phase 7: Verification cluster — 4-6 weeks (parallel)

- **Sub-phases**:
  1. **Phase 7a** (3 days): V-A3-3 — fix `discharge_rate` denominator
     per ADR-0008.
  2. **Phase 7b** (deferred to v2): V-14 — f32 PMT Lean proof. 2-4
     weeks (bit-pattern) or 2-3 months (IEEE-754) on top of
     `BitVecArena`.
  3. **Phase 7c** (2 weeks): V-A3-8 — `verify_information_flow_from_ir`
     indirect flows. Write the ADR, implement the fix.
- **Milestone**: IVE `discharge_rate` is accurate; information-flow
  verifier catches indirect leaks.

### Total revised epic

~20-25 weeks of VUMA-layer work, of which ~7 weeks (Phase 2 security
cluster) is the only true P0 urgency. The rest can ship in any order
after Phase 1 (DONE) and Phase 3 (IVE soundness lockstep).

---

## 11. Open questions

Items that still need design work before an ADR can be written.

### 11.1 Unified `VumaType` refactor

- **What**: replace the parser-side `Type` enum, the codegen-side
  `IRType` enum, and the IVE-side `PmtLayoutSpec` type table with a
  single unified `VumaType` enum that all three layers consume.
- **Why it matters**: would eliminate V-34, V-35, V-42, V-44, V-46,
  V-03, V-NEW-2, V-NEW-1 in one stroke.
- **Why it's deferred**: 2-3 week refactor touching every layer. The
  v2 reality is that those 8 bugs are mostly P2 and live in
  non-production paths (per ADR-0011); the refactor is architecturally
  correct but no longer urgent. The V-34 fix has already landed,
  removing the only true P0 from this cluster.
- **Next step**: separate RFC, not an ADR. Tracked but not actionable
  in the current epic.

### 11.2 Effect enum is deleted (ADR-0021) — VEEE/WOMB may need their own effect tracking

- **What**: ADR-0021 deletes the `Effect` enum, `EffectSet`,
  `analyze_program_effects`, and the `pipeline.rs:4431` debug-logging
  call site. IVE has zero references to `Effect`; the optimizer uses
  its own `has_side_effects` at `opt.rs:527`.
- **Why it matters**: VEEE's incremental computation (ADR-0016) and
  WOMB's reactive runtime may need their own effect-tracking
  infrastructure. If they do, they should track effects in their own
  layer (VEEE or WOMB), not in VUMA IR.
- **Open question**: does the VEEE/WOMB reactive model need an effect
  enum, or can it be modeled entirely through `State<T>` +
  `layout_mark_dirty` calls (the incremental-computation lowering per
  ADR-0016)?
- **Next step**: defer to the VEEE/WOMB fine drafts.

### 11.3 f32-literal-immediate materialization bug (separate from V-34)

- **What**: there is a separate f32-literal-immediate materialization
  bug that makes `newton_sqrt` fail even with V-34 fixed. The
  `materialize_f32_immediates` pass at `src/codegen/src/opt.rs`
  (load-bearing — must run after folding and before codegen to avoid
  f32-bit-immediate corruption on x86_64) has a bug where f32
  literals are sometimes materialized as f64 immediates and then
  bit-truncated.
- **Why it matters**: blocks f32-heavy algorithms (Newton-Raphson
  square root, f32-based iterative solvers).
- **Why it's not V-34**: V-34 is about the IRType of state fields;
  this bug is about the materialization of f32 literal immediates in
  the optimizer.
- **Next step**: file a new catalog entry (proposed: V-NEW-9) and
  write an ADR with the fix design.

### 11.4 reg_isel F32 BinOp support (currently falls back to stack-slot ISel)

- **What**: some `reg_isel.rs` backends do not handle `BinOp` with
  `IRType::F32` and fall back to the stack-slot ISel path. This is
  not a `contains_fork` opt-out (which is correct per V-A2-9 REFUTED);
  it's a genuine gap in the register-based emitter's F32 BinOp
  coverage.
- **Why it matters**: f32 arithmetic on these backends goes through
  the slower stack-slot path; correctness is preserved but
  performance suffers.
- **Open question**: which backends have this gap? Is it the same set
  as the V-A2-7/V-A2-8 softfloat-stub backends (hppa, m68k), or does
  it affect other backends too?
- **Next step**: audit `reg_isel.rs` across all 15 substantive
  backends for F32 BinOp coverage. File catalog entries per backend
  as needed.

### 11.5 Canonical HMAC location (cross-layer bootstrap)

- **What**: ADR-0007 mandates hand-written HMAC-SHA-256 in pure Rust
  inside `src/codegen/src/hmac_sha256.rs`. The `womb/crypto/mac_kdf/hmac.vuma`
  module (193 LOC, RFC 2104) exists as a reference implementation in
  VUMA source form but cannot be consumed at build time without a
  VUMA self-compilation bootstrap.
- **Why it matters**: if VUMA plans to self-compile for other reasons
  (e.g., `json.vuma` for `vuma.toml` parsing, `http.vuma` for a
  package registry), the bootstrap would let VUMA consume
  `womb/crypto/mac_kdf/hmac.vuma` directly instead of maintaining a
  Rust re-implementation.
- **Why it's deferred**: VUMA self-compilation is substantial
  infrastructure not planned for v1. The Rust re-implementation is
  the right path for v1.
- **Next step**: defer to a future "VUMA self-compilation bootstrap"
  RFC. The Rust re-implementation (per ADR-0007) is correct for v1.

---

## 12. References

### 12.1 ADRs

| ADR | Title | Status | Closes |
|---|---|---|---|
| [ADR-0001](adr/ADR-0001.md) | Fix `bridge_type_to_ir_type` to map f32/f64 | Accepted (severity: P0→P1 by ADR-0011, then REVERTED to P0 by Wave S-Z) | V-34 |
| [ADR-0002](adr/ADR-0002.md) | Fix `type_size_from_name` + `type_alignment` for layout names | Accepted (severity revised by ADR-0011: P0→P2) | V-35, V-42, V-44 |
| [ADR-0003](adr/ADR-0003.md) | Thread IRType through StateRead/StateWrite + fix `Alloc { size: 0 }` | Accepted (severity revised by ADR-0011: P0→P2) | V-36, V-A2-1 |
| [ADR-0004](adr/ADR-0004.md) | Migrate `build_pmt_layout_specs` + IVE `rederive_layout` to `_with_layouts` | Accepted (framing revised by ADR-0011: P0→P1) | V-03, V-NEW-2, V-40 |
| [ADR-0005](adr/ADR-0005.md) | Delete unused build-deps + legacy `bridge_type_size` | Accepted | V-40, deps cleanup |
| [ADR-0006](adr/ADR-0006.md) | Defer f32 PMT Lean proof to v2; use runtime `__float_overflow_trap` only | Accepted (effort revised by ADR-0011) | V-14 |
| [ADR-0007](adr/ADR-0007.md) | Wire `verify_capability` + migrate to HMAC-SHA256 | Accepted (promoted by ADR-0024; severity P1→P0) | V-16, V-A3-2, V-A3-6 |
| [ADR-0008](adr/ADR-0008.md) | Fix `discharge_rate` denominator to include `failed` | Accepted | V-A3-3 |
| [ADR-0009](adr/ADR-0009.md) | Re-run full test suite on `main` HEAD | Accepted (DONE; QEMU 10.0+ mandated) | V-39 (stale baseline) |
| [ADR-0010](adr/ADR-0010.md) | Adopt "5 external crates maximum" dependency policy | Accepted | deps policy |
| [ADR-0011](adr/ADR-0011.md) | Re-audit corrections to ADR-0001 through ADR-0010 | Accepted | (meta-ADR) |
| [ADR-0012](adr/ADR-0012.md) | Adopt "VEEE" as the name for the UX language layer | Accepted | (VEEE-layer) |
| [ADR-0013](adr/ADR-0013.md) | Adopt the three-layer architecture (VUMA / WOMB / VEEE) | Accepted | (cross-layer) |
| [ADR-0014](adr/ADR-0014.md) | VEEE compiles to VUMA AST, not to VUMA IR | Accepted | (VEEE-layer) |
| [ADR-0015](adr/ADR-0015.md) | ~~VEEE backend strategy — Cranelift (dev) + VUMA codegen (prod) + MLIR→SPIR-V (GPU)~~ | **SUPERSEDED by ADR-0022 + ADR-0023** | (violates hand-write philosophy) |
| [ADR-0016](adr/ADR-0016.md) | VEEE's incremental computation engine lives in VEEE, not VUMA | Accepted | (VEEE-layer) |
| [ADR-0017](adr/ADR-0017.md) | VEEE's monotonicity types are a VEEE-layer type-system feature | Accepted | (VEEE-layer) |
| [ADR-0018](adr/ADR-0018.md) | ~~GPU path for VEEE goes through MLIR→SPIR-V~~ | **SUPERSEDED by ADR-0022** | (violates hand-write philosophy) |
| [ADR-0019](adr/ADR-0019.md) | WOMB UI modules live in `womb/ui/`; IrqRing generalizes to `womb/sync/` | Accepted | (WOMB-layer) |
| [ADR-0020](adr/ADR-0020.md) | Fix broken `womb/net/*.vuma` imports (V-WOMB-1) | Accepted | V-WOMB-1 |
| [ADR-0021](adr/ADR-0021.md) | Delete the `Effect` enum (it is dead code) | Accepted | V-A3-7 |
| [ADR-0022](adr/ADR-0022.md) | Hand-written SPIR-V backend (supersedes ADR-0018's MLIR approach) | Accepted | V-GPU, V-26 |
| [ADR-0023](adr/ADR-0023.md) | VEEE dev builds use VUMA's codegen with `--dev` flags, not Cranelift | Accepted | (supersedes ADR-0015's dev track) |
| [ADR-0024](adr/ADR-0024.md) | Promote ADR-0007 from Proposed to Accepted | Accepted | (promotes ADR-0007) |
| [ADR-0025](adr/ADR-0025.md) | Extend SIMD coverage incrementally | Accepted | V-13, V-A2-3 |

### 12.2 Research reports

- `docs/vuma-side-research-draft.md` — v1 rough draft (Wave A, superseded)
- `docs/vuma-side-research-draft-v2.md` — v2 corrected draft (Wave F)
- `docs/vuma-side-problem-catalog.md` — master catalog (49+ entries)
- `docs/test-report-waves-s-z.md` — empirical test results (Wave S-Z)
- `docs/research/AD-AE-s390x-w1-investigation.md` — s390x/W1 analysis
- `docs/research/A-1-*.md` through `A-4-*.md` — Wave A deep audit
- `docs/research/F-1-type-bridge-reality.md` — type-bridge re-audit
- `docs/research/F-2-ive-proof-reality.md` — IVE/Lean proof re-audit
- `docs/research/F-3-backend-test-reality.md` — backend/test re-audit
- `docs/research/J-1-womb-layer.md` — WOMB layer inventory
- `docs/research/K-1-veee-rename-design.md` — VEEE rename + three-layer design

### 12.3 Test reports

- `test_results/summary.json` — fresh baseline (2026-08-01 14:36:54 UTC,
  28036/29963 = 93.57%)
- `test_results/failures.txt` — 1896 failures across 437 tests

### 12.4 Source files cited

| File | Lines | What's there |
|---|---|---|
| `src/pipeline.rs` | 6503-6530 | `bridge_type_to_ir_type` (V-34 FIX landed) |
| `src/pipeline.rs` | 6532-6550 | legacy `bridge_type_size` (V-03/V-40) |
| `src/pipeline.rs` | 6557-6620 | `bridge_type_size_with_layouts` (the fixed variant) |
| `src/pipeline.rs` | 6625-6699 | `build_layout_registry` (correct multi-pass) |
| `src/pipeline.rs` | 6715-6756 | `build_pmt_layout_specs` (V-03 site) |
| `src/pipeline.rs` | 7400-7410 | `resolve_state_array_access` (V-46 site) |
| `src/pipeline.rs` | 9228, 9297, 9598 | `allocate(<non-literal>)` truncation (V-NEW-1) |
| `src/parser/src/to_scg.rs` | 3846, 4057-4065 | `type_alignment` + `type_size_from_name` (V-35, V-44) |
| `src/codegen/src/scg_to_ir.rs` | 6002-6028 | `StateRead`/`StateWrite` (V-36) |
| `src/codegen/src/scg_to_ir.rs` | 6044-6067 | `StateInit`/`ArenaNew`/`ArenaAlloc` (V-A2-1) |
| `src/ive/src/verification.rs` | 264-267 | parity docstring (V-NEW-2 rationale) |
| `src/ive/src/verification.rs` | 268-291 | `rederive_layout` (V-NEW-2) |
| `src/ive/src/verification.rs` | 2379 | `let known = true` stub (V-A3-6) |
| `src/codegen/src/capability.rs` | 117-118 | hardcoded `b"vuma_dev_signing_key"` (V-A3-2) |
| `src/codegen/src/ipc.rs` | 996-1007 | FNV-1a × 4 signature (V-16) |
| `src/codegen/src/regalloc.rs` | 2836-2897 | `resolve_register_reuse_conflicts` (V-A2-9 REFUTED) |
| `src/codegen/src/effects.rs` | (whole file) | dead `Effect` enum (V-A3-7, ADR-0021) |
| `src/codegen/src/hppa/mod.rs` | 2304-3031, 3695-3699 | F64 softfloat (real), F32 stub (V-A2-7) |
| `src/codegen/src/m68k/mod.rs` | 3904-3921 | F32 softfloat stub (V-A2-8) |
| `src/bin/compile_dump.rs` | 233-235 | `discharge_rate` formula (V-A3-3) |
| `proof/PMT/` | 82 files | 280 theorems, 0 sorries |

---

## Appendix A — Per-entry resolution table

Every VUMA-layer catalog entry, with status / ADR / layer / effort /
dependencies / next action.

| ID | Status | ADR | Layer | Effort | Deps | Next action |
|---|---|---|---|---|---|---|
| V-01 | Out of scope (WOMB/native-host) | — | WOMB | — | — | defer to WOMB draft |
| V-02 | Out of scope (GPU stack, depends on V-GPU) | ADR-0022 | cross-layer | — | V-GPU, V-26 | defer to cross-layer draft |
| V-03 | Open (P1) | ADR-0004 | VUMA | 1 week | ADR-0001 ✅ | land Phase 3a lockstep |
| V-04 | REDUNDANT (subsumed by V-03/V-35) | — | — | — | — | none |
| V-05 | REDUNDANT (already implemented at `pipeline.rs:8075`) | — | — | — | — | none |
| V-07 | REDUNDANT (`ArgMode` exists; 3-day `effects.rs` fix) | — | VUMA | 3 days | — | defer |
| V-08 through V-32 | Out of scope (WOMB-layer UI features) | — | WOMB | — | V-02 | defer to WOMB draft |
| V-11 | Open (P1, deferred — write ADR) | none | VUMA | 2 weeks | V-A3-5 | write the ADR |
| V-13 | Resolved by ADR-0025 | ADR-0025 | VUMA | Phase 1: 2wk; Phase 2: incremental | V-A2-3 | implement Phase 1 (V-A2-3 fix) |
| V-14 | Open (P1, defer to v2) | ADR-0006 | VUMA | 2-4wk bit-pattern / 2-3mo IEEE-754 | none | defer to v2 planning |
| V-16 | Open (P0) | ADR-0007 (promoted by ADR-0024) | VUMA | 7 weeks | ADR-0005 | begin Phase 2a (hand-written HMAC-SHA-256) |
| V-26 | Open (P1, deferred — write ADR) | none (ADR-0022 depends on it) | VUMA | 2 weeks | none | write the ADR with syntax design |
| V-27 | Out of scope (GPU stack) | ADR-0022 | cross-layer | — | — | defer to cross-layer draft |
| V-28 | Out of scope (GPU stack) | ADR-0022 | cross-layer | — | — | defer to cross-layer draft |
| V-34 | **FIXED** on `main` at `a58dee80` | ADR-0001 | VUMA | 3 days (DONE) | — | closed |
| V-35 | Open (P2) | ADR-0002 | VUMA | 1 week | none | implement when dormant stub is needed |
| V-36 | Open (P2) | ADR-0003 | VUMA | 1 week | ADR-0001 ✅, ADR-0004 | implement alongside V-03 lockstep |
| V-37 | REFUTED (padding IS at `pipeline.rs:6741-6744`) | — | — | — | — | none |
| V-39 | Resolved (ADR-0009 DONE; fresh baseline 93.67%/93.57%) | ADR-0009 | VUMA | ongoing | — | triage 4 broken backends + 3 regressions |
| V-40 | Open (P2, bundled as Change 3 of ADR-0004) | ADR-0005 | VUMA | 1 day | ADR-0004 | land alongside ADR-0004 |
| V-41 | Open (P3, deferred) | none | VUMA | 2 days | none | defer |
| V-42 | Subsumed by V-35 | ADR-0002 | VUMA | — | — | none |
| V-43 | Open (P2, deferred) | none | VUMA | 1 week | none | defer |
| V-44 | Open (P2) | ADR-0002 | VUMA | 2 days | none | implement when dormant stub is needed |
| V-45 | Open (P3, deferred) | none | VUMA | 1 day | none | defer |
| V-46 | Open (P1, deferred — write ADR) | none | VUMA | 1 week | ADR-0002 | write the ADR |
| V-47 | Open (P2, deferred) | none | VUMA | 1 week | none | defer |
| V-48 | Open (P2, deferred) | none | VUMA | 1 week | none | defer |
| V-49 | Open (P2, deferred) | none | VUMA | 1 week | none | defer |
| V-A2-1 | Open (P2) | ADR-0003 | VUMA | 1 week | ADR-0001 ✅, ADR-0004 | implement alongside V-03 lockstep |
| V-A2-2 | Open (P1, deferred — write ADR) | none | VUMA | 1 week | ADR-0001 ✅ | write the ADR |
| V-A2-3 | Open (P1, Phase 1 of ADR-0025) | ADR-0025 | VUMA | 2 weeks | none | implement Phase 1 (vectorizer fix) |
| V-A2-4 | Open (P3 cleanup) | none | VUMA | 3 weeks | none | defer |
| V-A2-5 | Open (P2, deferred) | none | VUMA | (not estimated) | none | defer |
| V-A2-6 | Open (P2, deferred) | none | VUMA | (not estimated) | none | defer |
| V-A2-7 | Open (P2; F64 sub/mul/div REFUTED, F32 stub + F64 lt/le real) | ADR-0025 (context) | VUMA | 2 weeks | none | defer until HPPA is a real target |
| V-A2-8 | Open (P2) | none | VUMA | 1 week | none | defer until m68k is a real target |
| V-A2-9 | **DROPPED** (REFUTED — `resolve_register_reuse_conflicts` explicitly models this) | — | — | — | — | none |
| V-A3-1 | Subsumed by V-03/V-NEW-2 | ADR-0004 | VUMA | — | — | none |
| V-A3-2 | Open (P0, part of security cluster) | ADR-0007 | VUMA | 1 week (subset of 7-week epic) | ADR-0005 | begin Phase 2b (replace hardcoded key) |
| V-A3-3 | Open (P1) | ADR-0008 | VUMA | 3 days | none | land as standalone PR |
| V-A3-4 | Open (P2, deferred) | none | VUMA | 1 day | none | defer |
| V-A3-5 | Open (P2, deferred) | none | VUMA | 2 weeks | V-11 | defer until V-11 lands |
| V-A3-6 | Open (P2, part of security cluster) | ADR-0007 | VUMA | 1 day (subset of 7-week epic) | ADR-0005 | begin Phase 2c (wire IVE verifier) |
| V-A3-7 | **RESOLVED** by ADR-0021 (delete `Effect` enum) | ADR-0021 | VUMA | 1 day | none | land the deletion |
| V-A3-8 | Open (P1, deferred — write ADR) | none | VUMA | 2 weeks | none | write the ADR |
| V-GPU | Greenfield, tracked separately | ADR-0022 | cross-layer | 3-6 months | V-26 | defer to cross-layer draft |
| V-NEW-1 | Open (P1, deferred — write ADR) | none | VUMA | 1 week | ADR-0002, ADR-0003 | write the ADR |
| V-NEW-2 | Open (P1, lockstep with V-03) | ADR-0004 | VUMA | 3 days | ADR-0001 ✅ | land Phase 3b lockstep |
| V-NEW-6 | Open (P2, deferred) | none | VUMA | 3 days | none | defer |
| V-NEW-7 | Open (P2, deferred) | none | VUMA | 1 day | none | defer |
| V-NEW-8 | Open (P2, deferred) | none | VUMA | 1 week | ADR-0009 ✅ | defer |
| V-WOMB-1 | Open (P1; fix is WOMB-side, CI check is VUMA-side) | ADR-0020 | WOMB (cross-layer) | 1 day | none | land the path-string updates |
| V-S390X-1 (new) | Open (P1, deferred — write ADR) | none | VUMA | 1-2 weeks | none | write the ADR; assign to backend team |

### Resolution count summary

- **Fixed / Resolved / Dropped / Refuted / Redundant**: 11 (V-34, V-04, V-05, V-07 [partial], V-13, V-37, V-39, V-42, V-A2-9, V-A3-1, V-A3-7, V-WOMB-1 [ADR-0020 written])
- **Open P0**: 3 (V-16, V-A3-2, V-A3-6 — all in the security cluster, all addressed by ADR-0007)
- **Open P1**: 8 (V-03, V-11, V-26, V-A3-3, V-A3-8, V-A2-2, V-A2-3, V-NEW-1, V-NEW-2, V-46, V-S390X-1)
- **Open P2**: 12 (V-35, V-36, V-40, V-43, V-44, V-47, V-48, V-49, V-A2-1, V-A2-5, V-A2-6, V-A2-7, V-A2-8, V-A3-4, V-A3-5, V-NEW-6, V-NEW-7, V-NEW-8)
- **Open P3**: 3 (V-41, V-45, V-A2-4)
- **Out of scope (WOMB/VEEE/cross-layer)**: 24 (V-01, V-02, V-08 through V-32, V-27, V-28, V-GPU)

(Counts overlap because some entries have multiple statuses — e.g.,
V-WOMB-1 has ADR-0020 written but the fix is not yet landed; V-07 is
partially REDUNDANT but still needs a 3-day `effects.rs` cleanup.)
