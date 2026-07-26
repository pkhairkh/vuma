# VUMA — Verified-Unsafe Memory Access

VUMA is a statically-typed systems programming language and compiler framework
whose distinguishing feature is **invariant verification at compile time**
augmented by **mandatory runtime bounds checks on arena-allocated accesses**.
The `--safe` CLI flag is always on (it is a no-op retained only for backward
compatibility): every `Seq` access into an arena-allocated state buffer is
preceded by an `UGe` bounds check that traps via the `__oob_trap` stub
(exit 134) on out-of-bounds access, so the runtime memory-safety overhead of
those arena accesses is intentionally **paid**, not avoided. Raw-pointer
arithmetic and `length_expr=None` accesses are not bounded and remain
unchecked (future SoftBound work). Source programs are parsed to an AST,
lifted to a Semantic Code Graph (SCG), verified by the Invariant Verification
Engine (IVE), lowered to a backend-neutral IR, optimized, register-allocated,
and emitted as native object code (ELF) or WebAssembly. The compiler targets
**19 production backends** spanning x86, ARM, RISC-V, MIPS, PowerPC, s390x,
SPARC, LoongArch, Alpha, HPPA, M68K, and wasm32. Programs are modeled as
**Programs as Memory Transformations (PMT)** — typed state transformations on
a single backing arena — and verified against liveness, exclusivity, origin,
interpretation, and cleanup invariants before code emission.

---

## 1. Key Metrics

| Metric | Value |
|--------------------------------|------------------------------------|
| Backends | 19 (15 Complete, 4 Experimental) |
| Gold-standard test programs | 1 589 (canonical: `tests/gold_standard/manifest.json`; run `make verify-manifest`)|
| Backend × test matrix coverage | 1 589 × 19 = 30 191 runs |
| Implementation language | Rust (≈ 370 K LOC across `src/`) |
| License | MIT |
| Minimum Rust toolchain | nightly-2026-03-01 (rustc 1.87+) |

---

## 2. Architecture

VUMA is organized as a 10-stage pipeline — parse → AST → SCG → IVE → IR →
IPC-lowering → opt → regalloc → backend → ELF/Wasm — implemented as a Cargo
workspace of nine Rust crates plus an LSP module and a package manager. The
default verification discipline (`VerificationLevel::Pmt`) treats every program
as a state-transformation over a typed backing arena and runs three PMT state
verifiers; the five legacy pointer invariants (liveness, exclusivity,
interpretation, origin, cleanup) are available at higher verification levels.
The full architecture, crate inventory, backend matrix, and verification
pipeline are documented in [docs/caveats.md](docs/caveats.md).

---

## 3. Quick Start

### 3.1 Build

Prerequisites:

- **Rust nightly-2026-03-01** (rustc 1.87+): `rustup toolchain install nightly-2026-03-01`
- **wasmtime** ≥ 47.0 (to execute `wasm32` binaries via `scripts/wasm32_runner.py`)
- **qemu-user-static** 7.2.0 — 14 arches (aarch64, arm, mips64, mipsel, mips64el, ppc64, ppc64le, riscv64, s390x, sparc64, m68k, hppa, alpha, loongarch64) for cross-arch execution of the non-native backends. The mips64 backend emits a little-endian MIPS64 ELF; use `qemu-mips64el-static`.

```bash
# Clone and build the compiler front-end + codegen.
git clone https://github.com/pkhairkh/vuma
cd vuma
cargo build --profile release-fast --bin compile_dump --bin dump_ir
# The release-fast profile disables LTO for ~5× faster iteration builds;
# the resulting binaries land in target/release-fast/.
```

### 3.2 Compile and run a test program

```bash
# Compile a VUMA source to an x86_64 ELF executable, then run it natively.
./target/release-fast/compile_dump examples/fibonacci.vuma out.bin x86_64
./out.bin; echo "exit=$?"

# Cross-compile to aarch64 and run under QEMU user-mode emulation.
./target/release-fast/compile_dump examples/fibonacci.vuma out.arm64 aarch64
qemu-aarch64 ./out.arm64; echo "exit=$?"

# Compile to wasm32 and run under wasmtime.
./target/release-fast/compile_dump examples/fibonacci.vuma out.wasm wasm32
python3 scripts/wasm32_runner.py out.wasm
```

### 3.3 Run the gold-standard suite

```bash
# Full 1 589 × 19 = 30 191-run matrix across all 19 backends under QEMU + wasmtime.
bash scripts/pi5_test_suite.sh --workers 4
cat test_results/summary.json
```

---

## 4. Documentation Index

| Document | Scope |
|-----------------------------------------------------|----------------------------------------------------|
| [docs/caveats.md](docs/caveats.md) | 10-stage pipeline, crates, backend matrix, verification |
| [docs/architecture/](docs/architecture/) | Per-subsystem audits (IVE, PMT, IPC, caveats) |
| [docs/caveats.md](docs/caveats.md) | Stage-by-stage compilation walkthrough |
| [docs/language-reference.md](docs/language-reference.md) | VUMA language reference: types, expressions, builtins, FFI |
| [docs/language/](docs/language/) | Tutorial, semantics, calling-convention notes |
| [docs/backends/](docs/backends/) | Per-backend ABI tables, ISel strategy, QEMU notes |
| [docs/testing/](docs/testing/) | Gold-standard harness, CI workflows, KAT vectors |
| [docs/building.md](docs/building.md) | Build prerequisites, quick start, troubleshooting |

The repository also ships reference material outside `docs/`:

- `examples/` — 52 self-contained `.vuma` programs (atomics, channels, sha256d,
 FFI, lock-free queue, doubly-linked list, mmap + sha256d).
- `womb/` — VUMA standard library: alloc, collections, crypto (hashes,
 symmetric, asymmetric, post-quantum, MAC/KDF, bignum), encoding, env, fs,
 graph, io, kernel, net (TCP, TLS 1.2/1.3, SSH, QUIC, HTTP/2/3, DNS,
 websocket), string, syscalls.
- `tests/gold_standard/` — gold-standard programs across the categories listed in `tests/gold_standard/manifest.json` (the manifest is the canonical source of truth; run `make verify-manifest` to confirm it matches the filesystem, or `make regen-manifest` to rebuild it after adding/removing `.vuma` files).
- `scripts/` — test runners (`pi5_test_suite.sh`, `wasm32_runner.py`), QEMU
 boot scripts, fuzz harnesses, KAT generators.

---

## 5. Formal Verification (Lean 4)

VUMA ships a machine-checked formalization of its PMT (Programs as Memory
Transformations) memory model in Lean 4. The proofs live in `proof/` and
verify:

- **Capacity preservation**: arena allocation never exceeds capacity.
- **Field-bounds safety**: field accesses stay within layout bounds.
- **Linearity / no-UAF**: consumed state buffers cannot be reused.
- **Guard page**: in-arena accesses never trip the mmap guard page.
- **Soundness**: well-typed programs either produce a result or trap with a
 canonical exit code (1, 134, or 135). No undefined behavior.
- **IVE soundness**: each `verify_*` predicate in the Invariant Verification
 Engine is proven sound (transform, state-reads, state-writes) — .
- **Simulation relation**: a step-wise simulation relation between the Lean
 model and the Rust `Arena` / `IRProgram` is defined and proven preserved
 by allocation and execution — .
- **Extraction correctness**: the Rust capacity/field-bounds/
 linearity/PMT checkers (`proof/extracted/pmt_check.rs`) are hand-translated
 from their Lean specifications and cross-checked by a parity test
 (FFI extraction deferred to Wave 1, IVE-1-*) — .
- **Iris invariants** `[cap_bnd]`, `[live_mirror]`, `[guard]`: formalized as
 proper separation-logic named invariants with ghost state (`ExRA` / `AgRA`
 / `Own` / `Sep`) and the Iris frame rule — -Iris / `proof/PMT/Iris/`.
- **ArenaRes**: formalises the arena resource bundle from
 `pmt-iris-spec.md` — packages `[cap_bnd]`, bump-pointer ghost, and
 cap-own into one exclusive resource; comes with
 `alloc_preserves_arena_res` and projection lemmas to `CapBndInv` /
 `CapacityInvariant` — `proof/PMT/Iris/ArenaRes.lean`.
- **FractionalPerm `↦{q}`**: Iris fractional permission for partial field
 ownership (the `q ∈ (0,1]` fragment from `pmt-iris-spec.md`) —
 `proof/PMT/Iris/FractionalPerm.lean`.
- **`wp` weakest-precondition calculus**: Iris `wp` on PMT instructions
 (Hoare-triple / weakest-precondition machinery from `pmt-iris-spec.md`) — `proof/PMT/Iris/WeakestPrecond.lean`. Together with `ArenaRes` and
 `FractionalPerm`, this brings the shipped Iris construct count to **6 / 17**
 of the constructs enumerated in `pmt-iris-spec.md`.
- **BitVecArena**: models the Rust `Arena` using `BitVec 64` (mirrors `usize`
 on 64-bit platforms), making `usize` arithmetic overflow — the actual
 failure mode `checked_add` defends against — syntactically expressible and
 distinct from capacity overflow — `proof/PMT/BitVecArena.lean`.
- **MmapArena**: models the `alloc::alloc`-returns-null failure path of
 `Arena::create` / `Arena::grow` via `raw_create: Nat → Except TrapCode
 RawArena`, closing the simulation-soundness gap in the arena-fidelity
 audit — `proof/PMT/MmapArena.lean`.
- **PipelineSim**: scaffolding for the Lean↔Rust pipeline-conformance
  theorem. Following CompCert's translation validation approach, it models
  the specification that `pipeline::compile` *will eventually* be required
  to meet (`PipelineSpec`), and proves Lean's own `exec` already meets that
  spec (`exec_satisfies_pipeline_spec`). The two headline theorems
  `pmt_soundness_restate` and `pmt_soundness_no_oob_restate` (renamed in
  PMT-0-C from `pipeline_compile_sound` / `pipeline_compile_no_oob`) are
  **direct restatements** of `pmt_soundness` /
  `no_oob_trap_for_well_typed_strong` — sorry-free, but with **no Rust-side
  hypothesis**, because the pre-PMT-0-C `hconforms : PipelineSpec prog s`
  hypothesis was unused in the proof bodies (the
  `PipelineSpec.compiled_matches_exec` field is `exec prog s = exec prog s`,
  a `rfl` tautology). The "real" pipeline-conformance theorem — one that
  discharges a non-vacuous `PipelineSpec prog s` tying Lean `exec` to the
  actual Rust `pipeline::compile` output — is deferred to Wave 1 PMT-1-G
  (extraction + Rust-parity testing) — `proof/PMT/PipelineSim.lean`.
- **Rust integration (runtime checkers)**: the verified checkers are
 hand-translated to Rust at `src/codegen/src/runtime/pmt_check.rs`, gated
 behind the `pmt-runtime-check` Cargo feature — **now WIRED into
 `arena.rs`** so the verified `verified_capacity_check` runs on every
 arena allocation in production (no longer a stub). `@[export]`
 attributes on `proof/PMT/Extraction.lean` (`lean_verified_capacity_check`,
 `lean_verified_field_bounds_check`, `lean_verified_linearity_check`,
 `lean_verified_pmt_check`) reserve C symbols for a future FFI bridge
 (Wave 1 — IVE-1-*; not yet linked; the production path runs the
 hand-translation in `src/codegen/src/runtime/pmt_check.rs`). The
 feature forwards from the root `Cargo.toml`, so
 `cargo build --features pmt-runtime-check` works from the repo root.
 A parity test (`tests/pmt_parity_test.rs`, 5 tests) plus a feature-flag
 wiring test (`tests/pmt_feature_flag_test.rs`, 3 tests) confirm the
 Rust matches the Lean semantics on all test cases and that the checker
 is reachable from the arena allocation path.

### 5.1 Building the Proofs

Install [elan](https://github.com/leanprover/elan) (Lean toolchain manager):

```bash
curl -sSf https://raw.githubusercontent.com/leanprover/elan/master/elan-init.sh | sh -s -- -y
source $HOME/.elan/env
```

Then build from the repo root via the Makefile (or `justfile` equivalents
`just proof`, `just proof-check`):

```bash
make proof # = cd proof && lake build
make proof-check # = ./scripts/check-lean.sh (verifies sorry-free)
make proof-test # = cd proof && lake exe test
make verify-all # = ./scripts/verify-all.sh (Lean + CI + docs)
```

Or directly from the `proof/` directory:

```bash
cd proof && lake build && lake exe test
```

See [docs/proof/W5-lake-build.md](docs/proof/W5-lake-build.md)
for the full Lake build matrix, toolchain pinning, and troubleshooting.

### 5.2 Proof Modules

The proof library comprises **23+ Lean modules** (~5,000 LOC) plus **9 test
modules** under `proof/PMT/Test/`. All theorems (~90) are proven without
`sorry` (only 6 explicitly documented hard-proof `sorry` stubs remain —
all in the Iris lemma modules, each tagged `-- TODO` with a counterexample
analysis). Module inventory:

| Module | Contents |
|----------------------------------------------|----------------------------------------------------------------|
| `proof/PMT/Basic.lean` | Arena, Field, Layout, CapacityInvariant |
| `proof/PMT/Field.lean` | FieldBounds, Linearity, LinearResource |
| `proof/PMT/Liveness.lean` | `state_read_requires_live`, GuardPage |
| `proof/PMT/Soundness.lean` | Step, WellTyped, `pmt_soundness` theorem (sorry-free) |
| `proof/PMT/RawArena.lean` | Faithful mirror of Rust `Arena` (pointers, alignment, lifecycle) |
| `proof/PMT/PmtInstr.lean` | Lean mirror of PMT-relevant `IRInstr` subset |
| `proof/PMT/IRProgram.lean` | Lean mirror of `IRProgram`/`IRFunction`/`IRBlock` |
| `proof/PMT/WellTypedStrong.lean` | Strengthened WellTyped (dataflow + field safety) |
| `proof/PMT/ExecFunction.lean` | IRFunction → Program flattening |
| `proof/PMT/SimRel.lean` | Simulation relation Lean ↔ Rust |
| `proof/PMT/BitVecArena.lean` | `BitVec 64`-based arena model — expresses `usize` arithmetic overflow |
| `proof/PMT/MmapArena.lean` | `raw_create` models `alloc::alloc`-returns-null (OOM) failure path |
| `proof/PMT/PipelineSim.lean` | `PipelineSpec` scaffolding + `pmt_soundness_restate` / `pmt_soundness_no_oob_restate` (PMT-0-C: degenerate `hconforms` removed; real conformance deferred to PMT-1-G) |
| `proof/PMT/ArenaProperties.lean` | Composition lemmas over `RawArena` (e.g. `raw_alloc_alive_succeeds`) |
| `proof/PMT/AdditionalTheorems.lean` | Extra soundness/correctness theorems |
| `proof/PMT/{Helper,IR,Misc}Lemmas.lean` | Supporting lemma libraries used across modules |
| `proof/PMT/Iris/CapBndInvariant.lean` | Iris `[cap_bnd]` named invariant (ghost `Own γ v`, `ExRA`, `AgRA`, `Sep`) |
| `proof/PMT/Iris/LiveMirrorInvariant.lean` | Iris `[live_mirror]` named invariant for liveness tracking |
| `proof/PMT/Iris/GuardInvariant.lean` | Iris `[guard]` named invariant for mmap guard page |
| `proof/PMT/Iris/Composition.lean` | Iris frame rule + composition lemmas across named invariants |
| `proof/PMT/Iris/ArenaRes.lean` | Iris `ArenaRes` resource bundle (`[cap_bnd]` + bump-pointer ghost + cap-own) — `alloc_preserves_arena_res` |
| `proof/PMT/Iris/FractionalPerm.lean` | Iris fractional permission `↦{q}` (q ∈ (0,1]) for partial field ownership |
| `proof/PMT/Iris/WeakestPrecond.lean` | Iris `wp` calculus on PMT instructions (Hoare-triple machinery) |
| `proof/PMT/IVE/Soundness/*.lean` | IVE soundness: `verify_transform_sound`, `verify_state_reads_sound`, `verify_state_writes_sound`, Composition (4 submodules) |
| `proof/PMT/Extraction.lean` | Verified extraction of Rust capacity/field-bounds/linearity/PMT checkers |
| `proof/PMT/ExtractionLemmas.lean` | Lemmas bridging Lean specs to the hand-translated Rust checkers (`src/codegen/src/runtime/pmt_check.rs`, parity-tested) |

Test modules under `proof/PMT/Test/` (9): `ValidProgram`, `EmptyProgram`,
`OverflowProgram`, `UafProgram`, `MultiStepProgram`, `PropertyTests`,
`ArenaBasicSim`, `EdgeCases`, plus the `SorryFreeAudit` script that
re-checks the sorry-free invariant at test time — run via `lake exe test`
/ `make proof-test`. The Rust hand-translations of the verified checkers
live at `src/codegen/src/runtime/pmt_check.rs` (gated behind the
`pmt-runtime-check` Cargo feature, **wired into `arena.rs`** so the
verified `verified_capacity_check` runs in production) and are exercised
by the parity test `tests/pmt_parity_test.rs` (5 tests) plus the
feature-flag wiring test `tests/pmt_feature_flag_test.rs` (3 tests).

The Lean proofs are kept **deliberately decoupled from the Cargo workspace**
(no `build.rs` hook) so the Rust dev loop is not slowed by a 5-60 s `lake build`
on every `cargo check`. Build the proofs explicitly via `make proof` /
`just proof`.

See [docs/caveats.md](docs/caveats.md)
for the Iris specification and [docs/proof/](docs/proof/)
for the verification audit reports.

---

## 6. Caveats

VUMA is at `0.2.0-alpha.1`. Prospective users should read
[docs/caveats.md](docs/caveats.md) (re-audited
row-by-row in -f) before relying on any feature; that file is the
canonical source of truth for all known stubs, partial implementations, and
architectural issues. Highlights to be aware of up-front: (i) **PMT** in this
codebase means "Programs as Memory Transformations," not "Persistent Memory
Transaction"; (ii) the `--safe` CLI flag is now mandatory (always on, -d) — arena-allocated accesses get `__oob_trap` bounds checks, but
raw-pointer and `length_expr=None` accesses remain unchecked; (iii) "no
buffer overflow" is true only for the arena bump pointer, not for arbitrary
pointer arithmetic; (iv) the 4 thin-wrapper backends (`aarch64_be`, `armeb`,
`mips64be`, `ppc64le`) byte-swap around their LE/BE parent backends; (v)
`spawn_worker` on wasm32 emulates `fork(2)` by running parent and child
branches sequentially in a single process — not real isolation (-a
added a one-shot `vuma_log!(warn)` diagnostic); (vi) the canonical-pipeline
`emit_elf` (`src/codegen/src/emit.rs`) is AArch64-only — non-AArch64 `--isa`
values route through the direct AST→codegen bridge, which skips
full IVE gating; use `vuma build` for that.

---

## 7. License

Copyright © 2026 VUMA Project Contributors. Released under the **MIT License**;
see [LICENSE](LICENSE) for the full text.
